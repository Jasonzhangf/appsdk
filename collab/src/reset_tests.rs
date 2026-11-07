use super::*;

fn temp_root(name: &str) -> PathBuf {
    PathBuf::from(format!(
        "/tmp/cr-{name}-{}-{}",
        std::process::id(),
        now_ms()
    ))
}

fn seed_legacy_runtime(root: &Path) {
    std::fs::create_dir_all(root.join(".agent-collab/server")).unwrap();
}

fn reset_route(root: &Path, registered_ms: i64) -> crate::server::HostRouteRecord {
    crate::server::HostRouteRecord {
        app_scope_id: "appserver-cli".into(),
        project_scope: root.display().to_string(),
        canonical_root: root.display().to_string(),
        storage_root: root.display().to_string(),
        registered_ms,
    }
}

fn publish_reset_routes(state: &Path, routes: &[crate::server::HostRouteRecord]) {
    crate::server::binding_store::publish_route_records(&state.join("routes.json"), routes)
        .unwrap()
        .require_confirmed_route()
        .unwrap();
}

fn assert_current_project_marker(root: &Path) {
    let marker = root.join(".agent-collab");
    assert!(marker.is_dir(), "current baseline marker is missing");
    assert!(
        std::fs::read_dir(&marker).unwrap().next().is_none(),
        "the current baseline is a discovery marker, not project-local runtime state"
    );
    assert!(!marker.join("server").exists());
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
    let retired = reset_route(&project.canonicalize().unwrap(), 1);
    let kept = reset_route(&other.canonicalize().unwrap(), 2);
    publish_reset_routes(&state, &[retired, kept.clone()]);

    assert_eq!(retire_host_routes(&host, &project).unwrap(), 1);
    let remaining = std::fs::read(state.join("routes.json")).unwrap();
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&remaining).unwrap(),
        json!([kept])
    );

    // Idempotent: a second pass removes nothing and leaves the table intact.
    assert_eq!(retire_host_routes(&host, &project).unwrap(), 0);
    assert_eq!(std::fs::read(state.join("routes.json")).unwrap(), remaining);

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
fn reset_retires_nonempty_legacy_runtime_files() {
    let root = temp_root("nonempty-runtime");
    seed_legacy_runtime(&root);
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
    assert_current_project_marker(&root);
    let archives = archive_roots(&state);
    assert_eq!(archives.len(), 1);
    assert_eq!(
        std::fs::read(archives[0].join(".agent-collab/server/events.jsonl")).unwrap(),
        b"{}\n"
    );
    assert_eq!(
        std::fs::read(archives[0].join(".agent-collab/server/log.txt")).unwrap(),
        b"old log\n"
    );

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
    seed_legacy_runtime(&root);
    std::fs::create_dir_all(root.join(".agent-collab/runs")).unwrap();
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
    assert_current_project_marker(&root);
    let archives = archive_roots(&state);
    assert_eq!(archives.len(), 1);
    assert_eq!(
        std::fs::read(archives[0].join(".agent-collab/runs/stale.jsonl")).unwrap(),
        b"{}\n"
    );
    assert_eq!(
        std::fs::read(archives[0].join(".agent-collab/server/runtimes/old/journal.jsonl")).unwrap(),
        b"legacy\n"
    );

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
    let routes = state.join("routes.json");
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

fn archive_roots(state: &Path) -> Vec<PathBuf> {
    let path = state.join("archives");
    if !path.exists() {
        return Vec::new();
    }
    let mut roots = std::fs::read_dir(path)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect::<Vec<_>>();
    roots.sort();
    roots
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

    assert_current_project_marker(&root);
    assert!(!root.join(".agent-collab/server/journal.jsonl").exists());
    assert!(archive_roots(&state).is_empty());

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

    let live_record = reset_route(&live.canonicalize().unwrap(), 1);
    publish_reset_routes(
        &state,
        &[
            live_record.clone(),
            reset_route(&uninitialized.canonicalize().unwrap(), 1),
            reset_route(&missing, 1),
        ],
    );

    assert_eq!(prune_stale_host_routes(&host).unwrap(), 2);
    let remaining = std::fs::read(state.join("routes.json")).unwrap();
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&remaining).unwrap(),
        json!([live_record])
    );

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

    // The current discovery marker replaces the retired local control plane.
    assert_current_project_marker(&root);
    assert!(!root.join(".agent-collab/server/journal.jsonl").exists());

    let first_archives = archive_roots(&state);
    assert_eq!(first_archives.len(), 1);
    let original =
        std::fs::read(first_archives[0].join(".agent-collab/server/journal.jsonl")).unwrap();
    assert_eq!(
        original,
        b"{\"ev\":\"NotificationSubscribed\",\"subscription\":{}}\n"
    );

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
    assert_eq!(archive_roots(&state), first_archives);
    assert_eq!(
        std::fs::read(first_archives[0].join(".agent-collab/server/journal.jsonl")).unwrap(),
        original
    );
    assert_current_project_marker(&root);

    // Non-empty runtime artifacts are historical state too. They must be
    // archived and cleared rather than being accepted as an empty baseline.
    seed_legacy_runtime(&root);
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
    let archives = archive_roots(&state);
    assert_eq!(archives.len(), 2);
    let added = archives
        .iter()
        .find(|path| !first_archives.contains(path))
        .unwrap();
    assert_eq!(
        std::fs::read(added.join(".agent-collab/server/events.jsonl")).unwrap(),
        b"{}\n"
    );
    assert_eq!(
        std::fs::read(added.join(".agent-collab/server/log.txt")).unwrap(),
        b"started\n"
    );
    assert!(!root.join(".agent-collab/server/events.jsonl").exists());
    assert!(!root.join(".agent-collab/server/log.txt").exists());
    assert_current_project_marker(&root);

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
    assert_eq!(ResetLevel::select(false, true).unwrap(), ResetLevel::Host);
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

/// C2: a project that IS the daemon's storage root must be recoverable with
/// `collab reset --project`. The project level retires that project's business
/// payload and its resident `.agent-collab/server/` index, leaves every other
/// project alone, and keeps the host-level audit trail.
#[test]
fn reset_project_retires_a_legacy_resident_index() {
    let root = temp_root("project-guard");
    let project = root.join("project");
    let other = root.join("other");
    let state = root.join("state");
    std::fs::create_dir_all(project.join(".agent-collab/server")).unwrap();
    for payload in ["mailbox", "review", "agy", "runs", "merge-queue", "handoff"] {
        std::fs::create_dir_all(project.join(".agent-collab").join(payload)).unwrap();
    }
    let resident_journal = b"{\"ev\":\"Sent\",\"msg\":{}}\n";
    std::fs::write(
        project.join(".agent-collab/server/journal.jsonl"),
        resident_journal,
    )
    .unwrap();
    std::fs::write(project.join(".agent-collab/server/events.jsonl"), b"{}\n").unwrap();
    std::fs::write(
        project.join(".agent-collab/server/log.txt"),
        b"resident log\n",
    )
    .unwrap();
    std::fs::write(project.join(".agent-collab/mailbox/old.jsonl"), b"{}\n").unwrap();
    std::fs::write(project.join(".agent-collab/runs/old.jsonl"), b"{}\n").unwrap();

    // Another project's state and route must survive this run.
    std::fs::create_dir_all(other.join(".agent-collab/server")).unwrap();
    std::fs::create_dir_all(other.join(".agent-collab/mailbox")).unwrap();
    std::fs::write(other.join(".agent-collab/server/journal.jsonl"), b"{}\n").unwrap();
    std::fs::write(other.join(".agent-collab/mailbox/keep.jsonl"), b"{}\n").unwrap();

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
    let kept_route = reset_route(&other.canonicalize().unwrap(), 2);
    publish_reset_routes(
        &state,
        &[
            reset_route(&project.canonicalize().unwrap(), 1),
            kept_route.clone(),
        ],
    );
    std::fs::create_dir_all(state.join("archives/prior")).unwrap();
    std::fs::write(state.join("archives/prior/manifest.json"), b"{}\n").unwrap();

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
    .expect("the storage root must be recoverable with a project reset");

    // The project's business payload and resident index are retired.
    assert!(!project.join(".agent-collab/server/journal.jsonl").exists());
    assert!(!project.join(".agent-collab/server/events.jsonl").exists());
    assert!(!project.join(".agent-collab/server/log.txt").exists());
    assert!(!project.join(".agent-collab/mailbox/old.jsonl").exists());
    assert!(!project.join(".agent-collab/runs/old.jsonl").exists());
    assert_current_project_marker(&project);
    let archives = archive_roots(&state);
    let archive_root = archives
        .iter()
        .find(|path| **path != state.join("archives/prior"))
        .unwrap();
    assert_eq!(archives.len(), 2);
    assert_eq!(
        std::fs::read(archive_root.join(".agent-collab/server/journal.jsonl")).unwrap(),
        resident_journal
    );

    // Another project is untouched, including its route record.
    assert_eq!(
        std::fs::read(other.join(".agent-collab/server/journal.jsonl")).unwrap(),
        b"{}\n"
    );
    assert_eq!(
        std::fs::read(other.join(".agent-collab/mailbox/keep.jsonl")).unwrap(),
        b"{}\n"
    );
    let routes = std::fs::read(state.join("routes.json")).unwrap();
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&routes).unwrap(),
        json!([kept_route])
    );

    // Unrelated prior archive contents remain intact.
    assert!(state.join("archives/prior/manifest.json").exists());
    assert_eq!(
        std::fs::read(state.join("archives/prior/manifest.json")).unwrap(),
        b"{}\n"
    );

    std::fs::remove_dir_all(root).ok();
}

/// Host reset refuses malformed, foreign-owned or missing current state.
#[test]
fn reset_host_still_refuses_a_storage_root_that_does_not_own_the_index() {
    let root = temp_root("host-foreign");
    let recorded = root.join("recorded");
    let foreign = root.join("foreign");
    let other = root.join("other");
    let state = root.join("state");
    for path in [
        recorded.join(".agent-collab/server"),
        foreign.join(".agent-collab/server"),
        state.clone(),
    ] {
        std::fs::create_dir_all(path).unwrap();
    }
    std::fs::create_dir_all(&other).unwrap();

    // `recorded` is the daemon's declared storage root; `foreign` is not.
    std::fs::write(
        state.join("service.json"),
        serde_json::to_vec(&json!({
            "desired_state": "down",
            "generation": 1,
            "service_scope_root": recorded.display().to_string(),
        }))
        .unwrap(),
    )
    .unwrap();
    let host_paths = HostPaths::for_state_root(&state).unwrap();

    // An invalid current file at an unowned root stays untouched.
    let current_file = foreign.join(".agent-collab/server/state.json");
    let invalid = b"{invalid current state}";
    std::fs::write(&current_file, invalid).unwrap();
    let error = run(
        &Scope {
            root: foreign.clone(),
        },
        &host_paths,
        ResetRequest {
            approval: "operator authorized the host reset".into(),
            discard_legacy: true,
            level: ResetLevel::Host,
            storage_root: Some(foreign.clone()),
            include_runs: false,
        },
    )
    .expect_err("a root the descriptor does not name must be refused")
    .to_string();
    assert!(error.contains("CURRENT_STATE_DECODE_FAILED:"), "{error}");
    assert_eq!(std::fs::read(&current_file).unwrap(), invalid);
    assert!(!state.join("archives").exists());
    assert!(!state.join("reset.jsonl").exists());

    // Current relationships naming only another project cannot prove ownership.
    let mut current = crate::server::state::State::default();
    current
        .apply_checked(&crate::server::state::Event::GlobalProjectRegistered {
            registration: crate::server::global_state::ProjectRegistration::new(
                crate::scope::ProjectScopeId::new(
                    other.canonicalize().unwrap().display().to_string(),
                )
                .unwrap(),
                crate::identity::AppServerId::new("appserver-cli").unwrap(),
            )
            .unwrap(),
        })
        .unwrap();
    assert!(crate::server::current_state::save(&current_file, &current).unwrap());
    let before = std::fs::read(&current_file).unwrap();
    let error = run(
        &Scope {
            root: foreign.clone(),
        },
        &host_paths,
        ResetRequest {
            approval: "operator authorized the host reset".into(),
            discard_legacy: true,
            level: ResetLevel::Host,
            storage_root: Some(foreign.clone()),
            include_runs: false,
        },
    )
    .expect_err("current state with no relationship to this root must be refused")
    .to_string();
    assert!(error.contains("RESET_STORAGE_ROOT_INVALID"), "{error}");
    assert_eq!(std::fs::read(&current_file).unwrap(), before);
    assert!(!state.join("archives").exists());

    // A root without current state is not an established storage owner.
    let error = run(
        &Scope {
            root: foreign.clone(),
        },
        &host_paths,
        ResetRequest {
            approval: "operator authorized the host reset".into(),
            discard_legacy: true,
            level: ResetLevel::Host,
            storage_root: Some(root.join("missing")),
            include_runs: false,
        },
    )
    .expect_err("a root without current state must be refused")
    .to_string();
    assert!(error.contains("RESET_STORAGE_ROOT_INVALID"), "{error}");

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
    assert_current_project_marker(&project);

    std::fs::remove_dir_all(root).ok();
}
