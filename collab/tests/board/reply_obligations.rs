use super::*;

const THREAD_C: &str = "01a0d642-64c1-7e52-9b73-6f1a2b3c4d5e";
const THREAD_D: &str = "01a0d643-75d2-8f63-94d5-7b2c3d4e5f6a";

#[test]
fn an_empty_linked_reply_does_not_clear_the_requirement() {
    let fixture = AppFixture::new();
    fixture.run_public(&["context"], THREAD_A);
    let bob = fixture.run_public(&["context"], THREAD_B);
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

fn original_receive_receipt(fixture: &AppFixture, receive_id: &str) -> Value {
    fixture.current_state()["receive_receipts"][receive_id].clone()
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
fn reply_obligation_successor_can_read_consumed_original_and_reply() {
    let fixture = AppFixture::new();
    let alice = fixture.run_public(&["context"], THREAD_A);
    let bob = fixture.run_public(&["context"], THREAD_B);

    let sent = fixture.run_public(
        &[
            "sendmessage",
            "--to",
            "Bob",
            "--subject",
            "successor answer",
            "--reply-required",
            "Answer after the name moves",
        ],
        THREAD_A,
    );
    let msg_id = sent["msg_id"].as_str().unwrap().to_owned();

    let unrelated = fixture.run_public(&["context"], THREAD_C);
    assert_eq!(unrelated["name"], "Carol");
    let unrelated_denied = rejected(&fixture, &["msg", &msg_id], THREAD_C);
    assert!(
        unrelated_denied.contains("CALLER_NOT_PERMITTED"),
        "{unrelated_denied}"
    );
    let unrelated_reply = rejected(
        &fixture,
        &[
            "sendmessage",
            "--to",
            "Alice",
            "--subject",
            "unrelated answer",
            "--in-reply-to",
            &msg_id,
            "An unrelated caller answers",
        ],
        THREAD_C,
    );
    assert!(
        unrelated_reply.contains("REPLY_OBLIGATION_MISMATCH"),
        "{unrelated_reply}"
    );

    let received = fixture.run_public(
        &[
            "recv",
            "--receive-id",
            "successor-original-recv",
            "--timeout",
            "0",
        ],
        THREAD_B,
    );
    assert_eq!(received["count"], 1, "{received}");
    assert_eq!(received["messages"][0]["id"], msg_id);
    let original_record = fixture.current_state()["msgs"][&msg_id].clone();
    assert_eq!(
        pending(&fixture, THREAD_B).len(),
        1,
        "consumption must keep the reply obligation open"
    );
    let original_receipt = original_receive_receipt(&fixture, "successor-original-recv");

    let successor = fixture.run_public(
        &[
            "context",
            "--approve-grant",
            r#"{"action":"restore_name","requested_name":"Bob","authorization":"fixture user replaces Bob","replace_confirmed":true}"#,
        ],
        THREAD_C,
    );
    assert_eq!(successor["name"], "Bob");
    let successor_id = successor["identity"]["worker_id"].as_str().unwrap();
    assert_ne!(successor_id, bob["identity"]["worker_id"].as_str().unwrap());
    let obligations = pending(&fixture, THREAD_C);
    assert_eq!(obligations.len(), 1, "{obligations:?}");
    assert_eq!(obligations[0]["message_id"], msg_id);

    fixture.run_public(&["context"], THREAD_D);
    let wrong_target = rejected(
        &fixture,
        &[
            "sendmessage",
            "--to",
            "Carol",
            "--subject",
            "wrong successor target",
            "--in-reply-to",
            &msg_id,
            "The successor answers the wrong target",
        ],
        THREAD_C,
    );
    assert!(
        wrong_target.contains("REPLY_OBLIGATION_MISMATCH"),
        "{wrong_target}"
    );
    let empty_reply = rejected(
        &fixture,
        &[
            "sendmessage",
            "--to",
            "Alice",
            "--subject",
            "empty successor answer",
            "--in-reply-to",
            &msg_id,
            "   ",
        ],
        THREAD_C,
    );
    assert!(empty_reply.contains("REPLY_BODY_REQUIRED"), "{empty_reply}");
    assert_eq!(pending(&fixture, THREAD_C).len(), 1);

    fixture.run_public(&["context"], THREAD_B);
    let former_recipient = rejected(
        &fixture,
        &[
            "sendmessage",
            "--to",
            "Alice",
            "--subject",
            "late answer",
            "--in-reply-to",
            &msg_id,
            "The former holder answers after responsibility moved",
        ],
        THREAD_B,
    );
    assert!(
        former_recipient.contains("REPLY_OBLIGATION_MISMATCH"),
        "{former_recipient}"
    );
    assert_eq!(
        pending(&fixture, THREAD_C).len(),
        1,
        "an answer from the former holder must not settle the obligation"
    );

    let obligation_before_inspection =
        fixture.current_state()["reply_obligations"][&msg_id].clone();

    let inspected = fixture.run_public(&["msg", &msg_id], THREAD_C);
    assert_eq!(inspected["id"], msg_id);
    assert_eq!(inspected["body"], "Answer after the name moves");
    assert_eq!(
        inspected["from"], alice["identity"]["worker_id"],
        "historical sender remains unchanged"
    );
    assert_eq!(
        inspected["to"], bob["identity"]["worker_id"],
        "historical recipient remains unchanged"
    );
    assert_eq!(inspected["consumed_by_recv"], true);
    assert!(
        inspected["reply_obligation"]
            .as_object()
            .is_some_and(|obligation| obligation["message_id"] == msg_id),
        "{inspected}"
    );
    let state_after_inspection = fixture.current_state();
    assert_eq!(state_after_inspection["msgs"][&msg_id], original_record);
    assert_eq!(
        state_after_inspection["reply_obligations"][&msg_id], obligation_before_inspection,
        "inspection must not change obligation policy or reminder state"
    );

    let fresh = fixture.run_public(
        &[
            "recv",
            "--receive-id",
            "successor-after-read",
            "--timeout",
            "0",
        ],
        THREAD_C,
    );
    assert!(
        fresh["messages"]
            .as_array()
            .into_iter()
            .flatten()
            .all(|message| message["id"] != msg_id),
        "a consumed original must not reappear: {fresh}"
    );
    assert_eq!(
        original_receive_receipt(&fixture, "successor-original-recv"),
        original_receipt,
        "inspection must not change the original receive receipt"
    );
    assert_eq!(
        fixture.run_public(&["msg", &msg_id], THREAD_C),
        inspected,
        "inspection must be read-only"
    );

    let reply = fixture.run_public(
        &[
            "sendmessage",
            "--to",
            "Alice",
            "--subject",
            "re: successor answer",
            "--in-reply-to",
            &msg_id,
            "The successor answered",
        ],
        THREAD_C,
    );
    assert!(pending(&fixture, THREAD_C).is_empty());

    let answered = fixture.run_public(
        &[
            "recv",
            "--receive-id",
            "successor-original-answer",
            "--timeout",
            "0",
        ],
        THREAD_A,
    );
    assert_eq!(answered["count"], 1, "{answered}");
    assert_eq!(answered["messages"][0]["id"], reply["msg_id"]);
    assert_eq!(answered["messages"][0]["in_reply_to"], msg_id);

    let state_after_reply = fixture.current_state();
    assert_eq!(state_after_reply["msgs"][&msg_id], original_record);
    assert!(
        state_after_reply["reply_obligations"]
            .get(&msg_id)
            .is_none(),
        "the linked reply must settle only this obligation: {state_after_reply}"
    );
    let denied_after_settlement = rejected(&fixture, &["msg", &msg_id], THREAD_C);
    assert!(
        denied_after_settlement.contains("CALLER_NOT_PERMITTED"),
        "{denied_after_settlement}"
    );
    assert_eq!(
        original_receive_receipt(&fixture, "successor-original-recv"),
        original_receipt,
        "the original receive fact must remain with its historical consumer"
    );
}

#[test]
fn reply_obligation_successor_answers_the_current_sender_after_both_names_move() {
    let fixture = AppFixture::new();
    let alice = fixture.run_public(&["context"], THREAD_A);
    let bob = fixture.run_public(&["context"], THREAD_B);

    let sent = fixture.run_public(
        &[
            "sendmessage",
            "--to",
            "Bob",
            "--subject",
            "both names move",
            "--reply-required",
            "Answer the current sender",
        ],
        THREAD_A,
    );
    let msg_id = sent["msg_id"].as_str().unwrap().to_owned();
    fixture.run_public(
        &[
            "recv",
            "--receive-id",
            "both-names-original-recv",
            "--timeout",
            "0",
        ],
        THREAD_B,
    );

    let recipient_successor = fixture.run_public(
        &[
            "context",
            "--approve-grant",
            r#"{"action":"restore_name","requested_name":"Bob","authorization":"fixture user replaces Bob","replace_confirmed":true}"#,
        ],
        THREAD_C,
    );
    assert_eq!(recipient_successor["name"], "Bob");

    let sender_successor = fixture.run_public(
        &[
            "context",
            "--approve-grant",
            r#"{"action":"restore_name","requested_name":"Alice","authorization":"fixture user replaces Alice","replace_confirmed":true}"#,
        ],
        THREAD_B,
    );
    assert_eq!(sender_successor["name"], "Alice");
    assert_ne!(
        sender_successor["identity"]["worker_id"],
        alice["identity"]["worker_id"]
    );

    let obligations = pending(&fixture, THREAD_C);
    assert_eq!(obligations.len(), 1, "{obligations:?}");
    assert_eq!(
        obligations[0]["from"], sender_successor["identity"]["worker_id"],
        "the obligation must project the current sender, not the historical sender"
    );

    let inspected = fixture.run_public(&["msg", &msg_id], THREAD_C);
    assert_eq!(inspected["id"], msg_id);
    assert_eq!(inspected["body"], "Answer the current sender");

    let reply = fixture.run_public(
        &[
            "sendmessage",
            "--to",
            "Alice",
            "--subject",
            "re: both names move",
            "--in-reply-to",
            &msg_id,
            "The successor answers the current sender",
        ],
        THREAD_C,
    );
    assert!(pending(&fixture, THREAD_C).is_empty());

    let answered = fixture.run_public(
        &[
            "recv",
            "--receive-id",
            "both-names-answer",
            "--timeout",
            "0",
        ],
        THREAD_B,
    );
    assert_eq!(answered["count"], 1, "{answered}");
    assert_eq!(answered["messages"][0]["id"], reply["msg_id"]);
    assert_eq!(
        answered["messages"][0]["to"],
        sender_successor["identity"]["worker_id"]
    );
    assert_eq!(answered["messages"][0]["in_reply_to"], msg_id);
}

#[test]
fn reply_obligation_successor_access_stops_without_open_obligation() {
    let fixture = AppFixture::new();
    fixture.run_public(&["context"], THREAD_A);
    let bob = fixture.run_public(&["context"], THREAD_B);
    let unrelated = fixture.run_public(
        &[
            "sendmessage",
            "--to",
            "Bob",
            "--subject",
            "no obligation",
            "historical consumed only",
        ],
        THREAD_A,
    );
    let unrelated_id = unrelated["msg_id"].as_str().unwrap().to_owned();
    fixture.run_public(
        &[
            "recv",
            "--receive-id",
            "successor-unrelated-recv",
            "--timeout",
            "0",
        ],
        THREAD_B,
    );
    let successor = fixture.run_public(
        &[
            "context",
            "--approve-grant",
            r#"{"action":"restore_name","requested_name":"Bob","authorization":"fixture user replaces Bob","replace_confirmed":true}"#,
        ],
        THREAD_C,
    );
    assert_eq!(successor["name"], "Bob");
    assert_ne!(
        successor["identity"]["worker_id"],
        bob["identity"]["worker_id"]
    );
    let denied = rejected(&fixture, &["msg", &unrelated_id], THREAD_C);
    assert!(denied.contains("CALLER_NOT_PERMITTED"), "{denied}");
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
