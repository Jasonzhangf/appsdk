//! Real tmux adaptor acceptance through the public daemon protocol.
//! No shared endpoint, pane runner, model process or installed binary is used.
include!("support/context_operation_fixture.rs");

struct OwnedAnchor {
    socket: PathBuf,
    name: String,
    created: bool,
}

impl OwnedAnchor {
    fn start(root: &Path) -> Self {
        let socket = root.join("foreign-anchor.sock");
        assert!(!socket.exists());
        let mut owned = Self {
            socket,
            name: format!("foreign-anchor-20261115-{}", std::process::id()),
            created: false,
        };
        owned.run(&[
            "-f",
            "/dev/null",
            "new-session",
            "-d",
            "-s",
            &owned.name,
            "-c",
            root.to_str().unwrap(),
            "sleep 600",
        ]);
        owned.created = true;
        owned
    }

    fn run(&self, args: &[&str]) -> String {
        let output = Command::new("tmux")
            .arg("-S")
            .arg(&self.socket)
            .args(args)
            .env_remove("TMUX")
            .env_remove("TMUX_PANE")
            .output()
            .unwrap();
        assert!(output.status.success(), "owned tmux {args:?}: {output:?}");
        String::from_utf8(output.stdout).unwrap().trim().to_owned()
    }

    fn endpoint(&self, session: &str, thread: &str) -> Value {
        let observed = self.run(&[
            "display-message",
            "-p",
            "-t",
            &self.name,
            "#{pid}|#{session_id}|#{pane_id}|#{pane_pid}",
        ]);
        let fields: Vec<_> = observed.split('|').collect();
        assert_eq!(fields.len(), 4);
        json!({"socket_path":self.socket,"server_pid":fields[0].parse::<u32>().unwrap(),
            "tmux_session_id":fields[1],"pane_id":fields[2],"pane_pid":fields[3].parse::<u32>().unwrap(),
            "codex_session_id":session,"codex_thread_id":thread})
    }
}

impl Drop for OwnedAnchor {
    fn drop(&mut self) {
        if !self.created {
            return;
        }
        // The sole cleanup target is the name created above on this new socket.
        // No server shutdown, pane termination or session inventory cleanup.
        let output = Command::new("tmux")
            .arg("-S")
            .arg(&self.socket)
            .args(["kill-session", "-t", &self.name])
            .output();
        if !std::thread::panicking() {
            assert!(
                output.unwrap().status.success(),
                "clean owned named session"
            );
        }
    }
}

fn wire(fixture: &Fixture, context: &Value, mut request: Value) -> Value {
    request["project_context"] = context.clone();
    let mut stream = UnixStream::connect(fixture.host_state.join("server.sock")).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    writeln!(stream, "{request}").unwrap();
    let mut line = String::new();
    BufReader::new(stream).read_line(&mut line).unwrap();
    serde_json::from_str(&line).unwrap()
}

fn ok(value: Value) -> Value {
    assert_eq!(value["ok"], true, "{value}");
    value
}

fn context(root: &Path, app: &str) -> Value {
    let root = root.canonicalize().unwrap();
    json!({"project_scope":root,"canonical_root":root,"app_scope_id":app})
}

fn bound_context(root: &Path, app: &str, binding: &Value) -> Value {
    let mut value = context(root, app);
    value["runtime_context"] = json!({"agent_id":binding["agent_id"],
        "runtime_id":binding["runtime_id"],"appserver_id":binding["app_scope_id"],
        "binding_id":binding["binding_id"],"session_id":binding["session_id"],
        "native_thread_id":binding["native_thread_id"]});
    value
}

fn read(path: &Path) -> Value {
    serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
}

fn mapping(fixture: &Fixture, project: &Path, app: &str, worker: &str) -> Value {
    let map = read(&fixture.host_state.join("identity-map.json"));
    let project = project.canonicalize().unwrap();
    let slots: Vec<_> = map["slots"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|slot| {
            slot["identity_status"] == "active"
                && slot["identity_key"]["project_scope"] == json!(project)
                && slot["identity_key"]["app_scope_id"] == app
                && slot["current"]["binding"]["agent_id"] == worker
        })
        .collect();
    assert_eq!(slots.len(), 1, "one current owner in exact project/app");
    slots[0]["current"].clone()
}

#[test]
fn foreign_physical_anchor_public_transfer_preserves_state_and_fences_old_carrier() {
    scenario(false, false, false);
}

#[test]
fn foreign_physical_anchor_transfer_is_independent_of_daemon_and_project_cwd() {
    scenario(false, true, false);
}

#[test]
fn foreign_physical_anchor_missing_session_is_non_mutating() {
    scenario(true, false, false);
}

#[test]
fn foreign_anchor_with_master_grant_same_project() {
    scenario(false, false, true);
}

#[test]
fn foreign_anchor_with_master_grant_cross_project() {
    scenario(false, true, true);
}

#[test]
fn foreign_anchor_with_master_grant_rejected_replacement_preserves_authority() {
    scenario(true, true, true);
}

fn grants(fixture: &Fixture, project: &Path, app: &str) -> Value {
    let state = fixture.current_state_for(project, app);
    let grants: Vec<_> = state["global"]["projects"]
        .as_object()
        .unwrap()
        .values()
        .flat_map(|project| {
            project["master_grants"]
                .as_object()
                .unwrap()
                .values()
                .cloned()
        })
        .collect();
    json!(grants)
}

fn promote(fixture: &Fixture, project: &Path, app: &str, worker: &str, token: &str) -> Value {
    let current = mapping(fixture, project, app, worker);
    let receipt = ok(wire(
        fixture,
        &bound_context(project, app, &current["binding"]),
        json!({"op":"MasterPromote","worker_id":worker,"token":token,
            "approval":"User-authorized isolated foreign-anchor master-grant acceptance"}),
    ));
    assert_eq!(receipt["master"], worker, "{receipt}");
    let current = mapping(fixture, project, app, worker);
    let typed = grants(fixture, project, app);
    assert_eq!(typed.as_array().unwrap().len(), 1, "{typed}");
    assert_eq!(typed[0]["agent_id"], worker);
    assert_eq!(typed[0]["binding_id"], current["binding"]["binding_id"]);
    assert_eq!(
        typed[0]["project_scope"],
        json!(project.canonicalize().unwrap())
    );
    assert_eq!(typed[0]["app_scope_id"], app);
    assert_eq!(typed[0]["granted_by"], "user");
    assert!(!typed[0]["approval"].as_str().unwrap().is_empty());
    let map = read(&fixture.host_state.join("identity-map.json"));
    assert!(map["slots"]
        .as_array()
        .unwrap()
        .iter()
        .any(|slot| slot["identity_key"]["name"] == "Jenny" && slot["current"] == current));
    let status = ok(wire(
        fixture,
        &bound_context(project, app, &current["binding"]),
        json!({"op":"MasterStatus"}),
    ));
    assert_eq!(status["master"]["worker_id"], worker, "{status}");
    eprintln!("AUTHORIZED_MASTER {}", typed);
    current
}

fn scenario(failure_control: bool, cross_project: bool, existing_grant: bool) {
    let mut fixture = Fixture::without_tmux("foreign-anchor-20261115");
    // The daemon cwd and request project differ. Neither is inferred from the other.
    let project = fixture.root.join("requested-project");
    seed_baseline(&project);
    let target_project = if cross_project {
        let target = fixture.root.join("replacement-project");
        seed_baseline(&target);
        target
    } else {
        project.clone()
    };
    let anchor = OwnedAnchor::start(&project);
    std::fs::write(
        fixture.host_state.join("tmux-endpoints.json"),
        json!({"endpoints":[anchor.socket]}).to_string(),
    )
    .unwrap();
    let old_endpoint = anchor.endpoint("prior-carrier", "prior-thread");
    let old_context = context(&project, "foreign-prior-app");
    ok(wire(
        &fixture,
        &old_context,
        json!({"op":"Register","worker_id":"prior-owner",
        "token":"private-test-token","cwd":project,
        "candidates":{"tmux":{"endpoint":old_endpoint,"cwd":project}}}),
    ));
    // A second real holder proves that exact source revocation is isolated.
    // Keep its guard alive until all assertions and replay have completed.
    let unrelated = if existing_grant {
        let root = fixture.root.join("unrelated-project");
        seed_baseline(&root);
        let owned = OwnedAnchor::start(&root);
        ok(wire(
            &fixture,
            &context(&root, "unrelated-app"),
            json!({"op":"Register","worker_id":"unrelated-owner","token":"unrelated-test-token",
                "cwd":root,"candidates":{"tmux":{"endpoint":owned.endpoint("unrelated-carrier", "unrelated-thread"),"cwd":root}}}),
        ));
        promote(
            &fixture,
            &root,
            "unrelated-app",
            "unrelated-owner",
            "unrelated-test-token",
        );
        let baseline = grants(&fixture, &root, "unrelated-app");
        Some((owned, root, baseline))
    } else {
        None
    };
    let old = if existing_grant {
        promote(
            &fixture,
            &project,
            "foreign-prior-app",
            "prior-owner",
            "private-test-token",
        )
    } else {
        mapping(&fixture, &project, "foreign-prior-app", "prior-owner")
    };
    let source_name = if existing_grant { "Jenny" } else { "Alice" };
    let source_grants = existing_grant.then(|| grants(&fixture, &project, "foreign-prior-app"));
    let source_map = read(&fixture.host_state.join("identity-map.json"));
    let source_generation = source_map["slots"]
        .as_array()
        .unwrap()
        .iter()
        .find(|slot| slot["current"] == old)
        .unwrap()["generation"]
        .as_u64()
        .unwrap();
    let old_binding = &old["binding"];
    assert_eq!(old_binding["session_id"], "prior-carrier");
    let old_context = bound_context(&project, "foreign-prior-app", old_binding);
    ok(wire(
        &fixture,
        &old_context,
        json!({"op":"TaskRegister","worker_id":"prior-owner",
        "token":"private-test-token","task_id":"preserved-task","priority":"p1",
        "next_step":"retain responsibility through anchor replacement"}),
    ));
    let message = ok(wire(
        &fixture,
        &old_context,
        json!({"op":"Send", "from":"prior-owner",
        "worker_id":"prior-owner","token":"private-test-token",
        "command":{"command_id":"preserved-message-command","operation_id":"preserved-message-operation",
            "actor_binding_id":old_binding["binding_id"],
            "scope":{"project_scope_id":old_binding["project_scope"],"app_scope_id":"foreign-prior-app"}},
        "to":"prior-owner","type":"notify","subject":"anchor continuity",
        "body":"retain original mailbox content","delivery":"queued"}),
    ));
    let message_id = message["msg_id"].as_str().unwrap();
    let old_state = fixture.current_state_for(&project, "foreign-prior-app");
    assert!(!old_state["msgs"][message_id].is_null());

    let before = fixture.current_resources();
    let next_context = context(&target_project, "appserver-cli");
    let mut facts = json!({"caller_pid":old_endpoint["pane_pid"],
        "session_id":"replacement-carrier","thread_id":"replacement-thread",
        "pane_hint":old_endpoint["pane_id"]});
    if failure_control {
        facts.as_object_mut().unwrap().remove("session_id");
    }
    let response = wire(
        &fixture,
        &next_context,
        json!({"op":"IdentityContext","facts":facts}),
    );
    if failure_control {
        assert_eq!(response["result"]["outcome"], "missing_facts", "{response}");
        assert!(
            response["result"]["requires"]["fields"]
                .as_array()
                .unwrap()
                .contains(&json!("session_id")),
            "{response}"
        );
        assert_eq!(
            fixture.current_resources(),
            before,
            "no partial publication or state mutation"
        );
        let mut missing_pane = old_endpoint.clone();
        missing_pane["pane_id"] = json!("%99999999");
        missing_pane["codex_session_id"] = json!("failed-carrier");
        missing_pane["codex_thread_id"] = json!("failed-thread");
        let rejected = wire(
            &fixture,
            &context(&project, "foreign-prior-app"),
            json!({"op":"Register","worker_id":"failed-owner","token":"failed-token",
                "cwd":project,"candidates":{"tmux":{"endpoint":missing_pane,"cwd":project}}}),
        );
        assert_eq!(rejected["ok"], false, "{rejected}");
        assert!(
            rejected["error"]
                .as_str()
                .unwrap()
                .contains("TRANSPORT_NONE"),
            "{rejected}"
        );
        assert_eq!(
            fixture.current_resources(),
            before,
            "real missing-pane probe has no partial effects"
        );
        let lookup = ok(wire(
            &fixture,
            &old_context,
            json!({"op":"RouteResolveNative","session_id":"prior-carrier","native_thread_id":"prior-thread"}),
        ));
        assert_eq!(lookup["agent_id"], "prior-owner", "{lookup}");
        if let Some(expected) = &source_grants {
            assert_eq!(&grants(&fixture, &project, "foreign-prior-app"), expected);
            let status = ok(wire(&fixture, &old_context, json!({"op":"MasterStatus"})));
            assert_eq!(status["master"]["worker_id"], "prior-owner", "{status}");
            eprintln!("NEGATIVE_SOURCE_MASTER {}", expected);
        }
        if let Some((_, root, expected)) = &unrelated {
            assert_eq!(&grants(&fixture, root, "unrelated-app"), expected);
        }
        eprintln!("REJECTED_REPLACEMENT {} {}", response, rejected);
        return;
    }
    let response = ok(response);
    let snapshot = &response["snapshot"];
    let binding = &snapshot["binding"];
    assert_eq!(binding["session_id"], "replacement-carrier", "{response}");
    assert_eq!(binding["native_thread_id"], "replacement-thread");
    let worker = binding["agent_id"].as_str().unwrap();
    let current = mapping(&fixture, &target_project, "appserver-cli", worker);
    assert_eq!(&current["binding"], binding);
    let map = read(&fixture.host_state.join("identity-map.json"));
    let owners: Vec<_> = map["slots"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|slot| {
            slot["identity_status"] == "active"
                && !slot["current"].is_null()
                && slot["current"]["binding"]["tmux_endpoint"]["socket_path"]
                    == json!(anchor.socket)
                && slot["current"]["binding"]["tmux_endpoint"]["pane_id"] == old_endpoint["pane_id"]
        })
        .collect();
    assert_eq!(owners.len(), 1, "one active physical owner: {map}");
    assert_eq!(owners[0]["current"]["binding"], *binding);
    let released = map["slots"]
        .as_array()
        .unwrap()
        .iter()
        .find(|slot| {
            slot["identity_key"]["project_scope"] == old_binding["project_scope"]
                && slot["identity_key"]["app_scope_id"] == "foreign-prior-app"
                && slot["identity_key"]["name"] == source_name
        })
        .unwrap();
    assert!(released["current"].is_null());
    assert_eq!(released["previous"]["binding"], *old_binding);
    assert_eq!(released["generation"], source_generation + 1);
    assert!(
        map["effects"].as_array().unwrap().is_empty(),
        "transfer effects consumed"
    );
    let state = fixture.current_state_for(&project, "foreign-prior-app");
    assert_eq!(
        state["tasks"]["preserved-task"],
        old_state["tasks"]["preserved-task"]
    );
    for field in ["id", "from", "to", "body", "subject", "state", "created_ms"] {
        assert_eq!(
            state["msgs"][message_id][field], old_state["msgs"][message_id][field],
            "message {field}"
        );
    }
    // Readable old business responsibility remains in its original runtime.
    let task = ok(wire(
        &fixture,
        &context(&project, "foreign-prior-app"),
        json!({"op":"TaskStatus","task_id":"preserved-task"}),
    ));
    assert_eq!(task["owner_name"], source_name, "{task}");
    assert_eq!(task["created_by"], "prior-owner");
    let source_task = task.clone();
    assert_eq!(
        task["status"],
        old_state["tasks"]["preserved-task"]["status"]
    );
    let stale = wire(
        &fixture,
        &old_context,
        json!({"op":"TaskRegister","worker_id":"prior-owner",
        "token":"private-test-token","task_id":"forbidden-old-carrier"}),
    );
    assert_eq!(stale["ok"], false, "{stale}");
    assert!(
        fixture.current_state_for(&project, "foreign-prior-app")["tasks"]["forbidden-old-carrier"]
            .is_null()
    );
    let next_context = bound_context(&target_project, "appserver-cli", binding);
    if existing_grant {
        assert_eq!(grants(&fixture, &project, "foreign-prior-app"), json!([]));
        assert_eq!(
            grants(&fixture, &target_project, "appserver-cli"),
            json!([])
        );
        for scope in [&old_context, &next_context] {
            let status = ok(wire(&fixture, scope, json!({"op":"MasterStatus"})));
            assert!(status["master"].is_null(), "{status}");
        }
    }
    let lookup = ok(wire(
        &fixture,
        &next_context,
        json!({"op":"RouteResolveNative",
        "session_id":"replacement-carrier","native_thread_id":"replacement-thread"}),
    ));
    assert_eq!(lookup["agent_id"], worker, "{lookup}");
    assert_eq!(
        lookup["project_scope"],
        json!(target_project.canonicalize().unwrap())
    );
    assert_eq!(lookup["binding_id"], binding["binding_id"]);
    assert_eq!(lookup["app_scope_id"], "appserver-cli");
    let physical = ok(wire(
        &fixture,
        &json!(null),
        json!({"op":"RouteResolve",
        "tmux_endpoint":anchor.endpoint("replacement-carrier", "replacement-thread")}),
    ));
    assert_eq!(physical["agent_id"], worker, "{physical}");
    let stale_lookup = wire(
        &fixture,
        &old_context,
        json!({"op":"RouteResolveNative",
        "session_id":"prior-carrier","native_thread_id":"prior-thread"}),
    );
    assert_eq!(stale_lookup["ok"], false, "{stale_lookup}");
    assert!(stale_lookup["error"]
        .as_str()
        .unwrap()
        .contains("ROUTE_RESOLVE_NOT_FOUND"));
    let new_state = fixture.current_state_for(&target_project, "appserver-cli");
    let token = new_state["workers"][worker]["token"].as_str().unwrap();
    ok(wire(
        &fixture,
        &next_context,
        json!({"op":"TaskRegister","worker_id":worker,
        "token":token,"task_id":"replacement-operation"}),
    ));
    let task = ok(wire(
        &fixture,
        &next_context,
        json!({"op":"TaskStatus","task_id":"replacement-operation"}),
    ));
    assert_eq!(task["owner"], worker);
    let restored = ok(wire(
        &fixture,
        &context(&target_project, "appserver-cli"),
        json!({"op":"IdentityContext",
        "facts":{"session_id":"replacement-carrier","thread_id":"replacement-thread"}}),
    ));
    assert_eq!(restored["snapshot"]["binding"], *binding);
    assert_eq!(restored["snapshot"]["name"], snapshot["name"]);
    assert!(
        restored["snapshot"]["master"].is_null(),
        "replacement peer must not acquire master"
    );
    if existing_grant {
        // Neither a fresh peer nor a stale former master can issue authority.
        for (scope, actor, credential) in [
            (&next_context, worker, token),
            (&old_context, "prior-owner", "private-test-token"),
        ] {
            let denied = wire(
                &fixture,
                scope,
                json!({"op":"MasterDelegate",
                "worker_id":actor,"token":credential,"target_id":actor,
                "approval":"isolated negative authorization control"}),
            );
            assert_eq!(denied["ok"], false, "{denied}");
            let error = denied["error"].as_str().unwrap();
            if actor == worker {
                assert_eq!(
                    error,
                    "MASTER_DELEGATE_DENIED: caller must hold the current master grant"
                );
            } else {
                assert!(
                    error.starts_with("PROJECT_ROUTE_NOT_READY/UNSUPPORTED:"),
                    "{denied}"
                );
            }
            eprintln!("AUTHORITY_DENIED {}", denied);
        }
        assert_eq!(grants(&fixture, &project, "foreign-prior-app"), json!([]));
        assert_eq!(
            grants(&fixture, &target_project, "appserver-cli"),
            json!([])
        );
    }
    assert_eq!(
        anchor.run(&[
            "display-message",
            "-p",
            "-t",
            &anchor.name,
            "#{session_name}"
        ]),
        anchor.name
    );
    let map_before_restart = read(&fixture.host_state.join("identity-map.json"));
    if let Some((_, root, expected)) = &unrelated {
        assert_eq!(&grants(&fixture, root, "unrelated-app"), expected);
    }
    if existing_grant {
        assert_ne!(snapshot["name"], "Jenny");
        eprintln!(
            "PUBLIC_REPLACEMENT_RECEIPT {}",
            json!({
                "source_name":source_name,"target_name":snapshot["name"],
                "binding":binding,"native_route":lookup,"physical_route":physical,
                "old_route_denied":stale_lookup,"old_mutation_denied":stale,
                "source_task_status":source_task,
                "message_id":message_id,"source_grants":grants(&fixture, &project, "foreign-prior-app"),
                "target_grants":grants(&fixture, &target_project, "appserver-cli")
            })
        );
    }
    fixture.restart_daemon();
    assert_eq!(
        read(&fixture.host_state.join("identity-map.json")),
        map_before_restart
    );
    let replayed = ok(wire(
        &fixture,
        &next_context,
        json!({"op":"RouteResolveNative",
        "session_id":"replacement-carrier","native_thread_id":"replacement-thread"}),
    ));
    assert_eq!(replayed["agent_id"], worker);
    assert_eq!(replayed["binding_id"], binding["binding_id"]);
    let retained = fixture.current_state_for(&project, "foreign-prior-app");
    assert_eq!(
        retained["tasks"]["preserved-task"],
        old_state["tasks"]["preserved-task"]
    );
    assert_eq!(
        retained["msgs"][message_id]["body"],
        "retain original mailbox content"
    );
    if existing_grant {
        assert_eq!(grants(&fixture, &project, "foreign-prior-app"), json!([]));
        assert_eq!(
            grants(&fixture, &target_project, "appserver-cli"),
            json!([])
        );
        eprintln!(
            "REPLACEMENT_CURRENT {} SOURCE_RELEASED {}",
            binding, released
        );
    }
    if let Some((_, root, expected)) = &unrelated {
        assert_eq!(&grants(&fixture, root, "unrelated-app"), expected);
        let status = ok(wire(
            &fixture,
            &context(root, "unrelated-app"),
            json!({"op":"MasterStatus"}),
        ));
        assert_eq!(status["master"]["worker_id"], "unrelated-owner", "{status}");
        eprintln!("UNRELATED_MASTER_RETAINED {}", expected);
    }
}
