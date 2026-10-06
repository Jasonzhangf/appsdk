//! Authorized retirement of a legacy project-local Collab control plane.
//!
//! This is the single offline reset owner. It is deliberately separate from
//! `collab migrate`, which preserves history, and it never claims any delivery,
//! review, install, or communication evidence. The operation is only reachable
//! with an explicit operator approval string, requires the host daemon to be
//! down, archives the retired bytes, removes the project-local control plane
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
                         <storage-root>/.agent-collab/server/journal.jsonl and routes.jsonl \
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
    files: usize,
    sockets: Vec<String>,
    bytes: u64,
    digest: String,
}

fn archive_matches(entry: &RetiredRoot, files: usize, bytes: u64, digest: &str) -> bool {
    files == entry.files && bytes == entry.bytes && digest == entry.digest
}

fn source_matches(entry: &RetiredRoot, sockets: &[String]) -> bool {
    sockets == entry.sockets
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or(0)
}

fn fnv1a64(bytes: &[u8], seed: u64) -> u64 {
    let mut hash = seed;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

/// Deterministic tree digest for regular files. Unix sockets have no durable
/// bytes and are returned separately so reset can retire a stale endpoint
/// without pretending that it copied socket state into the archive.
fn tree_digest(root: &Path) -> std::io::Result<(usize, Vec<String>, u64, String)> {
    let root_metadata = std::fs::symlink_metadata(root)?;
    if root_metadata.file_type().is_symlink() {
        return Err(control_tree_symlink_error(root));
    }
    if !root_metadata.is_dir() {
        // A single control file is a one-file tree. Its archive is a directory
        // holding that file, so the digest must describe that same shape: one
        // file named after the source, hashed as name then content.
        let name = root
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        if is_unix_socket(&root_metadata) {
            return Ok((
                0,
                vec![name],
                0,
                format!("fnv1a64:{:016x}", 0xcbf29ce484222325_u64),
            ));
        }
        let content = std::fs::read(root)?;
        let hash = fnv1a64(&content, fnv1a64(name.as_bytes(), 0xcbf29ce484222325_u64));
        return Ok((
            1,
            Vec::new(),
            root_metadata.len(),
            format!("fnv1a64:{hash:016x}"),
        ));
    }
    let mut files = Vec::new();
    collect_files(root, root, &mut files)?;
    files.sort();
    let mut count = 0usize;
    let mut sockets = Vec::new();
    let mut bytes = 0u64;
    let mut hash = 0xcbf29ce484222325_u64;
    for relative in &files {
        let path = root.join(relative);
        let metadata = std::fs::symlink_metadata(&path)?;
        if metadata.file_type().is_symlink() {
            return Err(control_tree_symlink_error(&path));
        } else if is_unix_socket(&metadata) {
            sockets.push(relative.clone());
            continue;
        } else {
            hash = fnv1a64(relative.as_bytes(), hash);
            let content = std::fs::read(&path)?;
            hash = fnv1a64(&content, hash);
        }
        count += 1;
        bytes += metadata.len();
    }
    Ok((count, sockets, bytes, format!("fnv1a64:{hash:016x}")))
}

fn is_unix_socket(metadata: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::FileTypeExt;
    metadata.file_type().is_socket()
}

fn collect_files(root: &Path, current: &Path, out: &mut Vec<String>) -> std::io::Result<()> {
    for entry in std::fs::read_dir(current)? {
        let entry = entry?;
        let path = entry.path();
        let metadata = std::fs::symlink_metadata(&path)?;
        if metadata.file_type().is_symlink() {
            return Err(control_tree_symlink_error(&path));
        }
        if metadata.is_dir() {
            collect_files(root, &path, out)?;
        } else {
            let relative = path
                .strip_prefix(root)
                .map_err(|error| std::io::Error::other(error.to_string()))?;
            out.push(relative.to_string_lossy().into_owned());
        }
    }
    Ok(())
}

fn control_tree_symlink_error(path: &Path) -> std::io::Error {
    std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        format!(
            "RESET_CONTROL_TREE_SYMLINK_REJECTED: {} is a symlink; reset refuses to \
             create a non-self-contained archive",
            path.display()
        ),
    )
}

fn copy_tree(source: &Path, destination: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(destination)?;
    let source_metadata = std::fs::symlink_metadata(source)?;
    if source_metadata.file_type().is_symlink() {
        return Err(control_tree_symlink_error(source));
    }
    if !source_metadata.is_dir() {
        if !is_unix_socket(&source_metadata) {
            let name = source
                .file_name()
                .map(|name| name.to_owned())
                .unwrap_or_else(|| std::ffi::OsString::from("control-file"));
            let to = destination.join(name);
            std::fs::copy(source, &to)?;
            std::fs::File::open(&to)?.sync_all()?;
        }
        sync_dir(destination)?;
        return Ok(());
    }
    for entry in std::fs::read_dir(source)? {
        let entry = entry?;
        let from = entry.path();
        let to = destination.join(entry.file_name());
        let metadata = std::fs::symlink_metadata(&from)?;
        if metadata.file_type().is_symlink() {
            return Err(control_tree_symlink_error(&from));
        } else if metadata.is_dir() {
            copy_tree(&from, &to)?;
        } else if is_unix_socket(&metadata) {
            continue;
        } else {
            std::fs::copy(&from, &to)?;
            std::fs::File::open(&to)?.sync_all()?;
        }
    }
    sync_dir(destination)?;
    Ok(())
}

fn sync_dir(path: &Path) -> std::io::Result<()> {
    std::fs::File::open(path)?.sync_all()
}

fn reject_unsafe_control_roots(root: &Path) -> anyhow::Result<()> {
    for relative in LEGACY_CONTROL_ROOTS {
        let path = root.join(relative);
        match std::fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                anyhow::bail!(
                    "RESET_CONTROL_ROOT_SYMLINK_REJECTED: {} is a symlink; reset refuses \
                     to archive or write outside the project root",
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

fn slugify(root: &Path) -> String {
    let text = root.to_string_lossy();
    let mut slug = String::new();
    for ch in text.chars() {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch.to_ascii_lowercase());
        } else if !slug.ends_with('-') {
            slug.push('-');
        }
    }
    let slug = slug.trim_matches('-').to_string();
    if slug.is_empty() {
        "project".to_string()
    } else {
        slug.chars().take(80).collect()
    }
}

fn append_reset_record(host_paths: &HostPaths, record: &serde_json::Value) -> anyhow::Result<()> {
    use std::io::Write;
    let path = host_paths.state_root().join("reset.jsonl");
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)?;
    let mut line = serde_json::to_vec(record)?;
    line.push(b'\n');
    file.write_all(&line)?;
    file.sync_data()?;
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
                .with_file_name(format!("routes.jsonl.reset-restore-{}", std::process::id()));
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
            // A retire entry may be a single control file (routes.jsonl) or a
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

/// Rewrite the host route journal without the retired project's records.
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
    let path = host_paths.state_root().join("routes.jsonl");
    let content = match std::fs::read(&path) {
        Ok(content) => content,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(error) => return Err(error.into()),
    };
    if content.is_empty() {
        return Ok(0);
    }
    if !content.ends_with(b"\n") {
        anyhow::bail!(
            "HOST_ROUTE_DURABILITY_FAILED: route journal must end with a newline: {}",
            path.display()
        );
    }
    let mut kept = Vec::new();
    let mut removed = 0usize;
    for chunk in content.split_inclusive(|byte| *byte == b'\n') {
        let line = chunk.strip_suffix(b"\n").unwrap_or(chunk);
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        if line.iter().all(|byte| byte.is_ascii_whitespace()) {
            anyhow::bail!(
                "HOST_ROUTE_DURABILITY_FAILED: empty route journal line in {}",
                path.display()
            );
        }
        let record: serde_json::Value = serde_json::from_slice(line)?;
        match classify(&record) {
            Some(RouteDisposition::Retire) => {
                removed += 1;
                continue;
            }
            Some(RouteDisposition::Keep) | None => {}
        }
        kept.extend_from_slice(chunk);
    }
    if removed == 0 {
        return Ok(0);
    }
    use std::io::Write;
    let tmp = path.with_file_name(format!("routes.jsonl.reset-{}", std::process::id()));
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&tmp)?;
    file.write_all(&kept)?;
    file.sync_data()?;
    drop(file);
    std::fs::rename(&tmp, &path)?;
    std::fs::File::open(host_paths.state_root())?.sync_all()?;
    Ok(removed)
}

/// Archive the retire set, verify byte equality, and write the archive manifest.
///
/// Archive first, verify, and only then remove. A failed archive leaves the
/// retired control plane untouched. Every level shares this step, so the
/// manifest shape and the durability rules cannot drift between levels.
fn archive_retired(
    archive_root: &Path,
    state_root: &Path,
    retired: &[RetiredRoot],
    run_id: &str,
    project_root: &Path,
    approval: &str,
) -> anyhow::Result<()> {
    std::fs::create_dir_all(archive_root)?;
    for entry in retired {
        let destination = archive_root.join(&entry.relative);
        copy_tree(&entry.absolute, &destination)?;
        let (files, sockets, bytes, digest) = tree_digest(&destination)?;
        if !archive_matches(entry, files, bytes, &digest) {
            anyhow::bail!(
                "RESET_ARCHIVE_MISMATCH: {} -> {} ({files} files, {} sockets, {bytes} bytes, \
                 {digest}) does not match the source ({} files, {} sockets, {} bytes, {})",
                entry.absolute.display(),
                destination.display(),
                sockets.len(),
                entry.files,
                entry.sockets.len(),
                entry.bytes,
                entry.digest
            );
        }
        let source_sockets = tree_digest(&entry.absolute)?.1;
        if !source_matches(entry, &source_sockets) {
            anyhow::bail!(
                "RESET_SOURCE_SOCKET_CHANGED: {} socket inventory changed during archive",
                entry.absolute.display()
            );
        }
    }
    let manifest = json!({
        "schema": "collab-reset/v1",
        "run_id": run_id,
        "project_root": project_root,
        "approval": approval,
        "retired": retired
            .iter()
            .map(|entry| json!({
                "path": entry.relative,
                "files": entry.files,
                "sockets": entry.sockets,
                "bytes": entry.bytes,
                "digest": entry.digest,
            }))
            .collect::<Vec<_>>(),
        "delivery_verified": false,
        "created_ms": now_ms(),
    });
    let manifest_path = archive_root.join("manifest.json");
    std::fs::write(
        &manifest_path,
        format!("{}\n", serde_json::to_string_pretty(&manifest)?),
    )?;
    std::fs::File::open(&manifest_path)?.sync_all()?;
    sync_dir(archive_root)?;
    if let Some(archives_dir) = archive_root.parent() {
        sync_dir(archives_dir)?;
    }
    sync_dir(state_root)?;
    Ok(())
}

/// The single reset entry. It authorizes the run, resolves the level, and
/// dispatches to the level body.
///
/// The three levels share this authorization and the same transaction shape.
/// Only the inventory and the retire set differ.
pub fn run(scope: &Scope, host_paths: &HostPaths, request: ResetRequest) -> anyhow::Result<()> {
    if !request.discard_legacy {
        anyhow::bail!(
            "RESET_AUTHORIZATION_REQUIRED: collab reset requires --discard-legacy; \
             use `collab migrate` to preserve history"
        );
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
/// root's project journal is the index the daemon replays, so the project level
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

fn run_project(scope: &Scope, host_paths: &HostPaths, request: &ResetRequest) -> anyhow::Result<()> {
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
            "RESET_DAEMON_LIVE: a Collab daemon is reachable at {}; run `collab down` \
             before retiring the project control plane",
            host_paths.socket_path().display()
        );
    }
    if resident_index_root(host_paths)?.is_some_and(|owner| owner == root) {
        anyhow::bail!(
            "RESET_PROJECT_HOLDS_HOST_INDEX: {} is the storage root of the running daemon, so its \
             .agent-collab/ is the live route index; retire that index with `collab reset --host \
             --storage-root {}` instead",
            root.display(),
            root.display()
        );
    }

    let run_id = format!("reset-{}-{}", now_ms(), std::process::id());
    let archive_root = host_paths
        .state_root()
        .join("archives")
        .join(format!("{}-{run_id}", slugify(&root)));
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
        let (files, sockets, bytes, digest) = tree_digest(&absolute)?;
        retired.push(RetiredRoot {
            relative: relative.to_string(),
            absolute,
            staged: None,
            files,
            sockets,
            bytes,
            digest,
        });
    }

    let already_reset = retired.is_empty();
    if !already_reset {
        archive_retired(
            &archive_root,
            host_paths.state_root(),
            &retired,
            &run_id,
            &root,
            &request.approval,
        )?;
    }

    let routes_path = host_paths.state_root().join("routes.jsonl");
    let reset_record_path = host_paths.state_root().join("reset.jsonl");
    let route_snapshot = snapshot_file(routes_path)?;
    let reset_record_snapshot = snapshot_file(reset_record_path)?;
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
            "archive_root": if already_reset { None } else { Some(archive_root) },
            "removed_host_routes": removed_routes,
            "removed_stale_host_routes": removed_stale_routes,
            "retired": retired
                .iter()
                .map(|entry| json!({
                    "path": entry.relative,
                    "files": entry.files,
                    "sockets": entry.sockets,
                    "bytes": entry.bytes,
                    "digest": entry.digest,
                }))
                .collect::<Vec<_>>(),
            "archive_durable": !already_reset,
            "delivery_verified": false,
            "next": "run collab up, then collab init from this exact project root",
        });
        append_reset_record(host_paths, &record)?;
        discard_staged_roots(&retired)?;
        Ok(())
    })();

    if let Err(error) = transaction {
        let rollback_roots = rollback_retired_roots(&mut retired);
        let rollback_routes = restore_file(&route_snapshot);
        let rollback_record = restore_file(&reset_record_snapshot);
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
            .and(rollback_record)
            .and(rollback_doc)
            .and(rollback_baseline)
            .and(rollback_docs_dir)
        {
            let incomplete = json!({
                "schema": "collab-reset/v1",
                "run_id": run_id,
                "at_ms": now_ms(),
                "project_root": root,
                "approval": request.approval,
                "status": "incomplete",
                "delivery_verified": false,
                "error": error.to_string(),
                "rollback_error": rollback_error.to_string(),
            });
            let _ = append_reset_record(host_paths, &incomplete);
            anyhow::bail!("RESET_INCOMPLETE: {error}; rollback also failed: {rollback_error}");
        }
        return Err(error.context("reset rolled back; legacy control plane restored"));
    }

    println!("{}", serde_json::to_string_pretty(&record)?);
    Ok(())
}

/// Resolve and verify the root that owns the live host index.
///
/// `routes.jsonl` does not identify the index root, so the operator must name
/// it. The journal inside it must exist, and a non-empty journal must hold a
/// record for that root, so a typo cannot retarget the operation at another
/// project's control plane.
fn resolve_index_root(
    request: &ResetRequest,
    level: &str,
) -> anyhow::Result<(PathBuf, crate::server::state::State)> {
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
    let index_journal = storage_root.join(".agent-collab/server/journal.jsonl");
    if !index_journal.is_file() {
        anyhow::bail!(
            "RESET_STORAGE_ROOT_INVALID: --{level} storage root {} has no \
             .agent-collab/server/journal.jsonl, so it does not own the live host index",
            storage_root.display()
        );
    }
    let state = crate::server::replay_host_index(&storage_root)?;
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
            "RESET_STORAGE_ROOT_INVALID: {} does not own the live host index at {}; the journal \
             holds no route or project for that root",
            storage_root.display(),
            index_journal.display()
        );
    }
    Ok((storage_root, state))
}

/// The host control-plane entries that level 3 retires.
///
/// Each label is a single path segment: it names the archive directory and it
/// suffixes the staging path. `reset.jsonl` and `archives/` are the audit trail
/// and are never listed. The project business payload under
/// `<storage_root>/.agent-collab/` belongs to `--project`, not here.
fn host_control_plane_entries(
    state_root: &Path,
    storage_root: &Path,
    include_runs: bool,
) -> Vec<(String, PathBuf)> {
    let server_dir = storage_root.join(".agent-collab/server");
    let mut entries = vec![
        ("host-routes-jsonl".to_owned(), state_root.join("routes.jsonl")),
        (
            "host-journal-jsonl".to_owned(),
            state_root.join("journal.jsonl"),
        ),
        (
            "host-events-jsonl".to_owned(),
            state_root.join("events.jsonl"),
        ),
        ("host-log-txt".to_owned(), state_root.join("log.txt")),
        ("host-identities".to_owned(), state_root.join("identities")),
        ("host-projects".to_owned(), state_root.join("projects")),
        (
            "resident-index-journal-jsonl".to_owned(),
            server_dir.join("journal.jsonl"),
        ),
        (
            "resident-index-events-jsonl".to_owned(),
            server_dir.join("events.jsonl"),
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
/// It retires the host-side control state and the resident project's runtime
/// journal, which together are the live index. It keeps the audit trail
/// (`reset.jsonl` and `archives/`), the project business payload under
/// `<storage_root>/.agent-collab/`, the external service descriptor, and
/// `~/.collab/runs/` unless `--include-runs` was given.
fn run_host(scope: &Scope, host_paths: &HostPaths, request: &ResetRequest) -> anyhow::Result<()> {
    let (storage_root, _state) = resolve_index_root(request, "host")?;
    let state_root = host_paths.state_root().to_path_buf();

    let _lock =
        crate::server::acquire_reset_lock(&host_paths.lock_path(), &host_paths.socket_path())?;
    if crate::client::alive(&host_paths.socket_path()) {
        anyhow::bail!(
            "RESET_DAEMON_LIVE: a Collab daemon is reachable at {}; run `collab down` \
             before rebuilding the host control plane",
            host_paths.socket_path().display()
        );
    }

    let run_id = format!("reset-{}-{}", now_ms(), std::process::id());
    let archive_root = state_root.join("archives").join(format!("host-{run_id}"));
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
        let (files, sockets, bytes, digest) = tree_digest(&path)?;
        retired.push(RetiredRoot {
            relative: label,
            absolute: path,
            staged: None,
            files,
            sockets,
            bytes,
            digest,
        });
    }

    let already_reset = retired.is_empty();
    if !already_reset {
        archive_retired(
            &archive_root,
            &state_root,
            &retired,
            &run_id,
            &storage_root,
            &request.approval,
        )?;
    }

    let reset_record_snapshot = snapshot_file(state_root.join("reset.jsonl"))?;
    let record = json!({
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
        "archive_root": if already_reset { None } else { Some(archive_root) },
        "retired": retired
            .iter()
            .map(|entry| json!({
                "path": entry.relative,
                "files": entry.files,
                "sockets": entry.sockets,
                "bytes": entry.bytes,
                "digest": entry.digest,
            }))
            .collect::<Vec<_>>(),
        "archive_durable": !already_reset,
        "delivery_verified": false,
        "next": "run collab up; it recreates the host index, identities, and project registry",
    });

    let transaction = (|| -> anyhow::Result<()> {
        if !already_reset {
            stage_retired_roots(&mut retired, &run_id)?;
        }
        append_reset_record(host_paths, &record)?;
        discard_staged_roots(&retired)?;
        Ok(())
    })();
    if let Err(error) = transaction {
        let rollback_roots = rollback_retired_roots(&mut retired);
        let rollback_record = restore_file(&reset_record_snapshot);
        if let Err(rollback_error) = rollback_roots.and(rollback_record) {
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
            "daemon.lock" | "server.pid" | "DOWN" | "journal.jsonl" | "events.jsonl" | "log.txt"
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
