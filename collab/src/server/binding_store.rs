use super::binding_slot::{
    BindingControlSet, BindingValue, CommitOutcome, IdentityBindingKey, IdentityBindingSlot,
    MoveNameOutcome, PendingEffect,
};
use super::{load_host_route_records, HostRouteRecord};
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;

pub(crate) const BINDING_CONTROL_FILE: &str = "routes.jsonl";
pub(crate) const BINDING_CONTROL_KIND: &str = "binding_control";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct BindingControlLine {
    kind: String,
    slots: Vec<IdentityBindingSlot>,
    #[serde(default)]
    effects: Vec<PendingEffect>,
}

/// Whether the rename landed and the parent directory sync result is known.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Durability {
    Confirmed,
    Unknown,
}

impl Durability {
    pub(crate) fn require_confirmed_route(self) -> Result<(), String> {
        match self {
            Self::Confirmed => Ok(()),
            Self::Unknown => Err(
                "HOST_ROUTE_DURABILITY_UNKNOWN: published=true; parent directory sync is unconfirmed"
                    .into(),
            ),
        }
    }
}

/// A single typed control edit. Route lines are preserved by the store; the
/// control set is the only ownership fact this resource publishes.
#[derive(Debug, Clone)]
pub(crate) enum ControlChange {
    Commit {
        key: IdentityBindingKey,
        value: BindingValue,
        expected_generation: Option<u64>,
    },
    Unregister {
        key: IdentityBindingKey,
        expected_generation: Option<u64>,
    },
    MoveName {
        target: IdentityBindingKey,
        value: BindingValue,
        source_names: Vec<String>,
        expected_generation: Option<u64>,
    },
    CommitWithEffect {
        key: IdentityBindingKey,
        value: BindingValue,
        expected_generation: Option<u64>,
        effect: PendingEffect,
    },
    MoveNameWithEffect {
        target: IdentityBindingKey,
        value: BindingValue,
        source_names: Vec<String>,
        expected_generation: Option<u64>,
        effect: PendingEffect,
    },
    SetEffect {
        effect: PendingEffect,
    },
    ClearEffect {
        effect_id: String,
    },
    RemoveScope {
        project_scope: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StoreChangeOutcome {
    Slot(CommitOutcome),
    Removed { removed: usize },
    Moved(MoveNameOutcome),
    Effect { changed: bool },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct StoreCommit {
    pub(crate) outcome: StoreChangeOutcome,
    pub(crate) durability: Durability,
}

/// The host control resource is the sole durable binding owner. It uses one
/// atomic whole-file replace discipline shared with the route journal: a crash
/// leaves either the previous valid set or the complete new set, never a
/// partially written identity. No journal is appended and no binding history
/// is replayed from a project journal.
pub(crate) struct BindingControlStore {
    path: std::path::PathBuf,
}

#[cfg(test)]
thread_local! {
    static FAIL_PARENT_SYNC: std::cell::RefCell<Option<std::path::PathBuf>> = const { std::cell::RefCell::new(None) };
}

impl BindingControlStore {
    pub(crate) fn new(state_root: &Path) -> Self {
        Self::at(&state_root.join(BINDING_CONTROL_FILE))
    }

    /// Construct a store over an exact shared route/control file path. This is
    /// used by the route writers that already hold the journal path.
    pub(crate) fn at(path: &Path) -> Self {
        Self {
            path: path.to_path_buf(),
        }
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    /// Test-only injection for the next publication-confirmation sync, including
    /// an unchanged retry. The visible file stays in place.
    #[cfg(test)]
    pub(crate) fn fail_parent_sync_on_next_publish(&self) {
        FAIL_PARENT_SYNC.with(|path| *path.borrow_mut() = Some(self.path.clone()));
    }

    /// Load the current control set directly. A missing file is an empty set.
    /// A corrupt or invalid file fails closed; it never falls back to project
    /// journals or an empty projection that could silently drop identities.
    pub(crate) fn load(&self) -> Result<BindingControlSet, String> {
        self.load_with_presence().map(|(set, _)| set)
    }

    /// Load the control set and report whether the shared route file contains
    /// a control line. Legacy route files have no such line; callers must not
    /// treat the empty set as an active control gate. Routes without a
    /// control line stay lazy and materialize only when selected.
    pub(crate) fn load_with_presence(&self) -> Result<(BindingControlSet, bool), String> {
        let content = match std::fs::read_to_string(&self.path) {
            Ok(content) => content,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok((BindingControlSet::new(), false))
            }
            Err(error) => {
                return Err(format!(
                    "BINDING_CONTROL_READ_FAILED: {}: {error}",
                    self.path.display()
                ))
            }
        };
        if content.is_empty() {
            return Ok((BindingControlSet::new(), false));
        }
        let mut set = None;
        let mut present = false;
        for (index, line) in content.lines().enumerate() {
            let line = line.trim_end_matches('\r');
            if line.trim().is_empty() {
                continue;
            }
            let value: serde_json::Value = serde_json::from_str(line).map_err(|error| {
                format!(
                    "BINDING_CONTROL_DECODE_FAILED: {}: line {}: {error}",
                    self.path.display(),
                    index + 1
                )
            })?;
            if value.get("kind").and_then(serde_json::Value::as_str) != Some(BINDING_CONTROL_KIND) {
                continue;
            }
            present = true;
            let line: BindingControlLine = serde_json::from_value(value).map_err(|error| {
                format!(
                    "BINDING_CONTROL_DECODE_FAILED: {}: line {}: {error}",
                    self.path.display(),
                    index + 1
                )
            })?;
            if line.kind != BINDING_CONTROL_KIND {
                return Err(format!(
                    "BINDING_CONTROL_INVALID: {}: unexpected control kind {}",
                    self.path.display(),
                    line.kind
                ));
            }
            if set.is_some() {
                return Err(format!(
                    "BINDING_CONTROL_INVALID: {}: duplicate binding control line",
                    self.path.display()
                ));
            }
            set = Some(BindingControlSet::from_parts(line.slots, line.effects));
        }
        let set = set.unwrap_or_else(BindingControlSet::new);
        set.validate().map_err(|error| {
            format!("BINDING_CONTROL_INVALID: {}: {error}", self.path.display())
        })?;
        Ok((set, present))
    }

    /// Load both the route records and the control set from the shared file.
    /// This is the single read used by every shared-resource writer.
    pub(crate) fn load_shared(&self) -> Result<(Vec<HostRouteRecord>, BindingControlSet), String> {
        let routes = load_host_route_records(&self.path)?;
        let control = self.load()?;
        Ok((routes, control))
    }

    /// Apply one typed control change and atomically publish the shared
    /// resource. Unchanged facts never rewrite the file. A post-rename parent
    /// sync failure returns Durability::Unknown without rolling back the new
    /// file that is already visible.
    pub(crate) fn commit(
        &self,
        change: ControlChange,
    ) -> Result<StoreCommit, String> {
        let (routes, mut control) = self.load_shared()?;
        let mut effect_changed = false;
        let outcome = match change {
            ControlChange::Commit {
                key,
                value,
                expected_generation,
            } => StoreChangeOutcome::Slot(
                control
                    .commit(&key, value, expected_generation)
                    .map_err(|error| {
                        format!(
                            "BINDING_CONTROL_COMMIT_REJECTED: {}: {error}",
                            self.path.display()
                        )
                    })?,
            ),
            ControlChange::CommitWithEffect {
                key,
                value,
                expected_generation,
                effect,
            } => {
                let slot = control
                    .commit(&key, value, expected_generation)
                    .map_err(|error| {
                        format!(
                            "BINDING_CONTROL_COMMIT_REJECTED: {}: {error}",
                            self.path.display()
                        )
                    })?;
                effect_changed = control.set_effect(effect).map_err(|error| {
                    format!(
                        "BINDING_CONTROL_COMMIT_REJECTED: {}: {error}",
                        self.path.display()
                    )
                })?;
                StoreChangeOutcome::Slot(slot)
            }
            ControlChange::Unregister {
                key,
                expected_generation,
            } => {
                // A legacy deployment can retire an identity that was never
                // converted into control data. Retiring it is already
                // satisfied, so no empty slot is invented and no new control
                // line is written.
                let outcome = if control.slot(&key).is_none() {
                    CommitOutcome::Unchanged { generation: 0 }
                } else {
                    control.unregister(&key, expected_generation).map_err(|error| {
                        format!(
                            "BINDING_CONTROL_COMMIT_REJECTED: {}: {error}",
                            self.path.display()
                        )
                    })?
                };
                StoreChangeOutcome::Slot(outcome)
            }
            ControlChange::MoveName {
                target,
                value,
                source_names,
                expected_generation,
            } => StoreChangeOutcome::Moved(
                control
                    .move_name(&target, value, &source_names, expected_generation)
                    .map_err(|error| {
                        format!(
                            "BINDING_CONTROL_COMMIT_REJECTED: {}: {error}",
                            self.path.display()
                        )
                    })?,
            ),
            ControlChange::MoveNameWithEffect {
                target,
                value,
                source_names,
                expected_generation,
                effect,
            } => {
                let moved = control
                    .move_name(&target, value, &source_names, expected_generation)
                    .map_err(|error| {
                        format!(
                            "BINDING_CONTROL_COMMIT_REJECTED: {}: {error}",
                            self.path.display()
                        )
                    })?;
                effect_changed = control.set_effect(effect).map_err(|error| {
                    format!(
                        "BINDING_CONTROL_COMMIT_REJECTED: {}: {error}",
                        self.path.display()
                    )
                })?;
                StoreChangeOutcome::Moved(moved)
            }
            ControlChange::SetEffect { effect } => {
                let changed = control.set_effect(effect).map_err(|error| {
                    format!(
                        "BINDING_CONTROL_COMMIT_REJECTED: {}: {error}",
                        self.path.display()
                    )
                })?;
                StoreChangeOutcome::Effect { changed }
            }
            ControlChange::ClearEffect { effect_id } => {
                let changed = control.clear_effect(&effect_id);
                StoreChangeOutcome::Effect { changed }
            }
            ControlChange::RemoveScope { project_scope } => {
                StoreChangeOutcome::Removed {
                    removed: control.remove_scope(project_scope.as_str()),
                }
            }
        };
        let durability = if effect_changed {
            self.publish_shared(&routes, &control)?
        } else {
            match outcome {
            StoreChangeOutcome::Slot(CommitOutcome::Unchanged { .. })
            | StoreChangeOutcome::Removed { removed: 0 } => {
                // Stores are reconstructed between requests. Matching bytes
                // do not prove that an earlier rename was directory-synced.
                // Confirm that publication without rewriting the resource or
                // rotating its generation. A truly absent resource has no
                // publication to confirm (legacy unregister/reset no-op).
                match self.path.try_exists() {
                    Ok(true) => self.confirm_publication(),
                    Ok(false) => Durability::Confirmed,
                    Err(error) => {
                        return Err(format!(
                            "BINDING_CONTROL_READ_FAILED: {}: {error}",
                            self.path.display()
                        ))
                    }
                }
            }
            StoreChangeOutcome::Effect { changed: false } => {
                match self.path.try_exists() {
                    Ok(true) => self.confirm_publication(),
                    Ok(false) => Durability::Confirmed,
                    Err(error) => {
                        return Err(format!(
                            "BINDING_CONTROL_READ_FAILED: {}: {error}",
                            self.path.display()
                        ))
                    }
                }
            }
            StoreChangeOutcome::Moved(MoveNameOutcome {
                target: CommitOutcome::Unchanged { .. },
                sources_retired: 0,
            }) => {
                // Stores are reconstructed between requests. Matching bytes
                // do not prove that an earlier rename was directory-synced.
                // Confirm that publication without rewriting the resource or
                // rotating its generation. A truly absent resource has no
                // publication to confirm (legacy unregister/reset no-op).
                match self.path.try_exists() {
                    Ok(true) => self.confirm_publication(),
                    Ok(false) => Durability::Confirmed,
                    Err(error) => {
                        return Err(format!(
                            "BINDING_CONTROL_READ_FAILED: {}: {error}",
                            self.path.display()
                        ))
                    }
                }
            }
            _ => self.publish_shared(&routes, &control)?,
            }
        };
        Ok(StoreCommit {
            outcome,
            durability,
        })
    }

    /// Persist one complete control set while preserving the route records.
    /// Exposed for tests and legacy callers; production uses `commit`.
    pub(crate) fn save(&self, set: &BindingControlSet) -> Result<Durability, String> {
        let (routes, _) = self.load_shared()?;
        self.publish_shared(&routes, set)
    }

    /// Serialize one complete shared resource and publish it atomically. This
    /// is the only write path for the combined route + binding_control file.
    pub(crate) fn publish_shared(
        &self,
        routes: &[HostRouteRecord],
        set: &BindingControlSet,
    ) -> Result<Durability, String> {
        set.validate().map_err(|error| {
            format!(
                "BINDING_CONTROL_SAVE_REJECTED: {}: {error}",
                self.path.display()
            )
        })?;
        let parent = self.path.parent().ok_or_else(|| {
            format!(
                "BINDING_CONTROL_SAVE_FAILED: {} has no parent",
                self.path.display()
            )
        })?;
        std::fs::create_dir_all(parent).map_err(|error| {
            format!(
                "BINDING_CONTROL_SAVE_FAILED: create parent {}: {error}",
                parent.display()
            )
        })?;

        let mut body = Vec::new();
        for route in routes {
            serde_json::to_writer(&mut body, route).map_err(|error| {
                format!(
                    "BINDING_CONTROL_SAVE_FAILED: serialize route {}: {error}",
                    self.path.display()
                )
            })?;
            body.push(b'\n');
        }
        // Publish a control line when the set has any slot or pending effect.
        // A completely empty set is byte-equivalent to a legacy route-only
        // file, so route writers must not silently upgrade it into "has a
        // control line".
        if set.slots().next().is_some() || set.pending_effects().next().is_some() {
            let control = BindingControlLine {
                kind: BINDING_CONTROL_KIND.to_owned(),
                slots: set.slots().cloned().collect(),
                effects: set.pending_effects().cloned().collect(),
            };
            let mut line = serde_json::to_vec(&control).map_err(|error| {
                format!(
                    "BINDING_CONTROL_SAVE_FAILED: serialize control set: {error}"
                )
            })?;
            line.push(b'\n');
            body.extend_from_slice(&line);
        }

        static TEMP_SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let sequence = TEMP_SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let temp_name = format!(
            "{}.tmp-{}-{}",
            self.path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or(BINDING_CONTROL_FILE),
            std::process::id(),
            sequence
        );
        let temp = self.path.with_file_name(temp_name);

        let mut temp_file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temp)
            .map_err(|error| {
                format!(
                    "BINDING_CONTROL_SAVE_FAILED: create temp {}: {error}",
                    temp.display()
                )
            })?;
        use std::os::unix::fs::PermissionsExt;
        if let Err(error) = temp_file.set_permissions(std::fs::Permissions::from_mode(0o600)) {
            let _ = std::fs::remove_file(&temp);
            return Err(format!(
                "BINDING_CONTROL_SAVE_FAILED: set temp permissions {}: {error}",
                temp.display()
            ));
        }
        if let Err(error) = temp_file.write_all(&body) {
            let _ = std::fs::remove_file(&temp);
            return Err(format!(
                "BINDING_CONTROL_SAVE_FAILED: write temp {}: {error}",
                temp.display()
            ));
        }
        if let Err(error) = temp_file.sync_data() {
            let _ = std::fs::remove_file(&temp);
            return Err(format!(
                "BINDING_CONTROL_SAVE_FAILED: flush temp {}: {error}",
                temp.display()
            ));
        }
        drop(temp_file);
        if let Err(error) = std::fs::rename(&temp, &self.path) {
            let _ = std::fs::remove_file(&temp);
            return Err(format!(
                "BINDING_CONTROL_SAVE_FAILED: publish {}: {error}",
                self.path.display()
            ));
        }
        Ok(self.confirm_publication())
    }

    fn confirm_publication(&self) -> Durability {
        #[cfg(test)]
        if FAIL_PARENT_SYNC.with(|path| {
            let mut path = path.borrow_mut();
            if path.as_ref() == Some(&self.path) {
                path.take();
                true
            } else {
                false
            }
        }) {
            return Durability::Unknown;
        }
        if sync_parent_dir(&self.path).is_err() {
            return Durability::Unknown;
        }
        Durability::Confirmed
    }
}

fn sync_parent_dir(path: &Path) -> std::io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| std::io::Error::other("path has no parent"))?;
    let directory = std::fs::File::open(parent)?;
    directory.sync_all()
}
