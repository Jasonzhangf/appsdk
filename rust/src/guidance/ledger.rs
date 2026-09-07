use super::{assert_id, assert_no_symlink, canonical, fail, read_json};
use serde_json::Value;
use std::fs::{self, OpenOptions};
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
    fs::rename(staging, path)
        .unwrap_or_else(|_| fail("GUIDANCE_WRITE_FAILED", "repair project permissions"));
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
    read_json(&root.join(relative), "GUIDANCE_PLAN_NOT_FOUND")
}

pub(super) fn read_events(root: &Path, task: &str) -> Vec<Value> {
    let path = event_file(root, task);
    if !path.is_file() {
        return Vec::new();
    }
    let content = fs::read_to_string(path)
        .unwrap_or_else(|_| fail("GUIDANCE_EVENTS_INVALID", "repair the task event ledger"));
    let mut events = Vec::new();
    for (index, line) in content.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let event: Value = serde_json::from_str(line).unwrap_or_else(|_| {
            fail(
                format!("GUIDANCE_EVENTS_INVALID:line={}", index + 1),
                "repair the task event ledger",
            )
        });
        events.push(event);
    }
    events
}
