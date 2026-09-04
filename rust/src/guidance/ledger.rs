use super::{assert_id, assert_no_symlink, canonical, fail, read_json};
use serde_json::Value;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

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
    fs::read_to_string(path)
        .unwrap_or_else(|_| fail("GUIDANCE_EVENTS_INVALID", "repair the task event ledger"))
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            serde_json::from_str(line)
                .unwrap_or_else(|_| fail("GUIDANCE_EVENTS_INVALID", "repair the task event ledger"))
        })
        .collect()
}
