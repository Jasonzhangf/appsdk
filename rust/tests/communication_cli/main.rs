use chrono::{DateTime, Duration, SecondsFormat};
use serde_json::{json, Value};
use std::fs;
use std::io::Write;
#[cfg(unix)]
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use std::process::Command;

fn binary() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_appsdk"))
}

fn temp_root(name: &str) -> PathBuf {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = std::env::temp_dir().join(format!("appsdk-communication-{name}-{nonce}"));
    fs::create_dir_all(&root).unwrap();
    root
}

fn call(root: &Path, request: Value) -> Value {
    call_with_host(root, &root.join(".appsdk-host"), request)
}

fn call_with_host(root: &Path, host: &Path, request: Value) -> Value {
    let output = Command::new(binary())
        .args([
            "communication",
            root.to_str().unwrap(),
            "--json",
            &serde_json::to_string(&request).unwrap(),
        ])
        .env("APPSDK_HOME", host)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "request failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

fn call_error(root: &Path, request: Value) -> String {
    call_error_with_host(root, &root.join(".appsdk-host"), request)
}

fn call_error_with_host(root: &Path, host: &Path, request: Value) -> String {
    let output = Command::new(binary())
        .args([
            "communication",
            root.to_str().unwrap(),
            "--json",
            &serde_json::to_string(&request).unwrap(),
        ])
        .env("APPSDK_HOME", host)
        .output()
        .unwrap();
    assert!(!output.status.success());
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn delivery_attempt_fields(root: &Path, message_id: &str) -> (String, String) {
    let mailbox = root.join(".appsdk-control/communication/mailbox.jsonl");
    let contents = fs::read_to_string(mailbox).unwrap();
    contents
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .find_map(|event| {
            (event["kind"] == "message.delivery_attempt"
                && event["data"]["messageId"] == message_id)
                .then(|| {
                    (
                        event["data"]["attempt"]["attemptId"]
                            .as_str()
                            .unwrap()
                            .to_owned(),
                        event["data"]["attempt"]["nonce"]
                            .as_str()
                            .unwrap()
                            .to_owned(),
                    )
                })
        })
        .unwrap()
}

fn count_events(events: &[Value], kind: &str) -> usize {
    events.iter().filter(|event| event["kind"] == kind).count()
}

fn retain_mailbox_through(root: &Path, event_kind: &str) {
    let mailbox = root.join(".appsdk-control/communication/mailbox.jsonl");
    let contents = fs::read_to_string(&mailbox).unwrap();
    let lines: Vec<&str> = contents.lines().collect();
    let marker = format!("\"kind\":\"{event_kind}\"");
    let end = lines
        .iter()
        .position(|line| line.contains(&marker))
        .unwrap();
    let retained = lines[..=end].join("\n");
    fs::write(mailbox, format!("{retained}\n")).unwrap();
}

fn retain_mailbox_through_last(root: &Path, event_kind: &str) {
    let mailbox = root.join(".appsdk-control/communication/mailbox.jsonl");
    let contents = fs::read_to_string(&mailbox).unwrap();
    let lines: Vec<&str> = contents.lines().collect();
    let marker = format!("\"kind\":\"{event_kind}\"");
    let end = lines
        .iter()
        .rposition(|line| line.contains(&marker))
        .unwrap();
    let retained = lines[..=end].join("\n");
    fs::write(mailbox, format!("{retained}\n")).unwrap();
}

fn retain_mailbox_through_occurrence(root: &Path, event_kind: &str, occurrence: usize) {
    assert!(occurrence > 0);
    let mailbox = root.join(".appsdk-control/communication/mailbox.jsonl");
    let contents = fs::read_to_string(&mailbox).unwrap();
    let lines: Vec<&str> = contents.lines().collect();
    let marker = format!("\"kind\":\"{event_kind}\"");
    let end = lines
        .iter()
        .enumerate()
        .filter(|(_, line)| line.contains(&marker))
        .nth(occurrence - 1)
        .map(|(index, _)| index)
        .unwrap();
    let retained = lines[..=end].join("\n");
    fs::write(mailbox, format!("{retained}\n")).unwrap();
}

fn remove_mailbox_events(root: &Path, event_kind: &str) {
    let mailbox = root.join(".appsdk-control/communication/mailbox.jsonl");
    let marker = format!("\"kind\":\"{event_kind}\"");
    let contents = fs::read_to_string(&mailbox).unwrap();
    let retained: Vec<&str> = contents
        .lines()
        .filter(|line| !line.contains(&marker))
        .collect();
    fs::write(mailbox, format!("{}\n", retained.join("\n"))).unwrap();
}

fn register_scope(
    root: &Path,
    scope_id: &str,
    appserver_id: &str,
    _project_root: &str,
    sessions: &[&str],
) {
    register_scope_with_host(
        root,
        &root.join(".appsdk-host"),
        scope_id,
        appserver_id,
        sessions,
    );
}

fn register_scope_with_host(
    root: &Path,
    host: &Path,
    scope_id: &str,
    appserver_id: &str,
    sessions: &[&str],
) {
    let runtime_id = format!("runtime-{scope_id}");
    let bound_project_root = root.canonicalize().unwrap();
    let bound_project_root = bound_project_root.to_str().unwrap();
    call_with_host(
        root,
        host,
        json!({
            "op": "register_runtime",
            "runtime": {
                "runtimeId": runtime_id,
                "appserverId": appserver_id,
                "namespace": "codex_tui",
                "endpoint": format!("mock://{scope_id}"),
                "projectRoot": bound_project_root,
                "capabilities": ["send_message_to_thread"],
                "processId": std::process::id()
            }
        }),
    );
    call_with_host(
        root,
        host,
        json!({
            "op": "register_scope",
            "scope": {
                "scopeId": scope_id,
                "appserverId": appserver_id,
                "namespace": "codex_tui",
                "endpoint": format!("mock://{scope_id}"),
                "projectRoot": bound_project_root,
                "sessionIds": sessions,
                "runtimeId": format!("runtime-{scope_id}")
            }
        }),
    );
}

fn register_agent(
    root: &Path,
    scope_id: &str,
    session_id: &str,
    agent_id: &str,
    role: &str,
    parent: Option<Value>,
) {
    register_agent_with_host(
        root,
        &root.join(".appsdk-host"),
        scope_id,
        session_id,
        agent_id,
        role,
        parent,
    );
}

fn register_agent_with_host(
    root: &Path,
    host: &Path,
    scope_id: &str,
    session_id: &str,
    agent_id: &str,
    role: &str,
    parent: Option<Value>,
) {
    let mut agent = json!({
        "scopeId": scope_id,
        "sessionId": session_id,
        "agentId": agent_id,
        "role": role,
        "runtimeId": format!("runtime-{scope_id}")
    });
    if role == "master" {
        agent["masterGrant"] = json!("user approved master for this scope");
    }
    if let Some(parent) = parent {
        agent["parent"] = parent;
    }
    call_with_host(
        root,
        host,
        json!({ "op": "register_agent", "agent": agent }),
    );
}

fn register_agent_with_lease(
    root: &Path,
    scope_id: &str,
    session_id: &str,
    agent_id: &str,
    role: &str,
    lease_ms: u64,
) -> Value {
    let mut agent = json!({
        "scopeId": scope_id,
        "sessionId": session_id,
        "agentId": agent_id,
        "role": role,
        "leaseMs": lease_ms,
        "runtimeId": format!("runtime-{scope_id}")
    });
    if role == "master" {
        agent["masterGrant"] = json!("user approved master for this scope");
    }
    call(root, json!({ "op": "register_agent", "agent": agent }))
}

fn after(timestamp: &str, seconds: i64) -> String {
    (DateTime::parse_from_rfc3339(timestamp).unwrap() + Duration::seconds(seconds))
        .to_rfc3339_opts(SecondsFormat::Millis, true)
}

mod runtime_identity;
mod routing_discovery;
mod idle_wakeup;
mod delivery_retry;
mod master_wake;
mod rebind_loops;
