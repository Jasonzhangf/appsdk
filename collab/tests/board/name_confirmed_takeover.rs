use super::*;

#[test]
fn user_confirmed_unknown_jenny_takeover_commits_name_and_master() {
    let mut fixture = AppFixture::new();
    let old = fixture.run_public(
        &[
            "context",
            "--approve-grant",
            r#"{"action":"restore_master","authorization":"fixture user restores Jenny"}"#,
        ],
        THREAD_A,
    );
    fixture.run_public(&["context"], THREAD_B);
    fixture.append_name_events(&[json!({"ev":"AgentPresenceObserved", "presence":{
        "project_scope":std::fs::canonicalize(&fixture.project).unwrap(),
        "worker_id":old["identity"]["worker_id"], "session_id":THREAD_A,
        "presence":"unknown", "reason":"fixture unresponsive Jenny", "observed_ms":1
    }})]);
    let restored = fixture.run_public(&["context", "--approve-grant",
        r#"{"action":"restore_master","authorization":"fixture user confirms replacing unknown Jenny","replace_confirmed":true}"#], THREAD_B);
    assert_eq!(restored["name"], "Jenny");
    assert_eq!(restored["name_resolution"]["outcome"], "committed");
    assert_eq!(
        restored["master"]["worker_id"],
        restored["identity"]["worker_id"]
    );
}
