use super::*;
use crate::proto::{DshCandidate, TmuxCandidate};

#[test]
fn rex_requires_the_agent_session_without_native_thread_fields() {
    let facts = IdentityFacts {
        rex: Some(crate::proto::RexCandidate {
            server: "unix:///tmp/rex.sock".into(),
            session_id: "session:11111111-1111-1111-1111-111111111111".into(),
            block_id: "block:22222222-2222-2222-2222-222222222222".into(),
            cwd: "/remote/project".into(),
        }),
        ..Default::default()
    };
    assert_eq!(required_fields(&facts), vec!["session_id"]);
    let facts = IdentityFacts {
        session_id: Some("agent-session".into()),
        ..facts
    };
    assert!(required_fields(&facts).is_empty());
    validate_facts(&facts).unwrap();
    let mixed = IdentityFacts {
        endpoint: Some("unix:///tmp/other.sock".into()),
        ..facts
    };
    assert!(validate_facts(&mixed).is_err());
}

fn tmux_anchor() -> TmuxCandidate {
    TmuxCandidate {
        endpoint: crate::proto::TmuxEndpoint {
            socket_path: "/tmp/tmux-test.sock".into(),
            server_pid: 42,
            tmux_session_id: "$7".into(),
            pane_id: "%3".into(),
            pane_pid: 99,
            codex_session_id: Some("saved-agent-session".into()),
            codex_thread_id: Some("saved-agent-thread".into()),
        },
        cwd: "/tmp/project".into(),
    }
}

#[test]
fn dsh_anchor_alone_requires_no_appserver_fields() {
    let facts = IdentityFacts {
        dsh_session_id: Some("session-1".into()),
        ..IdentityFacts::default()
    };
    assert!(required_fields(&facts).is_empty());
}

/// A tmux anchor with no observed endpoint is complete on its own.
#[test]
fn tmux_placement_requires_agent_session_but_no_appserver_fields() {
    let facts = IdentityFacts {
        tmux: Some(tmux_anchor()),
        ..IdentityFacts::default()
    };
    assert_eq!(required_fields(&facts), vec!["session_id"]);
    let facts = IdentityFacts {
        session_id: Some("current-agent".into()),
        ..facts
    };
    assert!(required_fields(&facts).is_empty());
}

/// Native selection requires four fields; the higher-priority pane does not.
#[test]
fn an_observed_endpoint_still_requires_the_four_appserver_fields() {
    let facts = IdentityFacts {
        dsh_session_id: Some("session-1".into()),
        endpoint: Some("unix:///tmp/appserver.sock".into()),
        ..IdentityFacts::default()
    };
    assert_eq!(required_fields(&facts), vec!["thread_id", "namespace"]);
    let facts = IdentityFacts {
        session_id: Some("current-agent".into()),
        tmux: Some(tmux_anchor()),
        endpoint: Some("unix:///tmp/appserver.sock".into()),
        ..IdentityFacts::default()
    };
    assert!(required_fields(&facts).is_empty());
}

/// With no anchor and no endpoint all four facts are requested.
#[test]
fn no_anchor_requests_all_four_fields() {
    let facts = IdentityFacts::default();
    assert_eq!(
        required_fields(&facts),
        vec!["session_id", "thread_id", "endpoint", "namespace"]
    );
}
