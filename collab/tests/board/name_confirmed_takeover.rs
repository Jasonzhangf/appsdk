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
    let project = fixture
        .project
        .canonicalize()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    fixture.update_current_state(|state| {
        state["agent_presence"][&project][old["identity"]["worker_id"].as_str().unwrap()] = json!({
            "project_scope":project,
            "worker_id":old["identity"]["worker_id"], "session_id":THREAD_A,
            "presence":"unknown", "reason":"fixture unresponsive Jenny", "observed_ms":1
        })
    });
    let restored = fixture.run_public(&["context", "--approve-grant",
        r#"{"action":"restore_master","authorization":"fixture user confirms replacing unknown Jenny","replace_confirmed":true}"#], THREAD_B);
    assert_eq!(restored["name"], "Jenny");
    assert_eq!(
        restored["master"]["worker_id"],
        restored["identity"]["worker_id"]
    );
}
