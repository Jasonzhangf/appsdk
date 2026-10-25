include!("support/context_operation_fixture.rs");

fn authorize(f: &Fixture, op: &str) -> Value {
    f.run_ok(
        &["context", "--op", op, "--approve-grant",
          r#"{"action":"authorize_first_registered_peer","authorization":"user authorizes first registered peer as project master"}"#],
        None,
    )
}

fn register(f: &Fixture, host: &TestAppServer, id: &str) -> Value {
    let facts = json!({"session_id": id, "thread_id": id,
        "endpoint": host.endpoint(), "namespace": "codex_tui"});
    f.context_provide(id, &facts, None)
}

#[test]
fn bootstrap_public_first_master_second_peer_retry_and_replay() {
    let mut f = Fixture::without_tmux("bootstrap-public");
    let policy = authorize(&f, "policy");
    assert_eq!(policy["result"]["outcome"], "completed", "{policy}");
    assert_eq!(authorize(&f, "policy")["result"], policy["result"]);
    assert_eq!(
        f.context_query("policy", None)["result"]["outcome"],
        "completed"
    );
    let a = TestAppServer::start_with_effect_at(&f.root, "a.sock", "a", "a", "ack_only");
    let b = TestAppServer::start_with_effect_at(&f.root, "b.sock", "b", "b", "ack_only");
    let first = register(&f, &a, "a");
    assert_eq!(
        first["result"]["snapshot"]["identity"]["role"], "master",
        "{first}"
    );
    // A caller that lost this response can query/retry the same operation.
    assert_eq!(f.context_query("a", None)["result"]["outcome"], "completed");
    let retried = register(&f, &a, "a");
    assert_eq!(retried["result"]["outcome"], "completed");
    assert_eq!(
        retried["result"]["snapshot"]["binding"],
        first["result"]["snapshot"]["binding"]
    );
    assert_eq!(retried["result"]["snapshot"]["identity"]["role"], "master");
    assert_eq!(
        events(&f)
            .iter()
            .filter(|event| event["ev"] == "GlobalMasterGranted")
            .count(),
        1
    );
    let second = register(&f, &b, "b");
    assert_eq!(
        second["result"]["snapshot"]["identity"]["role"], "worker",
        "{second}"
    );
    f.restart_daemon();
    assert_eq!(
        f.context_query("policy", None)["result"]["outcome"],
        "completed"
    );
    let restored = register(&f, &a, "a");
    assert_eq!(
        restored["result"]["snapshot"]["identity"]["role"], "master",
        "{restored}"
    );
}

fn events(f: &Fixture) -> Vec<Value> {
    std::fs::read_to_string(f.project_journal())
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

fn peer_intent(f: &Fixture, host: &TestAppServer, id: &str, op: &str) -> Value {
    let facts =
        json!({"session_id":id,"thread_id":id,"endpoint":host.endpoint(),"namespace":"codex_tui"})
            .to_string();
    f.run_ok(
        &[
            "context",
            "--op",
            op,
            "--provide",
            &facts,
            "--approve-grant",
            r#"{"action":"restore_peer","authorization":"user explicitly requests ordinary peer"}"#,
        ],
        None,
    )
}

#[test]
fn bootstrap_public_rejection_scope_and_absence() {
    let f = Fixture::without_tmux("bootstrap-rejections");
    for (op, approval, expected) in [
        (
            "missing-auth",
            json!({"action":"authorize_first_registered_peer"}),
            "BOOTSTRAP_AUTHORIZATION_REQUIRED",
        ),
        (
            "denial",
            json!({"action":"authorize_first_registered_peer","authorization":"user text","decision":"denied"}),
            "BOOTSTRAP_APPROVAL_NOT_APPROVED",
        ),
        (
            "wrong-project",
            json!({"action":"authorize_first_registered_peer","authorization":"user text","project_scope":"other"}),
            "BOOTSTRAP_APPROVAL_SCOPE_MISMATCH",
        ),
        (
            "wrong-app",
            json!({"action":"authorize_first_registered_peer","authorization":"user text","app_scope_id":"other"}),
            "BOOTSTRAP_APPROVAL_SCOPE_MISMATCH",
        ),
    ] {
        let output = f.command(
            &[
                "context",
                "--op",
                op,
                "--approve-grant",
                &approval.to_string(),
            ],
            None,
        );
        assert!(!output.status.success());
        assert!(
            String::from_utf8_lossy(&output.stderr).contains(expected),
            "{output:?}"
        );
    }
    let host = TestAppServer::start_with_effect_at(&f.root, "a.sock", "a", "a", "ack_only");
    assert_eq!(
        register(&f, &host, "a")["result"]["snapshot"]["identity"]["role"],
        "worker"
    );
    assert!(!events(&f)
        .iter()
        .any(|event| event["ev"] == "GlobalMasterGranted"));
}

#[test]
fn bootstrap_public_history_explicit_peer_and_consumed_clear() {
    for scenario in ["history", "explicit-peer", "clear", "incumbent"] {
        let mut f = Fixture::without_tmux(&format!("bootstrap-{scenario}"));
        let a = TestAppServer::start_with_effect_at(&f.root, "a.sock", "a", "a", "ack_only");
        if scenario == "history" {
            assert_eq!(
                register(&f, &a, "a")["result"]["snapshot"]["identity"]["role"],
                "worker"
            );
        }
        if scenario == "incumbent" {
            let facts = json!({"session_id":"a","thread_id":"a","endpoint":a.endpoint(),"namespace":"codex_tui"}).to_string();
            let existing = f.run_ok(&["context", "--op", "incumbent", "--provide", &facts,
                "--approve-grant", r#"{"action":"restore_master","authorization":"user authorizes existing master"}"#], None);
            assert_eq!(existing["result"]["snapshot"]["identity"]["role"], "master");
        }
        authorize(&f, "policy");
        if scenario == "explicit-peer" {
            assert_eq!(
                peer_intent(&f, &a, "a", "explicit-peer")["result"]["snapshot"]["identity"]["role"],
                "worker"
            );
        } else if scenario != "history" && scenario != "incumbent" {
            assert_eq!(
                register(&f, &a, "a")["result"]["snapshot"]["identity"]["role"],
                "master"
            );
            if scenario == "clear" {
                assert_eq!(
                    peer_intent(&f, &a, "a", "clear")["result"]["snapshot"]["identity"]["role"],
                    "worker"
                );
            }
        }
        // The incumbent host is offline; its grant must still block takeover.
        drop(a);
        f.restart_daemon();
        let b = TestAppServer::start_with_effect_at(&f.root, "b.sock", "b", "b", "ack_only");
        assert_eq!(
            register(&f, &b, "b")["result"]["snapshot"]["identity"]["role"],
            "worker"
        );
        let grants = events(&f)
            .iter()
            .filter(|event| event["ev"] == "GlobalMasterGranted")
            .count();
        assert_eq!(
            grants,
            usize::from(matches!(scenario, "clear" | "incumbent")),
            "{scenario}"
        );
    }
}

#[test]
fn bootstrap_public_concurrent_first_registration() {
    let f = Fixture::without_tmux("bootstrap-concurrent");
    authorize(&f, "policy");
    let a = TestAppServer::start_with_effect_at(&f.root, "a.sock", "a", "a", "ack_only");
    let b = TestAppServer::start_with_effect_at(&f.root, "b.sock", "b", "b", "ack_only");
    let mut children = Vec::new();
    for (id, host) in [("a", &a), ("b", &b)] {
        let facts = json!({"session_id":id,"thread_id":id,"endpoint":host.endpoint(),"namespace":"codex_tui"}).to_string();
        let mut command = f.configured_command(&["context", "--op", id, "--provide", &facts], None);
        children.push(
            command
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap(),
        );
    }
    let roles: Vec<String> = children
        .into_iter()
        .map(|child| {
            let output = child.wait_with_output().unwrap();
            assert!(output.status.success(), "{output:?}");
            let value: Value = serde_json::from_slice(&output.stdout).unwrap();
            value["result"]["snapshot"]["identity"]["role"]
                .as_str()
                .unwrap()
                .to_owned()
        })
        .collect();
    assert_eq!(roles.iter().filter(|role| *role == "master").count(), 1);
    assert_eq!(roles.iter().filter(|role| *role == "worker").count(), 1);
    assert_eq!(
        events(&f)
            .iter()
            .filter(|event| event["ev"] == "GlobalMasterGranted")
            .count(),
        1
    );
}

#[test]
fn bootstrap_public_unavailable_journal_rejects_registration() {
    let mut f = Fixture::without_tmux("bootstrap-durable-failure");
    authorize(&f, "policy");
    f.stop_daemon();
    f.hide_project_journal();
    f.start_daemon();
    let host = TestAppServer::start_with_effect_at(&f.root, "a.sock", "a", "a", "ack_only");
    let facts = json!({"session_id":"a","thread_id":"a","endpoint":host.endpoint(),"namespace":"codex_tui"}).to_string();
    let output = f.command(&["context", "--op", "a", "--provide", &facts], None);
    assert!(!output.status.success(), "{output:?}");
    f.restore_project_journal();
    assert!(!events(&f)
        .iter()
        .any(|event| event["ev"] == "GlobalMasterGranted"));
}

#[test]
fn bootstrap_public_incomplete_policy_command_fails_replay() {
    let mut f = Fixture::without_tmux("bootstrap-incomplete-policy");
    authorize(&f, "policy");
    f.stop_daemon();
    let path = f.project_journal();
    let rows = events(&f);
    let completion = rows
        .iter()
        .position(|event| {
            event["ev"] == "CommandCompleted" && event["command_id"] == "bootstrap-policy"
        })
        .expect("real policy command completion");
    // Model a lost completion after real producer business bytes synced.
    let prefix = rows[..completion]
        .iter()
        .map(|row| format!("{row}\n"))
        .collect::<String>();
    std::fs::write(&path, prefix).unwrap();
    f.restart_daemon();
    let host = TestAppServer::start_with_effect_at(&f.root, "a.sock", "a", "a", "ack_only");
    let facts = json!({"session_id":"a","thread_id":"a","endpoint":host.endpoint(),"namespace":"codex_tui"}).to_string();
    let output = f.command(&["context", "--op", "a", "--provide", &facts], None);
    assert!(
        !output.status.success(),
        "incomplete command must fail closed: {output:?}"
    );
    assert!(!events(&f)
        .iter()
        .any(|event| event["ev"] == "GlobalMasterGranted"));
}

/// Send one raw public wire envelope to the isolated daemon.
fn legacy_wire(f: &Fixture, request: Value) -> Value {
    let mut stream = UnixStream::connect(f.host_state.join("server.sock")).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    stream
        .write_all(serde_json::to_string(&request).unwrap().as_bytes())
        .unwrap();
    stream.write_all(b"\n").unwrap();
    stream.flush().unwrap();
    let mut line = String::new();
    BufReader::new(stream).read_line(&mut line).unwrap();
    serde_json::from_str(&line).unwrap()
}

/// Read the authoritative project journal for this fixture's explicit project
/// scope. Unreadable routes or journals and malformed records panic: missing or
/// broken evidence must never be mistaken for proof that no effect ran.
fn recorded_events(f: &Fixture) -> Vec<Value> {
    let project_scope = std::fs::canonicalize(&f.root)
        .expect("canonicalize fixture project root")
        .to_string_lossy()
        .into_owned();
    let routes_path = f.host_routes();
    let routes = std::fs::read_to_string(&routes_path)
        .unwrap_or_else(|error| panic!("read host routes {}: {error}", routes_path.display()));
    let storage_root = routes
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            serde_json::from_str::<Value>(line)
                .unwrap_or_else(|error| panic!("parse host route {line:?}: {error}"))
        })
        .find(|record| {
            record["op"] == "register" && record["project_scope"] == project_scope.as_str()
        })
        .and_then(|record| record["storage_root"].as_str().map(PathBuf::from))
        .unwrap_or_else(|| {
            panic!(
                "no registered project route for {project_scope}; cannot prove absence of effects"
            )
        });
    let journal_path = storage_root.join(PROJECT_JOURNAL);
    let journal = std::fs::read_to_string(&journal_path).unwrap_or_else(|error| {
        panic!(
            "read authoritative project journal {}: {error}",
            journal_path.display()
        )
    });
    journal
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            serde_json::from_str::<Value>(line)
                .unwrap_or_else(|error| panic!("parse project journal line {line:?}: {error}"))
        })
        .collect()
}

fn event_count(rows: &[Value], event: &str) -> usize {
    rows.iter().filter(|row| row["ev"] == event).count()
}

#[test]
fn bootstrap_public_legacy_wire_policy_without_operation_id_is_rejected() {
    let f = Fixture::without_tmux("bootstrap-legacy-wire");
    let host = TestAppServer::start_with_effect_at(&f.root, "a.sock", "a", "a", "ack_only");
    let facts = json!({"session_id":"a","thread_id":"a","endpoint":host.endpoint(),"namespace":"codex_tui"});
    let canonical_root = std::fs::canonicalize(&f.root).unwrap();
    // Establish the project's authoritative journal through the public policy
    // owner before the rejected request. A missing route or journal is then an
    // error, never evidence of absence, and the counts below prove that the
    // rejected request changed none of the registration, grant, or policy
    // state the successful producer already committed.
    assert_eq!(authorize(&f, "policy")["result"]["outcome"], "completed");
    let before = recorded_events(&f);
    assert_eq!(event_count(&before, "Registered"), 0);
    assert_eq!(event_count(&before, "GlobalRuntimeBound"), 0);
    assert_eq!(event_count(&before, "GlobalMasterGranted"), 0);
    assert_eq!(event_count(&before, "GlobalBootstrapPolicySet"), 1);
    let response = legacy_wire(
        &f,
        json!({
            "op": "IdentityContext",
            "identity_context": {
                "operation_id": "",
                "invocation": "approved_recovery",
                "action": "context",
                "facts": facts,
                "grant_approval": {
                    "action": "authorize_first_registered_peer",
                    "authorization": "user authorizes first registered peer as project master"
                },
                "query": false,
                "query_capability": "legacy-wire-policy"
            },
            "project_context": {
                "app_scope_id": "appserver-cli",
                "canonical_root": canonical_root.to_string_lossy(),
                "project_scope": canonical_root.to_string_lossy()
            }
        }),
    );
    assert!(
        !response["ok"].as_bool().unwrap_or(true),
        "a policy action without a durable operation id must not succeed as registration: {response}"
    );
    assert!(
        response["error"]
            .as_str()
            .unwrap_or_default()
            .starts_with("IDENTITY_OPERATION_ID_REQUIRED"),
        "missing durable operation identity must fail explicitly: {response}"
    );
    let after = recorded_events(&f);
    for event in [
        "Registered",
        "GlobalRuntimeBound",
        "GlobalMasterGranted",
        "GlobalBootstrapPolicySet",
    ] {
        assert_eq!(
            event_count(&after, event),
            event_count(&before, event),
            "policy request without an operation id must not register, grant, or consume policy: {event} {response}"
        );
    }
    // The rejected request also must not consume the pending policy or change
    // the ordinary legacy contract: a plain legacy context still registers and
    // becomes master under the still-pending producer authorization.
    let ordinary = legacy_wire(
        &f,
        json!({
            "op": "IdentityContext",
            "identity_context": {
                "operation_id": "",
                "invocation": "automatic",
                "action": "context",
                "facts": facts,
                "query": false
            },
            "project_context": {
                "app_scope_id": "appserver-cli",
                "canonical_root": canonical_root.to_string_lossy(),
                "project_scope": canonical_root.to_string_lossy()
            }
        }),
    );
    assert_eq!(ordinary["ok"], true, "{ordinary}");
    assert_eq!(
        ordinary["snapshot"]["identity"]["role"], "master",
        "{ordinary}"
    );
    let final_rows = recorded_events(&f);
    assert_eq!(event_count(&final_rows, "Registered"), 1);
    assert_eq!(event_count(&final_rows, "GlobalMasterGranted"), 1);
}
