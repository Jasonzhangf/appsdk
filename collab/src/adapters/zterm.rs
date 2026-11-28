//! External `zterm` CLI adapter for remote slot/manager discovery and cross-machine delivery.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::process::Command;

const ZTERM_BIN_ENV: &str = "ZTERM_BIN";
const LIST_MANAGERS_SUBJECT: &str = "collab:list-managers";

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
    #[serde(default)]
    pub status: String,
    pub deliverable: bool,
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
}
