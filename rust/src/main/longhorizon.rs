use super::*;

pub(super) fn parse_duration_to_ms(s: &str) -> Result<u64, String> {
    let s = s.trim();
    if s.is_empty() {
        return Err("EMPTY_DURATION".into());
    }
    if let Ok(ms) = s.parse::<u64>() {
        return Ok(ms);
    }
    let (num_part, unit) = s.split_at(s.len() - 1);
    let num: u64 = num_part
        .parse()
        .map_err(|_| format!("INVALID_DURATION_NUMBER:{}", num_part))?;
    let multiplier = match unit {
        "s" | "S" => Ok(1_000_u64),
        "m" | "M" => Ok(60_000_u64),
        "h" | "H" => Ok(3_600_000_u64),
        "d" | "D" => Ok(86_400_000_u64),
        _ => Err(format!("UNKNOWN_DURATION_UNIT:{}", unit)),
    }?;
    num.checked_mul(multiplier)
        .ok_or_else(|| format!("GOAL_DURATION_OVERFLOW:{}", s))
}

pub(super) fn long_horizon_record(root: &Path) -> Result<Option<Value>, String> {
    let file = root.join(".appsdk-control/long-task-goal.json");
    let content = match fs::read_to_string(&file) {
        Ok(content) => content,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(format!(
                "GOAL_RECORD_READ_FAILED:{}:{}",
                file.display(),
                error
            ));
        }
    };
    serde_json::from_str(&content)
        .map(Some)
        .map_err(|error| format!("GOAL_RECORD_JSON_INVALID:{}", error))
}

pub(super) fn collab_status_all(root: &Path) -> Result<Value, String> {
    let mut command = Command::new("collab");
    command.args(["status", "--all"]).current_dir(root);
    let out = run_goal_collab_command(command, GOAL_COLLAB_READ_TIMEOUT)
        .map_err(|error| format!("COLLAB_STATUS_UNAVAILABLE:{}", error))?;
    if !out.status.success() {
        let detail = String::from_utf8_lossy(&out.stderr).trim().to_string();
        return Err(format!(
            "COLLAB_STATUS_FAILED:exit={}{}",
            out.status.code().unwrap_or(1),
            if detail.is_empty() {
                String::new()
            } else {
                format!(":{}", detail)
            }
        ));
    }
    serde_json::from_slice(&out.stdout)
        .map_err(|error| format!("COLLAB_STATUS_JSON_INVALID:{}", error))
}

pub(super) fn collab_master_status(root: &Path) -> Result<Value, String> {
    let mut command = Command::new("collab");
    command.args(["master", "status"]).current_dir(root);
    let out = run_goal_collab_command(command, GOAL_COLLAB_READ_TIMEOUT)
        .map_err(|error| format!("GOAL_OWNER_MASTER_STATUS_UNAVAILABLE:{}", error))?;
    if !out.status.success() {
        let detail = String::from_utf8_lossy(&out.stderr).trim().to_string();
        return Err(format!(
            "GOAL_OWNER_MASTER_STATUS_FAILED:exit={}{}",
            out.status.code().unwrap_or(1),
            if detail.is_empty() {
                String::new()
            } else {
                format!(":{}", detail)
            }
        ));
    }
    serde_json::from_slice(&out.stdout)
        .map_err(|error| format!("GOAL_OWNER_MASTER_STATUS_JSON_INVALID:{}", error))
}

pub(super) fn goal_record_subscription_id(record: &Value) -> Option<String> {
    record
        .get("subscription_id")
        .and_then(Value::as_str)
        .or_else(|| {
            record
                .get("collab_subscription")
                .and_then(|value| value.get("subscription_id"))
                .and_then(Value::as_str)
        })
        .filter(|id| !id.trim().is_empty())
        .map(str::to_owned)
}

pub(super) fn goal_record_read(root: &Path) -> Result<Option<Value>, String> {
    long_horizon_record(root)
}

pub(super) fn goal_record_write(root: &Path, record: &Value) -> Result<(), String> {
    let control_dir = root.join(".appsdk-control");
    fs::create_dir_all(&control_dir)
        .map_err(|error| format!("GOAL_CONTROL_DIR_CREATE_FAILED:{}", error))?;
    let file = control_dir.join("long-task-goal.json");
    let content = serde_json::to_string_pretty(record)
        .map_err(|error| format!("GOAL_RECORD_SERIALIZE_FAILED:{}", error))?
        + "\n";
    let staging = control_dir.join(format!(
        "long-task-goal.json.staging.{}.{}",
        std::process::id(),
        goal_now_ms()
    ));
    fs::write(&staging, content).map_err(|error| format!("GOAL_RECORD_WRITE_FAILED:{}", error))?;
    if let Err(error) = fs::rename(&staging, &file) {
        let _ = fs::remove_file(&staging);
        return Err(format!("GOAL_RECORD_WRITE_FAILED:{}", error));
    }
    Ok(())
}

pub(super) fn goal_mark_recovery_required(record: &mut Value, error: String) {
    record["desired"] = Value::String("recovery_required".into());
    record["observed"] = Value::String("unknown".into());
    record["remote_state"] = Value::String("unknown".into());
    record["active"] = Value::Bool(false);
    record["error"] = Value::String(error);
    record["recovery"] = Value::String(
        "Restore Collab if needed, then rerun appsdk goal subscribe --goal <path.md> to rearm a fresh one-shot deadline; no automatic renewal is attempted".into(),
    );
    record["revision"] = Value::Number((record["revision"].as_u64().unwrap_or(0) + 1).into());
}

pub(super) fn goal_subscribe_failure_allows_no_subscription_recovery(error: &str) -> bool {
    error.starts_with("COLLAB_SUBSCRIBE_FAILED:")
        || error.starts_with("COLLAB_UNAVAILABLE:")
        || matches!(
            error,
            "GOAL_COLLAB_COMMAND_TIMEOUT" | "GOAL_COLLAB_OUTPUT_DRAIN_TIMEOUT"
        )
}

#[cfg(unix)]
pub(super) fn goal_try_advisory_lock(file: &fs::File) -> Result<(), String> {
    const LOCK_EX: c_int = 2;
    const LOCK_NB: c_int = 4;
    unsafe extern "C" {
        fn flock(fd: c_int, operation: c_int) -> c_int;
    }
    let result = unsafe { flock(file.as_raw_fd(), LOCK_EX | LOCK_NB) };
    if result == 0 {
        return Ok(());
    }
    let error = std::io::Error::last_os_error();
    if error.kind() == ErrorKind::WouldBlock {
        Err("GOAL_LOCK_BUSY".into())
    } else {
        Err(format!("GOAL_LOCK_ADVISORY_FAILED:{}", error))
    }
}

#[cfg(not(unix))]
pub(super) fn goal_try_advisory_lock(_file: &fs::File) -> Result<(), String> {
    Ok(())
}

pub(super) fn goal_lock_metadata(path: &Path) -> Result<String, String> {
    let contents = fs::read_to_string(path)
        .map_err(|error| format!("GOAL_LOCK_METADATA_READ_FAILED:{}", error))?;
    let pid = contents
        .split_whitespace()
        .find_map(|field| field.strip_prefix("pid="))
        .ok_or_else(|| "GOAL_LOCK_METADATA_INVALID:pid missing".to_string())?;
    pid.parse::<u32>()
        .map_err(|error| format!("GOAL_LOCK_METADATA_INVALID:{}", error))?;
    let owner = contents
        .split_whitespace()
        .find_map(|field| field.strip_prefix("owner="))
        .filter(|owner| !owner.trim().is_empty())
        .ok_or_else(|| "GOAL_LOCK_METADATA_INVALID:owner missing".to_string())?;
    Ok(format!("pid={} owner={}", pid, owner))
}

pub(super) fn goal_lock_recovery_receipt(
    control_dir: &Path,
    quarantined: &Path,
    original: &str,
    reason: &str,
) -> Result<(), String> {
    let receipt = control_dir.join(format!(
        "long-task-goal.lock.recovery.{}.{}.json",
        std::process::id(),
        goal_now_ms()
    ));
    let payload = serde_json::json!({
        "schema_version": 1,
        "status": "recovered",
        "reason": reason,
        "original_metadata": original,
        "quarantined_path": quarantined.to_string_lossy(),
        "recovered_by_pid": std::process::id(),
        "recovered_at": chrono::Utc::now().to_rfc3339(),
        "recovery": "The advisory lock was released by its prior process; the old path was quarantined atomically before reacquisition."
    });
    fs::write(
        &receipt,
        serde_json::to_string_pretty(&payload)
            .map_err(|error| format!("GOAL_LOCK_RECOVERY_RECEIPT_SERIALIZE_FAILED:{}", error))?
            + "\n",
    )
    .map_err(|error| format!("GOAL_LOCK_RECOVERY_RECEIPT_WRITE_FAILED:{}", error))
}

pub(super) fn goal_now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(i64::MAX as u128) as i64)
        .unwrap_or(0)
}

pub(super) fn join_goal_output_reader(reader: Option<thread::JoinHandle<Vec<u8>>>) {
    if let Some(reader) = reader {
        let _ = reader.join();
    }
}

pub(super) fn detach_goal_output_readers(
    stdout_reader: Option<thread::JoinHandle<Vec<u8>>>,
    stderr_reader: Option<thread::JoinHandle<Vec<u8>>>,
) {
    // A killed command may have descendants holding inherited pipe writers;
    // join them off the timeout path after those writers close.
    let _ = thread::spawn(move || {
        join_goal_output_reader(stdout_reader);
        join_goal_output_reader(stderr_reader);
    });
}

pub(super) fn join_goal_output_reader_until(
    reader: thread::JoinHandle<Vec<u8>>,
    deadline: Instant,
) -> Result<Vec<u8>, thread::JoinHandle<Vec<u8>>> {
    let reader = reader;
    while !reader.is_finished() {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(reader);
        }
        thread::sleep(remaining.min(Duration::from_millis(10)));
    }
    Ok(reader.join().ok().unwrap_or_default())
}

pub(super) fn drain_goal_output_readers(
    stdout_reader: Option<thread::JoinHandle<Vec<u8>>>,
    stderr_reader: Option<thread::JoinHandle<Vec<u8>>>,
    deadline: Instant,
) -> Result<(Vec<u8>, Vec<u8>), ()> {
    let stdout = match stdout_reader {
        Some(reader) => match join_goal_output_reader_until(reader, deadline) {
            Ok(stdout) => stdout,
            Err(reader) => {
                detach_goal_output_readers(Some(reader), stderr_reader);
                return Err(());
            }
        },
        None => Vec::new(),
    };
    let stderr = match stderr_reader {
        Some(reader) => match join_goal_output_reader_until(reader, deadline) {
            Ok(stderr) => stderr,
            Err(reader) => {
                detach_goal_output_readers(None, Some(reader));
                return Err(());
            }
        },
        None => Vec::new(),
    };
    Ok((stdout, stderr))
}

// Collab may queue a command behind an active daemon batch. Keep every goal
// lifecycle call within one declared 120-second batch budget.
pub(super) fn run_goal_collab_command(
    mut command: Command,
    timeout: Duration,
) -> Result<Output, String> {
    let mut child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("GOAL_COLLAB_COMMAND_UNAVAILABLE:{}", error))?;
    let stdout_reader = child.stdout.take().map(|mut pipe| {
        thread::spawn(move || {
            let mut stdout = Vec::new();
            let _ = pipe.read_to_end(&mut stdout);
            stdout
        })
    });
    let stderr_reader = child.stderr.take().map(|mut pipe| {
        thread::spawn(move || {
            let mut stderr = Vec::new();
            let _ = pipe.read_to_end(&mut stderr);
            stderr
        })
    });
    let started = Instant::now();
    let deadline = started + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                return match drain_goal_output_readers(stdout_reader, stderr_reader, deadline) {
                    Ok((stdout, stderr)) => Ok(Output {
                        status,
                        stdout,
                        stderr,
                    }),
                    Err(()) => Err("GOAL_COLLAB_OUTPUT_DRAIN_TIMEOUT".into()),
                };
            }
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(10)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                detach_goal_output_readers(stdout_reader, stderr_reader);
                return Err("GOAL_COLLAB_COMMAND_TIMEOUT".into());
            }
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                detach_goal_output_readers(stdout_reader, stderr_reader);
                return Err(format!("GOAL_COLLAB_COMMAND_WAIT_FAILED:{}", error));
            }
        }
    }
}

pub(super) fn context_has_live_tmux_transport(context: &Value) -> bool {
    let transport = &context["identity"]["transport"];
    let endpoint = &transport["tmux_endpoint"];
    transport["kind"].as_str() == Some("tmux")
        && endpoint["socket_path"]
            .as_str()
            .is_some_and(|value| !value.trim().is_empty())
        && endpoint["server_pid"].as_u64().is_some_and(|pid| pid > 0)
        && endpoint["tmux_session_id"]
            .as_str()
            .is_some_and(|value| !value.trim().is_empty())
        && endpoint["pane_id"]
            .as_str()
            .is_some_and(|value| !value.trim().is_empty())
        && endpoint["pane_pid"].as_u64().is_some_and(|pid| pid > 0)
        && transport["endpoint"].as_str() == endpoint["socket_path"].as_str()
        && context["liveness"]["live"].as_bool() == Some(true)
        && context["liveness"]["presence"].as_str() == Some("present")
        && context["liveness"]["transport_kind"].as_str() == Some("tmux")
        && context["liveness"]["endpoint"].as_str() == endpoint["socket_path"].as_str()
}

pub(super) fn verified_goal_master(root: &Path) -> Result<String, String> {
    let status = collab_status_all(root)?;
    let mut context_command = Command::new("collab");
    context_command.args(["context"]).current_dir(root);
    let context_output = run_goal_collab_command(context_command, GOAL_COLLAB_READ_TIMEOUT)
        .map_err(|error| format!("GOAL_OWNER_CONTEXT_UNAVAILABLE:{}", error))?;
    if !context_output.status.success() {
        return Err(format!(
            "GOAL_OWNER_CONTEXT_FAILED:exit={}",
            context_output.status.code().unwrap_or(1)
        ));
    }
    let context: Value = serde_json::from_slice(&context_output.stdout)
        .map_err(|error| format!("GOAL_OWNER_CONTEXT_JSON_INVALID:{}", error))?;
    let owner = context["identity"]["worker_id"]
        .as_str()
        .filter(|owner| !owner.trim().is_empty())
        .ok_or_else(|| "GOAL_OWNER_IDENTITY_MISSING".to_string())?;

    let worker = status["workers"]
        .as_array()
        .and_then(|workers| {
            workers
                .iter()
                .find(|worker| worker["id"].as_str() == Some(owner))
        })
        .ok_or_else(|| "GOAL_OWNER_WORKER_MISSING".to_string())?;
    if worker["endpoint_live"].as_bool() != Some(true) {
        return Err("GOAL_OWNER_NOT_LIVE:verified Collab owner endpoint is not live".into());
    }
    if worker["identity_valid"].as_bool() != Some(true) {
        return Err("GOAL_OWNER_IDENTITY_INVALID:verified Collab owner identity is invalid".into());
    }

    let master_status = collab_master_status(root)?;
    let master = master_status
        .get("master")
        .filter(|value| value.is_object())
        .ok_or_else(|| "GOAL_OWNER_MASTER_IDENTITY_MISSING".to_string())?;
    let master_owner = master["worker_id"]
        .as_str()
        .filter(|worker_id| !worker_id.trim().is_empty())
        .ok_or_else(|| "GOAL_OWNER_MASTER_IDENTITY_MISSING".to_string())?;
    if master["endpoint_live"].as_bool() != Some(true) {
        return Err("GOAL_OWNER_MASTER_NOT_LIVE".into());
    }
    if master_owner != owner {
        return Err(format!(
            "GOAL_OWNER_IDENTITY_MISMATCH:context={} master={}",
            owner, master_owner
        ));
    }
    if !context_has_live_tmux_transport(&context) {
        return Err("GOAL_OWNER_CONTEXT_TRANSPORT_NOT_LIVE".into());
    }
    match worker["suspected_offline"].as_bool() {
        Some(false) => {}
        Some(true) if master_owner == owner && master["endpoint_live"].as_bool() == Some(true) => {}
        _ => {
            return Err(
                "GOAL_OWNER_SUSPECTED_OFFLINE:verified Collab owner is suspected offline".into(),
            );
        }
    }
    Ok(owner.to_string())
}

pub(super) fn parse_goal_subscription_response(stdout: &[u8]) -> Result<(Value, String), String> {
    let response: Value = serde_json::from_slice(stdout)
        .map_err(|error| format!("COLLAB_SUBSCRIBE_RESPONSE_INVALID:{}", error))?;
    let subscription_id = {
        let subscription = response.get("subscription").unwrap_or(&response);
        subscription
            .get("subscription_id")
            .and_then(Value::as_str)
            .or_else(|| subscription.get("id").and_then(Value::as_str))
            .filter(|id| !id.trim().is_empty())
            .ok_or_else(|| "COLLAB_SUBSCRIBE_RESPONSE_MISSING_ID".to_string())?
            .to_string()
    };
    Ok((response, subscription_id))
}

pub(super) fn parse_goal_cancel_response(
    stdout: &[u8],
    expected_id: &str,
) -> Result<Value, String> {
    let response: Value = serde_json::from_slice(stdout)
        .map_err(|error| format!("GOAL_CANCEL_RESPONSE_INVALID:{}", error))?;
    let response_id = response
        .get("subscription_id")
        .and_then(Value::as_str)
        .or_else(|| {
            response
                .get("subscription")
                .and_then(|subscription| subscription.get("subscription_id"))
                .and_then(Value::as_str)
        })
        .or_else(|| {
            response
                .get("subscription")
                .and_then(|subscription| subscription.get("id"))
                .and_then(Value::as_str)
        });
    if response_id != Some(expected_id) {
        return Err("GOAL_CANCEL_RESPONSE_ID_MISMATCH".to_string());
    }
    if response.get("status").and_then(Value::as_str) != Some("cancelled") {
        return Err("GOAL_CANCEL_RESPONSE_STATUS_INVALID".to_string());
    }
    Ok(response)
}

pub(super) fn goal_cancel_subscription(
    root: &Path,
    subscription_id: &str,
) -> Result<Value, String> {
    let mut command = Command::new("collab");
    command
        .args(["notify", "unsubscribe", subscription_id])
        .current_dir(root);
    let out = run_goal_collab_command(command, GOAL_COLLAB_WRITE_TIMEOUT).map_err(|error| {
        if error == "GOAL_COLLAB_COMMAND_TIMEOUT" {
            error
        } else {
            format!("GOAL_CANCEL_COLLAB_FAILED:{}", error)
        }
    })?;
    if !out.status.success() {
        return Err(format!(
            "GOAL_CANCEL_COLLAB_FAILED:exit={}{}",
            out.status.code().unwrap_or(1),
            if out.stderr.is_empty() {
                String::new()
            } else {
                format!(":{}", String::from_utf8_lossy(&out.stderr).trim())
            }
        ));
    }
    parse_goal_cancel_response(&out.stdout, subscription_id)
}

pub(super) fn goal_subscription_status(
    root: &Path,
    subscription_id: &str,
) -> Result<(String, Value), String> {
    let mut command = Command::new("collab");
    command.args(["notify", "status"]).current_dir(root);
    let out = run_goal_collab_command(command, GOAL_COLLAB_READ_TIMEOUT)
        .map_err(|error| format!("GOAL_STATUS_COLLAB_UNAVAILABLE:{}", error))?;
    if !out.status.success() {
        let detail = String::from_utf8_lossy(&out.stderr).trim().to_string();
        return Err(format!(
            "GOAL_STATUS_COLLAB_FAILED:exit={}{}",
            out.status.code().unwrap_or(1),
            if detail.is_empty() {
                String::new()
            } else {
                format!(":{}", detail)
            }
        ));
    }
    let response: Value = serde_json::from_slice(&out.stdout)
        .map_err(|error| format!("GOAL_STATUS_RESPONSE_INVALID:{}", error))?;
    let subscriptions = response
        .get("subscriptions")
        .and_then(Value::as_array)
        .ok_or_else(|| "GOAL_STATUS_RESPONSE_MISSING_SUBSCRIPTIONS".to_string())?;
    let subscription = subscriptions.iter().find(|subscription| {
        subscription
            .get("id")
            .and_then(Value::as_str)
            .or_else(|| subscription.get("subscription_id").and_then(Value::as_str))
            == Some(subscription_id)
    });
    let Some(subscription) = subscription else {
        return Err(format!(
            "GOAL_STATUS_SUBSCRIPTION_NOT_FOUND:{}",
            subscription_id
        ));
    };
    let remote_status = subscription
        .get("status")
        .and_then(Value::as_str)
        .filter(|status| !status.trim().is_empty())
        .ok_or_else(|| "GOAL_STATUS_SUBSCRIPTION_STATUS_MISSING".to_string())?;
    Ok((remote_status.to_string(), subscription.clone()))
}

pub(super) fn goal_subscription_by_subject(
    root: &Path,
    subject: &str,
) -> Result<Option<(String, String, Value)>, String> {
    let mut command = Command::new("collab");
    command.args(["notify", "status"]).current_dir(root);
    let out = run_goal_collab_command(command, GOAL_COLLAB_READ_TIMEOUT)
        .map_err(|error| format!("GOAL_RECONCILE_COLLAB_UNAVAILABLE:{}", error))?;
    if !out.status.success() {
        return Err(format!(
            "GOAL_RECONCILE_COLLAB_FAILED:exit={}",
            out.status.code().unwrap_or(1)
        ));
    }
    let response: Value = serde_json::from_slice(&out.stdout)
        .map_err(|error| format!("GOAL_RECONCILE_RESPONSE_INVALID:{}", error))?;
    let subscriptions = response
        .get("subscriptions")
        .and_then(Value::as_array)
        .ok_or_else(|| "GOAL_RECONCILE_RESPONSE_MISSING_SUBSCRIPTIONS".to_string())?;
    let matches: Vec<(String, String, Value)> = subscriptions
        .iter()
        .filter_map(|subscription| {
            let id = subscription
                .get("id")
                .and_then(Value::as_str)
                .or_else(|| subscription.get("subscription_id").and_then(Value::as_str))
                .filter(|id| !id.trim().is_empty())?;
            let status = subscription.get("status").and_then(Value::as_str)?;
            let event = subscription.get("event").and_then(Value::as_str)?;
            let remote_subject = subscription.get("subject").and_then(Value::as_str)?;
            (event == "deadline" && remote_subject == subject && status == "armed")
                .then(|| (id.to_string(), status.to_string(), subscription.clone()))
        })
        .collect();
    if matches.len() > 1 {
        return Err(format!(
            "GOAL_RECONCILE_SUBJECT_AMBIGUOUS:{} armed deadline subscriptions match",
            matches.len()
        ));
    }
    Ok(matches.into_iter().next())
}

pub(super) fn goal_subject_candidates(
    record: Option<&Value>,
    canonical_subject: &str,
) -> Vec<String> {
    let mut candidates = Vec::new();
    let mut push_unique = |subject: String| {
        if !subject.trim().is_empty() && !candidates.iter().any(|item| item == &subject) {
            candidates.push(subject);
        }
    };
    if let Some(record) = record {
        if let Some(subject) = record["subject"]
            .as_str()
            .filter(|subject| !subject.trim().is_empty())
        {
            // The retained subject is the compatibility anchor. Query it first
            // so a legacy basename subscription is never silently abandoned.
            push_unique(subject.to_string());
        }
        if let Some(goal_path) = record["goal_path"].as_str() {
            if let Some(basename) = Path::new(goal_path)
                .file_name()
                .and_then(|name| name.to_str())
            {
                push_unique(format!("goal:{}", basename));
            }
        }
    }
    push_unique(canonical_subject.to_string());
    candidates
}

pub(super) fn goal_subscription_by_subject_candidates(
    root: &Path,
    record: Option<&Value>,
    canonical_subject: &str,
) -> Result<Option<(String, String, Value, String)>, String> {
    let mut matches: Vec<(String, String, Value, String)> = Vec::new();
    for subject in goal_subject_candidates(record, canonical_subject) {
        if let Some((id, status, remote_record)) = goal_subscription_by_subject(root, &subject)? {
            if !matches
                .iter()
                .any(|(matched_id, _, _, _)| matched_id == &id)
            {
                matches.push((id, status, remote_record, subject));
            }
        }
    }
    if matches.len() > 1 {
        return Err(format!(
            "GOAL_RECONCILE_SUBJECT_AMBIGUOUS:{} distinct armed deadline subscriptions match retained or canonical subjects",
            matches.len()
        ));
    }
    Ok(matches.into_iter().next())
}

pub(super) fn goal_subscription_is_terminal(status: &str) -> bool {
    matches!(status, "consumed" | "expired" | "cancelled")
}

// Collab leaves a skipped goal deadline `armed` with its one-shot trigger in
// the past, so the retained remote record alone cannot prove the subscription
// is still ahead. Every `deadline-*` status reason is written on the due path
// (busy-skipped or readiness-unavailable), which makes it due evidence too.
pub(super) fn goal_deadline_trigger_is_due(remote_record: &Value, now_ms: i64) -> bool {
    if remote_record["status"].as_str() != Some("armed") {
        return false;
    }
    if remote_record["status_reason"]
        .as_str()
        .is_some_and(|reason| reason.starts_with("deadline-"))
    {
        return true;
    }
    let fired_count = remote_record["fired_count"].as_u64().unwrap_or(0);
    let next_trigger = remote_record["trigger_times_ms"]
        .as_array()
        .and_then(|times| {
            usize::try_from(fired_count)
                .ok()
                .and_then(|index| times.get(index))
        })
        .and_then(Value::as_i64)
        .or_else(|| remote_record["trigger_ms"].as_i64());
    // A goal deadline always carries its trigger, so a record without one is
    // not evidence that the retained deadline is already due.
    next_trigger.is_some_and(|trigger| trigger <= now_ms)
}

pub(super) fn goal_fail(format_json: bool, error: &str, record: Option<&Value>) -> ! {
    eprintln!("{}", error);
    if format_json {
        let mut payload = record.cloned().unwrap_or_else(|| {
            serde_json::json!({
                "active": false,
                "desired": "unknown",
                "observed": "unknown"
            })
        });
        payload["ok"] = Value::Bool(false);
        payload["error"] = Value::String(error.to_string());
        payload["recovery"] = Value::String(
            "inspect the retained goal record and retry after restoring Collab or local storage"
                .into(),
        );
        println!("{}", serde_json::to_string_pretty(&payload).unwrap());
    }
    std::process::exit(1);
}

pub(super) fn open_bugs_json(root: &Path) -> Result<Vec<Value>, String> {
    let git_bug = match locate_git_bug_binary() {
        Ok(path) => path,
        Err(err) => return Err(err),
    };
    let read = |dir: &Path| {
        Command::new(&git_bug)
            .args(["bug", "--status", "open", "-f", "json"])
            .current_dir(dir)
            .output()
    };
    let out = run_git_bug_read(|| read(root), true)
        .map_err(|err| format!("GIT_BUG_EXECUTION_FAILED:{}", err))?;

    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        return Err(if err.is_empty() {
            format!(
                "GIT_BUG_OPEN_READ_FAILED: exit={}",
                out.status.code().unwrap_or(-1)
            )
        } else {
            format!("GIT_BUG_OPEN_READ_FAILED:{}", err)
        });
    }
    let mut bugs: Vec<Value> = serde_json::from_slice(&out.stdout)
        .map_err(|e| format!("GIT_BUG_OPEN_JSON_INVALID:{}", e))?;
    let rank = |bug: &Value| -> u8 {
        let labels = bug["labels"].as_array().cloned().unwrap_or_default();
        for (priority, score) in [
            ("P0", 0u8),
            ("p0", 0),
            ("P1", 1),
            ("p1", 1),
            ("P2", 2),
            ("p2", 2),
        ] {
            if labels.iter().any(|l| l.as_str() == Some(priority)) {
                return score;
            }
        }
        3
    };
    bugs.sort_by_key(rank);
    Ok(bugs)
}

/// Pull the first meaningful prose out of the goal document so one read shows
/// what the project is for, without shipping the whole file into a wake.
pub(super) fn goal_objective_excerpt(
    goal_path: &Path,
    max_lines: usize,
    max_chars: usize,
) -> String {
    let content = match fs::read_to_string(goal_path) {
        Ok(text) => text,
        Err(err) => return format!("(无法读取目标文档: {})", err),
    };

    let mut lines = content.lines().peekable();
    if lines.peek() == Some(&"---") {
        lines.next();
        for line in lines.by_ref() {
            if line.trim() == "---" {
                break;
            }
        }
    }

    let mut picked: Vec<&str> = Vec::new();
    let mut in_fence = false;
    for line in lines {
        let trimmed = line.trim();
        if trimmed.starts_with("```") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence || trimmed.is_empty() {
            continue;
        }
        picked.push(trimmed);
        if picked.len() >= max_lines {
            break;
        }
    }

    if picked.is_empty() {
        return "(目标文档为空)".to_string();
    }

    let mut excerpt = picked.join("\n");
    if excerpt.chars().count() > max_chars {
        excerpt = excerpt.chars().take(max_chars).collect::<String>() + " …";
    }
    excerpt
}
