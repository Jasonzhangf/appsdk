use super::binding_slot::{
    BindingControlSet, BindingValue, CommitOutcome, IdentityBindingKey, IdentityBindingSlot,
    MoveNameOutcome, PendingEffect,
};
use super::HostRouteRecord;
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;

pub(crate) const BINDING_CONTROL_FILE: &str = "identity-map.json";

/// Routes locate storage and communication only. They carry no identity map.
pub(crate) fn publish_route_records(
    path: &Path,
    routes: &[HostRouteRecord],
) -> Result<Durability, String> {
    let body =
        serde_json::to_vec(routes).map_err(|error| format!("HOST_ROUTE_ENCODE_FAILED: {error}"))?;
    BindingControlStore::at(path).publish_bytes(&body)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct IdentityMapFile {
    schema_version: u32,
    slots: Vec<IdentityBindingSlot>,
    #[serde(default)]
    effects: Vec<PendingEffect>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pending_resets: Vec<String>,
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

/// A single typed edit to the sole identity control resource.
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
    UnregisterWithEffect {
        key: IdentityBindingKey,
        effect: PendingEffect,
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
        companion: Option<(IdentityBindingKey, BindingValue, PendingEffect)>,
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
/// atomic whole-file replace discipline: a crash
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

    /// Exact path constructor for isolated fixtures and the atomic writer.
    pub(crate) fn at(path: &Path) -> Self {
        Self {
            path: path.to_path_buf(),
        }
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    /// Retire identity eligibility and retain the exact reset scope before
    /// the existing reset owner clears any business state.
    pub(crate) fn begin_reset(&self, scope: &str) -> Result<(), String> {
        let mut map = self.load()?;
        map.remove_scope_except_dsh(scope);
        if !map.pending_resets.iter().any(|pending| pending == scope) {
            map.pending_resets.push(scope.to_owned());
        }
        self.save(&map)?.require_confirmed_route()
    }

    pub(crate) fn complete_reset(&self, scope: &str) -> Result<(), String> {
        let mut map = self.load()?;
        map.pending_resets.retain(|pending| pending != scope);
        self.save(&map)?.require_confirmed_route()
    }

    /// The caller holds the manager's registration gate and has authenticated
    /// an exact lifecycle target. Business cleanup cannot retire its successor.
    pub(crate) fn retire_binding(
        &self,
        binding: &super::global_state::RuntimeBinding,
    ) -> Result<(), String> {
        let map = self.load()?;
        let Some(key) = map.key_for_worker(
            &binding.project_scope,
            &binding.app_scope_id,
            binding.agent_id.as_str(),
        )?
        else {
            return Ok(());
        };
        let slot = map.slot(key).expect("active key");
        if slot.active_binding().map(|value| &value.binding) != Some(binding) {
            return Err(
                "RETIRE_TARGET_CHANGED: current mapping differs from the exact lifecycle target"
                    .into(),
            );
        }
        self.commit(ControlChange::Unregister {
            key: key.clone(),
            expected_generation: Some(slot.generation),
        })?
        .durability
        .require_confirmed_route()
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

    pub(crate) fn load_with_presence(&self) -> Result<(BindingControlSet, bool), String> {
        let bytes = match std::fs::read(&self.path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok((BindingControlSet::new(), false))
            }
            Err(error) => {
                return Err(format!(
                    "IDENTITY_MAP_READ_FAILED: {}: {error}",
                    self.path.display()
                ))
            }
        };
        let file: IdentityMapFile = serde_json::from_slice(&bytes).map_err(|error| {
            format!(
                "IDENTITY_MAP_DECODE_FAILED: {}: {error}",
                self.path.display()
            )
        })?;
        if file.schema_version != 1 {
            return Err(format!(
                "IDENTITY_MAP_VERSION_UNSUPPORTED: {}: {}",
                self.path.display(),
                file.schema_version
            ));
        }
        let mut control = BindingControlSet::from_parts(file.slots, file.effects);
        control.pending_resets = file.pending_resets;
        control
            .validate()
            .map_err(|error| format!("IDENTITY_MAP_INVALID: {}: {error}", self.path.display()))?;
        Ok((control, true))
    }

    /// Apply one typed control change and atomically publish the shared
    /// resource. Unchanged facts never rewrite the file. A post-rename parent
    /// sync failure returns Durability::Unknown without rolling back the new
    /// file that is already visible.
    pub(crate) fn commit(&self, change: ControlChange) -> Result<StoreCommit, String> {
        let mut control = self.load()?;
        let control_before = control.clone();
        let mut effect_changed = false;
        let outcome = match change {
            ControlChange::Commit {
                key,
                value,
                expected_generation,
            } => {
                StoreChangeOutcome::Slot(control.commit(&key, value, expected_generation).map_err(
                    |error| {
                        format!(
                            "BINDING_CONTROL_COMMIT_REJECTED: {}: {error}",
                            self.path.display()
                        )
                    },
                )?)
            }
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
            ControlChange::UnregisterWithEffect { key, effect } => {
                let outcome = control
                    .unregister(&key, None)
                    .map_err(|error| error.to_string())?;
                effect_changed = control
                    .set_effect(effect)
                    .map_err(|error| error.to_string())?;
                StoreChangeOutcome::Slot(outcome)
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
                    control
                        .unregister(&key, expected_generation)
                        .map_err(|error| {
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
                companion,
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
                if let Some((key, value, effect)) = companion {
                    let changed = control
                        .commit(&key, value, None)
                        .map_err(|error| error.to_string())?;
                    effect_changed |= matches!(changed, CommitOutcome::Committed { .. });
                    effect_changed |= control
                        .set_effect(effect)
                        .map_err(|error| error.to_string())?;
                }
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
            ControlChange::RemoveScope { project_scope } => StoreChangeOutcome::Removed {
                removed: control.remove_scope(project_scope.as_str()),
            },
        };
        let collection_changed = control_before != control;
        let durability = if effect_changed || collection_changed {
            self.save(&control)?
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
                StoreChangeOutcome::Effect { changed: false } => match self.path.try_exists() {
                    Ok(true) => self.confirm_publication(),
                    Ok(false) => Durability::Confirmed,
                    Err(error) => {
                        return Err(format!(
                            "BINDING_CONTROL_READ_FAILED: {}: {error}",
                            self.path.display()
                        ))
                    }
                },
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
                _ => self.save(&control)?,
            }
        };
        Ok(StoreCommit {
            outcome,
            durability,
        })
    }

    /// Persist the complete map with one atomic replacement.
    pub(crate) fn save(&self, set: &BindingControlSet) -> Result<Durability, String> {
        set.validate().map_err(|error| {
            format!(
                "BINDING_CONTROL_SAVE_REJECTED: {}: {error}",
                self.path.display()
            )
        })?;
        let file = IdentityMapFile {
            schema_version: 1,
            slots: set.slots().cloned().collect(),
            effects: set.pending_effects().cloned().collect(),
            pending_resets: set.pending_resets.clone(),
        };
        let body = serde_json::to_vec_pretty(&file).map_err(|error| {
            format!(
                "IDENTITY_MAP_ENCODE_FAILED: {}: {error}",
                self.path.display()
            )
        })?;
        self.publish_bytes(&body)
    }

    fn publish_bytes(&self, body: &[u8]) -> Result<Durability, String> {
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
        if let Err(error) = temp_file.write_all(body) {
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
