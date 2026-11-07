use super::*;

const NEXT_SESSION: &str = "01a0d641-13f1-7b31-8d84-530f0f2d3fdd";

fn setup() -> (AppFixture, String, String) {
    let fixture = AppFixture::new();
    let alice = fixture.run_public(&["context"], THREAD_A);
    let bob = fixture.run_public(&["context"], THREAD_B);
    assert_eq!(alice["name"], "Alice");
    assert_eq!(bob["name"], "Bob");
    let jenny = fixture.run_public(
        &[
            "context",
            "--approve-grant",
            r#"{"action":"restore_master","authorization":"isolated T8 Jenny restoration"}"#,
        ],
        THREAD_A,
    );
    assert_eq!(jenny["name"], "Jenny");
    (
        fixture,
        alice["identity"]["worker_id"].as_str().unwrap().into(),
        bob["identity"]["worker_id"].as_str().unwrap().into(),
    )
}

fn publish_and_claim(fixture: &AppFixture, id: &str) {
    fixture.run_public(
        &[
            "board",
            "publish",
            id,
            "--title",
            id,
            "--description",
            "T8 consumer",
            "--delivery-condition",
            "name continues work",
            "--test-condition",
            "public CLI assertions",
        ],
        THREAD_A,
    );
    let claimed = fixture.run_public(
        &["board", "claim", id, "--expected-revision", "1"],
        THREAD_B,
    );
    assert_eq!(claimed["task"]["owner_name"], "Bob");
    assert_eq!(claimed["task"]["owner_session_id"], THREAD_B);
}

fn task<'a>(board: &'a Value, id: &str) -> &'a Value {
    board["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|task| task["id"] == id)
        .unwrap()
}

#[test]
fn multiple_open_tasks_follow_current_session_and_preserve_closed_history() {
    let (mut fixture, master, peer) = setup();
    let scope = std::fs::canonicalize(&fixture.project).unwrap();
    let mut records = Vec::new();
    for (id, status) in [
        ("t8-working", "working"),
        ("t8-accepted", "accepted"),
        ("t8-closed", "closed"),
    ] {
        publish_and_claim(&fixture, id);
        let worktree = fixture.root.join(id);
        // Lifecycle evidence/resources are pre-existing inputs. This consumer
        // tests name continuation, not delivery/review/closure admission.
        records.push((id, status, worktree));
    }
    fixture.update_current_state(|state| {
        for (id, status, worktree) in records {
            state["tasks"][id] = json!({
                "id":id,"owner":peer,"created_by":master,"status":status,"priority":"p2",
                "feature_id":"t8-feature","worktree_path":worktree,"branch":"codex/t8",
                "base_commit":"t8-base","next_step":"retain this work","created_ms":1,"updated_ms":2
            });
            state["task_lifecycle"][id] = json!({
                "delivery_evidence":"T8 retained delivery","delivered_ms":2,"delivery_commit":"t8-candidate",
                "review_evidence":"T8 retained review","reviewer":master,"reviewed_ms":2
            });
            state["worktree_bindings"][format!("binding-{id}")] = json!({
                "worktree_root":worktree,"owning_project_scope":scope,"task_id":id,
                "owner_agent_id":peer,"binding_id":format!("binding-{id}"),"base_commit":"t8-base"
            });
        if status != "closed" {
            state["pending_merges"][id] = json!({
                "task_id":id,"owner":peer,"requested_by":master,"requested_ms":2,"candidate_commit":"t8-candidate"
            });
        }
    }
    });
    let closed_assignment = fixture.current_state()["task_assignments"]["t8-closed"].clone();
    let before_board = fixture.run_public(&["board", "show"], THREAD_A);
    let before: Vec<_> = ["t8-working", "t8-accepted", "t8-closed"]
        .iter()
        .map(|id| fixture.run_public(&["task", "status", id], THREAD_A))
        .collect();

    // A new native carrier needs explicit name authorization. Its thread is
    // a communication address and cannot select an old identity by itself.
    let fresh = fixture.run_public(&["context"], NEXT_SESSION);
    assert_ne!(fresh["name"], "Bob");
    let output = fixture.command_public(&["context", "--approve-grant",
        r#"{"action":"restore_name","requested_name":"Bob","authorization":"user continues Bob work on the new carrier"}"#], NEXT_SESSION);
    assert!(
        output.status.success(),
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let receipt: Value = serde_json::from_slice(&output.stdout).unwrap();
    let context = &receipt["result"]["snapshot"];
    assert_eq!(context["name"], "Bob");
    assert_eq!(
        context["identity"]["worker_id"],
        fresh["identity"]["worker_id"]
    );
    assert_eq!(context["session_id"], NEXT_SESSION);
    assert_eq!(context["binding"]["session_id"], NEXT_SESSION);

    let board = fixture.run_public(&["board", "show"], THREAD_A);
    for (index, id) in ["t8-working", "t8-accepted", "t8-closed"]
        .iter()
        .enumerate()
    {
        let durable = fixture.run_public(&["task", "status", id], THREAD_A);
        for key in [
            "id",
            "status",
            "feature_id",
            "worktree",
            "branch",
            "delivery",
            "review",
        ] {
            assert_eq!(durable[key], before[index][key], "{id}: durable {key}");
        }
        if *id == "t8-closed" {
            for view in [&before_board, &board] {
                assert!(!view["tasks"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|row| row["id"] == *id));
            }
            assert_eq!(
                durable, before[index],
                "closed durable history must remain unchanged"
            );
            continue;
        }
        assert_eq!(durable["owner"], fresh["identity"]["worker_id"]);
        let old = task(&before_board, id);
        let current = task(&board, id);
        for key in [
            "id",
            "owner_name",
            "status",
            "next_step",
            "delivery_evidence",
            "review_evidence",
            "cleanup_status",
        ] {
            assert_eq!(current[key], old[key], "{id}: board {key}");
        }
        assert_eq!(current["owner"], fresh["identity"]["worker_id"]);
        assert_eq!(current["cleanup_status"], "pending");
        assert_eq!(current["owner_session_id"], NEXT_SESSION);
        assert_eq!(current["needs_reassignment"], false);
        let mut expected_merge = before[index]["merge"].clone();
        expected_merge["owner"] = fresh["identity"]["worker_id"].clone();
        assert_eq!(
            durable["merge"], expected_merge,
            "{id}: current merge owner and retained request"
        );
    }
    let state = fixture.current_state();
    for id in ["t8-working", "t8-accepted"] {
        let assignment = &state["task_assignments"][id];
        assert_eq!(assignment["session_id"], NEXT_SESSION);
    }
    assert_eq!(state["task_assignments"]["t8-closed"], closed_assignment);
}

#[test]
fn unavailable_holder_reflows_once_to_jenny_and_board_reads_do_not_write() {
    let (mut fixture, master, peer) = setup();
    publish_and_claim(&fixture, "t8-orphan");
    let project = fixture
        .project
        .canonicalize()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    fixture.update_current_state(|state| {
        state["agent_presence"][&project][peer.as_str()] = json!({
            "project_scope":project,
            "worker_id":peer,"session_id":THREAD_B,"presence":"offline",
            "reason":"fixture current agent exited","observed_ms":1
        })
    });
    let received = fixture.run_public(&["recv", "--timeout", "5"], THREAD_A);
    let messages: Vec<_> = received["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|message| {
            message["body"]
                .as_str()
                .unwrap_or_default()
                .contains("t8-orphan")
        })
        .collect();
    assert_eq!(messages.len(), 1, "{received}");
    assert_eq!(messages[0]["to"], master);
    let body = messages[0]["body"].as_str().unwrap();
    assert!(
        body.contains("owner_offline")
            && body.contains("重新 assign")
            && body.contains("Jenny 不是执行者"),
        "{body}"
    );
    // Let delivery bookkeeping settle before comparing board-read effects.
    let no_repeat = fixture.run_public(&["recv", "--timeout", "2"], THREAD_A);
    assert_eq!(no_repeat["count"], 0, "{no_repeat}");
    let state = fixture.current_state();
    let protected = json!({"task":state["tasks"]["t8-orphan"], "assignment":state["task_assignments"]["t8-orphan"]});
    let map = std::fs::read(fixture.host_state.join("identity-map.json")).unwrap();
    for _ in 0..2 {
        let board = fixture.run_public(&["board", "show"], THREAD_A);
        let orphan = task(&board, "t8-orphan");
        assert_eq!(orphan["owner"], "");
        assert_eq!(orphan["owner_name"], "Bob");
        assert!(orphan["owner_session_id"].is_null());
        assert_eq!(orphan["needs_reassignment"], true);
        assert_eq!(orphan["reassignment_reason"], "owner_offline");
    }
    let state = fixture.current_state();
    assert_eq!(
        json!({"task":state["tasks"]["t8-orphan"], "assignment":state["task_assignments"]["t8-orphan"]}),
        protected
    );
    assert_eq!(
        std::fs::read(fixture.host_state.join("identity-map.json")).unwrap(),
        map
    );
    let durable = fixture.run_public(&["task", "status", "t8-orphan"], THREAD_A);
    assert_eq!(durable["owner"], peer, "Jenny must not become executor");
}
