//! Native non-tmux public CLI enrollment, takeover and cross-project consumer.
use super::*;

const NEXT: &str = "01a0d641-13f1-7b31-8d84-530f0f2d3fdd";
const OUTSIDER: &str = "01a0d641-13f1-7b31-8d84-530f0f2d3fee";

fn command(host: &AppFixture, native: &AppFixture, thread: &str, args: &[&str]) -> Command {
    let mut command = Command::new(binary());
    command
        .args(args)
        .current_dir(&native.project)
        .env("COLLAB_STATE_DIR", &host.host_state)
        .env("HOME", host.root.join("home"))
        .env("CODEX_HOME", host.root.join("home"))
        .env("COLLAB_APPSERVER_SOCKET", &native.app_socket)
        .env("CODEX_INTERNAL_ORIGINATOR_OVERRIDE", "Codex TUI")
        .env("CODEX_SESSION_ID", thread)
        .env("CODEX_THREAD_ID", thread);
    for key in [
        "TMUX",
        "TMUX_PANE",
        "REX_SERVER",
        "REX_SESSION",
        "REX_BLOCK",
        "COLLAB_SOCKET_PATH",
        "COLLAB_HOST_SOCKET",
        "COLLAB_LOCK_PATH",
        "COLLAB_HOST_LOCK",
        "CODEX_APP_SERVER_SOCKET",
        "COLLAB_APPSERVER_NAMESPACE",
        "COLLAB_WORKER",
        "DSH_SESSION_ID",
        "AGY_SESSION_ID",
    ] {
        command.env_remove(key);
    }
    command
}

fn ok(host: &AppFixture, native: &AppFixture, thread: &str, args: &[&str]) -> Value {
    let output = command(host, native, thread, args).output().unwrap();
    assert!(
        output.status.success(),
        "{args:?}: {} {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

fn denied(
    host: &AppFixture,
    native: &AppFixture,
    thread: &str,
    args: &[&str],
    expected: Option<&str>,
) {
    let output = command(host, native, thread, args).output().unwrap();
    let text = format!(
        "{} {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !output.status.success(),
        "unexpected admission {args:?}: {text}"
    );
    if let Some(expected) = expected {
        assert!(text.contains(expected), "{text}");
    }
}

fn continuity(replace_sender: bool) {
    let source = AppFixture::with_named_project(false, "continuity-source");
    let target = AppFixture::with_named_project(false, "continuity-target");
    let unrelated = AppFixture::with_named_project(false, "continuity-unrelated");
    ok(&source, &source, THREAD_A, &["context"]);
    ok(&source, &target, THREAD_B, &["context"]);
    ok(&source, &unrelated, THREAD_A, &["context"]);
    ok(&source, &source, OUTSIDER, &["context"]);
    ok(&source, &target, OUTSIDER, &["context"]);
    let sent = ok(
        &source,
        &source,
        THREAD_A,
        &[
            "send",
            "continuity-target.Alice",
            "--subject",
            "continuity",
            "original body",
        ],
    );
    let id = sent["msg_id"].as_str().unwrap();
    let reference = sent["message_ref"].as_str().unwrap();
    let received = ok(&source, &target, THREAD_B, &["recv"]);
    assert!(received["messages"]
        .as_array()
        .unwrap()
        .iter()
        .any(|m| m["id"] == id));
    let before = source.current_state_for(&target.project, "appserver-cli");
    assert_eq!(
        ok(&source, &source, THREAD_A, &["msg", reference])["consumed_by_recv"],
        true
    );
    let sender = if replace_sender {
        ok(
            &source,
            &source,
            NEXT,
            &[
                "context",
                "--approve-grant",
                r#"{"action":"restore_name","requested_name":"Alice","authorization":"fixture user replaces source Alice","replace_confirmed":true}"#,
            ],
        );
        NEXT
    } else {
        THREAD_A
    };
    assert_eq!(
        ok(&source, &source, sender, &["msg", reference])["body"],
        "original body"
    );
    let pending = ok(
        &source,
        &source,
        sender,
        &[
            "send",
            "continuity-target.Alice",
            "--subject",
            "pending handoff",
            "pending body",
        ],
    );
    ok(
        &source,
        &target,
        NEXT,
        &[
            "context",
            "--approve-grant",
            r#"{"action":"restore_name","requested_name":"Alice","authorization":"fixture user replaces target Alice","replace_confirmed":true}"#,
        ],
    );
    assert_eq!(
        ok(&source, &target, NEXT, &["msg", id])["consumed_by_recv"],
        true
    );
    assert_eq!(
        ok(&source, &target, NEXT, &["msg", reference])["body"],
        "original body"
    );
    let pending_received = ok(&source, &target, NEXT, &["recv"]);
    assert_eq!(
        pending_received["messages"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|m| m["id"] == pending["msg_id"])
            .count(),
        1
    );
    assert!(
        !pending_received["messages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|m| m["id"] == id),
        "consumed original must not replay"
    );
    for (native, thread) in [
        (&target, THREAD_B),
        (&target, OUTSIDER),
        (&source, OUTSIDER),
    ] {
        denied(&source, native, thread, &["msg", reference], None);
        denied(
            &source,
            native,
            thread,
            &[
                "send",
                "continuity-source.Alice",
                "--subject",
                "forged",
                "no",
                "--in-reply-to",
                reference,
            ],
            None,
        );
    }
    if replace_sender {
        denied(&source, &source, THREAD_A, &["msg", reference], None);
        denied(
            &source,
            &source,
            THREAD_A,
            &[
                "send",
                "continuity-target.Alice",
                "--subject",
                "stale sender",
                "no",
                "--in-reply-to",
                reference,
            ],
            None,
        );
    }
    denied(
        &source,
        &unrelated,
        THREAD_A,
        &["msg", reference],
        Some("CALLER_NOT_PERMITTED"),
    );
    denied(
        &source,
        &unrelated,
        THREAD_A,
        &[
            "send",
            "continuity-source.Alice",
            "--subject",
            "foreign Alice",
            "no",
            "--in-reply-to",
            reference,
        ],
        Some("IN_REPLY_TO_OWNER_MISMATCH"),
    );
    // Former carriers can register another name, but that new current binding
    // must not regain the original Alice message through historical raw IDs.
    let former = ok(&source, &target, THREAD_B, &["context"]);
    assert_ne!(former["name"], "Alice");
    denied(
        &source,
        &target,
        THREAD_B,
        &["msg", id],
        Some("CALLER_NOT_PERMITTED"),
    );
    denied(
        &source,
        &target,
        THREAD_B,
        &[
            "send",
            "continuity-source.Alice",
            "--subject",
            "former holder",
            "no",
            "--in-reply-to",
            reference,
        ],
        Some("IN_REPLY_TO_PARTICIPANT_MISMATCH"),
    );
    if replace_sender {
        let former = ok(&source, &source, THREAD_A, &["context"]);
        assert_ne!(former["name"], "Alice");
        denied(
            &source,
            &source,
            THREAD_A,
            &["msg", reference],
            Some("CALLER_NOT_PERMITTED"),
        );
    }
    denied(
        &source,
        &source,
        sender,
        &["msg", id],
        Some("MESSAGE_NOT_FOUND"),
    );
    denied(
        &source,
        &source,
        sender,
        &[
            "send",
            "continuity-target.Alice",
            "--subject",
            "wrong reversal",
            "no",
            "--in-reply-to",
            reference,
        ],
        Some("IN_REPLY_TO_OWNER_MISMATCH"),
    );
    denied(
        &source,
        &target,
        NEXT,
        &[
            "send",
            "continuity-source.Bob",
            "--subject",
            "wrong recipient",
            "no",
            "--in-reply-to",
            reference,
        ],
        Some("IN_REPLY_TO_PARTICIPANT_MISMATCH"),
    );
    for input in [id, reference] {
        for policy in ["--reply-required", "--reply-on-completion"] {
            denied(
                &source,
                &target,
                NEXT,
                &[
                    "send",
                    "continuity-source.Alice",
                    "--subject",
                    "invalid policy",
                    "no",
                    "--in-reply-to",
                    input,
                    policy,
                ],
                Some("UNSUPPORTED"),
            );
        }
    }
    assert_eq!(
        source.current_state()["msgs"]
            .as_object()
            .map_or(0, |m| m.len()),
        0
    );
    assert_eq!(
        ok(&source, &source, sender, &["msg", reference])["answered"],
        false
    );
    let reply = ok(
        &source,
        &target,
        NEXT,
        &[
            "send",
            "continuity-source.Alice",
            "--subject",
            "reply",
            "answer",
            "--in-reply-to",
            reference,
        ],
    );
    let reply_id = reply["msg_id"].as_str().unwrap();
    for (native, thread) in [(&source, sender), (&target, NEXT)] {
        assert_eq!(
            ok(&source, native, thread, &["msg", reference])["answered"],
            true
        );
    }
    let received = ok(&source, &source, sender, &["recv"]);
    assert_eq!(
        received["messages"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|m| m["id"] == reply_id)
            .count(),
        1
    );
    assert!(ok(&source, &source, sender, &["recv"])["messages"]
        .as_array()
        .unwrap()
        .is_empty());
    assert_eq!(
        ok(
            &source,
            &target,
            NEXT,
            &["msg", reply["message_ref"].as_str().unwrap()]
        )["consumed_by_recv"],
        true
    );
    let legacy = ok(
        &source,
        &target,
        NEXT,
        &[
            "send",
            "continuity-source.Alice",
            "--subject",
            "raw reply",
            "second answer",
            "--in-reply-to",
            id,
        ],
    );
    assert_eq!(
        source.current_state()["msgs"][legacy["msg_id"].as_str().unwrap()]["in_reply_to"],
        id
    );
    let after = source.current_state_for(&target.project, "appserver-cli");
    assert_eq!(after["msgs"][id], before["msgs"][id]);
    for (receive_id, receipt) in before["receive_receipts"].as_object().unwrap() {
        assert_eq!(after["receive_receipts"][receive_id], *receipt);
    }
    assert_eq!(after["msgs"].as_object().unwrap().len(), 2);
    assert_eq!(source.current_state()["msgs"].as_object().unwrap().len(), 2);
}

#[test]
fn sender_and_consumed_recipient_continue_after_public_takeover() {
    continuity(true);
}

#[test]
fn consumed_recipient_can_reply_after_public_takeover() {
    continuity(false);
}

#[test]
fn consumed_recipient_can_send_linked_reply_after_public_takeover() {
    let source = AppFixture::with_named_project(false, "continuity-source");
    let target = AppFixture::with_named_project(false, "continuity-target");
    ok(&source, &source, THREAD_A, &["context"]);
    ok(&source, &target, THREAD_B, &["context"]);
    let sent = ok(
        &source,
        &source,
        THREAD_A,
        &[
            "send",
            "continuity-target.Alice",
            "--subject",
            "recipient red",
            "original",
        ],
    );
    ok(&source, &target, THREAD_B, &["recv"]);
    ok(
        &source,
        &target,
        NEXT,
        &[
            "context",
            "--approve-grant",
            r#"{"action":"restore_name","requested_name":"Alice","authorization":"fixture recipient takeover","replace_confirmed":true}"#,
        ],
    );
    ok(
        &source,
        &target,
        NEXT,
        &[
            "send",
            "continuity-source.Alice",
            "--subject",
            "reply",
            "answer",
            "--in-reply-to",
            sent["message_ref"].as_str().unwrap(),
        ],
    );
    assert_eq!(
        ok(
            &source,
            &source,
            THREAD_A,
            &["msg", sent["message_ref"].as_str().unwrap()]
        )["answered"],
        true
    );
}
