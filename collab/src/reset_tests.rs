use super::*;

fn temp_root(name: &str) -> PathBuf {
    PathBuf::from(format!(
        "/tmp/cr-{name}-{}-{}",
        std::process::id(),
        now_ms()
    ))
}

#[test]
fn tree_digest_is_stable_and_content_sensitive() {
    let root = temp_root("digest");
    std::fs::create_dir_all(root.join("nested")).unwrap();
    std::fs::write(root.join("a.jsonl"), b"one\n").unwrap();
    std::fs::write(root.join("nested/b"), b"two\n").unwrap();
    let first = tree_digest(&root).unwrap();
    let second = tree_digest(&root).unwrap();
    assert_eq!(first, second);
    std::fs::write(root.join("nested/b"), b"changed\n").unwrap();
    assert_ne!(first, tree_digest(&root).unwrap());
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn tree_digest_ignores_stale_unix_socket_bytes() {
    let root = temp_root("digest-socket");
    std::fs::create_dir_all(root.join("server")).unwrap();
    std::fs::write(root.join("server/journal.jsonl"), b"one\n").unwrap();
    let socket = root.join("server/server.sock");
    let listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();
    drop(listener);

    let (files, sockets, bytes, _) = tree_digest(&root).unwrap();
    assert_eq!(files, 1);
    assert_eq!(sockets, vec![String::from("server/server.sock")]);
    assert_eq!(bytes, 4);

    std::fs::remove_dir_all(root).ok();
}

#[test]
fn source_verification_requires_the_inspected_socket_inventory() {
    let entry = RetiredRoot {
        relative: ".agent-collab".into(),
        absolute: PathBuf::from("/tmp/source"),
        staged: None,
        files: 1,
        sockets: vec!["server/server.sock".into()],
        bytes: 4,
        digest: "fnv1a64:test".into(),
    };

    assert!(source_matches(&entry, &["server/server.sock".into()]));
    assert!(!source_matches(&entry, &[]));
    assert!(archive_matches(&entry, 1, 4, "fnv1a64:test"));
}

#[test]
fn tree_digest_rejects_nested_symlinks() {
    let root = temp_root("digest-symlink");
    let target = temp_root("digest-symlink-target");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(&target, b"external\n").unwrap();
    std::os::unix::fs::symlink(&target, root.join("journal.jsonl")).unwrap();

    let error = tree_digest(&root).unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
    assert!(
        error
            .to_string()
            .contains("RESET_CONTROL_TREE_SYMLINK_REJECTED"),
        "{error}"
    );

    std::fs::remove_dir_all(root).ok();
    std::fs::remove_file(target).ok();
}

#[test]
fn retire_host_routes_preserves_unrelated_records() {
    let state = temp_root("routes-state");
    let project = temp_root("routes-project");
    let other = temp_root("routes-other");
    for path in [&state, &project, &other] {
        std::fs::create_dir_all(path).unwrap();
    }
    let host = HostPaths::from_state_root(&state).unwrap();
    let retired = json!({
        "version": 1,
        "op": "register",
        "app_scope_id": "appserver-cli",
        "project_scope": project.canonicalize().unwrap().to_string_lossy(),
        "canonical_root": project.canonicalize().unwrap().to_string_lossy(),
        "storage_root": project.canonicalize().unwrap().to_string_lossy(),
        "registered_ms": 1,
    });
    let kept = json!({
        "version": 1,
        "op": "register",
        "app_scope_id": "appserver-cli",
        "project_scope": other.canonicalize().unwrap().to_string_lossy(),
        "canonical_root": other.canonicalize().unwrap().to_string_lossy(),
        "storage_root": other.canonicalize().unwrap().to_string_lossy(),
        "registered_ms": 2,
    });
    std::fs::write(state.join("routes.jsonl"), format!("{retired}\n{kept}\n")).unwrap();

    assert_eq!(retire_host_routes(&host, &project).unwrap(), 1);
    let remaining = std::fs::read_to_string(state.join("routes.jsonl")).unwrap();
    assert!(remaining.contains(&other.canonicalize().unwrap().to_string_lossy().to_string()));
    assert!(!remaining.contains(
        &project
            .canonicalize()
            .unwrap()
            .to_string_lossy()
            .to_string()
    ));

    // Idempotent: a second pass removes nothing and leaves the table intact.
    assert_eq!(retire_host_routes(&host, &project).unwrap(), 0);
    assert_eq!(
        std::fs::read_to_string(state.join("routes.jsonl")).unwrap(),
        remaining
    );

    for path in [&state, &project, &other] {
        std::fs::remove_dir_all(path).ok();
    }
}

#[test]
fn reset_requires_explicit_authorization() {
    let root = temp_root("auth");
    std::fs::create_dir_all(&root).unwrap();
    scope::init(&root).unwrap();
    let state = temp_root("auth-state");
    std::fs::create_dir_all(&state).unwrap();
    let host = HostPaths::from_state_root(&state).unwrap();
    let scope = Scope { root: root.clone() };

    let missing_flag = run(
        &scope,
        &host,
        ResetRequest {
            approval: "user text".into(),
            discard_legacy: false,
            ..ResetRequest::default()
        },
    );
    assert!(missing_flag
        .unwrap_err()
        .to_string()
        .contains("RESET_AUTHORIZATION_REQUIRED"));

    let missing_approval = run(
        &scope,
        &host,
        ResetRequest {
            approval: "  ".into(),
            discard_legacy: true,
            ..ResetRequest::default()
        },
    );
    assert!(missing_approval
        .unwrap_err()
        .to_string()
        .contains("RESET_AUTHORIZATION_REQUIRED"));

    std::fs::remove_dir_all(root).ok();
    std::fs::remove_dir_all(state).ok();
}

#[test]
fn reset_retires_nonempty_runtime_files_in_current_scaffold() {
    let root = temp_root("nonempty-runtime");
    scope::init(&root).unwrap();
    std::fs::write(root.join(".agent-collab/server/events.jsonl"), b"{}\n").unwrap();
    std::fs::write(root.join(".agent-collab/server/log.txt"), b"old log\n").unwrap();

    let state = temp_root("nonempty-runtime-state");
    std::fs::create_dir_all(&state).unwrap();
    let host = HostPaths::from_state_root(&state).unwrap();
    let scope = Scope { root: root.clone() };

    run(
        &scope,
        &host,
        ResetRequest {
            approval: "operator authorized legacy retirement".into(),
            discard_legacy: true,
            ..ResetRequest::default()
        },
    )
    .unwrap();

    assert!(!root.join(".agent-collab/server/events.jsonl").exists());
    assert!(!root.join(".agent-collab/server/log.txt").exists());
    let record = read_last_reset_record(&state);
    assert_eq!(record["already_reset"], json!(false));
    assert_eq!(record["archive_durable"], json!(true));
    assert_eq!(record["delivery_verified"], json!(false));

    std::fs::remove_dir_all(root).ok();
    std::fs::remove_dir_all(state).ok();
}

#[test]
fn reset_does_not_mutate_unrelated_project_or_global_configuration() {
    let root = temp_root("unrelated-config");
    std::fs::create_dir_all(root.join(".agent-collab/server")).unwrap();
    std::fs::write(
        root.join(".agent-collab/server/journal.jsonl"),
        b"{\"ev\":\"NotificationSubscribed\",\"subscription\":{}}\n",
    )
    .unwrap();
    std::fs::write(root.join(".mcp.json"), b"{\"keep\":true}\n").unwrap();
    std::fs::create_dir_all(root.join(".codex")).unwrap();
    std::fs::write(root.join(".codex/config.toml"), b"model = \"keep\"\n").unwrap();
    std::fs::create_dir_all(root.join(".claude")).unwrap();
    std::fs::write(root.join(".claude/settings.json"), b"{\"keep\":true}\n").unwrap();

    let state = temp_root("unrelated-config-state");
    std::fs::create_dir_all(&state).unwrap();
    let host = HostPaths::from_state_root(&state).unwrap();
    let scope = Scope { root: root.clone() };

    run(
        &scope,
        &host,
        ResetRequest {
            approval: "operator authorized legacy retirement".into(),
            discard_legacy: true,
            ..ResetRequest::default()
        },
    )
    .unwrap();

    assert_eq!(
        std::fs::read(root.join(".mcp.json")).unwrap(),
        b"{\"keep\":true}\n"
    );
    assert_eq!(
        std::fs::read(root.join(".codex/config.toml")).unwrap(),
        b"model = \"keep\"\n"
    );
    assert_eq!(
        std::fs::read(root.join(".claude/settings.json")).unwrap(),
        b"{\"keep\":true}\n"
    );

    std::fs::remove_dir_all(root).ok();
    std::fs::remove_dir_all(state).ok();
}

#[test]
fn reset_does_not_treat_nonempty_project_state_as_empty_baseline() {
    let root = temp_root("nonempty-baseline");
    scope::init(&root).unwrap();
    std::fs::write(root.join(".agent-collab/runs/stale.jsonl"), b"{}\n").unwrap();
    std::fs::create_dir_all(root.join(".agent-collab/server/runtimes/old")).unwrap();
    std::fs::write(
        root.join(".agent-collab/server/runtimes/old/journal.jsonl"),
        b"legacy\n",
    )
    .unwrap();

    let state = temp_root("nonempty-baseline-state");
    std::fs::create_dir_all(&state).unwrap();
    let host = HostPaths::from_state_root(&state).unwrap();
    let scope = Scope { root: root.clone() };

    run(
        &scope,
        &host,
        ResetRequest {
            approval: "operator authorized legacy retirement".into(),
            discard_legacy: true,
            ..ResetRequest::default()
        },
    )
    .unwrap();

    assert!(!root.join(".agent-collab/runs/stale.jsonl").exists());
    assert!(!root.join(".agent-collab/server/runtimes").exists());
    let record = read_last_reset_record(&state);
    assert_eq!(record["already_reset"], json!(false));
    assert_eq!(record["delivery_verified"], json!(false));

    std::fs::remove_dir_all(root).ok();
    std::fs::remove_dir_all(state).ok();
}

#[test]
fn reset_rejects_symlinked_guidance_without_mutating_the_target() {
    let root = temp_root("symlink-guidance");
    let target = temp_root("symlink-guidance-target");
    std::fs::create_dir_all(root.join("docs")).unwrap();
    std::fs::write(&target, b"external guidance\n").unwrap();
    std::os::unix::fs::symlink(&target, root.join("docs/collab.md")).unwrap();

    let state = temp_root("symlink-guidance-state");
    std::fs::create_dir_all(&state).unwrap();
    let host = HostPaths::from_state_root(&state).unwrap();
    let scope = Scope { root: root.clone() };

    let error = run(
        &scope,
        &host,
        ResetRequest {
            approval: "operator authorized current baseline repair".into(),
            discard_legacy: true,
            ..ResetRequest::default()
        },
    )
    .unwrap_err();
    assert!(
        error.to_string().contains("RESET_DOC_SYMLINK_REJECTED"),
        "{error:#}"
    );
    assert_eq!(std::fs::read(&target).unwrap(), b"external guidance\n");
    assert!(!state.join("reset.jsonl").exists());

    std::fs::remove_dir_all(root).ok();
    std::fs::remove_dir_all(state).ok();
    std::fs::remove_file(target).ok();
}

#[test]
fn reset_rejects_symlinked_control_root_without_mutating_the_target() {
    let root = temp_root("symlink-control");
    let target = temp_root("symlink-control-target");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::create_dir_all(&target).unwrap();
    std::fs::write(target.join("journal.jsonl"), b"external\n").unwrap();
    std::os::unix::fs::symlink(&target, root.join(".agent-collab")).unwrap();

    let state = temp_root("symlink-control-state");
    std::fs::create_dir_all(&state).unwrap();
    let host = HostPaths::from_state_root(&state).unwrap();
    let scope = Scope { root: root.clone() };

    let error = run(
        &scope,
        &host,
        ResetRequest {
            approval: "operator authorized current baseline repair".into(),
            discard_legacy: true,
            ..ResetRequest::default()
        },
    )
    .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("RESET_CONTROL_ROOT_SYMLINK_REJECTED"),
        "{error:#}"
    );
    assert_eq!(
        std::fs::read(target.join("journal.jsonl")).unwrap(),
        b"external\n"
    );
    assert!(!state.join("archives").exists());
    assert!(!state.join("reset.jsonl").exists());

    std::fs::remove_dir_all(root).ok();
    std::fs::remove_dir_all(state).ok();
    std::fs::remove_dir_all(target).ok();
}

#[test]
fn reset_rejects_symlink_inside_control_root_without_archiving() {
    let root = temp_root("symlink-control-entry");
    let target = temp_root("symlink-control-entry-target");
    std::fs::create_dir_all(root.join(".agent-collab/server")).unwrap();
    std::fs::write(&target, b"external\n").unwrap();
    std::os::unix::fs::symlink(&target, root.join(".agent-collab/server/journal.jsonl"))
        .unwrap();

    let state = temp_root("symlink-control-entry-state");
    std::fs::create_dir_all(&state).unwrap();
    let host = HostPaths::from_state_root(&state).unwrap();
    let scope = Scope { root: root.clone() };

    let error = run(
        &scope,
        &host,
        ResetRequest {
            approval: "operator authorized current baseline repair".into(),
            discard_legacy: true,
            ..ResetRequest::default()
        },
    )
    .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("RESET_CONTROL_TREE_SYMLINK_REJECTED"),
        "{error:#}"
    );
    assert_eq!(std::fs::read(&target).unwrap(), b"external\n");
    assert!(root.join(".agent-collab/server/journal.jsonl").is_symlink());
    assert!(!state.join("archives").exists());
    assert!(!state.join("reset.jsonl").exists());

    std::fs::remove_dir_all(root).ok();
    std::fs::remove_dir_all(state).ok();
    std::fs::remove_file(target).ok();
}

#[test]
fn reset_rolls_back_legacy_roots_when_route_rewrite_fails() {
    let root = temp_root("rollback-routes");
    std::fs::create_dir_all(root.join(".agent-collab/server")).unwrap();
    let legacy = b"{\"ev\":\"NotificationSubscribed\",\"subscription\":{}}\n";
    std::fs::write(root.join(".agent-collab/server/journal.jsonl"), legacy).unwrap();

    let state = temp_root("rollback-routes-state");
    std::fs::create_dir_all(&state).unwrap();
    let routes = state.join("routes.jsonl");
    std::fs::write(&routes, b"{not-json}\n").unwrap();
    let host = HostPaths::from_state_root(&state).unwrap();
    let scope = Scope { root: root.clone() };

    let error = run(
        &scope,
        &host,
        ResetRequest {
            approval: "operator authorized legacy retirement".into(),
            discard_legacy: true,
            ..ResetRequest::default()
        },
    )
    .unwrap_err();
    assert!(
        error.to_string().contains("reset rolled back")
            || error.to_string().contains("DAEMON_MIGRATION_REQUIRED"),
        "{error:#}"
    );
    assert_eq!(
        std::fs::read(root.join(".agent-collab/server/journal.jsonl")).unwrap(),
        legacy
    );
    assert_eq!(std::fs::read(&routes).unwrap(), b"{not-json}\n");
    assert!(!state.join("reset.jsonl").exists());
    assert!(root.join(".agent-collab").is_dir());

    std::fs::remove_dir_all(root).ok();
    std::fs::remove_dir_all(state).ok();
}

#[test]
fn reset_rolls_back_legacy_roots_when_reset_record_write_fails() {
    use std::os::unix::fs::PermissionsExt;

    let root = temp_root("rollback-record");
    std::fs::create_dir_all(root.join(".agent-collab/server")).unwrap();
    let legacy = b"{\"ev\":\"NotificationSubscribed\",\"subscription\":{}}\n";
    std::fs::write(root.join(".agent-collab/server/journal.jsonl"), legacy).unwrap();
    std::fs::create_dir_all(root.join("docs")).unwrap();
    let original_doc = b"# Project-owned Collab notes\n";
    std::fs::write(root.join("docs/collab.md"), original_doc).unwrap();

    let state = temp_root("rollback-record-state");
    std::fs::create_dir_all(&state).unwrap();
    let reset_record = state.join("reset.jsonl");
    let original_record = b"{\"schema\":\"collab-reset/v1\",\"existing\":true}\n";
    std::fs::write(&reset_record, original_record).unwrap();
    let mut permissions = std::fs::metadata(&reset_record).unwrap().permissions();
    permissions.set_mode(0o444);
    std::fs::set_permissions(&reset_record, permissions).unwrap();

    let host = HostPaths::from_state_root(&state).unwrap();
    let scope = Scope { root: root.clone() };

    let error = run(
        &scope,
        &host,
        ResetRequest {
            approval: "operator authorized legacy retirement".into(),
            discard_legacy: true,
            ..ResetRequest::default()
        },
    )
    .unwrap_err();
    assert!(
        error.to_string().contains("reset rolled back")
            || error.to_string().contains("DAEMON_MIGRATION_REQUIRED"),
        "{error:#}"
    );
    assert_eq!(
        std::fs::read(root.join(".agent-collab/server/journal.jsonl")).unwrap(),
        legacy
    );
    assert_eq!(
        std::fs::read(root.join("docs/collab.md")).unwrap(),
        original_doc
    );
    assert!(root.join(".agent-collab").is_dir());
    assert!(root
        .read_dir()
        .unwrap()
        .flatten()
        .all(|entry| !entry.file_name().to_string_lossy().contains("reset-stage")));
    assert_eq!(std::fs::read(&reset_record).unwrap(), original_record);

    std::fs::remove_dir_all(root).ok();
    std::fs::remove_dir_all(state).ok();
}

fn read_last_reset_record(state: &Path) -> serde_json::Value {
    let text = std::fs::read_to_string(state.join("reset.jsonl")).unwrap();
    let last = text.lines().last().unwrap();
    serde_json::from_str(last).unwrap()
}

#[test]
fn reset_rejects_a_held_legacy_project_writer_lock_before_archive() {
    use std::os::unix::io::AsRawFd;

    let root = temp_root("legacy-lock");
    std::fs::create_dir_all(root.join(".agent-collab/server")).unwrap();
    let legacy = b"{\"ev\":\"NotificationSubscribed\",\"subscription\":{}}\n";
    std::fs::write(root.join(".agent-collab/server/journal.jsonl"), legacy).unwrap();
    let lock_path = root.join(".agent-collab/server/daemon.lock");
    let lock_file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .open(&lock_path)
        .unwrap();
    let rc = unsafe { libc::flock(lock_file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    assert_eq!(rc, 0);

    let state = temp_root("legacy-lock-state");
    std::fs::create_dir_all(&state).unwrap();
    let host = HostPaths::from_state_root(&state).unwrap();
    let scope = Scope { root: root.clone() };

    let error = run(
        &scope,
        &host,
        ResetRequest {
            approval: "operator authorized legacy retirement".into(),
            discard_legacy: true,
            ..ResetRequest::default()
        },
    )
    .unwrap_err();
    assert!(error.to_string().contains("DAEMON_MIGRATION_REQUIRED"));
    assert_eq!(
        std::fs::read(root.join(".agent-collab/server/journal.jsonl")).unwrap(),
        legacy
    );
    assert!(!state.join("archives").exists());
    assert!(!state.join("reset.jsonl").exists());

    drop(lock_file);
    std::fs::remove_dir_all(root).ok();
    std::fs::remove_dir_all(state).ok();
}

#[test]
fn reset_rebuilds_an_uninitialized_project_root() {
    let root = temp_root("uninitialized");
    std::fs::create_dir_all(&root).unwrap();
    let state = temp_root("uninitialized-state");
    std::fs::create_dir_all(&state).unwrap();
    let host = HostPaths::from_state_root(&state).unwrap();
    let scope = Scope { root: root.clone() };

    run(
        &scope,
        &host,
        ResetRequest {
            approval: "operator authorized current baseline repair".into(),
            discard_legacy: true,
            ..ResetRequest::default()
        },
    )
    .unwrap();

    assert!(root.join(".agent-collab/server").is_dir());
    assert!(!root.join(".agent-collab/server/journal.jsonl").exists());
    let record = read_last_reset_record(&state);
    assert_eq!(record["already_reset"], json!(true));
    assert_eq!(record["retired"], json!([]));
    assert_eq!(record["delivery_verified"], json!(false));

    std::fs::remove_dir_all(root).ok();
    std::fs::remove_dir_all(state).ok();
}

#[test]
fn prune_stale_host_routes_removes_only_provably_dead_roots() {
    let state = temp_root("prune-state");
    let live = temp_root("prune-live");
    let uninitialized = temp_root("prune-uninitialized");
    let missing = temp_root("prune-missing");
    for path in [&state, &live, &uninitialized] {
        std::fs::create_dir_all(path).unwrap();
    }
    std::fs::create_dir_all(live.join(".agent-collab")).unwrap();
    let host = HostPaths::from_state_root(&state).unwrap();

    let record = |root: &Path| {
        json!({
            "version": 1,
            "op": "register",
            "app_scope_id": "appserver-cli",
            "project_scope": root.canonicalize().unwrap().to_string_lossy(),
            "canonical_root": root.canonicalize().unwrap().to_string_lossy(),
            "storage_root": root.canonicalize().unwrap().to_string_lossy(),
            "registered_ms": 1,
        })
    };
    let missing_record = json!({
        "version": 1,
        "op": "register",
        "app_scope_id": "appserver-cli",
        "project_scope": missing.to_string_lossy(),
        "canonical_root": missing.to_string_lossy(),
        "storage_root": missing.to_string_lossy(),
        "registered_ms": 1,
    });
    std::fs::write(
        state.join("routes.jsonl"),
        format!(
            "{}\n{}\n{}\n",
            record(&live),
            record(&uninitialized),
            missing_record
        ),
    )
    .unwrap();

    assert_eq!(prune_stale_host_routes(&host).unwrap(), 2);
    let remaining = std::fs::read_to_string(state.join("routes.jsonl")).unwrap();
    assert!(remaining.contains(&live.canonicalize().unwrap().to_string_lossy().to_string()));
    assert!(!remaining.contains(
        &uninitialized
            .canonicalize()
            .unwrap()
            .to_string_lossy()
            .to_string()
    ));

    for path in [&state, &live, &uninitialized] {
        std::fs::remove_dir_all(path).ok();
    }
}

#[test]
fn reset_retires_legacy_control_plane_and_rebuilds_baseline() {
    let root = temp_root("retire");
    std::fs::create_dir_all(root.join(".agent-collab/server")).unwrap();
    std::fs::write(
        root.join(".agent-collab/server/journal.jsonl"),
        b"{\"ev\":\"NotificationSubscribed\",\"subscription\":{}}\n",
    )
    .unwrap();

    let state = temp_root("retire-state");
    std::fs::create_dir_all(&state).unwrap();
    let host = HostPaths::from_state_root(&state).unwrap();
    let scope = Scope { root: root.clone() };

    run(
        &scope,
        &host,
        ResetRequest {
            approval: "operator authorized legacy retirement".into(),
            discard_legacy: true,
            ..ResetRequest::default()
        },
    )
    .unwrap();

    // Current empty baseline exists and the legacy journal is gone.
    assert!(root.join(".agent-collab/server").is_dir());
    assert!(!root.join(".agent-collab/server/journal.jsonl").exists());

    // The audit record is explicit that this proves reset only.
    let record = read_last_reset_record(&state);
    assert_eq!(record["delivery_verified"], json!(false));
    assert_eq!(record["already_reset"], json!(false));
    assert_eq!(record["archive_durable"], json!(true));
    assert_eq!(record["schema"], json!("collab-reset/v1"));
    assert!(record["archive_root"].is_string());

    // Second run is idempotent and does not claim delivery.
    run(
        &scope,
        &host,
        ResetRequest {
            approval: "operator authorized legacy retirement".into(),
            discard_legacy: true,
            ..ResetRequest::default()
        },
    )
    .unwrap();
    let record = read_last_reset_record(&state);
    assert_eq!(record["already_reset"], json!(true));
    assert_eq!(record["archive_durable"], json!(false));
    assert_eq!(record["delivery_verified"], json!(false));
    assert!(record["archive_root"].is_null());

    // Non-empty runtime artifacts are historical state too. They must be
    // archived and cleared rather than being accepted as a current empty
    // baseline.
    std::fs::write(root.join(".agent-collab/server/journal.jsonl"), b"").unwrap();
    std::fs::write(root.join(".agent-collab/server/events.jsonl"), b"{}\n").unwrap();
    std::fs::write(root.join(".agent-collab/server/log.txt"), b"started\n").unwrap();
    run(
        &scope,
        &host,
        ResetRequest {
            approval: "operator authorized legacy retirement".into(),
            discard_legacy: true,
            ..ResetRequest::default()
        },
    )
    .unwrap();
    let record = read_last_reset_record(&state);
    assert_eq!(record["already_reset"], json!(false));
    assert_eq!(record["archive_durable"], json!(true));
    assert!(!record["retired"].as_array().unwrap().is_empty());
    assert!(!root.join(".agent-collab/server/events.jsonl").exists());
    assert!(!root.join(".agent-collab/server/log.txt").exists());
    assert_eq!(record["delivery_verified"], json!(false));

    std::fs::remove_dir_all(root).ok();
    std::fs::remove_dir_all(state).ok();
}

#[test]
fn reset_retires_stale_project_socket_without_copying_socket_state() {
    use std::os::unix::net::UnixListener;

    let root = temp_root("retire-socket");
    std::fs::create_dir_all(root.join(".agent-collab/server")).unwrap();
    std::fs::write(
        root.join(".agent-collab/server/journal.jsonl"),
        b"{\"ev\":\"Sent\",\"msg\":{}}\n",
    )
    .unwrap();
    let legacy_socket = root.join(".agent-collab/server/server.sock");
    let listener = UnixListener::bind(&legacy_socket).unwrap();
    drop(listener);

    let state = temp_root("retire-socket-state");
    std::fs::create_dir_all(&state).unwrap();
    let host = HostPaths::from_state_root(&state).unwrap();
    let scope = Scope { root: root.clone() };

    run(
        &scope,
        &host,
        ResetRequest {
            approval: "operator authorized legacy retirement".into(),
            discard_legacy: true,
            ..ResetRequest::default()
        },
    )
    .unwrap();

    assert!(!legacy_socket.exists());
    assert!(!root.join(".agent-collab/server/journal.jsonl").exists());
    let archive_entries = std::fs::read_dir(state.join("archives")).unwrap().count();
    assert!(archive_entries > 0);
    let manifest_path = std::fs::read_dir(state.join("archives"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path()
        .join("manifest.json");
    let manifest: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(manifest_path).unwrap()).unwrap();
    assert_eq!(
        manifest["retired"][0]["sockets"][0],
        json!("server/server.sock")
    );

    std::fs::remove_dir_all(root).ok();
    std::fs::remove_dir_all(state).ok();
}

#[test]
fn reset_level_requires_exactly_one_selector() {
    let error = ResetLevel::select(false, false, false)
        .expect_err("no level must fail before any file is read")
        .to_string();
    assert!(error.contains("RESET_LEVEL_REQUIRED"), "{error}");
    assert!(ResetLevel::select(true, true, false).is_err());
    assert!(ResetLevel::select(true, false, true).is_err());
    assert!(ResetLevel::select(false, true, true).is_err());
    assert_eq!(
        ResetLevel::select(true, false, false).unwrap(),
        ResetLevel::Routes
    );
    assert_eq!(
        ResetLevel::select(false, true, false).unwrap(),
        ResetLevel::Project
    );
    assert_eq!(
        ResetLevel::select(false, false, true).unwrap(),
        ResetLevel::Host
    );
}

#[test]
fn reset_level_flags_are_gated_per_level() {
    let mut request = ResetRequest {
        approval: "operator authorized the reset".into(),
        discard_legacy: true,
        ..ResetRequest::default()
    };
    // The project level needs no storage root and rejects one.
    assert!(request.validate_level_flags().is_ok());
    request.storage_root = Some(PathBuf::from("/tmp/not-used"));
    let error = request
        .validate_level_flags()
        .expect_err("--project must reject --storage-root")
        .to_string();
    assert!(error.contains("RESET_LEVEL_FLAG_MISMATCH"), "{error}");

    // The routes level requires the storage root and rejects --include-runs.
    request.storage_root = None;
    request.level = ResetLevel::Routes;
    let error = request
        .validate_level_flags()
        .expect_err("--routes must require --storage-root")
        .to_string();
    assert!(error.contains("RESET_STORAGE_ROOT_REQUIRED"), "{error}");
    request.storage_root = Some(PathBuf::from("/tmp/not-used"));
    request.include_runs = true;
    let error = request
        .validate_level_flags()
        .expect_err("--routes must reject --include-runs")
        .to_string();
    assert!(error.contains("RESET_LEVEL_FLAG_MISMATCH"), "{error}");

    // The host level requires the storage root and rejects --keep.
    request.include_runs = false;
    request.level = ResetLevel::Host;
    assert!(request.validate_level_flags().is_ok());
    request.keep = vec!["binding-not-used".into()];
    let error = request
        .validate_level_flags()
        .expect_err("--host must reject --keep")
        .to_string();
    assert!(error.contains("RESET_LEVEL_FLAG_MISMATCH"), "{error}");
}

fn route_binding(
    scope: &crate::server::global_state::RouteScope,
    agent: &str,
    session: &str,
    thread: &str,
    pane: &str,
) -> crate::server::RuntimeBinding {
    let mut binding = crate::server::RuntimeBinding::new_with_session(
        scope.project_scope_id.clone(),
        scope.app_scope_id.clone(),
        crate::identity::AgentId::new(agent).unwrap(),
        crate::identity::RuntimeId::new(format!("runtime-{agent}")).unwrap(),
        crate::identity::BindingId::new(format!("binding-{agent}")).unwrap(),
        1,
        Some(crate::identity::SessionId::new(session).unwrap()),
        Some(crate::identity::NativeThreadId::new(thread).unwrap()),
    )
    .unwrap();
    binding.tmux_endpoint = Some(crate::proto::TmuxEndpoint {
        socket_path: "/tmp/collab-reset-routes/t".to_owned(),
        server_pid: 1,
        tmux_session_id: "$7".to_owned(),
        pane_id: pane.to_owned(),
        pane_pid: 1,
        codex_session_id: Some(session.to_owned()),
        codex_thread_id: Some(thread.to_owned()),
    });
    binding
}

/// One L1 run must clear every ambiguous pane it is authorized for, and the
/// retirement must be durable across a fresh replay of the same journal.
#[test]
fn reset_routes_retires_each_ambiguous_pane_and_keeps_the_named_survivor() {
    let root = temp_root("routes-multi");
    let storage = root.join("storage");
    let server_dir = storage.join(".agent-collab/server");
    std::fs::create_dir_all(&server_dir).unwrap();
    let journal_path = server_dir.join("journal.jsonl");
    std::fs::File::create(&journal_path).unwrap();

    let scope = crate::server::global_state::RouteScope {
        app_scope_id: crate::identity::AppServerId::new(crate::identity::CLI_APP_SERVER_ID)
            .unwrap(),
        project_scope_id: crate::server::GlobalState::canonical_project_scope(&storage).unwrap(),
    };
    let keep_a = route_binding(&scope, "keep-a", "session-keep-a", "thread-keep-a", "%70");
    let stale_a = route_binding(&scope, "stale-a", "session-stale-a", "thread-stale-a", "%70");
    let keep_b = route_binding(&scope, "keep-b", "session-keep-b", "thread-keep-b", "%71");
    let stale_b = route_binding(&scope, "stale-b", "session-stale-b", "thread-stale-b", "%71");
    let mut lines = String::new();
    for binding in [&keep_a, &stale_a, &keep_b, &stale_b] {
        lines.push_str(
            &serde_json::to_string(&crate::server::state::Event::GlobalCurrentThreadRouteSet {
                binding: binding.clone(),
            })
            .unwrap(),
        );
        lines.push('\n');
    }
    std::fs::write(&journal_path, lines).unwrap();

    let host_paths = HostPaths::for_state_root(root.join("host-state")).unwrap();
    std::fs::create_dir_all(host_paths.state_root()).unwrap();
    run(
        &Scope { root: root.clone() },
        &host_paths,
        ResetRequest {
            approval: "operator authorized the pane cleanup".into(),
            discard_legacy: true,
            level: ResetLevel::Routes,
            storage_root: Some(storage.clone()),
            keep: vec!["binding-keep-a".into(), "binding-keep-b".into()],
            include_runs: false,
        },
    )
    .unwrap();

    let after = crate::server::replay_host_index(&storage).unwrap();
    for (kept, pane) in [(&keep_a, "%70"), (&keep_b, "%71")] {
        let endpoint = kept.tmux_endpoint.as_ref().unwrap();
        let claimants = after
            .global
            .tmux_pane_route_claimants_in_scope(&scope, endpoint);
        assert_eq!(
            claimants.len(),
            1,
            "pane {pane} must keep exactly one claimant: {claimants:?}"
        );
        assert_eq!(claimants[0].binding_id, kept.binding_id);
    }
    assert!(
        after.global.lookup_retired_route_claim(&stale_a).is_some(),
        "the retirement must survive a fresh replay"
    );
    assert!(after.global.lookup_retired_route_claim(&stale_b).is_some());
    assert!(after.global.lookup_retired_route_claim(&keep_a).is_none());
    assert!(after.global.lookup_retired_route_claim(&keep_b).is_none());

    // The audit record names the authorization, the run, and both survivors.
    let reset_log = std::fs::read_to_string(host_paths.state_root().join("reset.jsonl")).unwrap();
    let record: serde_json::Value = serde_json::from_str(reset_log.lines().last().unwrap()).unwrap();
    assert_eq!(record["level"], json!("routes"));
    assert_eq!(
        record["approval"],
        json!("operator authorized the pane cleanup")
    );
    assert_eq!(
        record["kept_binding_ids"],
        json!(["binding-keep-a", "binding-keep-b"])
    );
    assert_eq!(
        record["retired_claims"]
            .as_array()
            .unwrap()
            .iter()
            .map(|claim| claim["binding_id"].clone())
            .collect::<Vec<_>>(),
        vec![json!("binding-stale-a"), json!("binding-stale-b")]
    );

    std::fs::remove_dir_all(root).ok();
}

/// An ambiguous pane whose survivor is not named changes nothing.
#[test]
fn reset_routes_refuses_an_unnamed_pane_without_touching_the_journal() {
    let root = temp_root("routes-conflict");
    let storage = root.join("storage");
    let server_dir = storage.join(".agent-collab/server");
    std::fs::create_dir_all(&server_dir).unwrap();
    let journal_path = server_dir.join("journal.jsonl");
    std::fs::File::create(&journal_path).unwrap();

    let scope = crate::server::global_state::RouteScope {
        app_scope_id: crate::identity::AppServerId::new(crate::identity::CLI_APP_SERVER_ID)
            .unwrap(),
        project_scope_id: crate::server::GlobalState::canonical_project_scope(&storage).unwrap(),
    };
    let keep_a = route_binding(&scope, "conflict-a", "session-conflict-a", "thread-conflict-a", "%72");
    let stale_a = route_binding(&scope, "conflict-b", "session-conflict-b", "thread-conflict-b", "%72");
    let keep_b = route_binding(&scope, "conflict-c", "session-conflict-c", "thread-conflict-c", "%73");
    let stale_b = route_binding(&scope, "conflict-d", "session-conflict-d", "thread-conflict-d", "%73");
    let mut lines = String::new();
    for binding in [&keep_a, &stale_a, &keep_b, &stale_b] {
        lines.push_str(
            &serde_json::to_string(&crate::server::state::Event::GlobalCurrentThreadRouteSet {
                binding: binding.clone(),
            })
            .unwrap(),
        );
        lines.push('\n');
    }
    std::fs::write(&journal_path, lines.clone()).unwrap();
    let before = std::fs::read(&journal_path).unwrap();

    let host_paths = HostPaths::for_state_root(root.join("host-state")).unwrap();
    std::fs::create_dir_all(host_paths.state_root()).unwrap();
    // `binding-conflict-a` is the survivor of `%72` only; `%73` stays unnamed.
    let error = run(
        &Scope { root: root.clone() },
        &host_paths,
        ResetRequest {
            approval: "operator authorized the pane cleanup".into(),
            discard_legacy: true,
            level: ResetLevel::Routes,
            storage_root: Some(storage.clone()),
            keep: vec!["binding-conflict-a".into()],
            include_runs: false,
        },
    )
    .expect_err("an unnamed ambiguous pane must stop the run")
    .to_string();
    assert!(error.contains("RESET_KEEP_REQUIRED"), "{error}");
    assert!(error.contains("%73"), "the error must name the pane: {error}");
    assert!(
        error.contains("binding-conflict-c") && error.contains("binding-conflict-d"),
        "the error must list the claimants: {error}"
    );
    assert_eq!(
        std::fs::read(&journal_path).unwrap(),
        before,
        "a refused run must not write the journal"
    );

    std::fs::remove_dir_all(root).ok();
}

/// The post-commit check must reject a journal that does not carry the
/// retirement, and must accept one that does.
#[test]
fn verify_retirement_requires_the_retirement_and_a_live_survivor() {
    let root = temp_root("verify");
    let storage = root.join("storage");
    let server_dir = storage.join(".agent-collab/server");
    std::fs::create_dir_all(&server_dir).unwrap();
    let journal_path = server_dir.join("journal.jsonl");
    std::fs::File::create(&journal_path).unwrap();

    let scope = crate::server::global_state::RouteScope {
        app_scope_id: crate::identity::AppServerId::new(crate::identity::CLI_APP_SERVER_ID)
            .unwrap(),
        project_scope_id: crate::server::GlobalState::canonical_project_scope(&storage).unwrap(),
    };
    let kept = route_binding(
        &scope,
        "verify-kept",
        "session-verify-kept",
        "thread-verify-kept",
        "%74",
    );
    let stale = route_binding(
        &scope,
        "verify-stale",
        "session-verify-stale",
        "thread-verify-stale",
        "%74",
    );
    let mut lines = String::new();
    for binding in [&kept, &stale] {
        lines.push_str(
            &serde_json::to_string(&crate::server::state::Event::GlobalCurrentThreadRouteSet {
                binding: binding.clone(),
            })
            .unwrap(),
        );
        lines.push('\n');
    }
    std::fs::write(&journal_path, lines).unwrap();

    // The target is still live, so the postcondition must not hold.
    let error = verify_retirement(
        &journal_path,
        std::slice::from_ref(&stale),
        &["binding-verify-kept".to_owned()],
    )
    .expect_err("an unretired target must fail the postcondition")
    .to_string();
    assert!(error.contains("RESET_VERIFY_FAILED"), "{error}");

    // A kept claimant that is not live is also a failed postcondition.
    let error = verify_retirement(&journal_path, &[], &["binding-absent".to_owned()])
        .expect_err("a kept claimant that is not live must fail the postcondition")
        .to_string();
    assert!(error.contains("RESET_VERIFY_FAILED"), "{error}");

    // With the retirement appended, the same target passes.
    append_retirement_events(
        &journal_path,
        std::slice::from_ref(&stale),
        "operator authorized the pane cleanup",
    )
    .unwrap();
    verify_retirement(
        &journal_path,
        std::slice::from_ref(&stale),
        &["binding-verify-kept".to_owned()],
    )
    .unwrap();

    std::fs::remove_dir_all(root).ok();
}

/// The project level must not retire the root whose `.agent-collab/` is the
/// live route index; that is the host level's job.
#[test]
fn reset_project_refuses_the_root_that_holds_the_live_index() {
    let root = temp_root("project-guard");
    let project = root.join("project");
    let state = root.join("state");
    std::fs::create_dir_all(project.join(".agent-collab/server")).unwrap();
    std::fs::create_dir_all(&state).unwrap();
    std::fs::write(
        state.join("service.json"),
        serde_json::to_vec(&json!({
            "desired_state": "down",
            "generation": 1,
            "service_scope_root": project.display().to_string(),
        }))
        .unwrap(),
    )
    .unwrap();

    let host_paths = HostPaths::for_state_root(&state).unwrap();
    let error = run(
        &Scope {
            root: project.clone(),
        },
        &host_paths,
        ResetRequest {
            approval: "operator authorized the project reset".into(),
            discard_legacy: true,
            ..ResetRequest::default()
        },
    )
    .expect_err("the resident index root must be refused")
    .to_string();
    assert!(error.contains("RESET_PROJECT_HOLDS_HOST_INDEX"), "{error}");
    assert!(
        project.join(".agent-collab/server").exists(),
        "a refused run must not retire the project control plane"
    );

    // A project that is not the resident root is still allowed to run.
    let other = root.join("other");
    std::fs::create_dir_all(other.join(".agent-collab/server")).unwrap();
    run(
        &Scope { root: other.clone() },
        &host_paths,
        ResetRequest {
            approval: "operator authorized the project reset".into(),
            discard_legacy: true,
            ..ResetRequest::default()
        },
    )
    .unwrap();
    assert!(
        is_current_empty_baseline(&other.join(".agent-collab")),
        "the project level rebuilds an empty baseline"
    );

    std::fs::remove_dir_all(root).ok();
}
