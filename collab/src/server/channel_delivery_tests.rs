use super::{
    deliver_committed_reply, persist_result_locked, ChannelDeliveryResult, ChannelDeliveryStatus,
    PlatformTimestamp,
};
use crate::config::QqBotDelivery;
use crate::server::channel_envelope::ChannelReplyRoute;
use crate::server::peer_tests::test_server;
use crate::server::state::{now_ms, Event, Message};
use crate::server::Server;
use serde_json::json;
use std::path::{Path, PathBuf};
use std::sync::Arc;

const MESSAGE_ID: &str = "channel-reply-out-1";
const CHANNEL_INSTANCE: &str = "qqbot@host";
const REPLY_ROUTE: &str = "qqbot://c2c/user-1";

struct Fixture {
    server: Arc<Server>,
    root: PathBuf,
    helper_dir: PathBuf,
}

fn accepted_response() -> &'static str {
    r#"{"kind":"accepted","message_id":"channel-reply-out-1","platform_message_id":"platform-out-1","timestamp":"2026-11-08T12:00:00Z","ref_idx":"7"}"#
}

fn write_helper(dir: &Path, body: &str) -> PathBuf {
    std::fs::create_dir_all(dir).unwrap();
    let helper = dir.join("qqbot-send-stub.mjs");
    std::fs::write(&helper, body).unwrap();
    helper
}

fn standard_helper(response: &str, delay: bool) -> String {
    let delayed = if delay { "sleep 0.25\n" } else { "" };
    let output = if response.is_empty() {
        String::new()
    } else {
        format!("printf '%s\\n' '{response}'\n")
    };
    format!(
        "#!/bin/sh\nHERE=$(CDPATH= cd -- \"$(dirname -- \"$0\")\" && pwd)\nprintf 'called\\n' >> \"$HERE/calls.txt\"\nprintf '%s\\n' \"$@\" > \"$HERE/args.txt\"\ncat > \"$HERE/stdin.json\"\n{delayed}{output}"
    )
}

fn fixture_with_state_root(helper_body: &str, helper_state_is_server_state: bool) -> Fixture {
    let (mut server, root) = test_server();
    server.config.notifications.enabled = false;
    let helper_dir = root.join("qqbot-helper");
    let helper = write_helper(&helper_dir, helper_body);
    let runtime_root = helper_dir.join("runtime");
    let state_root = if helper_state_is_server_state {
        server.state_path.parent().unwrap().to_path_buf()
    } else {
        helper_dir.join("state")
    };
    std::fs::create_dir_all(&runtime_root).unwrap();
    std::fs::create_dir_all(&state_root).unwrap();
    server.config.qqbot.delivery = Some(QqBotDelivery {
        node_program: PathBuf::from("/bin/sh"),
        helper_path: helper,
        runtime_root,
        state_root,
        deadline_ms: 2_000,
    });
    server.commit(&[
        Event::Sent {
            msg: Message {
                id: MESSAGE_ID.into(),
                from: "agent".into(),
                to: REPLY_ROUTE.into(),
                mtype: "reply".into(),
                subject: Some("answer".into()),
                body: "full reply body\nsecond line".into(),
                in_reply_to: Some("platform-in-1".into()),
                created_ms: now_ms(),
                state: "pending".into(),
                wake_attempt_count: 0,
                last_wake_attempt_ms: 0,
                retry_attempted: false,
            },
        },
        Event::ChannelReplyRouteBound {
            message_id: MESSAGE_ID.into(),
            route: ChannelReplyRoute {
                source_channel: "qqbot".into(),
                channel_instance: CHANNEL_INSTANCE.into(),
                external_conversation: "u1:session".into(),
                reply_route: REPLY_ROUTE.into(),
                in_reply_to: "platform-in-1".into(),
            },
        },
    ]);
    Fixture {
        server: Arc::new(server),
        root,
        helper_dir,
    }
}

fn fixture(helper_body: &str) -> Fixture {
    fixture_with_state_root(helper_body, false)
}

fn helper_calls(fixture: &Fixture) -> usize {
    std::fs::read_to_string(fixture.helper_dir.join("calls.txt"))
        .unwrap_or_default()
        .lines()
        .count()
}

#[tokio::test]
async fn accepted_reply_uses_original_route_full_body_and_is_never_resent() {
    let fixture = fixture(&standard_helper(accepted_response(), false));
    let result = deliver_committed_reply(fixture.server.clone(), MESSAGE_ID.into())
        .await
        .unwrap();
    assert_eq!(result.status, ChannelDeliveryStatus::Accepted);
    assert_eq!(
        result.platform_message_id.as_deref(),
        Some("platform-out-1")
    );
    assert_eq!(result.platform_ref_idx.as_deref(), Some("7"));
    assert_eq!(
        result.platform_timestamp,
        Some(PlatformTimestamp::Text("2026-11-08T12:00:00Z".into()))
    );
    assert_eq!(helper_calls(&fixture), 1);

    let arguments = std::fs::read_to_string(fixture.helper_dir.join("args.txt")).unwrap();
    assert_eq!(
        arguments.lines().collect::<Vec<_>>(),
        vec![
            "--runtime-root",
            fixture.helper_dir.join("runtime").to_str().unwrap(),
            "--state-root",
            fixture.helper_dir.join("state").to_str().unwrap(),
        ]
    );
    let request: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(fixture.helper_dir.join("stdin.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(
        request,
        json!({
            "message_id": MESSAGE_ID,
            "scope": "c2c",
            "target_id": "user-1",
            "content": "[agent]\nfull reply body\nsecond line",
        })
    );

    let repeated = deliver_committed_reply(fixture.server.clone(), MESSAGE_ID.into())
        .await
        .unwrap();
    assert_eq!(repeated, result);
    assert_eq!(helper_calls(&fixture), 1);
    let mut state = fixture.server.state.lock().unwrap();
    assert_eq!(state.msgs[MESSAGE_ID].state, "pending");
    assert_eq!(
        state.channel_delivery_results[MESSAGE_ID].status,
        ChannelDeliveryStatus::Accepted
    );
    let downgrade = persist_result_locked(
        &fixture.server,
        &mut state,
        ChannelDeliveryResult {
            message_id: MESSAGE_ID.into(),
            channel_instance: CHANNEL_INSTANCE.into(),
            status: ChannelDeliveryStatus::Uncertain,
            platform_message_id: None,
            error: Some("late downgrade".into()),
            platform_timestamp: None,
            platform_ref_idx: None,
            updated_ms: 0,
        },
    )
    .unwrap_err();
    assert!(
        downgrade.contains("CHANNEL_DELIVERY_RESULT_FINAL"),
        "{downgrade}"
    );
    assert_eq!(
        state.channel_delivery_results[MESSAGE_ID]
            .platform_message_id
            .as_deref(),
        Some("platform-out-1")
    );
    drop(state);
    std::fs::remove_dir_all(fixture.root).ok();
}

#[tokio::test]
async fn missing_configuration_is_a_durable_failure_without_crashing_or_spawning() {
    let mut fixture = fixture(&standard_helper(accepted_response(), false));
    Arc::get_mut(&mut fixture.server)
        .unwrap()
        .config
        .qqbot
        .delivery = None;
    let result = deliver_committed_reply(fixture.server.clone(), MESSAGE_ID.into())
        .await
        .unwrap();
    assert_eq!(result.status, ChannelDeliveryStatus::Failed);
    assert!(result
        .error
        .as_deref()
        .unwrap_or_default()
        .contains("QQBOT_DELIVERY_NOT_CONFIGURED"));
    assert_eq!(helper_calls(&fixture), 0);
    let state = fixture.server.state.lock().unwrap();
    assert_eq!(
        state.channel_delivery_results[MESSAGE_ID].status,
        ChannelDeliveryStatus::Failed
    );
    assert_eq!(state.msgs[MESSAGE_ID].state, "pending");
    drop(state);
    std::fs::remove_dir_all(fixture.root).ok();
}

#[tokio::test]
async fn malformed_original_route_fails_without_helper_or_target_guessing() {
    let (mut server, root) = test_server();
    let helper_dir = root.join("qqbot-helper");
    let helper = write_helper(&helper_dir, &standard_helper(accepted_response(), false));
    let runtime_root = helper_dir.join("runtime");
    let state_root = helper_dir.join("state");
    std::fs::create_dir_all(&runtime_root).unwrap();
    std::fs::create_dir_all(&state_root).unwrap();
    server.config.qqbot.delivery = Some(QqBotDelivery {
        node_program: PathBuf::from("/bin/sh"),
        helper_path: helper,
        runtime_root,
        state_root,
        deadline_ms: 2_000,
    });
    let malformed_route = "qqbot://c2c";
    server.commit(&[
        Event::Sent {
            msg: Message {
                id: MESSAGE_ID.into(),
                from: "agent".into(),
                to: malformed_route.into(),
                mtype: "reply".into(),
                subject: Some("answer".into()),
                body: "body".into(),
                in_reply_to: Some("platform-in-1".into()),
                created_ms: now_ms(),
                state: "pending".into(),
                wake_attempt_count: 0,
                last_wake_attempt_ms: 0,
                retry_attempted: false,
            },
        },
        Event::ChannelReplyRouteBound {
            message_id: MESSAGE_ID.into(),
            route: ChannelReplyRoute {
                source_channel: "qqbot".into(),
                channel_instance: CHANNEL_INSTANCE.into(),
                external_conversation: "u1:session".into(),
                reply_route: malformed_route.into(),
                in_reply_to: "platform-in-1".into(),
            },
        },
    ]);
    let result = deliver_committed_reply(Arc::new(server), MESSAGE_ID.into())
        .await
        .unwrap();
    assert_eq!(result.status, ChannelDeliveryStatus::Failed);
    assert!(result
        .error
        .as_deref()
        .unwrap_or_default()
        .contains("QQBOT_ROUTE_INVALID"));
    assert_eq!(
        std::fs::read_to_string(helper_dir.join("calls.txt"))
            .unwrap_or_default()
            .lines()
            .count(),
        0
    );
    std::fs::remove_dir_all(root).ok();
}

#[tokio::test]
async fn explicit_pre_send_failure_is_durable_and_not_retried() {
    let response = r#"{"kind":"failed","message_id":"channel-reply-out-1","code":"QQBOT_CREDENTIALS_MISSING","error":"credentials are unavailable"}"#;
    let fixture = fixture(&standard_helper(response, false));
    let result = deliver_committed_reply(fixture.server.clone(), MESSAGE_ID.into())
        .await
        .unwrap();
    assert_eq!(result.status, ChannelDeliveryStatus::Failed);
    assert_eq!(
        result.error.as_deref(),
        Some("QQBOT_CREDENTIALS_MISSING: credentials are unavailable")
    );
    assert_eq!(helper_calls(&fixture), 1);
    let repeated = deliver_committed_reply(fixture.server.clone(), MESSAGE_ID.into())
        .await
        .unwrap();
    assert_eq!(repeated, result);
    assert_eq!(helper_calls(&fixture), 1);
    std::fs::remove_dir_all(fixture.root).ok();
}

#[tokio::test]
async fn helper_eof_after_start_is_uncertain_and_not_retried() {
    let fixture = fixture(&standard_helper("", false));
    let result = deliver_committed_reply(fixture.server.clone(), MESSAGE_ID.into())
        .await
        .unwrap();
    assert_eq!(result.status, ChannelDeliveryStatus::Uncertain);
    assert!(result
        .error
        .as_deref()
        .unwrap_or_default()
        .contains("QQBOT_HELPER_RESPONSE_UNKNOWN"));
    assert_eq!(helper_calls(&fixture), 1);
    let repeated = deliver_committed_reply(fixture.server.clone(), MESSAGE_ID.into())
        .await
        .unwrap();
    assert_eq!(repeated, result);
    assert_eq!(helper_calls(&fixture), 1);
    std::fs::remove_dir_all(fixture.root).ok();
}

#[tokio::test]
async fn helper_deadline_bounds_the_whole_call_and_is_not_retried() {
    let mut fixture = fixture(&standard_helper(accepted_response(), true));
    Arc::get_mut(&mut fixture.server)
        .unwrap()
        .config
        .qqbot
        .delivery
        .as_mut()
        .unwrap()
        .deadline_ms = 25;
    let result = deliver_committed_reply(fixture.server.clone(), MESSAGE_ID.into())
        .await
        .unwrap();
    assert_eq!(result.status, ChannelDeliveryStatus::Uncertain);
    assert!(result
        .error
        .as_deref()
        .unwrap_or_default()
        .contains("QQBOT_HELPER_DEADLINE_UNKNOWN"));
    let repeated = deliver_committed_reply(fixture.server.clone(), MESSAGE_ID.into())
        .await
        .unwrap();
    assert_eq!(repeated, result);
    assert_eq!(helper_calls(&fixture), 1);
    std::fs::remove_dir_all(fixture.root).ok();
}

#[tokio::test]
async fn persisted_sending_projects_uncertain_after_restart_without_resend() {
    let fixture = fixture(&standard_helper(accepted_response(), false));
    fixture
        .server
        .commit(&[Event::ChannelDeliveryResultRecorded {
            result: ChannelDeliveryResult {
                message_id: MESSAGE_ID.into(),
                channel_instance: CHANNEL_INSTANCE.into(),
                status: ChannelDeliveryStatus::Sending,
                platform_message_id: None,
                error: None,
                platform_timestamp: None,
                platform_ref_idx: None,
                updated_ms: now_ms(),
            },
        }]);
    let result = deliver_committed_reply(fixture.server.clone(), MESSAGE_ID.into())
        .await
        .unwrap();
    assert_eq!(result.status, ChannelDeliveryStatus::Uncertain);
    assert!(result
        .error
        .as_deref()
        .unwrap_or_default()
        .contains("CHANNEL_DELIVERY_SENDING_UNRECOVERED"));
    assert_eq!(helper_calls(&fixture), 0);
    assert_eq!(
        fixture
            .server
            .state
            .lock()
            .unwrap()
            .channel_delivery_results[MESSAGE_ID]
            .status,
        ChannelDeliveryStatus::Sending
    );
    std::fs::remove_dir_all(fixture.root).ok();
}

#[tokio::test]
async fn concurrent_duplicates_share_one_send_claim() {
    let fixture = fixture(&standard_helper(accepted_response(), true));
    let first = tokio::spawn(deliver_committed_reply(
        fixture.server.clone(),
        MESSAGE_ID.into(),
    ));
    let second = tokio::spawn(deliver_committed_reply(
        fixture.server.clone(),
        MESSAGE_ID.into(),
    ));
    let first = first.await.unwrap().unwrap();
    let second = second.await.unwrap().unwrap();
    assert!(
        first.status == ChannelDeliveryStatus::Accepted
            || second.status == ChannelDeliveryStatus::Accepted
    );
    assert_ne!(first.status, ChannelDeliveryStatus::Failed);
    assert_ne!(second.status, ChannelDeliveryStatus::Failed);
    assert_eq!(helper_calls(&fixture), 1);
    std::fs::remove_dir_all(fixture.root).ok();
}

#[tokio::test]
async fn final_result_persistence_failure_keeps_sending_and_reports_uncertain() {
    let block_state = r#"#!/bin/sh
HERE=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
printf 'called\n' >> "$HERE/calls.txt"
cat > "$HERE/stdin.json"
rm -rf -- "$4"
printf '%s\n' 'not a directory' > "$4"
printf '%s\n' '{"kind":"accepted","message_id":"channel-reply-out-1","platform_message_id":"platform-out-1","timestamp":"2026-11-08T12:00:00Z"}'
"#;
    let fixture = fixture_with_state_root(block_state, true);
    let error = deliver_committed_reply(fixture.server.clone(), MESSAGE_ID.into())
        .await
        .unwrap_err();
    assert!(
        error.contains("CHANNEL_DELIVERY_RESULT_UNCERTAIN"),
        "{error}"
    );
    assert_eq!(
        fixture
            .server
            .state
            .lock()
            .unwrap()
            .channel_delivery_results[MESSAGE_ID]
            .status,
        ChannelDeliveryStatus::Sending
    );
    let projected = deliver_committed_reply(fixture.server.clone(), MESSAGE_ID.into())
        .await
        .unwrap();
    assert_eq!(projected.status, ChannelDeliveryStatus::Uncertain);
    assert_eq!(helper_calls(&fixture), 1);
    std::fs::remove_dir_all(fixture.root).ok();
}

#[test]
fn legacy_accepted_result_without_new_platform_fields_still_deserializes() {
    let result: ChannelDeliveryResult = serde_json::from_value(json!({
        "message_id": "legacy",
        "channel_instance": "qqbot",
        "status": "accepted",
        "platform_message_id": "platform-legacy",
        "updated_ms": 1
    }))
    .unwrap();
    assert_eq!(result.status, ChannelDeliveryStatus::Accepted);
    assert!(result.platform_timestamp.is_none());
    assert!(result.platform_ref_idx.is_none());
}
