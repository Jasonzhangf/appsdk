use super::*;

fn rejected_output(fixture: &AppFixture, args: &[&str], thread: &str) -> String {
    let output = fixture.command_public(args, thread);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !output.status.success(),
        "unexpected success for {args:?}: stdout={stdout} stderr={stderr}"
    );
    format!("{stdout} {stderr}")
}

#[test]
fn task_lifecycle_closes_without_git_or_resource_operations() {
    let fixture = AppFixture::new();
    let alice = fixture.run_public(&["context"], THREAD_A);
    assert_eq!(alice["identity"]["kind"], "peer");

    let registered = fixture.run_public(&["task", "register", "no-git-task"], THREAD_A);
    assert_eq!(registered["status"], "working");

    let delivered = fixture.run_public(
        &[
            "task",
            "deliver",
            "no-git-task",
            "--evidence",
            "public CLI evidence; no worktree or commit",
        ],
        THREAD_A,
    );
    assert_eq!(delivered["status"], "delivered", "{delivered}");

    let accepted = fixture.run_public(
        &[
            "task",
            "review",
            "no-git-task",
            "--accept",
            "--evidence",
            "reviewed without Git",
        ],
        THREAD_A,
    );
    assert_eq!(accepted["status"], "accepted", "{accepted}");

    let closed = fixture.run_public(&["task", "close", "no-git-task"], THREAD_A);
    assert_eq!(closed["status"], "closed", "{closed}");

    let status = fixture.run_public(&["task", "status", "no-git-task"], THREAD_A);
    assert_eq!(status["status"], "closed", "{status}");
}

#[test]
fn retired_git_task_entries_fail_closed_without_side_effects() {
    let fixture = AppFixture::new();
    fixture.run_public(&["context"], THREAD_A);
    fixture.run_public(&["task", "register", "retired-git"], THREAD_A);
    let before = fixture.run_public(&["task", "status", "retired-git"], THREAD_A);

    let integrated = rejected_output(
        &fixture,
        &[
            "task",
            "integrated",
            "retired-git",
            "--commit",
            "deadbeef",
            "--evidence",
            "legacy integration evidence",
        ],
        THREAD_A,
    );
    assert!(
        integrated.contains("COLLAB_GIT_OWNERSHIP_DISABLED"),
        "{integrated}"
    );

    let finalized = rejected_output(
        &fixture,
        &["task", "finalize-cleanup", "retired-git"],
        THREAD_A,
    );
    assert!(
        finalized.contains("COLLAB_GIT_OWNERSHIP_DISABLED"),
        "{finalized}"
    );

    let after = fixture.run_public(&["task", "status", "retired-git"], THREAD_A);
    assert_eq!(before, after);
    assert_eq!(after["status"], "working", "{after}");
}
