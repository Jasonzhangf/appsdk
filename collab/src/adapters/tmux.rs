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
    let endpoint = observe_caller_at(host_paths.state_root(), std::process::id(), hint.as_deref())?
        .ok_or_else(|| {
            "TMUX_OBSERVATION_UNAVAILABLE: caller ancestry does not identify a live tmux pane"
                .to_owned()
        })?;
    let cwd = std::env::current_dir()
        .and_then(|path| path.canonicalize())
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
            "#{pid}\t#{session_id}\t#{pane_id}\t#{pane_pid}",
        ],
    )?;
    let text = String::from_utf8(output.stdout)
        .map_err(|_| "TMUX_OBSERVATION_INVALID: non-UTF-8 tmux metadata".to_owned())?;
    text.lines()
        .map(|line| {
            let fields = line.split('\t').collect::<Vec<_>>();
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

    let foreground_command = match foreground_command(endpoint) {
        Ok(command) => command,
        Err(_) => return Ok(AgentObservation::unknown("foreground_query_failed")),
    };
    if let Some(observation) = codex_presence::classify_without_screen(&foreground_command) {
        return Ok(observation);
    }

    let visible_screen = match capture_visible(endpoint) {
        Ok(screen) => screen,
        Err(_) => {
            return Ok(AgentObservation {
                agent_presence: AgentPresence::Online,
                activity: AgentActivity::Unknown,
                background: false,
                foreground_command: Some(foreground_command),
                reason: "visible_screen_unavailable".into(),
            });
        }
    };
    Ok(codex_presence::classify_codex(
        &foreground_command,
        &visible_screen,
    ))
}

pub fn view(endpoint: &TmuxEndpoint) -> Result<serde_json::Value, String> {
    let observation = observe_agent(endpoint)?;
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
    let (thread_state, thread_status, can_accept_direct_input) = match observation.agent_presence {
        AgentPresence::Online => match observation.activity {
            AgentActivity::Busy => ("working", "working", Some(false)),
            AgentActivity::Idle => ("idle", "idle", Some(true)),
            AgentActivity::WaitingUser => ("waiting_user", "waiting_user", Some(false)),
            AgentActivity::Unknown => ("unknown", "unknown", None),
        },
        AgentPresence::Offline | AgentPresence::Unknown => ("unknown", "unknown", None),
    };
    Ok(serde_json::json!({
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
    }))
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

fn foreground_command(endpoint: &TmuxEndpoint) -> Result<String, String> {
    let output = tmux_command(
        &endpoint.socket_path,
        &[
            "list-panes",
            "-t",
            &endpoint.pane_id,
            "-F",
            "#{session_id}\t#{pane_id}\t#{pane_current_command}",
        ],
    )
    .map_err(|error| format!("TMUX_FOREGROUND_QUERY_FAILED: {error}"))?;
    let text = String::from_utf8(output.stdout)
        .map_err(|_| "TMUX_FOREGROUND_QUERY_FAILED: non-UTF-8 tmux metadata".to_owned())?;
    codex_presence::parse_foreground_command(&endpoint.pane_id, &text)
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
    use std::path::PathBuf;
    use std::time::Duration;

    struct IsolatedTmux {
        socket: PathBuf,
        root: PathBuf,
        session: String,
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
            let session = "collab-test".to_owned();
            let mut command = Command::new("tmux");
            command
                .arg("-S")
                .arg(&socket)
                .args(["new-session", "-d", "-s", &session]);
            if let Some(pane_command) = pane_command {
                command.arg(pane_command);
            }
            let output = command.output().expect("start isolated tmux server");
            assert!(
                output.status.success(),
                "start isolated tmux: {}",
                String::from_utf8_lossy(&output.stderr)
            );
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
                session,
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
                server_pid: fields.next().unwrap().parse().unwrap(),
                tmux_session_id: fields.next().unwrap().into(),
                pane_id: fields.next().unwrap().into(),
                pane_pid: fields.next().unwrap().parse().unwrap(),
                codex_session_id: None,
                codex_thread_id: None,
            }
        }
    }

    impl Drop for IsolatedTmux {
        fn drop(&mut self) {
            let _ = Command::new("tmux")
                .arg("-S")
                .arg(&self.socket)
                .args(["kill-session", "-t", &self.session])
                .output();
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    pub(crate) struct TmuxFailureWrapper {
        root: PathBuf,
        original_path: Option<std::ffi::OsString>,
    }

    impl TmuxFailureWrapper {
        pub(crate) fn fail_command(command_name: &str) -> Self {
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
                    "#!/bin/sh\nfor arg do\n  if [ \"$arg\" = \"{command_name}\" ]; then\n    echo 'forced {command_name} failure' >&2\n    exit 71\n  fi\ndone\nexec '{real_tmux}' \"$@\"\n"
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
        assert_eq!(stable["agent_presence"], "offline");
        assert_eq!(stable["activity"], "unknown");
        assert!(stable["pane_output_changed"].is_boolean());
        assert!(matches!(
            stable["pane_output_observation"].as_str(),
            Some("initial" | "changed" | "stable")
        ));
    }

    #[test]
    fn view_reports_shell_offline_despite_historical_codex_busy_text() {
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
        assert_eq!(observed["agent_presence"], "offline", "{observed}");
        assert_eq!(observed["activity"], "unknown", "{observed}");
        assert_eq!(observed["background"], false, "{observed}");
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
}
