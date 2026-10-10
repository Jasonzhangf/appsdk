use super::*;

fn setup() -> (AppFixture, String, String) {
    let mut fixture = AppFixture::new();
    let first = fixture.run_public(&["context"], THREAD_A);
    fixture.initialized = true;
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
        !peer_publish.status.success(),
        "peer is not project publisher"
    );
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
