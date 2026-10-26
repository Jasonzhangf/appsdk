use super::*;

const THREAD_C: &str = "01a0d641-13f1-7b31-8d84-530f0f2d3fdd";

fn setup() -> (AppFixture, String, String) {
    let mut fixture = AppFixture::new();
    let first = fixture.run_public(&["context"], THREAD_A);
    let second = fixture.run_public(&["context"], THREAD_B);
    fixture.run_public(
        &[
            "master",
            "promote",
            "--approval",
            "isolated dashboard fixture approval",
        ],
        THREAD_A,
    );
    (
        fixture,
        first["identity"]["worker_id"].as_str().unwrap().into(),
        second["identity"]["worker_id"].as_str().unwrap().into(),
    )
}

fn publish(fixture: &AppFixture, id: &str) -> Value {
    fixture.run_public(
        &[
            "board",
            "publish",
            id,
            "--title",
            "任务板接口",
            "--description",
            "公开协议测试",
            "--delivery-condition",
            "接口行为通过",
            "--test-condition",
            "cargo test --test appserver_two_tui_integration",
            "--priority",
            "p1",
        ],
        THREAD_A,
    )
}

fn board_journal_path(fixture: &AppFixture) -> PathBuf {
    let project = std::fs::canonicalize(&fixture.project).unwrap();
    let record = std::fs::read_to_string(fixture.host_state.join("routes.jsonl"))
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .find(|row| row["project_scope"] == project.to_string_lossy().as_ref())
        .expect("project route record");
    PathBuf::from(record["storage_root"].as_str().unwrap())
        .join(".agent-collab/server/journal.jsonl")
}

fn append_board_events(fixture: &mut AppFixture, events: &[Value]) {
    fixture.stop_daemon();
    let journal = board_journal_path(fixture);
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(&journal)
        .expect("open project journal");
    for event in events {
        writeln!(file, "{}", serde_json::to_string(event).unwrap()).unwrap();
    }
    file.sync_all().unwrap();
    fixture.start_daemon();
}

#[test]
fn unclosed_task_with_retired_owner_is_visible_without_an_assignee() {
    let (mut fixture, master, peer) = setup();
    publish(&fixture, "orphaned-open-task");
    let claimed = fixture.run_public(
        &[
            "board",
            "claim",
            "orphaned-open-task",
            "--expected-revision",
            "1",
        ],
        THREAD_B,
    );
    assert_eq!(claimed["task"]["owner"], peer);
    fixture.stop_daemon();
    let project = std::fs::canonicalize(&fixture.project).unwrap();
    let record = std::fs::read_to_string(fixture.host_state.join("routes.jsonl"))
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .find(|row| row["project_scope"] == project.to_string_lossy().as_ref())
        .unwrap();
    let journal = PathBuf::from(record["storage_root"].as_str().unwrap())
        .join(".agent-collab/server/journal.jsonl");
    // Start the read-only consumer from a retained task whose owner has
    // already retired. Retirement admission itself is not under test here.
    let retired = json!({"ev": "WorkerClosed", "worker_id": peer,
        "closed_by": master, "reason": "owned fixture retired identity",
        "snapshot_captured_ms": null, "at_ms": 1});
    use std::io::Write;
    writeln!(
        std::fs::OpenOptions::new()
            .append(true)
            .open(&journal)
            .unwrap(),
        "{retired}"
    )
    .unwrap();
    let task_records = |path: &std::path::Path| -> Vec<Value> {
        std::fs::read_to_string(path)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .filter(|row| {
                row["task"]["id"] == "orphaned-open-task" || row["task_id"] == "orphaned-open-task"
            })
            .collect()
    };
    let before = task_records(&journal);
    fixture.start_daemon();
    let board = fixture.run_public(&["board", "show"], THREAD_A);
    let task = board["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|task| task["id"] == "orphaned-open-task")
        .unwrap();
    assert_eq!(task["owner"], "", "{task}");
    assert_eq!(task["owner_display_name"], "", "{task}");
    assert_eq!(task["publisher"], master);
    assert_eq!(task["status"], "working");
    assert_eq!(task["revision"], claimed["task"]["revision"]);
    assert_eq!(
        task_records(&journal),
        before,
        "board must preserve task ownership and history"
    );
}

fn role_patrol_text(context: &Value) -> String {
    let brief = &context["role_brief"];
    let responsibilities = brief["responsibilities"]
        .as_array()
        .expect("role_brief responsibilities are explicit")
        .iter()
        .filter_map(Value::as_str);
    let next_action = brief["next_action"]
        .as_str()
        .expect("role_brief next_action is explicit");
    responsibilities
        .chain(std::iter::once(next_action))
        .collect::<Vec<_>>()
        .join("\n")
}

fn assert_role_patrol_mentions(label: &str, context: &Value, role: &str, keywords: &[&str]) {
    assert_eq!(context["role_brief"]["role"], role, "{label}: {context}");
    let text = role_patrol_text(context);
    let haystack = text.to_ascii_lowercase();
    for keyword in keywords {
        assert!(
            haystack.contains(keyword.to_ascii_lowercase().as_str()),
            "{label}: role_brief responsibilities/next_action must mention {keyword:?}: {text}"
        );
    }
}

fn notification_subscription_count(fixture: &AppFixture, thread: &str, label: &str) -> usize {
    let status = fixture.run_public(&["notify", "status"], thread);
    let subscriptions = status["subscriptions"]
        .as_array()
        .unwrap_or_else(|| panic!("{label}: notify status lists subscriptions: {status}"));
    subscriptions.len()
}

fn assert_no_serialized_role_control(label: &str, value: &Value) {
    match value {
        Value::Object(map) => {
            for (key, child) in map {
                let normalized = key.to_ascii_lowercase();
                assert!(
                    !matches!(
                        normalized.as_str(),
                        "authority"
                            | "derivation"
                            | "token"
                            | "role_brief"
                            | "managed_subagent"
                            | "must_obey_master"
                            | "may_decline_master_invite"
                    ),
                    "{label}: serialized role control surface leaked through key {key:?}: {value}"
                );
                assert_no_serialized_role_control(label, child);
            }
        }
        Value::Array(items) => {
            for item in items {
                assert_no_serialized_role_control(label, item);
            }
        }
        _ => {}
    }
}

#[test]
fn display_name_defaults_follow_current_master_without_renaming_identity() {
    let (mut fixture, master, peer) = setup();
    let board = fixture.run_public(&["board", "show"], THREAD_A);
    assert_eq!(board["master"]["display_name"], "Jenny");
    let members = board["workers"].as_array().unwrap();
    assert_eq!(
        members.iter().find(|w| w["id"] == master).unwrap()["display_name"],
        "Jenny"
    );
    assert_eq!(
        members.iter().find(|w| w["id"] == peer).unwrap()["display_name"],
        "Bob"
    );
    restore_online_master(&mut fixture, THREAD_B);
    // Handoff does not operate the former executor. Its own later context
    // call obtains an ordinary pool name without taking Jenny back.
    let resumed_peer = fixture.run_public(&["context"], THREAD_A);
    assert_eq!(resumed_peer["identity"]["worker_id"], master);
    assert_eq!(resumed_peer["identity"]["display_name"], "Alice");
    let board = fixture.run_public(&["board", "show"], THREAD_A);
    assert_eq!(board["master"]["worker_id"], peer);
    assert_eq!(board["master"]["display_name"], "Jenny");
    assert_eq!(
        board["workers"]
            .as_array()
            .unwrap()
            .iter()
            .find(|w| w["id"] == master)
            .unwrap()["display_name"],
        "Alice"
    );
}

fn restore_online_master(fixture: &mut AppFixture, thread: &str) -> Value {
    // Presence is the precondition under test. A responsive Fake AppServer
    // alone is not a committed name-presence observation.
    let journal = std::fs::read_to_string(board_journal_path(fixture)).unwrap();
    let holder = journal
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .filter(|event| {
            event["ev"] == "NameResolutionCommitted" && event["occupancy"]["name"] == "Jenny"
        })
        .last()
        .expect("current Jenny name occupancy");
    append_board_events(
        fixture,
        &[json!({"ev":"AgentPresenceObserved", "presence":{
            "project_scope":holder["project_scope"], "worker_id":holder["occupancy"]["worker_id"],
            "session_id":holder["occupancy"]["session_id"], "presence":"online",
            "reason":"controlled current name-holder presence", "observed_ms":1
        }})],
    );
    let unconfirmed = fixture.command_public(&["context", "--approve-grant",
        r#"{"action":"restore_master","authorization":"isolated fixture requests replacement"}"#], thread);
    assert!(
        !unconfirmed.status.success(),
        "online replacement needs this request's second confirmation"
    );
    fixture.run_public(&["context", "--approve-grant",
        r#"{"action":"restore_master","authorization":"isolated fixture confirms online replacement","replace_confirmed":true}"#], thread)
}

#[test]
fn context_name_restore_persists_and_preserves_task_and_message_identity() {
    let (mut fixture, master, peer) = setup();
    let named = fixture.run_public(&["context", "--approve-grant",
        r#"{"action":"restore_name","requested_name":"Carol","authorization":"isolated fixture changes its free work name"}"#], THREAD_B);
    assert_eq!(named["identity"]["worker_id"], peer);
    assert_eq!(named["identity"]["display_name"], "Carol");
    let who = fixture.run_public(&["who"], THREAD_B);
    assert_eq!(
        who["workers"]
            .as_array()
            .unwrap()
            .iter()
            .find(|w| w["id"] == peer)
            .unwrap()["display_name"],
        "Carol"
    );
    let status = fixture.run_public(&["worker", "status", &peer], THREAD_B);
    assert_eq!(status["workers"][0]["display_name"], "Carol");
    assert_eq!(
        fixture.run_public(&["board", "show"], THREAD_A)["master"]["display_name"],
        "Jenny"
    );
    publish(&fixture, "named-work");
    let claimed = fixture.run_public(
        &["board", "claim", "named-work", "--expected-revision", "1"],
        THREAD_B,
    );
    assert_eq!(claimed["task"]["owner"], peer);
    assert_eq!(claimed["task"]["owner_display_name"], "Carol");
    assert_eq!(claimed["task"]["publisher"], master);
    assert_eq!(claimed["task"]["publisher_display_name"], "Jenny");
    fixture.run_public(
        &[
            "sendmessage",
            "--to",
            &master,
            "--subject",
            "named-peer",
            "stable route",
        ],
        THREAD_B,
    );
    let received = fixture.run_public(&["recv", "--timeout", "1"], THREAD_A);
    let message = received["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|message| message["subject"] == "named-peer")
        .expect("the sent peer message must be consumed independently of daemon notices");
    assert_eq!(message["from"], peer);
    assert_eq!(message["body"], "stable route");
    for name in ["  ", "Alice\nInjected"] {
        assert!(!fixture
            .command_public(
                &[
                    "context",
                    "--approve-grant",
                    &json!({
                        "action":"restore_name", "requested_name":name,
                        "authorization":"isolated invalid-name counterexample"
                    })
                    .to_string()
                ],
                THREAD_B
            )
            .status
            .success());
    }
    fixture.restart_daemon();
    let restored = fixture.run_public(&["context"], THREAD_B);
    assert_eq!(restored["identity"]["display_name"], "Carol");
    assert_eq!(restored["identity"]["worker_id"], peer);
    assert_eq!(restored["master"]["display_name"], "Jenny");
    let task = fixture.run_public(&["board", "show"], THREAD_B);
    let task = task["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|task| task["id"] == "named-work")
        .unwrap();
    assert_eq!(task["owner"], peer);
    assert_eq!(task["owner_display_name"], "Carol");
}

#[test]
fn board_publish_is_shared_but_does_not_start_execution() {
    let (fixture, master, _) = setup();
    let published = publish(&fixture, "board-one");
    assert_eq!(published["task"]["id"], "board-one");
    assert_eq!(published["task"]["status"], "pending");
    assert_eq!(published["task"]["owner"], master);
    assert_eq!(published["task"]["title"], "任务板接口");
    assert_eq!(published["task"]["revision"], 1);
    let board = fixture.run_public(&["board", "show"], THREAD_B);
    let tasks = board["tasks"].as_array().unwrap();
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0]["id"], "board-one");
    assert!(!serde_json::to_string(&board).unwrap().contains("\"token\""));
    let status = fixture.run_public(&["task", "status", "board-one"], THREAD_B);
    assert_eq!(status["status"], "pending");
    assert_eq!(status["owner"], master);
    assert!(!status["completion_missing"].is_null());
    for absent in [
        "delivery",
        "review",
        "integration",
        "merge",
        "cleanup",
        "keepalive",
    ] {
        assert!(
            status.get(absent).is_none(),
            "empty lifecycle or unrelated history {absent}: {status}"
        );
    }
    let bypass = fixture.command_public(
        &["task", "update", "board-one", "--status", "working"],
        THREAD_A,
    );
    assert!(!bypass.status.success(), "pending task must require accept");
    let peer_publish = fixture.command_public(
        &[
            "board",
            "publish",
            "peer-published",
            "--title",
            "x",
            "--description",
            "x",
            "--delivery-condition",
            "x",
            "--test-condition",
            "x",
        ],
        THREAD_B,
    );
    assert!(
        peer_publish.status.success(),
        "registered peer may publish pending work: {peer_publish:?}"
    );
}

#[test]
fn peer_publication_can_be_claimed_by_self_or_master() {
    let (fixture, master, peer) = setup();
    let peer_publish = fixture.run_public(
        &[
            "board",
            "publish",
            "peer-owned-queue",
            "--title",
            "peer 发布",
            "--description",
            "peer 队列",
            "--delivery-condition",
            "peer 交付",
            "--test-condition",
            "CLI 黑盒通过",
        ],
        THREAD_B,
    );
    assert_eq!(peer_publish["task"]["owner"], peer);
    assert_eq!(peer_publish["task"]["status"], "pending");
    let master_claim = fixture.run_public(
        &[
            "board",
            "claim",
            "peer-owned-queue",
            "--expected-revision",
            "1",
        ],
        THREAD_A,
    );
    assert_eq!(master_claim["task"]["owner"], master);
    assert_eq!(master_claim["task"]["status"], "working");
    assert_eq!(master_claim["task"]["revision"], 2);

    let own_publish = fixture.run_public(
        &[
            "board",
            "publish",
            "peer-self-claim",
            "--title",
            "后续工作",
            "--description",
            "排队登记",
            "--delivery-condition",
            "完成下一工作",
            "--test-condition",
            "CLI 黑盒通过",
        ],
        THREAD_B,
    );
    assert_eq!(own_publish["task"]["owner"], peer);
    let own_claim = fixture.run_public(
        &[
            "board",
            "claim",
            "peer-self-claim",
            "--expected-revision",
            "1",
        ],
        THREAD_B,
    );
    assert_eq!(own_claim["task"]["owner"], peer);
    assert_eq!(own_claim["task"]["publisher"], peer);
    assert_eq!(own_claim["task"]["status"], "working");
}

#[test]
fn concurrent_claims_admit_exactly_one_and_stale_claim_preserves_state() {
    let (fixture, master, peer) = setup();
    publish(&fixture, "claim-race");
    let fixture = std::sync::Arc::new(fixture);
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let first_barrier = std::sync::Arc::clone(&barrier);
    let second_barrier = barrier.clone();
    let first_fixture = std::sync::Arc::clone(&fixture);
    let second_fixture = first_fixture.clone();
    let first = std::thread::spawn(move || {
        first_barrier.wait();
        first_fixture.command_public(
            &["board", "claim", "claim-race", "--expected-revision", "1"],
            THREAD_A,
        )
    });
    let second = std::thread::spawn(move || {
        second_barrier.wait();
        second_fixture.command_public(
            &["board", "claim", "claim-race", "--expected-revision", "1"],
            THREAD_B,
        )
    });
    let first = first.join().unwrap();
    let second = second.join().unwrap();
    let successes = [&first, &second]
        .iter()
        .filter(|output| output.status.success())
        .count();
    assert_eq!(
        successes, 1,
        "exactly one concurrent claim may succeed: {first:?} {second:?}"
    );
    let board = fixture.run_public(&["board", "show"], THREAD_A);
    let task = board["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|task| task["id"] == "claim-race")
        .unwrap();
    assert_eq!(task["status"], "working");
    let actual_owner = task["owner"].as_str().unwrap();
    assert!(actual_owner == master || actual_owner == peer);
    assert_eq!(task["revision"], 2);
    let stale = fixture.command_public(
        &["board", "claim", "claim-race", "--expected-revision", "1"],
        THREAD_A,
    );
    assert!(!stale.status.success(), "stale claim must fail explicitly");
    let after = fixture.run_public(&["task", "status", "claim-race"], THREAD_A);
    assert_eq!(after["owner"], actual_owner);
    assert_eq!(after["status"], "working");
}

#[test]
fn blocked_task_keeps_responsibility_but_does_not_block_next_work_or_resume() {
    let (fixture, _, peer) = setup();
    publish(&fixture, "blocked-first");
    fixture.run_public(
        &[
            "board",
            "claim",
            "blocked-first",
            "--expected-revision",
            "1",
        ],
        THREAD_B,
    );
    fixture.run_public(
        &[
            "board",
            "update",
            "blocked-first",
            "--expected-revision",
            "2",
            "--status",
            "blocked",
            "--next",
            "等待外部输入",
        ],
        THREAD_B,
    );
    let queued = fixture.run_public(
        &[
            "board",
            "publish",
            "blocked-next",
            "--title",
            "阻塞后续",
            "--description",
            "继续无冲突工作",
            "--delivery-condition",
            "第二项完成",
            "--test-condition",
            "CLI 黑盒通过",
        ],
        THREAD_B,
    );
    assert_eq!(queued["task"]["status"], "pending");
    fixture.run_public(
        &["board", "claim", "blocked-next", "--expected-revision", "1"],
        THREAD_B,
    );
    let premature_resume = fixture.command_public(
        &[
            "board",
            "update",
            "blocked-first",
            "--expected-revision",
            "3",
            "--status",
            "working",
        ],
        THREAD_B,
    );
    assert!(
        premature_resume.status.success(),
        "a peer can orchestrate multiple independent goals: {premature_resume:?}"
    );
    fixture.run_public(
        &[
            "board",
            "update",
            "blocked-next",
            "--expected-revision",
            "2",
            "--status",
            "cancelled",
        ],
        THREAD_B,
    );
    let resumed = fixture.run_public(
        &[
            "board",
            "update",
            "blocked-first",
            "--expected-revision",
            "4",
            "--status",
            "working",
            "--next",
            "外部输入已恢复",
        ],
        THREAD_B,
    );
    assert_eq!(resumed["task"]["owner"], peer);
    assert_eq!(resumed["task"]["status"], "working");
}

#[test]
fn claimed_and_queued_tasks_survive_daemon_replay() {
    let (mut fixture, _, peer) = setup();
    publish(&fixture, "replay-active");
    fixture.run_public(
        &[
            "board",
            "claim",
            "replay-active",
            "--expected-revision",
            "1",
        ],
        THREAD_B,
    );
    let queued = fixture.run_public(
        &[
            "board",
            "publish",
            "replay-queued",
            "--title",
            "排队任务",
            "--description",
            "daemon 重启后仍待领取",
            "--delivery-condition",
            "恢复后领取",
            "--test-condition",
            "CLI 黑盒通过",
        ],
        THREAD_B,
    );
    assert_eq!(queued["task"]["revision"], 1);

    fixture.restart_daemon();

    let active = fixture.run_public(&["task", "status", "replay-active"], THREAD_A);
    assert_eq!(active["owner"], peer);
    assert_eq!(active["status"], "working");
    let board = fixture.run_public(&["board", "show"], THREAD_A);
    let tasks = board["tasks"].as_array().unwrap();
    let active = tasks
        .iter()
        .find(|task| task["id"] == "replay-active")
        .unwrap();
    let queued = tasks
        .iter()
        .find(|task| task["id"] == "replay-queued")
        .unwrap();
    assert_eq!(active["status"], "working");
    assert_eq!(active["revision"], 2);
    assert_eq!(queued["status"], "pending");
    assert_eq!(queued["owner"], peer);
    assert_eq!(queued["publisher"], peer);
    assert_eq!(queued["revision"], 1);
}

#[test]
fn board_owner_update_is_versioned_and_peer_tasks_remain_independent() {
    let (fixture, _, _) = setup();
    fixture.run_public(
        &["task", "register", "peer-own", "--next", "实现接口"],
        THREAD_B,
    );
    let board = fixture.run_public(&["board", "show"], THREAD_A);
    let task = board["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|task| task["id"] == "peer-own")
        .unwrap();
    let revision = task["revision"].as_u64().unwrap().to_string();
    let updated = fixture.run_public(
        &[
            "board",
            "update",
            "peer-own",
            "--expected-revision",
            &revision,
            "--status",
            "verifying",
            "--next",
            "执行黑盒回归",
        ],
        THREAD_B,
    );
    assert_eq!(updated["task"]["status"], "verifying");
    assert!(updated["task"]["revision"].as_u64().unwrap() > revision.parse::<u64>().unwrap());
    let stale = fixture.command_public(
        &[
            "board",
            "update",
            "peer-own",
            "--expected-revision",
            &revision,
            "--next",
            "旧进度",
        ],
        THREAD_B,
    );
    assert!(!stale.status.success());
    let master_write = fixture.command_public(
        &[
            "board",
            "update",
            "peer-own",
            "--expected-revision",
            &updated["task"]["revision"].as_u64().unwrap().to_string(),
            "--next",
            "代写进度",
        ],
        THREAD_A,
    );
    assert!(!master_write.status.success());
    let task_after = fixture.run_public(&["task", "status", "peer-own"], THREAD_A);
    assert_eq!(task_after["next_step"], "执行黑盒回归");
}

#[test]
fn board_invitation_transfers_owner_only_after_peer_acceptance() {
    let (fixture, master, peer) = setup();
    publish(&fixture, "invitation");
    let invited = fixture.run_public(
        &[
            "board",
            "invite",
            "invitation",
            "--to",
            &peer,
            "--expected-revision",
            "1",
        ],
        THREAD_A,
    );
    assert_eq!(invited["task"]["owner"], master);
    assert_eq!(invited["task"]["status"], "invited");
    assert_eq!(invited["task"]["revision"], 2);
    assert_eq!(invited["consumed"], false);
    let outsider = fixture.command_public(
        &[
            "board",
            "respond",
            "invitation",
            "--accept",
            "--expected-revision",
            "2",
        ],
        THREAD_A,
    );
    assert!(!outsider.status.success());
    let declined = fixture.run_public(
        &[
            "board",
            "respond",
            "invitation",
            "--decline",
            "--expected-revision",
            "2",
            "--reason",
            "保留独立工作",
        ],
        THREAD_B,
    );
    assert_eq!(declined["task"]["owner"], master);
    assert_eq!(declined["task"]["status"], "pending");
    assert_eq!(declined["task"]["revision"], 3);
    fixture.run_public(
        &[
            "board",
            "invite",
            "invitation",
            "--to",
            &peer,
            "--expected-revision",
            "3",
        ],
        THREAD_A,
    );
    let accepted = fixture.run_public(
        &[
            "board",
            "respond",
            "invitation",
            "--accept",
            "--expected-revision",
            "4",
        ],
        THREAD_B,
    );
    assert_eq!(accepted["task"]["owner"], peer);
    assert_eq!(accepted["task"]["status"], "working");
    assert_eq!(accepted["task"]["revision"], 5);
}

#[test]
fn working_task_can_be_reassigned_and_decline_or_withdraw_preserves_owner_status() {
    let (fixture, master, peer) = setup();
    publish(&fixture, "working-reassign");
    let claimed = fixture.run_public(
        &[
            "board",
            "claim",
            "working-reassign",
            "--expected-revision",
            "1",
        ],
        THREAD_B,
    );
    assert_eq!(claimed["task"]["owner"], peer);
    assert_eq!(claimed["task"]["status"], "working");

    let carol = fixture.run_public(&["context"], THREAD_C)["identity"]["worker_id"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_ne!(carol, master);

    let invited = fixture.run_public(
        &[
            "board",
            "invite",
            "working-reassign",
            "--to",
            &carol,
            "--expected-revision",
            "2",
        ],
        THREAD_A,
    );
    assert_eq!(invited["task"]["owner"], peer);
    assert_eq!(invited["task"]["status"], "working");
    assert_eq!(invited["task"]["invited_peer"], carol);
    let duplicate = fixture.command_public(
        &[
            "board",
            "invite",
            "working-reassign",
            "--to",
            &carol,
            "--expected-revision",
            "3",
        ],
        THREAD_A,
    );
    assert!(
        !duplicate.status.success(),
        "one task keeps one pending invitation"
    );
    let stale = fixture.command_public(
        &[
            "board",
            "respond",
            "working-reassign",
            "--accept",
            "--expected-revision",
            "2",
        ],
        THREAD_C,
    );
    assert!(!stale.status.success());
    assert!(String::from_utf8_lossy(&stale.stderr).contains("BOARD_STALE_REVISION"));

    let declined = fixture.run_public(
        &[
            "board",
            "respond",
            "working-reassign",
            "--decline",
            "--expected-revision",
            "3",
            "--reason",
            "not this round",
        ],
        THREAD_C,
    );
    assert_eq!(declined["task"]["owner"], peer);
    assert_eq!(declined["task"]["status"], "working");
    assert_eq!(declined["task"]["revision"], 4);

    let reinvited = fixture.run_public(
        &[
            "board",
            "invite",
            "working-reassign",
            "--to",
            &carol,
            "--expected-revision",
            "4",
        ],
        THREAD_A,
    );
    assert_eq!(reinvited["task"]["status"], "working");
    let withdrawn = fixture.run_public(
        &[
            "board",
            "withdraw",
            "working-reassign",
            "--expected-revision",
            "5",
            "--reason",
            "keep current owner",
        ],
        THREAD_A,
    );
    assert_eq!(withdrawn["task"]["owner"], peer);
    assert_eq!(withdrawn["task"]["status"], "working");
    assert_eq!(withdrawn["task"]["revision"], 6);

    let durable = fixture.run_public(&["task", "status", "working-reassign"], THREAD_A);
    assert_eq!(durable["owner"], peer);
    assert_eq!(durable["status"], "working");
}

#[test]
fn offline_name_holder_projects_empty_owner_then_name_reassignment_preserves_work() {
    resource_reassignment_consumer("accepted");
}

#[test]
fn working_resource_reassignment_allows_new_owner_to_resume() {
    resource_reassignment_consumer("working");
}

fn resource_reassignment_consumer(status: &str) {
    let (mut fixture, master, peer) = setup();
    publish(&fixture, "reassign-work");
    fixture.run_public(
        &[
            "board",
            "claim",
            "reassign-work",
            "--expected-revision",
            "1",
        ],
        THREAD_B,
    );
    let carol = fixture.run_public(&["context"], THREAD_C)["identity"]["worker_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let project_scope = std::fs::canonicalize(&fixture.project)
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let worktree = "/tmp/collab-t5-reassign-worktree";
    let events = vec![
        json!({
            "ev": "TaskUpdated",
            "task": {
                "id": "reassign-work",
                "owner": peer,
                "created_by": master,
                "feature_id": "reassign-feature",
                "worktree_path": worktree,
                "branch": "codex/reassign-work",
                "base_commit": "base-reassign",
                "priority": "p1",
                "status": status,
                "next_step": "integrate accepted candidate",
                "wait": null,
                "created_ms": 1,
                "updated_ms": 1000
            }
        }),
        json!({
            "ev": "TaskLifecycleUpdated",
            "task_id": "reassign-work",
            "record": {
                "delivery_evidence": "cargo test --test board green",
                "delivered_ms": 900,
                "delivery_commit": "candidate-reassign",
                "review_evidence": "independent review pass",
                "reviewer": master,
                "reviewed_ms": 950
            }
        }),
        json!({
            "ev": "WorktreeBound",
            "binding": {
                "worktree_root": worktree,
                "owning_project_scope": project_scope,
                "task_id": "reassign-work",
                "owner_agent_id": peer,
                "binding_id": "binding-reassign-work",
                "base_commit": "base-reassign"
            }
        }),
        json!({
            "ev": "MergeRequested",
            "request": {
                "task_id": "reassign-work",
                "owner": peer,
                "requested_by": master,
                "requested_ms": 950,
                "candidate_commit": "candidate-reassign"
            }
        }),
        json!({
            "ev": "NameResolutionCommitted",
            "project_scope": project_scope,
            "occupancy": {
                "name": "Bob",
                "worker_id": peer,
                "session_id": THREAD_B,
                "occupancy_revision": 1,
                "updated_ms": 1000
            },
            "assignments": []
        }),
        json!({
            "ev": "TaskAssignmentUpdated",
            "assignment": {
                "task_id": "reassign-work",
                "name": "Bob",
                "session_id": THREAD_B,
                "assignment_revision": 1,
                "updated_ms": 1000
            }
        }),
        json!({
            "ev": "NameResolutionCommitted",
            "project_scope": project_scope,
            "occupancy": {
                "name": "Carol",
                "worker_id": carol,
                "session_id": THREAD_C,
                "occupancy_revision": 1,
                "updated_ms": 1000
            },
            "assignments": []
        }),
        json!({
            "ev": "AgentPresenceObserved",
            "presence": {
                "project_scope": project_scope,
                "worker_id": peer,
                "session_id": THREAD_B,
                "presence": "offline",
                "reason": "fixture: agent exited",
                "observed_ms": 1001
            }
        }),
        json!({
            "ev": "AgentPresenceObserved",
            "presence": {
                "project_scope": project_scope,
                "worker_id": carol,
                "session_id": THREAD_C,
                "presence": "online",
                "reason": "fixture: current session observed",
                "observed_ms": 1001
            }
        }),
    ];
    append_board_events(&mut fixture, &events);

    let board = fixture.run_public(&["board", "show"], THREAD_A);
    let task = board["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|task| task["id"] == "reassign-work")
        .unwrap();
    assert_eq!(task["owner"], "", "{task}");
    assert_eq!(task["owner_display_name"], "", "{task}");
    assert_eq!(task["status"], status);
    assert_eq!(task["owner_name"], "Bob");
    assert!(task["owner_session_id"].is_null());
    assert_eq!(task["needs_reassignment"], true);
    assert_eq!(task["delivery_evidence"], "cargo test --test board green");
    assert_eq!(task["review_evidence"], "independent review pass");
    assert_eq!(task["cleanup_status"], "pending");
    let revision = task["revision"].as_u64().unwrap();

    let durable = fixture.run_public(&["task", "status", "reassign-work"], THREAD_A);
    assert_eq!(durable["owner"], peer);
    assert_eq!(durable["status"], status);
    assert_eq!(durable["worktree"], worktree);
    assert_eq!(durable["branch"], "codex/reassign-work");
    assert_eq!(
        durable["delivery"]["evidence"],
        "cargo test --test board green"
    );
    assert_eq!(durable["review"]["evidence"], "independent review pass");
    assert_eq!(durable["merge"]["owner"], peer);
    assert_eq!(durable["merge"]["pending"], true);

    let invited = fixture.run_public(
        &[
            "board",
            "invite",
            "reassign-work",
            "--to",
            "Carol",
            "--expected-revision",
            &revision.to_string(),
        ],
        THREAD_A,
    );
    assert_eq!(invited["task"]["status"], status);
    assert_eq!(invited["task"]["invited_peer"], carol);
    let declined = fixture.run_public(
        &[
            "board",
            "respond",
            "reassign-work",
            "--decline",
            "--reason",
            "later",
            "--expected-revision",
            &invited["task"]["revision"].to_string(),
        ],
        THREAD_C,
    );
    assert_eq!(declined["task"]["status"], status);
    let retained = fixture.run_public(&["task", "status", "reassign-work"], THREAD_A);
    assert_eq!(retained["owner"], peer);
    assert_eq!(retained["merge"]["owner"], peer);
    assert_eq!(retained["delivery"], durable["delivery"]);
    assert_eq!(retained["review"], durable["review"]);
    let invited = fixture.run_public(
        &[
            "board",
            "invite",
            "reassign-work",
            "--to",
            "Carol",
            "--expected-revision",
            &declined["task"]["revision"].to_string(),
        ],
        THREAD_A,
    );
    let withdrawn = fixture.run_public(
        &[
            "board",
            "withdraw",
            "reassign-work",
            "--reason",
            "later",
            "--expected-revision",
            &invited["task"]["revision"].to_string(),
        ],
        THREAD_A,
    );
    assert_eq!(withdrawn["task"]["status"], status);
    let retained = fixture.run_public(&["task", "status", "reassign-work"], THREAD_A);
    assert_eq!(retained["owner"], peer);
    assert_eq!(retained["merge"], durable["merge"]);
    assert_eq!(retained["worktree"], durable["worktree"]);
    assert_eq!(retained["delivery"], durable["delivery"]);
    let invited = fixture.run_public(
        &[
            "board",
            "invite",
            "reassign-work",
            "--to",
            "Carol",
            "--expected-revision",
            &withdrawn["task"]["revision"].to_string(),
        ],
        THREAD_A,
    );
    let invitation_revision = invited["task"]["revision"].as_u64().unwrap();

    let accepted = fixture.run_public(
        &[
            "board",
            "respond",
            "reassign-work",
            "--accept",
            "--expected-revision",
            &invitation_revision.to_string(),
        ],
        THREAD_C,
    );
    assert_eq!(accepted["task"]["owner"], carol);
    assert_eq!(accepted["task"]["status"], status);
    assert_eq!(accepted["task"]["owner_name"], "Carol");
    assert_eq!(accepted["task"]["owner_session_id"], THREAD_C);
    assert_eq!(accepted["task"]["needs_reassignment"], false);
    assert_eq!(
        accepted["task"]["delivery_evidence"],
        "cargo test --test board green"
    );
    assert_eq!(
        accepted["task"]["review_evidence"],
        "independent review pass"
    );

    let reassigned = fixture.run_public(&["task", "status", "reassign-work"], THREAD_A);
    assert_eq!(reassigned["owner"], carol);
    assert_eq!(reassigned["status"], status);
    assert_eq!(reassigned["worktree"], worktree);
    assert_eq!(reassigned["branch"], "codex/reassign-work");
    assert_eq!(reassigned["feature_id"], "reassign-feature");
    assert_eq!(
        reassigned["delivery"]["evidence"],
        "cargo test --test board green"
    );
    assert_eq!(reassigned["review"]["evidence"], "independent review pass");
    assert_eq!(reassigned["merge"]["owner"], carol);
    assert_eq!(reassigned["merge"]["pending"], true);

    let board = fixture.run_public(&["board", "show"], THREAD_A);
    let task = board["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|task| task["id"] == "reassign-work")
        .unwrap();
    assert_eq!(task["owner"], carol);
    assert_eq!(task["owner_name"], "Carol");
    assert_eq!(task["needs_reassignment"], false);
    if status == "working" {
        let revision = task["revision"].as_u64().unwrap();
        fixture.run_public(
            &[
                "board",
                "update",
                "reassign-work",
                "--expected-revision",
                &revision.to_string(),
                "--status",
                "blocked",
            ],
            THREAD_C,
        );
        let blocked = fixture.run_public(&["board", "show"], THREAD_C);
        let revision = blocked["tasks"]
            .as_array()
            .unwrap()
            .iter()
            .find(|task| task["id"] == "reassign-work")
            .unwrap()["revision"]
            .as_u64()
            .unwrap();
        let resumed = fixture.run_public(
            &[
                "board",
                "update",
                "reassign-work",
                "--expected-revision",
                &revision.to_string(),
                "--status",
                "working",
            ],
            THREAD_C,
        );
        assert_eq!(resumed["task"]["owner"], carol);
        assert_eq!(resumed["task"]["status"], "working");
    }
}

#[test]
fn closed_invitation_cannot_transfer_and_master_withdraw_only_clears_metadata() {
    for status in ["closed", "cancelled"] {
        let (mut fixture, master, peer) = setup();
        publish(&fixture, "closed-offer");
        let forbidden = fixture.command_public(
            &[
                "board",
                "invite",
                "closed-offer",
                "--to",
                &master,
                "--expected-revision",
                "1",
            ],
            THREAD_B,
        );
        assert!(!forbidden.status.success());
        assert!(String::from_utf8_lossy(&forbidden.stderr).contains("BOARD_MASTER_REQUIRED"));
        let invited = fixture.run_public(
            &[
                "board",
                "invite",
                "closed-offer",
                "--to",
                &peer,
                "--expected-revision",
                "1",
            ],
            THREAD_A,
        );
        append_board_events(
            &mut fixture,
            &[json!({"ev": "TaskUpdated", "task": {
                "id": "closed-offer", "owner": master, "created_by": master,
                "feature_id": null, "worktree_path": null, "branch": null, "base_commit": null,
                "priority": "p1", "status": status, "next_step": "historical", "wait": null,
                "created_ms": 1, "updated_ms": 2
            }})],
        );
        let board = fixture.run_public(&["board", "show"], THREAD_A);
        assert!(
            !board["tasks"]
                .as_array()
                .unwrap()
                .iter()
                .any(|task| task["id"] == "closed-offer"),
            "closed work must leave the board: {board}"
        );
        let historical = fixture.run_public(&["task", "status", "closed-offer"], THREAD_A);
        assert_eq!(historical["status"], status);
        assert_eq!(historical["owner"], master);
        // The isolated TaskUpdated event above advances the invitation's revision once.
        let revision = (invited["task"]["revision"].as_u64().unwrap() + 1).to_string();
        let response = fixture.command_public(
            &[
                "board",
                "respond",
                "closed-offer",
                "--accept",
                "--expected-revision",
                &revision,
            ],
            THREAD_B,
        );
        assert!(!response.status.success());
        assert!(String::from_utf8_lossy(&response.stderr).contains("BOARD_TASK_CLOSED"));
        let response = fixture.command_public(
            &[
                "board",
                "invite",
                "closed-offer",
                "--to",
                &peer,
                "--expected-revision",
                &revision,
            ],
            THREAD_A,
        );
        assert!(!response.status.success());
        let withdrawn = fixture.run_public(
            &[
                "board",
                "withdraw",
                "closed-offer",
                "--expected-revision",
                &revision,
                "--reason",
                "expire invitation",
            ],
            THREAD_A,
        );
        assert_eq!(withdrawn["task"]["status"], status);
        assert_eq!(withdrawn["task"]["owner"], master);
        assert!(withdrawn["task"]["invited_peer"].is_null());
        assert_eq!(withdrawn["task"]["next_step"], "historical");
    }
}

#[test]
fn peer_may_start_independent_work_while_an_invitation_is_pending() {
    let (fixture, master, peer) = setup();
    publish(&fixture, "offer-one");
    publish(&fixture, "offer-two");
    fixture.run_public(
        &[
            "board",
            "invite",
            "offer-one",
            "--to",
            &peer,
            "--expected-revision",
            "1",
        ],
        THREAD_A,
    );
    let second = fixture.command_public(
        &[
            "board",
            "invite",
            "offer-two",
            "--to",
            &peer,
            "--expected-revision",
            "1",
        ],
        THREAD_A,
    );
    assert!(
        second.status.success(),
        "a peer may receive multiple goal invitations: {second:?}"
    );
    fixture.run_public(
        &[
            "task",
            "register",
            "independent",
            "--next",
            "用户自己的任务",
        ],
        THREAD_B,
    );
    let accept = fixture.command_public(
        &[
            "board",
            "respond",
            "offer-one",
            "--accept",
            "--expected-revision",
            "2",
        ],
        THREAD_B,
    );
    assert!(
        accept.status.success(),
        "a peer may accept another goal: {accept:?}"
    );
    let declined = fixture.run_public(
        &[
            "board",
            "respond",
            "offer-two",
            "--decline",
            "--expected-revision",
            "2",
            "--reason",
            "正在自己的任务上工作",
        ],
        THREAD_B,
    );
    assert_eq!(declined["task"]["owner"], master);
    let independent = fixture.run_public(&["task", "status", "independent"], THREAD_B);
    assert_eq!(independent["owner"], peer);
    assert_eq!(independent["next_step"], "用户自己的任务");
}

#[test]
fn role_patrol_context_reminders_follow_master_and_peer_roles_across_restoration() {
    let mut fixture = AppFixture::new();
    let first_peer = fixture.run_public(&["context"], THREAD_B);
    let peer_keywords = [
        "patrol",
        "subscription",
        "scope",
        "master",
        "block",
        "non-overlapping bounded scopes",
        "close-before-open/sese",
        "canonical intake and deduplication",
        "owning project contract",
        "re-arm",
        "report channel",
        "stop conditions",
    ];
    assert_role_patrol_mentions("first peer context", &first_peer, "worker", &peer_keywords);

    fixture.run_public(&["context"], THREAD_A);
    let promoted = fixture.run_public(
        &[
            "master",
            "promote",
            "--approval",
            "isolated role patrol fixture promotion",
        ],
        THREAD_A,
    );
    let master_keywords = [
        "buglist",
        "p0",
        "p1",
        "peer",
        "progress",
        "worktree",
        "runtime",
        "cleanup",
        "registr",
        "promot",
        "restor",
        "inspect",
        "monitor",
        "event",
        "subscription",
        "only its own project",
        "canonical issue intake",
        "currently supported monitored events",
        "non-overlapping",
        "close-before-open/sese",
        "canonical intake and deduplication",
        "owning project contract",
        "goal",
        "deadline",
        "re-arm",
        "dispatch receipt",
        "report channel",
        "stop conditions",
    ];
    assert_role_patrol_mentions("master promotion", &promoted, "master", &master_keywords);
    let repeated_master = fixture.run_public(&["context"], THREAD_A);
    assert_role_patrol_mentions(
        "repeated master context",
        &repeated_master,
        "master",
        &master_keywords,
    );

    restore_online_master(&mut fixture, THREAD_B);
    let demoted_peer = fixture.run_public(&["context"], THREAD_A);
    assert_role_patrol_mentions(
        "demoted peer context",
        &demoted_peer,
        "worker",
        &peer_keywords,
    );

    let tasks_before = fixture.run_public(&["board", "show"], THREAD_A)["tasks"]
        .as_array()
        .expect("board show lists tasks")
        .len();
    assert_eq!(
        tasks_before, 0,
        "role context must not fabricate inspection tasks"
    );
    let master_subscriptions_before =
        notification_subscription_count(&fixture, THREAD_B, "master before restoration");
    let peer_subscriptions_before =
        notification_subscription_count(&fixture, THREAD_A, "peer before restoration");

    fixture.restart_daemon();

    let restored_master = fixture.run_public(&["context"], THREAD_B);
    assert_role_patrol_mentions(
        "restored master context",
        &restored_master,
        "master",
        &master_keywords,
    );
    let restored_peer = fixture.run_public(&["context"], THREAD_A);
    assert_role_patrol_mentions(
        "restored peer context",
        &restored_peer,
        "worker",
        &peer_keywords,
    );
    assert_eq!(
        notification_subscription_count(&fixture, THREAD_B, "master after restoration"),
        master_subscriptions_before,
        "context restoration must not create master patrol subscriptions"
    );
    assert_eq!(
        notification_subscription_count(&fixture, THREAD_A, "peer after restoration"),
        peer_subscriptions_before,
        "context restoration must not create peer patrol subscriptions"
    );
    let tasks_after = fixture.run_public(&["board", "show"], THREAD_A)["tasks"]
        .as_array()
        .expect("board show lists tasks")
        .len();
    assert_eq!(
        tasks_after, tasks_before,
        "context restoration must not fabricate board tasks"
    );
}

#[test]
fn role_patrol_board_invitation_is_durable_without_serialized_control_state() {
    let (fixture, master, peer) = setup();
    publish(&fixture, "role-patrol-invitation");
    let invited = fixture.run_public(
        &[
            "board",
            "invite",
            "role-patrol-invitation",
            "--to",
            &peer,
            "--expected-revision",
            "1",
        ],
        THREAD_A,
    );
    assert_eq!(invited["task"]["owner"], master);
    assert_eq!(invited["task"]["status"], "invited");

    let received = fixture.run_public(&["recv", "--timeout", "1"], THREAD_B);
    let invitation = received["messages"]
        .as_array()
        .expect("recv lists durable messages")
        .iter()
        .find(|message| message["subject"] == "task-invitation:role-patrol-invitation")
        .expect("recipient receives the board invitation");
    assert_eq!(invitation["id"], invited["msg_id"]);
    assert_no_serialized_role_control("board invitation", invitation);

    let body = invitation["body"]
        .as_str()
        .expect("board invitation carries a text body");
    let body_lower = body.to_ascii_lowercase();
    assert!(
        !body.contains("Role task:") && !body.contains("Responsibilities:"),
        "invitation must carry its task contract without replaying the registered role brief: {body}"
    );
    for required in [
        "任务板接口",
        "公开协议测试",
        "接口行为通过",
        "cargo test --test appserver_two_tui_integration",
    ] {
        assert!(
            body.contains(required),
            "missing task contract {required}: {body}"
        );
    }
    assert!(
        !body_lower.contains("collab recv"),
        "the invitation body is already the recv payload; it must not require another read: {body}"
    );
    assert!(
        body.contains("collab board respond")
            && body.contains("--accept")
            && body.contains("--decline")
            && body.contains("--expected-revision"),
        "accepting or declining still carries the typed revision check: {body}"
    );

    let board = fixture.run_public(&["board", "show"], THREAD_A);
    let task = board["tasks"]
        .as_array()
        .expect("board show lists tasks")
        .iter()
        .find(|task| task["id"] == "role-patrol-invitation")
        .expect("invited task remains visible");
    assert_eq!(task["owner"], master);
    assert_eq!(task["status"], "invited");

    let accepted = fixture.run_public(
        &[
            "board",
            "respond",
            "role-patrol-invitation",
            "--accept",
            "--expected-revision",
            "2",
        ],
        THREAD_B,
    );
    assert_eq!(accepted["task"]["owner"], peer);
    assert_eq!(accepted["task"]["status"], "working");
}

#[test]
fn recv_public_replay_keeps_payload_and_reply_without_delivery_diagnostics() {
    let (fixture, _, peer) = setup();
    let sent = fixture.run_public(
        &[
            "sendmessage",
            "--to",
            &peer,
            "--subject",
            "receipt",
            "--reply-required",
            "body with records and query_result",
        ],
        THREAD_A,
    );
    let id = sent["msg_id"].as_str().unwrap();
    let first = fixture.run_public(
        &["recv", "--receive-id", "minimal-receipt", "--timeout", "0"],
        THREAD_B,
    );
    let message = first["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|message| message["id"] == id)
        .expect("received original message");
    assert_eq!(message["body"], "body with records and query_result");
    assert!(!message["reply_obligation"].is_null(), "{message}");
    for field in [
        "wake_attempt_count",
        "last_wake_attempt_ms",
        "retry_attempted",
    ] {
        assert!(
            message.get(field).is_none(),
            "delivery diagnostic {field}: {message}"
        );
    }
    let next = fixture.run_public(
        &[
            "sendmessage",
            "--to",
            &peer,
            "--subject",
            "later",
            "leave this unread",
        ],
        THREAD_A,
    );
    let replay = fixture.run_public(
        &["recv", "--receive-id", "minimal-receipt", "--timeout", "0"],
        THREAD_B,
    );
    assert_eq!(replay["replayed"], true);
    assert_eq!(first["messages"], replay["messages"]);
    let later = fixture.run_public(&["msg", next["msg_id"].as_str().unwrap()], THREAD_B);
    assert_ne!(
        later["state"], "read",
        "replay consumed another batch: {later}"
    );
    let second = fixture.run_public(
        &["recv", "--receive-id", "minimal-next", "--timeout", "0"],
        THREAD_B,
    );
    assert!(second["messages"]
        .as_array()
        .unwrap()
        .iter()
        .any(|message| message["id"] == next["msg_id"]));
}
