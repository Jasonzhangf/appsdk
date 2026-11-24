//! Authorized retirement of a legacy project-local Collab control plane.
//!
//! This is the single offline reset owner. It never claims delivery, review,
//! install, or communication evidence. The operation is only reachable
//! with an explicit operator approval string, requires the host daemon to be
//! down, removes the project-local control plane
//! plus its stale host route record, and rebuilds the current empty baseline.

use crate::scope::{self, HostPaths, Scope};
use anyhow::Context;
use serde_json::json;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// Legacy project-local control roots owned by Collab.
///
/// `.appsdk-control` belongs to the AppSDK governance transaction and is
/// deliberately not touched here. Business source, runtime data, `active/`,
/// and `protected/` are never listed.
const LEGACY_CONTROL_ROOTS: [&str; 2] = [".agent-collab", ".agent-collab-v2"];

/// Which accumulated burden this run retires.
///
/// The two levels share one pipeline. The level selects the inventory and the
/// retire set; it never selects a different transaction shape.
///
/// Duplicate route claimants need no level here. One tmux pane owns exactly one
/// binding host-wide, and the route reducer enforces that on every write and on
/// every replay, so there is no ambiguous pane for an operator to resolve.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResetLevel {
    /// Rebuild this project's runtime baseline.
    Project,
    /// Rebuild the host control plane.
    Host,
}

impl ResetLevel {
    /// Resolve the level from the two mutually exclusive selectors.
    ///
    /// Exactly one selector is required. A run with none or with two fails
    /// before any control file is read or written.
    pub fn select(project: bool, host: bool) -> anyhow::Result<Self> {
        match (project, host) {
            (true, false) => Ok(Self::Project),
            (false, true) => Ok(Self::Host),
            _ => anyhow::bail!(
                "RESET_LEVEL_REQUIRED: collab reset needs exactly one of --project or --host"
            ),
        }
    }
}

pub struct ResetRequest {
    pub approval: String,
    pub discard_legacy: bool,
    pub level: ResetLevel,
    /// The live host index root. Required for `Host`.
    pub storage_root: Option<PathBuf>,
    /// `Host`: also remove `~/.collab/runs/`.
    pub include_runs: bool,
}

impl Default for ResetRequest {
    /// The project-level shape with no operator authorization.
    ///
    /// This exists so a caller that only needs the project baseline states the
    /// fields it cares about. It is never a valid request on its own: the
    /// approval text and `--discard-legacy` are still required by `run`.
    fn default() -> Self {
        Self {
            approval: String::new(),
            discard_legacy: false,
            level: ResetLevel::Project,
            storage_root: None,
            include_runs: false,
        }
    }
}

impl ResetRequest {
    /// Reject flags that do not belong to the selected level.
    ///
    /// A silently ignored flag is a silent no-op on a destructive operation, so
    /// a flag that the level does not use is an error rather than a no-op.
    fn validate_level_flags(&self) -> anyhow::Result<()> {
        match self.level {
            ResetLevel::Project => {
                if self.storage_root.is_some() {
                    anyhow::bail!(
                        "RESET_LEVEL_FLAG_MISMATCH: --storage-root belongs to --host, not \
                         --project"
                    );
                }
                if self.include_runs {
                    anyhow::bail!(
                        "RESET_LEVEL_FLAG_MISMATCH: --include-runs belongs to --host, not \
                         --project"
                    );
                }
            }
            ResetLevel::Host => {
                if self.storage_root.is_none() {
                    anyhow::bail!(
                        "RESET_STORAGE_ROOT_REQUIRED: --host must name the live host index \
                         with --storage-root <path>; the index is \
                         <storage-root>/.agent-collab/server/state.json and routes.json \
                         does not identify it"
                    );
                }
            }
        }
        Ok(())
    }
}

struct RetiredRoot {
    relative: String,
    absolute: PathBuf,
    staged: Option<PathBuf>,
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or(0)
}

fn reject_unsafe_control_roots(root: &Path) -> anyhow::Result<()> {
    for relative in LEGACY_CONTROL_ROOTS {
        let path = root.join(relative);
        match std::fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                anyhow::bail!(
                    "RESET_CONTROL_ROOT_SYMLINK_REJECTED: {} is a symlink; reset refuses \
                     to remove or write outside the project root",
                    path.display()
                );
            }
            Ok(metadata) if !metadata.is_dir() => {
                anyhow::bail!(
                    "RESET_CONTROL_ROOT_INVALID: {} is not a directory",
                    path.display()
                );
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

fn reject_unsafe_control_path(path: &Path) -> anyhow::Result<()> {
    let metadata = std::fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() {
        anyhow::bail!(
            "RESET_CONTROL_TREE_SYMLINK_REJECTED: {} is a symlink; reset refuses to remove it",
            path.display()
        );
    }
    if metadata.is_dir() {
        for entry in std::fs::read_dir(path)? {
            reject_unsafe_control_path(&entry?.path())?;
        }
    }
    Ok(())
}

struct RouteSnapshot {
    path: PathBuf,
    original: Option<Vec<u8>>,
}

fn snapshot_file(path: PathBuf) -> anyhow::Result<RouteSnapshot> {
    let original = match std::fs::read(&path) {
        Ok(content) => Some(content),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(error.into()),
    };
    Ok(RouteSnapshot { path, original })
}

fn reject_symlinked_guidance(root: &Path) -> anyhow::Result<()> {
    for candidate in [root.join("docs"), root.join("docs/collab.md")] {
        match std::fs::symlink_metadata(&candidate) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                anyhow::bail!(
                    "RESET_DOC_SYMLINK_REJECTED: {} is a symlink; reset refuses to write \
                     outside the project-owned Collab guidance path",
                    candidate.display()
                );
            }
            Ok(_) | Err(_) => {}
        }
    }
    Ok(())
}

fn restore_file(snapshot: &RouteSnapshot) -> anyhow::Result<()> {
    match &snapshot.original {
        Some(content) => {
            use std::io::Write;
            let tmp = snapshot
                .path
                .with_file_name(format!(".reset-restore-{}", std::process::id()));
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&tmp)?;
            file.write_all(content)?;
            file.sync_data()?;
            drop(file);
            std::fs::rename(&tmp, &snapshot.path)?;
            if let Some(parent) = snapshot.path.parent() {
                std::fs::File::open(parent)?.sync_all()?;
            }
        }
        None => match std::fs::remove_file(&snapshot.path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        },
    }
    Ok(())
}

fn stage_retired_roots(retired: &mut [RetiredRoot], run_id: &str) -> anyhow::Result<()> {
    for entry in retired.iter_mut() {
        let staged = entry
            .absolute
            .with_file_name(format!("{}.reset-stage-{run_id}", entry.relative));
        std::fs::rename(&entry.absolute, &staged)?;
        entry.staged = Some(staged);
    }
    Ok(())
}

fn rollback_retired_roots(retired: &mut [RetiredRoot]) -> anyhow::Result<()> {
    let mut errors = Vec::new();
    for entry in retired.iter_mut().rev() {
        let Some(staged) = entry.staged.take() else {
            continue;
        };
        if entry.absolute.is_dir() {
            if let Err(error) = std::fs::remove_dir_all(&entry.absolute) {
                errors.push(format!(
                    "remove replacement {}: {error}",
                    entry.absolute.display()
                ));
                continue;
            }
        }
        if let Err(error) = std::fs::rename(&staged, &entry.absolute) {
            errors.push(format!(
                "{} -> {}: {error}",
                staged.display(),
                entry.absolute.display()
            ));
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        anyhow::bail!("{}", errors.join("; "))
    }
}

fn discard_staged_roots(retired: &[RetiredRoot]) -> anyhow::Result<()> {
    for entry in retired {
        if let Some(staged) = &entry.staged {
            // A retire entry may be a single control file (routes.json) or a
            // directory (identities/), so removal must not assume a directory.
            if staged.is_dir() {
                std::fs::remove_dir_all(staged)?;
            } else {
                std::fs::remove_file(staged)?;
            }
        }
    }
    Ok(())
}

/// Replace the current host route table without the retired project's records.
///
/// The route table is host-wide durable state, so this is an atomic
/// replacement: a crash leaves either the previous valid table or the complete
/// new table. Unrelated routes are preserved byte-for-byte.
fn retire_host_routes(host_paths: &HostPaths, root: &Path) -> anyhow::Result<usize> {
    let root = std::fs::canonicalize(root)?;
    rewrite_host_routes(host_paths, |record| {
        let matches_project = record
            .get("canonical_root")
            .and_then(|value| value.as_str())
            .and_then(|value| std::fs::canonicalize(value).ok())
            .is_some_and(|value| value == root);
        matches_project.then_some(RouteDisposition::Retire)
    })
}

enum RouteDisposition {
    Keep,
    Retire,
}

/// Remove host routes whose canonical project root can no longer be used.
///
/// A route is provably stale only when its canonical root does not exist, or
/// when the root exists but no longer has the `.agent-collab` initialization
/// marker. Every other record is retained byte-for-byte. This is the
/// "ignore legacy residue" half of reset: stale routes must not keep a daemon
/// from replaying the current baseline.
fn prune_stale_host_routes(host_paths: &HostPaths) -> anyhow::Result<usize> {
    rewrite_host_routes(host_paths, |record| {
        let Some(canonical_root) = record
            .get("canonical_root")
            .and_then(|value| value.as_str())
        else {
            return Some(RouteDisposition::Retire);
        };
        match std::fs::canonicalize(canonical_root) {
            Ok(root) if root.join(".agent-collab").is_dir() => Some(RouteDisposition::Keep),
            Ok(_) => Some(RouteDisposition::Retire),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                Some(RouteDisposition::Retire)
            }
            // Permission and I/O errors are unknown, not stale. Keep the
            // record so a transient mount failure cannot erase a valid route.
            Err(_) => Some(RouteDisposition::Keep),
        }
    })
}

fn rewrite_host_routes<F>(host_paths: &HostPaths, mut classify: F) -> anyhow::Result<usize>
where
    F: FnMut(&serde_json::Value) -> Option<RouteDisposition>,
{
    let path = host_paths.state_root().join("routes.json");
    let content = match std::fs::read(&path) {
        Ok(content) => content,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(error) => return Err(error.into()),
    };
    let records: Vec<serde_json::Value> = serde_json::from_slice(&content)?;
    let mut kept = Vec::new();
    let mut removed = 0usize;
    for record in records {
        match classify(&record) {
            Some(RouteDisposition::Retire) => {
                removed += 1;
                continue;
            }
            Some(RouteDisposition::Keep) | None => {}
        }
        kept.push(record);
    }
    if removed == 0 {
        return Ok(0);
    }
    use std::io::Write;
    let tmp = path.with_file_name(format!("routes.json.reset-{}", std::process::id()));
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&tmp)?;
    file.write_all(&serde_json::to_vec(&kept)?)?;
    file.sync_data()?;
    drop(file);
    std::fs::rename(&tmp, &path)?;
    std::fs::File::open(host_paths.state_root())?.sync_all()?;
    Ok(removed)
}

/// The single reset entry. It authorizes the run, resolves the level, and
/// dispatches to the level body.
///
/// The three levels share this authorization and the same transaction shape.
/// Only the inventory and the retire set differ.
pub fn run(scope: &Scope, host_paths: &HostPaths, request: ResetRequest) -> anyhow::Result<()> {
    if !request.discard_legacy {
        anyhow::bail!("RESET_AUTHORIZATION_REQUIRED: collab reset requires --discard-legacy");
    }
    if request.approval.trim().is_empty() {
        anyhow::bail!(
            "RESET_AUTHORIZATION_REQUIRED: collab reset requires a non-empty --approval \
             naming the operator authorization"
        );
    }
    request.validate_level_flags()?;
    match request.level {
        ResetLevel::Project => run_project(scope, host_paths, &request),
        ResetLevel::Host => run_host(scope, host_paths, &request),
    }
}

/// The canonical project root whose `.agent-collab/` is the live route index.
///
/// The host daemon records the root it was started from in `service.json`. That
/// root's current state belongs to the resident daemon, so the project level
/// must not retire it: the host level is the operation that owns it.
///
/// `None` means only "no descriptor, so no root holds the live index": a fresh
/// host with no `service.json` has no resident index and the project level is
/// free to run. A descriptor that exists but cannot be read, parsed, or lacks a
/// resolvable `service_scope_root` is an error, not `None`. Collapsing those to
/// `None` would fail open and let the project level retire the daemon's own
/// index root.
fn resident_index_root(host_paths: &HostPaths) -> anyhow::Result<Option<PathBuf>> {
    let path = host_paths.state_root().join("service.json");
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(anyhow::anyhow!(
                "RESET_INDEX_ROOT_UNRESOLVED: {} cannot be read: {error}; the project level \
                 cannot prove that {} is not the live index",
                path.display(),
                path.display()
            ))
        }
    };
    let value = match serde_json::from_str::<serde_json::Value>(&text) {
        Ok(value) => value,
        Err(error) => {
            return Err(anyhow::anyhow!(
                "RESET_INDEX_ROOT_UNRESOLVED: {} is not valid JSON ({error}); the project level \
                 cannot prove that {} is not the live index",
                path.display(),
                path.display()
            ))
        }
    };
    let Some(recorded) = value
        .get("service_scope_root")
        .and_then(|field| field.as_str())
    else {
        return Err(anyhow::anyhow!(
            "RESET_INDEX_ROOT_UNRESOLVED: {} has no service_scope_root; the project level \
             cannot prove that {} is not the live index",
            path.display(),
            path.display()
        ));
    };
    let canonical = match std::fs::canonicalize(recorded) {
        Ok(root) => root,
        Err(error) => {
            return Err(anyhow::anyhow!(
                "RESET_INDEX_ROOT_UNRESOLVED: service_scope_root {} in {} cannot be resolved: \
                 {error}",
                recorded,
                path.display()
            ))
        }
    };
    Ok(Some(canonical))
}

fn run_project(
    scope: &Scope,
    host_paths: &HostPaths,
    request: &ResetRequest,
) -> anyhow::Result<()> {
    let root = std::fs::canonicalize(&scope.root)?;

    // Prove exclusivity before touching any control plane. Keep both the
    // current host lock and every lock understood by a pre-host-endpoint
    // daemon alive through the complete transaction.
    let _lock =
        crate::server::acquire_reset_lock(&host_paths.lock_path(), &host_paths.socket_path())?;
    let _legacy_writer_fence =
        crate::server::acquire_legacy_writer_fence(&Scope { root: root.clone() }, host_paths)?;
    if crate::client::alive(&host_paths.socket_path()) {
        anyhow::bail!(
            "RESET_DAEMON_LIVE: offline maintenance is unavailable while the user-managed service is running at {}",
            host_paths.socket_path().display()
        );
    }
    // The project level may be run from the daemon's own storage root. In that
    // case the project retire set below already covers the resident index under
    // `.agent-collab/server/`, so the run retires it together with the business
    // payload and records that it did so. The descriptor is still read, and an
    // unreadable one still fails closed, because that read is what proves which
    // root holds the index.
    let held_resident_index = resident_index_root(host_paths)?.is_some_and(|owner| owner == root);

    let run_id = format!("reset-{}-{}", now_ms(), std::process::id());
    reject_unsafe_control_roots(&root)?;

    let mut retired = Vec::new();
    for relative in LEGACY_CONTROL_ROOTS {
        let absolute = root.join(relative);
        match std::fs::symlink_metadata(&absolute) {
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(error).with_context(|| {
                    format!(
                        "RESET_CONTROL_ROOT_INSPECTION_FAILED: cannot inspect {}",
                        absolute.display()
                    )
                });
            }
        }
        // The current scaffold also creates `.agent-collab/`. An empty
        // current baseline has no durable reducer state and is not a legacy
        // control plane, so a repeated reset must be idempotent instead of
        // archiving and rebuilding that scaffold forever.
        if relative == ".agent-collab" && is_current_empty_baseline(&absolute) {
            continue;
        }
        reject_unsafe_control_path(&absolute)?;
        retired.push(RetiredRoot {
            relative: relative.to_string(),
            absolute,
            staged: None,
        });
    }

    let already_reset = retired.is_empty();

    let routes_path = host_paths.state_root().join("routes.json");
    let route_snapshot = snapshot_file(routes_path)?;
    reject_symlinked_guidance(&root)?;
    let docs_dir = root.join("docs");
    let docs_dir_existed = docs_dir.is_dir();
    let collab_doc_snapshot = snapshot_file(docs_dir.join("collab.md"))?;
    let baseline_existed = root.join(".agent-collab").exists();
    let mut removed_routes = 0usize;
    let mut removed_stale_routes = 0usize;
    let mut record = serde_json::Value::Null;
    let transaction = (|| -> anyhow::Result<()> {
        if !already_reset {
            stage_retired_roots(&mut retired, &run_id)?;
        }

        // Rebuild only the Collab-owned current empty baseline. The full init
        // path also edits project MCP/editor settings and global AppSDK
        // configuration; reset must not mutate those unrelated owners.
        scope::init_collab_baseline(&root)?;
        if !root.join(".agent-collab").is_dir() {
            anyhow::bail!(
                "RESET_BASELINE_INVALID: {} was not created by the current scaffold",
                root.join(".agent-collab").display()
            );
        }

        removed_routes = retire_host_routes(host_paths, &root)?;
        removed_stale_routes = prune_stale_host_routes(host_paths)?;
        // A reset must not retain an older project-local guidance file while
        // claiming that the current scaffold was rebuilt.
        std::fs::create_dir_all(root.join("docs"))?;
        std::fs::write(root.join("docs/collab.md"), scope::COLLAB_DOC)?;
        record = json!({
            "schema": "collab-reset/v1",
            "run_id": run_id,
            "at_ms": now_ms(),
            "project_root": root,
            "approval": request.approval,
            "already_reset": already_reset,
            "held_resident_index": held_resident_index,
            "archive": if already_reset { "absent" } else { "removed" },
            "removed_host_routes": removed_routes,
            "removed_stale_host_routes": removed_stale_routes,
            "retired": retired
                .iter()
                .map(|entry| json!(entry.relative))
                .collect::<Vec<_>>(),
            "delivery_verified": false,
            "next": "run collab up, then collab init from this exact project root",
        });
        discard_staged_roots(&retired)?;
        Ok(())
    })();

    if let Err(error) = transaction {
        let rollback_roots = rollback_retired_roots(&mut retired);
        let rollback_routes = restore_file(&route_snapshot);
        let rollback_doc = restore_file(&collab_doc_snapshot);
        let rollback_baseline: anyhow::Result<()> = if baseline_existed {
            Ok(())
        } else {
            std::fs::remove_dir_all(root.join(".agent-collab"))
                .or_else(|error| {
                    if error.kind() == std::io::ErrorKind::NotFound {
                        Ok(())
                    } else {
                        Err(error)
                    }
                })
                .map_err(anyhow::Error::from)
        };
        let rollback_docs_dir: anyhow::Result<()> = if docs_dir_existed {
            Ok(())
        } else {
            std::fs::remove_dir(&docs_dir)
                .or_else(|error| {
                    if error.kind() == std::io::ErrorKind::NotFound
                        || error.kind() == std::io::ErrorKind::DirectoryNotEmpty
                    {
                        Ok(())
                    } else {
                        Err(error)
                    }
                })
                .map_err(anyhow::Error::from)
        };
        if let Err(rollback_error) = rollback_roots
            .and(rollback_routes)
            .and(rollback_doc)
            .and(rollback_baseline)
            .and(rollback_docs_dir)
        {
            anyhow::bail!("RESET_INCOMPLETE: {error}; rollback also failed: {rollback_error}");
        }
        return Err(error.context("reset rolled back; legacy control plane restored"));
    }

    println!("{}", serde_json::to_string_pretty(&record)?);
    Ok(())
}

/// Resolve and verify the root that owns the live host index.
///
/// `routes.json` does not identify the index root, so the operator must name
/// it. The current state inside it must exist, and non-empty state must hold a
/// record for that root, so a typo cannot retarget the operation at another
/// project's control plane.
///
/// Returns the canonical storage root, the current state when readable, and
/// the original current-state error when the descriptor proves ownership.
///
/// An unreadable current file does not abort the run unconditionally. It cannot prove
/// ownership, so the daemon's own `service_scope_root` descriptor must prove
/// that this root is the index root instead. When the descriptor names a
/// different root, the original state error propagates unchanged and nothing
/// is retired.
fn resolve_index_root(
    request: &ResetRequest,
    level: &str,
    host_paths: &HostPaths,
) -> anyhow::Result<(PathBuf, Option<crate::server::state::State>, Option<String>)> {
    let raw = request
        .storage_root
        .as_ref()
        .expect("validated: this level requires --storage-root");
    let storage_root = std::fs::canonicalize(raw).map_err(|error| {
        anyhow::anyhow!(
            "RESET_STORAGE_ROOT_INVALID: {} cannot be resolved: {error}",
            raw.display()
        )
    })?;
    let current_file = storage_root.join(".agent-collab/server/state.json");
    if !current_file.is_file() {
        anyhow::bail!(
            "RESET_STORAGE_ROOT_INVALID: --{level} storage root {} has no \
             .agent-collab/server/state.json, so it does not own the current host state",
            storage_root.display()
        );
    }
    let state = match crate::server::current_state::load(&current_file, false) {
        Ok(state) => state,
        Err(error) => {
            let descriptor_owns_index = resident_index_root(host_paths)
                .ok()
                .flatten()
                .is_some_and(|owner| owner == storage_root);
            if !descriptor_owns_index {
                return Err(anyhow::Error::msg(error));
            }
            return Ok((storage_root, None, Some(error.to_string())));
        }
    };
    let expected_scope = crate::server::GlobalState::canonical_project_scope(&storage_root)
        .map_err(|error| anyhow::anyhow!("RESET_STORAGE_ROOT_INVALID: {error}"))?;
    let has_any_record =
        !state.global.current_thread_routes.is_empty() || !state.global.projects.is_empty();
    let owns_root = state.global.projects.contains_key(expected_scope.as_str())
        || state
            .global
            .current_thread_routes
            .values()
            .any(|binding| binding.project_scope == expected_scope);
    if has_any_record && !owns_root {
        anyhow::bail!(
            "RESET_STORAGE_ROOT_INVALID: {} does not own the current host state at {}; the state \
             holds no route or project for that root",
            storage_root.display(),
            current_file.display()
        );
    }
    Ok((storage_root, Some(state), None))
}

/// The host control-plane entries that level 3 retires.
///
/// Each label is a single path segment: it suffixes the staging path.
/// The project business payload under
/// `<storage_root>/.agent-collab/` belongs to `--project`, not here.
fn host_control_plane_entries(
    state_root: &Path,
    storage_root: &Path,
    include_runs: bool,
) -> Vec<(String, PathBuf)> {
    let server_dir = storage_root.join(".agent-collab/server");
    let mut entries = vec![
        ("host-routes".to_owned(), state_root.join("routes.json")),
        (
            "host-identity-map".to_owned(),
            state_root.join("identity-map.json"),
        ),
        ("host-log-txt".to_owned(), state_root.join("log.txt")),
        ("host-identities".to_owned(), state_root.join("identities")),
        ("host-projects".to_owned(), state_root.join("projects")),
        ("host-archives".to_owned(), state_root.join("archives")),
        (
            "resident-current-state".to_owned(),
            server_dir.join("state.json"),
        ),
        (
            "resident-index-log-txt".to_owned(),
            server_dir.join("log.txt"),
        ),
    ];
    if include_runs {
        entries.push(("host-runs".to_owned(), state_root.join("runs")));
    }
    entries
}

/// Level 3: rebuild the host control plane.
///
/// It retires the host-side control state and the resident project's current
/// state, which together are the live index. It removes existing archives and
/// keeps the project business payload under
/// `<storage_root>/.agent-collab/`, the external service descriptor, and
/// `~/.collab/runs/` unless `--include-runs` was given.
fn run_host(scope: &Scope, host_paths: &HostPaths, request: &ResetRequest) -> anyhow::Result<()> {
    let (storage_root, _state, degraded_replay_error) =
        resolve_index_root(request, "host", host_paths)?;
    let state_root = host_paths.state_root().to_path_buf();

    let _lock =
        crate::server::acquire_reset_lock(&host_paths.lock_path(), &host_paths.socket_path())?;
    if crate::client::alive(&host_paths.socket_path()) {
        anyhow::bail!(
            "RESET_DAEMON_LIVE: offline maintenance is unavailable while the user-managed service is running at {}",
            host_paths.socket_path().display()
        );
    }

    let run_id = format!("reset-{}-{}", now_ms(), std::process::id());
    let mut retired = Vec::new();
    for (label, path) in
        host_control_plane_entries(&state_root, &storage_root, request.include_runs)
    {
        match std::fs::symlink_metadata(&path) {
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(error).with_context(|| {
                    format!(
                        "RESET_CONTROL_ROOT_INSPECTION_FAILED: cannot inspect {}",
                        path.display()
                    )
                });
            }
        }
        reject_unsafe_control_path(&path)?;
        retired.push(RetiredRoot {
            relative: label,
            absolute: path,
            staged: None,
        });
    }

    let already_reset = retired.is_empty();

    let mut record = json!({
        "schema": "collab-reset/v1",
        "run_id": run_id,
        "at_ms": now_ms(),
        "level": "host",
        "project_root": scope.root,
        "storage_root": storage_root,
        "state_root": state_root,
        "approval": request.approval,
        "already_reset": already_reset,
        "include_runs": request.include_runs,
        "archive": if already_reset { "absent" } else { "removed" },
        "retired": retired
            .iter()
            .map(|entry| json!(entry.relative))
            .collect::<Vec<_>>(),
        "delivery_verified": false,
        "next": "run collab up; it recreates the host index, identities, and project registry",
    });
    if let Some(state_error) = &degraded_replay_error {
        record["degraded"] = json!(true);
        record["current_state"] = json!("unreadable");
        record["current_state_error"] = json!(state_error);
        record["warning"] = json!(format!(
            "the resident state at {} could not be read; reset removed it and \
             rebuilt the host control plane without it: {state_error}",
            storage_root
                .join(".agent-collab/server/state.json")
                .display()
        ));
    }

    let transaction = (|| -> anyhow::Result<()> {
        if !already_reset {
            stage_retired_roots(&mut retired, &run_id)?;
        }
        discard_staged_roots(&retired)?;
        Ok(())
    })();
    if let Err(error) = transaction {
        let rollback_roots = rollback_retired_roots(&mut retired);
        if let Err(rollback_error) = rollback_roots {
            anyhow::bail!("RESET_INCOMPLETE: {error}; rollback also failed: {rollback_error}");
        }
        return Err(error.context("reset rolled back; the host control plane is restored"));
    }

    println!("{}", serde_json::to_string_pretty(&record)?);
    Ok(())
}

fn is_current_empty_baseline(agent_collab: &Path) -> bool {
    let expected_roots = [
        "handoff",
        "mailbox",
        "mailboxes",
        "merge-queue",
        "messages",
        "runs",
        "server",
    ];
    let Ok(entries) = std::fs::read_dir(agent_collab) else {
        return false;
    };
    let mut roots = Vec::new();
    for entry in entries.filter_map(Result::ok) {
        let Ok(file_type) = entry.file_type() else {
            return false;
        };
        if !file_type.is_dir() {
            return false;
        }
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            return false;
        };
        roots.push(name.to_owned());
    }
    roots.sort();
    if roots.is_empty() {
        return true;
    }
    if roots != expected_roots {
        return false;
    }
    for root in [
        "handoff",
        "mailbox",
        "mailboxes",
        "merge-queue",
        "messages",
        "runs",
    ] {
        let Ok(mut entries) = std::fs::read_dir(agent_collab.join(root)) else {
            return false;
        };
        if entries.next().is_some() {
            return false;
        }
    }

    let server = agent_collab.join("server");
    let Ok(entries) = std::fs::read_dir(&server) else {
        return false;
    };
    entries.filter_map(Result::ok).all(|entry| {
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            return false;
        };
        if !matches!(
            name,
            "daemon.lock" | "server.pid" | "DOWN" | "state.json" | "log.txt"
        ) {
            return false;
        }
        let Ok(metadata) = entry.metadata() else {
            return false;
        };
        metadata.is_file() && metadata.len() == 0
    })
}

#[cfg(test)]
#[path = "reset_tests.rs"]
mod tests;
