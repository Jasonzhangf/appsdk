// Unit regression for the close owner: a historical pending merge inherited
// from migrated journal data must be cleared by normal task close; an
// unclosed task's pending merge and its notice must survive untouched.
// Run via: cargo test --manifest-path collab/Cargo.toml --locked --bin collab close_historical_obligation

use super::*;
use crate::server::peer_tests::{register_synthetic_peer, test_server};
use crate::server::state::{default_priority, now_ms, Message, PendingMerge};
use serde_json::json;

fn register_accepted_task(server: &Server, owner: &str, id: &str) {
    assert!(
        handle_task_register(
            server,
            owner.into(),
            format!("token-{owner}"),
            id.into(),
            None,
            Some(format!("feature-{id}")),
            None,
            None,
            None,
            default_priority(),
        )
        .ok,
        "register {id} failed"
    );
    for status in ["verifying", "reviewed"] {
        assert!(
            handle_task_update(
                server,
                owner.into(),
                format!("token-{owner}"),
                id.into(),
                Some(status.into()),
                None,
            )
            .ok,
            "update {id} to {status} failed"
        );
    }
    assert!(
        handle_task_deliver(
            server,
            owner.into(),
            format!("token-{owner}"),
            id.into(),
            Some(format!("delivery evidence for {id}")),
            Some(format!("/tmp/close-hist-{id}")),
        )
        .ok,
        "deliver {id} failed"
    );
    assert!(
        handle_task_review(
            server,
            owner.into(),
            format!("token-{owner}"),
            id.into(),
            true,
            false,
            "review evidence".into(),
        )
        .ok,
        "review {id} failed"
    );
    assert_eq!(
        server.state.lock().unwrap().tasks[id].status,
        "accepted",
        "{id} must be accepted"
    );
}

fn seed_historical_pending_merge(server: &Server, task_id: &str, owner: &str) {
    let mut st = server.state.lock().unwrap();
    st.pending_merges.insert(
        task_id.into(),
        PendingMerge {
            task_id: task_id.into(),
            owner: owner.into(),
            requested_by: owner.into(),
            requested_ms: now_ms(),
            candidate_commit: None,
        },
    );
    st.msgs.insert(
        format!("merge-notice-{task_id}"),
        Message {
            id: format!("merge-notice-{task_id}"),
            from: "collab-server".into(),
            to: owner.into(),
            mtype: "notify".into(),
            subject: Some(format!("merge-pending:{task_id}")),
            body: "legacy merge pending".into(),
            in_reply_to: None,
            created_ms: now_ms(),
            state: "pending".into(),
            wake_attempt_count: 0,
            last_wake_attempt_ms: 0,
            retry_attempted: false,
        },
    );
}

#[test]
fn normal_close_clears_historical_pending_merge_and_supersedes_notice() {
    let (server, root) = test_server();
    register_synthetic_peer(&server, "owner", "%owner");
    register_accepted_task(&server, "owner", "task-hist");
    seed_historical_pending_merge(&server, "task-hist", "owner");

    let closed = handle_task_close(
        &server,
        "owner".into(),
        "token-owner".into(),
        "task-hist".into(),
        false,
        None,
    );
    assert!(
        closed.ok,
        "normal close of a task with a historical pending merge must succeed: {}",
        closed.error.unwrap_or_default()
    );
    assert_eq!(closed.data["disposition"], json!("closed"));

    let st = server.state.lock().unwrap();
    assert!(
        !st.pending_merges.contains_key("task-hist"),
        "close must clear a historical pending merge obligation"
    );
    assert_eq!(
        st.msgs["merge-notice-task-hist"].state, "superseded",
        "close must supersede the stale merge-pending notice"
    );
    drop(st);
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn normal_close_keeps_unclosed_task_pending_merge() {
    let (server, root) = test_server();
    register_synthetic_peer(&server, "owner", "%owner");
    register_accepted_task(&server, "owner", "task-hist");
    register_accepted_task(&server, "owner", "task-open");
    seed_historical_pending_merge(&server, "task-hist", "owner");
    seed_historical_pending_merge(&server, "task-open", "owner");

    let closed = handle_task_close(
        &server,
        "owner".into(),
        "token-owner".into(),
        "task-hist".into(),
        false,
        None,
    );
    assert!(closed.ok, "{}", closed.error.unwrap_or_default());

    let st = server.state.lock().unwrap();
    assert!(
        !st.pending_merges.contains_key("task-hist"),
        "closed task obligation cleared"
    );
    assert!(
        st.pending_merges.contains_key("task-open"),
        "unclosed task obligation must survive"
    );
    assert_eq!(
        st.msgs["merge-notice-task-open"].state, "pending",
        "unclosed task notice must remain actionable"
    );
    drop(st);
    std::fs::remove_dir_all(root).ok();
}
