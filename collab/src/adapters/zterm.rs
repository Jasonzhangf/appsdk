//! External `zterm` CLI adapter for remote slot/manager discovery and cross-machine delivery.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::process::Command;

const ZTERM_BIN_ENV: &str = "ZTERM_BIN";
const LIST_MANAGERS_SUBJECT: &str = "collab:list-managers";
const LIST_MANAGERS_REPLY_SUBJECT: &str = "collab:list-managers-reply";
const REMOTE_REPORT_SUBJECT: &str = "collab:remote-report";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ZtermInboundMessage {
    pub message_id: String,
    pub sender_device_id: String,
    #[serde(default)]
    pub sender_daemon_host_id: String,
    #[serde(default)]
    pub receiver_device_id: String,
    #[serde(default)]
    pub receiver_daemon_host_id: String,
    #[serde(default)]
    pub subject: String,
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub state: String,
    #[serde(default)]
    pub received_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ZtermStreamEvent {
    InboundMessage(ZtermInboundMessage),
    ConnectionState(serde_json::Value),
    Other(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ZtermDaemonStatus {
    pub running: bool,
    pub raw: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ZtermSlot {
    pub slot: u32,
    pub device_label: String,
    pub name: String,
    pub raw: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteManagerTarget {
    pub peer: String,
    pub manager: String,
    #[serde(default)]
    pub project: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(default)]
    pub status: String,
    pub deliverable: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub peer_counts: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ZtermDeliveryOutcome {
    Received,
    ResultUnknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ZtermDeliveryReceipt {
    pub peer: String,
    pub manager: String,
    #[serde(default)]
    pub message_id: Option<String>,
    pub outcome: ZtermDeliveryOutcome,
    pub delivered: bool,
    pub retry_allowed: bool,
    pub raw: String,
}

fn zterm_binary() -> PathBuf {
    std::env::var_os(ZTERM_BIN_ENV)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("zterm"))
}

fn run_zterm(args: &[&str]) -> Result<std::process::Output, String> {
    Command::new(zterm_binary())
        .args(args)
        .output()
        .map_err(|error| format!("ZTERM_CLI_UNAVAILABLE: {error}"))
}

pub fn daemon_status() -> Result<ZtermDaemonStatus, String> {
    let output = run_zterm(&["daemon", "status"])?;
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
    if !output.status.success() {
        return Err(format!(
            "ZTERM_DAEMON_UNAVAILABLE: {}",
            if stderr.is_empty() { stdout } else { stderr }
        ));
    }
    let running = stdout.contains("running") || stdout.contains("active_count=");
    Ok(ZtermDaemonStatus {
        running,
        raw: stdout,
    })
}

pub fn parse_slot_list(output: &str) -> Vec<ZtermSlot> {
    output
        .lines()
        .filter_map(|line| {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                return None;
            }
            let rest = trimmed
                .strip_prefix("槽位")
                .or_else(|| trimmed.strip_prefix("Slot"))?
                .trim();
            let (slot_part, detail) = rest.split_once('·')?;
            let slot = slot_part.trim().parse::<u32>().ok()?;
            let (device_label, name) = match detail.split_once('/') {
                Some((device, name)) => (device.trim().to_owned(), name.trim().to_owned()),
                None => (detail.trim().to_owned(), String::new()),
            };
            Some(ZtermSlot {
                slot,
                device_label,
                name,
                raw: trimmed.to_owned(),
            })
        })
        .collect()
}

pub fn list_slots() -> Result<Vec<ZtermSlot>, String> {
    let output = run_zterm(&["slot", "list"])?;
    let stdout = String::from_utf8_lossy(&output.stdout).to_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
    if !output.status.success() {
        return Err(format!(
            "ZTERM_SLOT_LIST_FAILED: {}",
            if stderr.is_empty() {
                stdout.trim().to_owned()
            } else {
                stderr
            }
        ));
    }
    Ok(parse_slot_list(&stdout))
}

/// Query deliverable remote managers on `peer` via `zterm app send`.
///
/// When `peer` is omitted, discovers candidate peers from `zterm slot list`
/// (using each slot's `device_label`) and aggregates their deliverable managers.
pub fn list_remote_managers(peer: Option<&str>) -> Result<Vec<RemoteManagerTarget>, String> {
    let peers: Vec<String> = match peer {
        Some(peer) => vec![peer.to_owned()],
        None => {
            let slots = list_slots()?;
            let mut unique = Vec::new();
            for slot in slots {
                if !slot.device_label.is_empty() && !unique.contains(&slot.device_label) {
                    unique.push(slot.device_label);
                }
            }
            unique
        }
    };
    let mut managers = Vec::new();
    for target_peer in peers {
        let output = run_zterm(&[
            "app",
            "send",
            "--peer",
            &target_peer,
            "--subject",
            LIST_MANAGERS_SUBJECT,
            "--body",
            "{}",
        ])?;
        if !output.status.success() {
            let code = output.status.code().unwrap_or(-1);
            let detail = String::from_utf8_lossy(&output.stderr).trim().to_owned();
            return Err(format!(
                "ZTERM_MANAGER_DISCOVERY_FAILED: peer={target_peer} exit={code} {detail}"
            ));
        }
        let stdout = String::from_utf8_lossy(&output.stdout);
        if let Ok(parsed) = serde_json::from_str::<Vec<RemoteManagerTarget>>(&stdout) {
            managers.extend(parsed.into_iter().filter(|m| m.deliverable));
        } else if let Ok(value) = serde_json::from_str::<serde_json::Value>(&stdout) {
            if let Some(items) = value.get("managers").and_then(|v| v.as_array()) {
                for item in items {
                    if let Ok(manager) = serde_json::from_value::<RemoteManagerTarget>(item.clone())
                    {
                        if manager.deliverable {
                            managers.push(manager);
                        }
                    }
                }
            }
        }
    }
    Ok(managers)
}

/// Deliver one message to a named remote manager on `peer` via `zterm app send`.
///
/// Exit `0` means `received`. Exit `3` (`resultUnknown`) is surfaced honestly
/// with `retry_allowed = false` so callers never blindly replay an uncertain send.
pub fn send_to_remote_manager(
    peer: &str,
    manager: &str,
    subject: &str,
    body: &str,
    message_id: Option<&str>,
) -> Result<ZtermDeliveryReceipt, String> {
    let envelope = serde_json::json!({
        "target_manager": manager,
        "subject": subject,
        "body": body,
    })
    .to_string();
    let mut args = vec!["app", "send", "--peer", peer];
    if let Some(message_id) = message_id {
        args.extend(["--message-id", message_id]);
    }
    args.extend(["--subject", subject, "--body", &envelope]);
    let output = run_zterm(&args)?;
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
    match output.status.code() {
        Some(0) => Ok(ZtermDeliveryReceipt {
            peer: peer.to_owned(),
            manager: manager.to_owned(),
            message_id: message_id.map(str::to_owned),
            outcome: ZtermDeliveryOutcome::Received,
            delivered: true,
            retry_allowed: false,
            raw: stdout,
        }),
        Some(3) => Ok(ZtermDeliveryReceipt {
            peer: peer.to_owned(),
            manager: manager.to_owned(),
            message_id: message_id.map(str::to_owned),
            outcome: ZtermDeliveryOutcome::ResultUnknown,
            delivered: false,
            retry_allowed: false,
            raw: if stderr.is_empty() { stdout } else { stderr },
        }),
        code => Err(format!(
            "ZTERM_DELIVERY_FAILED: peer={peer} manager={manager} exit={} {}",
            code.map_or_else(|| "signal".into(), |c| c.to_string()),
            if stderr.is_empty() { stdout } else { stderr }
        )),
    }
}

pub fn parse_stream_line(line: &str) -> Option<ZtermStreamEvent> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return None;
    }
    let frame: serde_json::Value = serde_json::from_str(trimmed).ok()?;
    let frame_type = frame.get("type")?.as_str()?;
    let payload = frame.get("payload")?.clone();
    match frame_type {
        "app-inbound-message" => serde_json::from_value::<ZtermInboundMessage>(payload)
            .ok()
            .map(ZtermStreamEvent::InboundMessage),
        "app-connection-state" => Some(ZtermStreamEvent::ConnectionState(payload)),
        other => Some(ZtermStreamEvent::Other(other.to_owned())),
    }
}

pub fn consume_message(
    message_id: &str,
    sender_daemon_host_id: Option<&str>,
) -> Result<(), String> {
    let mut args = vec!["app", "consume", "--message-id", message_id];
    if let Some(host_id) = sender_daemon_host_id.filter(|value| !value.trim().is_empty()) {
        args.extend(["--sender-daemon-host-id", host_id]);
    }
    let output = run_zterm(&args)?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_owned();
        return Err(format!(
            "ZTERM_CONSUME_FAILED: message_id={message_id} {}",
            if stderr.is_empty() { stdout } else { stderr }
        ));
    }
    Ok(())
}

pub fn send_raw(
    peer: &str,
    subject: &str,
    body: &str,
    message_id: Option<&str>,
) -> Result<ZtermDeliveryReceipt, String> {
    let mut args = vec!["app", "send", "--peer", peer];
    if let Some(message_id) = message_id {
        args.extend(["--message-id", message_id]);
    }
    args.extend(["--subject", subject, "--body", body]);
    let output = run_zterm(&args)?;
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
    match output.status.code() {
        Some(0) => Ok(ZtermDeliveryReceipt {
            peer: peer.to_owned(),
            manager: String::new(),
            message_id: message_id.map(str::to_owned),
            outcome: ZtermDeliveryOutcome::Received,
            delivered: true,
            retry_allowed: false,
            raw: stdout,
        }),
        Some(3) => Ok(ZtermDeliveryReceipt {
            peer: peer.to_owned(),
            manager: String::new(),
            message_id: message_id.map(str::to_owned),
            outcome: ZtermDeliveryOutcome::ResultUnknown,
            delivered: false,
            retry_allowed: false,
            raw: if stderr.is_empty() { stdout } else { stderr },
        }),
        code => Err(format!(
            "ZTERM_DELIVERY_FAILED: peer={peer} exit={} {}",
            code.map_or_else(|| "signal".into(), |c| c.to_string()),
            if stderr.is_empty() { stdout } else { stderr }
        )),
    }
}

pub fn list_local_deliverable_managers(state_root: &std::path::Path) -> Vec<RemoteManagerTarget> {
    let mut managers = Vec::new();
    let routes_path = state_root.join("routes.json");
    if let Ok(raw) = std::fs::read_to_string(&routes_path) {
        if let Ok(routes) = serde_json::from_str::<serde_json::Value>(&raw) {
            if let Some(projects) = routes.get("projects").and_then(|v| v.as_object()) {
                for (project_id, entry) in projects {
                    let project_name = entry
                        .get("project_root")
                        .and_then(|v| v.as_str())
                        .unwrap_or(project_id)
                        .to_owned();
                    let manager_name = entry
                        .get("master_name")
                        .and_then(|v| v.as_str())
                        .unwrap_or("Jenny")
                        .to_owned();
                    managers.push(RemoteManagerTarget {
                        peer: String::new(),
                        manager: manager_name,
                        project: Some(project_name),
                        summary: entry
                            .get("summary")
                            .and_then(|v| v.as_str())
                            .map(str::to_owned),
                        status: "registered".into(),
                        deliverable: true,
                        peer_counts: None,
                    });
                }
            }
        }
    }
    managers
}

pub fn handle_inbound_message(
    state_root: &std::path::Path,
    msg: &ZtermInboundMessage,
    seen_ids: &mut std::collections::HashSet<String>,
) -> Result<serde_json::Value, String> {
    if !seen_ids.insert(msg.message_id.clone()) {
        consume_message(&msg.message_id, Some(&msg.sender_daemon_host_id))?;
        return Ok(serde_json::json!({
            "message_id": msg.message_id,
            "action": "deduplicated",
        }));
    }

    let action = if msg.subject == LIST_MANAGERS_SUBJECT {
        let managers = list_local_deliverable_managers(state_root);
        let reply_body = serde_json::json!({ "managers": managers }).to_string();
        let receipt = send_raw(
            &msg.sender_device_id,
            LIST_MANAGERS_REPLY_SUBJECT,
            &reply_body,
            None,
        )?;
        serde_json::json!({
            "action": "list_managers_replied",
            "peer": msg.sender_device_id,
            "delivered": receipt.delivered,
            "managers_count": managers.len(),
        })
    } else if msg.subject == LIST_MANAGERS_REPLY_SUBJECT {
        let cache_path = state_root.join("zterm-remote-managers.json");
        let mut cache = std::fs::read_to_string(&cache_path)
            .ok()
            .and_then(|raw| serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(&raw).ok())
            .unwrap_or_default();
        let parsed: serde_json::Value = serde_json::from_str(&msg.body)
            .unwrap_or_else(|_| serde_json::json!({ "raw": msg.body }));
        cache.insert(msg.sender_device_id.clone(), parsed.clone());
        let _ = std::fs::create_dir_all(state_root);
        std::fs::write(
            &cache_path,
            serde_json::to_vec_pretty(&cache).unwrap_or_default(),
        )
        .map_err(|error| format!("ZTERM_CACHE_WRITE_FAILED: {error}"))?;
        serde_json::json!({
            "action": "remote_managers_cached",
            "peer": msg.sender_device_id,
            "payload": parsed,
        })
    } else {
        let inbox_name = if msg.subject == REMOTE_REPORT_SUBJECT {
            "zterm-ceo-inbox.jsonl"
        } else {
            "zterm-inbound.jsonl"
        };
        let inbox_path = state_root.join(inbox_name);
        let _ = std::fs::create_dir_all(state_root);
        let record = serde_json::json!({
            "message_id": msg.message_id,
            "sender_device_id": msg.sender_device_id,
            "sender_daemon_host_id": msg.sender_daemon_host_id,
            "subject": msg.subject,
            "body": msg.body,
            "received_at": msg.received_at,
        });
        use std::io::Write;
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&inbox_path)
            .map_err(|error| format!("ZTERM_INBOX_WRITE_FAILED: {error}"))?;
        writeln!(file, "{record}")
            .map_err(|error| format!("ZTERM_INBOX_WRITE_FAILED: {error}"))?;
        serde_json::json!({
            "action": if msg.subject == REMOTE_REPORT_SUBJECT { "ceo_report_recorded" } else { "inbound_recorded" },
            "peer": msg.sender_device_id,
            "inbox": inbox_path,
        })
    };

    // Strict semantic order: only consume after local persistence/reply succeeds.
    consume_message(&msg.message_id, Some(&msg.sender_daemon_host_id))?;
    Ok(serde_json::json!({
        "message_id": msg.message_id,
        "result": action,
    }))
}

pub fn listen_stream_loop(
    state_root: &std::path::Path,
    max_events: Option<usize>,
) -> Result<Vec<serde_json::Value>, String> {
    use std::io::BufRead;
    let mut seen_ids = std::collections::HashSet::new();
    let mut handled = Vec::new();
    let mut backoff_ms = 500u64;

    loop {
        let mut child = Command::new(zterm_binary())
            .args(["app", "stream", "--json"])
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|error| format!("ZTERM_STREAM_SPAWN_FAILED: {error}"))?;

        let Some(stdout) = child.stdout.take() else {
            let _ = child.kill();
            return Err("ZTERM_STREAM_PIPE_MISSING".into());
        };
        let reader = std::io::BufReader::new(stdout);
        let mut disconnected = false;
        for line in reader.lines() {
            let Ok(line) = line else {
                break;
            };
            match parse_stream_line(&line) {
                Some(ZtermStreamEvent::InboundMessage(msg)) => {
                    backoff_ms = 500;
                    let outcome = handle_inbound_message(state_root, &msg, &mut seen_ids)?;
                    handled.push(outcome);
                    if max_events.is_some_and(|limit| handled.len() >= limit) {
                        let _ = child.kill();
                        let _ = child.wait();
                        return Ok(handled);
                    }
                }
                Some(ZtermStreamEvent::ConnectionState(_)) => {
                    disconnected = true;
                }
                _ => {}
            }
        }
        let _ = child.kill();
        let _ = child.wait();
        if max_events.is_some_and(|limit| handled.len() >= limit) {
            return Ok(handled);
        }
        if !disconnected {
            std::thread::sleep(std::time::Duration::from_millis(backoff_ms));
            backoff_ms = (backoff_ms * 2).min(10_000);
        } else {
            std::thread::sleep(std::time::Duration::from_millis(backoff_ms));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    struct EnvGuard {
        prev: Option<std::ffi::OsString>,
    }

    impl EnvGuard {
        fn set(path: &std::path::Path) -> Self {
            let prev = std::env::var_os(ZTERM_BIN_ENV);
            unsafe {
                std::env::set_var(ZTERM_BIN_ENV, path);
            }
            Self { prev }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            unsafe {
                match &self.prev {
                    Some(val) => std::env::set_var(ZTERM_BIN_ENV, val),
                    None => std::env::remove_var(ZTERM_BIN_ENV),
                }
            }
        }
    }

    #[test]
    fn parses_real_zterm_slot_list_output() {
        let slots = parse_slot_list("槽位 1 · MacBook Air / zcamapp-0\n槽位 2 · Linux Box / appsdk-1\n");
        assert_eq!(slots.len(), 2);
        assert_eq!(slots[0].slot, 1);
        assert_eq!(slots[0].device_label, "MacBook Air");
        assert_eq!(slots[0].name, "zcamapp-0");
        assert_eq!(slots[1].slot, 2);
        assert_eq!(slots[1].device_label, "Linux Box");
        assert_eq!(slots[1].name, "appsdk-1");
    }

    #[test]
    fn mock_zterm_cli_lists_deliverable_managers_and_enforces_delivery_semantics() {
        let _lock = ENV_LOCK.lock().unwrap();
        let root = std::env::temp_dir().join(format!("zterm-adapter-test-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let script = root.join("mock-zterm");
        std::fs::write(
            &script,
            r#"#!/bin/sh
if [ "$1" = "daemon" ] && [ "$2" = "status" ]; then
  echo "zterm autostart service running: host=0.0.0.0 port=3333"
  exit 0
fi
if [ "$1" = "slot" ] && [ "$2" = "list" ]; then
  echo "槽位 1 · peer-mac / appsdk-0"
  exit 0
fi
if [ "$1" = "app" ] && [ "$2" = "send" ]; then
  if echo "$*" | grep -q "collab:list-managers"; then
    echo '{"managers":[{"peer":"peer-mac","manager":"Jenny","project":"appsdk","status":"idle","deliverable":true},{"peer":"peer-mac","manager":"Old","project":"appsdk","status":"offline","deliverable":false}]}'
    exit 0
  fi
  if echo "$*" | grep -q "unknown-case"; then
    echo "resultUnknown" >&2
    exit 3
  fi
  if echo "$*" | grep -q "fail-case"; then
    echo "peer offline" >&2
    exit 1
  fi
  echo '{"status":"received"}'
  exit 0
fi
exit 2
"#,
        )
        .unwrap();
        let mut perms = std::fs::metadata(&script).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&script, perms).unwrap();
        let _guard = EnvGuard::set(&script);

        let status = daemon_status().unwrap();
        assert!(status.running);

        let managers = list_remote_managers(None).unwrap();
        assert_eq!(managers.len(), 1);
        assert_eq!(managers[0].peer, "peer-mac");
        assert_eq!(managers[0].manager, "Jenny");
        assert!(managers[0].deliverable);

        let ok = send_to_remote_manager("peer-mac", "Jenny", "sync", "hello", Some("m-1")).unwrap();
        assert_eq!(ok.outcome, ZtermDeliveryOutcome::Received);
        assert!(ok.delivered);
        assert!(!ok.retry_allowed);

        let unk = send_to_remote_manager("peer-mac", "Jenny", "unknown-case", "hello", None).unwrap();
        assert_eq!(unk.outcome, ZtermDeliveryOutcome::ResultUnknown);
        assert!(!unk.delivered);
        assert!(!unk.retry_allowed);

        let err = send_to_remote_manager("peer-mac", "Jenny", "fail-case", "hello", None).unwrap_err();
        assert!(err.starts_with("ZTERM_DELIVERY_FAILED:"));

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn mock_zterm_stream_routes_manager_discovery_dedup_and_ceo_reports_before_consume() {
        let _lock = ENV_LOCK.lock().unwrap();
        let root = std::env::temp_dir().join(format!("zterm-stream-test-{}", std::process::id()));
        let state_root = root.join("state");
        std::fs::create_dir_all(&state_root).unwrap();
        std::fs::write(
            state_root.join("routes.json"),
            r#"{"projects":{"p1":{"project_root":"/tmp/demo","master_name":"Jenny"}}}"#,
        )
        .unwrap();
        let log_file = root.join("calls.log");
        let script = root.join("mock-zterm");
        std::fs::write(
            &script,
            format!(
                r#"#!/bin/sh
echo "$*" >> "{log}"
if [ "$1" = "app" ] && [ "$2" = "stream" ]; then
  echo '{{"type":"app-inbound-message","payload":{{"messageId":"m-list","senderDeviceId":"mac-studio","senderDaemonHostId":"host-1","subject":"collab:list-managers","body":"{{}}"}}}}'
  echo '{{"type":"app-inbound-message","payload":{{"messageId":"m-list","senderDeviceId":"mac-studio","senderDaemonHostId":"host-1","subject":"collab:list-managers","body":"{{}}"}}}}'
  echo '{{"type":"app-inbound-message","payload":{{"messageId":"m-rep","senderDeviceId":"macbook-air","senderDaemonHostId":"host-2","subject":"collab:remote-report","body":"milestone done"}}}}'
  echo '{{"type":"app-inbound-message","payload":{{"messageId":"m-cache","senderDeviceId":"macbook-air","senderDaemonHostId":"host-2","subject":"collab:list-managers-reply","body":"{{\"managers\":[{{\"peer\":\"macbook-air\",\"manager\":\"Jenny\",\"deliverable\":true}}]}}"}}}}'
  exit 0
fi
if [ "$1" = "app" ] && [ "$2" = "send" ]; then
  echo '{{"type":"app-send-result","payload":{{"status":"received"}}}}'
  exit 0
fi
if [ "$1" = "app" ] && [ "$2" = "consume" ]; then
  exit 0
fi
exit 1
"#,
                log = log_file.display()
            ),
        )
        .unwrap();
        let mut perms = std::fs::metadata(&script).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&script, perms).unwrap();
        let _guard = EnvGuard::set(&script);

        let handled = listen_stream_loop(&state_root, Some(4)).unwrap();
        assert_eq!(handled.len(), 4);
        assert_eq!(handled[0]["result"]["action"], "list_managers_replied");
        assert_eq!(handled[0]["result"]["managers_count"], 1);
        assert_eq!(handled[1]["action"], "deduplicated");
        assert_eq!(handled[2]["result"]["action"], "ceo_report_recorded");
        assert_eq!(handled[3]["result"]["action"], "remote_managers_cached");

        let ceo_inbox = std::fs::read_to_string(state_root.join("zterm-ceo-inbox.jsonl")).unwrap();
        assert!(ceo_inbox.contains("milestone done"));
        let cache = std::fs::read_to_string(state_root.join("zterm-remote-managers.json")).unwrap();
        assert!(cache.contains("Jenny"));
        let calls = std::fs::read_to_string(&log_file).unwrap();
        assert_eq!(calls.matches("app send").count(), 1);
        assert_eq!(calls.matches("app consume").count(), 4);

        let _ = std::fs::remove_dir_all(&root);
    }
}
