use crate::proto::{TmuxCandidate, TmuxEndpoint};
use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::{Mutex, OnceLock};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PanePresence {
    Present,
    Missing,
    Unknown,
}

static LAST_PANE_OUTPUT: OnceLock<Mutex<std::collections::HashMap<String, u64>>> = OnceLock::new();
const MAX_PANE_OBSERVATIONS: usize = 256;

pub fn candidate_from_env() -> Result<TmuxCandidate, String> {
    let tmux = std::env::var("TMUX").map_err(|_| {
        "TMUX_ENDPOINT_MISSING: run this command from the registered tmux pane, then retry"
            .to_owned()
    })?;
    let pane_id = std::env::var("TMUX_PANE")
        .map_err(|_| "TMUX_ENDPOINT_MISSING: TMUX_PANE is unavailable; run this command from the registered tmux pane, then retry".to_owned())?;
    validate_pane_id(&pane_id)?;
    let mut parts = tmux.rsplitn(3, ',');
    let _client_session = parts.next();
    let server_pid = parts
        .next()
        .and_then(|part| part.parse::<u32>().ok())
        .ok_or_else(|| "TMUX_ENDPOINT_INVALID: malformed tmux server pid".to_owned())?;
    let socket_path = parts
        .next()
        .filter(|path| path.starts_with('/'))
        .ok_or_else(|| "TMUX_ENDPOINT_INVALID: malformed tmux socket path".to_owned())?;
    let output = tmux_command(
        socket_path,
        &[
            "display-message",
            "-p",
            "-t",
            &pane_id,
            "#{session_id}\t#{pane_id}\t#{pane_pid}",
        ],
    )?;
    let line = String::from_utf8(output.stdout)
        .map_err(|_| "TMUX_ENDPOINT_INVALID: non-UTF-8 tmux endpoint".to_owned())?;
    let mut fields = line.trim_end().split('\t');
    let tmux_session_id = fields.next().unwrap_or_default().to_owned();
    let observed_pane_id = fields.next().unwrap_or_default();
    let pane_pid = fields
        .next()
        .and_then(|value| value.parse::<u32>().ok())
        .ok_or_else(|| "TMUX_ENDPOINT_INVALID: malformed pane pid".to_owned())?;
    if !tmux_session_id.starts_with('$') || observed_pane_id != pane_id {
        return Err("TMUX_ENDPOINT_INVALID: pane identity changed during discovery".into());
    }
    let cwd = std::env::current_dir()
        .and_then(std::fs::canonicalize)
        .map_err(|error| format!("TMUX_ENDPOINT_INVALID: resolve cwd: {error}"))?
        .to_str()
        .ok_or_else(|| "TMUX_ENDPOINT_INVALID: cwd is not UTF-8".to_owned())?
        .to_owned();
    let candidate = TmuxCandidate {
        endpoint: TmuxEndpoint {
            socket_path: socket_path.to_owned(),
            server_pid,
            tmux_session_id,
            pane_id,
            pane_pid,
            codex_session_id: nonempty_env("CODEX_SESSION_ID"),
            codex_thread_id: nonempty_env("CODEX_THREAD_ID"),
        },
        cwd,
    };
    validate_endpoint(&candidate.endpoint)?;
    Ok(candidate)
}

fn nonempty_env(key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .filter(|value| !value.trim().is_empty())
}

pub fn probe(endpoint: &TmuxEndpoint) -> Result<PanePresence, String> {
    validate_endpoint(endpoint)?;
    let args = [
        "display-message",
        "-p",
        "-t",
        endpoint.pane_id.as_str(),
        "#{pid}\t#{session_id}\t#{pane_id}\t#{pane_pid}\t#{pane_dead}",
    ];
    match tmux_command(&endpoint.socket_path, &args) {
        Ok(output) => {
            let line = String::from_utf8_lossy(&output.stdout);
            let mut fields = line.trim_end().split('\t');
            let server_pid = fields.next().and_then(|value| value.parse::<u32>().ok());
            let session = fields.next().unwrap_or_default();
            let pane = fields.next().unwrap_or_default();
            let pid = fields.next().and_then(|value| value.parse::<u32>().ok());
            let dead = fields.next();
            if server_pid != Some(endpoint.server_pid)
                || session != endpoint.tmux_session_id
                || pane != endpoint.pane_id
                || pid != Some(endpoint.pane_pid)
                || dead == Some("1")
            {
                return Ok(PanePresence::Missing);
            }
            if dead == Some("0") {
                Ok(PanePresence::Present)
            } else {
                Ok(PanePresence::Unknown)
            }
        }
        Err(error) if error.contains("can't find pane") => Ok(PanePresence::Missing),
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

pub fn view(endpoint: &TmuxEndpoint) -> Result<serde_json::Value, String> {
    match probe(endpoint)? {
        PanePresence::Missing => return Err("TMUX_PANE_MISSING: target pane is gone".into()),
        PanePresence::Unknown => return Err("TMUX_PANE_UNKNOWN: target pane is uncertain".into()),
        PanePresence::Present => {}
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
    let (changed, observation) = record_observation(key, digest)?;
    Ok(serde_json::json!({
        "thread": {
            "status": {"type": "unknown"},
            "canAcceptDirectInput": null,
            "turns": [{"status": "unknown"}]
        },
        "thread_state": "unknown",
        "pane_output_changed": changed,
        "pane_output_observation": observation,
        "transport": "tmux"
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

fn buffer_name(message_id: &str) -> String {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    message_id.hash(&mut hasher);
    format!("collab-{:016x}", hasher.finish())
}

fn validate_pane_id(id: &str) -> Result<(), String> {
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

pub fn same_pane_route(left: &TmuxEndpoint, right: &TmuxEndpoint) -> bool {
    left.socket_path == right.socket_path
        && left.server_pid == right.server_pid
        && left.tmux_session_id == right.tmux_session_id
        && left.pane_id == right.pane_id
        && left.pane_pid == right.pane_pid
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
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::time::Duration;

    struct IsolatedTmux {
        socket: PathBuf,
        root: PathBuf,
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
            let mut command = Command::new("tmux");
            command
                .arg("-S")
                .arg(&socket)
                .args(["new-session", "-d", "-s", "collab-test"]);
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
            Self { socket, root }
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
                .args(["kill-server"])
                .output();
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    struct TmuxFailureWrapper {
        root: PathBuf,
        original_path: Option<std::ffi::OsString>,
    }

    impl TmuxFailureWrapper {
        fn fail_command(command_name: &str) -> Self {
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
    fn candidate_from_env_resolves_the_current_isolated_tmux_pane() {
        let _guard = crate::scope::TEST_ENV_LOCK.lock().unwrap();
        let tmux = IsolatedTmux::start();
        let endpoint = tmux.endpoint();
        let previous_tmux = std::env::var_os("TMUX");
        let previous_pane = std::env::var_os("TMUX_PANE");
        let previous_session = std::env::var_os("CODEX_SESSION_ID");
        let previous_thread = std::env::var_os("CODEX_THREAD_ID");
        std::env::set_var(
            "TMUX",
            format!("{},{},0", endpoint.socket_path, endpoint.server_pid),
        );
        std::env::set_var("TMUX_PANE", &endpoint.pane_id);
        std::env::set_var("CODEX_SESSION_ID", "codex-session-test");
        std::env::set_var("CODEX_THREAD_ID", "codex-thread-test");
        let candidate = candidate_from_env();
        match previous_tmux {
            Some(value) => std::env::set_var("TMUX", value),
            None => std::env::remove_var("TMUX"),
        }
        match previous_pane {
            Some(value) => std::env::set_var("TMUX_PANE", value),
            None => std::env::remove_var("TMUX_PANE"),
        }
        match previous_session {
            Some(value) => std::env::set_var("CODEX_SESSION_ID", value),
            None => std::env::remove_var("CODEX_SESSION_ID"),
        }
        match previous_thread {
            Some(value) => std::env::set_var("CODEX_THREAD_ID", value),
            None => std::env::remove_var("CODEX_THREAD_ID"),
        }
        let candidate = candidate.unwrap();
        assert_eq!(candidate.endpoint.socket_path, endpoint.socket_path);
        assert_eq!(candidate.endpoint.server_pid, endpoint.server_pid);
        assert_eq!(candidate.endpoint.tmux_session_id, endpoint.tmux_session_id);
        assert_eq!(candidate.endpoint.pane_id, endpoint.pane_id);
        assert_eq!(candidate.endpoint.pane_pid, endpoint.pane_pid);
        assert_eq!(
            candidate.endpoint.codex_session_id.as_deref(),
            Some("codex-session-test")
        );
        assert_eq!(
            candidate.endpoint.codex_thread_id.as_deref(),
            Some("codex-thread-test")
        );
        assert!(!candidate.cwd.is_empty());
    }

    #[test]
    fn pane_route_identity_includes_socket_session_and_process_generation() {
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
        for changed in [
            TmuxEndpoint {
                socket_path: "/tmp/tmux-b.sock".into(),
                ..endpoint.clone()
            },
            TmuxEndpoint {
                server_pid: 12,
                ..endpoint.clone()
            },
            TmuxEndpoint {
                tmux_session_id: "$3".into(),
                ..endpoint.clone()
            },
            TmuxEndpoint {
                pane_id: "%4".into(),
                ..endpoint.clone()
            },
            TmuxEndpoint {
                pane_pid: 34,
                ..endpoint.clone()
            },
        ] {
            assert!(!same_pane_route(&endpoint, &changed));
        }
    }

    #[test]
    fn buffer_name_is_stable_and_does_not_copy_message_text() {
        assert_eq!(buffer_name("message-1"), buffer_name("message-1"));
        assert!(!buffer_name("secret body").contains("secret"));
    }

    #[test]
    fn pane_probe_distinguishes_present_missing_and_unknown() {
        let tmux = IsolatedTmux::start();
        let endpoint = tmux.endpoint();
        assert_eq!(probe(&endpoint).unwrap(), PanePresence::Present);
        let restarted = TmuxEndpoint {
            server_pid: endpoint.server_pid + 1,
            ..endpoint.clone()
        };
        assert_eq!(probe(&restarted).unwrap(), PanePresence::Missing);

        let killed = Command::new("tmux")
            .arg("-S")
            .arg(&tmux.socket)
            .args(["kill-pane", "-t", &endpoint.pane_id])
            .output()
            .unwrap();
        assert!(killed.status.success());
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
        let tmux = IsolatedTmux::start();
        let endpoint = tmux.endpoint();
        let initial = view(&endpoint).unwrap();
        assert_eq!(initial["thread_state"], "unknown");
        std::thread::sleep(Duration::from_millis(1600));

        let stable = view(&endpoint).unwrap();
        assert_eq!(stable["thread_state"], "unknown");
        assert_eq!(stable["thread"]["status"]["type"], "unknown");
        assert!(stable["pane_output_changed"].is_boolean());
        assert!(matches!(
            stable["pane_output_observation"].as_str(),
            Some("initial" | "changed" | "stable")
        ));
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
