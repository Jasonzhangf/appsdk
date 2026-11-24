use super::*;

#[test]
fn manual_communication_recovery_flag_is_retired() {
    let fixture = AppFixture::new();
    fixture.run_public(&["context"], THREAD_A);
    let before = fixture.current_state();
    let output = fixture.command_public(&["context", "--recover-communication"], THREAD_A);
    assert!(
        !output.status.success(),
        "retired recovery entry must fail: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("unexpected argument '--recover-communication'"),
        "{stderr}"
    );
    assert_eq!(fixture.current_state(), before);
}

#[test]
fn ordinary_context_does_not_create_communication_recovery() {
    let fixture = AppFixture::new();
    fixture.run_public(&["context"], THREAD_A);
    let before = fixture.current_state();
    let restored = fixture.run_public(&["context"], THREAD_A);
    assert!(
        restored.get("outcome").is_none(),
        "ordinary context is not a communication operation: {restored}"
    );
    let after = fixture.current_state();
    assert_eq!(
        after["msgs"], before["msgs"],
        "identity restoration must not send a recovery message"
    );
}
