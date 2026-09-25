use std::path::{Path, PathBuf};
use std::process::Command;

use crate::identity::{validate_id_for_protocol, AppServerId};
use crate::server::{load_host_route_records, validate_host_route_record};
use serde::{Deserialize, Serialize};

pub const COLLAB_STATE_DIR_ENV: &str = "COLLAB_STATE_DIR";
pub const HOME_ENV: &str = "HOME";
pub const COLLAB_SOCKET_PATH_ENV: &str = "COLLAB_SOCKET_PATH";
pub const COLLAB_HOST_SOCKET_ENV: &str = "COLLAB_HOST_SOCKET";
pub const COLLAB_LOCK_PATH_ENV: &str = "COLLAB_LOCK_PATH";
pub const COLLAB_HOST_LOCK_ENV: &str = "COLLAB_HOST_LOCK";

#[cfg(test)]
pub(crate) static TEST_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostPaths {
    state_root: PathBuf,
    socket_path: PathBuf,
    lock_path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CanonicalProjectRoute {
    pub root: PathBuf,
    pub app_scope_id: AppServerId,
}

impl HostPaths {
    pub fn from_state_root(root: impl AsRef<Path>) -> anyhow::Result<Self> {
        let state_root = validate_host_path(root.as_ref().to_path_buf(), "host state root")?;
        Ok(Self {
            socket_path: state_root.join("server.sock"),
            lock_path: state_root.join("daemon.lock"),
            state_root,
        })
    }

    pub fn for_state_root(root: impl AsRef<Path>) -> anyhow::Result<Self> {
        Self::from_state_root(root)
    }

    pub fn resolve_from_env() -> anyhow::Result<Self> {
        let state_root = resolve_state_root(
            std::env::var_os(COLLAB_STATE_DIR_ENV),
            std::env::var_os(HOME_ENV),
        )?;
        let mut paths = Self::from_state_root(state_root)?;
        apply_endpoint_overrides(
            &mut paths,
            first_env_path([COLLAB_SOCKET_PATH_ENV, COLLAB_HOST_SOCKET_ENV])?,
            first_env_path([COLLAB_LOCK_PATH_ENV, COLLAB_HOST_LOCK_ENV])?,
        )?;
        Ok(paths)
    }

    pub fn from_env() -> anyhow::Result<Self> {
        Self::resolve_from_env()
    }

    pub fn resolve() -> anyhow::Result<Self> {
        Self::resolve_from_env()
    }

    pub fn for_project(_project_root: &Path) -> anyhow::Result<Self> {
        Self::resolve_from_env()
    }

    pub fn state_root(&self) -> &Path {
        &self.state_root
    }

    pub fn server_dir(&self) -> PathBuf {
        self.state_root.clone()
    }

    pub fn socket_path(&self) -> PathBuf {
        self.socket_path.clone()
    }

    pub fn sock_path(&self) -> PathBuf {
        self.socket_path()
    }

    pub fn lock_path(&self) -> PathBuf {
        self.lock_path.clone()
    }

    pub fn down_path(&self) -> PathBuf {
        self.state_root.join("DOWN")
    }

    pub fn events_path(&self) -> PathBuf {
        self.state_root.join("events.jsonl")
    }

    pub fn journal_path(&self) -> PathBuf {
        self.state_root.join("journal.jsonl")
    }

    pub fn pid_path(&self) -> PathBuf {
        self.state_root.join("server.pid")
    }

    pub fn log_path(&self) -> PathBuf {
        self.state_root.join("log.txt")
    }

    pub fn ensure_root(&self) -> std::io::Result<()> {
        std::fs::create_dir_all(&self.state_root)
    }
}

/// Load the durable project routes used by exact-root recovery commands.
fn load_route_records(host_paths: &HostPaths) -> anyhow::Result<Vec<CanonicalProjectRoute>> {
    let route_journal = host_paths.state_root().join("routes.jsonl");
    let mut records = Vec::new();
    for record in load_host_route_records(&route_journal).map_err(anyhow::Error::msg)? {
        let canonical_root = match std::fs::canonicalize(&record.canonical_root) {
            Ok(root) => root,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(anyhow::anyhow!(
                    "cannot canonicalize route root {}: {error}",
                    record.canonical_root
                ));
            }
        };
        if !canonical_root.join(".agent-collab").is_dir() {
            continue;
        }
        if is_linked_worktree_root(&canonical_root)? {
            continue;
        }
        let (_, root, _) = validate_host_route_record(&record).map_err(anyhow::Error::msg)?;
        records.push(CanonicalProjectRoute {
            root,
            app_scope_id: AppServerId::new(record.app_scope_id)?,
        });
    }
    records.sort_by(|left, right| {
        right
            .root
            .components()
            .count()
            .cmp(&left.root.components().count())
            .then_with(|| left.root.cmp(&right.root))
    });
    Ok(records)
}

pub(crate) fn is_linked_worktree_root(root: &Path) -> anyhow::Result<bool> {
    let top_level = Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .current_dir(root)
        .output();
    let Ok(top_level) = top_level else {
        return Ok(false);
    };
    if !top_level.status.success() {
        return Ok(false);
    }
    let top_level = std::fs::canonicalize(String::from_utf8_lossy(&top_level.stdout).trim())?;
    if top_level != root {
        return Ok(false);
    }
    let output = Command::new("git")
        .args(["rev-parse", "--git-common-dir"])
        .current_dir(root)
        .output();
    let Ok(output) = output else {
        return Ok(false);
    };
    if !output.status.success() {
        return Ok(false);
    }
    let common = PathBuf::from(String::from_utf8_lossy(&output.stdout).trim());
    let common = if common.is_absolute() {
        common
    } else {
        root.join(common)
    };
    let common = std::fs::canonicalize(common)?;
    let main_root = common.parent().ok_or_else(|| {
        anyhow::anyhow!("Git common directory has no parent: {}", common.display())
    })?;
    Ok(root != main_root)
}

/// Resolve the route bound to one registered peer identity at the exact cwd.
///
/// The identity's App Server scope is authoritative. The caller's cwd may be
/// a Git worktree, so it must be inside the route's canonical root but is not
/// allowed to select a different project by itself.
pub fn canonical_route_for_identity(
    host_paths: &HostPaths,
    cwd: &Path,
    app_scope_id: &AppServerId,
) -> anyhow::Result<CanonicalProjectRoute> {
    let cwd = std::fs::canonicalize(cwd)?;
    let matches = load_route_records(host_paths)?
        .into_iter()
        .filter(|route| {
            &route.app_scope_id == app_scope_id && cwd.strip_prefix(&route.root).is_ok()
        })
        .collect::<Vec<_>>();
    let (mut matches, git_roots) = narrow_routes_for_git_worktree(&cwd, matches)?;
    match matches.len() {
        1 => Ok(matches.pop().unwrap()),
        0 => anyhow::bail!(
            "no registered Collab route matches app scope {} for the Git main worktree {} containing cwd {}",
            app_scope_id,
            git_roots
                .as_ref()
                .map(|roots| roots.main_root.display().to_string())
                .unwrap_or_else(|| "not detected".into()),
            cwd.display()
        ),
        _ => {
            let roots = matches
                .iter()
                .map(|route| route.root.display().to_string())
                .collect::<Vec<_>>();
            anyhow::bail!(
                "multiple canonical Collab routes match app scope {} and contain cwd {}: {}",
                app_scope_id,
                cwd.display(),
                roots.join(", ")
            )
        }
    }
}

pub fn route_for_tmux_pane(host_paths: &HostPaths) -> anyhow::Result<CanonicalProjectRoute> {
    let candidate =
        crate::client::adapters::tmux::candidate_from_env().map_err(anyhow::Error::msg)?;
    let route = crate::client::resolve_route(&host_paths.socket_path(), &candidate.endpoint)?;
    Ok(CanonicalProjectRoute {
        root: PathBuf::from(route.canonical_root),
        app_scope_id: route.app_scope_id,
    })
}

/// Resolve the canonical registered project route for a read-only command.
///
/// This lookup is deliberately identity-free so `master status` can answer
/// from a linked worktree or a freshly reset project before the caller has a
/// local route. It still requires an exact registered route and never guesses
/// a project from an ancestor directory.
pub fn canonical_route_for_cwd(
    host_paths: &HostPaths,
    cwd: &Path,
) -> anyhow::Result<CanonicalProjectRoute> {
    let cwd = std::fs::canonicalize(cwd)?;
    let matches = load_route_records(host_paths)?
        .into_iter()
        .filter(|route| cwd.strip_prefix(&route.root).is_ok())
        .collect::<Vec<_>>();
    let (mut matches, _) = narrow_routes_for_git_worktree(&cwd, matches)?;
    match matches.len() {
        1 => Ok(matches.pop().unwrap()),
        0 => anyhow::bail!("no registered Collab route contains cwd {}", cwd.display()),
        _ => {
            let roots = matches
                .iter()
                .map(|route| route.root.display().to_string())
                .collect::<Vec<_>>();
            anyhow::bail!(
                "multiple canonical Collab routes contain cwd {}: {}",
                cwd.display(),
                roots.join(", ")
            )
        }
    }
}

fn narrow_routes_for_git_worktree(
    cwd: &Path,
    mut matches: Vec<CanonicalProjectRoute>,
) -> anyhow::Result<(Vec<CanonicalProjectRoute>, Option<GitWorktreeRoots>)> {
    let git_roots = git_worktree_roots_if_any(cwd)?;
    if let Some(git_roots) = &git_roots {
        let nested_matches = matches
            .iter()
            .filter(|route| {
                route.root != git_roots.main_root
                    && route.root != git_roots.worktree_root
                    && route.root.starts_with(&git_roots.worktree_root)
            })
            .cloned()
            .collect::<Vec<_>>();
        if !nested_matches.is_empty() {
            return Ok((nested_matches, Some(git_roots.clone())));
        }
        if matches
            .iter()
            .any(|route| route.root == git_roots.main_root)
        {
            matches.retain(|route| route.root == git_roots.main_root);
        } else if matches
            .iter()
            .any(|route| route.root == git_roots.worktree_root)
            && git_roots.worktree_root != git_roots.main_root
        {
            // A linked worktree may not register its own route. When the
            // route is rooted at the worktree itself, fail closed instead of
            // treating it as a second project.
            matches.retain(|route| route.root != git_roots.worktree_root);
        }
    }
    Ok((matches, git_roots))
}

#[derive(Clone)]
struct GitWorktreeRoots {
    main_root: PathBuf,
    worktree_root: PathBuf,
}

fn git_worktree_roots_if_any(cwd: &Path) -> anyhow::Result<Option<GitWorktreeRoots>> {
    let worktree_output = Command::new("git")
        .args(["rev-parse", "--path-format=absolute", "--show-toplevel"])
        .current_dir(cwd)
        .output()?;
    if !worktree_output.status.success() {
        let stderr = String::from_utf8_lossy(&worktree_output.stderr);
        if stderr.contains("not a git repository") {
            return Ok(None);
        }
        anyhow::bail!(
            "cannot resolve Git worktree root from {}: {}",
            cwd.display(),
            stderr.trim()
        );
    }
    let worktree_root = PathBuf::from(String::from_utf8_lossy(&worktree_output.stdout).trim());
    if !worktree_root.is_absolute() {
        anyhow::bail!(
            "Git worktree root is not absolute for {}: {}",
            cwd.display(),
            worktree_root.display()
        );
    }
    let worktree_root = std::fs::canonicalize(worktree_root)?;

    let worktrees_output = Command::new("git")
        .args(["worktree", "list", "--porcelain"])
        .current_dir(cwd)
        .output()?;
    if !worktrees_output.status.success() {
        anyhow::bail!(
            "cannot list Git worktrees from {}: {}",
            cwd.display(),
            String::from_utf8_lossy(&worktrees_output.stderr).trim()
        );
    }

    let main_root = std::fs::canonicalize(
        String::from_utf8_lossy(&worktrees_output.stdout)
            .lines()
            .find_map(|line| line.strip_prefix("worktree "))
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "Git worktree list has no worktree entry for {}",
                    cwd.display()
                )
            })?,
    )?;

    Ok(Some(GitWorktreeRoots {
        main_root,
        worktree_root,
    }))
}

fn apply_endpoint_overrides(
    paths: &mut HostPaths,
    socket_override: Option<PathBuf>,
    lock_override: Option<PathBuf>,
) -> anyhow::Result<()> {
    if let Some(value) = socket_override {
        let parent = value.parent().ok_or_else(|| {
            anyhow::anyhow!("host socket path has no parent: {}", value.display())
        })?;
        if parent != paths.state_root() {
            anyhow::bail!(
                "host socket path must be inside host state root {}: {}",
                paths.state_root().display(),
                value.display()
            );
        }
        paths.socket_path = value;
    }
    if let Some(value) = lock_override {
        let expected = paths.state_root().join("daemon.lock");
        if value != expected {
            anyhow::bail!(
                "host lock path must be {} so client and daemon share one lock owner: {}",
                expected.display(),
                value.display()
            );
        }
        paths.lock_path = value;
    }
    Ok(())
}

fn first_env_path<const N: usize>(names: [&str; N]) -> anyhow::Result<Option<PathBuf>> {
    for name in names {
        if let Some(value) = nonempty_env(name) {
            return Ok(Some(validate_host_path(
                PathBuf::from(value),
                "host endpoint",
            )?));
        }
    }
    Ok(None)
}

fn nonempty_env(name: &str) -> Option<std::ffi::OsString> {
    std::env::var_os(name).filter(|value| !value.is_empty())
}

fn resolve_state_root(
    state_dir: Option<std::ffi::OsString>,
    home: Option<std::ffi::OsString>,
) -> anyhow::Result<PathBuf> {
    if let Some(value) = state_dir.filter(|value| !value.is_empty()) {
        return Ok(PathBuf::from(value));
    }
    if let Some(value) = home.filter(|value| !value.is_empty()) {
        return Ok(PathBuf::from(value).join(".collab"));
    }
    anyhow::bail!(
        "collab host state root is unavailable; set ${COLLAB_STATE_DIR_ENV} or ${HOME_ENV}"
    )
}

fn validate_host_path(path: PathBuf, label: &str) -> anyhow::Result<PathBuf> {
    if !path.is_absolute() {
        anyhow::bail!("{label} must be an absolute path: {}", path.display());
    }
    if path.components().any(|component| {
        matches!(
            component,
            std::path::Component::ParentDir | std::path::Component::CurDir
        )
    }) {
        anyhow::bail!(
            "{label} must not contain '.' or '..' path components: {}",
            path.display()
        );
    }
    Ok(path)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ProjectScopeId(String);

impl ProjectScopeId {
    pub fn new(value: impl Into<String>) -> anyhow::Result<Self> {
        let value = value.into();
        if value.is_empty() {
            anyhow::bail!("project scope id must not be empty");
        }
        if value.chars().any(char::is_control) {
            anyhow::bail!("project scope id must not contain control characters");
        }
        if !Path::new(&value).is_absolute() {
            anyhow::bail!("project scope id must be an absolute path");
        }
        Ok(Self(value))
    }

    fn from_registered_cwd(cwd: &Path) -> anyhow::Result<Self> {
        let root = normalize_registered_cwd(cwd)?;
        let value = root.to_str().ok_or_else(|| {
            anyhow::anyhow!("registered project cwd must be valid UTF-8 for the wire scope")
        })?;
        Self::new(value.to_owned())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn validate(&self) -> anyhow::Result<()> {
        Self::new(self.0.clone()).map(|_| ())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RouteScope {
    pub app_scope_id: AppServerId,
    pub project_scope_id: ProjectScopeId,
}

impl RouteScope {
    pub fn for_registered_project(
        app_scope_id: AppServerId,
        registered_cwd: &Path,
    ) -> anyhow::Result<Self> {
        Ok(Self {
            app_scope_id,
            project_scope_id: ProjectScopeId::from_registered_cwd(registered_cwd)?,
        })
    }

    pub fn validate_registered_cwd(&self, registered_cwd: &Path) -> anyhow::Result<()> {
        let normalized = ProjectScopeId::from_registered_cwd(registered_cwd)?;
        if self.project_scope_id != normalized {
            anyhow::bail!(
                "project cwd is outside the registered project scope: {}",
                registered_cwd.display()
            );
        }
        Ok(())
    }

    pub fn validate(&self) -> anyhow::Result<()> {
        validate_id_for_protocol(self.app_scope_id.as_str())?;
        self.project_scope_id.validate()
    }

    pub fn validate_same_route(&self, other: &Self) -> anyhow::Result<()> {
        if self != other {
            anyhow::bail!("route scope mismatch");
        }
        Ok(())
    }
}

fn normalize_registered_cwd(cwd: &Path) -> anyhow::Result<PathBuf> {
    if !cwd.is_absolute() || !cwd.is_dir() {
        anyhow::bail!(
            "registered project cwd must be an existing absolute directory: {}",
            cwd.display()
        );
    }
    Ok(std::fs::canonicalize(cwd)?)
}

fn validate_project_root(root: PathBuf) -> anyhow::Result<PathBuf> {
    if !root.is_absolute() || !root.is_dir() {
        anyhow::bail!(
            "project root must be an existing absolute directory: {}",
            root.display()
        );
    }
    Ok(root)
}

pub fn project_root() -> anyhow::Result<PathBuf> {
    Ok(Scope::resolve()?.root)
}

/// Resolve the exact current project root for identity recovery.
///
/// Recovery is the operation that restores a missing native-thread route, so
/// it cannot use the route selector that it is responsible for repairing.
pub fn resolve_for_recovery() -> anyhow::Result<Scope> {
    let cwd = validate_project_root(std::env::current_dir()?)?;
    let host_paths = HostPaths::resolve()?;
    let route = canonical_route_for_cwd(&host_paths, &cwd)?;
    Scope::from_project_root(route.root)
}

/// Resolve the project rooted at the process cwd for lifecycle commands.
///
/// Daemon start/stop and configuration commands own local lifecycle state.
/// They must not follow `CODEX_THREAD_ID` to a different project route.
pub fn lifecycle_project_root() -> anyhow::Result<PathBuf> {
    validate_project_root(std::env::current_dir()?)
}

/// Resolve the exact destination for `collab init`. Initialization binds to
/// the process cwd.
pub fn project_root_for_init() -> anyhow::Result<PathBuf> {
    init_project_root(std::env::current_dir()?)
}

fn init_project_root(cwd: PathBuf) -> anyhow::Result<PathBuf> {
    validate_project_root(cwd)
}

/// Create only the current Collab-owned empty project baseline.
///
/// Reset uses this instead of [`init`] so retiring a legacy control plane
/// cannot mutate project MCP settings, editor permissions, or global AppSDK
/// configuration as an unrecorded side effect.
pub fn init_collab_baseline(root: &Path) -> std::io::Result<PathBuf> {
    let base = root.join(".agent-collab");
    for sub in [
        "runs",
        "handoff",
        "merge-queue",
        "mailbox",
        "messages",
        "mailboxes",
        "server",
    ] {
        std::fs::create_dir_all(base.join(sub))?;
    }
    Ok(base)
}

pub fn init(root: &Path) -> std::io::Result<PathBuf> {
    let base = init_collab_baseline(root)?;
    let docs = root.join("docs");
    std::fs::create_dir_all(&docs)?;
    let collab_doc = docs.join("collab.md");
    if !collab_doc.exists() {
        std::fs::write(&collab_doc, COLLAB_DOC)?;
    }
    ensure_project_collab_mcp(root)?;
    ensure_codex_collab_permissions(root)?;
    ensure_claude_collab_permissions(root)?;
    crate::config::ensure_written()
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::Other, error))?;
    Ok(base)
}

fn collab_mcp_command() -> String {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join("collab-mcp")))
        .filter(|p| p.is_file())
        .map(|p| p.to_string_lossy().into_owned())
        .or_else(|| {
            std::env::var_os("PATH").and_then(|path| {
                std::env::split_paths(&path).find_map(|dir| {
                    let candidate = dir.join("collab-mcp");
                    candidate
                        .is_file()
                        .then(|| candidate.to_string_lossy().into_owned())
                })
            })
        })
        .unwrap_or_else(|| "collab-mcp".into())
}

fn ensure_project_collab_mcp(root: &Path) -> std::io::Result<()> {
    merge_collab_mcp(root.join(".mcp.json"))
}

fn merge_collab_mcp(path: PathBuf) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut root_value = if path.exists() {
        let contents = std::fs::read_to_string(&path)?;
        serde_json::from_str(&contents).map_err(|error| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("invalid JSON in {}: {error}", path.display()),
            )
        })?
    } else {
        serde_json::json!({})
    };
    let servers = root_value
        .as_object_mut()
        .ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("JSON root in {} must be an object", path.display()),
            )
        })?
        .entry("mcpServers")
        .or_insert_with(|| serde_json::json!({}))
        .as_object_mut()
        .ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("mcpServers in {} must be an object", path.display()),
            )
        })?;
    if servers.contains_key("collab") {
        return Ok(());
    }
    servers.insert(
        "collab".into(),
        serde_json::json!({"command": collab_mcp_command()}),
    );
    std::fs::write(
        path,
        format!("{}\n", serde_json::to_string_pretty(&root_value).unwrap()),
    )
}

fn ensure_codex_collab_permissions(root: &Path) -> std::io::Result<()> {
    let path = root.join(".codex").join("config.toml");
    std::fs::create_dir_all(path.parent().unwrap())?;
    let mut table = if path.exists() {
        let contents = std::fs::read_to_string(&path)?;
        let value = toml::from_str::<toml::Value>(&contents).map_err(|error| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("invalid TOML in {}: {error}", path.display()),
            )
        })?;
        value.as_table().cloned().ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("TOML root in {} must be a table", path.display()),
            )
        })?
    } else {
        toml::Table::new()
    };
    {
        let servers = table
            .entry("mcp_servers")
            .or_insert_with(|| toml::Value::Table(toml::Table::new()))
            .as_table_mut()
            .ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("mcp_servers in {} must be a table", path.display()),
                )
            })?;
        if !servers.contains_key("collab") {
            let mut collab = toml::Table::new();
            collab.insert("command".into(), toml::Value::String(collab_mcp_command()));
            servers.insert("collab".into(), toml::Value::Table(collab));
        }
    }
    table.remove("sandbox_mode");
    table.remove("approval_policy");
    std::fs::write(
        path,
        format!("{}\n", toml::to_string_pretty(&table).unwrap()),
    )
}

fn merge_allow_patterns(
    value: &mut serde_json::Value,
    key: &str,
    patterns: &[&str],
    path: &Path,
) -> std::io::Result<()> {
    let list = value
        .as_object_mut()
        .map(|table| table.entry(key).or_insert_with(|| serde_json::json!([])))
        .and_then(|item| item.as_array_mut());
    let Some(list) = list else {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("{key} in {} must be an array", path.display()),
        ));
    };
    for pattern in patterns {
        if !list.iter().any(|item| item.as_str() == Some(*pattern)) {
            list.push(serde_json::json!(pattern));
        }
    }
    Ok(())
}

fn ensure_claude_collab_permissions(root: &Path) -> std::io::Result<()> {
    let path = root.join(".claude").join("settings.json");
    std::fs::create_dir_all(path.parent().unwrap())?;
    let mut root_value = if path.exists() {
        let contents = std::fs::read_to_string(&path)?;
        serde_json::from_str(&contents).map_err(|error| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("invalid JSON in {}: {error}", path.display()),
            )
        })?
    } else {
        serde_json::json!({})
    };
    let table = root_value.as_object_mut().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("JSON root in {} must be an object", path.display()),
        )
    })?;
    let permissions = table
        .entry("permissions")
        .or_insert_with(|| serde_json::json!({}));
    merge_allow_patterns(
        permissions,
        "allow",
        &["Bash(collab)", "Bash(collab *)", "Bash(collab-mcp)"],
        &path,
    )?;
    std::fs::write(
        path,
        format!("{}\n", serde_json::to_string_pretty(&root_value).unwrap()),
    )
}

pub const COLLAB_DOC: &str = r#"# collab workflow

This project uses the local `collab` daemon for multi-agent coordination.
The source and build truth lives in the AppSDK repository's `collab/` directory;
the installed independent binaries are `~/.cargo/bin/collab` and
`~/.cargo/bin/collab-mcp`. From an AppSDK checkout, use
`scripts/install-global-collab.sh`; do not build a second copy from an external
Collab checkout.

The daemon is detached. Normal commands may start it when no explicit `DOWN`
marker exists. `collab init` creates the local
`.agent-collab/server` skeleton, so old projects need no manual repair. Use
`collab down` only for an explicit stop; use `collab up` to clear that stop and
start it again. Never start a second daemon.
Existing projects migrate through `collab migrate inspect`, `plan`, `apply`,
controlled daemon upgrade/restart, identity rebind, and `verify`;
deleting `.agent-collab`, editing JSON state, clearing mailboxes, copying
tokens, mixed runtime writes, and guessing thread identity are deprecated.

## Runtime boundary

- Every peer registration must include a server-verified App Server candidate.
- Registration owns one deterministic seven-day default direct-message lease;
  daemon restart restores it only while the registered App Server thread still
  matches the peer identity. A shorter explicit lease cannot suppress it.
- App Server is the only registered notification transport.
- Server state, journal, and mailbox are durable truth; a failed wake cannot
  roll back state or fabricate success.
- The runtime is part of the worker identity boundary, not a task preference.

## Roles

- Every registered identity is an equal `peer`; there is no inferred master
  from first registration. Codex root is not Collab master.
- `collab init` and peer registration never create a master. A master exists
  only when a registered peer has a live server-verified transport and was assigned by
  user-approved self-promotion or live-master delegation. A recorded identity
  with a dead App Server thread is not a live master.
- If a live master exists, other peers cannot promote; only that master may
  `collab master delegate <peer>`. If no live master exists, a peer may
  `collab master promote --approval "<user text>"` itself after explicit user
  approval. Master authority is arbitration only; it does not take another
  peer's task. Independent peers may temporarily decline a master
  collaboration invite to protect their own task; managed subagents must obey
  the master.
- Each peer self-registers one task and owns its full worktree, test,
  integration, main verification, push, cleanup, and resource lifecycle.
- Task owner, resource holder, integration lease, and daemon operator are
  scoped capabilities, never durable identity roles.
- Peers send no normal progress reports. P2P communication is limited to
  durable resource occupancy and release coordination.

## Task lifecycle

```
working -> verifying -> reviewed -> delivered
        -> accepted -> integrated/merged -> cleanup_pending
        -> cleanup_verified -> closed
        -> rework -> working
blocked -> bounded waiting -> resource release/timeout -> owner recheck
```

Task records use a fixed shape:
`id / owner / feature_id / worktree_path / branch / base_commit / priority /
 status`. Normal statuses are `working`, `blocked`, `waiting`, `verifying`,
`reviewed`, `delivered`, `accepted`, `rework`, `merged`, `closed`, and
`cancelled`.

## Common commands

```sh
collab up                         # clear explicit down and start daemon
collab down                       # explicit stop; disables auto-restart
collab who                        # registered peers + local state projection
collab task status [task-id]      # durable task registry
collab notify methods             # discover opt-in notification methods
collab notify subscribe --event direct-message --ttl-seconds 600
collab notify status
collab context                    # single automatic state entry: baseline/daemon/identity/registration + role operations
collab master status              # live master, or recorded-but-dead identity
collab master promote --approval "<user text>"
collab master delegate <peer>     # live master only
collab task register <id> --feature <feature-id> --worktree <path> \
  --branch <branch> --base-commit <sha> --priority p2
collab task wait <id> --for <blocking-task>
collab task deliver <id> --evidence "commit=<sha>; gates=pass" --worktree <path>
collab task block <id> --next "blocked: <evidence and next condition>"
collab task review <id> --accept --evidence "review gates=pass"
collab task integrated <id> --commit <main-sha> --evidence "main gates=pass"
collab task close <id>            # owner; verifies merged/clean, releases claim
collab task close <id> --force --reason "..."  # master/approved fallback close
```

`collab context` is the single automatic agent state entry. It resolves the
canonical project root (live route, registered route, or local baseline),
creates a missing `.agent-collab` baseline, starts the daemon unless an explicit
`DOWN` marker exists, restores/registers the peer identity, and returns the
server snapshot plus `operations`. The default agent flow does not start with
`collab master status`, `appsdk init .`, `collab down`/`up`, or
`collab route resolve`; those remain explicit human diagnostics only.

Live master dispatch uses `collab subagent dispatch --request-id <id>
--subject <topic> "<body>"` with optional
`--feature-id/--worktree-path/--branch/--base-commit/--priority/--next-step`.
`--request-id` is ASCII `[A-Za-z0-9_-]`, max 80 bytes, and idempotent.
`--worktree-path` must be `<project-main>/playground/<short-slug>` (leaf max 32
ASCII bytes, no `..`). Only the live registered master can dispatch; the
selected peer must be registered, present, non-managed, and inactive. Success
returns `request_id`, `message_id`, `task_id`, `target`, `status: assigned`,
`admission.*`, and `notification` (`sent` | `subscribed-not-sent` |
`mailbox-only-no-subscription`). `sent` is not consumption; verify with
`collab msg <id>` (`consumed_by_recv`) and `collab task status <task-id>`.

Peers never share worktrees. Each task owner starts from latest main in one
declared clean `./playground/` worktree, implements and tests, commits the exact
change set, syncs latest main again, verifies the candidate, acquires a short
integration lease, merges the exact commit to main, verifies and pushes main,
then closes the task to remove only its clean merged worktree/branch and persist
a cleanup receipt. A bound worktree is a mandatory cleanup obligation;
`delivered`/`merged` are not cleanup completion, and a task with a pending or
unproven cleanup cannot become closed or pass audit. Delivery is an owner-local
durable milestone and sends no peer notification. `/goal`
delegation and interactive task recognition are intentionally deferred.

## Message handling

On a notification, use its id and abbreviated subject to weigh urgency against
the current task. Query durable state before acting when the notice is relevant.
`collab sendmessage` requires `--subject` and accepts only explicit coordination
or asynchronous-result notices. Never type peer messages into a terminal. After the
receiving Agent registers a finite subscription, the daemon may send one id,
abbreviated subject, safe one-line original body preview, and final submit as
one App Server immediate turn submission. The direct-message lease is reusable
until expiry; resource and deadline subscriptions remain one-shot.

`collab inbox` and `collab msg <id>` query the durable local mailbox after a
registered App Server thread becomes unavailable; mailbox state remains
authoritative.

## Notifications and waits

There is no periodic continuation. Agent-owned subscriptions are exact-event,
exact-subject, and finite. Direct-message delivery is serialized and reusable
until expiry; other subscriptions are one-shot. No registration, absent,
unknown, working, expired, cancelled, consumed, or exhausted message produces
App Server input. Every wait stores waiter, blocking task owner, reason, deadline,
resume events, and P2P escalation. Timeout changes state without unsolicited
messages; resource release notifies only an exact active subscriber.
"#;

/// Scope guard used by every command except init.
#[derive(Clone)]
pub struct Scope {
    pub root: PathBuf,
}

impl Scope {
    pub fn resolve() -> anyhow::Result<Self> {
        let cwd = std::env::current_dir()?;
        Self::resolve_from_cwd(&cwd)
    }

    fn local_baseline_is_authoritative(cwd: &Path) -> anyhow::Result<bool> {
        if !cwd.join(".agent-collab").is_dir() {
            return Ok(false);
        }
        let output = Command::new("git")
            .args(["rev-parse", "--git-common-dir"])
            .current_dir(cwd)
            .output();
        let Ok(output) = output else {
            return Ok(true);
        };
        if !output.status.success() {
            return Ok(true);
        }
        let common = PathBuf::from(String::from_utf8_lossy(&output.stdout).trim());
        let common = if common.is_absolute() {
            common
        } else {
            cwd.join(common)
        };
        let common = std::fs::canonicalize(common)?;
        let worktree = std::fs::canonicalize(
            String::from_utf8_lossy(
                &Command::new("git")
                    .args(["rev-parse", "--show-toplevel"])
                    .current_dir(cwd)
                    .output()?
                    .stdout,
            )
            .trim(),
        )?;
        let main_root = common.parent().ok_or_else(|| {
            anyhow::anyhow!("Git common directory has no parent: {}", common.display())
        })?;
        Ok(worktree == main_root || cwd != worktree)
    }

    fn resolve_from_cwd(cwd: &Path) -> anyhow::Result<Self> {
        // Validate the host endpoint while resolution is still fallible.
        // The infallible compatibility accessors below are only used after
        // this check (or by isolated unit fixtures).
        let host_paths = HostPaths::resolve()?;
        Self::resolve_from_cwd_with_host_paths(cwd, &host_paths, None)
    }

    fn resolve_from_cwd_with_host_paths(
        cwd: &Path,
        host_paths: &HostPaths,
        worker_id: Option<String>,
    ) -> anyhow::Result<Self> {
        // A live Codex session/thread is the authoritative route key. The
        // tmux pane is only a last-resort recovery anchor when both Codex
        // runtime IDs are absent, so a tmux-hosted TUI must not prefer the
        // pane route over its native thread.
        if std::env::var_os("CODEX_THREAD_ID").is_some() {
            return Self::resolve_from_cwd_without_thread(cwd, host_paths, worker_id);
        }
        if std::env::var_os("TMUX_PANE").is_some() {
            let route = route_for_tmux_pane(host_paths)?;
            return Ok(Scope { root: route.root });
        }
        Self::resolve_from_cwd_without_thread(cwd, host_paths, worker_id)
    }

    fn resolve_from_cwd_without_thread(
        cwd: &Path,
        host_paths: &HostPaths,
        worker_id: Option<String>,
    ) -> anyhow::Result<Self> {
        if Self::local_baseline_is_authoritative(cwd)? {
            return Self::from_project_root(cwd.to_path_buf());
        }
        let identity_scope = Scope {
            root: cwd.to_path_buf(),
        };
        if let Some(identity) =
            crate::identity::load_existing_at(host_paths, &identity_scope, worker_id)?
        {
            let runtime = identity.runtime.as_ref().ok_or_else(|| {
                anyhow::anyhow!(
                    "persisted Collab identity {} has no registered runtime",
                    identity.worker_id
                )
            })?;
            let route = canonical_route_for_identity(&host_paths, &cwd, &runtime.appserver_id)?;
            return Ok(Scope { root: route.root });
        }
        anyhow::bail!("no .agent-collab found in inherited cwd {}", cwd.display())
    }

    fn from_project_root(root: PathBuf) -> anyhow::Result<Self> {
        if root.join(".agent-collab").is_dir() {
            Ok(Scope { root })
        } else {
            Err(anyhow::anyhow!(
                "no .agent-collab found in exact project root {}; run `collab init` there first",
                root.display()
            ))
        }
    }
    pub fn server_dir(&self) -> PathBuf {
        // This remains the project-local reducer/journal directory.  The
        // daemon socket and lease are exposed separately through host_paths.
        self.root.join(".agent-collab").join("server")
    }

    pub fn host_paths(&self) -> anyhow::Result<HostPaths> {
        // A few in-process startup fixtures construct a bare `Scope` without
        // running `collab init`. Keep those fixtures isolated from the real
        // host endpoint; production scopes always have the project marker and
        // therefore use the host-wide state root below.
        #[cfg(test)]
        if !self.root.join(".agent-collab").is_dir()
            || self.server_dir().join("daemon.lock").exists()
        {
            return HostPaths::from_state_root(self.server_dir());
        }
        HostPaths::for_project(&self.root)
    }

    pub fn host_server_dir(&self) -> PathBuf {
        self.host_paths()
            .expect("Scope::resolve validates the host endpoint")
            .server_dir()
    }

    pub fn sock_path(&self) -> PathBuf {
        self.host_paths()
            .expect("Scope::resolve validates the host endpoint")
            .socket_path()
    }

    pub fn route_scope(&self, app_scope_id: AppServerId) -> anyhow::Result<RouteScope> {
        RouteScope::for_registered_project(app_scope_id, &self.root)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn test_root(name: &str) -> PathBuf {
        Path::new("/tmp").join(format!(
            "cs-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    struct EnvVarGuard {
        key: &'static str,
        previous: Option<std::ffi::OsString>,
    }

    impl EnvVarGuard {
        fn without(key: &'static str) -> Self {
            let previous = std::env::var_os(key);
            std::env::remove_var(key);
            Self { key, previous }
        }
    }

    impl Drop for EnvVarGuard {
        fn drop(&mut self) {
            if let Some(previous) = self.previous.take() {
                std::env::set_var(self.key, previous);
            }
        }
    }

    #[test]
    fn init_scope_uses_unmarked_process_cwd() {
        let cwd = test_root("init-unmarked-cwd");
        std::fs::create_dir_all(&cwd).unwrap();
        let resolved = init_project_root(cwd.clone()).unwrap();
        assert_eq!(resolved, cwd);
        assert!(!resolved.join(".agent-collab").exists());
        std::fs::remove_dir_all(resolved).ok();
    }

    #[test]
    fn init_scope_rejects_a_missing_process_cwd() {
        let missing = test_root("init-missing-cwd");
        assert!(init_project_root(missing).is_err());
    }

    #[test]
    fn exact_root_never_captures_ancestor_or_sibling_state() {
        let parent = test_root("exact-scope");
        let first = parent.join("first");
        let second = parent.join("second");
        init(&parent).unwrap();
        std::fs::create_dir_all(&first).unwrap();
        init(&second).unwrap();

        assert!(Scope::from_project_root(first).is_err());
        assert_eq!(
            Scope::from_project_root(second.clone()).unwrap().root,
            second
        );

        std::fs::remove_dir_all(parent).ok();
    }

    #[test]
    fn route_scope_uses_exact_registered_project_cwd() {
        let parent = test_root("route-scope");
        let registered = parent.join("registered");
        let sibling = parent.join("sibling");
        std::fs::create_dir_all(&registered).unwrap();
        std::fs::create_dir_all(&sibling).unwrap();
        let route = RouteScope::for_registered_project(
            AppServerId::new("appserver-1").unwrap(),
            &registered,
        )
        .unwrap();

        route.validate_registered_cwd(&registered).unwrap();
        assert!(route.validate_registered_cwd(&sibling).is_err());
        assert!(route.validate_registered_cwd(&parent).is_err());
        assert_eq!(
            route.project_scope_id.as_str(),
            registered.canonicalize().unwrap().to_string_lossy()
        );
        std::fs::remove_dir_all(parent).ok();
    }

    #[test]
    fn identity_route_resolves_only_for_its_app_scope_and_contains_cwd() {
        let root = test_root("worktree-route");
        let canonical = root.join("project");
        let worktree = canonical.join("playground/task-a");
        let sibling = root.join("project-other");
        let unrelated = root.join("unrelated");
        std::fs::create_dir_all(canonical.join(".agent-collab")).unwrap();
        std::fs::create_dir_all(&worktree).unwrap();
        std::fs::create_dir_all(&sibling).unwrap();
        std::fs::create_dir_all(&unrelated).unwrap();

        let state_root = root.join("host-state");
        std::fs::create_dir_all(&state_root).unwrap();
        let route = json!({
            "version": 1,
            "op": "register",
            "app_scope_id": "appserver-cli",
            "project_scope": canonical.canonicalize().unwrap(),
            "canonical_root": canonical.canonicalize().unwrap(),
            "storage_root": canonical.canonicalize().unwrap(),
            "registered_ms": 1
        });
        std::fs::write(state_root.join("routes.jsonl"), format!("{route}\n")).unwrap();

        let host_paths = HostPaths::for_state_root(&state_root).unwrap();
        let app_scope = AppServerId::new("appserver-cli").unwrap();
        let resolved = canonical_route_for_identity(&host_paths, &worktree, &app_scope).unwrap();
        assert_eq!(resolved.root, canonical.canonicalize().unwrap());
        assert_eq!(resolved.app_scope_id.as_str(), "appserver-cli");
        assert!(canonical_route_for_identity(
            &host_paths,
            &unrelated,
            &AppServerId::new("appserver-cli").unwrap()
        )
        .is_err());
        assert!(canonical_route_for_identity(
            &host_paths,
            &sibling,
            &AppServerId::new("appserver-cli").unwrap()
        )
        .is_err());
        assert!(canonical_route_for_identity(
            &host_paths,
            &worktree,
            &AppServerId::new("appserver-other").unwrap()
        )
        .is_err());

        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn scope_resolve_reuses_identity_route_from_a_worktree() {
        let _env_guard = TEST_ENV_LOCK.lock().unwrap();
        let _tmux_env_guard = EnvVarGuard::without("TMUX_PANE");
        let previous_thread = std::env::var_os("CODEX_THREAD_ID");
        std::env::remove_var("CODEX_THREAD_ID");
        let root = test_root("scope-worktree-identity");
        let canonical = root.join("project");
        let worktree = canonical.join("playground/task-a");
        let state_root = root.join("host-state");
        std::fs::create_dir_all(canonical.join(".agent-collab")).unwrap();
        std::fs::create_dir_all(&state_root).unwrap();

        let status = Command::new("git")
            .args(["init", "-q", "-b", "main"])
            .current_dir(&canonical)
            .status()
            .unwrap();
        assert!(status.success());
        let status = Command::new("git")
            .args([
                "-c",
                "user.name=Collab Test",
                "-c",
                "user.email=collab-test@example.invalid",
                "commit",
                "--allow-empty",
                "-q",
                "-m",
                "initial",
            ])
            .current_dir(&canonical)
            .status()
            .unwrap();
        assert!(status.success());
        let status = Command::new("git")
            .args([
                "worktree",
                "add",
                "-q",
                "-b",
                "task-a",
                worktree.to_str().unwrap(),
                "main",
            ])
            .current_dir(&canonical)
            .status()
            .unwrap();
        assert!(status.success());
        std::fs::create_dir_all(worktree.join(".agent-collab")).unwrap();

        let canonical = canonical.canonicalize().unwrap();
        let worktree = worktree.canonicalize().unwrap();
        let route = json!({
            "version": 1,
            "op": "register",
            "app_scope_id": "appserver-cli",
            "project_scope": canonical,
            "canonical_root": canonical,
            "storage_root": canonical,
            "registered_ms": 1
        });
        std::fs::write(state_root.join("routes.jsonl"), format!("{route}\n")).unwrap();
        let identity_dir = state_root.join("identities/worker-a");
        std::fs::create_dir_all(&identity_dir).unwrap();
        std::fs::write(
            identity_dir.join("identity.json"),
            json!({
                "worker_id": "worker-a",
                "token": "token-a",
                "runtime": {
                    "agent_id": "worker-a",
                    "runtime_id": "runtime-a",
                    "appserver_id": "appserver-cli",
                    "endpoint_generation": 1,
                    "binding_id": "binding-a",
                    "native_thread_id": "thread-a"
                },
                "transport": {
                    "kind": "appserver",
                    "endpoint": "unix:///tmp/test.sock",
                    "namespace": "codex_tui",
                    "thread_id": "thread-a",
                    "capabilities": [],
                    "self_check": "test"
                }
            })
            .to_string(),
        )
        .unwrap();

        let host_paths = HostPaths::for_state_root(&state_root).unwrap();
        let resolved = Scope::resolve_from_cwd_with_host_paths(
            &worktree,
            &host_paths,
            Some("worker-a".into()),
        )
        .unwrap();

        assert_eq!(resolved.root, canonical);
        match previous_thread {
            Some(value) => std::env::set_var("CODEX_THREAD_ID", value),
            None => std::env::remove_var("CODEX_THREAD_ID"),
        }
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn lifecycle_scope_uses_the_exact_cwd_without_requiring_a_baseline() {
        let root = test_root("lifecycle-exact-cwd");
        let initialized = root.join("initialized");
        let uninitialized = root.join("uninitialized");
        std::fs::create_dir_all(initialized.join(".agent-collab")).unwrap();
        std::fs::create_dir_all(&uninitialized).unwrap();

        assert_eq!(
            validate_project_root(initialized.clone()).unwrap(),
            initialized
        );
        assert_eq!(
            validate_project_root(uninitialized.clone()).unwrap(),
            uninitialized
        );

        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn recovery_scope_uses_canonical_route_without_a_native_thread_route() {
        let _env_guard = TEST_ENV_LOCK.lock().unwrap();
        let _tmux_env_guard = EnvVarGuard::without("TMUX_PANE");
        let previous_cwd = std::env::current_dir().unwrap();
        let previous_thread = std::env::var_os("CODEX_THREAD_ID");
        let previous_state = std::env::var_os(COLLAB_STATE_DIR_ENV);
        let root = test_root("recovery-worktree-without-route");
        let canonical = root.join("project");
        let worktree = canonical.join("playground/task-a");
        let state_root = root.join("host-state");
        std::fs::create_dir_all(canonical.join(".agent-collab")).unwrap();
        std::fs::create_dir_all(&state_root).unwrap();
        let status = Command::new("git")
            .args(["init", "-q", "-b", "main"])
            .current_dir(&canonical)
            .status()
            .unwrap();
        assert!(status.success());
        let status = Command::new("git")
            .args([
                "-c",
                "user.name=Collab Test",
                "-c",
                "user.email=collab-test@example.invalid",
                "commit",
                "--allow-empty",
                "-q",
                "-m",
                "initial",
            ])
            .current_dir(&canonical)
            .status()
            .unwrap();
        assert!(status.success());
        let status = Command::new("git")
            .args([
                "worktree",
                "add",
                "-q",
                "-b",
                "task-a",
                worktree.to_str().unwrap(),
                "main",
            ])
            .current_dir(&canonical)
            .status()
            .unwrap();
        assert!(status.success());

        let canonical = canonical.canonicalize().unwrap();
        let worktree = worktree.canonicalize().unwrap();
        let route = json!({
            "version": 1,
            "op": "register",
            "app_scope_id": "appserver-cli",
            "project_scope": canonical,
            "canonical_root": canonical,
            "storage_root": canonical,
            "registered_ms": 1
        });
        std::fs::write(state_root.join("routes.jsonl"), format!("{route}\n")).unwrap();
        std::env::set_var(COLLAB_STATE_DIR_ENV, &state_root);
        std::env::set_current_dir(&worktree).unwrap();
        std::env::set_var("CODEX_THREAD_ID", "missing-native-route");

        let resolved = resolve_for_recovery().unwrap();

        assert_eq!(resolved.root, canonical);
        std::env::set_current_dir(previous_cwd).unwrap();
        match previous_thread {
            Some(value) => std::env::set_var("CODEX_THREAD_ID", value),
            None => std::env::remove_var("CODEX_THREAD_ID"),
        }
        match previous_state {
            Some(value) => std::env::set_var(COLLAB_STATE_DIR_ENV, value),
            None => std::env::remove_var(COLLAB_STATE_DIR_ENV),
        }
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn scope_resolve_prefers_a_fresh_local_baseline_over_a_stale_identity_route() {
        let _env_guard = TEST_ENV_LOCK.lock().unwrap();
        let _tmux_env_guard = EnvVarGuard::without("TMUX_PANE");
        let previous_thread = std::env::var_os("CODEX_THREAD_ID");
        std::env::remove_var("CODEX_THREAD_ID");
        let root = test_root("scope-fresh-reset");
        let project = root.join("project");
        let state_root = root.join("host-state");
        std::fs::create_dir_all(project.join(".agent-collab")).unwrap();
        std::fs::create_dir_all(&state_root).unwrap();

        let identity_dir = state_root.join("identities/worker-a");
        std::fs::create_dir_all(&identity_dir).unwrap();
        std::fs::write(
            identity_dir.join("identity.json"),
            json!({
                "worker_id": "worker-a",
                "token": "token-a",
                "runtime": {
                    "agent_id": "worker-a",
                    "runtime_id": "runtime-a",
                    "appserver_id": "appserver-cli",
                    "endpoint_generation": 1,
                    "binding_id": "binding-a",
                    "native_thread_id": "thread-a"
                },
                "transport": {
                    "kind": "appserver",
                    "endpoint": "unix:///tmp/test.sock",
                    "namespace": "codex_tui",
                    "thread_id": "thread-a",
                    "capabilities": [],
                    "self_check": "test"
                }
            })
            .to_string(),
        )
        .unwrap();

        let host_paths = HostPaths::for_state_root(&state_root).unwrap();
        let resolved =
            Scope::resolve_from_cwd_with_host_paths(&project, &host_paths, Some("worker-a".into()))
                .unwrap();

        assert_eq!(resolved.root, project);
        match previous_thread {
            Some(value) => std::env::set_var("CODEX_THREAD_ID", value),
            None => std::env::remove_var("CODEX_THREAD_ID"),
        }
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn scope_resolve_preserves_a_nested_project_inside_a_linked_worktree() {
        let _env_guard = TEST_ENV_LOCK.lock().unwrap();
        let _tmux_env_guard = EnvVarGuard::without("TMUX_PANE");
        let previous_thread = std::env::var_os("CODEX_THREAD_ID");
        std::env::remove_var("CODEX_THREAD_ID");
        let root = test_root("scope-nested-worktree");
        let canonical = root.join("project");
        let worktree = canonical.join("playground/task-a");
        let nested = worktree.join("services/service-a");
        let state_root = root.join("host-state");
        std::fs::create_dir_all(canonical.join(".agent-collab")).unwrap();
        std::fs::create_dir_all(&state_root).unwrap();

        let status = Command::new("git")
            .args(["init", "-q", "-b", "main"])
            .current_dir(&canonical)
            .status()
            .unwrap();
        assert!(status.success());
        let status = Command::new("git")
            .args([
                "-c",
                "user.name=Collab Test",
                "-c",
                "user.email=collab-test@example.invalid",
                "commit",
                "--allow-empty",
                "-q",
                "-m",
                "initial",
            ])
            .current_dir(&canonical)
            .status()
            .unwrap();
        assert!(status.success());
        let status = Command::new("git")
            .args([
                "worktree",
                "add",
                "-q",
                "-b",
                "task-a",
                worktree.to_str().unwrap(),
                "main",
            ])
            .current_dir(&canonical)
            .status()
            .unwrap();
        assert!(status.success());
        std::fs::create_dir_all(nested.join(".agent-collab")).unwrap();
        let nested = nested.canonicalize().unwrap();

        let host_paths = HostPaths::for_state_root(&state_root).unwrap();
        let resolved = Scope::resolve_from_cwd_with_host_paths(&nested, &host_paths, None).unwrap();

        assert_eq!(resolved.root, nested);
        match previous_thread {
            Some(value) => std::env::set_var("CODEX_THREAD_ID", value),
            None => std::env::remove_var("CODEX_THREAD_ID"),
        }
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn read_only_route_resolution_preserves_non_cli_scope_from_a_linked_worktree() {
        let root = test_root("read-only-worktree-route");
        let canonical = root.join("project");
        let worktree = canonical.join("playground/task-a");
        let state_root = root.join("host-state");
        std::fs::create_dir_all(canonical.join(".agent-collab")).unwrap();
        std::fs::create_dir_all(&state_root).unwrap();

        let status = Command::new("git")
            .args(["init", "-q", "-b", "main"])
            .current_dir(&canonical)
            .status()
            .unwrap();
        assert!(status.success());
        let status = Command::new("git")
            .args([
                "-c",
                "user.name=Collab Test",
                "-c",
                "user.email=collab-test@example.invalid",
                "commit",
                "--allow-empty",
                "-q",
                "-m",
                "initial",
            ])
            .current_dir(&canonical)
            .status()
            .unwrap();
        assert!(status.success());
        let status = Command::new("git")
            .args([
                "worktree",
                "add",
                "-q",
                "-b",
                "task-a",
                worktree.to_str().unwrap(),
                "main",
            ])
            .current_dir(&canonical)
            .status()
            .unwrap();
        assert!(status.success());

        let canonical = canonical.canonicalize().unwrap();
        let worktree = worktree.canonicalize().unwrap();
        let route = json!({
            "version": 1,
            "op": "register",
            "app_scope_id": "appserver-vscode",
            "project_scope": canonical,
            "canonical_root": canonical,
            "storage_root": canonical,
            "registered_ms": 1
        });
        std::fs::write(state_root.join("routes.jsonl"), format!("{route}\n")).unwrap();

        let host_paths = HostPaths::for_state_root(&state_root).unwrap();
        let resolved = canonical_route_for_cwd(&host_paths, &worktree).unwrap();

        assert_eq!(resolved.root, canonical);
        assert_eq!(resolved.app_scope_id.as_str(), "appserver-vscode");
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn read_only_route_resolution_keeps_a_nested_project_inside_a_linked_worktree() {
        let root = test_root("read-only-nested-worktree-route");
        let canonical = root.join("project");
        let worktree = canonical.join("playground/task-a");
        let nested = worktree.join("services/service-a");
        let state_root = root.join("host-state");
        std::fs::create_dir_all(canonical.join(".agent-collab")).unwrap();
        std::fs::create_dir_all(&state_root).unwrap();

        let status = Command::new("git")
            .args(["init", "-q", "-b", "main"])
            .current_dir(&canonical)
            .status()
            .unwrap();
        assert!(status.success());
        let status = Command::new("git")
            .args([
                "-c",
                "user.name=Collab Test",
                "-c",
                "user.email=collab-test@example.invalid",
                "commit",
                "--allow-empty",
                "-q",
                "-m",
                "initial",
            ])
            .current_dir(&canonical)
            .status()
            .unwrap();
        assert!(status.success());
        let status = Command::new("git")
            .args([
                "worktree",
                "add",
                "-q",
                "-b",
                "task-a",
                worktree.to_str().unwrap(),
                "main",
            ])
            .current_dir(&canonical)
            .status()
            .unwrap();
        assert!(status.success());
        std::fs::create_dir_all(nested.join(".agent-collab")).unwrap();

        let canonical = canonical.canonicalize().unwrap();
        let nested = nested.canonicalize().unwrap();
        let main_route = json!({
            "version": 1,
            "op": "register",
            "app_scope_id": "appserver-cli",
            "project_scope": canonical,
            "canonical_root": canonical,
            "storage_root": canonical,
            "registered_ms": 1
        });
        let nested_route = json!({
            "version": 1,
            "op": "register",
            "app_scope_id": "appserver-cli",
            "project_scope": nested,
            "canonical_root": nested,
            "storage_root": nested,
            "registered_ms": 2
        });
        std::fs::write(
            state_root.join("routes.jsonl"),
            format!("{main_route}\n{nested_route}\n"),
        )
        .unwrap();

        let host_paths = HostPaths::for_state_root(&state_root).unwrap();
        let resolved = canonical_route_for_cwd(&host_paths, &nested).unwrap();

        assert_eq!(resolved.root, nested);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn read_only_route_resolution_rejects_a_worktree_only_route() {
        let root = test_root("read-only-worktree-only-route");
        let canonical = root.join("project");
        let worktree = canonical.join("playground/task-a");
        let state_root = root.join("host-state");
        std::fs::create_dir_all(canonical.join(".agent-collab")).unwrap();
        std::fs::create_dir_all(&state_root).unwrap();

        let status = Command::new("git")
            .args(["init", "-q", "-b", "main"])
            .current_dir(&canonical)
            .status()
            .unwrap();
        assert!(status.success());
        let status = Command::new("git")
            .args([
                "-c",
                "user.name=Collab Test",
                "-c",
                "user.email=collab-test@example.invalid",
                "commit",
                "--allow-empty",
                "-q",
                "-m",
                "initial",
            ])
            .current_dir(&canonical)
            .status()
            .unwrap();
        assert!(status.success());
        let status = Command::new("git")
            .args([
                "worktree",
                "add",
                "-q",
                "-b",
                "task-a",
                worktree.to_str().unwrap(),
                "main",
            ])
            .current_dir(&canonical)
            .status()
            .unwrap();
        assert!(status.success());
        let worktree = worktree.canonicalize().unwrap();
        std::fs::create_dir_all(worktree.join(".agent-collab")).unwrap();
        let route = json!({
            "version": 1,
            "op": "register",
            "app_scope_id": "appserver-cli",
            "project_scope": worktree,
            "canonical_root": worktree,
            "storage_root": worktree,
            "registered_ms": 1
        });
        std::fs::write(state_root.join("routes.jsonl"), format!("{route}\n")).unwrap();

        let host_paths = HostPaths::for_state_root(&state_root).unwrap();
        let error = canonical_route_for_cwd(&host_paths, &worktree).unwrap_err();
        assert!(
            error.to_string().contains("no registered Collab route"),
            "unexpected error: {error}"
        );
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn identity_route_prefers_git_main_root_when_worktree_route_is_stale() {
        let root = test_root("worktree-route-disambiguation");
        let canonical = root.join("project");
        let worktree = canonical.join("playground/task-a");
        let state_root = root.join("host-state");
        std::fs::create_dir_all(canonical.join(".agent-collab")).unwrap();
        std::fs::create_dir_all(&worktree).unwrap();
        std::fs::create_dir_all(&state_root).unwrap();

        let status = Command::new("git")
            .args(["init", "-q", "-b", "main"])
            .current_dir(&canonical)
            .status()
            .unwrap();
        assert!(status.success());
        let status = Command::new("git")
            .args([
                "-c",
                "user.name=Collab Test",
                "-c",
                "user.email=collab-test@example.invalid",
                "commit",
                "--allow-empty",
                "-q",
                "-m",
                "initial",
            ])
            .current_dir(&canonical)
            .status()
            .unwrap();
        assert!(status.success());
        let status = Command::new("git")
            .args([
                "worktree",
                "add",
                "-q",
                "-b",
                "task-a",
                worktree.to_str().unwrap(),
                "main",
            ])
            .current_dir(&canonical)
            .status()
            .unwrap();
        assert!(status.success());

        let canonical = canonical.canonicalize().unwrap();
        let worktree = worktree.canonicalize().unwrap();
        let canonical_route = json!({
            "version": 1,
            "op": "register",
            "app_scope_id": "appserver-cli",
            "project_scope": canonical,
            "canonical_root": canonical,
            "storage_root": canonical,
            "registered_ms": 1
        });
        std::fs::write(
            state_root.join("routes.jsonl"),
            format!("{canonical_route}\n"),
        )
        .unwrap();

        let host_paths = HostPaths::for_state_root(&state_root).unwrap();
        let app_scope = AppServerId::new("appserver-cli").unwrap();
        let resolved = canonical_route_for_identity(&host_paths, &worktree, &app_scope).unwrap();

        assert_eq!(resolved.root, canonical);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn identity_route_rejects_a_worktree_only_route() {
        let root = test_root("worktree-route-only");
        let canonical = root.join("project");
        let worktree = canonical.join("playground/task-a");
        let state_root = root.join("host-state");
        std::fs::create_dir_all(canonical.join(".agent-collab")).unwrap();
        std::fs::create_dir_all(&worktree).unwrap();
        std::fs::create_dir_all(&state_root).unwrap();

        let status = Command::new("git")
            .args(["init", "-q", "-b", "main"])
            .current_dir(&canonical)
            .status()
            .unwrap();
        assert!(status.success());
        let status = Command::new("git")
            .args([
                "-c",
                "user.name=Collab Test",
                "-c",
                "user.email=collab-test@example.invalid",
                "commit",
                "--allow-empty",
                "-q",
                "-m",
                "initial",
            ])
            .current_dir(&canonical)
            .status()
            .unwrap();
        assert!(status.success());
        let status = Command::new("git")
            .args([
                "worktree",
                "add",
                "-q",
                "-b",
                "task-a",
                worktree.to_str().unwrap(),
                "main",
            ])
            .current_dir(&canonical)
            .status()
            .unwrap();
        assert!(status.success());

        let worktree = worktree.canonicalize().unwrap();
        std::fs::create_dir_all(worktree.join(".agent-collab")).unwrap();
        let worktree_route = json!({
            "version": 1,
            "op": "register",
            "app_scope_id": "appserver-cli",
            "project_scope": worktree,
            "canonical_root": worktree,
            "storage_root": worktree,
            "registered_ms": 1
        });
        std::fs::write(
            state_root.join("routes.jsonl"),
            format!("{worktree_route}\n"),
        )
        .unwrap();

        let host_paths = HostPaths::for_state_root(&state_root).unwrap();
        let app_scope = AppServerId::new("appserver-cli").unwrap();
        let error = canonical_route_for_identity(&host_paths, &worktree, &app_scope).unwrap_err();
        assert!(
            error.to_string().contains("Git main worktree"),
            "unexpected error: {error}"
        );
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn identity_route_selects_canonical_main_when_worktree_route_also_exists() {
        let root = test_root("worktree-route-both");
        let canonical = root.join("project");
        let worktree = canonical.join("playground/task-a");
        let state_root = root.join("host-state");
        std::fs::create_dir_all(canonical.join(".agent-collab")).unwrap();
        std::fs::create_dir_all(&worktree).unwrap();
        std::fs::create_dir_all(&state_root).unwrap();

        let status = Command::new("git")
            .args(["init", "-q", "-b", "main"])
            .current_dir(&canonical)
            .status()
            .unwrap();
        assert!(status.success());
        let status = Command::new("git")
            .args([
                "-c",
                "user.name=Collab Test",
                "-c",
                "user.email=collab-test@example.invalid",
                "commit",
                "--allow-empty",
                "-q",
                "-m",
                "initial",
            ])
            .current_dir(&canonical)
            .status()
            .unwrap();
        assert!(status.success());
        let status = Command::new("git")
            .args([
                "worktree",
                "add",
                "-q",
                "-b",
                "task-a",
                worktree.to_str().unwrap(),
                "main",
            ])
            .current_dir(&canonical)
            .status()
            .unwrap();
        assert!(status.success());

        let canonical = canonical.canonicalize().unwrap();
        let worktree = worktree.canonicalize().unwrap();
        std::fs::create_dir_all(worktree.join(".agent-collab")).unwrap();
        let canonical_route = json!({
            "version": 1,
            "op": "register",
            "app_scope_id": "appserver-cli",
            "project_scope": canonical,
            "canonical_root": canonical,
            "storage_root": canonical,
            "registered_ms": 1
        });
        let worktree_route = json!({
            "version": 1,
            "op": "register",
            "app_scope_id": "appserver-cli",
            "project_scope": worktree,
            "canonical_root": worktree,
            "storage_root": worktree,
            "registered_ms": 2
        });
        std::fs::write(
            state_root.join("routes.jsonl"),
            format!("{worktree_route}\n{canonical_route}\n"),
        )
        .unwrap();

        let host_paths = HostPaths::for_state_root(&state_root).unwrap();
        let app_scope = AppServerId::new("appserver-cli").unwrap();
        let resolved = canonical_route_for_identity(&host_paths, &worktree, &app_scope).unwrap();

        assert_eq!(resolved.root, canonical);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn identity_route_preserves_a_nested_project_root() {
        let root = test_root("nested-project-route");
        let repository = root.join("repo");
        let project = repository.join("services/service-a");
        let worktree = project.join("playground/task-a");
        let state_root = root.join("host-state");
        std::fs::create_dir_all(project.join(".agent-collab")).unwrap();
        std::fs::create_dir_all(&worktree).unwrap();
        std::fs::create_dir_all(&state_root).unwrap();

        let status = Command::new("git")
            .args(["init", "-q", "-b", "main"])
            .current_dir(&repository)
            .status()
            .unwrap();
        assert!(status.success());
        let status = Command::new("git")
            .args([
                "-c",
                "user.name=Collab Test",
                "-c",
                "user.email=collab-test@example.invalid",
                "commit",
                "--allow-empty",
                "-q",
                "-m",
                "initial",
            ])
            .current_dir(&repository)
            .status()
            .unwrap();
        assert!(status.success());
        let status = Command::new("git")
            .args([
                "worktree",
                "add",
                "-q",
                "-b",
                "task-a",
                worktree.to_str().unwrap(),
                "main",
            ])
            .current_dir(&repository)
            .status()
            .unwrap();
        assert!(status.success());

        let project = project.canonicalize().unwrap();
        let worktree = worktree.canonicalize().unwrap();
        std::fs::create_dir_all(worktree.join(".agent-collab")).unwrap();
        let route = json!({
            "version": 1,
            "op": "register",
            "app_scope_id": "appserver-cli",
            "project_scope": project,
            "canonical_root": project,
            "storage_root": project,
            "registered_ms": 1
        });
        std::fs::write(state_root.join("routes.jsonl"), format!("{route}\n")).unwrap();

        let host_paths = HostPaths::for_state_root(&state_root).unwrap();
        let app_scope = AppServerId::new("appserver-cli").unwrap();
        let resolved = canonical_route_for_identity(&host_paths, &worktree, &app_scope).unwrap();

        assert_eq!(resolved.root, project);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn identity_route_preserves_a_submodule_root() {
        let root = test_root("submodule-project-route");
        let superproject = root.join("superproject");
        let submodule = superproject.join("modules/service-a");
        let state_root = root.join("host-state");
        std::fs::create_dir_all(&submodule).unwrap();
        std::fs::create_dir_all(&state_root).unwrap();

        let status = Command::new("git")
            .args(["init", "-q", "-b", "main"])
            .current_dir(&superproject)
            .status()
            .unwrap();
        assert!(status.success());
        let status = Command::new("git")
            .args([
                "-c",
                "user.name=Collab Test",
                "-c",
                "user.email=collab-test@example.invalid",
                "commit",
                "--allow-empty",
                "-q",
                "-m",
                "initial superproject",
            ])
            .current_dir(&superproject)
            .status()
            .unwrap();
        assert!(status.success());

        let status = Command::new("git")
            .args(["init", "-q", "-b", "main"])
            .current_dir(&submodule)
            .status()
            .unwrap();
        assert!(status.success());
        let status = Command::new("git")
            .args([
                "-c",
                "user.name=Collab Test",
                "-c",
                "user.email=collab-test@example.invalid",
                "commit",
                "--allow-empty",
                "-q",
                "-m",
                "initial submodule",
            ])
            .current_dir(&submodule)
            .status()
            .unwrap();
        assert!(status.success());
        let status = Command::new("git")
            .args([
                "-c",
                "protocol.file.allow=always",
                "submodule",
                "add",
                submodule.to_str().unwrap(),
                "modules/service-a",
            ])
            .current_dir(&superproject)
            .status()
            .unwrap();
        assert!(status.success());

        let submodule = submodule.canonicalize().unwrap();
        std::fs::create_dir_all(submodule.join(".agent-collab")).unwrap();
        let route = json!({
            "version": 1,
            "op": "register",
            "app_scope_id": "appserver-cli",
            "project_scope": submodule,
            "canonical_root": submodule,
            "storage_root": submodule,
            "registered_ms": 1
        });
        std::fs::write(state_root.join("routes.jsonl"), format!("{route}\n")).unwrap();

        let host_paths = HostPaths::for_state_root(&state_root).unwrap();
        let app_scope = AppServerId::new("appserver-cli").unwrap();
        let resolved = canonical_route_for_identity(&host_paths, &submodule, &app_scope).unwrap();

        assert_eq!(resolved.root, submodule);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn route_scope_serializes_two_levels_and_does_not_mutate_paths() {
        let root = test_root("route-serialization");
        std::fs::create_dir_all(&root).unwrap();
        let route =
            RouteScope::for_registered_project(AppServerId::new("appserver-1").unwrap(), &root)
                .unwrap();
        let before = route.clone();
        let encoded = serde_json::to_value(&route).unwrap();
        assert_eq!(encoded["app_scope_id"], "appserver-1");
        assert_eq!(
            encoded["project_scope_id"],
            root.canonicalize().unwrap().to_string_lossy().as_ref()
        );
        assert_eq!(
            serde_json::from_value::<RouteScope>(encoded).unwrap(),
            route
        );
        assert_eq!(route, before);
        std::fs::remove_dir_all(root).ok();
    }

    #[cfg(unix)]
    #[test]
    fn non_utf8_registered_cwd_fails_closed_without_scope_collision() {
        use std::ffi::OsString;
        use std::os::unix::ffi::OsStringExt;

        let parent = test_root("non-utf8-route-scope");
        std::fs::create_dir_all(&parent).unwrap();
        let first = parent.join(OsString::from_vec(b"project-\xff".to_vec()));
        let second = parent.join(OsString::from_vec(b"project-\xfe".to_vec()));
        if std::fs::create_dir(&first).is_err() || std::fs::create_dir(&second).is_err() {
            std::fs::remove_dir_all(parent).ok();
            return;
        }

        assert!(RouteScope::for_registered_project(
            AppServerId::new("appserver-1").unwrap(),
            &first
        )
        .is_err());
        assert!(RouteScope::for_registered_project(
            AppServerId::new("appserver-1").unwrap(),
            &second
        )
        .is_err());
        std::fs::remove_dir_all(parent).ok();
    }

    #[test]
    fn long_registered_cwd_has_a_valid_unbounded_project_scope() {
        let base = test_root("long-route-scope");
        let mut root = base.clone();
        for index in 0..24 {
            root = root.join(format!("segment-{index:02}-abcdef"));
        }
        std::fs::create_dir_all(&root).unwrap();
        let route =
            RouteScope::for_registered_project(AppServerId::new("appserver-1").unwrap(), &root)
                .unwrap();
        assert!(route.project_scope_id.as_str().len() > 256);
        route.validate_registered_cwd(&root).unwrap();
        std::fs::remove_dir_all(base).ok();
    }

    #[test]
    fn host_endpoint_is_stable_across_project_roots() {
        let host_root = test_root("host-endpoint").join("state");
        let first_project = test_root("host-project-one");
        let second_project = test_root("host-project-two");
        std::fs::create_dir_all(&first_project).unwrap();
        std::fs::create_dir_all(&second_project).unwrap();

        let first = HostPaths::for_state_root(&host_root).unwrap();
        let second = HostPaths::for_state_root(&host_root).unwrap();
        assert_eq!(first.socket_path(), second.socket_path());
        assert_eq!(first.lock_path(), second.lock_path());
        assert_ne!(
            first.socket_path(),
            first_project.join(".agent-collab/server/server.sock")
        );
        assert_ne!(
            second.socket_path(),
            second_project.join(".agent-collab/server/server.sock")
        );

        std::fs::remove_dir_all(first_project).ok();
        std::fs::remove_dir_all(second_project).ok();
        std::fs::remove_dir_all(host_root.parent().unwrap()).ok();
    }

    #[test]
    fn host_endpoint_rejects_relative_state_roots() {
        let error = HostPaths::for_state_root("collab-state").unwrap_err();
        assert!(error.to_string().contains("absolute"));
    }

    #[test]
    fn default_host_endpoint_uses_dot_collab_in_home() {
        let home = test_root("host-home-default");
        std::fs::create_dir_all(&home).unwrap();
        let state_root =
            resolve_state_root(Some("".into()), Some(home.clone().into_os_string())).unwrap();
        assert_eq!(state_root, home.join(".collab"));
        std::fs::remove_dir_all(home).ok();
    }

    #[test]
    fn host_endpoint_rejects_split_socket_root() {
        let root = test_root("host-socket-split");
        let mut paths = HostPaths::for_state_root(&root).unwrap();
        let error = apply_endpoint_overrides(
            &mut paths,
            Some(root.join("nested").join("server.sock")),
            None,
        )
        .unwrap_err();
        assert!(error.to_string().contains("inside host state root"));
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn host_endpoint_rejects_split_lock_owner() {
        let root = test_root("host-lock-split");
        let mut paths = HostPaths::for_state_root(&root).unwrap();
        let error =
            apply_endpoint_overrides(&mut paths, None, Some(root.join("other.lock"))).unwrap_err();
        assert!(error.to_string().contains("one lock owner"));
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn init_releases_collab_doc_only_once() {
        let root = test_root("init");
        init(&root).unwrap();
        let path = root.join("docs/collab.md");
        assert!(path.exists());
        let first = std::fs::read_to_string(&path).unwrap();
        assert!(first.contains("# collab workflow"));
        for mcp in [root.join(".mcp.json")] {
            assert!(std::fs::read_to_string(&mcp)
                .unwrap()
                .contains("collab-mcp"));
        }

        init(&root).unwrap();
        let second = std::fs::read_to_string(&path).unwrap();
        assert_eq!(first, second);
        std::fs::write(
            root.join(".mcp.json"),
            r#"{"mcpServers":{"other":{"command":"keep-me"}}}"#,
        )
        .unwrap();
        init(&root).unwrap();
        let generic: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(root.join(".mcp.json")).unwrap())
                .unwrap();
        assert_eq!(generic["mcpServers"]["other"]["command"], "keep-me");
        assert!(generic["mcpServers"]["collab"]["command"]
            .as_str()
            .unwrap()
            .contains("collab-mcp"));
        let codex: toml::Value =
            toml::from_str(&std::fs::read_to_string(root.join(".codex/config.toml")).unwrap())
                .unwrap();
        assert!(codex.get("sandbox_mode").is_none());
        assert!(codex.get("approval_policy").is_none());
        assert!(codex["mcp_servers"]["collab"]["command"]
            .as_str()
            .unwrap()
            .contains("collab-mcp"));
        std::fs::write(
            root.join(".codex/config.toml"),
            "model = \"keep-me\"\nsandbox_mode = \"workspace-write\"\n",
        )
        .unwrap();
        init(&root).unwrap();
        let upgraded: toml::Value =
            toml::from_str(&std::fs::read_to_string(root.join(".codex/config.toml")).unwrap())
                .unwrap();
        assert_eq!(upgraded["model"].as_str(), Some("keep-me"));
        assert!(upgraded.get("sandbox_mode").is_none());
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn init_rejects_malformed_existing_configuration_without_overwriting_it() {
        let json_root = test_root("init-invalid-json");
        std::fs::create_dir_all(&json_root).unwrap();
        let json_path = json_root.join(".mcp.json");
        let json_before = "{\"mcpServers\": [";
        std::fs::write(&json_path, json_before).unwrap();
        let json_error = init(&json_root).unwrap_err();
        assert!(json_error.to_string().contains("invalid JSON"));
        assert_eq!(std::fs::read_to_string(&json_path).unwrap(), json_before);

        let toml_root = test_root("init-invalid-toml");
        let toml_path = toml_root.join(".codex/config.toml");
        std::fs::create_dir_all(toml_path.parent().unwrap()).unwrap();
        let toml_before = "mcp_servers = [";
        std::fs::write(&toml_path, toml_before).unwrap();
        let toml_error = init(&toml_root).unwrap_err();
        assert!(toml_error.to_string().contains("invalid TOML"));
        assert_eq!(std::fs::read_to_string(&toml_path).unwrap(), toml_before);

        let claude_root = test_root("init-invalid-claude");
        let claude_path = claude_root.join(".claude/settings.json");
        std::fs::create_dir_all(claude_path.parent().unwrap()).unwrap();
        let claude_before = "{\"permissions\": [";
        std::fs::write(&claude_path, claude_before).unwrap();
        let claude_error = init(&claude_root).unwrap_err();
        assert!(claude_error.to_string().contains("invalid JSON"));
        assert_eq!(
            std::fs::read_to_string(&claude_path).unwrap(),
            claude_before
        );

        let json_shape_root = test_root("init-invalid-mcp-shape");
        std::fs::create_dir_all(&json_shape_root).unwrap();
        let json_shape_path = json_shape_root.join(".mcp.json");
        let json_shape_before = "{\"mcpServers\": []}";
        std::fs::write(&json_shape_path, json_shape_before).unwrap();
        let json_shape_error = init(&json_shape_root).unwrap_err();
        assert!(json_shape_error.to_string().contains("must be an object"));
        assert_eq!(
            std::fs::read_to_string(&json_shape_path).unwrap(),
            json_shape_before
        );

        let toml_shape_root = test_root("init-invalid-mcp-table");
        let toml_shape_path = toml_shape_root.join(".codex/config.toml");
        std::fs::create_dir_all(toml_shape_path.parent().unwrap()).unwrap();
        let toml_shape_before = "mcp_servers = []\n";
        std::fs::write(&toml_shape_path, toml_shape_before).unwrap();
        let toml_shape_error = init(&toml_shape_root).unwrap_err();
        assert!(toml_shape_error.to_string().contains("must be a table"));
        assert_eq!(
            std::fs::read_to_string(&toml_shape_path).unwrap(),
            toml_shape_before
        );

        std::fs::remove_dir_all(json_root).ok();
        std::fs::remove_dir_all(toml_root).ok();
        std::fs::remove_dir_all(claude_root).ok();
        std::fs::remove_dir_all(json_shape_root).ok();
        std::fs::remove_dir_all(toml_shape_root).ok();
    }

    #[test]
    fn identity_route_rejects_malformed_control_records() {
        let root = test_root("worktree-route-invalid");
        let canonical = root.join("project");
        let worktree = canonical.join("playground/task-a");
        let state_root = root.join("host-state");
        std::fs::create_dir_all(canonical.join(".agent-collab")).unwrap();
        std::fs::create_dir_all(&worktree).unwrap();
        std::fs::create_dir_all(&state_root).unwrap();
        let canonical = canonical.canonicalize().unwrap();

        let valid = json!({
            "version": 1,
            "op": "register",
            "app_scope_id": "appserver-cli",
            "project_scope": canonical,
            "canonical_root": canonical,
            "storage_root": canonical,
            "registered_ms": 1
        });
        let cases = [
            json!({
                "version": 1,
                "op": "register",
                "app_scope_id": "appserver-cli",
                "project_scope": canonical,
                "canonical_root": canonical,
                "registered_ms": 1
            }),
            json!({
                "version": 1,
                "op": "register",
                "app_scope_id": "appserver-cli",
                "project_scope": canonical,
                "canonical_root": canonical,
                "storage_root": canonical,
                "registered_ms": 1,
                "unknown": true
            }),
            json!({
                "version": 1,
                "op": "register",
                "app_scope_id": "appserver-cli",
                "project_scope": root.join("other-project"),
                "canonical_root": canonical,
                "storage_root": canonical,
                "registered_ms": 1
            }),
        ];

        for invalid in cases {
            std::fs::write(
                state_root.join("routes.jsonl"),
                format!("{invalid}\n{valid}\n"),
            )
            .unwrap();
            let host_paths = HostPaths::for_state_root(&state_root).unwrap();
            let app_scope = AppServerId::new("appserver-cli").unwrap();
            assert!(canonical_route_for_identity(&host_paths, &worktree, &app_scope).is_err());
        }

        std::fs::write(
            state_root.join("routes.jsonl"),
            format!("{valid}\n{valid}\n"),
        )
        .unwrap();
        let host_paths = HostPaths::for_state_root(&state_root).unwrap();
        let app_scope = AppServerId::new("appserver-cli").unwrap();
        assert!(canonical_route_for_identity(&host_paths, &worktree, &app_scope).is_err());

        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn identity_route_ignores_missing_stale_root_and_keeps_current_route() {
        let root = test_root("worktree-route-missing-root");
        let canonical = root.join("project");
        let worktree = canonical.join("playground/task-a");
        let missing = root.join("removed-worktree");
        let state_root = root.join("host-state");
        std::fs::create_dir_all(canonical.join(".agent-collab")).unwrap();
        std::fs::create_dir_all(&worktree).unwrap();
        std::fs::create_dir_all(&state_root).unwrap();
        let canonical = canonical.canonicalize().unwrap();

        let stale = json!({
            "version": 1,
            "op": "register",
            "app_scope_id": "appserver-cli",
            "project_scope": missing,
            "canonical_root": missing,
            "storage_root": missing,
            "registered_ms": 1
        });
        let current = json!({
            "version": 1,
            "op": "register",
            "app_scope_id": "appserver-cli",
            "project_scope": canonical,
            "canonical_root": canonical,
            "storage_root": canonical,
            "registered_ms": 2
        });
        std::fs::write(
            state_root.join("routes.jsonl"),
            format!("{stale}\n{current}\n"),
        )
        .unwrap();

        let host_paths = HostPaths::for_state_root(&state_root).unwrap();
        let app_scope = AppServerId::new("appserver-cli").unwrap();
        let resolved = canonical_route_for_identity(&host_paths, &worktree, &app_scope).unwrap();

        assert_eq!(resolved.root, canonical);

        std::fs::remove_dir_all(root).ok();
    }
}
