//! Consumer contract for the durable CEO automatic-notice gate.
//!
//! This test imports only the canonical helper source. It proves the serde
//! defaults and backward compatibility of the gate without touching the daemon,
//! journal, policy owner or the (not yet mounted) patrol engine.

#[path = "../src/ceo_report_state.rs"]
mod ceo_report_state;

use ceo_report_state::{CeoNoticeGate, CeoProgressContent, CeoReportState};

fn content(subject: &str, body: &str) -> CeoProgressContent {
    CeoProgressContent::new(subject, body)
}

#[test]
fn notice_gate_defaults_are_all_empty() {
    let gate = CeoNoticeGate::default();
    assert_eq!(gate.next_due_ms, 0);
    assert_eq!(gate.urgent_generation_seen, 0);
    assert_eq!(gate.pending_message_id, None);
    assert_eq!(gate.pending_worker_id, None);
}

#[test]
fn report_state_defaults_include_an_empty_notice_gate() {
    let state = CeoReportState::default();
    assert_eq!(state.notice_gate, CeoNoticeGate::default());
}

#[test]
fn old_journal_without_notice_gate_restores_the_default_gate() {
    let json = r#"{"projects":{},"batches":{},"urgent_generation":0}"#;
    let state: CeoReportState = serde_json::from_str(json).expect("legacy state decodes");
    assert_eq!(state.notice_gate, CeoNoticeGate::default());
}

#[test]
fn notice_gate_reservation_round_trips_through_json() {
    let gate = CeoNoticeGate {
        next_due_ms: 1_234,
        urgent_generation_seen: 7,
        pending_message_id: Some("m-1".into()),
        pending_worker_id: Some("ceo".into()),
    };
    let decoded: CeoNoticeGate =
        serde_json::from_str(&serde_json::to_string(&gate).expect("serialize"))
            .expect("round-trip decodes");
    assert_eq!(decoded, gate);
}

#[test]
fn gate_presence_does_not_change_progress_or_ack_semantics() {
    let mut state = CeoReportState::new();
    assert!(state.record_progress("alpha", content("summary", "first progress"), false));
    assert_eq!(state.notice_gate, CeoNoticeGate::default());

    let pending = state.pending_revisions();
    assert_eq!(pending.len(), 1);
    assert!(state.pending_urgent_revisions().is_empty());

    // The notice gate is untouched by progress recording; acknowledgement also
    // leaves it alone (grant reset owns gate reset in the later wiring).
    state
        .prepare_batch(
            "batch-1",
            pending
                .iter()
                .map(|revision| (revision.project.clone(), revision.revision))
                .collect(),
            false,
        )
        .expect("batch prepares");
    assert!(state.acknowledge_batch("batch-1", 1000).is_ok());
    assert_eq!(state.notice_gate, CeoNoticeGate::default());
}
