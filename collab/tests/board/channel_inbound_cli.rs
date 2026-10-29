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
