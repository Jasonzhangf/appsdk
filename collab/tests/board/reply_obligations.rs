use super::*;

#[test]
fn reply_policy_is_retired_and_plain_send_stays_correlation_only() {
    let fixture = AppFixture::new();
    fixture.run_public(&["context"], THREAD_A);
    fixture.run_public(&["context"], THREAD_B);

    let sent = fixture.run_public(
        &[
            "sendmessage",
            "--to",
            "Bob",
            "--subject",
            "plain correlation",
            "ordinary body",
        ],
        THREAD_A,
    );
    let msg_id = sent["msg_id"].as_str().unwrap();
    let state = fixture.current_state();
    assert!(state.get("reply_obligations").is_none());
    assert!(state["msgs"][msg_id]["in_reply_to"].is_null());

    let rejected = fixture.command_public(
        &[
            "sendmessage",
            "--to",
            "Bob",
            "--subject",
            "policy rejected",
            "--reply-required",
            "body",
        ],
        THREAD_A,
    );
    assert!(!rejected.status.success());
    let output = format!(
        "{}{}",
        String::from_utf8_lossy(&rejected.stdout),
        String::from_utf8_lossy(&rejected.stderr)
    );
    assert!(output.contains("unexpected argument"), "{output}");
}
