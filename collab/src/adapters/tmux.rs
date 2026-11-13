use crate::proto::{TmuxCandidate, TmuxEndpoint, TmuxPaneAnchor};
use std::collections::{BTreeSet, HashMap};
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::{Mutex, OnceLock};

mod codex_presence;

pub use codex_presence::{AgentActivity, AgentObservation, AgentPresence};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PanePresence {
    Present,
    Missing,
    Unknown,
}

static LAST_PANE_OUTPUT: OnceLock<Mutex<std::collections::HashMap<String, u64>>> = OnceLock::new();
const MAX_PANE_OBSERVATIONS: usize = 256;
pub const ENDPOINT_CAPABILITY_FILE: &str = "tmux-endpoints.json";

#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct EndpointCapabilities {
    endpoints: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ObservedPane {
    server_pid: u32,
    tmux_session_id: String,
    pane_id: String,
    pane_pid: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ForegroundPaneFacts {
    pane_pid: u32,
    pane_tty: String,
    command: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ProcessFact {
    pid: u32,
    ppid: u32,
    tty: String,
    state: String,
    executable: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct ObservedTmuxExecution {
    pub(crate) pid: u32,
    pub(crate) start_id: String,
}

pub(crate) struct ExecutionObservation {
    pub(crate) observation: AgentObservation,
    pub(crate) execution: Option<ObservedTmuxExecution>,
}

static LAST_TMUX_EXECUTIONS: OnceLock<Mutex<HashMap<String, ObservedTmuxExecution>>> =
    OnceLock::new();
const MAX_TMUX_EXECUTIONS: usize = 256;

pub fn configured_endpoints(state_root: &Path) -> Result<Vec<String>, String> {
    let path = state_root.join(ENDPOINT_CAPABILITY_FILE);
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(format!(
                "TMUX_OBSERVATION_CAPABILITY_INVALID: read {}: {error}",
                path.display()
            ));
        }
    };
    let capabilities: EndpointCapabilities = serde_json::from_slice(&bytes).map_err(|error| {
        format!(
            "TMUX_OBSERVATION_CAPABILITY_INVALID: parse {}: {error}",
            path.display()
        )
    })?;
    for endpoint in &capabilities.endpoints {
        validate_socket_path(endpoint)?;
    }
    Ok(capabilities.endpoints)
}

/// Host-owned endpoints available for observation: the typed capability file
/// plus the global `[tmux] endpoints` configuration.
pub fn host_endpoints_at(state_root: &Path) -> Result<Vec<String>, String> {
    let mut endpoints = configured_endpoints(state_root)?;
    endpoints.extend(
        crate::config::tmux_endpoints()
            .map_err(|error| format!("TMUX_OBSERVATION_CAPABILITY_INVALID: {error}"))?,
    );
    endpoints.sort();
    endpoints.dedup();
    Ok(endpoints)
}

pub fn observe_caller(
    endpoints: impl IntoIterator<Item = String>,
    caller_pid: u32,
    pane_hint: Option<&str>,
) -> Result<Option<TmuxEndpoint>, String> {
    if caller_pid == 0 {
        return Err("TMUX_OBSERVATION_INVALID: caller pid must be non-zero".into());
    }
    let endpoints = endpoints
        .into_iter()
        .map(|path| {
            validate_socket_path(&path)?;
            Ok(path)
        })
        .collect::<Result<BTreeSet<_>, String>>()?;
    if endpoints.is_empty() {
        if pane_hint.is_some() {
            return Err(
                "TMUX_OBSERVATION_CAPABILITY_MISSING: no configured or recorded tmux endpoint is available"
                    .into(),
            );
        }
        return Ok(None);
    }
    let parents = process_parents()?;
    let ancestors = caller_ancestors(caller_pid, &parents);
    let mut matches = Vec::new();
    let mut observation_errors = Vec::new();
    for socket_path in endpoints {
        match list_panes(&socket_path) {
            Ok(panes) => {
                for pane in panes {
                    if ancestors.contains(&pane.pane_pid) {
                        matches.push((socket_path.clone(), pane));
                    }
                }
            }
            Err(error) if endpoint_is_absent(&error) => {}
            Err(error) => observation_errors.push(format!("{socket_path}: {error}")),
        }
    }
    if matches.len() > 1 {
        if let Some(hint) = pane_hint.filter(|hint| validate_pane_id(hint).is_ok()) {
            matches.retain(|(_, pane)| pane.pane_id == hint);
        }
    }
    let (socket_path, pane) = match matches.len() {
        0 => {
            return if observation_errors.is_empty() {
                Ok(None)
            } else {
                Err(format!(
                    "TMUX_OBSERVATION_UNAVAILABLE: {}",
                    observation_errors.join("; ")
                ))
            };
        }
        1 => matches.pop().unwrap(),
        _ => {
            let targets = matches
                .iter()
                .map(|(_, pane)| format!("{}:{}", pane.tmux_session_id, pane.pane_id))
                .collect::<Vec<_>>()
                .join(", ");
            return Err(format!(
                "TMUX_OBSERVATION_AMBIGUOUS: caller ancestry matches multiple panes: {targets}"
            ));
        }
    };
    let endpoint = TmuxEndpoint {
        socket_path,
        server_pid: pane.server_pid,
        tmux_session_id: pane.tmux_session_id,
        pane_id: pane.pane_id,
        pane_pid: pane.pane_pid,
        codex_session_id: None,
        codex_thread_id: None,
    };
    validate_endpoint(&endpoint)?;
    Ok(Some(endpoint))
}

/// Pane hint exported by tmux. The `TMUX` value (socket path/server pid/client
/// session) is deliberately never parsed: it is caller-controlled placement
/// data and cannot select a Collab identity.
pub fn pane_hint_from_env() -> Option<String> {
    std::env::var("TMUX_PANE")
        .ok()
        .filter(|value| validate_pane_id(value).is_ok())
}

/// Read-only endpoints persisted in identity receipts. This keeps legacy
/// callers free of socket environment injection while still using only
/// host-owned capability records.
pub fn persisted_identity_endpoints(state_root: &Path) -> Result<Vec<String>, String> {
    let root = state_root.join("identities");
    let entries = match std::fs::read_dir(&root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(format!(
                "TMUX_OBSERVATION_CAPABILITY_INVALID: read {}: {error}",
                root.display()
            ));
        }
    };
    let mut endpoints = BTreeSet::new();
    for entry in entries {
        let entry = entry.map_err(|error| {
            format!(
                "TMUX_OBSERVATION_CAPABILITY_INVALID: read {}: {error}",
                root.display()
            )
        })?;
        if !entry
            .file_type()
            .map_err(|error| format!("TMUX_OBSERVATION_CAPABILITY_INVALID: {error}"))?
            .is_dir()
        {
            continue;
        }
        let bytes = match std::fs::read(entry.path().join("identity.json")) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(format!(
                    "TMUX_OBSERVATION_CAPABILITY_INVALID: read identity receipt: {error}"
                ));
            }
        };
        let receipt: serde_json::Value = serde_json::from_slice(&bytes).map_err(|error| {
            format!("TMUX_OBSERVATION_CAPABILITY_INVALID: parse identity receipt: {error}")
        })?;
        if receipt["transport"]["kind"].as_str() != Some("tmux") {
            continue;
        }
        if let Some(socket) = receipt["transport"]["tmux_endpoint"]["socket_path"].as_str() {
            validate_socket_path(socket)?;
            endpoints.insert(socket.to_owned());
        }
    }
    Ok(endpoints.into_iter().collect())
}

pub fn known_endpoints_at(state_root: &Path) -> Result<Vec<String>, String> {
    let mut endpoints = host_endpoints_at(state_root)?;
    endpoints.extend(persisted_identity_endpoints(state_root)?);
    endpoints.sort();
    endpoints.dedup();
    Ok(endpoints)
}

pub fn observe_caller_at(
    state_root: &Path,
    caller_pid: u32,
    pane_hint: Option<&str>,
) -> Result<Option<TmuxEndpoint>, String> {
    observe_caller(known_endpoints_at(state_root)?, caller_pid, pane_hint)
}

/// Compatibility wrapper for callers that still request the current tmux
/// candidate. It observes live ancestry against host-owned endpoint records;
/// it does not parse `TMUX`.
pub fn candidate_from_env() -> Result<TmuxCandidate, String> {
    let host_paths = crate::scope::HostPaths::resolve().map_err(|error| {
        format!("TMUX_OBSERVATION_CAPABILITY_INVALID: resolve host state root: {error}")
    })?;
    let hint = pane_hint_from_env();
    let mut endpoint =
        observe_caller_at(host_paths.state_root(), std::process::id(), hint.as_deref())?
            .ok_or_else(|| {
                "TMUX_OBSERVATION_UNAVAILABLE: caller ancestry does not identify a live tmux pane"
                    .to_owned()
            })?;
    let host_facts = crate::host_runtime::host_runtime_facts();
    if let Some(agy_session) = host_facts.agy_session_id {
        endpoint.codex_session_id = Some(agy_session);
        endpoint.codex_thread_id = None;
    }
    let cwd = crate::scope::process_cwd()
        .and_then(|path| path.canonicalize().map_err(anyhow::Error::from))
        .map_err(|error| format!("TMUX_OBSERVATION_INVALID: resolve cwd: {error}"))?
        .to_str()
        .ok_or_else(|| "TMUX_OBSERVATION_INVALID: cwd is not UTF-8".to_owned())?
        .to_owned();
    Ok(TmuxCandidate { endpoint, cwd })
}

/// Compatibility wrapper for the remaining pane-anchor consumers. The socket
/// path comes from the live observed endpoint, never from the `TMUX` value.
pub fn anchor_from_env() -> Result<TmuxPaneAnchor, String> {
    let candidate = candidate_from_env()?;
    Ok(TmuxPaneAnchor {
        socket_path: candidate.endpoint.socket_path,
        pane_id: candidate.endpoint.pane_id,
    })
}

fn validate_socket_path(path: &str) -> Result<(), String> {
    if !Path::new(path).is_absolute() || path.chars().any(char::is_control) {
        return Err(
            "TMUX_OBSERVATION_CAPABILITY_INVALID: endpoint must be an absolute socket path".into(),
        );
    }
    Ok(())
}

fn process_parents() -> Result<HashMap<u32, u32>, String> {
    let output = Command::new("ps")
        .args(["-axo", "pid=,ppid="])
        .output()
        .map_err(|error| format!("TMUX_OBSERVATION_UNAVAILABLE: start ps: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "TMUX_OBSERVATION_UNAVAILABLE: ps failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let text = String::from_utf8(output.stdout)
        .map_err(|_| "TMUX_OBSERVATION_UNAVAILABLE: ps returned non-UTF-8".to_owned())?;
    let mut parents = HashMap::new();
    for line in text.lines() {
        let mut fields = line.split_whitespace();
        let (Some(pid), Some(parent), None) = (fields.next(), fields.next(), fields.next()) else {
            return Err("TMUX_OBSERVATION_UNAVAILABLE: malformed ps process table".into());
        };
        let pid = pid
            .parse::<u32>()
            .map_err(|_| "TMUX_OBSERVATION_UNAVAILABLE: malformed ps pid".to_owned())?;
        let parent = parent
            .parse::<u32>()
            .map_err(|_| "TMUX_OBSERVATION_UNAVAILABLE: malformed ps parent pid".to_owned())?;
        parents.insert(pid, parent);
    }
    Ok(parents)
}

fn process_snapshot() -> Result<Vec<ProcessFact>, String> {
    let output = Command::new("ps")
        .args(["-axo", "pid=,ppid=,tty=,state=,comm="])
        .output()
        .map_err(|error| format!("TMUX_OBSERVATION_UNAVAILABLE: start ps: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "TMUX_OBSERVATION_UNAVAILABLE: ps failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let text = String::from_utf8(output.stdout)
        .map_err(|_| "TMUX_OBSERVATION_UNAVAILABLE: ps returned non-UTF-8".to_owned())?;
    parse_process_snapshot(&text)
}

fn parse_process_snapshot(text: &str) -> Result<Vec<ProcessFact>, String> {
    let mut processes = Vec::new();
    let mut seen = BTreeSet::new();
    for line in text.lines().filter(|line| !line.trim().is_empty()) {
        let mut fields = line.split_whitespace();
        let (Some(pid), Some(ppid), Some(tty), Some(state)) =
            (fields.next(), fields.next(), fields.next(), fields.next())
        else {
            return Err("TMUX_OBSERVATION_UNAVAILABLE: malformed ps process table".into());
        };
        let executable = fields.collect::<Vec<_>>().join(" ");
        if executable.is_empty() {
            return Err("TMUX_OBSERVATION_UNAVAILABLE: missing process executable".into());
        }
        let pid = pid
            .parse::<u32>()
            .map_err(|_| "TMUX_OBSERVATION_UNAVAILABLE: malformed ps pid".to_owned())?;
        let ppid = ppid
            .parse::<u32>()
            .map_err(|_| "TMUX_OBSERVATION_UNAVAILABLE: malformed ps parent pid".to_owned())?;
        if !seen.insert(pid) {
            return Err("TMUX_OBSERVATION_UNAVAILABLE: duplicate ps pid".into());
        }
        processes.push(ProcessFact {
            pid,
            ppid,
            tty: tty.to_owned(),
            state: state.to_owned(),
            executable,
        });
    }
    if processes.is_empty() {
        return Err("TMUX_OBSERVATION_UNAVAILABLE: empty ps process table".into());
    }
    Ok(processes)
}

fn process_start_id(pid: u32) -> Result<Option<String>, String> {
    crate::host_runtime::exact_process_start_id(pid)
        .map_err(|_| "TMUX_OBSERVATION_UNAVAILABLE: exact process identity query failed".into())
}

fn execution_key(endpoint: &TmuxEndpoint) -> String {
    format!(
        "{}\0{}\0{}\0{}",
        endpoint.socket_path, endpoint.server_pid, endpoint.tmux_session_id, endpoint.pane_id
    )
}

fn remembered_execution(key: &str) -> Result<Option<ObservedTmuxExecution>, String> {
    let executions = LAST_TMUX_EXECUTIONS.get_or_init(|| Mutex::new(HashMap::new()));
    let executions = executions.lock().map_err(|_| {
        "TMUX_OBSERVATION_UNAVAILABLE: execution observation lock poisoned".to_owned()
    })?;
    Ok(executions.get(key).cloned())
}

fn remember_execution(key: String, execution: ObservedTmuxExecution) -> Result<(), String> {
    let executions = LAST_TMUX_EXECUTIONS.get_or_init(|| Mutex::new(HashMap::new()));
    let mut executions = executions.lock().map_err(|_| {
        "TMUX_OBSERVATION_UNAVAILABLE: execution observation lock poisoned".to_owned()
    })?;
    executions.insert(key.clone(), execution);
    if executions.len() > MAX_TMUX_EXECUTIONS {
        if let Some(evicted) = executions
            .keys()
            .find(|candidate| *candidate != &key)
            .cloned()
        {
            executions.remove(&evicted);
        }
    }
    Ok(())
}

fn current_codex_candidates<'a>(
    processes: &'a [ProcessFact],
    pane_pid: u32,
    pane_tty: &str,
) -> Vec<&'a ProcessFact> {
    let descendants = process_descendants(processes, pane_pid);
    let pane_tty = normalize_tty(pane_tty);
    processes
        .iter()
        .filter(|process| !process_is_zombie(process))
        .filter(|process| codex_presence::is_codex_executable(&process.executable))
        .filter(|process| {
            descendants.contains(&process.pid)
                || pane_tty
                    .as_deref()
                    .is_some_and(|tty| normalize_tty(&process.tty).as_deref() == Some(tty))
        })
        .collect()
}

fn process_descendants(processes: &[ProcessFact], root: u32) -> BTreeSet<u32> {
    let mut descendants = BTreeSet::from([root]);
    loop {
        let mut changed = false;
        for process in processes {
            if descendants.contains(&process.ppid) && descendants.insert(process.pid) {
                changed = true;
            }
        }
        if !changed {
            return descendants;
        }
    }
}

fn normalize_tty(tty: &str) -> Option<String> {
    let tty = Path::new(tty).file_name()?.to_str()?;
    if tty.is_empty() || tty == "??" || tty == "-" {
        return None;
    }
    Some(tty.to_owned())
}

fn process_is_zombie(process: &ProcessFact) -> bool {
    process.state.starts_with('Z')
}

fn caller_ancestors(caller_pid: u32, parents: &HashMap<u32, u32>) -> BTreeSet<u32> {
    let mut ancestors = BTreeSet::new();
    let mut current = caller_pid;
    while current != 0 && ancestors.insert(current) {
        let Some(parent) = parents.get(&current) else {
            break;
        };
        current = *parent;
    }
    ancestors
}

fn list_panes(socket_path: &str) -> Result<Vec<ObservedPane>, String> {
    let output = tmux_command(
        socket_path,
        &[
            "list-panes",
            "-a",
            "-F",
            // Printable `|` survives locales that mangle control separators
            // (tmux 3.x under a launchd agent with no UTF-8 locale replaces
            // `\t` with `_`, which made the observer reject every pane).
            "#{pid}|#{session_id}|#{pane_id}|#{pane_pid}",
        ],
    )?;
    let text = String::from_utf8(output.stdout)
        .map_err(|_| "TMUX_OBSERVATION_INVALID: non-UTF-8 tmux metadata".to_owned())?;
    parse_pane_lines(&text)
}

fn parse_pane_lines(text: &str) -> Result<Vec<ObservedPane>, String> {
    text.lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            // tmux is asked for `|`-separated fields. A field may itself
            // contain whitespace, so split on the literal separator rather than
            // whitespace.
            let fields = line.split('|').collect::<Vec<_>>();
            if fields.len() != 4 {
                return Err("TMUX_OBSERVATION_INVALID: malformed tmux pane metadata".into());
            }
            let server_pid = fields[0]
                .parse::<u32>()
                .map_err(|_| "TMUX_OBSERVATION_INVALID: malformed tmux server pid".to_owned())?;
            let pane_pid = fields[3]
                .parse::<u32>()
                .map_err(|_| "TMUX_OBSERVATION_INVALID: malformed tmux pane pid".to_owned())?;
            validate_pane_id(fields[2])?;
            if !fields[1].starts_with('$') {
                return Err("TMUX_OBSERVATION_INVALID: malformed tmux session id".into());
            }
            Ok(ObservedPane {
                server_pid,
                tmux_session_id: fields[1].to_owned(),
                pane_id: fields[2].to_owned(),
                pane_pid,
            })
        })
        .collect()
}

fn endpoint_is_absent(error: &str) -> bool {
    let error = error.to_ascii_lowercase();
    error.contains("no server running") || error.contains("error connecting to")
}

/// Validates a fixed tmux session name. The TUI renames its live session to the
/// role registration name once; the name must be safe to pass as a tmux target
/// and stable across recovery. tmux target syntax treats `:` and `.` as
/// separators, so a fixed name never contains them.
fn validate_session_name(name: &str) -> Result<(), String> {
    if name.trim().is_empty()
        || name.starts_with('-')
        || name
            .chars()
            .any(|character| character.is_control() || character.is_whitespace())
        || name.contains(':')
        || name.contains('.')
    {
        return Err("TMUX_SESSION_NAME_INVALID: malformed fixed session name".into());
    }
    Ok(())
}

/// Reads the current tmux session name for an observed endpoint. This is the
/// TUI identity anchor: it stays stable across pane and `$N` changes, unlike
/// the socket/pane/PID placement.
pub fn observe_session_name(endpoint: &TmuxEndpoint) -> Result<String, String> {
    validate_endpoint(endpoint)?;
    let output = tmux_command(
        &endpoint.socket_path,
        &[
            "display-message",
            "-p",
            "-t",
            &endpoint.tmux_session_id,
            "#{session_name}",
        ],
    )
    .map_err(|error| format!("TMUX_SESSION_NAME_QUERY_FAILED: {error}"))?;
    let name = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if name.is_empty() || name.chars().any(char::is_control) {
        return Err("TMUX_SESSION_NAME_QUERY_FAILED: empty or malformed session name".into());
    }
    Ok(name)
}

/// Renames the observed TUI tmux session to its fixed role name and reads the
/// name back. Renaming to the current name is idempotent. A failed rename or a
/// read-back mismatch is explicit: callers must abort the identity commit and
/// keep the previous session name instead of persisting a half-anchor.
/// Observe the current session name only when the endpoint is live. A missing
/// server yields `Ok(None)`: the caller has no live tmux anchor
/// to move, so identity handlers skip the rename instead of failing a
/// non-TUI/one-shot caller. Other observation failures stay explicit.
pub fn session_name_if_live(endpoint: &TmuxEndpoint) -> Result<Option<String>, String> {
    validate_endpoint(endpoint)?;
    let output = match tmux_command(
        &endpoint.socket_path,
        &[
            "list-sessions",
            "-F",
            "#{pid}|#{session_id}|#{session_name}",
        ],
    ) {
        Ok(output) => output,
        Err(error)
            if error == format!("no server running on {}", endpoint.socket_path)
                || error
                    == format!(
                        "error connecting to {} (No such file or directory)",
                        endpoint.socket_path
                    ) =>
        {
            return Ok(None)
        }
        Err(error) => return Err(format!("TMUX_SESSION_NAME_QUERY_FAILED: {error}")),
    };
    let text = std::str::from_utf8(&output.stdout)
        .map_err(|error| format!("TMUX_SESSION_NAME_QUERY_FAILED: {error}"))?;
    for line in text.lines() {
        let fields: Vec<_> = line.splitn(3, '|').collect();
        if fields.len() != 3 {
            return Err("TMUX_SESSION_NAME_QUERY_FAILED: malformed session inventory".into());
        }
        let pid: u32 = fields[0]
            .parse()
            .map_err(|_| "TMUX_SESSION_NAME_QUERY_FAILED: malformed server pid")?;
        if pid == endpoint.server_pid && fields[1] == endpoint.tmux_session_id {
            if fields[2].trim().is_empty() || fields[2].chars().any(char::is_control) {
                return Err(
                    "TMUX_SESSION_NAME_QUERY_FAILED: empty or malformed session name".into(),
                );
            }
            return Ok(Some(fields[2].to_owned()));
        }
    }
    Ok(None)
}

/// Find a live tmux pane whose session currently carries `name`. This is used
/// only to move an orphaned role-name session aside before a successor takes
/// the same name; the socket-scoped listing does not touch the session.
pub fn find_live_session_by_name(
    socket_path: &str,
    name: &str,
) -> Result<Option<TmuxEndpoint>, String> {
    validate_session_name(name)?;
    let output = match tmux_command(
        socket_path,
        &[
            "list-panes",
            "-a",
            "-F",
            "#{pid}\t#{session_id}\t#{session_name}\t#{pane_id}\t#{pane_pid}",
        ],
    ) {
        Ok(output) => output,
        Err(error) if endpoint_is_absent(&error) => return Ok(None),
        Err(error) => return Err(format!("TMUX_SESSION_LOOKUP_FAILED: {error}")),
    };
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let fields = line.split('\t').collect::<Vec<_>>();
        if fields.len() != 5 || fields[2] != name {
            continue;
        }
        return Ok(Some(TmuxEndpoint {
            socket_path: socket_path.to_owned(),
            server_pid: fields[0]
                .parse()
                .map_err(|error| format!("TMUX_SESSION_LOOKUP_INVALID: {error}"))?,
            tmux_session_id: fields[1].to_owned(),
            pane_id: fields[3].to_owned(),
            pane_pid: fields[4]
                .parse()
                .map_err(|error| format!("TMUX_SESSION_LOOKUP_INVALID: {error}"))?,
            codex_session_id: None,
            codex_thread_id: None,
        }));
    }
    Ok(None)
}

/// Read the current session name of a registered TUI binding, requiring the
/// tmux endpoint to be reachable. Unlike `session_name_if_live`, a missing or
/// unreachable server here is a typed failure: the identity owner must abort
/// rather than commit a name without a live, readable anchor. `caller_pid`-less
/// non-TUI callers never reach this path because they have no registered tmux
/// endpoint.
pub fn require_live_session_name(endpoint: &TmuxEndpoint) -> Result<String, String> {
    match observe_session_name(endpoint) {
        Ok(name) => Ok(name),
        Err(error) if endpoint_is_absent(&error) => {
            Err(format!("TMUX_TUI_ANCHOR_UNREACHABLE: {error}"))
        }
        Err(error) => Err(error),
    }
}

pub fn rename_session(endpoint: &TmuxEndpoint, target: &str) -> Result<(), String> {
    validate_endpoint(endpoint)?;
    validate_session_name(target)?;
    let previous = observe_session_name(endpoint)?;
    if previous == target {
        return Ok(());
    }
    if let Err(error) = tmux_command(
        &endpoint.socket_path,
        &["rename-session", "-t", &endpoint.tmux_session_id, target],
    ) {
        let error = format!("TMUX_SESSION_RENAME_FAILED: {error}");
        return Err(append_rename_rollback_error(
            error,
            restore_session_name(endpoint, &previous),
        ));
    }
    let observed = match observe_session_name(endpoint) {
        Ok(observed) => observed,
        Err(error) => {
            let error = format!("TMUX_SESSION_RENAME_FAILED: readback: {error}");
            return Err(append_rename_rollback_error(
                error,
                restore_session_name(endpoint, &previous),
            ));
        }
    };
    if observed != target {
        let error =
            format!("TMUX_SESSION_RENAME_FAILED: readback returned {observed} instead of {target}");
        return Err(append_rename_rollback_error(
            error,
            restore_session_name(endpoint, &previous),
        ));
    }
    Ok(())
}

fn restore_session_name(endpoint: &TmuxEndpoint, previous: &str) -> Result<(), String> {
    if observe_session_name(endpoint)? == previous {
        return Ok(());
    }
    tmux_command(
        &endpoint.socket_path,
        &["rename-session", "-t", &endpoint.tmux_session_id, previous],
    )
    .map_err(|error| error.to_string())?;
    let observed = observe_session_name(endpoint)?;
    if observed != previous {
        return Err(format!(
            "readback returned {observed} instead of the previous name {previous}"
        ));
    }
    Ok(())
}

fn append_rename_rollback_error(error: String, rollback: Result<(), String>) -> String {
    match rollback {
        Ok(()) => error,
        Err(rollback) => format!("{error}; TMUX_SESSION_RENAME_ROLLBACK_FAILED: {rollback}"),
    }
}

pub fn probe(endpoint: &TmuxEndpoint) -> Result<PanePresence, String> {
    validate_endpoint(endpoint)?;
    let args = [
        "list-panes",
        "-t",
        endpoint.pane_id.as_str(),
        "-F",
        "#{pane_id}",
    ];
    pane_probe_result(
        &endpoint.pane_id,
        tmux_command(&endpoint.socket_path, &args)
            .map(|output| String::from_utf8_lossy(&output.stdout).into_owned()),
    )
}

fn pane_probe_result(
    pane_id: &str,
    result: Result<String, String>,
) -> Result<PanePresence, String> {
    match result {
        Ok(panes)
            if panes.lines().all(|pane| validate_pane_id(pane).is_ok())
                && panes.lines().any(|pane| pane == pane_id) =>
        {
            Ok(PanePresence::Present)
        }
        Ok(_) => Ok(PanePresence::Unknown),
        Err(error) if error == format!("can't find pane: {pane_id}") => Ok(PanePresence::Missing),
        Err(error) => Err(format!("TMUX_PROBE_UNKNOWN: {error}")),
    }
}

/// Reports input submission and pane observation separately. It never claims
/// the mailbox message was consumed; only the recipient's collab recv proves it.
pub fn notify(
    endpoint: &TmuxEndpoint,
    message_id: &str,
    text: &str,
) -> Result<serde_json::Value, String> {
    match probe(endpoint)? {
        PanePresence::Present => {}
        PanePresence::Missing => return Err("TMUX_PANE_MISSING: target pane is gone".into()),
        PanePresence::Unknown => return Err("TMUX_PANE_UNKNOWN: target pane is uncertain".into()),
    }
    let before = capture(endpoint)?;
    let buffer = buffer_name(message_id);
    let mut child = Command::new("tmux")
        .args([
            "-S",
            &endpoint.socket_path,
            "load-buffer",
            "-b",
            &buffer,
            "-",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("TMUX_TEXT_SUBMIT_FAILED: {error}"))?;
    child
        .stdin
        .take()
        .ok_or_else(|| "TMUX_TEXT_SUBMIT_FAILED: stdin unavailable".to_owned())?
        .write_all(text.as_bytes())
        .map_err(|error| format!("TMUX_TEXT_SUBMIT_FAILED: {error}"))?;
    let loaded = child
        .wait_with_output()
        .map_err(|error| format!("TMUX_TEXT_SUBMIT_FAILED: {error}"))?;
    if !loaded.status.success() {
        return Err(format!(
            "TMUX_TEXT_SUBMIT_FAILED: {}",
            String::from_utf8_lossy(&loaded.stderr).trim()
        ));
    }
    tmux_command(
        &endpoint.socket_path,
        &[
            "paste-buffer",
            "-d",
            "-p",
            "-b",
            &buffer,
            "-t",
            &endpoint.pane_id,
        ],
    )
    .map_err(|error| format!("TMUX_TEXT_SUBMIT_FAILED: {error}"))?;
    let after_text = capture(endpoint)?;
    tmux_command(
        &endpoint.socket_path,
        &["send-keys", "-t", &endpoint.pane_id, "Enter"],
    )
    .map_err(|error| format!("TMUX_ENTER_SUBMIT_FAILED: {error}"))?;
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(500);
    let mut after_enter = capture(endpoint)?;
    while after_enter == after_text && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(50));
        after_enter = capture(endpoint)?;
    }
    Ok(serde_json::json!({
        "transport": "tmux",
        "message_id": message_id,
        "text_submitted": true,
        "enter_submitted": true,
        "pane_output_changed_after_enter": after_enter != after_text,
        "pane_output_changed_since_before": after_enter != before,
        "consumed": false
    }))
}

/// Read-only observation directed at the current Codex TUI. Presence and
/// activity remain separate, and every result carries a stable reason. The
/// pane address is not treated as proof that an agent is present.
pub fn observe_agent(endpoint: &TmuxEndpoint) -> Result<AgentObservation, String> {
    let previous = remembered_execution(&execution_key(endpoint))?;
    let result = observe_agent_with_execution(endpoint, previous.as_ref())?;
    if let Some(execution) = result.execution {
        remember_execution(execution_key(endpoint), execution)?;
    }
    Ok(result.observation)
}

/// The daemon supplies its current durable reference explicitly. This path
/// never reads or writes the standalone view cache.
pub(crate) fn observe_agent_with_execution(
    endpoint: &TmuxEndpoint,
    previous: Option<&ObservedTmuxExecution>,
) -> Result<ExecutionObservation, String> {
    let mut execution = None;
    let observation = observe_with_execution(endpoint, previous, &mut execution)?;
    Ok(ExecutionObservation {
        observation,
        execution,
    })
}

fn observe_with_execution(
    endpoint: &TmuxEndpoint,
    previous: Option<&ObservedTmuxExecution>,
    execution: &mut Option<ObservedTmuxExecution>,
) -> Result<AgentObservation, String> {
    validate_endpoint(endpoint)?;
    match probe(endpoint) {
        Ok(PanePresence::Missing) => {
            return Ok(AgentObservation {
                agent_presence: AgentPresence::Offline,
                activity: AgentActivity::Unknown,
                background: false,
                foreground_command: None,
                reason: "bound_pane_missing".into(),
            });
        }
        Ok(PanePresence::Unknown) => {
            return Ok(AgentObservation::unknown("bound_pane_unknown"));
        }
        Ok(PanePresence::Present) => {}
        Err(_) => return Ok(AgentObservation::unknown("pane_probe_unavailable")),
    }

    let foreground = match foreground_pane_facts(endpoint) {
        Ok(foreground) => foreground,
        Err(_) => return Ok(AgentObservation::unknown("foreground_query_failed")),
    };
    if codex_presence::is_codex_command(&foreground.command) {
        match unique_current_execution(&foreground) {
            Ok(current) => *execution = current,
            Err(reason) => return Ok(AgentObservation::unknown(reason)),
        }
    } else {
        return Ok(observe_non_codex_with_process_facts(
            &foreground,
            codex_presence::is_shell_command(&foreground.command),
            previous,
            execution,
        ));
    }

    let visible_screen = match capture_visible(endpoint) {
        Ok(screen) => screen,
        Err(_) => {
            return Ok(AgentObservation {
                agent_presence: AgentPresence::Online,
                activity: AgentActivity::Unknown,
                background: false,
                foreground_command: Some(foreground.command),
                reason: "visible_screen_unavailable".into(),
            });
        }
    };
    Ok(codex_presence::classify_codex(
        &foreground.command,
        &visible_screen,
    ))
}

fn unique_current_execution(
    foreground: &ForegroundPaneFacts,
) -> Result<Option<ObservedTmuxExecution>, &'static str> {
    let Ok(processes) = process_snapshot() else {
        return Err("process_query_failed");
    };
    let candidates = current_codex_candidates(
        processes.as_slice(),
        foreground.pane_pid,
        &foreground.pane_tty,
    );
    if candidates.len() > 1 {
        return Err("codex_process_candidates_ambiguous");
    }
    let Some(candidate) = candidates.first() else {
        return Err("process_changed_during_observation");
    };
    let start_id = process_start_id(candidate.pid)
        .map_err(|_| "process_identity_query_failed")?
        .ok_or("process_changed_during_observation")?;
    Ok(Some(ObservedTmuxExecution {
        pid: candidate.pid,
        start_id,
    }))
}

fn observe_non_codex_with_process_facts(
    foreground: &ForegroundPaneFacts,
    shell_foreground: bool,
    previous: Option<&ObservedTmuxExecution>,
    execution: &mut Option<ObservedTmuxExecution>,
) -> AgentObservation {
    let processes = match process_snapshot() {
        Ok(processes) => processes,
        Err(_) => return AgentObservation::unknown("process_query_failed"),
    };
    let candidates = current_codex_candidates(
        processes.as_slice(),
        foreground.pane_pid,
        &foreground.pane_tty,
    );
    if candidates.len() > 1 {
        return AgentObservation::unknown("codex_process_candidates_ambiguous");
    }
    if let Some(candidate) = candidates.first() {
        let start_id = match process_start_id(candidate.pid) {
            Ok(Some(start_id)) => start_id,
            Ok(None) => return AgentObservation::unknown("process_changed_during_observation"),
            Err(_) => return AgentObservation::unknown("process_identity_query_failed"),
        };
        *execution = Some(ObservedTmuxExecution {
            pid: candidate.pid,
            start_id,
        });
        return AgentObservation {
            agent_presence: AgentPresence::Online,
            activity: AgentActivity::Unknown,
            background: false,
            foreground_command: Some(foreground.command.clone()),
            reason: "current_codex_process_still_running".into(),
        };
    }

    let Some(previous) = previous else {
        return AgentObservation::unknown(if shell_foreground {
            "foreground_shell_without_agent_exit_evidence"
        } else {
            "unsupported_foreground_process"
        });
    };
    // Persisted exit evidence requires the precise native identity query,
    // including when the process table no longer contains this PID.
    observe_exact_execution(previous, &processes, foreground, &process_start_id)
}

fn observe_exact_execution(
    previous: &ObservedTmuxExecution,
    processes: &[ProcessFact],
    foreground: &ForegroundPaneFacts,
    query: &dyn Fn(u32) -> Result<Option<String>, String>,
) -> AgentObservation {
    let current_start_id = match query(previous.pid) {
        Ok(Some(start_id)) => start_id,
        Ok(None) => return exited_codex_observation(),
        Err(_) => return AgentObservation::unknown("process_identity_query_failed"),
    };
    let same_execution = if current_start_id == previous.start_id {
        true
    } else {
        let previous =
            crate::server::binding_slot::RexExecutionFence::from_start_id(&previous.start_id);
        let current =
            crate::server::binding_slot::RexExecutionFence::from_start_id(&current_start_id);
        match previous
            .as_ref()
            .zip(current.as_ref())
            .and_then(|(previous, current)| previous.compare(current))
        {
            Some(_) => false,
            None => return AgentObservation::unknown("process_identity_query_failed"),
        }
    };
    if !same_execution {
        return exited_codex_observation();
    }
    if processes
        .iter()
        .any(|process| process.pid == previous.pid && process_is_zombie(process))
    {
        return exited_codex_observation();
    }
    AgentObservation {
        agent_presence: AgentPresence::Online,
        activity: AgentActivity::Unknown,
        background: false,
        foreground_command: Some(foreground.command.clone()),
        reason: "observed_codex_process_still_running".into(),
    }
}

fn exited_codex_observation() -> AgentObservation {
    AgentObservation {
        agent_presence: AgentPresence::Offline,
        activity: AgentActivity::Unknown,
        background: false,
        foreground_command: None,
        reason: "observed_codex_process_exited".into(),
    }
}

pub fn view(endpoint: &TmuxEndpoint) -> Result<serde_json::Value, String> {
    let observation = observe_agent(endpoint)?;
    if observation.reason == "bound_pane_missing" {
        return Ok(view_projection(observation, false, "missing"));
    }
    let output = capture(endpoint)?;
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    output.hash(&mut hasher);
    let digest = hasher.finish();
    let key = format!(
        "{}\0{}\0{}",
        endpoint.socket_path, endpoint.tmux_session_id, endpoint.pane_id
    );
    let (changed, pane_observation) = record_observation(key, digest)?;
    Ok(view_projection(observation, changed, pane_observation))
}

fn view_projection(
    observation: AgentObservation,
    changed: bool,
    pane_observation: &str,
) -> serde_json::Value {
    let (thread_state, thread_status, can_accept_direct_input) = match observation.agent_presence {
        AgentPresence::Online => match observation.activity {
            AgentActivity::Busy => ("working", "working", Some(false)),
            AgentActivity::Idle => ("idle", "idle", Some(true)),
            AgentActivity::WaitingUser => ("waiting_user", "waiting_user", Some(false)),
            AgentActivity::Error => ("error", "error", Some(false)),
            AgentActivity::Unknown => ("unknown", "unknown", None),
        },
        AgentPresence::Offline | AgentPresence::Unknown => ("unknown", "unknown", None),
    };
    serde_json::json!({
        "thread": {
            "status": {"type": thread_status},
            "canAcceptDirectInput": can_accept_direct_input,
            "turns": [{"status": thread_status}]
        },
        "thread_state": thread_state,
        "agent_presence": observation.agent_presence,
        "activity": observation.activity,
        "background": observation.background,
        "foreground_command": observation.foreground_command,
        "reason": observation.reason,
        "pane_output_changed": changed,
        "pane_output_observation": pane_observation,
        "transport": "tmux"
    })
}

pub fn snapshot(endpoint: &TmuxEndpoint, lines: usize) -> Result<serde_json::Value, String> {
    match probe(endpoint)? {
        PanePresence::Present => {}
        PanePresence::Missing => return Err("TMUX_PANE_MISSING: target pane is gone".into()),
        PanePresence::Unknown => return Err("TMUX_PANE_UNKNOWN: target pane is uncertain".into()),
    }
    let output = capture_lines(endpoint, lines)?;
    Ok(serde_json::json!({
        "transport": "tmux",
        "pane_id": endpoint.pane_id,
        "lines": lines,
        "output": output,
        "durable": false
    }))
}

fn record_observation(key: String, digest: u64) -> Result<(bool, &'static str), String> {
    let observations =
        LAST_PANE_OUTPUT.get_or_init(|| Mutex::new(std::collections::HashMap::new()));
    let mut observations = observations
        .lock()
        .map_err(|_| "TMUX_PANE_STATE_UNKNOWN: observation lock poisoned".to_owned())?;
    let previous = observations.insert(key.clone(), digest);
    if observations.len() > MAX_PANE_OBSERVATIONS {
        if let Some(evicted) = observations
            .keys()
            .find(|candidate| *candidate != &key)
            .cloned()
        {
            observations.remove(&evicted);
        }
    }
    let changed = previous.is_some_and(|previous| previous != digest);
    let observation = match previous {
        None => "initial",
        Some(previous) if previous == digest => "stable",
        Some(_) => "changed",
    };
    Ok((changed, observation))
}

fn capture(endpoint: &TmuxEndpoint) -> Result<String, String> {
    let output = tmux_command(
        &endpoint.socket_path,
        &["capture-pane", "-p", "-t", &endpoint.pane_id, "-S", "-80"],
    )
    .map_err(|error| format!("TMUX_CAPTURE_FAILED: {error}"))?;
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn capture_visible(endpoint: &TmuxEndpoint) -> Result<String, String> {
    let output = tmux_command(
        &endpoint.socket_path,
        &["capture-pane", "-p", "-t", &endpoint.pane_id],
    )
    .map_err(|error| format!("TMUX_CAPTURE_FAILED: {error}"))?;
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn foreground_pane_facts(endpoint: &TmuxEndpoint) -> Result<ForegroundPaneFacts, String> {
    let output = tmux_command(
        &endpoint.socket_path,
        &[
            "list-panes",
            "-t",
            &endpoint.pane_id,
            "-F",
            "#{session_id}|#{pane_id}|#{pane_pid}|#{pane_tty}|#{pane_current_command}",
        ],
    )
    .map_err(|error| format!("TMUX_FOREGROUND_QUERY_FAILED: {error}"))?;
    let text = String::from_utf8(output.stdout)
        .map_err(|_| "TMUX_FOREGROUND_QUERY_FAILED: non-UTF-8 tmux metadata".to_owned())?;
    parse_foreground_pane_facts(&endpoint.pane_id, &text)
}

fn parse_foreground_pane_facts(pane_id: &str, text: &str) -> Result<ForegroundPaneFacts, String> {
    for line in text.lines() {
        let mut fields = line.splitn(5, '|');
        let (Some(session), Some(pane), Some(pane_pid), Some(pane_tty), Some(command)) = (
            fields.next(),
            fields.next(),
            fields.next(),
            fields.next(),
            fields.next(),
        ) else {
            return Err("TMUX_FOREGROUND_QUERY_FAILED: malformed pane metadata".into());
        };
        let session_digits = session.strip_prefix('$').unwrap_or_default();
        if session_digits.is_empty()
            || !session_digits.bytes().all(|byte| byte.is_ascii_digit())
            || validate_pane_id(pane).is_err()
        {
            return Err("TMUX_FOREGROUND_QUERY_FAILED: malformed pane metadata".into());
        }
        if pane != pane_id {
            continue;
        }
        let pane_pid = pane_pid
            .parse::<u32>()
            .map_err(|_| "TMUX_FOREGROUND_QUERY_FAILED: malformed pane pid".to_owned())?;
        let command = command.trim();
        if pane_pid == 0
            || pane_tty.trim().is_empty()
            || command.is_empty()
            || pane_tty.chars().any(char::is_control)
            || command.chars().any(char::is_control)
        {
            return Err("TMUX_FOREGROUND_QUERY_FAILED: malformed pane facts".into());
        }
        return Ok(ForegroundPaneFacts {
            pane_pid,
            pane_tty: pane_tty.to_owned(),
            command: command.to_owned(),
        });
    }
    Err("TMUX_FOREGROUND_QUERY_FAILED: bound pane not returned".into())
}

fn capture_lines(endpoint: &TmuxEndpoint, lines: usize) -> Result<String, String> {
    // `capture-pane -S -N` includes the visible pane rows in addition to the
    // requested scrollback. Capture a generous bounded history once, then take
    // only the newest lines so `--lines` is an exact public maximum.
    let output = tmux_command(
        &endpoint.socket_path,
        &["capture-pane", "-p", "-t", &endpoint.pane_id, "-S", "-4096"],
    )
    .map_err(|error| format!("TMUX_CAPTURE_FAILED: {error}"))?;
    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .rev()
        .skip_while(|line| line.trim().is_empty())
        .take(lines)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<Vec<_>>()
        .join("\n"))
}

fn buffer_name(message_id: &str) -> String {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    message_id.hash(&mut hasher);
    format!("collab-{:016x}", hasher.finish())
}

pub(crate) fn validate_pane_id(id: &str) -> Result<(), String> {
    let digits = id.strip_prefix('%').unwrap_or_default();
    if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err("TMUX_ENDPOINT_INVALID: pane id must use tmux %N form".into());
    }
    Ok(())
}

pub fn validate_endpoint(endpoint: &TmuxEndpoint) -> Result<(), String> {
    if !std::path::Path::new(&endpoint.socket_path).is_absolute()
        || endpoint.socket_path.chars().any(char::is_control)
        || endpoint.server_pid == 0
        || endpoint.pane_pid == 0
    {
        return Err("TMUX_ENDPOINT_INVALID: malformed socket path or process id".into());
    }
    let session = endpoint
        .tmux_session_id
        .strip_prefix('$')
        .unwrap_or_default();
    if session.is_empty() || !session.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err("TMUX_ENDPOINT_INVALID: session id must use tmux $N form".into());
    }
    validate_pane_id(&endpoint.pane_id)?;
    for anchor in [
        endpoint.codex_session_id.as_deref(),
        endpoint.codex_thread_id.as_deref(),
    ]
    .into_iter()
    .flatten()
    {
        if anchor.trim().is_empty() || anchor.chars().any(char::is_control) {
            return Err("TMUX_IDENTITY_INVALID: malformed Codex identity anchor".into());
        }
    }
    Ok(())
}

/// One owned pane on one tmux server instance.
///
/// Ownership requires the server pid in addition to the persisted pane
/// address: after a tmux restart the same socket path and pane id can be
/// reissued to a different pane, so a pid mismatch is a new owner. Session
/// and shell pid drift on the same server do not change ownership. Use
/// `same_pane_route` for the persisted address and this predicate for every
/// ownership question.
pub fn same_owned_pane(left: &TmuxEndpoint, right: &TmuxEndpoint) -> bool {
    left.server_pid == right.server_pid && same_pane_route(left, right)
}

/// The persisted pane address: socket path plus pane id. Runtime observations
/// such as server/session/pane pids and native thread ids do not redefine it.
/// Ownership additionally requires the server instance in `same_owned_pane`.
pub fn same_pane_route(left: &TmuxEndpoint, right: &TmuxEndpoint) -> bool {
    left.socket_path == right.socket_path && left.pane_id == right.pane_id
}

fn tmux_command(socket: &str, args: &[&str]) -> Result<std::process::Output, String> {
    let output = Command::new("tmux")
        .arg("-S")
        .arg(socket)
        .args(args)
        .output()
        .map_err(|error| format!("start tmux: {error}"))?;
    if output.status.success() {
        Ok(output)
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_owned())
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::path::{Path, PathBuf};
    use std::time::Duration;

    #[test]
    fn exact_execution_requires_current_native_identity_and_keeps_query_failure_unknown() {
        let previous = ObservedTmuxExecution {
            pid: 700,
            start_id: "100:1".into(),
        };
        let foreground = ForegroundPaneFacts {
            pane_pid: 1,
            pane_tty: "/dev/ttys001".into(),
            command: "sh".into(),
        };
        for (identity, expected) in [
            (Ok(Some("100:1".into())), AgentPresence::Online),
            (Ok(Some("100:2".into())), AgentPresence::Offline),
            (Ok(None), AgentPresence::Offline),
            (Err("permission denied".into()), AgentPresence::Unknown),
        ] {
            let observed = observe_exact_execution(&previous, &[], &foreground, &|pid| {
                assert_eq!(pid, previous.pid);
                identity.clone()
            });
            assert_eq!(observed.agent_presence, expected, "{observed:?}");
            assert_eq!(observed.activity, AgentActivity::Unknown);
        }
    }

    #[test]
    fn exact_execution_keeps_incomparable_start_reference_unknown() {
        let foreground = ForegroundPaneFacts {
            pane_pid: 1,
            pane_tty: "/dev/ttys001".into(),
            command: "sh".into(),
        };
        for (remembered, current) in [
            ("12:7", "linux:b879bb0a-cf9a-47d1-b889-f63aa872d851:100:700"),
            (
                "linux:b879bb0a-cf9a-47d1-b889-f63aa872d851:100:700",
                "linux:11111111-1111-1111-1111-111111111111:100:701",
            ),
            (
                "linux:b879bb0a-cf9a-47d1-b889-f63aa872d851:100:700",
                "linux:b879bb0a-cf9a-47d1-b889-f63aa872d851:1000:7000",
            ),
        ] {
            let previous = ObservedTmuxExecution {
                pid: 700,
                start_id: remembered.into(),
            };
            let observed =
                observe_exact_execution(&previous, &[], &foreground, &|_| Ok(Some(current.into())));
            assert_eq!(observed.agent_presence, AgentPresence::Unknown);
            assert_eq!(observed.reason, "process_identity_query_failed");
        }

        let previous = ObservedTmuxExecution {
            pid: 700,
            start_id: "linux:b879bb0a-cf9a-47d1-b889-f63aa872d851:100:700".into(),
        };
        let observed = observe_exact_execution(&previous, &[], &foreground, &|_| {
            Ok(Some(
                "linux:b879bb0a-cf9a-47d1-b889-f63aa872d851:100:701".into(),
            ))
        });
        assert_eq!(observed.agent_presence, AgentPresence::Offline);
        assert_eq!(observed.reason, "observed_codex_process_exited");
    }

    struct IsolatedTmux {
        socket: PathBuf,
        root: PathBuf,
        session_id: String,
    }

    impl IsolatedTmux {
        fn start() -> Self {
            Self::start_with_command(None)
        }

        fn start_with_command(pane_command: Option<&str>) -> Self {
            let root = std::env::temp_dir().join(format!(
                "collab-tmux-test-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            std::fs::create_dir_all(&root).unwrap();
            let socket = root.join("tmux.sock");
            let session_name = "collab-test".to_owned();
            let mut command = Command::new("tmux");
            command
                .arg("-S")
                .arg(&socket)
                .args(["new-session", "-d", "-s", &session_name]);
            if let Some(pane_command) = pane_command {
                command.arg(pane_command);
            }
            let output = command.output().expect("start isolated tmux server");
            assert!(
                output.status.success(),
                "start isolated tmux: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            let queried = Command::new("tmux")
                .arg("-S")
                .arg(&socket)
                .args([
                    "display-message",
                    "-p",
                    "-t",
                    &session_name,
                    "#{session_id}",
                ])
                .output()
                .expect("query isolated tmux session id");
            assert!(
                queried.status.success(),
                "query isolated tmux session id: {}",
                String::from_utf8_lossy(&queried.stderr)
            );
            let session_id = String::from_utf8(queried.stdout)
                .unwrap()
                .trim_end()
                .to_owned();
            assert!(session_id.starts_with('$'), "invalid tmux session id");
            let split = Command::new("tmux")
                .arg("-S")
                .arg(&socket)
                .args(["split-window", "-d", "-t", "collab-test:0"])
                .output()
                .expect("create spare isolated tmux pane");
            assert!(
                split.status.success(),
                "split isolated tmux: {}",
                String::from_utf8_lossy(&split.stderr)
            );
            Self {
                socket,
                root,
                session_id,
            }
        }

        fn endpoint(&self) -> TmuxEndpoint {
            let output = Command::new("tmux")
                .arg("-S")
                .arg(&self.socket)
                .args([
                    "display-message",
                    "-p",
                    "-t",
                    "collab-test:0.0",
                    "#{pid}\t#{session_id}\t#{pane_id}\t#{pane_pid}",
                ])
                .output()
                .expect("query isolated tmux pane");
            assert!(
                output.status.success(),
                "query isolated tmux: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            let line = String::from_utf8(output.stdout).unwrap();
            let mut fields = line.trim_end().split('\t');
            TmuxEndpoint {
                socket_path: self.socket.to_string_lossy().into_owned(),
                server_pid: fields.next().unwrap().parse().unwrap_or_else(|error| {
                    panic!("invalid isolated tmux server PID in {line:?}: {error}")
                }),
                tmux_session_id: fields.next().unwrap().into(),
                pane_id: fields.next().unwrap().into(),
                pane_pid: fields.next().unwrap().parse().unwrap(),
                codex_session_id: None,
                codex_thread_id: None,
            }
        }

        fn fake_codex(&self) -> CodexFixture {
            CodexFixture::compile(&self.root)
        }

        fn send_command(&self, command: &str) {
            let endpoint = self.endpoint();
            let deadline = std::time::Instant::now() + Duration::from_secs(5);
            loop {
                let shell_ready = foreground_pane_facts(&endpoint)
                    .is_ok_and(|facts| codex_presence::is_shell_command(&facts.command));
                let prompt_visible = capture(&endpoint).is_ok_and(|screen| {
                    screen.lines().any(|line| {
                        let line = line.trim_end();
                        line.ends_with('$') || line.ends_with('%') || line.ends_with('#')
                    })
                });
                if shell_ready && prompt_visible {
                    break;
                }
                assert!(
                    std::time::Instant::now() < deadline,
                    "isolated shell prompt did not become ready"
                );
                std::thread::sleep(Duration::from_millis(20));
            }

            let typed = Command::new("tmux")
                .arg("-S")
                .arg(&self.socket)
                .args(["send-keys", "-l", "-t", "collab-test:0.0", command])
                .output()
                .expect("type into isolated tmux");
            assert!(
                typed.status.success(),
                "type into isolated tmux: {}",
                String::from_utf8_lossy(&typed.stderr)
            );
            let entered = Command::new("tmux")
                .arg("-S")
                .arg(&self.socket)
                .args(["send-keys", "-t", "collab-test:0.0", "Enter"])
                .output()
                .expect("submit isolated tmux command");
            assert!(
                entered.status.success(),
                "submit isolated tmux command: {}",
                String::from_utf8_lossy(&entered.stderr)
            );
        }

        fn send_key(&self, key: &str) {
            let output = Command::new("tmux")
                .arg("-S")
                .arg(&self.socket)
                .args(["send-keys", "-t", "collab-test:0.0", key])
                .output()
                .expect("send key into isolated tmux");
            assert!(
                output.status.success(),
                "send key into isolated tmux: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }

    impl Drop for IsolatedTmux {
        fn drop(&mut self) {
            let _ = Command::new("tmux")
                .arg("-S")
                .arg(&self.socket)
                .args(["kill-session", "-t", &self.session_id])
                .output();
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    struct CodexFixture {
        executable: PathBuf,
        pid_file: PathBuf,
        start_id: RefCell<Option<String>>,
    }

    impl CodexFixture {
        fn compile(root: &Path) -> Self {
            let source = root.join("codex-fixture.c");
            let executable = root.join("codex");
            let pid_file = root.join("codex.pid");
            std::fs::write(
                &source,
                r#"#define _POSIX_C_SOURCE 200809L
#include <fcntl.h>
#include <signal.h>
#include <stdio.h>
#include <unistd.h>

int main(int argc, char **argv) {
    char pid_text[32];
    int pid_length;
    int pid_file;
    int signal_number;
    sigset_t signals;

    if (argc != 2) {
        return 64;
    }
    sigemptyset(&signals);
    sigaddset(&signals, SIGTERM);
    sigaddset(&signals, SIGINT);
    if (sigprocmask(SIG_BLOCK, &signals, NULL) != 0) {
        return 70;
    }
    pid_length = snprintf(pid_text, sizeof(pid_text), "%ld\n", (long)getpid());
    if (pid_length <= 0 || (size_t)pid_length >= sizeof(pid_text)) {
        return 71;
    }
    pid_file = open(argv[1], O_WRONLY | O_CREAT | O_TRUNC, 0600);
    if (pid_file < 0) {
        return 72;
    }
    if (write(pid_file, pid_text, (size_t)pid_length) != pid_length) {
        close(pid_file);
        return 73;
    }
    if (close(pid_file) != 0) {
        return 74;
    }
    if (sigwait(&signals, &signal_number) != 0) {
        return 75;
    }
    return 0;
}
"#,
            )
            .expect("write native Codex fixture source");
            let output = Command::new("cc")
                .args(["-std=c11", "-O0", "-o"])
                .arg(&executable)
                .arg(&source)
                .output()
                .expect("compile native Codex fixture with cc");
            assert!(
                output.status.success(),
                "compile native Codex fixture: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            Self {
                executable,
                pid_file,
                start_id: RefCell::new(None),
            }
        }

        fn background_command(&self) -> String {
            format!(
                "{} {} & exec sh -i",
                shell_quote(&self.executable.to_string_lossy()),
                shell_quote(&self.pid_file.to_string_lossy())
            )
        }

        fn pid(&self) -> Option<u32> {
            std::fs::read_to_string(&self.pid_file)
                .ok()
                .and_then(|pid| pid.trim().parse().ok())
        }

        fn spawn_detached(&self) -> std::process::Child {
            use std::os::unix::process::CommandExt;
            let mut command = Command::new(&self.executable);
            command
                .arg(&self.pid_file)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null());
            unsafe {
                command.pre_exec(|| {
                    if libc::setsid() == -1 {
                        return Err(std::io::Error::last_os_error());
                    }
                    Ok(())
                });
            }
            command
                .spawn()
                .expect("spawn detached native Codex fixture")
        }

        fn terminate(&self) {
            let Some(pid) = self.pid() else {
                return;
            };
            let expected_start_id = self.start_id.borrow().clone();
            let current_start_id = process_start_id(pid).ok().flatten();
            if expected_start_id.is_some() && current_start_id != expected_start_id {
                self.clear_pid();
                return;
            }
            if expected_start_id.is_none() || current_start_id.is_none() {
                self.clear_pid();
                return;
            }
            terminate_test_process(pid, true);
            self.clear_pid();
        }

        fn clear_pid(&self) {
            let _ = std::fs::remove_file(&self.pid_file);
        }

        fn record_start_id(&self, pid: u32) {
            *self.start_id.borrow_mut() = process_start_id(pid)
                .expect("read native Codex fixture start identity through product process helper")
                .or_else(|| panic!("native Codex fixture disappeared before identity read"));
        }
    }

    impl Drop for CodexFixture {
        fn drop(&mut self) {
            self.terminate();
        }
    }

    pub(crate) struct TmuxFailureWrapper {
        root: PathBuf,
        original_path: Option<std::ffi::OsString>,
    }

    impl TmuxFailureWrapper {
        pub(crate) fn fail_command(command_name: &str) -> Self {
            Self::with_command_result(command_name, 71)
        }

        fn with_command_result(command_name: &str, exit_code: u8) -> Self {
            let output = Command::new("which").arg("tmux").output().unwrap();
            assert!(output.status.success());
            let real_tmux = String::from_utf8(output.stdout).unwrap().trim().to_owned();
            let root = std::env::temp_dir().join(format!(
                "collab-tmux-wrapper-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            std::fs::create_dir_all(&root).unwrap();
            let wrapper = root.join("tmux");
            std::fs::write(
                &wrapper,
                format!(
                    "#!/bin/sh\nfor arg do\n  if [ \"$arg\" = \"{command_name}\" ]; then\n    echo 'forced {command_name} failure' >&2\n    exit {exit_code}\n  fi\ndone\nexec '{real_tmux}' \"$@\"\n"
                ),
            )
            .unwrap();
            let mut permissions = std::fs::metadata(&wrapper).unwrap().permissions();
            use std::os::unix::fs::PermissionsExt;
            permissions.set_mode(0o755);
            std::fs::set_permissions(&wrapper, permissions).unwrap();
            let original_path = std::env::var_os("PATH");
            let mut paths = vec![root.clone()];
            if let Some(original) = &original_path {
                paths.extend(std::env::split_paths(original));
            }
            std::env::set_var("PATH", std::env::join_paths(paths).unwrap());
            Self {
                root,
                original_path,
            }
        }
    }

    impl Drop for TmuxFailureWrapper {
        fn drop(&mut self) {
            match self.original_path.take() {
                Some(path) => std::env::set_var("PATH", path),
                None => std::env::remove_var("PATH"),
            }
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn pane_id_requires_numeric_tmux_form() {
        assert!(validate_pane_id("%17").is_ok());
        assert!(validate_pane_id("17").is_err());
        assert!(validate_pane_id("%x").is_err());
        assert!(validate_pane_id("%1; kill-server").is_err());
    }

    #[test]
    fn caller_ancestry_disambiguates_overlapping_pane_ids() {
        let first = IsolatedTmux::start();
        let second = IsolatedTmux::start();
        let first_endpoint = first.endpoint();
        let second_endpoint = second.endpoint();
        assert_eq!(
            first_endpoint.pane_id, second_endpoint.pane_id,
            "fresh isolated servers should expose the overlapping pane id case"
        );
        let observed = observe_caller(
            vec![
                first.socket.to_string_lossy().into_owned(),
                second.socket.to_string_lossy().into_owned(),
            ],
            first_endpoint.pane_pid,
            Some(&first_endpoint.pane_id),
        )
        .unwrap()
        .expect("the caller's pane is observable");
        assert_eq!(observed.socket_path, first_endpoint.socket_path);
        assert_eq!(observed.server_pid, first_endpoint.server_pid);
        assert_eq!(observed.tmux_session_id, first_endpoint.tmux_session_id);
        assert_eq!(observed.pane_id, first_endpoint.pane_id);
        assert_eq!(observed.pane_pid, first_endpoint.pane_pid);
    }

    #[test]
    fn configured_endpoints_are_typed_host_capabilities() {
        let root = std::env::temp_dir().join(format!(
            "collab-tmux-capability-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            root.join(ENDPOINT_CAPABILITY_FILE),
            r#"{"endpoints":["/tmp/tmux-one.sock","/tmp/tmux-two.sock"]}"#,
        )
        .unwrap();
        assert_eq!(
            configured_endpoints(&root).unwrap(),
            vec!["/tmp/tmux-one.sock", "/tmp/tmux-two.sock"]
        );
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn list_panes_ignores_trailing_blank_lines() {
        let observed = parse_pane_lines("58649|$0|%0|58650\n\n").expect("trailing blanks parse");
        assert_eq!(observed.len(), 1);
        assert_eq!(observed[0].pane_id, "%0");
    }

    #[test]
    fn config_endpoints_are_available_without_capability_file() {
        let _guard = crate::scope::TEST_ENV_LOCK.lock().unwrap();
        let original_home = std::env::var_os("HOME");
        let home = std::env::temp_dir().join(format!(
            "collab-tmux-config-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(home.join(".appsdk")).unwrap();
        let tmux = IsolatedTmux::start();
        std::fs::write(
            home.join(".appsdk/config.toml"),
            format!(
                "[tmux]\nendpoints = [\"{}\"]\n",
                tmux.socket.to_string_lossy()
            ),
        )
        .unwrap();
        std::env::set_var("HOME", &home);
        let state_root = home.join("state");
        std::fs::create_dir_all(&state_root).unwrap();

        let endpoint = tmux.endpoint();
        let observed = observe_caller_at(&state_root, endpoint.pane_pid, Some(&endpoint.pane_id))
            .unwrap()
            .expect("configured tmux socket should identify the caller pane");
        assert_eq!(observed.socket_path, endpoint.socket_path);
        assert_eq!(observed.server_pid, endpoint.server_pid);
        assert_eq!(observed.pane_id, endpoint.pane_id);

        match original_home {
            Some(value) => std::env::set_var("HOME", value),
            None => std::env::remove_var("HOME"),
        }
        std::fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn pane_route_identity_is_the_socket_and_pane_address() {
        let endpoint = TmuxEndpoint {
            socket_path: "/tmp/tmux-a.sock".into(),
            server_pid: 11,
            tmux_session_id: "$1".into(),
            pane_id: "%2".into(),
            pane_pid: 33,
            codex_session_id: None,
            codex_thread_id: None,
        };
        assert!(same_pane_route(&endpoint, &endpoint));
        for drifted in [
            TmuxEndpoint {
                server_pid: 12,
                ..endpoint.clone()
            },
            TmuxEndpoint {
                tmux_session_id: "$3".into(),
                ..endpoint.clone()
            },
            TmuxEndpoint {
                pane_pid: 34,
                ..endpoint.clone()
            },
        ] {
            assert!(
                same_pane_route(&endpoint, &drifted),
                "the pane address must survive process/session drift"
            );
        }
        for changed in [
            TmuxEndpoint {
                socket_path: "/tmp/tmux-b.sock".into(),
                ..endpoint.clone()
            },
            TmuxEndpoint {
                pane_id: "%4".into(),
                ..endpoint.clone()
            },
        ] {
            assert!(!same_pane_route(&endpoint, &changed));
        }
    }

    #[test]
    fn pane_ownership_requires_the_same_tmux_server_instance() {
        let endpoint = TmuxEndpoint {
            socket_path: "/tmp/tmux-a.sock".into(),
            server_pid: 11,
            tmux_session_id: "$1".into(),
            pane_id: "%2".into(),
            pane_pid: 33,
            codex_session_id: None,
            codex_thread_id: None,
        };
        assert!(same_owned_pane(&endpoint, &endpoint));
        for drifted in [
            TmuxEndpoint {
                tmux_session_id: "$3".into(),
                ..endpoint.clone()
            },
            TmuxEndpoint {
                pane_pid: 34,
                ..endpoint.clone()
            },
        ] {
            assert!(
                same_owned_pane(&endpoint, &drifted),
                "session and shell pid drift stay on the same owned pane"
            );
        }
        let restarted = TmuxEndpoint {
            server_pid: 12,
            ..endpoint.clone()
        };
        assert!(
            !same_owned_pane(&endpoint, &restarted),
            "a reissued pane id on a new server instance is not the same owned pane"
        );
        assert!(
            same_pane_route(&endpoint, &restarted),
            "the persisted pane address still matches across server restarts"
        );
        for changed in [
            TmuxEndpoint {
                socket_path: "/tmp/tmux-b.sock".into(),
                ..endpoint.clone()
            },
            TmuxEndpoint {
                pane_id: "%4".into(),
                ..endpoint.clone()
            },
        ] {
            assert!(!same_owned_pane(&endpoint, &changed));
        }
    }

    #[test]
    fn buffer_name_is_stable_and_does_not_copy_message_text() {
        assert_eq!(buffer_name("message-1"), buffer_name("message-1"));
        assert!(!buffer_name("secret body").contains("secret"));
    }

    #[test]
    fn pane_probe_requires_exact_membership_or_explicit_target_absence() {
        assert_eq!(
            pane_probe_result("%0", Ok("%2\n%0\n".into())).unwrap(),
            PanePresence::Present
        );
        for output in ["", "\n", "%2\n", "95087\t\t\t\t\n", "%0\nbroken\n"] {
            assert_eq!(
                pane_probe_result("%0", Ok(output.into())).unwrap(),
                PanePresence::Unknown,
                "{output:?} cannot prove presence or absence"
            );
        }
        assert_eq!(
            pane_probe_result("%0", Err("can't find pane: %0".into())).unwrap(),
            PanePresence::Missing
        );
        for error in [
            "can't find pane: %1",
            "can't find pane: %0\nconnection failed",
            "no server running",
        ] {
            assert!(pane_probe_result("%0", Err(error.into()))
                .unwrap_err()
                .starts_with("TMUX_PROBE_UNKNOWN:"));
        }
    }

    #[test]
    fn pane_probe_distinguishes_present_missing_and_unknown() {
        let _guard = crate::scope::TEST_ENV_LOCK.lock().unwrap();
        let tmux = IsolatedTmux::start();
        let endpoint = tmux.endpoint();
        assert_eq!(probe(&endpoint).unwrap(), PanePresence::Present);
        let restarted = TmuxEndpoint {
            server_pid: endpoint.server_pid + 1,
            ..endpoint.clone()
        };
        assert_eq!(probe(&restarted).unwrap(), PanePresence::Present);

        // Keep the tmux server alive after the target pane is removed. Removing
        // the last pane would stop the service, which makes Unknown legitimate.
        let keepalive = Command::new("tmux")
            .arg("-S")
            .arg(&tmux.socket)
            .args(["new-session", "-d", "-s", "collab-keepalive", "sleep 60"])
            .output()
            .unwrap();
        assert!(
            keepalive.status.success(),
            "start keepalive pane: {}",
            String::from_utf8_lossy(&keepalive.stderr)
        );
        let killed = Command::new("tmux")
            .arg("-S")
            .arg(&tmux.socket)
            .args(["kill-pane", "-t", &endpoint.pane_id])
            .output()
            .unwrap();
        assert!(killed.status.success());
        let alive = Command::new("tmux")
            .arg("-S")
            .arg(&tmux.socket)
            .args(["has-session", "-t", "collab-keepalive"])
            .output()
            .unwrap();
        assert!(
            alive.status.success(),
            "the tmux server must remain alive for a Missing assertion"
        );
        // Preserve the native counterexample: display-message succeeds with
        // empty pane fields after target removal. Only the supported pane
        // query proves the absence; malformed display output is not Missing.
        for (label, args) in [
            (
                "legacy-display-message",
                vec![
                    "display-message",
                    "-p",
                    "-t",
                    endpoint.pane_id.as_str(),
                    "#{pid}\t#{session_id}\t#{pane_id}\t#{pane_pid}\t#{pane_dead}",
                ],
            ),
            (
                "list-panes-all",
                vec!["list-panes", "-a", "-F", "#{pane_id}"],
            ),
            (
                "list-panes-target",
                vec![
                    "list-panes",
                    "-t",
                    endpoint.pane_id.as_str(),
                    "-F",
                    "#{pane_id}",
                ],
            ),
            (
                "list-sessions",
                vec!["list-sessions", "-F", "#{session_name}"],
            ),
        ] {
            let raw = Command::new("tmux")
                .arg("-S")
                .arg(&tmux.socket)
                .args(&args)
                .output()
                .unwrap();
            eprintln!(
                "pane_probe diagnostic {label}: status={:?} stdout={:?} stderr={:?}",
                raw.status.code(),
                String::from_utf8_lossy(&raw.stdout),
                String::from_utf8_lossy(&raw.stderr)
            );
        }
        eprintln!(
            "pane_probe diagnostic target={} server_alive={} probe={:?}",
            endpoint.pane_id,
            alive.status.success(),
            probe(&endpoint)
        );
        assert_eq!(probe(&endpoint).unwrap(), PanePresence::Missing);

        let unknown = TmuxEndpoint {
            socket_path: tmux
                .root
                .join("missing-parent/absent.sock")
                .to_string_lossy()
                .into_owned(),
            ..endpoint
        };
        assert!(probe(&unknown)
            .unwrap_err()
            .starts_with("TMUX_PROBE_UNKNOWN:"));
    }

    #[test]
    fn stable_pane_output_remains_observation_and_does_not_claim_agent_idle() {
        let _guard = crate::scope::TEST_ENV_LOCK.lock().unwrap();
        let tmux = IsolatedTmux::start();
        let endpoint = tmux.endpoint();
        let initial = view(&endpoint).unwrap();
        assert_eq!(initial["thread_state"], "unknown");
        std::thread::sleep(Duration::from_millis(1600));

        let stable = view(&endpoint).unwrap();
        assert_eq!(stable["thread_state"], "unknown");
        assert_eq!(stable["thread"]["status"]["type"], "unknown");
        assert_eq!(stable["agent_presence"], "unknown");
        assert_eq!(stable["activity"], "unknown");
        assert_eq!(
            stable["reason"],
            "foreground_shell_without_agent_exit_evidence"
        );
        assert!(stable["pane_output_changed"].is_boolean());
        assert!(matches!(
            stable["pane_output_observation"].as_str(),
            Some("initial" | "changed" | "stable")
        ));
    }

    #[test]
    fn view_reports_offline_when_bound_pane_is_explicitly_missing() {
        let _guard = crate::scope::TEST_ENV_LOCK.lock().unwrap();
        let tmux = IsolatedTmux::start();
        let endpoint = tmux.endpoint();
        let keepalive = Command::new("tmux")
            .arg("-S")
            .arg(&tmux.socket)
            .args(["new-session", "-d", "-s", "collab-keepalive", "sleep 60"])
            .output()
            .unwrap();
        assert!(
            keepalive.status.success(),
            "start keepalive pane: {}",
            String::from_utf8_lossy(&keepalive.stderr)
        );
        let killed = Command::new("tmux")
            .arg("-S")
            .arg(&tmux.socket)
            .args(["kill-pane", "-t", &endpoint.pane_id])
            .output()
            .unwrap();
        assert!(killed.status.success());

        let observed = view(&endpoint).unwrap();
        assert_eq!(observed["agent_presence"], "offline", "{observed}");
        assert_eq!(observed["activity"], "unknown", "{observed}");
        assert_eq!(observed["reason"], "bound_pane_missing", "{observed}");
        assert_eq!(observed["pane_output_changed"], false, "{observed}");
        assert_eq!(observed["pane_output_observation"], "missing", "{observed}");
    }

    #[test]
    fn view_keeps_shell_unknown_despite_historical_codex_busy_text() {
        let _guard = crate::scope::TEST_ENV_LOCK.lock().unwrap();
        let tmux = IsolatedTmux::start_with_command(Some(
            "printf 'Codex history: esc to interrupt\\n'; exec sh -i",
        ));
        let endpoint = tmux.endpoint();
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        loop {
            if capture(&endpoint).unwrap().contains("Codex history") {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "shell fixture did not publish its historical line"
            );
            std::thread::sleep(Duration::from_millis(20));
        }

        let observed = view(&endpoint).unwrap();
        assert_eq!(observed["agent_presence"], "unknown", "{observed}");
        assert_eq!(observed["activity"], "unknown", "{observed}");
        assert_eq!(observed["background"], false, "{observed}");
        assert_eq!(
            observed["reason"], "foreground_shell_without_agent_exit_evidence",
            "{observed}"
        );
    }

    fn shell_quote(value: &str) -> String {
        format!("'{}'", value.replace('\'', "'\"'\"'"))
    }

    fn wait_for_view<F>(
        endpoint: &TmuxEndpoint,
        deadline: Duration,
        predicate: F,
    ) -> serde_json::Value
    where
        F: Fn(&serde_json::Value) -> bool,
    {
        let deadline = std::time::Instant::now() + deadline;
        loop {
            let observed = view(endpoint).unwrap();
            if predicate(&observed) {
                return observed;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "observation did not reach the expected state: {observed}"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn wait_for_remembered_execution(
        endpoint: &TmuxEndpoint,
        deadline: Duration,
        different_from: Option<u32>,
    ) -> ObservedTmuxExecution {
        let deadline = std::time::Instant::now() + deadline;
        loop {
            let observed = view(endpoint).unwrap();
            if let Ok(Some(execution)) = remembered_execution(&execution_key(endpoint)) {
                if different_from != Some(execution.pid) {
                    return execution;
                }
            }
            assert!(
                std::time::Instant::now() < deadline,
                "execution was not remembered: {observed}"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn wait_for_fixture_ready(
        endpoint: &TmuxEndpoint,
        fixture: &CodexFixture,
        deadline: Duration,
    ) -> u32 {
        let deadline = std::time::Instant::now() + deadline;
        loop {
            let pid = fixture.pid();
            let processes = process_snapshot().ok();
            let process = pid.and_then(|pid| {
                processes
                    .as_ref()?
                    .iter()
                    .find(|process| process.pid == pid)
                    .cloned()
            });
            let pane = foreground_pane_facts(endpoint).ok();
            let process_matches_pane =
                match (pid, processes.as_ref(), process.as_ref(), pane.as_ref()) {
                    (Some(pid), Some(processes), Some(process), Some(pane)) => {
                        let descendants = process_descendants(processes, pane.pane_pid);
                        let tty_matches = normalize_tty(&pane.pane_tty).is_some_and(|tty| {
                            normalize_tty(&process.tty).as_deref() == Some(tty.as_str())
                        });
                        descendants.contains(&pid) && tty_matches
                    }
                    _ => false,
                };
            if process_matches_pane
                && process.as_ref().is_some_and(|process| {
                    !process_is_zombie(process) && fixture_executable_is_codex(&process.executable)
                })
                && pane
                    .as_ref()
                    .is_some_and(|pane| codex_presence::is_shell_command(&pane.command))
            {
                fixture.record_start_id(pid.unwrap());
                return pid.unwrap();
            }
            assert!(
                std::time::Instant::now() < deadline,
                "native Codex fixture did not become ready: pid={pid:?}, process={process:?}, pane={pane:?}"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn wait_for_detached_fixture_ready(
        fixture: &CodexFixture,
        expected_pid: u32,
        deadline: Duration,
    ) -> ProcessFact {
        let deadline = std::time::Instant::now() + deadline;
        loop {
            let process = process_snapshot()
                .unwrap_or_default()
                .into_iter()
                .find(|process| process.pid == expected_pid);
            if fixture.pid() == Some(expected_pid)
                && process.as_ref().is_some_and(|process| {
                    !process_is_zombie(process) && fixture_executable_is_codex(&process.executable)
                })
            {
                fixture.record_start_id(expected_pid);
                return process.unwrap();
            }
            assert!(
                std::time::Instant::now() < deadline,
                "detached native Codex fixture did not become ready: pid_file={:?}, process={process:?}",
                fixture.pid()
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn fixture_executable_is_codex(executable: &str) -> bool {
        Path::new(executable)
            .file_name()
            .and_then(|name| name.to_str())
            == Some("codex")
    }

    fn codex_candidates(endpoint: &TmuxEndpoint) -> Vec<ProcessFact> {
        let foreground = foreground_pane_facts(endpoint).unwrap();
        let processes = process_snapshot().unwrap();
        current_codex_candidates(&processes, foreground.pane_pid, &foreground.pane_tty)
            .into_iter()
            .cloned()
            .collect()
    }

    fn terminate_test_process(pid: u32, suspended: bool) {
        unsafe {
            if suspended {
                libc::kill(pid as libc::pid_t, libc::SIGCONT);
            }
            libc::kill(pid as libc::pid_t, libc::SIGTERM);
        }
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while std::time::Instant::now() < deadline {
            let alive = process_snapshot()
                .unwrap_or_default()
                .iter()
                .any(|process| process.pid == pid && !process_is_zombie(process));
            if !alive {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn view_reports_offline_after_observed_codex_process_exits() {
        let _guard = crate::scope::TEST_ENV_LOCK.lock().unwrap();
        let tmux = IsolatedTmux::start();
        let endpoint = tmux.endpoint();
        let codex = tmux.fake_codex();
        tmux.send_command(&codex.background_command());

        let _pid = wait_for_fixture_ready(&endpoint, &codex, Duration::from_secs(3));
        let running = wait_for_view(&endpoint, Duration::from_secs(3), |observed| {
            observed["agent_presence"] == "online"
                && observed["reason"] == "current_codex_process_still_running"
        });
        assert_eq!(running["activity"], "unknown", "{running}");

        codex.terminate();
        let exited = wait_for_view(&endpoint, Duration::from_secs(7), |observed| {
            observed["agent_presence"] == "offline"
        });
        assert_eq!(exited["activity"], "unknown", "{exited}");
        assert_eq!(
            exited["reason"], "observed_codex_process_exited",
            "{exited}"
        );
    }

    #[test]
    fn view_does_not_report_background_codex_as_offline() {
        let _guard = crate::scope::TEST_ENV_LOCK.lock().unwrap();
        let tmux = IsolatedTmux::start();
        let endpoint = tmux.endpoint();
        let codex = tmux.fake_codex();
        tmux.send_command(&codex.background_command());

        let _pid = wait_for_fixture_ready(&endpoint, &codex, Duration::from_secs(3));
        let observed = wait_for_view(&endpoint, Duration::from_secs(2), |observed| {
            observed["agent_presence"] == "online"
        });
        assert_eq!(
            observed["reason"], "current_codex_process_still_running",
            "{observed}"
        );
        codex.terminate();
    }

    #[test]
    fn view_does_not_report_suspended_codex_as_offline() {
        let _guard = crate::scope::TEST_ENV_LOCK.lock().unwrap();
        let tmux = IsolatedTmux::start();
        let endpoint = tmux.endpoint();
        let codex = tmux.fake_codex();
        tmux.send_command(&codex.background_command());

        let pid = wait_for_fixture_ready(&endpoint, &codex, Duration::from_secs(3));
        unsafe {
            assert_eq!(libc::kill(pid as libc::pid_t, libc::SIGSTOP), 0);
        }
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        loop {
            let candidates = codex_candidates(&endpoint);
            if candidates
                .first()
                .is_some_and(|process| process.state.starts_with('T'))
            {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "the Codex fixture did not enter a stopped state"
            );
            std::thread::sleep(Duration::from_millis(20));
        }

        let observed = view(&endpoint).unwrap();
        assert_eq!(observed["agent_presence"], "online", "{observed}");
        assert_eq!(
            observed["reason"], "current_codex_process_still_running",
            "{observed}"
        );
        let candidates = codex_candidates(&endpoint);
        assert_eq!(candidates.len(), 1, "{candidates:?}");
        assert_eq!(candidates[0].pid, pid, "{candidates:?}");
        assert!(candidates[0].state.starts_with('T'), "{candidates:?}");
        codex.terminate();
    }

    #[test]
    fn view_keeps_remembered_detached_codex_online() {
        let _guard = crate::scope::TEST_ENV_LOCK.lock().unwrap();
        let tmux = IsolatedTmux::start();
        let endpoint = tmux.endpoint();
        let codex = tmux.fake_codex();
        let mut child = codex.spawn_detached();
        wait_for_detached_fixture_ready(&codex, child.id(), Duration::from_secs(3));
        let start_id = process_start_id(child.id()).unwrap().unwrap();
        remember_execution(
            execution_key(&endpoint),
            ObservedTmuxExecution {
                pid: child.id(),
                start_id,
            },
        )
        .unwrap();

        let observed = view(&endpoint).unwrap();
        assert_eq!(observed["agent_presence"], "online", "{observed}");
        assert_eq!(
            observed["reason"], "observed_codex_process_still_running",
            "{observed}"
        );
        unsafe {
            libc::kill(child.id() as libc::pid_t, libc::SIGTERM);
        }
        let _ = child.wait();
        codex.clear_pid();
    }

    #[test]
    fn view_uses_new_codex_execution_instead_of_previous_exit() {
        let _guard = crate::scope::TEST_ENV_LOCK.lock().unwrap();
        let tmux = IsolatedTmux::start();
        let endpoint = tmux.endpoint();
        let codex = tmux.fake_codex();
        tmux.send_command(&codex.background_command());

        let _pid = wait_for_fixture_ready(&endpoint, &codex, Duration::from_secs(3));
        let previous = wait_for_remembered_execution(&endpoint, Duration::from_secs(3), None);
        assert_eq!(codex.pid(), Some(previous.pid));
        codex.terminate();
        let exited = wait_for_view(&endpoint, Duration::from_secs(7), |observed| {
            observed["agent_presence"] == "offline"
        });
        assert_eq!(
            exited["reason"], "observed_codex_process_exited",
            "{exited}"
        );

        tmux.send_command(&codex.background_command());

        let _pid = wait_for_fixture_ready(&endpoint, &codex, Duration::from_secs(3));
        let observed = wait_for_view(&endpoint, Duration::from_secs(3), |observed| {
            observed["agent_presence"] == "online"
                && observed["reason"] == "current_codex_process_still_running"
        });
        assert_eq!(observed["agent_presence"], "online", "{observed}");
        assert_eq!(
            observed["reason"], "current_codex_process_still_running",
            "{observed}"
        );
        let current =
            wait_for_remembered_execution(&endpoint, Duration::from_secs(3), Some(previous.pid));
        assert_ne!(current.pid, previous.pid);
        assert_eq!(codex.pid(), Some(current.pid));
        codex.terminate();
    }

    struct ProcessFailureWrapper {
        root: PathBuf,
        original_path: Option<std::ffi::OsString>,
    }

    impl ProcessFailureWrapper {
        fn fail_ps() -> Self {
            let root = std::env::temp_dir().join(format!(
                "collab-ps-wrapper-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            std::fs::create_dir_all(&root).unwrap();
            let wrapper = root.join("ps");
            std::fs::write(&wrapper, "#!/bin/sh\nexit 71\n").unwrap();
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).unwrap();
            let original_path = std::env::var_os("PATH");
            let mut paths = vec![root.clone()];
            if let Some(original) = &original_path {
                paths.extend(std::env::split_paths(original));
            }
            std::env::set_var("PATH", std::env::join_paths(paths).unwrap());
            Self {
                root,
                original_path,
            }
        }
    }

    impl Drop for ProcessFailureWrapper {
        fn drop(&mut self) {
            match self.original_path.take() {
                Some(path) => std::env::set_var("PATH", path),
                None => std::env::remove_var("PATH"),
            }
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn view_keeps_shell_unknown_when_process_query_fails() {
        let _guard = crate::scope::TEST_ENV_LOCK.lock().unwrap();
        let tmux = IsolatedTmux::start();
        let endpoint = tmux.endpoint();
        let _wrapper = ProcessFailureWrapper::fail_ps();

        let observed = view(&endpoint).unwrap();
        assert_eq!(observed["agent_presence"], "unknown", "{observed}");
        assert_eq!(observed["activity"], "unknown", "{observed}");
        assert_eq!(observed["reason"], "process_query_failed", "{observed}");
    }

    #[test]
    #[ignore = "requires an installed Codex TUI; run only in the private tmux fixture"]
    fn real_codex_frontend_keeps_suspended_and_background_executions_online() {
        let _guard = crate::scope::TEST_ENV_LOCK.lock().unwrap();
        let tmux = IsolatedTmux::start();
        let endpoint = tmux.endpoint();
        let codex = std::env::var_os("COLLAB_REAL_CODEX")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                let output = Command::new("which")
                    .arg("codex")
                    .output()
                    .expect("locate installed Codex");
                assert!(output.status.success(), "installed Codex is required");
                PathBuf::from(
                    String::from_utf8(output.stdout)
                        .expect("Codex path is UTF-8")
                        .trim(),
                )
            });
        let home = tmux.root.join("home");
        let codex_home = tmux.root.join("codex-home");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(&codex_home).unwrap();
        let path = std::env::var("PATH").unwrap_or_default();
        let launch = format!(
            "env HOME={} CODEX_HOME={} PATH={} {} --no-alt-screen --no-daemon -C {}",
            shell_quote(&home.to_string_lossy()),
            shell_quote(&codex_home.to_string_lossy()),
            shell_quote(&path),
            shell_quote(&codex.to_string_lossy()),
            shell_quote(&tmux.root.to_string_lossy()),
        );

        tmux.send_command(&format!("{launch}; exec sh -i"));
        eprintln!("real Codex frontend: waiting for initial foreground process");
        let _running = wait_for_view(&endpoint, Duration::from_secs(15), |observed| {
            observed["agent_presence"] == "online"
        });
        let initial = wait_for_remembered_execution(&endpoint, Duration::from_secs(5), None);

        tmux.send_key("C-z");
        eprintln!("real Codex frontend: waiting for suspended process observation");
        let suspended = wait_for_view(&endpoint, Duration::from_secs(5), |observed| {
            observed["agent_presence"] == "online"
                && observed["reason"] == "current_codex_process_still_running"
        });
        assert_ne!(suspended["activity"], "offline");
        let stopped = codex_candidates(&endpoint);
        assert_eq!(stopped.len(), 1, "{stopped:?}");
        assert!(stopped[0].state.starts_with('T'), "{:?}", stopped[0]);

        terminate_test_process(stopped[0].pid, true);
        eprintln!("real Codex frontend: waiting for suspended process exit");
        let suspended_exited = wait_for_view(&endpoint, Duration::from_secs(10), |observed| {
            observed["agent_presence"] == "offline"
        });
        assert_eq!(
            suspended_exited["reason"], "observed_codex_process_exited",
            "{suspended_exited}"
        );

        tmux.send_command(&format!("{launch}; exec sh -i"));
        eprintln!("real Codex frontend: waiting for fresh foreground process");
        let normal = wait_for_view(&endpoint, Duration::from_secs(15), |observed| {
            observed["agent_presence"] == "online"
        });
        let normal_execution =
            wait_for_remembered_execution(&endpoint, Duration::from_secs(5), Some(initial.pid));
        assert_ne!(normal_execution.pid, initial.pid);
        assert_eq!(normal["agent_presence"], "online", "{normal}");

        tmux.send_key("C-c");
        eprintln!("real Codex frontend: waiting for normal foreground exit");
        let normal_exited = wait_for_view(&endpoint, Duration::from_secs(10), |observed| {
            observed["agent_presence"] == "offline"
        });
        assert_eq!(normal_exited["activity"], "unknown", "{normal_exited}");
        assert_eq!(
            normal_exited["reason"], "observed_codex_process_exited",
            "{normal_exited}"
        );

        tmux.send_command(&format!("{launch} &"));
        eprintln!("real Codex frontend: waiting for background process");
        let background = wait_for_view(&endpoint, Duration::from_secs(15), |observed| {
            observed["agent_presence"] == "online"
                && observed["reason"] == "current_codex_process_still_running"
        });
        assert_eq!(background["activity"], "unknown", "{background}");
        let background_processes = codex_candidates(&endpoint);
        assert_eq!(background_processes.len(), 1, "{background_processes:?}");
        let background_pid = background_processes[0].pid;
        assert_ne!(background_pid, initial.pid);
        terminate_test_process(background_pid, false);
        let first_background_exited =
            wait_for_view(&endpoint, Duration::from_secs(10), |observed| {
                observed["agent_presence"] == "offline"
            });
        assert_eq!(
            first_background_exited["reason"], "observed_codex_process_exited",
            "{first_background_exited}"
        );

        tmux.send_command(&format!("{launch} &"));
        eprintln!("real Codex frontend: waiting for replacement process");
        let replacement = wait_for_view(&endpoint, Duration::from_secs(15), |observed| {
            observed["agent_presence"] == "online"
                && observed["reason"] == "current_codex_process_still_running"
        });
        assert_eq!(replacement["activity"], "unknown", "{replacement}");
        let replacement_processes = codex_candidates(&endpoint);
        assert_eq!(replacement_processes.len(), 1, "{replacement_processes:?}");
        let replacement_pid = replacement_processes[0].pid;
        assert_ne!(replacement_pid, background_pid);

        for process in codex_candidates(&endpoint) {
            terminate_test_process(process.pid, false);
        }
    }

    #[test]
    fn snapshot_returns_only_the_latest_requested_lines() {
        let _guard = crate::scope::TEST_ENV_LOCK.lock().unwrap();
        let tmux = IsolatedTmux::start_with_command(Some(
            "i=1; while [ $i -le 30 ]; do printf 'SNAP-%03d\\n' \"$i\"; i=$((i+1)); done; sleep 60",
        ));
        let endpoint = tmux.endpoint();
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        loop {
            if capture(&endpoint).unwrap().contains("SNAP-030") {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "snapshot fixture did not publish its terminal line"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        let five = snapshot(&endpoint, 5).unwrap();
        let five_lines = five["output"].as_str().unwrap().lines().collect::<Vec<_>>();
        assert_eq!(five_lines.len(), 5, "{five:?}");
        assert_eq!(five_lines.last().copied(), Some("SNAP-030"));
        assert!(!five["output"].as_str().unwrap().contains("SNAP-025"));
        assert_eq!(five["durable"], false);

        let one = snapshot(&endpoint, 1).unwrap();
        assert_eq!(one["output"], "SNAP-030");
        let all = snapshot(&endpoint, 200).unwrap();
        assert_eq!(all["output"].as_str().unwrap().lines().count(), 30);
    }

    #[test]
    fn snapshot_capture_failure_is_explicit_and_does_not_return_text() {
        let _guard = crate::scope::TEST_ENV_LOCK.lock().unwrap();
        let tmux = IsolatedTmux::start();
        let endpoint = tmux.endpoint();
        let _wrapper = TmuxFailureWrapper::fail_command("capture-pane");
        let error = snapshot(&endpoint, 40).unwrap_err();
        assert!(error.starts_with("TMUX_CAPTURE_FAILED: forced capture-pane failure"));
    }

    #[test]
    fn pane_observation_cache_stays_bounded() {
        for index in 0..(MAX_PANE_OBSERVATIONS + 20) {
            record_observation(format!("cache-boundary-{index}"), index as u64).unwrap();
        }
        let observations = LAST_PANE_OUTPUT.get().unwrap().lock().unwrap();
        assert!(observations.len() <= MAX_PANE_OBSERVATIONS);
    }

    #[test]
    fn notify_pastes_text_then_sends_enter_and_never_claims_consumption() {
        let _guard = crate::scope::TEST_ENV_LOCK.lock().unwrap();
        let tmux = IsolatedTmux::start_with_command(Some("cat"));
        let endpoint = tmux.endpoint();
        let receipt = notify(&endpoint, "message-1", "collab durable wake").unwrap();
        assert_eq!(receipt["text_submitted"], true);
        assert_eq!(receipt["enter_submitted"], true);
        assert_eq!(receipt["consumed"], false);
        assert_eq!(receipt["pane_output_changed_after_enter"], true);
        assert!(capture(&endpoint).unwrap().contains("collab durable wake"));
    }

    #[test]
    fn failed_paste_is_explicit_and_does_not_report_submission() {
        let _guard = crate::scope::TEST_ENV_LOCK.lock().unwrap();
        let tmux = IsolatedTmux::start_with_command(Some("cat"));
        let endpoint = tmux.endpoint();
        let _wrapper = TmuxFailureWrapper::fail_command("paste-buffer");

        let error = notify(&endpoint, "message-paste-failure", "must not be consumed").unwrap_err();
        assert!(error.starts_with("TMUX_TEXT_SUBMIT_FAILED: forced paste-buffer failure"));
        assert!(!capture(&endpoint).unwrap().contains("must not be consumed"));
    }

    #[test]
    fn failed_enter_is_explicit_and_does_not_report_consumption() {
        let _guard = crate::scope::TEST_ENV_LOCK.lock().unwrap();
        let tmux = IsolatedTmux::start_with_command(Some("cat"));
        let endpoint = tmux.endpoint();
        let _wrapper = TmuxFailureWrapper::fail_command("send-keys");

        let error = notify(
            &endpoint,
            "message-enter-failure",
            "paste happened, no receive",
        )
        .unwrap_err();
        assert!(error.starts_with("TMUX_ENTER_SUBMIT_FAILED: forced send-keys failure"));
        assert!(capture(&endpoint)
            .unwrap()
            .contains("paste happened, no receive"));
        // A failed wake returns Err, never the transport result consumed=false/true object.
    }

    #[test]
    fn session_name_is_observed_and_rename_is_idempotent() {
        let _guard = crate::scope::TEST_ENV_LOCK.lock().unwrap();
        let tmux = IsolatedTmux::start();
        let endpoint = tmux.endpoint();
        assert_eq!(observe_session_name(&endpoint).unwrap(), "collab-test");

        rename_session(&endpoint, "Jenny").unwrap();
        assert_eq!(observe_session_name(&endpoint).unwrap(), "Jenny");

        // Renaming to the fixed name that is already in place is a no-op
        // success; callers must not treat a repeated context as a rename error.
        rename_session(&endpoint, "Jenny").unwrap();
        assert_eq!(observe_session_name(&endpoint).unwrap(), "Jenny");
    }

    #[test]
    fn drop_cleans_up_isolated_server_after_session_rename() {
        let _guard = crate::scope::TEST_ENV_LOCK.lock().unwrap();
        let (socket, root, server_pid);
        {
            let tmux = IsolatedTmux::start();
            let endpoint = tmux.endpoint();
            server_pid = endpoint.server_pid;
            socket = tmux.socket.clone();
            root = tmux.root.clone();
            rename_session(&endpoint, "Jenny").unwrap();
        }
        // Drop has run. The socket must be gone, and the tmux server must have
        // exited instead of surviving as an unreachable orphan.
        assert!(!socket.exists(), "socket root must be removed");
        assert!(!root.exists(), "temp root must be removed");
        let alive = Command::new("ps")
            .arg("-p")
            .arg(server_pid.to_string())
            .output()
            .expect("query tmux server pid");
        assert!(
            !alive.status.success(),
            "tmux server {} leaked after session rename",
            server_pid
        );
    }

    #[test]
    fn session_name_if_live_returns_none_for_absent_session_on_live_server() {
        let _guard = crate::scope::TEST_ENV_LOCK.lock().unwrap();
        let tmux = IsolatedTmux::start();
        let mut endpoint = tmux.endpoint();
        assert!(session_name_if_live(&endpoint).unwrap().is_some());
        endpoint.tmux_session_id = "$999999".into();
        assert_eq!(session_name_if_live(&endpoint).unwrap(), None);
    }

    #[test]
    fn session_name_if_live_keeps_a_different_server_pid_as_absence() {
        let _guard = crate::scope::TEST_ENV_LOCK.lock().unwrap();
        let tmux = IsolatedTmux::start();
        let mut endpoint = tmux.endpoint();
        assert!(session_name_if_live(&endpoint).unwrap().is_some());
        endpoint.server_pid += 1;
        assert_eq!(session_name_if_live(&endpoint).unwrap(), None);
    }

    #[test]
    fn session_name_if_live_keeps_missing_tmux_executable_as_an_error() {
        let _guard = crate::scope::TEST_ENV_LOCK.lock().unwrap();
        let tmux = IsolatedTmux::start();
        let endpoint = tmux.endpoint();
        let path = std::env::temp_dir().join(format!(
            "collab-tmux-empty-path-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&path).unwrap();
        let original_path = std::env::var_os("PATH");
        std::env::set_var("PATH", &path);
        let result = session_name_if_live(&endpoint);
        match original_path {
            Some(value) => std::env::set_var("PATH", value),
            None => std::env::remove_var("PATH"),
        }
        std::fs::remove_dir_all(path).unwrap();
        let error = result.unwrap_err();
        assert!(
            error.starts_with("TMUX_SESSION_NAME_QUERY_FAILED: start tmux:"),
            "{error}"
        );
    }

    #[test]
    fn session_name_if_live_keeps_inventory_command_failure_as_an_error() {
        let _guard = crate::scope::TEST_ENV_LOCK.lock().unwrap();
        let tmux = IsolatedTmux::start();
        let endpoint = tmux.endpoint();
        let _wrapper = TmuxFailureWrapper::fail_command("list-sessions");

        let error = session_name_if_live(&endpoint).unwrap_err();
        assert!(
            error.starts_with("TMUX_SESSION_NAME_QUERY_FAILED:"),
            "{error}"
        );
        assert!(error.contains("forced list-sessions failure"), "{error}");
    }

    #[test]
    fn session_name_if_live_returns_none_for_absent_server() {
        let endpoint = TmuxEndpoint {
            socket_path: std::env::temp_dir()
                .join("collab-session-name-if-live-absent.sock")
                .to_string_lossy()
                .into_owned(),
            server_pid: 42,
            tmux_session_id: "$1".into(),
            pane_id: "%1".into(),
            pane_pid: 99,
            codex_session_id: None,
            codex_thread_id: None,
        };
        // A non-TUI caller has no live anchor; the optional observation skips.
        assert_eq!(session_name_if_live(&endpoint).unwrap(), None);
    }

    #[test]
    fn registered_tui_endpoint_requires_a_live_readable_session() {
        let endpoint = TmuxEndpoint {
            socket_path: std::env::temp_dir()
                .join("collab-registered-tui-unreachable.sock")
                .to_string_lossy()
                .into_owned(),
            server_pid: 42,
            tmux_session_id: "$1".into(),
            pane_id: "%1".into(),
            pane_pid: 99,
            codex_session_id: None,
            codex_thread_id: None,
        };
        // A registered TUI binding is not optional: its endpoint must be live
        // and readable before a name commit, not silently skipped.
        let error = require_live_session_name(&endpoint).unwrap_err();
        assert!(error.starts_with("TMUX_TUI_ANCHOR_UNREACHABLE:"), "{error}");
    }

    #[test]
    fn registered_tui_endpoint_reads_name_when_the_session_is_live() {
        let _guard = crate::scope::TEST_ENV_LOCK.lock().unwrap();
        let tmux = IsolatedTmux::start();
        let endpoint = tmux.endpoint();
        assert_eq!(require_live_session_name(&endpoint).unwrap(), "collab-test");
    }

    #[test]
    fn rename_session_rejects_malformed_fixed_name() {
        for name in ["", " ", "a:b", "a.b", "a b", "-x"] {
            assert!(
                validate_session_name(name).is_err(),
                "{name:?} must not be a valid fixed session name"
            );
        }
        assert!(validate_session_name("Jenny").is_ok());
        assert!(validate_session_name("appsdk_Jenny").is_ok());
    }

    #[test]
    fn failed_rename_is_explicit_and_keeps_the_previous_name() {
        let _guard = crate::scope::TEST_ENV_LOCK.lock().unwrap();
        let tmux = IsolatedTmux::start();
        let endpoint = tmux.endpoint();
        let _wrapper = TmuxFailureWrapper::fail_command("rename-session");

        let error = rename_session(&endpoint, "Jenny").unwrap_err();
        assert!(
            error.starts_with("TMUX_SESSION_RENAME_FAILED: forced rename-session failure"),
            "{error}"
        );
        // The anchor must not change when the rename fails.
        assert_eq!(observe_session_name(&endpoint).unwrap(), "collab-test");
    }
}
