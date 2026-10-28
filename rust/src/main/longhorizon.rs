use super::*;
use std::io::{Read, Seek, SeekFrom};

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

pub(super) struct GoalScope {
    pub owner: String,
    pub worker_id: String,
    pub project: PathBuf,
    pub invocation_root: PathBuf,
    pub session_id: Option<String>,
    pub role: ExecutionRole,
}

impl GoalScope {
    pub fn control_dir(&self) -> PathBuf {
        // Reversible encoding keeps names (including slashes and Unicode) in
        // one path component without adding another identity registry.
        let encoded: String = self
            .owner
            .as_bytes()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        self.project.join(".appsdk-control/goals").join(encoded)
    }
}

fn read_goal_file(file: &Path) -> Result<Option<Value>, String> {
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

pub(super) fn goal_record_read(scope: &GoalScope) -> Result<Option<Value>, String> {
    if let Some(record) = read_goal_file(&scope.control_dir().join("long-task-goal.json"))? {
        if record["owner"].as_str() != Some(scope.owner.as_str()) {
            return Err("GOAL_RECORD_OWNER_MISMATCH".into());
        }
        return Ok(Some(record));
    }
    // Legacy data is optional compatibility input, not the current owner's
    // namespace. Preserve and report unusable legacy data without blocking a
    // separately authenticated owner's explicitly requested goal.
    for root in [&scope.invocation_root, &scope.project] {
        let mut record = match read_goal_file(&root.join(".appsdk-control/long-task-goal.json")) {
            Ok(Some(record)) => record,
            Ok(None) => continue,
            Err(error) => {
                eprintln!("GOAL_LEGACY_RECORD_NOT_IMPORTED:{error}");
                continue;
            }
        };
        let Some(owner) = record["owner"]
            .as_str()
            .filter(|owner| !owner.trim().is_empty())
        else {
            eprintln!("GOAL_LEGACY_RECORD_NOT_IMPORTED: legacy record has no owner");
            continue;
        };
        if owner != scope.owner && owner != scope.worker_id {
            continue;
        }
        record["owner"] = Value::String(scope.owner.clone());
        record["record_source"] = Value::String("legacy-read-only".into());
        return Ok(Some(record));
    }
    Ok(None)
}

pub(super) fn goal_record_write(scope: &GoalScope, record: &Value) -> Result<(), String> {
    if record["owner"].as_str() != Some(scope.owner.as_str()) {
        return Err("GOAL_RECORD_OWNER_MISMATCH".into());
    }
    let mut record = record.clone();
    record["record_source"] = Value::String("owner-scoped".into());
    record["project_scope"] = serde_json::json!(scope.project);
    record["session_id"] = serde_json::json!(scope.session_id);
    let control_dir = scope.control_dir();
    fs::create_dir_all(&control_dir)
        .map_err(|error| format!("GOAL_CONTROL_DIR_CREATE_FAILED:{}", error))?;
    let file = control_dir.join("long-task-goal.json");
    let content = serde_json::to_string_pretty(&record)
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
        || error.starts_with("GOAL_RECONCILE_ACTIVE_FAILED:")
        || matches!(
            error,
            "GOAL_COLLAB_COMMAND_TIMEOUT" | "GOAL_COLLAB_OUTPUT_DRAIN_TIMEOUT"
        )
}

pub(super) fn goal_try_advisory_lock(file: &fs::File) -> Result<(), String> {
    match platform::try_lock_exclusive(file) {
        Ok(()) => Ok(()),
        Err(platform::LockAttemptError::WouldBlock) => Err("GOAL_LOCK_BUSY".into()),
        Err(platform::LockAttemptError::Io(error)) => {
            Err(format!("GOAL_LOCK_ADVISORY_FAILED:{error}"))
        }
    }
}

pub(super) fn goal_lock_metadata_from_file(file: &mut fs::File) -> Result<String, String> {
    file.seek(SeekFrom::Start(0))
        .map_err(|error| format!("GOAL_LOCK_METADATA_READ_FAILED:{}", error))?;
    let mut contents = String::new();
    file.read_to_string(&mut contents)
        .map_err(|error| format!("GOAL_LOCK_METADATA_READ_FAILED:{}", error))?;
    Ok(contents)
}

pub(super) fn goal_lock_metadata(contents: &str) -> Result<String, String> {
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

pub(super) fn goal_lock_recovery_reason(original: &str) -> &'static str {
    if original.trim().is_empty() {
        "empty metadata"
    } else if goal_lock_metadata(original).is_ok() {
        "advisory lock released with stale metadata"
    } else {
        "invalid or truncated metadata"
    }
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

pub(super) fn resolve_goal_scope(root: &Path) -> Result<GoalScope, String> {
    goal_scope_from_context(&goal_context(root)?, root)
}

pub(super) fn goal_context(root: &Path) -> Result<Value, String> {
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
    Ok(crate::long_horizon_role::normalized_collab_context(
        &context,
    ))
}

pub(super) fn goal_scope_from_context(context: &Value, root: &Path) -> Result<GoalScope, String> {
    let worker_id = context["identity"]["worker_id"]
        .as_str()
        .filter(|owner| !owner.trim().is_empty())
        .ok_or_else(|| "GOAL_OWNER_IDENTITY_MISSING".to_string())?;
    let owner = context["name"]
        .as_str()
        .filter(|name| !name.trim().is_empty())
        .ok_or("GOAL_OWNER_NAME_MISSING")?;

    if context["identity"]["role"].as_str() == Some("managed-subagent")
        || context["identity"]["kind"].as_str() == Some("managed-subagent")
        || context["role_brief"]["derivation"]["kind"].as_str() == Some("managed-subagent")
    {
        return Err(
            "GOAL_OWNER_MANAGED_SUBAGENT: independent goals require an ordinary peer".into(),
        );
    }
    let project = context["binding"]["project_scope"]
        .as_str()
        .or_else(|| context["project_scope"].as_str())
        .filter(|p| !p.is_empty())
        .ok_or("GOAL_OWNER_PROJECT_SCOPE_MISSING")?;
    let project = PathBuf::from(project);
    if !project.is_absolute() {
        return Err("GOAL_OWNER_PROJECT_SCOPE_INVALID".into());
    }
    Ok(GoalScope {
        owner: owner.to_owned(),
        worker_id: worker_id.to_owned(),
        project,
        invocation_root: root.to_path_buf(),
        session_id: context["session_id"]
            .as_str()
            .or_else(|| context["binding"]["session_id"].as_str())
            .map(str::to_owned),
        role: execution_role(context),
    })
}

fn parse_goal_reconcile_response(
    stdout: &[u8],
    expected_goal_id: &str,
    expected_desired: &str,
) -> Result<Value, String> {
    let response: Value = serde_json::from_slice(stdout)
        .map_err(|error| format!("GOAL_RECONCILE_RESPONSE_INVALID:{}", error))?;
    if response.get("goal_id").and_then(Value::as_str) != Some(expected_goal_id) {
        return Err("GOAL_RECONCILE_RESPONSE_ID_MISMATCH".to_string());
    }
    if response.get("desired").and_then(Value::as_str) != Some(expected_desired) {
        return Err("GOAL_RECONCILE_RESPONSE_DESIRED_INVALID".to_string());
    }
    Ok(response)
}

pub(super) fn goal_reconcile_active(
    root: &Path,
    goal_id: &str,
    goal_owner: &str,
    trigger_ms: i64,
    ttl_seconds: u64,
) -> Result<(Value, String), String> {
    let mut command = Command::new("collab");
    command
        .args([
            "goal",
            "reconcile",
            "--goal-id",
            goal_id,
            "--owner",
            goal_owner,
            "--desired",
            "active",
            "--at-ms",
            &trigger_ms.to_string(),
            "--ttl-seconds",
            &ttl_seconds.to_string(),
        ])
        .current_dir(root);
    let out = run_goal_collab_command(command, GOAL_COLLAB_WRITE_TIMEOUT).map_err(|error| {
        if error == "GOAL_COLLAB_COMMAND_TIMEOUT" {
            error
        } else {
            format!("GOAL_RECONCILE_COLLAB_FAILED:{}", error)
        }
    })?;
    if !out.status.success() {
        return Err(format!(
            "GOAL_RECONCILE_COLLAB_FAILED:exit={}{}",
            out.status.code().unwrap_or(1),
            if out.stderr.is_empty() {
                String::new()
            } else {
                format!(":{}", String::from_utf8_lossy(&out.stderr).trim())
            }
        ));
    }
    let response = parse_goal_reconcile_response(&out.stdout, goal_id, "active")?;
    if response.get("status").and_then(Value::as_str) != Some("armed") {
        return Err("GOAL_RECONCILE_ACTIVE_NOT_ARMED".to_string());
    }
    let subscription_id = response
        .get("subscription")
        .and_then(|subscription| {
            subscription
                .get("subscription_id")
                .and_then(Value::as_str)
                .or_else(|| subscription.get("id").and_then(Value::as_str))
        })
        .filter(|id| !id.trim().is_empty())
        .ok_or_else(|| "GOAL_RECONCILE_RESPONSE_MISSING_ID".to_string())?
        .to_string();
    Ok((response, subscription_id))
}

pub(super) fn goal_reconcile_ended(
    root: &Path,
    goal_id: &str,
    goal_owner: &str,
    goal_subject: Option<&str>,
    reason: Option<&str>,
) -> Result<Value, String> {
    let mut command = Command::new("collab");
    command.args([
        "goal",
        "reconcile",
        "--goal-id",
        goal_id,
        "--owner",
        goal_owner,
        "--desired",
        "ended",
    ]);
    if let Some(goal_subject) = goal_subject {
        command.args(["--goal-subject", goal_subject]);
    }
    if let Some(reason) = reason {
        command.args(["--reason", reason]);
    }
    command.current_dir(root);
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
    parse_goal_reconcile_response(&out.stdout, goal_id, "ended")
}

pub(super) fn goal_subscription_status(
    root: &Path,
    scope: &GoalScope,
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
    goal_check_subscription_owner(scope, subscription)?;
    let remote_status = subscription
        .get("status")
        .and_then(Value::as_str)
        .filter(|status| !status.trim().is_empty())
        .ok_or_else(|| "GOAL_STATUS_SUBSCRIPTION_STATUS_MISSING".to_string())?;
    Ok((remote_status.to_string(), subscription.clone()))
}

pub(super) fn goal_subscription_by_subject(
    root: &Path,
    scope: &GoalScope,
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
        .filter(|subscription| goal_check_subscription_owner(scope, subscription).is_ok())
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
    scope: &GoalScope,
    record: Option<&Value>,
    canonical_subject: &str,
) -> Result<Option<(String, String, Value, String)>, String> {
    let mut matches: Vec<(String, String, Value, String)> = Vec::new();
    for subject in goal_subject_candidates(record, canonical_subject) {
        if let Some((id, status, remote_record)) =
            goal_subscription_by_subject(root, scope, &subject)?
        {
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

pub(super) fn goal_check_subscription_owner(
    scope: &GoalScope,
    subscription: &Value,
) -> Result<(), String> {
    if subscription["worker_id"]
        .as_str()
        .is_some_and(|owner| owner != scope.worker_id)
        || subscription["owner"]
            .as_str()
            .is_some_and(|owner| owner != scope.owner)
    {
        return Err("GOAL_SUBSCRIPTION_OWNER_MISMATCH".into());
    }
    Ok(())
}

pub(super) const PROJECT_PATROL_EVENT: &str = "master-idle";
pub(super) const PROJECT_PATROL_SUBJECT: &str = "project-patrol";
pub(super) const PROJECT_PATROL_EVERY_MS: i64 = 1_500_000;
pub(super) const PROJECT_PATROL_COMPAT_EVERY_MS: i64 = 900_000;
pub(super) const PROJECT_PATROL_EVERY_MS_STR: &str = "1500000";

fn is_project_patrol_interval(interval_ms: Option<i64>) -> bool {
    matches!(
        interval_ms,
        Some(PROJECT_PATROL_EVERY_MS | PROJECT_PATROL_COMPAT_EVERY_MS)
    )
}

pub(super) fn project_patrol_cadence_gap(interval_ms: Option<i64>) -> Option<&'static str> {
    (interval_ms == Some(PROJECT_PATROL_COMPAT_EVERY_MS))
        .then_some("PATROL_CADENCE_RUNTIME_GAP")
}

fn goal_notify_subscriptions(root: &Path) -> Result<Vec<Value>, String> {
    let mut command = Command::new("collab");
    command.args(["notify", "status"]).current_dir(root);
    let out = run_goal_collab_command(command, GOAL_COLLAB_READ_TIMEOUT)
        .map_err(|error| format!("GOAL_PATROL_STATUS_UNAVAILABLE:{}", error))?;
    if !out.status.success() {
        let detail = String::from_utf8_lossy(&out.stderr).trim().to_string();
        return Err(format!(
            "GOAL_PATROL_STATUS_FAILED:exit={}{}",
            out.status.code().unwrap_or(1),
            if detail.is_empty() {
                String::new()
            } else {
                format!(":{}", detail)
            }
        ));
    }
    let response: Value = serde_json::from_slice(&out.stdout)
        .map_err(|error| format!("GOAL_PATROL_STATUS_INVALID:{}", error))?;
    response
        .get("subscriptions")
        .and_then(Value::as_array)
        .cloned()
        .ok_or_else(|| "GOAL_PATROL_STATUS_MISSING_SUBSCRIPTIONS".to_string())
}

pub(super) fn goal_subscription_is_terminal(status: &str) -> bool {
    matches!(status, "consumed" | "expired" | "cancelled")
}

fn goal_patrol_subscription_is_usable(
    scope: &GoalScope,
    subscription: &Value,
    now_ms: i64,
) -> Option<String> {
    if goal_check_subscription_owner(scope, subscription).is_err()
        || subscription["event"].as_str() != Some(PROJECT_PATROL_EVENT)
        || !is_project_patrol_interval(subscription["interval_ms"].as_i64())
        || subscription["status"].as_str() != Some("armed")
        || subscription["subject"].as_str() != Some(PROJECT_PATROL_SUBJECT)
    {
        return None;
    }
    if subscription["expires_ms"]
        .as_i64()
        .is_some_and(|expires_ms| expires_ms <= now_ms)
    {
        return None;
    }
    if let (Some(fired), Some(repeat)) = (
        subscription["fired_count"].as_u64(),
        subscription["repeat_count"].as_u64(),
    ) {
        if fired >= repeat {
            return None;
        }
    }
    subscription["id"]
        .as_str()
        .or_else(|| subscription["subscription_id"].as_str())
        .filter(|id| !id.trim().is_empty())
        .map(str::to_owned)
}

fn goal_project_patrol_record(
    root: &Path,
    scope: &GoalScope,
) -> Result<Option<(String, Value)>, String> {
    let now_ms = goal_now_ms();
    let mut matches: Vec<(String, Value)> = goal_notify_subscriptions(root)?
        .into_iter()
        .filter_map(|subscription| {
            if goal_check_subscription_owner(scope, &subscription).is_err()
                || subscription["event"].as_str() != Some(PROJECT_PATROL_EVENT)
                || !is_project_patrol_interval(subscription["interval_ms"].as_i64())
                || subscription["subject"].as_str() != Some(PROJECT_PATROL_SUBJECT)
            {
                return None;
            }
            let current = subscription["status"].as_str() == Some("cancelled")
                || (subscription["status"].as_str() == Some("armed")
                    && subscription["expires_ms"]
                        .as_i64()
                        .is_none_or(|expires_ms| expires_ms > now_ms));
            if !current {
                return None;
            }
            subscription["id"]
                .as_str()
                .filter(|id| !id.trim().is_empty())
                .map(|id| (id.to_owned(), subscription.clone()))
        })
        .collect();
    matches.sort_by_key(|(_, subscription)| {
        (
            subscription["created_ms"].as_i64().unwrap_or(i64::MAX),
            subscription["id"].as_str().unwrap_or_default().to_string(),
        )
    });
    Ok(matches.into_iter().next())
}

pub(super) fn goal_project_patrol_subscription(
    root: &Path,
    scope: &GoalScope,
) -> Result<Option<(String, Value)>, String> {
    let now_ms = goal_now_ms();
    Ok(
        goal_project_patrol_record(root, scope)?.filter(|(_, subscription)| {
            goal_patrol_subscription_is_usable(scope, subscription, now_ms).is_some()
        }),
    )
}

fn goal_project_patrol_subscribe(
    root: &Path,
    scope: &GoalScope,
) -> Result<(String, Value), String> {
    let mut command = Command::new("collab");
    command
        .args([
            "notify",
            "subscribe",
            "--event",
            PROJECT_PATROL_EVENT,
            "--subject",
            PROJECT_PATROL_SUBJECT,
            "--every-ms",
            PROJECT_PATROL_EVERY_MS_STR,
            "--repeat-count",
            "100",
            "--ttl-seconds",
            "604800",
        ])
        .current_dir(root);
    let out = run_goal_collab_command(command, GOAL_COLLAB_WRITE_TIMEOUT)
        .map_err(|error| format!("GOAL_PATROL_SUBSCRIBE_FAILED:{}", error))?;
    if !out.status.success() {
        let detail = String::from_utf8_lossy(&out.stderr).trim().to_string();
        return Err(format!(
            "GOAL_PATROL_SUBSCRIBE_FAILED:exit={}{}",
            out.status.code().unwrap_or(1),
            if detail.is_empty() {
                String::new()
            } else {
                format!(":{}", detail)
            }
        ));
    }
    let response: Value = serde_json::from_slice(&out.stdout)
        .map_err(|error| format!("GOAL_PATROL_SUBSCRIBE_INVALID:{}", error))?;
    let subscription = response
        .get("subscription")
        .ok_or_else(|| "GOAL_PATROL_SUBSCRIBE_MISSING_SUBSCRIPTION".to_string())?;
    let subscription_id = goal_patrol_subscription_is_usable(scope, subscription, goal_now_ms())
        .ok_or_else(|| "GOAL_PATROL_SUBSCRIBE_NOT_ARMED".to_string())?;
    Ok((subscription_id, subscription.clone()))
}

pub(super) fn goal_ensure_project_patrol(root: &Path, scope: &GoalScope) -> Value {
    if scope.role != ExecutionRole::Master {
        return serde_json::json!({
            "applicable": false,
            "armed": false,
            "subscription_id": Value::Null,
            "every_ms": Value::Null,
            "source": "not-applicable",
            "error": Value::Null,
        });
    }
    let now = chrono::Utc::now().to_rfc3339();
    match goal_project_patrol_record(root, scope) {
        Ok(Some((subscription_id, subscription)))
            if subscription["status"].as_str() == Some("cancelled") =>
        {
            serde_json::json!({
                "applicable": true,
                "armed": false,
                "subscription_id": subscription_id,
                "every_ms": PROJECT_PATROL_EVERY_MS,
                "source": "cancelled",
                "status": "cancelled",
                "checked_at": now,
                "subscription": subscription,
                "error": Value::Null,
            })
        }
        Ok(Some((subscription_id, subscription)))
            if goal_patrol_subscription_is_usable(scope, &subscription, goal_now_ms())
                .is_some() =>
        {
            serde_json::json!({
                "applicable": true,
                "armed": true,
                "subscription_id": subscription_id,
                "every_ms": PROJECT_PATROL_EVERY_MS,
                "source": "reused",
                "status": "armed",
                "checked_at": now,
                "subscription": subscription,
                "error": Value::Null,
            })
        }
        Ok(None) => match goal_project_patrol_subscribe(root, scope) {
            Ok((subscription_id, subscription)) => serde_json::json!({
                "applicable": true,
                "armed": true,
                "subscription_id": subscription_id,
                "every_ms": PROJECT_PATROL_EVERY_MS,
                "source": "armed",
                "checked_at": now,
                "subscription": subscription,
                "error": Value::Null,
            }),
            Err(error) => serde_json::json!({
                "applicable": true,
                "armed": false,
                "subscription_id": Value::Null,
                "every_ms": PROJECT_PATROL_EVERY_MS,
                "source": "failed",
                "checked_at": now,
                "subscription": Value::Null,
                "error": error,
            }),
        },
        Ok(Some((_subscription_id, _subscription))) => {
            match goal_project_patrol_subscribe(root, scope) {
                Ok((subscription_id, subscription)) => serde_json::json!({
                    "applicable": true,
                    "armed": true,
                    "subscription_id": subscription_id,
                    "every_ms": PROJECT_PATROL_EVERY_MS,
                    "source": "armed",
                    "status": "armed",
                    "checked_at": now,
                    "subscription": subscription,
                    "error": Value::Null,
                }),
                Err(error) => serde_json::json!({
                    "applicable": true,
                    "armed": false,
                    "subscription_id": Value::Null,
                    "every_ms": PROJECT_PATROL_EVERY_MS,
                    "source": "failed",
                    "checked_at": now,
                    "subscription": Value::Null,
                    "error": error,
                }),
            }
        }
        Err(error) => serde_json::json!({
            "applicable": true,
            "armed": false,
            "subscription_id": Value::Null,
            "every_ms": PROJECT_PATROL_EVERY_MS,
            "source": "failed",
            "checked_at": now,
            "subscription": Value::Null,
            "error": error,
        }),
    }
}

pub(super) fn goal_project_patrol_projection(root: &Path, scope: &GoalScope) -> Value {
    if scope.role != ExecutionRole::Master {
        return serde_json::json!({
            "applicable": false,
            "armed": false,
            "subscription_id": Value::Null,
            "every_ms": PROJECT_PATROL_EVERY_MS,
            "error": Value::Null,
        });
    }
    match goal_project_patrol_record(root, scope) {
        Ok(Some((subscription_id, subscription)))
            if subscription["status"].as_str() == Some("cancelled") =>
        {
            serde_json::json!({
                "applicable": true,
                "armed": false,
                "subscription_id": subscription_id,
                "every_ms": PROJECT_PATROL_EVERY_MS,
                "subject": subscription["subject"].clone(),
                "status": "cancelled",
                "source": "cancelled",
                "error": Value::Null,
            })
        }
        Ok(Some((subscription_id, subscription)))
            if goal_patrol_subscription_is_usable(scope, &subscription, goal_now_ms())
                .is_some() =>
        {
            serde_json::json!({
                "applicable": true,
                "armed": true,
                "subscription_id": subscription_id,
                "every_ms": PROJECT_PATROL_EVERY_MS,
                "subject": subscription["subject"].clone(),
                "status": "armed",
                "source": "reused",
                "error": Value::Null,
            })
        }
        Ok(None) => serde_json::json!({
            "applicable": true,
            "armed": false,
            "subscription_id": Value::Null,
            "every_ms": PROJECT_PATROL_EVERY_MS,
            "status": "missing",
            "source": "missing",
            "error": Value::Null,
        }),
        Ok(Some((subscription_id, subscription))) => serde_json::json!({
            "applicable": true,
            "armed": false,
            "subscription_id": subscription_id,
            "every_ms": PROJECT_PATROL_EVERY_MS,
            "subject": subscription["subject"].clone(),
            "status": subscription["status"].clone(),
            "source": "inactive",
            "error": Value::Null,
        }),
        Err(error) => serde_json::json!({
            "applicable": true,
            "armed": false,
            "subscription_id": Value::Null,
            "every_ms": PROJECT_PATROL_EVERY_MS,
            "status": "error",
            "source": "failed",
            "error": error,
        }),
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::OpenOptions;
    use std::io::Write;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn goal_lock_captures_locked_metadata_for_recovery() {
        let root = std::env::temp_dir().join(format!(
            "appsdk-goal-lock-recovery-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        let path = root.join("long-task-goal.lock");
        let mut file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .open(&path)
            .unwrap();
        writeln!(file, "pid=123 owner=crashed-goal-worker").unwrap();
        file.sync_all().unwrap();
        goal_try_advisory_lock(&file).unwrap();

        let captured = goal_lock_metadata_from_file(&mut file).unwrap();
        assert_eq!(captured, "pid=123 owner=crashed-goal-worker\n");
        assert_eq!(
            goal_lock_recovery_reason(&captured),
            "advisory lock released with stale metadata"
        );
        assert_eq!(goal_lock_recovery_reason(""), "empty metadata");
        assert_eq!(
            goal_lock_recovery_reason("pid="),
            "invalid or truncated metadata"
        );

        drop(file);
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn goal_lock_is_nonblocking_and_released_on_close() {
        let root = std::env::temp_dir().join(format!(
            "appsdk-goal-lock-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        let path = root.join("goal.lock");
        let first = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .open(&path)
            .unwrap();
        goal_try_advisory_lock(&first).unwrap();

        let second = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&path)
            .unwrap();
        assert_eq!(
            goal_try_advisory_lock(&second).unwrap_err(),
            "GOAL_LOCK_BUSY"
        );
        drop(first);
        goal_try_advisory_lock(&second).unwrap();

        drop(second);
        fs::remove_dir_all(root).ok();
    }
}
