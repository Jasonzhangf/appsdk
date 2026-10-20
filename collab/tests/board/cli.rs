use super::*;

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

#[test]
fn display_name_defaults_follow_current_master_without_renaming_identity() {
    let (fixture, master, peer) = setup();
    let board = fixture.run_public(&["board", "show"], THREAD_A);
    assert_eq!(board["master"]["display_name"], "Jenny");
    let members = board["workers"].as_array().unwrap();
    assert_eq!(
        members.iter().find(|w| w["id"] == master).unwrap()["display_name"],
        "Jenny"
    );
    assert_eq!(
        members.iter().find(|w| w["id"] == peer).unwrap()["display_name"],
        peer
    );
    fixture.run_public(
        &["master", "promote", "--approval", "user chooses new master"],
        THREAD_B,
    );
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
        master
    );
}

#[test]
fn display_name_changes_persist_and_preserve_task_and_message_identity() {
    let (mut fixture, master, peer) = setup();
    let named = fixture.run_public(&["worker", "name", "Alice"], THREAD_B);
    assert_eq!(named["worker_id"], peer);
    assert_eq!(named["display_name"], "Alice");
    let who = fixture.run_public(&["who"], THREAD_B);
    assert_eq!(
        who["workers"]
            .as_array()
            .unwrap()
            .iter()
            .find(|w| w["id"] == peer)
            .unwrap()["display_name"],
        "Alice"
    );
    let status = fixture.run_public(&["worker", "status", &peer], THREAD_B);
    assert_eq!(status["workers"][0]["display_name"], "Alice");
    fixture.run_public(&["worker", "name", "June"], THREAD_A);
    assert_eq!(
        fixture.run_public(&["master", "status"], THREAD_A)["master"]["display_name"],
        "June"
    );
    publish(&fixture, "named-work");
    let claimed = fixture.run_public(
        &["board", "claim", "named-work", "--expected-revision", "1"],
        THREAD_B,
    );
    assert_eq!(claimed["task"]["owner"], peer);
    assert_eq!(claimed["task"]["owner_display_name"], "Alice");
    assert_eq!(claimed["task"]["publisher"], master);
    assert_eq!(claimed["task"]["publisher_display_name"], "June");
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
    assert_eq!(received["messages"][0]["from"], peer);
    for name in ["  ", "Alice\nInjected"] {
        assert!(!fixture
            .command_public(&["worker", "name", name], THREAD_B)
            .status
            .success());
    }
    fixture.restart_daemon();
    let restored = fixture.run_public(&["context"], THREAD_B);
    assert_eq!(restored["identity"]["display_name"], "Alice");
    assert_eq!(restored["master"]["display_name"], "June");
    let reset = fixture.run_public(&["worker", "name", "--reset"], THREAD_A);
    assert_eq!(reset["display_name"], "Jenny");
    fixture.run_public(&["worker", "name", "--reset"], THREAD_B);
    fixture.restart_daemon();
    assert_eq!(
        fixture.run_public(&["context"], THREAD_B)["identity"]["display_name"],
        peer
    );
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
        !premature_resume.status.success(),
        "a second active task must prevent resume"
    );
    assert_eq!(premature_resume.stderr.is_empty(), false);
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
            "3",
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
        !second.status.success(),
        "one peer cannot have two invitations"
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
    assert!(!accept.status.success(), "busy peer cannot accept");
    let declined = fixture.run_public(
        &[
            "board",
            "respond",
            "offer-one",
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
