use serde_json::Value;
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

#[allow(dead_code)]
pub(crate) fn canonical_json(value: &Value) -> String {
    match value {
        Value::Null => "null".into(),
        Value::Bool(value) => value.to_string(),
        Value::Number(value) => value.to_string(),
        Value::String(value) => serde_json::to_string(value).unwrap(),
        Value::Array(values) => format!(
            "[{}]",
            values
                .iter()
                .map(canonical_json)
                .collect::<Vec<_>>()
                .join(",")
        ),
        Value::Object(values) => {
            let mut keys = values.keys().collect::<Vec<_>>();
            keys.sort();
            format!(
                "{{{}}}",
                keys.iter()
                    .map(|key| format!(
                        "{}:{}",
                        serde_json::to_string(key).unwrap(),
                        canonical_json(&values[*key])
                    ))
                    .collect::<Vec<_>>()
                    .join(",")
            )
        }
    }
}

pub(crate) fn sha256_bytes(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("sha256:{:x}", hasher.finalize())
}

pub(crate) fn sha256_str(value: &str) -> String {
    sha256_bytes(value.as_bytes())
}

#[allow(dead_code)]
pub(crate) fn sha256_value(value: &Value) -> String {
    sha256_str(&canonical_json(value))
}

pub(crate) fn is_valid_id(value: &str, max_len: usize, allow_slash: bool) -> bool {
    !value.is_empty()
        && value.len() <= max_len
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric()
                || matches!(byte, b'-' | b'_' | b'.')
                || (allow_slash && byte == b'/')
        })
}

#[allow(dead_code)]
pub(crate) fn git_current_branch(root: &Path) -> Result<Option<String>, ()> {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["symbolic-ref", "--quiet", "--short", "HEAD"])
        .output()
        .map_err(|_| ())?;
    if output.status.success() {
        Ok(Some(
            String::from_utf8_lossy(&output.stdout).trim().to_string(),
        ))
    } else {
        Ok(None)
    }
}

#[allow(dead_code)]
pub(crate) fn staging_path_for(target: &Path) -> Result<PathBuf, std::time::SystemTimeError> {
    let nonce = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
    Ok(target.with_extension(format!("staging.{}.{}", std::process::id(), nonce)))
}

#[allow(dead_code)]
pub(crate) fn write_and_sync_file(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut file = File::create(path)?;
    file.write_all(bytes)?;
    file.sync_all()
}

#[allow(dead_code)]
pub(crate) fn sync_parent_dir(path: &Path) -> std::io::Result<()> {
    #[cfg(not(windows))]
    if let Some(parent) = path.parent() {
        let directory = File::open(parent)?;
        directory.sync_all()?;
    }
    #[cfg(windows)]
    let _ = path;
    Ok(())
}

#[allow(dead_code)]
pub(crate) fn is_symlink(path: &Path) -> bool {
    fs::symlink_metadata(path)
        .map(|metadata| metadata.file_type().is_symlink())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn canonical_json_sorts_object_keys_recursively() {
        let left = json!({"b": 1, "a": {"d": true, "c": [3, 2, 1]}});
        let right = json!({"a": {"c": [3, 2, 1], "d": true}, "b": 1});
        assert_eq!(canonical_json(&left), canonical_json(&right));
        assert_eq!(sha256_value(&left), sha256_value(&right));
        assert!(sha256_str("hello").starts_with("sha256:"));
        assert_eq!(sha256_str("hello").len(), 71);
    }

    #[test]
    fn is_valid_id_enforces_length_and_charset() {
        assert!(is_valid_id("abc-1_2.3", 160, false));
        assert!(!is_valid_id("abc/1", 160, false));
        assert!(is_valid_id("abc/1", 160, true));
        assert!(!is_valid_id("", 160, true));
        assert!(!is_valid_id(&"a".repeat(161), 160, true));
        assert!(!is_valid_id("bad id", 160, true));
    }
}
