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
`--worktree-path` must match the configured `[worktree].base` layout when set
(for example `<base>/<project-key>/<short-slug>`); legacy
`<project-main>/playground/<short-slug>` records stay valid during the
transition. Leaf max 32 ASCII bytes, no `..`. Only the live registered master
can dispatch; the selected peer must be registered, present, non-managed, and
inactive. Success returns `request_id`, `message_id`, `task_id`, `target`,
`status: assigned`, `admission.*`, and `notification` (`sent` |
`subscribed-not-sent` | `mailbox-only-no-subscription`). `sent` is not
consumption; verify with `collab msg <id>` (`consumed_by_recv`) and
`collab task status <task-id>`.

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
#[path = "scope_tests.rs"]
mod tests;
