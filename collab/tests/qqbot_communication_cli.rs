//! Public CLI-contract tests for the QQBot communication lifecycle.
//!
//! These tests exercise the real `collab` binary through its public command
//! line. They deliberately fail before any daemon connection, so they run in
//! sandboxes that forbid Unix-domain socket binds. Daemon-backed end-to-end
//! routing (default per conversation, reply-route replay, platform idempotency
//! and attachment round trips) must run in a socket-capable environment.
use std::path::PathBuf;
use std::process::{Command, Output};

fn binary() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_collab"))
}

fn isolated_state_root() -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "qqbot-cli-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).unwrap();
    root
}

fn run(args: &[&str]) -> Output {
    Command::new(binary())
        .args(args)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .env("COLLAB_STATE_DIR", isolated_state_root())
        .env("CODEX_HOME", isolated_state_root())
        .env_remove("CODEX_SESSION_ID")
        .env_remove("CODEX_THREAD_ID")
        .env_remove("TMUX")
        .env_remove("TMUX_PANE")
        .env_remove("COLLAB_WORKER")
        .output()
        .expect("run collab CLI")
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn qqbot_send_content_and_positional_body_are_mutually_exclusive() {
    let output = run(&[
        "sendmessage",
        "--to",
        "Bob",
        "--subject",
        "conflict",
        "--content",
        "c",
        "extra body",
    ]);
    assert!(!output.status.success(), "mixed syntax must fail");
    assert!(
        stderr(&output).contains("SEND_SYNTAX_CONFLICT"),
        "must name the conflict: {}",
        stderr(&output)
    );
}

#[test]
fn qqbot_legacy_send_still_requires_subject() {
    let output = run(&["sendmessage", "--to", "Bob", "body without subject"]);
    assert!(!output.status.success());
    assert!(
        stderr(&output).contains("MESSAGE_SUBJECT_REQUIRED"),
        "legacy send must still require --subject: {}",
        stderr(&output)
    );
}

#[test]
fn qqbot_new_send_syntax_is_accepted_by_the_parser() {
    let output = run(&["sendmessage", "Bob", "--content", "hello"]);
    let err = stderr(&output);
    // Parsing must succeed; the command may then fail before delivery for
    // identity/daemon reasons in this socket-free sandbox.
    assert!(
        !err.contains("unexpected argument")
            && !err.contains("unrecognized subcommand")
            && !err.contains("the following required arguments were not provided")
            && !err.contains("SEND_TARGET_REQUIRED")
            && !err.contains("SEND_SYNTAX_CONFLICT"),
        "new syntax must parse: {err}"
    );
}

#[test]
fn qqbot_bare_default_without_conversation_context_is_rejected() {
    let output = run(&["default", "project.Bob"]);
    assert!(!output.status.success());
    assert!(
        stderr(&output).contains("CONVERSATION_CONTEXT_MISSING"),
        "a plain terminal must not mutate a global default: {}",
        stderr(&output)
    );
}

#[test]
fn qqbot_list_is_a_recognized_read_only_command() {
    let output = run(&["list"]);
    let err = stderr(&output);
    assert!(
        !err.contains("unrecognized subcommand") && !err.contains("unexpected argument"),
        "list must be a recognized command: {err}"
    );
}
