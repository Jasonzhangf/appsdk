use super::*;

fn channel_envelope(platform_id: &str, reply_route: &str) -> String {
    json!({
        "source_channel": "qqbot",
        "channel_instance": "qqbot",
        "external_user": "qq-user-1",
        "external_conversation": "conversation-1",
        "platform_message_id": platform_id,
        "reply_route": reply_route,
        "recipient_address": "Alice",
        "attachments": []
    })
    .to_string()
}

/// The adapter-only envelope must survive the public `sendmessage` entry so a
/// QQBot reply returns through the original external route, not the Codex
/// window that happens to hold the recipient.
#[test]
fn channel_envelope_reaches_recv_and_the_reply_uses_the_original_route() {
    let fixture = AppFixture::new();
    let alice = fixture.run_public(&["context"], THREAD_A);
    assert_eq!(alice["name"], "Alice");

    let sent = fixture.run_public(
        &[
            "sendmessage",
            "--unknown-sender",
            "--to",
            "Alice",
            "--subject",
            "QQBot",
            "--content",
            "hello from qq",
            "--channel-envelope",
            &channel_envelope("platform-1", "qqbot://c2c/conversation-1"),
        ],
        THREAD_A,
    );
    let id = sent["msg_id"].as_str().expect("channel inbound msg id");

    let received = fixture.run_public(&["recv", "--timeout", "0"], THREAD_A);
    let message = received["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|message| message["id"] == id)
        .expect("channel inbound message consumed");
    assert_eq!(message["body"], "来源：QQBot\n消息：hello from qq");
    assert_eq!(
        message["channel"]["reply_route"], "qqbot://c2c/conversation-1",
        "{message}"
    );
    assert_eq!(
        message["channel"]["external_user"], "qq-user-1",
        "{message}"
    );
    assert_eq!(
        message["channel"]["platform_message_id"], "platform-1",
        "{message}"
    );

    let reply = fixture.run_public(
        &[
            "sendmessage",
            "--in-reply-to",
            id,
            "--content",
            "reply body",
        ],
        THREAD_A,
    );
    let reply_id = reply["msg_id"].as_str().expect("reply msg id");
    let view = fixture.run_public(&["msg", reply_id], THREAD_A);
    assert_eq!(
        view["to"], "qqbot://c2c/conversation-1",
        "reply must keep the original external route: {view}"
    );
}

#[test]
fn a_duplicate_platform_event_is_delivered_once() {
    let fixture = AppFixture::new();
    fixture.run_public(&["context"], THREAD_A);
    let envelope = channel_envelope("platform-dup", "qqbot://c2c/conversation-1");
    let args = [
        "sendmessage",
        "--unknown-sender",
        "--to",
        "Alice",
        "--subject",
        "QQBot",
        "--content",
        "same event",
        "--channel-envelope",
        envelope.as_str(),
    ];
    let first = fixture.run_public(&args, THREAD_A);
    let second = fixture.run_public(&args, THREAD_A);
    assert_eq!(first["msg_id"], second["msg_id"], "{second}");
    assert_eq!(second["deduplicated"], true, "{second}");

    let received = fixture.run_public(&["recv", "--timeout", "0"], THREAD_A);
    let count = received["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|message| {
            message["subject"] == "QQBot" && message["body"] == "来源：QQBot\n消息：same event"
        })
        .count();
    assert_eq!(
        count, 1,
        "duplicate platform event must deliver once: {received}"
    );
}

#[test]
fn malformed_channel_envelope_is_rejected_before_delivery() {
    let fixture = AppFixture::new();
    fixture.run_public(&["context"], THREAD_A);
    let output = fixture.command_public(
        &[
            "sendmessage",
            "--unknown-sender",
            "--to",
            "Alice",
            "--subject",
            "QQBot",
            "--content",
            "hello",
            "--channel-envelope",
            "{",
        ],
        THREAD_A,
    );
    assert!(!output.status.success(), "malformed envelope must fail");
    let text = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        text.contains("CHANNEL_ENVELOPE_INVALID"),
        "malformed envelope must name the parse error: {text}"
    );
}

#[test]
fn qqbot_ceo_address_requires_current_grant_and_reaches_its_holder() {
    let fixture = AppFixture::new();
    let ceo = fixture.run_public(&["context"], THREAD_A);
    fixture.run_public(&["context"], THREAD_B);
    let ceo_id = ceo["identity"]["worker_id"].as_str().unwrap();
    let envelope = channel_envelope("ceo-before-grant", "qqbot://c2c/conversation-1");
    let missing = fixture.command_public(
        &[
            "sendmessage",
            "--unknown-sender",
            "--to",
            "ceo",
            "--subject",
            "QQBot",
            "--content",
            "must not fall back",
            "--channel-envelope",
            &envelope,
        ],
        THREAD_A,
    );
    assert!(
        !missing.status.success(),
        "missing CEO grant must not route to a peer"
    );
    let peer_inbox = fixture.run_public(&["recv", "--timeout", "0"], THREAD_B);
    assert!(
        peer_inbox["messages"].as_array().unwrap().is_empty(),
        "{peer_inbox}"
    );

    let promoted = fixture.run_public(
        &["ceo", "promote", "--approval", "user approved isolated CEO"],
        THREAD_A,
    );
    assert_eq!(promoted["is_current"], true, "{promoted}");
    let envelope = channel_envelope("ceo-after-grant", "qqbot://c2c/conversation-1");
    let sent = fixture.run_public(
        &[
            "sendmessage",
            "--unknown-sender",
            "--to",
            "ceo",
            "--subject",
            "QQBot",
            "--content",
            "CEO inbound",
            "--channel-envelope",
            &envelope,
        ],
        THREAD_A,
    );
    let inbox = fixture.run_public(&["recv", "--timeout", "0"], THREAD_A);
    let message = inbox["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|message| message["id"] == sent["msg_id"])
        .expect("CEO receives the inbound message");
    assert_eq!(message["to"], ceo_id, "{message}");
    assert!(
        message["body"].as_str().unwrap().contains("CEO inbound"),
        "{message}"
    );
    let peer_inbox = fixture.run_public(&["recv", "--timeout", "0"], THREAD_B);
    assert!(
        peer_inbox["messages"]
            .as_array()
            .unwrap()
            .iter()
            .all(|message| message["id"] != sent["msg_id"]),
        "{peer_inbox}"
    );

    let replacement = fixture.run_public(
        &[
            "ceo",
            "promote",
            "--approval",
            "user approved replacement CEO",
        ],
        THREAD_B,
    );
    assert_eq!(replacement["is_current"], true, "{replacement}");
    let envelope = channel_envelope("ceo-after-transfer", "qqbot://c2c/conversation-1");
    let sent = fixture.run_public(
        &[
            "sendmessage",
            "--unknown-sender",
            "--to",
            "ceo",
            "--subject",
            "QQBot",
            "--content",
            "current CEO only",
            "--channel-envelope",
            &envelope,
        ],
        THREAD_A,
    );
    let replacement_inbox = fixture.run_public(&["recv", "--timeout", "0"], THREAD_B);
    assert!(
        replacement_inbox["messages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|message| message["id"] == sent["msg_id"]),
        "{replacement_inbox}"
    );
    let previous_inbox = fixture.run_public(&["recv", "--timeout", "0"], THREAD_A);
    assert!(
        previous_inbox["messages"]
            .as_array()
            .unwrap()
            .iter()
            .all(|message| message["id"] != sent["msg_id"]),
        "{previous_inbox}"
    );
}

#[test]
fn unread_qqbot_message_follows_the_named_holder_on_handoff() {
    let mut fixture = AppFixture::new();
    let alice = fixture.run_public(&["context"], THREAD_A);
    fixture.run_public(&["context"], THREAD_B);
    let envelope = channel_envelope("qq-handoff-event", "qqbot://c2c/conversation-1");
    let sent = fixture.run_public(
        &[
            "sendmessage",
            "--unknown-sender",
            "--to",
            "Alice",
            "--subject",
            "QQBot",
            "--content",
            "unread handoff",
            "--channel-envelope",
            &envelope,
        ],
        THREAD_A,
    );
    let project = fixture
        .project
        .canonicalize()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    fixture.update_current_state(|state| state["agent_presence"][&project][alice["identity"]["worker_id"].as_str().unwrap()] = json!({
        "project_scope":project,
        "worker_id":alice["identity"]["worker_id"], "session_id":THREAD_A,
        "presence":"online", "reason":"isolated fixture", "observed_ms":1
    }));
    let next = fixture.run_public(&["context", "--approve-grant",
        r#"{"action":"restore_name","requested_name":"Alice","authorization":"fixture confirmed handoff","replace_confirmed":true}"#], THREAD_B);
    let received = fixture.run_public(&["recv", "--timeout", "0"], THREAD_B);
    let message = received["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|message| message["id"] == sent["msg_id"])
        .expect("unread QQBot message must follow Alice");
    assert_eq!(message["to"], next["identity"]["worker_id"], "{message}");
    assert!(
        message["body"].as_str().unwrap().contains("unread handoff"),
        "{message}"
    );
}

#[test]
fn qqbot_receiver_delivers_only_to_the_current_ceo() {
    let fixture = AppFixture::new();
    let ceo = fixture.run_public(&["context"], THREAD_A);
    fixture.run_public(&["context"], THREAD_B);
    let runtime = fixture.root.join("qqbot-test-runtime");
    let state = runtime.join("state");
    let sdk = runtime.join("node_modules/@tencent-connect/qqbot-nodejs");
    std::fs::create_dir_all(&state).unwrap();
    std::fs::create_dir_all(&sdk).unwrap();
    std::fs::create_dir_all(fixture.root.join("home")).unwrap();
    std::fs::copy(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts/qqbot-receiver.mjs"),
        runtime.join("receiver.mjs"),
    )
    .unwrap();
    std::fs::write(
        sdk.join("package.json"),
        r#"{"type":"module","exports":"./index.js"}"#,
    )
    .unwrap();
    std::fs::write(
        sdk.join("index.js"),
        r#"
export class QQBot {
  constructor() { this.handlers = {}; }
  on(name, callback) { this.handlers[name] = callback; }
  async start() {
    setTimeout(() => this.handlers.message(null, {
      kind: 'c2c', messageId: process.env.FIXTURE_MESSAGE_ID,
      senderId: 'fixture-user', content: 'receiver event', attachments: [],
      replyTarget: { scope: 'c2c', targetId: 'fixture-conversation' },
    }), 0);
  }
  async sendText() { return {}; }
  stop() {}
}
"#,
    )
    .unwrap();
    std::fs::write(
        state.join("credentials.json"),
        r#"[{"appId":"fixture","appSecret":"fixture"}]"#,
    )
    .unwrap();
    std::fs::write(
        state.join("target.json"),
        r#"{"scope":"c2c","targetId":"fixture-conversation"}"#,
    )
    .unwrap();
    std::fs::write(
        state.join("channel-owner.json"),
        json!({
            "owner_project": fixture.project.canonicalize().unwrap(),
            "owner_worker_id": ceo["identity"]["worker_id"], "generation": 1
        })
        .to_string(),
    )
    .unwrap();
    let run_receiver = |message_id: &str| {
        Command::new("node")
            .arg(runtime.join("receiver.mjs").canonicalize().unwrap())
            .current_dir(&fixture.project)
            .env("COLLAB_BIN", binary())
            .env("COLLAB_STATE_DIR", &fixture.host_state)
            .env("HOME", fixture.root.join("home"))
            .env("CODEX_HOME", fixture.root.join("home"))
            .env("QQBOT_STATE_DIR", &state)
            .env("QQBOT_BATCH_WINDOW_MS", "10")
            .env("QQBOT_BATCH_MAX_WAIT_MS", "100")
            .env("FIXTURE_MESSAGE_ID", message_id)
            .env_remove("TMUX")
            .env_remove("TMUX_PANE")
            .output()
            .expect("run isolated QQBot receiver")
    };
    let missing = run_receiver("receiver-no-ceo");
    assert!(
        missing.status.success(),
        "{}",
        String::from_utf8_lossy(&missing.stderr)
    );
    let missing_output = String::from_utf8_lossy(&missing.stdout);
    assert!(
        missing_output.contains("CEO_GRANT_MISSING"),
        "{missing_output}"
    );
    assert!(
        !state.join("inbox.jsonl").exists(),
        "receiver must not save unused message history"
    );
    assert!(
        !state.join("receipts.jsonl").exists(),
        "receiver must not save unused receipt history"
    );
    let peer = fixture.run_public(&["recv", "--timeout", "0"], THREAD_B);
    assert!(peer["messages"].as_array().unwrap().is_empty(), "{peer}");

    fixture.run_public(
        &["ceo", "promote", "--approval", "user approved isolated CEO"],
        THREAD_A,
    );
    let delivered = run_receiver("receiver-current-ceo");
    assert!(
        delivered.status.success(),
        "{}",
        String::from_utf8_lossy(&delivered.stderr)
    );
    let output = String::from_utf8_lossy(&delivered.stdout);
    let record: Value = output
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .find(|record| record["event"] == "message_batch_received_and_delivered")
        .expect("receiver reports delivery");
    assert_eq!(
        record["resolvedWorkerId"], ceo["identity"]["worker_id"],
        "{record}"
    );
    let inbox = fixture.run_public(&["recv", "--timeout", "0"], THREAD_A);
    assert!(
        inbox["messages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|message| message["id"] == record["collabMessageId"]),
        "{inbox}"
    );
    assert!(!state.join("inbox.jsonl").exists());
    assert!(!state.join("receipts.jsonl").exists());
}
