use super::{assert_id, assert_no_symlink, canonical, fail};
use serde_json::Value;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

pub(super) struct TaskLock {
    path: PathBuf,
}

impl Drop for TaskLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

pub(super) fn lock_task(root: &Path, task: &str, op: &str) -> TaskLock {
    let dir = task_dir(root, task);
    fs::create_dir_all(&dir)
        .unwrap_or_else(|_| fail("GUIDANCE_WRITE_FAILED", "repair project permissions"));
    let path = dir.join("write.lock");
    let created = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_else(|_| fail("GUIDANCE_CLOCK_FAILED", "repair the host clock"))
        .as_nanos();
    let open_lock = || {
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
    };
    let mut file = match open_lock() {
        Ok(file) => file,
        Err(_) if stale_lock(&path) => {
            let _ = fs::remove_file(&path);
            match open_lock() {
                Ok(file) => file,
                Err(_) => fail(
                    format!("GUIDANCE_TASK_LOCKED:{}:{}", task, op),
                    "wait for the other plan/update writer to finish, or remove the stale write.lock only after verifying no live owner",
                ),
            }
        }
        Err(_) => {
            fail(
                format!("GUIDANCE_TASK_LOCKED:{}:{}", task, op),
                "wait for the other plan/update writer to finish, or remove the stale write.lock only after verifying no live owner",
            );
        }
    };
    writeln!(
        file,
        "pid={} op={} created={}",
        std::process::id(),
        op,
        created
    )
    .unwrap_or_else(|_| fail("GUIDANCE_WRITE_FAILED", "repair project permissions"));
    file.sync_all()
        .unwrap_or_else(|_| fail("GUIDANCE_WRITE_FAILED", "repair project permissions"));
    TaskLock { path }
}

fn stale_lock(path: &Path) -> bool {
    let Ok(content) = fs::read_to_string(path) else {
        return true;
    };
    let Some(pid) = content.lines().find_map(|line| {
        line.strip_prefix("pid=")
            .and_then(|value| value.split_whitespace().next())
            .and_then(|value| value.parse::<i32>().ok())
    }) else {
        return true;
    };
    let alive = Command::new("/bin/kill")
        .args(["-0", &pid.to_string()])
        .status()
        .is_ok_and(|status| status.success());
    !alive
}

pub(super) fn atomic_write(path: &Path, value: &Value) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .unwrap_or_else(|_| fail("GUIDANCE_WRITE_FAILED", "repair project permissions"));
    }
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_else(|_| fail("GUIDANCE_CLOCK_FAILED", "repair the host clock"))
        .as_nanos();
    let staging = path.with_extension(format!("staging.{}.{}", std::process::id(), nonce));
    fs::write(
        &staging,
        serde_json::to_string_pretty(value).unwrap() + "\n",
    )
    .unwrap_or_else(|_| fail("GUIDANCE_WRITE_FAILED", "repair project permissions"));
    File::open(&staging)
        .and_then(|file| file.sync_all())
        .unwrap_or_else(|_| fail("GUIDANCE_WRITE_FAILED", "repair project permissions"));
    fs::rename(staging, path)
        .unwrap_or_else(|_| fail("GUIDANCE_WRITE_FAILED", "repair project permissions"));
    if let Some(parent) = path.parent() {
        fs::File::open(parent)
            .and_then(|dir| dir.sync_all())
            .unwrap_or_else(|_| fail("GUIDANCE_WRITE_FAILED", "repair project permissions"));
    }
}

pub(super) fn append(path: &Path, value: &Value) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .unwrap_or_else(|_| fail("GUIDANCE_EVENT_WRITE_FAILED", "repair project permissions"));
    }
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .unwrap_or_else(|_| fail("GUIDANCE_EVENT_WRITE_FAILED", "repair project permissions"));
    writeln!(file, "{}", canonical(value))
        .unwrap_or_else(|_| fail("GUIDANCE_EVENT_WRITE_FAILED", "repair project permissions"));
    file.sync_all()
        .unwrap_or_else(|_| fail("GUIDANCE_EVENT_WRITE_FAILED", "repair project permissions"));
}

fn task_dir(root: &Path, task: &str) -> PathBuf {
    assert_id(task, "GUIDANCE_TASK_ID_INVALID");
    let relative = PathBuf::from(".appsdk-control/guidance").join(task);
    assert_no_symlink(root, &relative, "GUIDANCE_TASK_CONTROL_SYMLINK");
    root.join(relative)
}

pub(super) fn plan_file(root: &Path, task: &str) -> PathBuf {
    task_dir(root, task).join("plan.json")
}

pub(super) fn event_file(root: &Path, task: &str) -> PathBuf {
    task_dir(root, task).join("events.jsonl")
}

pub(super) fn read_plan(root: &Path, task: &str) -> Value {
    let relative = PathBuf::from(".appsdk-control/guidance")
        .join(task)
        .join("plan.json");
    assert_no_symlink(root, &relative, "GUIDANCE_PLAN_SYMLINK");
    let path = root.join(relative);
    let events = read_events(root, task);
    let journal_plan = events
        .iter()
        .rev()
        .find(|event| event.get("record_type").and_then(Value::as_str) == Some("PlanRecord"))
        .cloned();
    if let Some(plan) = journal_plan {
        let cache = if path.is_file() {
            let content = fs::read_to_string(&path)
                .unwrap_or_else(|_| fail("GUIDANCE_PLAN_NOT_FOUND", "restore the task plan"));
            match serde_json::from_str::<Value>(&content) {
                Ok(cache) => Some(cache),
                Err(_) => {
                    quarantine_plan(&path, "invalid");
                    None
                }
            }
        } else {
            None
        };
        let cache_matches = cache.as_ref().is_some_and(|cache| cache == &plan);
        if cache.is_some() && !cache_matches {
            quarantine_plan(&path, "stale");
        }
        if !cache_matches {
            atomic_write(&path, &plan);
        }
        return plan;
    }
    if path.is_file() {
        return serde_json::from_str(
            &fs::read_to_string(&path)
                .unwrap_or_else(|_| fail("GUIDANCE_PLAN_NOT_FOUND", "restore the task plan")),
        )
        .unwrap_or_else(|_| fail("GUIDANCE_PLAN_INVALID", "repair the task plan"));
    }
    fail(
        "GUIDANCE_PLAN_NOT_FOUND",
        "submit a plan before update/status",
    )
}

fn quarantine_plan(path: &Path, kind: &str) {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_else(|_| fail("GUIDANCE_CLOCK_FAILED", "repair the host clock"))
        .as_nanos();
    let quarantine = path.with_extension(format!("{}.{}.{}", kind, std::process::id(), nonce));
    fs::rename(path, quarantine)
        .unwrap_or_else(|_| fail("GUIDANCE_PLAN_RECOVERY_FAILED", "preserve the plan cache"));
}

pub(super) fn read_events(root: &Path, task: &str) -> Vec<Value> {
    let path = event_file(root, task);
    if !path.is_file() {
        return Vec::new();
    }
    let content = fs::read_to_string(&path)
        .unwrap_or_else(|_| fail("GUIDANCE_EVENTS_INVALID", "repair the task event ledger"));
    let mut events = Vec::new();
    let ends_with_newline = content.ends_with('\n');
    let mut offset = 0;
    for (index, segment) in content.split_inclusive('\n').enumerate() {
        let line = segment.trim_end_matches('\n').trim_end_matches('\r');
        if line.trim().is_empty() {
            offset += segment.len();
            continue;
        }
        let event: Value = match serde_json::from_str(line) {
            Ok(event) => event,
            Err(_) if index + 1 == content.split('\n').count() && !ends_with_newline => {
                quarantine_partial_tail(&path, &content[offset..]);
                break;
            }
            Err(_) => fail(
                format!("GUIDANCE_EVENTS_INVALID:line={}", index + 1),
                "repair the task event ledger",
            ),
        };
        events.push(event);
        offset += segment.len();
    }
    events
}

fn quarantine_partial_tail(path: &Path, tail: &str) {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_else(|_| fail("GUIDANCE_CLOCK_FAILED", "repair the host clock"))
        .as_nanos();
    let quarantine = path.with_extension(format!("corrupt-tail.{}.{}", std::process::id(), nonce));
    fs::write(&quarantine, tail).unwrap_or_else(|_| {
        fail(
            "GUIDANCE_EVENTS_RECOVERY_FAILED",
            "preserve the corrupt tail",
        )
    });
    File::open(&quarantine)
        .and_then(|file| file.sync_all())
        .unwrap_or_else(|_| {
            fail(
                "GUIDANCE_EVENTS_RECOVERY_FAILED",
                "preserve the corrupt tail",
            )
        });

    let staging = path.with_extension(format!("recovered.{}.{}", std::process::id(), nonce));
    let valid_prefix = fs::read_to_string(path).unwrap_or_else(|_| {
        fail(
            "GUIDANCE_EVENTS_RECOVERY_FAILED",
            "read the task event ledger",
        )
    });
    let valid_prefix = valid_prefix.strip_suffix(tail).unwrap_or_else(|| {
        fail(
            "GUIDANCE_EVENTS_RECOVERY_FAILED",
            "preserve the corrupt tail",
        )
    });
    fs::write(&staging, valid_prefix).unwrap_or_else(|_| {
        fail(
            "GUIDANCE_EVENTS_RECOVERY_FAILED",
            "restore the valid event prefix",
        )
    });
    File::open(&staging)
        .and_then(|file| file.sync_all())
        .and_then(|_| fs::rename(&staging, path))
        .unwrap_or_else(|_| {
            fail(
                "GUIDANCE_EVENTS_RECOVERY_FAILED",
                "restore the valid event prefix",
            )
        });
    if let Some(parent) = path.parent() {
        File::open(parent)
            .and_then(|file| file.sync_all())
            .unwrap_or_else(|_| fail("GUIDANCE_EVENTS_RECOVERY_FAILED", "sync the task ledger"));
    }
}
