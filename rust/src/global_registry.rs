//! AppSDK's host-wide project registration registry.
//!
//! The registry is deliberately smaller than a project governance store.  It
//! records which project roots have opted into the installed AppSDK and where
//! their project-owned state lives.  The canonical default is `~/.appsdk`;
//! `APPSDK_HOME` is an explicit isolated-run override (useful for tests and
//! sandboxes), never a project-controlled path.

use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::env;
use std::fs::{self, File, OpenOptions};
use std::io::{ErrorKind, Read, Write};
#[cfg(unix)]
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};

const REGISTRY_DIR: &str = ".appsdk";
const REGISTRY_FILE: &str = "projects.jsonl";
const REGISTRY_LOCK: &str = "projects.jsonl.lock";
const REGISTRY_SCHEMA_VERSION: u64 = 1;
const REGISTRY_EVENT: &str = "project.registered";
const REGISTRY_SOURCE: &str = "appsdk.init";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct RegistrationEvent {
    schema_version: u64,
    event: String,
    project_id: String,
    project_root: String,
    sdk_version: String,
    registered_at: String,
    source: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct RegistrationReceipt {
    pub registry_root: PathBuf,
    pub registry_path: PathBuf,
    pub project_id: String,
    pub project_root: PathBuf,
    pub sdk_version: String,
    pub idempotent: bool,
}

fn registry_root() -> Result<PathBuf, String> {
    let root = env::var_os("APPSDK_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(REGISTRY_DIR)))
        .ok_or_else(|| "GLOBAL_APPSDK_HOME_UNAVAILABLE: set HOME or APPSDK_HOME".to_string())?;
    if !root.is_absolute() {
        return Err(format!(
            "GLOBAL_APPSDK_HOME_INVALID: path must be absolute: {}",
            root.display()
        ));
    }
    Ok(root)
}

fn project_id(project_root: &Path) -> String {
    let mut digest = Sha256::new();
    digest.update(project_root.to_string_lossy().as_bytes());
    format!("project-{:x}", digest.finalize())
}

fn ensure_no_symlink(path: &Path, label: &str) -> Result<(), String> {
    if fs::symlink_metadata(path)
        .map(|metadata| metadata.file_type().is_symlink())
        .unwrap_or(false)
    {
        return Err(format!(
            "GLOBAL_REGISTRY_SYMLINK:{label}:{}",
            path.display()
        ));
    }
    Ok(())
}

fn is_platform_root_alias(path: &Path) -> bool {
    #[cfg(target_os = "macos")]
    {
        let Some(relative) = path.strip_prefix("/").ok() else {
            return false;
        };
        let expected = Path::new("/private").join(relative);
        return fs::canonicalize(path).is_ok_and(|canonical| canonical == expected);
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = path;
        false
    }
}

fn lock_registry(lock_path: &Path) -> Result<File, String> {
    let file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .open(lock_path)
        .map_err(|error| format!("GLOBAL_REGISTRY_LOCK_OPEN_FAILED:{error}"))?;
    #[cfg(unix)]
    {
        let result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        if result != 0 {
            let error = std::io::Error::last_os_error();
            if matches!(error.kind(), ErrorKind::WouldBlock) {
                return Err(
                    "GLOBAL_REGISTRY_BUSY: another AppSDK registration is in progress".into(),
                );
            }
            return Err(format!("GLOBAL_REGISTRY_LOCK_FAILED:{error}"));
        }
    }
    Ok(file)
}

fn validate_project_root(root: &Path) -> Result<(PathBuf, String), String> {
    if !root.is_dir() {
        return Err(format!(
            "GLOBAL_REGISTRY_PROJECT_INVALID: project root must be an existing directory: {}",
            root.display()
        ));
    }
    ensure_no_symlink(root, "project_root")?;
    let canonical_root = fs::canonicalize(root)
        .map_err(|error| format!("GLOBAL_REGISTRY_PROJECT_CANONICALIZE_FAILED:{error}"))?;
    let canonical_root_text = canonical_root
        .to_str()
        .ok_or_else(|| "GLOBAL_REGISTRY_PROJECT_INVALID: project root is not UTF-8".to_string())?
        .to_string();
    Ok((canonical_root, canonical_root_text))
}

fn validate_registry_root(root: &Path) -> Result<(), String> {
    if !root.is_absolute() {
        return Err(format!(
            "GLOBAL_APPSDK_HOME_INVALID: path must be absolute: {}",
            root.display()
        ));
    }
    let mut current = PathBuf::new();
    for component in root.components() {
        current.push(component.as_os_str());
        match fs::symlink_metadata(&current) {
            Ok(metadata) => {
                if metadata.file_type().is_symlink() {
                    if !is_platform_root_alias(&current)
                        || !fs::metadata(&current).is_ok_and(|target| target.is_dir())
                    {
                        return Err(format!(
                            "GLOBAL_REGISTRY_SYMLINK:registry_root:{}",
                            current.display()
                        ));
                    }
                } else if !metadata.is_dir() {
                    return Err(format!(
                        "GLOBAL_REGISTRY_ROOT_INVALID:not a directory:{}",
                        current.display()
                    ));
                }
            }
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(error) => {
                return Err(format!(
                    "GLOBAL_REGISTRY_ROOT_STAT_FAILED:{}:{error}",
                    current.display()
                ));
            }
        }
    }
    Ok(())
}

fn ensure_registry_root(root: &Path) -> Result<PathBuf, String> {
    validate_registry_root(root)?;
    let mut current = PathBuf::new();
    for component in root.components() {
        current.push(component.as_os_str());
        match fs::symlink_metadata(&current) {
            Ok(metadata) => {
                if metadata.file_type().is_symlink() {
                    if !is_platform_root_alias(&current)
                        || !fs::metadata(&current).is_ok_and(|target| target.is_dir())
                    {
                        return Err(format!(
                            "GLOBAL_REGISTRY_SYMLINK:registry_root:{}",
                            current.display()
                        ));
                    }
                } else if !metadata.is_dir() {
                    return Err(format!(
                        "GLOBAL_REGISTRY_ROOT_INVALID:not a directory:{}",
                        current.display()
                    ));
                }
            }
            Err(error) if error.kind() == ErrorKind::NotFound => {
                fs::create_dir(&current).map_err(|error| {
                    format!(
                        "GLOBAL_REGISTRY_CREATE_FAILED:{}:{error}",
                        current.display()
                    )
                })?;
                let metadata = fs::symlink_metadata(&current).map_err(|error| {
                    format!(
                        "GLOBAL_REGISTRY_ROOT_STAT_FAILED:{}:{error}",
                        current.display()
                    )
                })?;
                if metadata.file_type().is_symlink() || !metadata.is_dir() {
                    return Err(format!(
                        "GLOBAL_REGISTRY_ROOT_INVALID:not a directory:{}",
                        current.display()
                    ));
                }
            }
            Err(error) => {
                return Err(format!(
                    "GLOBAL_REGISTRY_ROOT_STAT_FAILED:{}:{error}",
                    current.display()
                ));
            }
        }
    }
    validate_registry_root(root)?;
    fs::canonicalize(root).map_err(|error| format!("GLOBAL_REGISTRY_CANONICALIZE_FAILED:{error}"))
}

fn read_latest(path: &Path, project_root: &str) -> Result<Option<RegistrationEvent>, String> {
    let mut file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("GLOBAL_REGISTRY_READ_FAILED:{error}")),
    };
    let mut text = String::new();
    file.read_to_string(&mut text)
        .map_err(|error| format!("GLOBAL_REGISTRY_READ_FAILED:{error}"))?;
    if !text.is_empty() && !text.as_bytes().ends_with(b"\n") {
        return Err("GLOBAL_REGISTRY_INVALID_LINE:missing final newline".into());
    }
    let mut latest = None;
    for (index, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            return Err(format!(
                "GLOBAL_REGISTRY_INVALID_LINE:{}:blank line",
                index + 1
            ));
        }
        let event: RegistrationEvent = serde_json::from_str(line)
            .map_err(|error| format!("GLOBAL_REGISTRY_INVALID_LINE:{}:{}", index + 1, error))?;
        if event.schema_version != REGISTRY_SCHEMA_VERSION
            || event.event != REGISTRY_EVENT
            || event.source != REGISTRY_SOURCE
            || !Path::new(&event.project_root).is_absolute()
            || event.project_root.trim().is_empty()
            || event.project_id.trim().is_empty()
            || event.sdk_version.trim().is_empty()
            || event.project_id != project_id(Path::new(&event.project_root))
        {
            return Err(format!(
                "GLOBAL_REGISTRY_INVALID_EVENT:{}:unsupported registration shape",
                index + 1
            ));
        }
        if event.project_root == project_root {
            latest = Some(event);
        }
    }
    Ok(latest)
}

/// Record one project registration in the host-wide AppSDK registry.
/// Re-registering the same canonical root and SDK version is idempotent.
pub fn register_project(root: &Path, sdk_version: &str) -> Result<RegistrationReceipt, String> {
    let registry_root = registry_root()?;
    register_project_at(root, registry_root.as_path(), sdk_version)
}

/// Record a project registration at an explicit registry root.
///
/// This is the same operation as [`register_project`], with the registry
/// location injected for isolated tests and controlled migration tooling.
/// Production callers should use [`register_project`] so the canonical host
/// location remains `~/.appsdk`.
pub fn register_project_at(
    project_root: &Path,
    registry_root: &Path,
    sdk_version: &str,
) -> Result<RegistrationReceipt, String> {
    let (canonical_root, canonical_root_text) = validate_project_root(project_root)?;
    if sdk_version.trim().is_empty() {
        return Err("GLOBAL_REGISTRY_SDK_VERSION_INVALID: version must not be empty".into());
    }

    let registry_root = ensure_registry_root(registry_root)?;
    ensure_no_symlink(&registry_root, "registry_root")?;
    let path = registry_root.join(REGISTRY_FILE);
    let lock_path = registry_root.join(REGISTRY_LOCK);
    ensure_no_symlink(&path, "registry_file")?;
    ensure_no_symlink(&lock_path, "registry_lock")?;
    let _lock = lock_registry(&lock_path)?;
    let id = project_id(&canonical_root);
    if let Some(existing) = read_latest(&path, &canonical_root_text)? {
        if existing.project_id == id && existing.sdk_version == sdk_version {
            return Ok(RegistrationReceipt {
                registry_root: registry_root.to_path_buf(),
                registry_path: path,
                project_id: id,
                project_root: canonical_root,
                sdk_version: sdk_version.to_string(),
                idempotent: true,
            });
        }
    }
    let event = RegistrationEvent {
        schema_version: REGISTRY_SCHEMA_VERSION,
        event: REGISTRY_EVENT.to_string(),
        project_id: id.clone(),
        project_root: canonical_root_text.to_string(),
        sdk_version: sdk_version.to_string(),
        registered_at: Utc::now().to_rfc3339(),
        source: REGISTRY_SOURCE.to_string(),
    };
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|error| format!("GLOBAL_REGISTRY_OPEN_FAILED:{error}"))?;
    let mut line = serde_json::to_vec(&event)
        .map_err(|error| format!("GLOBAL_REGISTRY_SERIALIZE_FAILED:{error}"))?;
    line.push(b'\n');
    file.write_all(&line)
        .and_then(|_| file.sync_all())
        .map_err(|error| format!("GLOBAL_REGISTRY_WRITE_FAILED:{error}"))?;
    Ok(RegistrationReceipt {
        registry_root: registry_root.to_path_buf(),
        registry_path: path,
        project_id: id,
        project_root: canonical_root,
        sdk_version: sdk_version.to_string(),
        idempotent: false,
    })
}

pub fn receipt_json(receipt: &RegistrationReceipt) -> Value {
    serde_json::to_value(receipt).unwrap_or_else(|_| Value::Null)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registration_is_idempotent_and_append_only() {
        let root = std::env::temp_dir().join(format!(
            "appsdk-global-registry-{}-{}",
            std::process::id(),
            Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        let project = root.join("project");
        let home = root.join("home").join(".appsdk");
        fs::create_dir_all(&project).unwrap();
        fs::create_dir_all(&home).unwrap();
        let first = register_project_at(&project, &home, "0.1.6").unwrap();
        let second = register_project_at(&project, &home, "0.1.6").unwrap();
        assert!(!first.idempotent);
        assert!(second.idempotent);
        assert_eq!(
            fs::read_to_string(home.join(REGISTRY_FILE))
                .unwrap()
                .lines()
                .count(),
            1
        );
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn malformed_registry_fails_closed() {
        let root = std::env::temp_dir().join(format!(
            "appsdk-global-registry-invalid-{}-{}",
            std::process::id(),
            Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        let project = root.join("project");
        let home = root.join("home").join(".appsdk");
        fs::create_dir_all(&project).unwrap();
        fs::create_dir_all(&home).unwrap();
        fs::write(home.join(REGISTRY_FILE), b"not-json\n").unwrap();
        let error = register_project_at(&project, &home, "0.1.6").unwrap_err();
        assert!(error.starts_with("GLOBAL_REGISTRY_INVALID_LINE:1:"));
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn unterminated_registry_fails_closed_before_idempotent_or_append() {
        let root = std::env::temp_dir().join(format!(
            "appsdk-global-registry-unterminated-{}-{}",
            std::process::id(),
            Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        let project = root.join("project");
        let registry = root.join("registry");
        fs::create_dir_all(&project).unwrap();
        let first = register_project_at(&project, &registry, "0.1.6").unwrap();
        let path = registry.join(REGISTRY_FILE);
        let bytes = fs::read(&path).unwrap();
        assert!(bytes.ends_with(b"\n"));
        fs::write(&path, &bytes[..bytes.len() - 1]).unwrap();
        let same_version = register_project_at(&project, &registry, "0.1.6").unwrap_err();
        assert_eq!(
            same_version,
            "GLOBAL_REGISTRY_INVALID_LINE:missing final newline"
        );
        let next_version = register_project_at(&project, &registry, "0.1.7").unwrap_err();
        assert_eq!(
            next_version,
            "GLOBAL_REGISTRY_INVALID_LINE:missing final newline"
        );
        assert_eq!(fs::read(&path).unwrap(), &bytes[..bytes.len() - 1]);
        assert_eq!(
            first.project_id,
            project_id(&project.canonicalize().unwrap())
        );
        fs::remove_dir_all(root).ok();
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_registry_ancestor_fails_closed_before_creation() {
        use std::os::unix::fs::symlink;

        let root = std::env::temp_dir().join(format!(
            "appsdk-global-registry-symlink-{}-{}",
            std::process::id(),
            Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        let project = root.join("project");
        let real_parent = root.join("real");
        let linked_parent = root.join("linked");
        fs::create_dir_all(&project).unwrap();
        fs::create_dir_all(&real_parent).unwrap();
        symlink(&real_parent, &linked_parent).unwrap();
        let requested = linked_parent.join("new-registry");

        let error = register_project_at(&project, &requested, "0.1.6").unwrap_err();
        assert!(error.starts_with("GLOBAL_REGISTRY_SYMLINK:registry_root:"));
        assert!(!real_parent.join("new-registry").exists());
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn distinct_canonical_projects_append_distinct_entries() {
        let root = std::env::temp_dir().join(format!(
            "appsdk-global-registry-distinct-{}-{}",
            std::process::id(),
            Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        let project_a = root.join("a");
        let project_b = root.join("b");
        let registry = root.join("registry");
        fs::create_dir_all(&project_a).unwrap();
        fs::create_dir_all(&project_b).unwrap();

        let first = register_project_at(&project_a, &registry, "0.1.6").unwrap();
        let second = register_project_at(&project_b, &registry, "0.1.6").unwrap();
        assert_ne!(first.project_id, second.project_id);
        assert_eq!(
            fs::read_to_string(registry.join(REGISTRY_FILE))
                .unwrap()
                .lines()
                .count(),
            2
        );
        fs::remove_dir_all(root).ok();
    }
}
