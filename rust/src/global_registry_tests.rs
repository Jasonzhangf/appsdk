use super::*;
use serde_json::Value;
use std::sync::{Arc, Barrier};
use std::thread;
use std::time::{Duration, Instant};

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
    let newer = register_project_at(&project, &home, "0.1.0007").unwrap();
    assert!(!newer.idempotent);
    let older_again = register_project_at(&project, &home, "0.1.6").unwrap();
    assert!(older_again.idempotent);
    assert_eq!(
        fs::read_to_string(home.join(REGISTRY_FILE))
            .unwrap()
            .lines()
            .count(),
        2
    );
    fs::remove_dir_all(root).ok();
}

#[test]
fn concurrent_first_registrations_share_new_registry_root() {
    let root = std::env::temp_dir().join(format!(
        "appsdk-global-registry-concurrent-first-{}-{}",
        std::process::id(),
        Utc::now().timestamp_nanos_opt().unwrap_or_default()
    ));
    let project_a = root.join("a");
    let project_b = root.join("b");
    let registry = root.join("new").join("registry");
    fs::create_dir_all(&project_a).unwrap();
    fs::create_dir_all(&project_b).unwrap();

    let start = Arc::new(Barrier::new(2));
    let handles = [project_a, project_b].map(|project| {
        let registry = registry.clone();
        let start = Arc::clone(&start);
        thread::spawn(move || {
            start.wait();
            let deadline = Instant::now() + Duration::from_secs(2);
            loop {
                match register_project_at(&project, &registry, "0.1.6") {
                    Ok(receipt) => return receipt,
                    Err(error) if error.starts_with("GLOBAL_REGISTRY_BUSY:") => {
                        assert!(
                            Instant::now() < deadline,
                            "concurrent first registration stayed busy"
                        );
                        thread::sleep(Duration::from_millis(1));
                    }
                    Err(error) => panic!("concurrent first registration failed: {error}"),
                }
            }
        })
    });
    let receipts = handles.map(|handle| handle.join().unwrap());

    assert_ne!(receipts[0].project_id, receipts[1].project_id);
    assert_eq!(
        fs::read_to_string(registry.join(REGISTRY_FILE))
            .unwrap()
            .lines()
            .count(),
        2
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
    let next_version = register_project_at(&project, &registry, "0.1.0007").unwrap_err();
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

#[test]
fn noncanonical_registry_project_root_fails_closed() {
    let root = std::env::temp_dir().join(format!(
        "appsdk-global-registry-noncanonical-{}-{}",
        std::process::id(),
        Utc::now().timestamp_nanos_opt().unwrap_or_default()
    ));
    let project = root.join("project");
    let registry = root.join("registry");
    fs::create_dir_all(&project).unwrap();
    register_project_at(&project, &registry, "0.1.6").unwrap();

    let path = registry.join(REGISTRY_FILE);
    let mut event: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
    let noncanonical_root = project.join("..").join("project");
    let noncanonical_root_text = noncanonical_root.to_str().unwrap();
    event["project_root"] = Value::String(noncanonical_root_text.to_string());
    event["project_id"] = Value::String(project_id(&noncanonical_root));
    fs::write(
        &path,
        format!("{}\n", serde_json::to_string(&event).unwrap()),
    )
    .unwrap();

    let error = register_project_at(&project, &registry, "0.1.6").unwrap_err();
    assert!(error.starts_with("GLOBAL_REGISTRY_INVALID_EVENT:1:"));
    fs::remove_dir_all(root).ok();
}

#[test]
fn missing_historical_project_does_not_block_registration() {
    let root = std::env::temp_dir().join(format!(
        "appsdk-global-registry-missing-project-{}-{}",
        std::process::id(),
        Utc::now().timestamp_nanos_opt().unwrap_or_default()
    ));
    let project_a = root.join("project-a");
    let project_b = root.join("project-b");
    let registry = root.join("registry");
    fs::create_dir_all(&project_a).unwrap();
    fs::create_dir_all(&project_b).unwrap();
    let removed_project = root.canonicalize().unwrap().join("removed-project");
    register_project_at(&project_a, &registry, "0.1.6").unwrap();

    let path = registry.join(REGISTRY_FILE);
    let mut event: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
    event["project_root"] = Value::String(removed_project.to_str().unwrap().to_string());
    event["project_id"] = Value::String(project_id(&removed_project));
    fs::write(
        &path,
        format!("{}\n", serde_json::to_string(&event).unwrap()),
    )
    .unwrap();

    let receipt = register_project_at(&project_b, &registry, "0.1.6").unwrap();
    assert!(!receipt.idempotent);
    assert_eq!(fs::read_to_string(&path).unwrap().lines().count(), 2);
    fs::remove_dir_all(root).ok();
}

#[test]
fn missing_ancestor_with_noncanonical_root_fails_closed() {
    let root = std::env::temp_dir().join(format!(
        "appsdk-global-registry-missing-ancestor-{}-{}",
        std::process::id(),
        Utc::now().timestamp_nanos_opt().unwrap_or_default()
    ));
    let project = root.join("project");
    let registry = root.join("registry");
    fs::create_dir_all(&project).unwrap();
    register_project_at(&project, &registry, "0.1.6").unwrap();

    let path = registry.join(REGISTRY_FILE);
    let mut event: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
    let noncanonical_root = root.join("missing").join("..").join("removed-project");
    event["project_root"] = Value::String(noncanonical_root.to_str().unwrap().to_string());
    event["project_id"] = Value::String(project_id(&noncanonical_root));
    fs::write(
        &path,
        format!("{}\n", serde_json::to_string(&event).unwrap()),
    )
    .unwrap();

    let error = register_project_at(&project, &registry, "0.1.6").unwrap_err();
    assert!(error.starts_with("GLOBAL_REGISTRY_INVALID_EVENT:1:"));
    fs::remove_dir_all(root).ok();
}

#[cfg(unix)]
#[test]
fn missing_root_under_symlinked_ancestor_fails_closed() {
    use std::os::unix::fs::symlink;

    let root = std::env::temp_dir().join(format!(
        "appsdk-global-registry-missing-symlink-{}-{}",
        std::process::id(),
        Utc::now().timestamp_nanos_opt().unwrap_or_default()
    ));
    let project = root.join("project");
    let registry = root.join("registry");
    let real_parent = root.join("real");
    let linked_parent = root.join("linked");
    fs::create_dir_all(&project).unwrap();
    fs::create_dir_all(&real_parent).unwrap();
    symlink(&real_parent, &linked_parent).unwrap();
    register_project_at(&project, &registry, "0.1.6").unwrap();

    let path = registry.join(REGISTRY_FILE);
    let mut event: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
    let linked_missing_root = linked_parent.join("removed-project");
    event["project_root"] = Value::String(linked_missing_root.to_str().unwrap().to_string());
    event["project_id"] = Value::String(project_id(&linked_missing_root));
    fs::write(
        &path,
        format!("{}\n", serde_json::to_string(&event).unwrap()),
    )
    .unwrap();

    let error = register_project_at(&project, &registry, "0.1.6").unwrap_err();
    assert!(error.starts_with("GLOBAL_REGISTRY_INVALID_EVENT:1:"));
    fs::remove_dir_all(root).ok();
}

#[cfg(target_os = "macos")]
#[test]
fn missing_root_under_tmp_alias_fails_closed() {
    let root = format!(
        "/tmp/appsdk-global-registry-missing-tmp-{}-{}",
        std::process::id(),
        Utc::now().timestamp_nanos_opt().unwrap_or_default()
    );
    let project = PathBuf::from(&root).join("project");
    let registry = PathBuf::from(&root).join("registry");
    fs::create_dir_all(&project).unwrap();
    register_project_at(&project, &registry, "0.1.6").unwrap();

    let path = registry.join(REGISTRY_FILE);
    let mut event: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
    let missing_root = PathBuf::from(&root).join("removed-project");
    fs::remove_dir_all(&project).unwrap();
    event["project_root"] = Value::String(missing_root.to_string_lossy().into_owned());
    event["project_id"] = Value::String(project_id(&missing_root));
    fs::write(
        &path,
        format!("{}\n", serde_json::to_string(&event).unwrap()),
    )
    .unwrap();

    let another_project = PathBuf::from(&root).join("another-project");
    fs::create_dir_all(&another_project).unwrap();
    let error = register_project_at(&another_project, &registry, "0.1.6").unwrap_err();
    assert!(error.starts_with("GLOBAL_REGISTRY_INVALID_EVENT:1:"));
    fs::remove_dir_all(PathBuf::from(&root)).ok();
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

#[test]
fn runtime_registration_is_idempotent_and_conflict_safe() {
    let root = std::env::temp_dir().join(format!(
        "appsdk-runtime-registry-{}-{}",
        std::process::id(),
        Utc::now().timestamp_nanos_opt().unwrap_or_default()
    ));
    let registry = root.join("host");
    let identity = RuntimeIdentity {
        runtime_id: "runtime-a".into(),
        appserver_id: "server-a".into(),
        namespace: "codex_tui".into(),
        endpoint: "unix:///tmp/server-a.sock".into(),
        project_root: "/workspace/app".into(),
        capabilities: vec![],
        process_id: std::process::id(),
    };
    let first = register_runtime_at(&identity, &registry).unwrap();
    let second = register_runtime_at(&identity, &registry).unwrap();
    assert!(!first.idempotent);
    assert!(second.idempotent);
    assert_eq!(
        runtime_at("runtime-a", &registry).unwrap().identity,
        identity
    );

    let mut changed = identity.clone();
    changed.endpoint = "unix:///tmp/forged.sock".into();
    let error = register_runtime_at(&changed, &registry).unwrap_err();
    assert_eq!(error, "GLOBAL_RUNTIME_IDENTITY_CONFLICT:runtime-a");
    assert_eq!(
        fs::read_to_string(registry.join(RUNTIME_FILE))
            .unwrap()
            .lines()
            .count(),
        1
    );
    fs::remove_dir_all(root).ok();
}

#[test]
fn runtime_refresh_preserves_stable_identity_and_replays_latest_volatile_fields() {
    let root = std::env::temp_dir().join(format!(
        "appsdk-runtime-refresh-{}-{}",
        std::process::id(),
        Utc::now().timestamp_nanos_opt().unwrap_or_default()
    ));
    let identity = RuntimeIdentity {
        runtime_id: "runtime-refresh".into(),
        appserver_id: "server-a".into(),
        namespace: "codex_app".into(),
        endpoint: "unix:///tmp/server-a.sock".into(),
        project_root: "/workspace/app".into(),
        capabilities: vec![],
        process_id: std::process::id(),
    };
    register_runtime_at(&identity, &root).unwrap();

    let mut refreshed = identity.clone();
    refreshed.process_id = std::process::id().saturating_add(1);
    let receipt = register_runtime_at(&refreshed, &root).unwrap();
    assert!(!receipt.idempotent);
    assert_eq!(
        runtime_at("runtime-refresh", &root).unwrap().identity,
        refreshed
    );

    let lines = fs::read_to_string(root.join(RUNTIME_FILE)).unwrap();
    assert_eq!(lines.lines().count(), 2);
    assert!(lines.lines().nth(1).unwrap().contains("runtime.refreshed"));
    fs::remove_dir_all(root).ok();
}

#[test]
fn capability_fingerprint_is_unambiguous_and_rejects_reused_digest() {
    let root = std::env::temp_dir().join(format!(
        "appsdk-runtime-capability-collision-{}-{}",
        std::process::id(),
        Utc::now().timestamp_nanos_opt().unwrap_or_default()
    ));
    let first = RuntimeIdentity {
        runtime_id: "runtime-capability-collision".into(),
        appserver_id: "server-a".into(),
        namespace: "codex_app".into(),
        endpoint: "unix:///tmp/server-a.sock".into(),
        project_root: "/workspace/app".into(),
        capabilities: vec!["alpha\0beta".into(), "gamma".into()],
        process_id: std::process::id(),
    };
    let mut second = first.clone();
    second.capabilities = vec!["alpha".into(), "beta\0gamma".into()];

    let ambiguous_bytes = |capabilities: &[String]| {
        let mut bytes = b"capabilities\0".to_vec();
        for capability in capabilities {
            bytes.extend_from_slice(capability.as_bytes());
            bytes.push(0);
        }
        bytes
    };
    assert_eq!(
        ambiguous_bytes(&first.capabilities),
        ambiguous_bytes(&second.capabilities)
    );
    let first_fingerprint = runtime_fingerprint(&first);
    let second_fingerprint = runtime_fingerprint(&second);
    assert_ne!(first_fingerprint, second_fingerprint);

    register_runtime_at(&first, &root).unwrap();
    let path = root.join(RUNTIME_FILE);
    let original = fs::read_to_string(&path).unwrap();
    let mut forged: RuntimeRecord = serde_json::from_str(&original).unwrap();
    forged.event = "runtime.refreshed".into();
    forged.identity = second;
    forged.fingerprint = first_fingerprint;
    fs::write(
        &path,
        format!("{}{}\n", original, serde_json::to_string(&forged).unwrap()),
    )
    .unwrap();

    let error = runtime_at("runtime-capability-collision", &root).unwrap_err();
    assert!(
        error.starts_with("GLOBAL_RUNTIME_REGISTRY_INVALID_EVENT:2:"),
        "{error}"
    );
    fs::remove_dir_all(root).ok();
}

#[test]
fn malformed_runtime_registry_fails_closed() {
    let root = std::env::temp_dir().join(format!(
        "appsdk-runtime-registry-invalid-{}-{}",
        std::process::id(),
        Utc::now().timestamp_nanos_opt().unwrap_or_default()
    ));
    fs::create_dir_all(&root).unwrap();
    let path = root.join(RUNTIME_FILE);
    fs::write(&path, "{}\n").unwrap();
    let identity = RuntimeIdentity {
        runtime_id: "runtime-a".into(),
        appserver_id: "server-a".into(),
        namespace: "codex_tui".into(),
        endpoint: "mock://server-a".into(),
        project_root: "/workspace/app".into(),
        capabilities: vec![],
        process_id: std::process::id(),
    };
    let error = register_runtime_at(&identity, &root).unwrap_err();
    assert!(error.starts_with("GLOBAL_RUNTIME_REGISTRY_INVALID_LINE:1:"));
    fs::remove_dir_all(root).ok();
}

#[test]
fn runtime_registry_reset_is_idempotent_and_creates_missing_baseline() {
    let root = std::env::temp_dir().join(format!(
        "appsdk-runtime-registry-reset-missing-{}-{}",
        std::process::id(),
        Utc::now().timestamp_nanos_opt().unwrap_or_default()
    ));
    let registry = root.join("host");

    let first = reset_runtime_registry_at(&registry, true, "approved reset").unwrap();
    assert!(first.idempotent);
    assert!(first.archive_path.is_none());
    assert!(!first.delivery_verified);
    assert_eq!(fs::read(registry.join(RUNTIME_FILE)).unwrap(), b"");

    let second = reset_runtime_registry_at(&registry, true, "approved reset").unwrap();
    assert!(second.idempotent);
    assert!(second.archive_path.is_none());
    assert!(!second.delivery_verified);
    assert_eq!(fs::read(registry.join(RUNTIME_FILE)).unwrap(), b"");
    fs::remove_dir_all(root).ok();
}

#[test]
fn runtime_registry_reset_archives_legacy_bytes_without_parsing() {
    let root = std::env::temp_dir().join(format!(
        "appsdk-runtime-registry-reset-archive-{}-{}",
        std::process::id(),
        Utc::now().timestamp_nanos_opt().unwrap_or_default()
    ));
    let registry = root.join("host");
    fs::create_dir_all(&registry).unwrap();
    let legacy = b"not-json legacy tmux bytes\n{\"tmuxSession\":\"old\"}\n";
    fs::write(registry.join(RUNTIME_FILE), legacy).unwrap();

    let receipt = reset_runtime_registry_at(&registry, true, "approved reset").unwrap();
    let archive = receipt.archive_path.expect("archive path");
    assert!(!receipt.idempotent);
    assert!(!receipt.delivery_verified);
    assert_eq!(receipt.archived_bytes, legacy.len() as u64);
    assert_eq!(fs::read(registry.join(RUNTIME_FILE)).unwrap(), b"");
    assert_eq!(fs::read(archive.join(RUNTIME_FILE)).unwrap(), legacy);
    assert!(archive.join("runtimes.jsonl.lock.snapshot").is_file());
    let manifest: Value =
        serde_json::from_slice(&fs::read(archive.join("manifest.json")).unwrap()).unwrap();
    assert_eq!(manifest["operation"], "reset_runtime_registry");
    assert_eq!(manifest["delivery_verified"], false);
    assert_eq!(manifest["archived_bytes"], legacy.len());
    assert_eq!(manifest["sha256"].as_str().map(str::len), Some(64));
    fs::remove_dir_all(root).ok();
}

#[test]
fn runtime_registry_reset_resumes_archived_transaction_without_losing_archive_identity() {
    let root = std::env::temp_dir().join(format!(
        "appsdk-runtime-registry-reset-resume-{}-{}",
        std::process::id(),
        Utc::now().timestamp_nanos_opt().unwrap_or_default()
    ));
    let registry = root.join("host");
    fs::create_dir_all(&registry).unwrap();
    let original = b"runtime-original\n";
    fs::write(registry.join(RUNTIME_FILE), original).unwrap();

    let first = reset_runtime_registry_at(&registry, true, "approved reset").unwrap();
    let archive_path = first.archive_path.clone().expect("archive path");
    let transaction_id = first.transaction_id.clone().expect("transaction id");

    let marker_path = registry.join(RUNTIME_RESET_MARKER);
    let mut marker: RuntimeResetTransaction =
        serde_json::from_slice(&fs::read(&marker_path).unwrap()).unwrap();
    marker.phase = "archived".into();
    fs::write(&marker_path, serde_json::to_vec_pretty(&marker).unwrap()).unwrap();
    fs::write(registry.join(RUNTIME_FILE), original).unwrap();

    let resumed = reset_runtime_registry_at(&registry, true, "approved reset").unwrap();
    assert!(resumed.idempotent);
    assert_eq!(resumed.archive_path, Some(archive_path.clone()));
    assert_eq!(resumed.transaction_id, Some(transaction_id.clone()));
    assert_eq!(fs::read(registry.join(RUNTIME_FILE)).unwrap(), b"");

    let retried = reset_runtime_registry_at(&registry, true, "approved reset").unwrap();
    assert!(retried.idempotent);
    assert_eq!(retried.archive_path, Some(archive_path));
    assert_eq!(retried.transaction_id, Some(transaction_id));
    fs::remove_dir_all(root).ok();
}

#[test]
fn runtime_registry_reset_requires_explicit_discard_and_approval() {
    let root = std::env::temp_dir().join(format!(
        "appsdk-runtime-registry-reset-confirm-{}-{}",
        std::process::id(),
        Utc::now().timestamp_nanos_opt().unwrap_or_default()
    ));
    let registry = root.join("host");
    fs::create_dir_all(&registry).unwrap();
    fs::write(registry.join(RUNTIME_FILE), b"legacy\n").unwrap();

    let missing_discard =
        reset_runtime_registry_at(&registry, false, "approved reset").unwrap_err();
    assert_eq!(
        missing_discard,
        "GLOBAL_RUNTIME_REGISTRY_RESET_REQUIRES_DISCARD_LEGACY_CONFIRMATION"
    );
    let missing_approval = reset_runtime_registry_at(&registry, true, "  ").unwrap_err();
    assert_eq!(
        missing_approval,
        "GLOBAL_RUNTIME_REGISTRY_RESET_APPROVAL_REQUIRED"
    );
    assert_eq!(fs::read(registry.join(RUNTIME_FILE)).unwrap(), b"legacy\n");
    fs::remove_dir_all(root).ok();
}
