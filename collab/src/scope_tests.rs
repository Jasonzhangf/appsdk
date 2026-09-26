    use super::*;
    use serde_json::json;

    fn test_root(name: &str) -> PathBuf {
        Path::new("/tmp").join(format!(
            "cs-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    struct EnvVarGuard {
        key: &'static str,
        previous: Option<std::ffi::OsString>,
    }

    impl EnvVarGuard {
        fn without(key: &'static str) -> Self {
            let previous = std::env::var_os(key);
            std::env::remove_var(key);
            Self { key, previous }
        }
    }

    impl Drop for EnvVarGuard {
        fn drop(&mut self) {
            if let Some(previous) = self.previous.take() {
                std::env::set_var(self.key, previous);
            }
        }
    }

    #[test]
    fn init_scope_uses_unmarked_process_cwd() {
        let cwd = test_root("init-unmarked-cwd");
        std::fs::create_dir_all(&cwd).unwrap();
        let resolved = init_project_root(cwd.clone()).unwrap();
        assert_eq!(resolved, cwd);
        assert!(!resolved.join(".agent-collab").exists());
        std::fs::remove_dir_all(resolved).ok();
    }

    #[test]
    fn init_scope_rejects_a_missing_process_cwd() {
        let missing = test_root("init-missing-cwd");
        assert!(init_project_root(missing).is_err());
    }

    #[test]
    fn exact_root_never_captures_ancestor_or_sibling_state() {
        let parent = test_root("exact-scope");
        let first = parent.join("first");
        let second = parent.join("second");
        init(&parent).unwrap();
        std::fs::create_dir_all(&first).unwrap();
        init(&second).unwrap();

        assert!(Scope::from_project_root(first).is_err());
        assert_eq!(
            Scope::from_project_root(second.clone()).unwrap().root,
            second
        );

        std::fs::remove_dir_all(parent).ok();
    }

    #[test]
    fn route_scope_uses_exact_registered_project_cwd() {
        let parent = test_root("route-scope");
        let registered = parent.join("registered");
        let sibling = parent.join("sibling");
        std::fs::create_dir_all(&registered).unwrap();
        std::fs::create_dir_all(&sibling).unwrap();
        let route = RouteScope::for_registered_project(
            AppServerId::new("appserver-1").unwrap(),
            &registered,
        )
        .unwrap();

        route.validate_registered_cwd(&registered).unwrap();
        assert!(route.validate_registered_cwd(&sibling).is_err());
        assert!(route.validate_registered_cwd(&parent).is_err());
        assert_eq!(
            route.project_scope_id.as_str(),
            registered.canonicalize().unwrap().to_string_lossy()
        );
        std::fs::remove_dir_all(parent).ok();
    }

    #[test]
    fn identity_route_resolves_only_for_its_app_scope_and_contains_cwd() {
        let root = test_root("worktree-route");
        let canonical = root.join("project");
        let worktree = canonical.join("playground/task-a");
        let sibling = root.join("project-other");
        let unrelated = root.join("unrelated");
        std::fs::create_dir_all(canonical.join(".agent-collab")).unwrap();
        std::fs::create_dir_all(&worktree).unwrap();
        std::fs::create_dir_all(&sibling).unwrap();
        std::fs::create_dir_all(&unrelated).unwrap();

        let state_root = root.join("host-state");
        std::fs::create_dir_all(&state_root).unwrap();
        let route = json!({
            "version": 1,
            "op": "register",
            "app_scope_id": "appserver-cli",
            "project_scope": canonical.canonicalize().unwrap(),
            "canonical_root": canonical.canonicalize().unwrap(),
            "storage_root": canonical.canonicalize().unwrap(),
            "registered_ms": 1
        });
        std::fs::write(state_root.join("routes.jsonl"), format!("{route}\n")).unwrap();

        let host_paths = HostPaths::for_state_root(&state_root).unwrap();
        let app_scope = AppServerId::new("appserver-cli").unwrap();
        let resolved = canonical_route_for_identity(&host_paths, &worktree, &app_scope).unwrap();
        assert_eq!(resolved.root, canonical.canonicalize().unwrap());
        assert_eq!(resolved.app_scope_id.as_str(), "appserver-cli");
        assert!(canonical_route_for_identity(
            &host_paths,
            &unrelated,
            &AppServerId::new("appserver-cli").unwrap()
        )
        .is_err());
        assert!(canonical_route_for_identity(
            &host_paths,
            &sibling,
            &AppServerId::new("appserver-cli").unwrap()
        )
        .is_err());
        assert!(canonical_route_for_identity(
            &host_paths,
            &worktree,
            &AppServerId::new("appserver-other").unwrap()
        )
        .is_err());

        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn scope_resolve_reuses_identity_route_from_a_worktree() {
        let _env_guard = TEST_ENV_LOCK.lock().unwrap();
        let _tmux_env_guard = EnvVarGuard::without("TMUX_PANE");
        let previous_thread = std::env::var_os("CODEX_THREAD_ID");
        std::env::remove_var("CODEX_THREAD_ID");
        let root = test_root("scope-worktree-identity");
        let canonical = root.join("project");
        let worktree = canonical.join("playground/task-a");
        let state_root = root.join("host-state");
        std::fs::create_dir_all(canonical.join(".agent-collab")).unwrap();
        std::fs::create_dir_all(&state_root).unwrap();

        let status = Command::new("git")
            .args(["init", "-q", "-b", "main"])
            .current_dir(&canonical)
            .status()
            .unwrap();
        assert!(status.success());
        let status = Command::new("git")
            .args([
                "-c",
                "user.name=Collab Test",
                "-c",
                "user.email=collab-test@example.invalid",
                "commit",
                "--allow-empty",
                "-q",
                "-m",
                "initial",
            ])
            .current_dir(&canonical)
            .status()
            .unwrap();
        assert!(status.success());
        let status = Command::new("git")
            .args([
                "worktree",
                "add",
                "-q",
                "-b",
                "task-a",
                worktree.to_str().unwrap(),
                "main",
            ])
            .current_dir(&canonical)
            .status()
            .unwrap();
        assert!(status.success());
        std::fs::create_dir_all(worktree.join(".agent-collab")).unwrap();

        let canonical = canonical.canonicalize().unwrap();
        let worktree = worktree.canonicalize().unwrap();
        let route = json!({
            "version": 1,
            "op": "register",
            "app_scope_id": "appserver-cli",
            "project_scope": canonical,
            "canonical_root": canonical,
            "storage_root": canonical,
            "registered_ms": 1
        });
        std::fs::write(state_root.join("routes.jsonl"), format!("{route}\n")).unwrap();
        let identity_dir = state_root.join("identities/worker-a");
        std::fs::create_dir_all(&identity_dir).unwrap();
        std::fs::write(
            identity_dir.join("identity.json"),
            json!({
                "worker_id": "worker-a",
                "token": "token-a",
                "runtime": {
                    "agent_id": "worker-a",
                    "runtime_id": "runtime-a",
                    "appserver_id": "appserver-cli",
                    "endpoint_generation": 1,
                    "binding_id": "binding-a",
                    "native_thread_id": "thread-a"
                },
                "transport": {
                    "kind": "appserver",
                    "endpoint": "unix:///tmp/test.sock",
                    "namespace": "codex_tui",
                    "thread_id": "thread-a",
                    "capabilities": [],
                    "self_check": "test"
                }
            })
            .to_string(),
        )
        .unwrap();

        let host_paths = HostPaths::for_state_root(&state_root).unwrap();
        let resolved = Scope::resolve_from_cwd_with_host_paths(
            &worktree,
            &host_paths,
            Some("worker-a".into()),
        )
        .unwrap();

        assert_eq!(resolved.root, canonical);
        match previous_thread {
            Some(value) => std::env::set_var("CODEX_THREAD_ID", value),
            None => std::env::remove_var("CODEX_THREAD_ID"),
        }
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn lifecycle_scope_uses_the_exact_cwd_without_requiring_a_baseline() {
        let root = test_root("lifecycle-exact-cwd");
        let initialized = root.join("initialized");
        let uninitialized = root.join("uninitialized");
        std::fs::create_dir_all(initialized.join(".agent-collab")).unwrap();
        std::fs::create_dir_all(&uninitialized).unwrap();

        assert_eq!(
            validate_project_root(initialized.clone()).unwrap(),
            initialized
        );
        assert_eq!(
            validate_project_root(uninitialized.clone()).unwrap(),
            uninitialized
        );

        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn recovery_scope_uses_canonical_route_without_a_native_thread_route() {
        let _env_guard = TEST_ENV_LOCK.lock().unwrap();
        let _tmux_env_guard = EnvVarGuard::without("TMUX_PANE");
        let previous_cwd = std::env::current_dir().unwrap();
        let previous_thread = std::env::var_os("CODEX_THREAD_ID");
        let previous_state = std::env::var_os(COLLAB_STATE_DIR_ENV);
        let root = test_root("recovery-worktree-without-route");
        let canonical = root.join("project");
        let worktree = canonical.join("playground/task-a");
        let state_root = root.join("host-state");
        std::fs::create_dir_all(canonical.join(".agent-collab")).unwrap();
        std::fs::create_dir_all(&state_root).unwrap();
        let status = Command::new("git")
            .args(["init", "-q", "-b", "main"])
            .current_dir(&canonical)
            .status()
            .unwrap();
        assert!(status.success());
        let status = Command::new("git")
            .args([
                "-c",
                "user.name=Collab Test",
                "-c",
                "user.email=collab-test@example.invalid",
                "commit",
                "--allow-empty",
                "-q",
                "-m",
                "initial",
            ])
            .current_dir(&canonical)
            .status()
            .unwrap();
        assert!(status.success());
        let status = Command::new("git")
            .args([
                "worktree",
                "add",
                "-q",
                "-b",
                "task-a",
                worktree.to_str().unwrap(),
                "main",
            ])
            .current_dir(&canonical)
            .status()
            .unwrap();
        assert!(status.success());

        let canonical = canonical.canonicalize().unwrap();
        let worktree = worktree.canonicalize().unwrap();
        let route = json!({
            "version": 1,
            "op": "register",
            "app_scope_id": "appserver-cli",
            "project_scope": canonical,
            "canonical_root": canonical,
            "storage_root": canonical,
            "registered_ms": 1
        });
        std::fs::write(state_root.join("routes.jsonl"), format!("{route}\n")).unwrap();
        std::env::set_var(COLLAB_STATE_DIR_ENV, &state_root);
        std::env::set_current_dir(&worktree).unwrap();
        std::env::set_var("CODEX_THREAD_ID", "missing-native-route");

        let resolved = resolve_for_recovery().unwrap();

        assert_eq!(resolved.root, canonical);
        std::env::set_current_dir(previous_cwd).unwrap();
        match previous_thread {
            Some(value) => std::env::set_var("CODEX_THREAD_ID", value),
            None => std::env::remove_var("CODEX_THREAD_ID"),
        }
        match previous_state {
            Some(value) => std::env::set_var(COLLAB_STATE_DIR_ENV, value),
            None => std::env::remove_var(COLLAB_STATE_DIR_ENV),
        }
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn scope_resolve_prefers_a_fresh_local_baseline_over_a_stale_identity_route() {
        let _env_guard = TEST_ENV_LOCK.lock().unwrap();
        let _tmux_env_guard = EnvVarGuard::without("TMUX_PANE");
        let previous_thread = std::env::var_os("CODEX_THREAD_ID");
        std::env::remove_var("CODEX_THREAD_ID");
        let root = test_root("scope-fresh-reset");
        let project = root.join("project");
        let state_root = root.join("host-state");
        std::fs::create_dir_all(project.join(".agent-collab")).unwrap();
        std::fs::create_dir_all(&state_root).unwrap();

        let identity_dir = state_root.join("identities/worker-a");
        std::fs::create_dir_all(&identity_dir).unwrap();
        std::fs::write(
            identity_dir.join("identity.json"),
            json!({
                "worker_id": "worker-a",
                "token": "token-a",
                "runtime": {
                    "agent_id": "worker-a",
                    "runtime_id": "runtime-a",
                    "appserver_id": "appserver-cli",
                    "endpoint_generation": 1,
                    "binding_id": "binding-a",
                    "native_thread_id": "thread-a"
                },
                "transport": {
                    "kind": "appserver",
                    "endpoint": "unix:///tmp/test.sock",
                    "namespace": "codex_tui",
                    "thread_id": "thread-a",
                    "capabilities": [],
                    "self_check": "test"
                }
            })
            .to_string(),
        )
        .unwrap();

        let host_paths = HostPaths::for_state_root(&state_root).unwrap();
        let resolved =
            Scope::resolve_from_cwd_with_host_paths(&project, &host_paths, Some("worker-a".into()))
                .unwrap();

        assert_eq!(resolved.root, project);
        match previous_thread {
            Some(value) => std::env::set_var("CODEX_THREAD_ID", value),
            None => std::env::remove_var("CODEX_THREAD_ID"),
        }
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn scope_resolve_preserves_a_nested_project_inside_a_linked_worktree() {
        let _env_guard = TEST_ENV_LOCK.lock().unwrap();
        let _tmux_env_guard = EnvVarGuard::without("TMUX_PANE");
        let previous_thread = std::env::var_os("CODEX_THREAD_ID");
        std::env::remove_var("CODEX_THREAD_ID");
        let root = test_root("scope-nested-worktree");
        let canonical = root.join("project");
        let worktree = canonical.join("playground/task-a");
        let nested = worktree.join("services/service-a");
        let state_root = root.join("host-state");
        std::fs::create_dir_all(canonical.join(".agent-collab")).unwrap();
        std::fs::create_dir_all(&state_root).unwrap();

        let status = Command::new("git")
            .args(["init", "-q", "-b", "main"])
            .current_dir(&canonical)
            .status()
            .unwrap();
        assert!(status.success());
        let status = Command::new("git")
            .args([
                "-c",
                "user.name=Collab Test",
                "-c",
                "user.email=collab-test@example.invalid",
                "commit",
                "--allow-empty",
                "-q",
                "-m",
                "initial",
            ])
            .current_dir(&canonical)
            .status()
            .unwrap();
        assert!(status.success());
        let status = Command::new("git")
            .args([
                "worktree",
                "add",
                "-q",
                "-b",
                "task-a",
                worktree.to_str().unwrap(),
                "main",
            ])
            .current_dir(&canonical)
            .status()
            .unwrap();
        assert!(status.success());
        std::fs::create_dir_all(nested.join(".agent-collab")).unwrap();
        let nested = nested.canonicalize().unwrap();

        let host_paths = HostPaths::for_state_root(&state_root).unwrap();
        let resolved = Scope::resolve_from_cwd_with_host_paths(&nested, &host_paths, None).unwrap();

        assert_eq!(resolved.root, nested);
        match previous_thread {
            Some(value) => std::env::set_var("CODEX_THREAD_ID", value),
            None => std::env::remove_var("CODEX_THREAD_ID"),
        }
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn read_only_route_resolution_preserves_non_cli_scope_from_a_linked_worktree() {
        let root = test_root("read-only-worktree-route");
        let canonical = root.join("project");
        let worktree = canonical.join("playground/task-a");
        let state_root = root.join("host-state");
        std::fs::create_dir_all(canonical.join(".agent-collab")).unwrap();
        std::fs::create_dir_all(&state_root).unwrap();

        let status = Command::new("git")
            .args(["init", "-q", "-b", "main"])
            .current_dir(&canonical)
            .status()
            .unwrap();
        assert!(status.success());
        let status = Command::new("git")
            .args([
                "-c",
                "user.name=Collab Test",
                "-c",
                "user.email=collab-test@example.invalid",
                "commit",
                "--allow-empty",
                "-q",
                "-m",
                "initial",
            ])
            .current_dir(&canonical)
            .status()
            .unwrap();
        assert!(status.success());
        let status = Command::new("git")
            .args([
                "worktree",
                "add",
                "-q",
                "-b",
                "task-a",
                worktree.to_str().unwrap(),
                "main",
            ])
            .current_dir(&canonical)
            .status()
            .unwrap();
        assert!(status.success());

        let canonical = canonical.canonicalize().unwrap();
        let worktree = worktree.canonicalize().unwrap();
        let route = json!({
            "version": 1,
            "op": "register",
            "app_scope_id": "appserver-vscode",
            "project_scope": canonical,
            "canonical_root": canonical,
            "storage_root": canonical,
            "registered_ms": 1
        });
        std::fs::write(state_root.join("routes.jsonl"), format!("{route}\n")).unwrap();

        let host_paths = HostPaths::for_state_root(&state_root).unwrap();
        let resolved = canonical_route_for_cwd(&host_paths, &worktree).unwrap();

        assert_eq!(resolved.root, canonical);
        assert_eq!(resolved.app_scope_id.as_str(), "appserver-vscode");
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn read_only_route_resolution_keeps_a_nested_project_inside_a_linked_worktree() {
        let root = test_root("read-only-nested-worktree-route");
        let canonical = root.join("project");
        let worktree = canonical.join("playground/task-a");
        let nested = worktree.join("services/service-a");
        let state_root = root.join("host-state");
        std::fs::create_dir_all(canonical.join(".agent-collab")).unwrap();
        std::fs::create_dir_all(&state_root).unwrap();

        let status = Command::new("git")
            .args(["init", "-q", "-b", "main"])
            .current_dir(&canonical)
            .status()
            .unwrap();
        assert!(status.success());
        let status = Command::new("git")
            .args([
                "-c",
                "user.name=Collab Test",
                "-c",
                "user.email=collab-test@example.invalid",
                "commit",
                "--allow-empty",
                "-q",
                "-m",
                "initial",
            ])
            .current_dir(&canonical)
            .status()
            .unwrap();
        assert!(status.success());
        let status = Command::new("git")
            .args([
                "worktree",
                "add",
                "-q",
                "-b",
                "task-a",
                worktree.to_str().unwrap(),
                "main",
            ])
            .current_dir(&canonical)
            .status()
            .unwrap();
        assert!(status.success());
        std::fs::create_dir_all(nested.join(".agent-collab")).unwrap();

        let canonical = canonical.canonicalize().unwrap();
        let nested = nested.canonicalize().unwrap();
        let main_route = json!({
            "version": 1,
            "op": "register",
            "app_scope_id": "appserver-cli",
            "project_scope": canonical,
            "canonical_root": canonical,
            "storage_root": canonical,
            "registered_ms": 1
        });
        let nested_route = json!({
            "version": 1,
            "op": "register",
            "app_scope_id": "appserver-cli",
            "project_scope": nested,
            "canonical_root": nested,
            "storage_root": nested,
            "registered_ms": 2
        });
        std::fs::write(
            state_root.join("routes.jsonl"),
            format!("{main_route}\n{nested_route}\n"),
        )
        .unwrap();

        let host_paths = HostPaths::for_state_root(&state_root).unwrap();
        let resolved = canonical_route_for_cwd(&host_paths, &nested).unwrap();

        assert_eq!(resolved.root, nested);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn read_only_route_resolution_rejects_a_worktree_only_route() {
        let root = test_root("read-only-worktree-only-route");
        let canonical = root.join("project");
        let worktree = canonical.join("playground/task-a");
        let state_root = root.join("host-state");
        std::fs::create_dir_all(canonical.join(".agent-collab")).unwrap();
        std::fs::create_dir_all(&state_root).unwrap();

        let status = Command::new("git")
            .args(["init", "-q", "-b", "main"])
            .current_dir(&canonical)
            .status()
            .unwrap();
        assert!(status.success());
        let status = Command::new("git")
            .args([
                "-c",
                "user.name=Collab Test",
                "-c",
                "user.email=collab-test@example.invalid",
                "commit",
                "--allow-empty",
                "-q",
                "-m",
                "initial",
            ])
            .current_dir(&canonical)
            .status()
            .unwrap();
        assert!(status.success());
        let status = Command::new("git")
            .args([
                "worktree",
                "add",
                "-q",
                "-b",
                "task-a",
                worktree.to_str().unwrap(),
                "main",
            ])
            .current_dir(&canonical)
            .status()
            .unwrap();
        assert!(status.success());
        let worktree = worktree.canonicalize().unwrap();
        std::fs::create_dir_all(worktree.join(".agent-collab")).unwrap();
        let route = json!({
            "version": 1,
            "op": "register",
            "app_scope_id": "appserver-cli",
            "project_scope": worktree,
            "canonical_root": worktree,
            "storage_root": worktree,
            "registered_ms": 1
        });
        std::fs::write(state_root.join("routes.jsonl"), format!("{route}\n")).unwrap();

        let host_paths = HostPaths::for_state_root(&state_root).unwrap();
        let error = canonical_route_for_cwd(&host_paths, &worktree).unwrap_err();
        assert!(
            error.to_string().contains("no registered Collab route"),
            "unexpected error: {error}"
        );
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn identity_route_prefers_git_main_root_when_worktree_route_is_stale() {
        let root = test_root("worktree-route-disambiguation");
        let canonical = root.join("project");
        let worktree = canonical.join("playground/task-a");
        let state_root = root.join("host-state");
        std::fs::create_dir_all(canonical.join(".agent-collab")).unwrap();
        std::fs::create_dir_all(&worktree).unwrap();
        std::fs::create_dir_all(&state_root).unwrap();

        let status = Command::new("git")
            .args(["init", "-q", "-b", "main"])
            .current_dir(&canonical)
            .status()
            .unwrap();
        assert!(status.success());
        let status = Command::new("git")
            .args([
                "-c",
                "user.name=Collab Test",
                "-c",
                "user.email=collab-test@example.invalid",
                "commit",
                "--allow-empty",
                "-q",
                "-m",
                "initial",
            ])
            .current_dir(&canonical)
            .status()
            .unwrap();
        assert!(status.success());
        let status = Command::new("git")
            .args([
                "worktree",
                "add",
                "-q",
                "-b",
                "task-a",
                worktree.to_str().unwrap(),
                "main",
            ])
            .current_dir(&canonical)
            .status()
            .unwrap();
        assert!(status.success());

        let canonical = canonical.canonicalize().unwrap();
        let worktree = worktree.canonicalize().unwrap();
        let canonical_route = json!({
            "version": 1,
            "op": "register",
            "app_scope_id": "appserver-cli",
            "project_scope": canonical,
            "canonical_root": canonical,
            "storage_root": canonical,
            "registered_ms": 1
        });
        std::fs::write(
            state_root.join("routes.jsonl"),
            format!("{canonical_route}\n"),
        )
        .unwrap();

        let host_paths = HostPaths::for_state_root(&state_root).unwrap();
        let app_scope = AppServerId::new("appserver-cli").unwrap();
        let resolved = canonical_route_for_identity(&host_paths, &worktree, &app_scope).unwrap();

        assert_eq!(resolved.root, canonical);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn identity_route_rejects_a_worktree_only_route() {
        let root = test_root("worktree-route-only");
        let canonical = root.join("project");
        let worktree = canonical.join("playground/task-a");
        let state_root = root.join("host-state");
        std::fs::create_dir_all(canonical.join(".agent-collab")).unwrap();
        std::fs::create_dir_all(&worktree).unwrap();
        std::fs::create_dir_all(&state_root).unwrap();

        let status = Command::new("git")
            .args(["init", "-q", "-b", "main"])
            .current_dir(&canonical)
            .status()
            .unwrap();
        assert!(status.success());
        let status = Command::new("git")
            .args([
                "-c",
                "user.name=Collab Test",
                "-c",
                "user.email=collab-test@example.invalid",
                "commit",
                "--allow-empty",
                "-q",
                "-m",
                "initial",
            ])
            .current_dir(&canonical)
            .status()
            .unwrap();
        assert!(status.success());
        let status = Command::new("git")
            .args([
                "worktree",
                "add",
                "-q",
                "-b",
                "task-a",
                worktree.to_str().unwrap(),
                "main",
            ])
            .current_dir(&canonical)
            .status()
            .unwrap();
        assert!(status.success());

        let worktree = worktree.canonicalize().unwrap();
        std::fs::create_dir_all(worktree.join(".agent-collab")).unwrap();
        let worktree_route = json!({
            "version": 1,
            "op": "register",
            "app_scope_id": "appserver-cli",
            "project_scope": worktree,
            "canonical_root": worktree,
            "storage_root": worktree,
            "registered_ms": 1
        });
        std::fs::write(
            state_root.join("routes.jsonl"),
            format!("{worktree_route}\n"),
        )
        .unwrap();

        let host_paths = HostPaths::for_state_root(&state_root).unwrap();
        let app_scope = AppServerId::new("appserver-cli").unwrap();
        let error = canonical_route_for_identity(&host_paths, &worktree, &app_scope).unwrap_err();
        assert!(
            error.to_string().contains("Git main worktree"),
            "unexpected error: {error}"
        );
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn identity_route_selects_canonical_main_when_worktree_route_also_exists() {
        let root = test_root("worktree-route-both");
        let canonical = root.join("project");
        let worktree = canonical.join("playground/task-a");
        let state_root = root.join("host-state");
        std::fs::create_dir_all(canonical.join(".agent-collab")).unwrap();
        std::fs::create_dir_all(&worktree).unwrap();
        std::fs::create_dir_all(&state_root).unwrap();

        let status = Command::new("git")
            .args(["init", "-q", "-b", "main"])
            .current_dir(&canonical)
            .status()
            .unwrap();
        assert!(status.success());
        let status = Command::new("git")
            .args([
                "-c",
                "user.name=Collab Test",
                "-c",
                "user.email=collab-test@example.invalid",
                "commit",
                "--allow-empty",
                "-q",
                "-m",
                "initial",
            ])
            .current_dir(&canonical)
            .status()
            .unwrap();
        assert!(status.success());
        let status = Command::new("git")
            .args([
                "worktree",
                "add",
                "-q",
                "-b",
                "task-a",
                worktree.to_str().unwrap(),
                "main",
            ])
            .current_dir(&canonical)
            .status()
            .unwrap();
        assert!(status.success());

        let canonical = canonical.canonicalize().unwrap();
        let worktree = worktree.canonicalize().unwrap();
        std::fs::create_dir_all(worktree.join(".agent-collab")).unwrap();
        let canonical_route = json!({
            "version": 1,
            "op": "register",
            "app_scope_id": "appserver-cli",
            "project_scope": canonical,
            "canonical_root": canonical,
            "storage_root": canonical,
            "registered_ms": 1
        });
        let worktree_route = json!({
            "version": 1,
            "op": "register",
            "app_scope_id": "appserver-cli",
            "project_scope": worktree,
            "canonical_root": worktree,
            "storage_root": worktree,
            "registered_ms": 2
        });
        std::fs::write(
            state_root.join("routes.jsonl"),
            format!("{worktree_route}\n{canonical_route}\n"),
        )
        .unwrap();

        let host_paths = HostPaths::for_state_root(&state_root).unwrap();
        let app_scope = AppServerId::new("appserver-cli").unwrap();
        let resolved = canonical_route_for_identity(&host_paths, &worktree, &app_scope).unwrap();

        assert_eq!(resolved.root, canonical);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn identity_route_preserves_a_nested_project_root() {
        let root = test_root("nested-project-route");
        let repository = root.join("repo");
        let project = repository.join("services/service-a");
        let worktree = project.join("playground/task-a");
        let state_root = root.join("host-state");
        std::fs::create_dir_all(project.join(".agent-collab")).unwrap();
        std::fs::create_dir_all(&worktree).unwrap();
        std::fs::create_dir_all(&state_root).unwrap();

        let status = Command::new("git")
            .args(["init", "-q", "-b", "main"])
            .current_dir(&repository)
            .status()
            .unwrap();
        assert!(status.success());
        let status = Command::new("git")
            .args([
                "-c",
                "user.name=Collab Test",
                "-c",
                "user.email=collab-test@example.invalid",
                "commit",
                "--allow-empty",
                "-q",
                "-m",
                "initial",
            ])
            .current_dir(&repository)
            .status()
            .unwrap();
        assert!(status.success());
        let status = Command::new("git")
            .args([
                "worktree",
                "add",
                "-q",
                "-b",
                "task-a",
                worktree.to_str().unwrap(),
                "main",
            ])
            .current_dir(&repository)
            .status()
            .unwrap();
        assert!(status.success());

        let project = project.canonicalize().unwrap();
        let worktree = worktree.canonicalize().unwrap();
        std::fs::create_dir_all(worktree.join(".agent-collab")).unwrap();
        let route = json!({
            "version": 1,
            "op": "register",
            "app_scope_id": "appserver-cli",
            "project_scope": project,
            "canonical_root": project,
            "storage_root": project,
            "registered_ms": 1
        });
        std::fs::write(state_root.join("routes.jsonl"), format!("{route}\n")).unwrap();

        let host_paths = HostPaths::for_state_root(&state_root).unwrap();
        let app_scope = AppServerId::new("appserver-cli").unwrap();
        let resolved = canonical_route_for_identity(&host_paths, &worktree, &app_scope).unwrap();

        assert_eq!(resolved.root, project);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn identity_route_preserves_a_submodule_root() {
        let root = test_root("submodule-project-route");
        let superproject = root.join("superproject");
        let submodule = superproject.join("modules/service-a");
        let state_root = root.join("host-state");
        std::fs::create_dir_all(&submodule).unwrap();
        std::fs::create_dir_all(&state_root).unwrap();

        let status = Command::new("git")
            .args(["init", "-q", "-b", "main"])
            .current_dir(&superproject)
            .status()
            .unwrap();
        assert!(status.success());
        let status = Command::new("git")
            .args([
                "-c",
                "user.name=Collab Test",
                "-c",
                "user.email=collab-test@example.invalid",
                "commit",
                "--allow-empty",
                "-q",
                "-m",
                "initial superproject",
            ])
            .current_dir(&superproject)
            .status()
            .unwrap();
        assert!(status.success());

        let status = Command::new("git")
            .args(["init", "-q", "-b", "main"])
            .current_dir(&submodule)
            .status()
            .unwrap();
        assert!(status.success());
        let status = Command::new("git")
            .args([
                "-c",
                "user.name=Collab Test",
                "-c",
                "user.email=collab-test@example.invalid",
                "commit",
                "--allow-empty",
                "-q",
                "-m",
                "initial submodule",
            ])
            .current_dir(&submodule)
            .status()
            .unwrap();
        assert!(status.success());
        let status = Command::new("git")
            .args([
                "-c",
                "protocol.file.allow=always",
                "submodule",
                "add",
                submodule.to_str().unwrap(),
                "modules/service-a",
            ])
            .current_dir(&superproject)
            .status()
            .unwrap();
        assert!(status.success());

        let submodule = submodule.canonicalize().unwrap();
        std::fs::create_dir_all(submodule.join(".agent-collab")).unwrap();
        let route = json!({
            "version": 1,
            "op": "register",
            "app_scope_id": "appserver-cli",
            "project_scope": submodule,
            "canonical_root": submodule,
            "storage_root": submodule,
            "registered_ms": 1
        });
        std::fs::write(state_root.join("routes.jsonl"), format!("{route}\n")).unwrap();

        let host_paths = HostPaths::for_state_root(&state_root).unwrap();
        let app_scope = AppServerId::new("appserver-cli").unwrap();
        let resolved = canonical_route_for_identity(&host_paths, &submodule, &app_scope).unwrap();

        assert_eq!(resolved.root, submodule);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn route_scope_serializes_two_levels_and_does_not_mutate_paths() {
        let root = test_root("route-serialization");
        std::fs::create_dir_all(&root).unwrap();
        let route =
            RouteScope::for_registered_project(AppServerId::new("appserver-1").unwrap(), &root)
                .unwrap();
        let before = route.clone();
        let encoded = serde_json::to_value(&route).unwrap();
        assert_eq!(encoded["app_scope_id"], "appserver-1");
        assert_eq!(
            encoded["project_scope_id"],
            root.canonicalize().unwrap().to_string_lossy().as_ref()
        );
        assert_eq!(
            serde_json::from_value::<RouteScope>(encoded).unwrap(),
            route
        );
        assert_eq!(route, before);
        std::fs::remove_dir_all(root).ok();
    }

    #[cfg(unix)]
    #[test]
    fn non_utf8_registered_cwd_fails_closed_without_scope_collision() {
        use std::ffi::OsString;
        use std::os::unix::ffi::OsStringExt;

        let parent = test_root("non-utf8-route-scope");
        std::fs::create_dir_all(&parent).unwrap();
        let first = parent.join(OsString::from_vec(b"project-\xff".to_vec()));
        let second = parent.join(OsString::from_vec(b"project-\xfe".to_vec()));
        if std::fs::create_dir(&first).is_err() || std::fs::create_dir(&second).is_err() {
            std::fs::remove_dir_all(parent).ok();
            return;
        }

        assert!(RouteScope::for_registered_project(
            AppServerId::new("appserver-1").unwrap(),
            &first
        )
        .is_err());
        assert!(RouteScope::for_registered_project(
            AppServerId::new("appserver-1").unwrap(),
            &second
        )
        .is_err());
        std::fs::remove_dir_all(parent).ok();
    }

    #[test]
    fn long_registered_cwd_has_a_valid_unbounded_project_scope() {
        let base = test_root("long-route-scope");
        let mut root = base.clone();
        for index in 0..24 {
            root = root.join(format!("segment-{index:02}-abcdef"));
        }
        std::fs::create_dir_all(&root).unwrap();
        let route =
            RouteScope::for_registered_project(AppServerId::new("appserver-1").unwrap(), &root)
                .unwrap();
        assert!(route.project_scope_id.as_str().len() > 256);
        route.validate_registered_cwd(&root).unwrap();
        std::fs::remove_dir_all(base).ok();
    }

    #[test]
    fn host_endpoint_is_stable_across_project_roots() {
        let host_root = test_root("host-endpoint").join("state");
        let first_project = test_root("host-project-one");
        let second_project = test_root("host-project-two");
        std::fs::create_dir_all(&first_project).unwrap();
        std::fs::create_dir_all(&second_project).unwrap();

        let first = HostPaths::for_state_root(&host_root).unwrap();
        let second = HostPaths::for_state_root(&host_root).unwrap();
        assert_eq!(first.socket_path(), second.socket_path());
        assert_eq!(first.lock_path(), second.lock_path());
        assert_ne!(
            first.socket_path(),
            first_project.join(".agent-collab/server/server.sock")
        );
        assert_ne!(
            second.socket_path(),
            second_project.join(".agent-collab/server/server.sock")
        );

        std::fs::remove_dir_all(first_project).ok();
        std::fs::remove_dir_all(second_project).ok();
        std::fs::remove_dir_all(host_root.parent().unwrap()).ok();
    }

    #[test]
    fn host_endpoint_rejects_relative_state_roots() {
        let error = HostPaths::for_state_root("collab-state").unwrap_err();
        assert!(error.to_string().contains("absolute"));
    }

    #[test]
    fn default_host_endpoint_uses_dot_collab_in_home() {
        let home = test_root("host-home-default");
        std::fs::create_dir_all(&home).unwrap();
        let state_root =
            resolve_state_root(Some("".into()), Some(home.clone().into_os_string())).unwrap();
        assert_eq!(state_root, home.join(".collab"));
        std::fs::remove_dir_all(home).ok();
    }

    #[test]
    fn host_endpoint_rejects_split_socket_root() {
        let root = test_root("host-socket-split");
        let mut paths = HostPaths::for_state_root(&root).unwrap();
        let error = apply_endpoint_overrides(
            &mut paths,
            Some(root.join("nested").join("server.sock")),
            None,
        )
        .unwrap_err();
        assert!(error.to_string().contains("inside host state root"));
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn host_endpoint_rejects_split_lock_owner() {
        let root = test_root("host-lock-split");
        let mut paths = HostPaths::for_state_root(&root).unwrap();
        let error =
            apply_endpoint_overrides(&mut paths, None, Some(root.join("other.lock"))).unwrap_err();
        assert!(error.to_string().contains("one lock owner"));
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn init_releases_collab_doc_only_once() {
        let root = test_root("init");
        init(&root).unwrap();
        let path = root.join("docs/collab.md");
        assert!(path.exists());
        let first = std::fs::read_to_string(&path).unwrap();
        assert!(first.contains("# collab workflow"));
        for mcp in [root.join(".mcp.json")] {
            assert!(std::fs::read_to_string(&mcp)
                .unwrap()
                .contains("collab-mcp"));
        }

        init(&root).unwrap();
        let second = std::fs::read_to_string(&path).unwrap();
        assert_eq!(first, second);
        std::fs::write(
            root.join(".mcp.json"),
            r#"{"mcpServers":{"other":{"command":"keep-me"}}}"#,
        )
        .unwrap();
        init(&root).unwrap();
        let generic: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(root.join(".mcp.json")).unwrap())
                .unwrap();
        assert_eq!(generic["mcpServers"]["other"]["command"], "keep-me");
        assert!(generic["mcpServers"]["collab"]["command"]
            .as_str()
            .unwrap()
            .contains("collab-mcp"));
        let codex: toml::Value =
            toml::from_str(&std::fs::read_to_string(root.join(".codex/config.toml")).unwrap())
                .unwrap();
        assert!(codex.get("sandbox_mode").is_none());
        assert!(codex.get("approval_policy").is_none());
        assert!(codex["mcp_servers"]["collab"]["command"]
            .as_str()
            .unwrap()
            .contains("collab-mcp"));
        std::fs::write(
            root.join(".codex/config.toml"),
            "model = \"keep-me\"\nsandbox_mode = \"workspace-write\"\n",
        )
        .unwrap();
        init(&root).unwrap();
        let upgraded: toml::Value =
            toml::from_str(&std::fs::read_to_string(root.join(".codex/config.toml")).unwrap())
                .unwrap();
        assert_eq!(upgraded["model"].as_str(), Some("keep-me"));
        assert!(upgraded.get("sandbox_mode").is_none());
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn init_rejects_malformed_existing_configuration_without_overwriting_it() {
        let json_root = test_root("init-invalid-json");
        std::fs::create_dir_all(&json_root).unwrap();
        let json_path = json_root.join(".mcp.json");
        let json_before = "{\"mcpServers\": [";
        std::fs::write(&json_path, json_before).unwrap();
        let json_error = init(&json_root).unwrap_err();
        assert!(json_error.to_string().contains("invalid JSON"));
        assert_eq!(std::fs::read_to_string(&json_path).unwrap(), json_before);

        let toml_root = test_root("init-invalid-toml");
        let toml_path = toml_root.join(".codex/config.toml");
        std::fs::create_dir_all(toml_path.parent().unwrap()).unwrap();
        let toml_before = "mcp_servers = [";
        std::fs::write(&toml_path, toml_before).unwrap();
        let toml_error = init(&toml_root).unwrap_err();
        assert!(toml_error.to_string().contains("invalid TOML"));
        assert_eq!(std::fs::read_to_string(&toml_path).unwrap(), toml_before);

        let claude_root = test_root("init-invalid-claude");
        let claude_path = claude_root.join(".claude/settings.json");
        std::fs::create_dir_all(claude_path.parent().unwrap()).unwrap();
        let claude_before = "{\"permissions\": [";
        std::fs::write(&claude_path, claude_before).unwrap();
        let claude_error = init(&claude_root).unwrap_err();
        assert!(claude_error.to_string().contains("invalid JSON"));
        assert_eq!(
            std::fs::read_to_string(&claude_path).unwrap(),
            claude_before
        );

        let json_shape_root = test_root("init-invalid-mcp-shape");
        std::fs::create_dir_all(&json_shape_root).unwrap();
        let json_shape_path = json_shape_root.join(".mcp.json");
        let json_shape_before = "{\"mcpServers\": []}";
        std::fs::write(&json_shape_path, json_shape_before).unwrap();
        let json_shape_error = init(&json_shape_root).unwrap_err();
        assert!(json_shape_error.to_string().contains("must be an object"));
        assert_eq!(
            std::fs::read_to_string(&json_shape_path).unwrap(),
            json_shape_before
        );

        let toml_shape_root = test_root("init-invalid-mcp-table");
        let toml_shape_path = toml_shape_root.join(".codex/config.toml");
        std::fs::create_dir_all(toml_shape_path.parent().unwrap()).unwrap();
        let toml_shape_before = "mcp_servers = []\n";
        std::fs::write(&toml_shape_path, toml_shape_before).unwrap();
        let toml_shape_error = init(&toml_shape_root).unwrap_err();
        assert!(toml_shape_error.to_string().contains("must be a table"));
        assert_eq!(
            std::fs::read_to_string(&toml_shape_path).unwrap(),
            toml_shape_before
        );

        std::fs::remove_dir_all(json_root).ok();
        std::fs::remove_dir_all(toml_root).ok();
        std::fs::remove_dir_all(claude_root).ok();
        std::fs::remove_dir_all(json_shape_root).ok();
        std::fs::remove_dir_all(toml_shape_root).ok();
    }

    #[test]
    fn identity_route_rejects_malformed_control_records() {
        let root = test_root("worktree-route-invalid");
        let canonical = root.join("project");
        let worktree = canonical.join("playground/task-a");
        let state_root = root.join("host-state");
        std::fs::create_dir_all(canonical.join(".agent-collab")).unwrap();
        std::fs::create_dir_all(&worktree).unwrap();
        std::fs::create_dir_all(&state_root).unwrap();
        let canonical = canonical.canonicalize().unwrap();

        let valid = json!({
            "version": 1,
            "op": "register",
            "app_scope_id": "appserver-cli",
            "project_scope": canonical,
            "canonical_root": canonical,
            "storage_root": canonical,
            "registered_ms": 1
        });
        let cases = [
            json!({
                "version": 1,
                "op": "register",
                "app_scope_id": "appserver-cli",
                "project_scope": canonical,
                "canonical_root": canonical,
                "registered_ms": 1
            }),
            json!({
                "version": 1,
                "op": "register",
                "app_scope_id": "appserver-cli",
                "project_scope": canonical,
                "canonical_root": canonical,
                "storage_root": canonical,
                "registered_ms": 1,
                "unknown": true
            }),
            json!({
                "version": 1,
                "op": "register",
                "app_scope_id": "appserver-cli",
                "project_scope": root.join("other-project"),
                "canonical_root": canonical,
                "storage_root": canonical,
                "registered_ms": 1
            }),
        ];

        for invalid in cases {
            std::fs::write(
                state_root.join("routes.jsonl"),
                format!("{invalid}\n{valid}\n"),
            )
            .unwrap();
            let host_paths = HostPaths::for_state_root(&state_root).unwrap();
            let app_scope = AppServerId::new("appserver-cli").unwrap();
            assert!(canonical_route_for_identity(&host_paths, &worktree, &app_scope).is_err());
        }

        std::fs::write(
            state_root.join("routes.jsonl"),
            format!("{valid}\n{valid}\n"),
        )
        .unwrap();
        let host_paths = HostPaths::for_state_root(&state_root).unwrap();
        let app_scope = AppServerId::new("appserver-cli").unwrap();
        assert!(canonical_route_for_identity(&host_paths, &worktree, &app_scope).is_err());

        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn identity_route_ignores_missing_stale_root_and_keeps_current_route() {
        let root = test_root("worktree-route-missing-root");
        let canonical = root.join("project");
        let worktree = canonical.join("playground/task-a");
        let missing = root.join("removed-worktree");
        let state_root = root.join("host-state");
        std::fs::create_dir_all(canonical.join(".agent-collab")).unwrap();
        std::fs::create_dir_all(&worktree).unwrap();
        std::fs::create_dir_all(&state_root).unwrap();
        let canonical = canonical.canonicalize().unwrap();

        let stale = json!({
            "version": 1,
            "op": "register",
            "app_scope_id": "appserver-cli",
            "project_scope": missing,
            "canonical_root": missing,
            "storage_root": missing,
            "registered_ms": 1
        });
        let current = json!({
            "version": 1,
            "op": "register",
            "app_scope_id": "appserver-cli",
            "project_scope": canonical,
            "canonical_root": canonical,
            "storage_root": canonical,
            "registered_ms": 2
        });
        std::fs::write(
            state_root.join("routes.jsonl"),
            format!("{stale}\n{current}\n"),
        )
        .unwrap();

        let host_paths = HostPaths::for_state_root(&state_root).unwrap();
        let app_scope = AppServerId::new("appserver-cli").unwrap();
        let resolved = canonical_route_for_identity(&host_paths, &worktree, &app_scope).unwrap();

        assert_eq!(resolved.root, canonical);

        std::fs::remove_dir_all(root).ok();
    }
