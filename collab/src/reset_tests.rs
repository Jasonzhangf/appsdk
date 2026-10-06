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
    std::os::unix::fs::symlink(&target, root.join(".agent-collab/server/journal.jsonl")).unwrap();

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
    let error = ResetLevel::select(false, false)
        .expect_err("no level must fail before any file is read")
        .to_string();
    assert!(error.contains("RESET_LEVEL_REQUIRED"), "{error}");
    assert!(ResetLevel::select(true, true).is_err());
    assert_eq!(
        ResetLevel::select(true, false).unwrap(),
        ResetLevel::Project
    );
    assert_eq!(
        ResetLevel::select(false, true).unwrap(),
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
    request.storage_root = None;
    request.include_runs = true;
    let error = request
        .validate_level_flags()
        .expect_err("--project must reject --include-runs")
        .to_string();
    assert!(error.contains("RESET_LEVEL_FLAG_MISMATCH"), "{error}");

    // The host level requires the storage root.
    request.include_runs = false;
    request.level = ResetLevel::Host;
    let error = request
        .validate_level_flags()
        .expect_err("--host must require --storage-root")
        .to_string();
    assert!(error.contains("RESET_STORAGE_ROOT_REQUIRED"), "{error}");
    request.storage_root = Some(PathBuf::from("/tmp/not-used"));
    assert!(request.validate_level_flags().is_ok());
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
        &Scope {
            root: other.clone(),
        },
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

/// A descriptor that exists but cannot be read, parsed, or resolved must fail
/// closed. Collapsing it to "no resident root" would let the project level
/// retire the daemon's own index root.
#[test]
fn reset_project_refuses_an_unresolvable_resident_index_descriptor() {
    let root = temp_root("index-root-unresolved");
    let project = root.join("project");
    let state = root.join("state");
    std::fs::create_dir_all(project.join(".agent-collab/server")).unwrap();
    std::fs::create_dir_all(&state).unwrap();

    let bodies: Vec<(String, &str)> = vec![
        ("{ this is not json".to_owned(), "not valid JSON"),
        (
            serde_json::to_string(&json!({"desired_state": "down", "generation": 1})).unwrap(),
            "no service_scope_root",
        ),
        (
            serde_json::to_string(&json!({
                "desired_state": "down",
                "generation": 1,
                "service_scope_root": state.join("empty").display().to_string(),
            }))
            .unwrap(),
            "cannot be resolved",
        ),
    ];
    for (body, label) in bodies {
        std::fs::write(state.join("service.json"), body).unwrap();
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
        .expect_err(&format!("a {label} descriptor must fail closed"))
        .to_string();
        assert!(
            error.contains("RESET_INDEX_ROOT_UNRESOLVED"),
            "a {label} descriptor must report RESET_INDEX_ROOT_UNRESOLVED: {error}"
        );
        assert!(
            error.contains("service.json"),
            "the error must name the descriptor: {error}"
        );
    }

    // No descriptor at all is the other case: nothing holds the live index, so
    // the project level runs.
    std::fs::remove_file(state.join("service.json")).unwrap();
    let host_paths = HostPaths::for_state_root(&state).unwrap();
    run(
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
    .unwrap();
    assert!(
        is_current_empty_baseline(&project.join(".agent-collab")),
        "without a descriptor the project level rebuilds an empty baseline"
    );

    std::fs::remove_dir_all(root).ok();
}
