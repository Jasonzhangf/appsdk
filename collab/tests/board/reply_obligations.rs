use super::*;

const THREAD_C: &str = "01a0d642-64c1-7e52-9b73-6f1a2b3c4d5e";

#[test]
fn an_empty_linked_reply_does_not_clear_the_requirement() {
    let fixture = AppFixture::new();
    fixture.run_public(&["context"], THREAD_A);
    fixture.run_public(&["context"], THREAD_B);
    let sent = fixture.run_public(
        &[
            "sendmessage",
            "--to",
            "Bob",
            "--subject",
            "answer",
            "--reply-required",
            "Please answer",
        ],
        THREAD_A,
    );
    let id = sent["msg_id"].as_str().unwrap();
    let error = rejected(
        &fixture,
        &[
            "sendmessage",
            "--to",
            "Alice",
            "--subject",
            "result",
            "--in-reply-to",
            id,
            "   ",
        ],
        THREAD_B,
    );
    assert!(error.contains("REPLY_BODY_REQUIRED"), "{error}");
    assert_eq!(pending(&fixture, THREAD_B).len(), 1);
}

fn pending(fixture: &AppFixture, thread: &str) -> Vec<Value> {
    let snapshot = fixture.run_public(&["context"], thread);
    snapshot
        .get("pending_reply_obligations")
        .unwrap_or_else(|| {
            panic!("context snapshot must project pending_reply_obligations: {snapshot}")
        })
        .as_array()
        .unwrap_or_else(|| panic!("pending_reply_obligations must be an array: {snapshot}"))
        .clone()
}

fn rejected(fixture: &AppFixture, args: &[&str], thread: &str) -> String {
    let result = fixture.command_public(args, thread);
    assert!(
        !result.status.success(),
        "unexpected success: {}",
        String::from_utf8_lossy(&result.stdout)
    );
    format!(
        "{} {}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    )
}

fn assert_reply_projected(message: &Value, msg_id: &str) {
    assert_eq!(message["id"], msg_id, "{message}");
    assert!(
        message
            .get("reply_obligation")
            .is_some_and(|value| !value.is_null()),
        "pending message must project a reply_obligation status: {message}"
    );
}

#[test]
fn required_reply_survives_read_and_ack_and_clears_on_linked_reply() {
    let fixture = AppFixture::new();
    let alice = fixture.run_public(&["context"], THREAD_A);
    fixture.run_public(&["context"], THREAD_B);

    let sent = fixture.run_public(
        &[
            "sendmessage",
            "--to",
            "Bob",
            "--subject",
            "delegated",
            "--reply-required",
            "Please handle this",
        ],
        THREAD_A,
    );
    let msg_id = sent["msg_id"].as_str().unwrap();

    assert_reply_projected(&fixture.run_public(&["msg", msg_id], THREAD_B), msg_id);
    let initial = pending(&fixture, THREAD_B);
    assert_eq!(initial.len(), 1, "{initial:?}");
    assert_eq!(initial[0]["message_id"], msg_id);
    assert_eq!(initial[0]["policy"], "required");
    // `from` must stay answerable: the current carrier of the original sender,
    // which is the exact target a linked reply must address.
    assert_eq!(initial[0]["from"], alice["identity"]["worker_id"]);

    let received = fixture.run_public(
        &["recv", "--receive-id", "required-recv", "--timeout", "0"],
        THREAD_B,
    );
    assert_eq!(received["count"], 1, "{received}");
    assert_reply_projected(&received["messages"][0], msg_id);
    fixture.run_public(&["ack", msg_id], THREAD_B);
    assert_eq!(
        fixture.run_public(&["msg", msg_id], THREAD_B)["state"],
        "read"
    );
    assert_eq!(
        pending(&fixture, THREAD_B).len(),
        1,
        "read/ack is not a reply"
    );

    fixture.run_public(
        &[
            "sendmessage",
            "--to",
            "Alice",
            "--subject",
            "re: delegated",
            "--in-reply-to",
            msg_id,
            "Result delivered",
        ],
        THREAD_B,
    );
    assert!(
        pending(&fixture, THREAD_B).is_empty(),
        "a linked reply from the real recipient clears the obligation"
    );
    assert!(
        pending(&fixture, THREAD_A).is_empty(),
        "the reply itself must not open a new obligation"
    );
}

#[test]
fn on_completion_reply_obligation_stays_visible_until_the_result_arrives() {
    let fixture = AppFixture::new();
    fixture.run_public(&["context"], THREAD_A);
    fixture.run_public(&["context"], THREAD_B);

    let sent = fixture.run_public(
        &[
            "sendmessage",
            "--to",
            "Bob",
            "--subject",
            "build",
            "--reply-on-completion",
            "Report when the build is done",
        ],
        THREAD_A,
    );
    let msg_id = sent["msg_id"].as_str().unwrap();

    let open = pending(&fixture, THREAD_B);
    assert_eq!(open.len(), 1, "{open:?}");
    assert_eq!(open[0]["policy"], "on_completion");
    assert_eq!(open[0]["reminded_idle_episode"], false);

    fixture.run_public(
        &["recv", "--receive-id", "completion-recv", "--timeout", "0"],
        THREAD_B,
    );
    fixture.run_public(&["ack", msg_id], THREAD_B);
    let still_open = pending(&fixture, THREAD_B);
    assert_eq!(still_open.len(), 1, "{still_open:?}");
    assert_eq!(still_open[0]["policy"], "on_completion");

    fixture.run_public(
        &[
            "sendmessage",
            "--to",
            "Alice",
            "--subject",
            "build done",
            "--in-reply-to",
            msg_id,
            "Build finished and verified",
        ],
        THREAD_B,
    );
    assert!(pending(&fixture, THREAD_B).is_empty());
    assert!(pending(&fixture, THREAD_A).is_empty());
}

#[test]
fn a_third_party_or_wrong_target_cannot_clear_the_obligation() {
    let fixture = AppFixture::new();
    fixture.run_public(&["context"], THREAD_A);
    fixture.run_public(&["context"], THREAD_B);
    fixture.run_public(&["context"], THREAD_C);

    let sent = fixture.run_public(
        &[
            "sendmessage",
            "--to",
            "Bob",
            "--subject",
            "guarded",
            "--reply-required",
            "Only Bob may answer Alice",
        ],
        THREAD_A,
    );
    let msg_id = sent["msg_id"].as_str().unwrap();
    assert_eq!(pending(&fixture, THREAD_B).len(), 1);

    let third_party = rejected(
        &fixture,
        &[
            "sendmessage",
            "--to",
            "Alice",
            "--subject",
            "proxy",
            "--in-reply-to",
            msg_id,
            "Carol answers for Bob",
        ],
        THREAD_C,
    );
    assert!(
        third_party.contains("REPLY_OBLIGATION_MISMATCH"),
        "{third_party}"
    );
    assert_eq!(
        pending(&fixture, THREAD_B).len(),
        1,
        "a third party must not clear another recipient's obligation"
    );

    let wrong_target = rejected(
        &fixture,
        &[
            "sendmessage",
            "--to",
            "Carol",
            "--subject",
            "wrong target",
            "--in-reply-to",
            msg_id,
            "Bob answers the wrong peer",
        ],
        THREAD_B,
    );
    assert!(
        wrong_target.contains("REPLY_OBLIGATION_MISMATCH"),
        "{wrong_target}"
    );
    assert_eq!(
        pending(&fixture, THREAD_B).len(),
        1,
        "an answer to the wrong target must keep the obligation"
    );

    fixture.run_public(
        &[
            "sendmessage",
            "--to",
            "Alice",
            "--subject",
            "real answer",
            "--in-reply-to",
            msg_id,
            "Bob answers Alice",
        ],
        THREAD_B,
    );
    assert!(pending(&fixture, THREAD_B).is_empty());
}

#[test]
fn a_plain_send_creates_no_reply_obligation() {
    let fixture = AppFixture::new();
    fixture.run_public(&["context"], THREAD_A);
    fixture.run_public(&["context"], THREAD_B);

    let sent = fixture.run_public(
        &[
            "sendmessage",
            "--to",
            "Bob",
            "--subject",
            "fyi",
            "No answer needed",
        ],
        THREAD_A,
    );
    let msg_id = sent["msg_id"].as_str().unwrap();
    let durable = fixture.run_public(&["msg", msg_id], THREAD_B);
    assert!(
        durable.get("reply_obligation").map_or(true, Value::is_null),
        "a plain message must not project a reply obligation: {durable}"
    );
    assert!(pending(&fixture, THREAD_B).is_empty());
    assert!(pending(&fixture, THREAD_A).is_empty());
}

#[test]
fn mutually_exclusive_reply_flags_are_rejected_explicitly() {
    let fixture = AppFixture::new();
    fixture.run_public(&["context"], THREAD_A);
    fixture.run_public(&["context"], THREAD_B);

    let conflict = rejected(
        &fixture,
        &[
            "sendmessage",
            "--to",
            "Bob",
            "--subject",
            "conflict",
            "--reply-required",
            "--reply-on-completion",
            "body",
        ],
        THREAD_A,
    );
    assert!(
        conflict.contains("cannot be used with") || conflict.contains("conflict"),
        "{conflict}"
    );
}
