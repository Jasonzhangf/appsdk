use serde_json::json;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

pub(super) fn append_command_invocation(
    root: &Path,
    command: &str,
    started_at: &str,
    elapsed_ms: Option<u128>,
    selection: Option<(&str, &str)>,
    outcome: &str,
    error_code: Option<&str>,
) -> Result<(), String> {
    let root = absolute_log_root(root)?;
    let path = root.join(".appsdk-control/command-invocations.jsonl");
    match fs::metadata(&root) {
        Ok(metadata) if metadata.is_dir() => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err("root:NotFound".to_owned());
        }
        Err(error) => return Err(format!("root:{:?}", error.kind())),
        Ok(_) => return Err("root:not_directory".to_owned()),
    }
    crate::platform::validate_contained_path(&root, &path).map_err(|error| match error {
        crate::platform::PathBoundaryError::BaseLink(_)
        | crate::platform::PathBoundaryError::BaseNotDirectory(_) => "root:boundary".to_owned(),
        _ => "path:boundary".to_owned(),
    })?;
    fs::create_dir_all(root.join(".appsdk-control"))
        .map_err(|error| format!("mkdir:{:?}", error.kind()))?;
    crate::platform::validate_contained_path(&root, &path)
        .map_err(|_| "path:boundary".to_owned())?;
    match fs::symlink_metadata(&path) {
        Ok(metadata) if !metadata.is_file() => return Err("path:not_regular_file".to_owned()),
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
            return Err(format!("path:{:?}", error.kind()));
        }
        _ => {}
    }

    let mut record = json!({
        "command": command,
        "started_at": started_at,
        "outcome": outcome,
    });
    if let Some(elapsed_ms) = elapsed_ms {
        record["elapsed_ms"] = json!(elapsed_ms);
    }
    if let Some((field, value)) = selection {
        record[field] = json!(value);
    }
    if let Some(error_code) = error_code {
        record["error_code"] = json!(error_code);
    }

    let mut line = serde_json::to_vec(&record).map_err(|_| "serialize:json".to_owned())?;
    line.push(b'\n');
    let mut file = fs::OpenOptions::new()
        .read(true)
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|error| format!("open:{:?}", error.kind()))?;
    crate::platform::try_lock_exclusive(&file).map_err(|error| match error {
        crate::platform::LockAttemptError::WouldBlock => "lock:would_block".to_owned(),
        crate::platform::LockAttemptError::Io(error) => format!("lock:{:?}", error.kind()),
    })?;
    file.write_all(&line)
        .map_err(|error| format!("append:{:?}", error.kind()))
}

fn absolute_log_root(root: &Path) -> Result<PathBuf, String> {
    if root.is_absolute() {
        return Ok(root.to_path_buf());
    }
    std::env::current_dir()
        .map(|cwd| cwd.join(root))
        .map_err(|error| format!("root:{:?}", error.kind()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_root(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "appsdk-command-invocation-{label}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ))
    }

    #[test]
    fn invoked_record_omits_elapsed_time() {
        let root = temp_root("invoked");
        fs::create_dir_all(&root).unwrap();
        append_command_invocation(
            &root,
            "verify",
            "2026-11-10T00:00:00Z",
            None,
            Some(("selection", "admission")),
            "invoked",
            None,
        )
        .unwrap();
        let raw =
            fs::read_to_string(root.join(".appsdk-control/command-invocations.jsonl")).unwrap();
        let record: serde_json::Value = serde_json::from_str(raw.trim()).unwrap();
        assert_eq!(record["command"], "verify");
        assert_eq!(record["outcome"], "invoked");
        assert_eq!(record["selection"], "admission");
        assert!(record.get("elapsed_ms").is_none());
        assert!(record.get("error_code").is_none());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn missing_root_is_not_created() {
        let root = temp_root("missing-root");
        let error = append_command_invocation(
            &root,
            "verify",
            "2026-11-10T00:00:00Z",
            None,
            Some(("selection", "default")),
            "invoked",
            None,
        )
        .unwrap_err();
        assert_eq!(error, "root:NotFound");
        assert!(!root.exists());
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_root_is_not_written_through() {
        use std::os::unix::fs::symlink;

        let base = temp_root("symlink-root");
        let outside = base.join("outside");
        fs::create_dir_all(&outside).unwrap();
        let sentinel = outside.join("command-invocations.jsonl");
        fs::write(&sentinel, "keep this content\n").unwrap();
        let linked_root = base.join("linked-root");
        symlink(&outside, &linked_root).unwrap();
        let error = append_command_invocation(
            &linked_root,
            "verify",
            "2026-11-10T00:00:00Z",
            None,
            Some(("selection", "default")),
            "invoked",
            None,
        )
        .unwrap_err();
        assert_eq!(error, "root:boundary");
        assert_eq!(
            fs::read_to_string(&sentinel).unwrap(),
            "keep this content\n"
        );
        fs::remove_dir_all(base).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_control_directory_is_not_written_through() {
        use std::os::unix::fs::symlink;

        let base = temp_root("symlink-control");
        let outside = base.join("outside");
        fs::create_dir_all(&outside).unwrap();
        let sentinel = outside.join("command-invocations.jsonl");
        fs::write(&sentinel, "keep this content\n").unwrap();
        let root = base.join("root");
        fs::create_dir_all(&root).unwrap();
        symlink(&outside, root.join(".appsdk-control")).unwrap();
        let error = append_command_invocation(
            &root,
            "verify",
            "2026-11-10T00:00:00Z",
            None,
            Some(("selection", "default")),
            "invoked",
            None,
        )
        .unwrap_err();
        assert_eq!(error, "path:boundary");
        assert_eq!(
            fs::read_to_string(&sentinel).unwrap(),
            "keep this content\n"
        );
        fs::remove_dir_all(base).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_log_target_is_not_written_through() {
        use std::os::unix::fs::symlink;

        let base = temp_root("symlink-target");
        let outside = base.join("outside");
        fs::create_dir_all(&outside).unwrap();
        let sentinel = outside.join("command-invocations.jsonl");
        fs::write(&sentinel, "keep this content\n").unwrap();
        let root = base.join("root");
        fs::create_dir_all(root.join(".appsdk-control")).unwrap();
        symlink(
            &sentinel,
            root.join(".appsdk-control/command-invocations.jsonl"),
        )
        .unwrap();
        let error = append_command_invocation(
            &root,
            "verify",
            "2026-11-10T00:00:00Z",
            None,
            Some(("selection", "default")),
            "invoked",
            None,
        )
        .unwrap_err();
        assert_eq!(error, "path:boundary");
        assert_eq!(
            fs::read_to_string(&sentinel).unwrap(),
            "keep this content\n"
        );
        fs::remove_dir_all(base).unwrap();
    }
}
