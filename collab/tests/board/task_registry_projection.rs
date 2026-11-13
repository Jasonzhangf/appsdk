use super::*;
fn assignment_ownership(state: &Value) -> std::collections::BTreeMap<String, [Value; 4]> {
    // TaskAssignment ownership is stable across name_stewardship::tick_locked.
    // That independent producer may update reflow and updated_ms. The scheduler-free
    // task_registry_public_queries_do_not_mutate_state test protects full query purity.
    state["task_assignments"]
        .as_object()
        .unwrap()
        .iter()
        .map(|(key, assignment)| {
            (
                key.clone(),
                [
                    assignment.get("task_id").expect("task_id").clone(),
                    assignment.get("name").expect("name").clone(),
                    assignment.get("session_id").expect("session_id").clone(),
                    assignment
                        .get("assignment_revision")
                        .expect("assignment_revision")
                        .clone(),
                ],
            )
        })
        .collect()
}

#[test]
fn task_status_follows_assignment_name_after_takeover() {
    const THREAD_C: &str = "01a0d63b-2f2c-72cf-93c1-2a4a1e2cf1cc";

    let fixture = AppFixture::new();
    let alice = fixture.run_public(&["context"], THREAD_A);
    let bob = fixture.run_public(&["context"], THREAD_B);

    fixture.run_public(
        &[
            "task",
            "register",
            "source-alice-work",
            "--next",
            "keep Alice work",
        ],
        THREAD_A,
    );
    fixture.run_public(
        &[
            "task",
            "register",
            "target-bob-work",
            "--next",
            "continue Bob work",
        ],
        THREAD_B,
    );

    let initial = fixture.current_state();
    assert_eq!(
        initial["task_assignments"]["source-alice-work"]["name"],
        alice["name"]
    );
    assert_eq!(
        initial["task_assignments"]["target-bob-work"]["name"],
        bob["name"]
    );

    let takeover = fixture.run_public(
        &[
            "context",
            "--op",
            "task-registry-projection-red",
            "--approve-grant",
            r#"{"action":"restore_name","requested_name":"Bob","authorization":"fixture user authorizes Bob takeover"}"#,
        ],
        THREAD_A,
    );
    assert_eq!(takeover["name"], "Bob", "{takeover}");
    assert_eq!(
        takeover["identity"]["worker_id"],
        alice["identity"]["worker_id"]
    );

    let before_query = fixture.current_state();
    let source = fixture.run_public(&["task", "status", "source-alice-work"], THREAD_A);
    assert_eq!(source["owner_name"], "Alice", "{source}");
    assert_eq!(source["owner"], "", "{source}");
    assert!(source["owner_session_id"].is_null(), "{source}");

    let target = fixture.run_public(&["task", "status", "target-bob-work"], THREAD_A);
    assert_eq!(target["owner_name"], "Bob", "{target}");
    assert_eq!(target["owner"], alice["identity"]["worker_id"], "{target}");
    assert_eq!(target["owner_session_id"], THREAD_A, "{target}");

    let all = fixture.run_public(&["status", "--all"], THREAD_A);
    let all_source = all["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|task| task["id"] == "source-alice-work")
        .unwrap();
    assert_eq!(all_source["owner_name"], "Alice", "{all_source}");
    assert_eq!(all_source["owner"], "", "{all_source}");
    assert!(all_source["owner_session_id"].is_null(), "{all_source}");
    let all_target = all["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|task| task["id"] == "target-bob-work")
        .unwrap();
    assert_eq!(all_target["owner_name"], "Bob", "{all_target}");
    assert_eq!(
        all_target["owner"], alice["identity"]["worker_id"],
        "{all_target}"
    );

    let listed = fixture.run_public(&["task", "status"], THREAD_A);
    let listed_tasks = listed["tasks"].as_array().unwrap();
    assert_eq!(listed_tasks.len(), 2, "{listed}");
    let mut listed_ids: Vec<&str> = listed_tasks
        .iter()
        .map(|task| task["id"].as_str().unwrap())
        .collect();
    listed_ids.sort_unstable();
    assert_eq!(
        listed_ids,
        ["source-alice-work", "target-bob-work"],
        "{listed}"
    );
    let listed_source = listed_tasks
        .iter()
        .find(|task| task["id"] == "source-alice-work")
        .unwrap();
    let listed_target = listed_tasks
        .iter()
        .find(|task| task["id"] == "target-bob-work")
        .unwrap();
    assert_eq!(listed_source, all_source, "{listed}");
    assert_eq!(listed_target, all_target, "{listed}");

    let after_task_reads = fixture.current_state();
    assert_eq!(after_task_reads["tasks"], before_query["tasks"]);
    assert_eq!(
        assignment_ownership(&after_task_reads),
        assignment_ownership(&before_query)
    );
    // The daemon may commit unrelated maintenance between public snapshots.
    // Whole-State query purity is covered by the scheduler-free dispatcher test.

    let before_context = fixture.current_state();
    let context = fixture.run_public(&["context"], THREAD_A);
    let context_ids: Vec<&str> = context["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|task| task["id"].as_str())
        .collect();
    assert!(!context_ids.contains(&"source-alice-work"), "{context}");
    assert!(context_ids.contains(&"target-bob-work"), "{context}");
    let after_context = fixture.current_state();
    assert_eq!(after_context["tasks"], before_context["tasks"]);
    assert_eq!(
        assignment_ownership(&after_context),
        assignment_ownership(&before_context)
    );

    let alice_again = fixture.run_public(&["context"], THREAD_C);
    assert_eq!(alice_again["name"], "Alice", "{alice_again}");
    let before_second_query = fixture.current_state();
    let source = fixture.run_public(&["task", "status", "source-alice-work"], THREAD_C);
    assert_eq!(source["owner_name"], "Alice", "{source}");
    assert_eq!(
        source["owner"], alice_again["identity"]["worker_id"],
        "{source}"
    );
    assert_eq!(source["owner_session_id"], THREAD_C, "{source}");
    let target = fixture.run_public(&["task", "status", "target-bob-work"], THREAD_C);
    assert_eq!(target["owner_name"], "Bob", "{target}");
    assert_eq!(target["owner"], alice["identity"]["worker_id"], "{target}");
    let after_second_query = fixture.current_state();
    // Whole-State purity is covered by the scheduler-free dispatcher test.
    assert_eq!(after_second_query["tasks"], before_second_query["tasks"]);
    assert_eq!(
        assignment_ownership(&after_second_query),
        assignment_ownership(&before_second_query)
    );
}
