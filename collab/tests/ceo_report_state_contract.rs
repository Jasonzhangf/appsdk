//! Independent consumer contract for the canonical CEO report state.
//!
//! The test imports the exact public source with `#[path]` and exercises only
//! the two-file foundation: coalescing, batch preparation and explicit
//! acknowledgement. It does not touch the daemon, CLI, journal or policy owner.

#[path = "../src/ceo_report_state.rs"]
mod ceo_report_state;

use ceo_report_state::{CeoAckResult, CeoProgressContent, CeoReportState, CeoReportStateError};
use std::collections::BTreeMap;

fn content(subject: &str, body: &str) -> CeoProgressContent {
    CeoProgressContent::new(subject, body)
}

fn coverage(entries: &[(&str, u64)]) -> BTreeMap<String, u64> {
    entries
        .iter()
        .map(|(project, revision)| ((*project).to_owned(), *revision))
        .collect()
}

#[test]
fn equal_business_content_is_a_noop_after_another_arrival() {
    let mut state = CeoReportState::new();
    assert!(state.record_progress("alpha", content("summary", "first progress"), false));
    let before = state.pending_revision("alpha").expect("alpha pending");

    // Same business content, different edge whitespace. Arrival time and message
    // identity are not accepted by this API, so this must not create a revision.
    assert!(!state.record_progress(
        "alpha",
        content("  summary  ", "\n first progress \t"),
        false
    ));

    let after = state
        .pending_revision("alpha")
        .expect("alpha still pending");
    assert_eq!(before, after);
    assert_eq!(state.pending_revisions().len(), 1);
}

#[test]
fn project_latest_merge_keeps_only_the_newest_business_progress() {
    let mut state = CeoReportState::new();
    assert!(state.record_progress("alpha", content("alpha", "first"), false));
    assert!(state.record_progress("beta", content("beta", "only"), false));
    assert!(state.record_progress("alpha", content("alpha", "second"), false));

    let alpha = state.pending_revision("alpha").expect("alpha pending");
    assert_eq!(alpha.revision, 2);
    assert_eq!(alpha.content, content("alpha", "second"));

    let beta = state.pending_revision("beta").expect("beta pending");
    assert_eq!(beta.revision, 1);
    assert_eq!(beta.content, content("beta", "only"));
    assert_eq!(state.pending_revisions().len(), 2);
}

#[test]
fn reading_and_preparing_do_not_mark_user_reported() {
    let mut state = CeoReportState::new();
    assert!(state.record_progress("alpha", content("alpha", "first"), false));
    let revision = state.pending_revision("alpha").expect("alpha pending");

    let batch = state
        .prepare_batch("batch-1", coverage(&[("alpha", revision.revision)]), false)
        .expect("prepare batch");
    assert_eq!(batch.batch_id, "batch-1");
    assert_eq!(state.last_actual_report_ms(), None);
    assert!(state.pending_revision("alpha").is_some());
    assert_eq!(state.batch_count(), 1);

    // Same pending batch content is reused, even when the caller supplies a new
    // ID. No duplicate in-flight batch is created.
    let duplicate = state
        .prepare_batch("batch-1-copy", coverage(&[("alpha", 1)]), false)
        .expect("reuse batch");
    assert_eq!(duplicate.batch_id, "batch-1");
    assert_eq!(state.batch_count(), 1);
    assert_eq!(state.last_actual_report_ms(), None);
    assert!(state.pending_revision("alpha").is_some());
}

#[test]
fn old_batch_ack_preserves_a_newer_same_project_revision() {
    let mut state = CeoReportState::new();
    assert!(state.record_progress("alpha", content("alpha", "first"), false));
    state
        .prepare_batch("batch-old", coverage(&[("alpha", 1)]), false)
        .expect("prepare old batch");
    assert!(state.record_progress("alpha", content("alpha", "second"), false));

    let ack = state
        .acknowledge_batch("batch-old", 1_000)
        .expect("ack old batch");
    // The acknowledged coverage still advances the watermark for revision 1,
    // while the newer revision 2 remains pending.
    assert_eq!(
        ack,
        CeoAckResult::Advanced {
            advanced_revisions: 1
        }
    );
    assert_eq!(state.last_actual_report_ms(), Some(1_000));
    assert_eq!(state.projects["alpha"].reported_revision, 1);

    let pending = state
        .pending_revision("alpha")
        .expect("newer alpha revision remains pending");
    assert_eq!(pending.revision, 2);
    assert_eq!(pending.content, content("alpha", "second"));

    state
        .prepare_batch("batch-new", coverage(&[("alpha", 2)]), false)
        .expect("prepare new batch");
    let ack = state
        .acknowledge_batch("batch-new", 2_000)
        .expect("ack new batch");
    assert_eq!(
        ack,
        CeoAckResult::Advanced {
            advanced_revisions: 1
        }
    );
    assert!(state.pending_revision("alpha").is_none());
    // The acknowledged content is already reported, so an equal new arrival
    // must not resurrect a pending revision.
    assert!(!state.record_progress("alpha", content("alpha", "second"), false));
    assert!(state.pending_revision("alpha").is_none());
    assert_eq!(state.last_actual_report_ms(), Some(2_000));
}

#[test]
fn ack_preserves_another_projects_pending_update() {
    let mut state = CeoReportState::new();
    assert!(state.record_progress("alpha", content("alpha", "first"), false));
    assert!(state.record_progress("beta", content("beta", "first"), false));
    state
        .prepare_batch("batch-both", coverage(&[("alpha", 1), ("beta", 1)]), false)
        .expect("prepare batch");

    assert!(state.record_progress("beta", content("beta", "second"), false));
    let ack = state
        .acknowledge_batch("batch-both", 3_000)
        .expect("ack batch");
    // Both covered revisions advance their watermarks: alpha to 1, beta to 1.
    assert_eq!(
        ack,
        CeoAckResult::Advanced {
            advanced_revisions: 2
        }
    );
    assert!(state.pending_revision("alpha").is_none());
    assert_eq!(state.projects["alpha"].reported_revision, 1);
    assert_eq!(state.projects["beta"].reported_revision, 1);

    let beta = state
        .pending_revision("beta")
        .expect("newer beta revision remains pending");
    assert_eq!(beta.revision, 2);
    assert_eq!(beta.content, content("beta", "second"));
}

#[test]
fn serde_roundtrip_preserves_pending_inflight_and_acknowledged_state() {
    let mut state = CeoReportState::new();
    assert!(state.record_progress("alpha", content("alpha", "first"), false));
    assert!(state.record_progress("beta", content("beta", "first"), false));
    state
        .prepare_batch("batch-alpha", coverage(&[("alpha", 1)]), false)
        .expect("prepare in-flight batch");

    let encoded = serde_json::to_string(&state).expect("serialize pending state");
    let mut restored: CeoReportState =
        serde_json::from_str(&encoded).expect("deserialize pending state");
    assert_eq!(restored, state);
    assert_eq!(restored.pending_revisions().len(), 2);
    assert_eq!(restored.batch_count(), 1);
    assert_eq!(restored.last_actual_report_ms(), None);

    let ack = restored
        .acknowledge_batch("batch-alpha", 4_000)
        .expect("ack after roundtrip");
    assert_eq!(
        ack,
        CeoAckResult::Advanced {
            advanced_revisions: 1
        }
    );
    assert!(restored.pending_revision("alpha").is_none());
    assert!(restored.pending_revision("beta").is_some());
    assert_eq!(restored.last_actual_report_ms(), Some(4_000));

    let encoded = serde_json::to_string(&restored).expect("serialize acknowledged state");
    let after: CeoReportState =
        serde_json::from_str(&encoded).expect("deserialize acknowledged state");
    assert_eq!(after, restored);
    assert_eq!(after.batch_count(), 0);
    assert_eq!(after.last_actual_report_ms(), Some(4_000));
    assert!(after.pending_revision("beta").is_some());
}

#[test]
fn repeated_ack_rejects_without_moving_time_or_clearing_new_progress() {
    let mut state = CeoReportState::new();
    assert!(state.record_progress("alpha", content("alpha", "first"), false));
    state
        .prepare_batch("batch-1", coverage(&[("alpha", 1)]), false)
        .expect("prepare batch");
    let first = state
        .acknowledge_batch("batch-1", 5_000)
        .expect("first ack");
    assert_eq!(
        first,
        CeoAckResult::Advanced {
            advanced_revisions: 1
        }
    );
    assert_eq!(state.last_actual_report_ms(), Some(5_000));

    // Equal content after acknowledgement stays coalesced with the reported
    // revision; only a meaningful new arrival becomes pending again.
    assert!(!state.record_progress("alpha", content("alpha", "first"), false));
    assert!(state.pending_revision("alpha").is_none());
    assert!(state.record_progress("alpha", content("alpha", "second"), false));
    assert!(state.record_progress("beta", content("beta", "new"), false));

    let repeated = state.acknowledge_batch("batch-1", 9_000);
    assert_eq!(
        repeated,
        Err(CeoReportStateError::UnknownBatch {
            batch_id: "batch-1".to_owned()
        })
    );
    assert_eq!(state.last_actual_report_ms(), Some(5_000));
    assert_eq!(
        state
            .pending_revision("alpha")
            .expect("new alpha revision remains")
            .revision,
        2
    );
    assert!(state.pending_revision("beta").is_some());
}

#[test]
fn typed_urgent_progress_stays_visible_while_ordinary_batch_is_inflight() {
    let mut state = CeoReportState::new();
    assert!(state.record_progress("alpha", content("alpha", "ordinary"), false));
    state
        .prepare_batch("ordinary-batch", coverage(&[("alpha", 1)]), false)
        .expect("prepare ordinary batch");

    assert!(state.record_progress("beta", content("beta", "blocked"), true));
    assert!(state.has_pending_urgent());
    let urgent = state.pending_urgent_revisions();
    assert_eq!(urgent.len(), 1);
    assert_eq!(urgent[0].project, "beta");
    assert_eq!(urgent[0].revision, 1);

    let ack = state
        .acknowledge_batch("ordinary-batch", 6_000)
        .expect("ack ordinary batch");
    assert_eq!(
        ack,
        CeoAckResult::Advanced {
            advanced_revisions: 1
        }
    );
    assert!(state.pending_revision("alpha").is_none());

    let still_urgent = state
        .pending_revision("beta")
        .expect("typed urgent revision remains pending");
    assert!(still_urgent.urgent);
    assert_eq!(still_urgent.revision, 1);
    assert_eq!(state.last_actual_report_ms(), Some(6_000));

    state
        .prepare_batch("urgent-batch", coverage(&[("beta", 1)]), true)
        .expect("prepare urgent batch");
    let ack = state
        .acknowledge_batch("urgent-batch", 7_000)
        .expect("ack urgent batch");
    assert_eq!(
        ack,
        CeoAckResult::Advanced {
            advanced_revisions: 1
        }
    );
    assert!(!state.has_pending_urgent());
    assert!(state.pending_urgent_revisions().is_empty());
    assert!(state.pending_revision("beta").is_none());
}

#[test]
fn body_text_containing_urgent_does_not_create_typed_urgency() {
    let mut state = CeoReportState::new();
    assert!(state.record_progress(
        "alpha",
        content("urgent P1", "urgent: everything is urgent"),
        false
    ));
    assert!(!state.has_pending_urgent());
    assert!(state.pending_urgent_revisions().is_empty());
    assert!(
        !state
            .pending_revision("alpha")
            .expect("alpha pending")
            .urgent
    );

    assert!(state.record_progress("beta", content("plain", "urgent?"), true));
    assert!(state.has_pending_urgent());
    let urgent = state.pending_urgent_revisions();
    assert_eq!(urgent.len(), 1);
    assert_eq!(urgent[0].project, "beta");
}

#[test]
fn explicit_urgent_upgrade_of_equal_content_is_a_new_revision() {
    let mut state = CeoReportState::new();
    assert!(state.record_progress("alpha", content("alpha", "same"), false));
    state
        .prepare_batch("batch-1", coverage(&[("alpha", 1)]), false)
        .expect("prepare batch");

    assert!(state.record_progress("alpha", content("alpha", "same"), true));
    let pending = state
        .pending_revision("alpha")
        .expect("urgent upgrade remains pending");
    assert_eq!(pending.revision, 2);
    assert!(pending.urgent);

    let ack = state
        .acknowledge_batch("batch-1", 8_000)
        .expect("ack old batch");
    // Revision 1 becomes the acknowledged watermark; the urgent revision 2
    // still stays pending and keeps its canonical urgent marker.
    assert_eq!(
        ack,
        CeoAckResult::Advanced {
            advanced_revisions: 1
        }
    );
    assert_eq!(state.projects["alpha"].reported_revision, 1);
    let pending = state
        .pending_revision("alpha")
        .expect("urgent revision must survive the old ack");
    assert_eq!(pending.revision, 2);
    assert!(pending.urgent);
}

#[test]
fn batch_rejections_are_explicit() {
    let mut state = CeoReportState::new();
    assert_eq!(
        state.prepare_batch("empty", BTreeMap::new(), false),
        Err(CeoReportStateError::EmptyCoverage {
            batch_id: "empty".to_owned()
        })
    );

    assert!(state.record_progress("alpha", content("alpha", "first"), false));
    state
        .prepare_batch("batch-1", coverage(&[("alpha", 1)]), false)
        .expect("prepare batch");
    assert_eq!(
        state.prepare_batch("batch-1", coverage(&[("alpha", 2)]), false),
        Err(CeoReportStateError::BatchConflict {
            batch_id: "batch-1".to_owned()
        })
    );
    assert_eq!(
        state.acknowledge_batch("missing", 1),
        Err(CeoReportStateError::UnknownBatch {
            batch_id: "missing".to_owned()
        })
    );
}

#[test]
fn partial_ack_advances_covered_revision_and_keeps_newer_same_project_pending() {
    // read B1(R1) -> update R2 -> read B2(R2) -> ack B1 must leave
    // reported_revision = 1 with revision 2 still pending and B2 still valid.
    let mut state = CeoReportState::new();
    assert!(state.record_progress("alpha", content("alpha", "v1"), false));
    state
        .prepare_batch("B1", coverage(&[("alpha", 1)]), false)
        .expect("prepare B1");

    assert!(state.record_progress("alpha", content("alpha", "v2"), false));
    state
        .prepare_batch("B2", coverage(&[("alpha", 2)]), false)
        .expect("prepare B2");
    // Recording a newer revision and preparing a newer batch must not invalidate
    // the older batch that is still waiting for acknowledgement.
    assert_eq!(state.batch_count(), 2);

    let ack = state.acknowledge_batch("B1", 1_000).expect("ack B1");
    assert_eq!(
        ack,
        CeoAckResult::Advanced {
            advanced_revisions: 1
        }
    );

    let progress = &state.projects["alpha"];
    assert_eq!(progress.reported_revision, 1);
    assert_eq!(progress.revision, 2);

    let pending = state
        .pending_revision("alpha")
        .expect("revision 2 stays pending");
    assert_eq!(pending.revision, 2);
    assert_eq!(pending.content, content("alpha", "v2"));

    // B1 is consumed, B2 survives because revision 2 is still unreported.
    assert_eq!(state.batch_count(), 1);
    assert!(state.batches.contains_key("B2"));

    let ack = state.acknowledge_batch("B2", 2_000).expect("ack B2");
    assert_eq!(
        ack,
        CeoAckResult::Advanced {
            advanced_revisions: 1
        }
    );
    assert!(state.pending_revision("alpha").is_none());
    assert_eq!(state.last_actual_report_ms(), Some(2_000));
}

#[test]
fn reverse_ack_order_cannot_regress_reported_revision() {
    let mut state = CeoReportState::new();
    assert!(state.record_progress("alpha", content("alpha", "v1"), false));
    assert!(state.record_progress("beta", content("beta", "v1"), false));
    state
        .prepare_batch("old-both", coverage(&[("alpha", 1), ("beta", 1)]), false)
        .expect("prepare old batch");
    assert!(state.record_progress("alpha", content("alpha", "v2"), false));
    state
        .prepare_batch("new-alpha", coverage(&[("alpha", 2)]), false)
        .expect("prepare new batch");

    // Acknowledge the newer alpha coverage first.
    let ack = state
        .acknowledge_batch("new-alpha", 1_000)
        .expect("ack new alpha");
    assert_eq!(
        ack,
        CeoAckResult::Advanced {
            advanced_revisions: 1
        }
    );
    assert_eq!(state.projects["alpha"].reported_revision, 2);

    // The older batch stays valid because beta revision 1 is still unreported.
    assert_eq!(state.batch_count(), 1);
    assert!(state.batches.contains_key("old-both"));

    let ack = state
        .acknowledge_batch("old-both", 2_000)
        .expect("ack old both");
    // Only beta advances; alpha must not move backwards from 2 to 1.
    assert_eq!(
        ack,
        CeoAckResult::Advanced {
            advanced_revisions: 1
        }
    );
    assert_eq!(state.projects["alpha"].reported_revision, 2);
    assert_eq!(state.projects["beta"].reported_revision, 1);
    assert!(state.pending_revision("alpha").is_none());
    assert!(state.pending_revision("beta").is_none());
}

#[test]
fn only_ack_prunes_batches_fully_covered_by_reported_watermarks() {
    let mut state = CeoReportState::new();
    assert!(state.record_progress("alpha", content("alpha", "v1"), false));
    state
        .prepare_batch("B1", coverage(&[("alpha", 1)]), false)
        .expect("prepare B1");
    // A newer revision and a newer read must not remove the older in-flight batch.
    assert!(state.record_progress("alpha", content("alpha", "v2"), false));
    state
        .prepare_batch("B2", coverage(&[("alpha", 2)]), false)
        .expect("prepare B2");
    assert_eq!(state.batch_count(), 2);

    // Acknowledging B2 raises the watermark to 2 and prunes the now fully
    // covered B1 while consuming B2.
    let ack = state.acknowledge_batch("B2", 500).expect("ack B2");
    assert_eq!(
        ack,
        CeoAckResult::Advanced {
            advanced_revisions: 1
        }
    );
    assert_eq!(state.batch_count(), 0);
    assert!(state.pending_revision("alpha").is_none());
    assert_eq!(state.projects["alpha"].reported_revision, 2);
}

#[test]
fn urgent_generation_advances_only_on_meaningful_typed_urgent_update() {
    let mut state = CeoReportState::new();
    assert_eq!(state.urgent_generation, 0);

    // Ordinary progress never advances the urgent generation.
    assert!(state.record_progress("alpha", content("alpha", "ordinary"), false));
    assert_eq!(state.urgent_generation, 0);

    // A new typed urgent revision advances it once.
    assert!(state.record_progress("beta", content("beta", "blocked"), true));
    assert_eq!(state.urgent_generation, 1);

    // A duplicate typed urgent with equal content creates no revision and no
    // new generation.
    assert!(!state.record_progress("beta", content("  beta ", " blocked\n"), true));
    assert_eq!(state.urgent_generation, 1);
    assert_eq!(state.projects["beta"].revision, 1);

    // An urgent upgrade of equal ordinary content is meaningful and advances.
    assert!(state.record_progress("alpha", content("alpha", "ordinary"), true));
    assert_eq!(state.urgent_generation, 2);
    assert_eq!(state.projects["alpha"].revision, 2);

    // Acknowledgement and reads never advance the urgent generation.
    state
        .prepare_batch("urgent-batch", coverage(&[("beta", 1)]), true)
        .expect("prepare urgent batch");
    state.pending_urgent_revisions();
    state
        .acknowledge_batch("urgent-batch", 1_000)
        .expect("ack urgent batch");
    assert_eq!(state.urgent_generation, 2);
}

#[test]
fn ack_never_clears_latest_canonical_urgent_marker() {
    let mut state = CeoReportState::new();
    assert!(state.record_progress("alpha", content("alpha", "blocked"), true));
    state
        .prepare_batch("urgent-batch", coverage(&[("alpha", 1)]), true)
        .expect("prepare urgent batch");

    let ack = state
        .acknowledge_batch("urgent-batch", 1_000)
        .expect("ack urgent batch");
    assert_eq!(
        ack,
        CeoAckResult::Advanced {
            advanced_revisions: 1
        }
    );
    // The revision is reported, but the canonical urgent marker on the latest
    // progress is retained for observers of the durable urgent signal.
    assert!(state.projects["alpha"].urgent);
    assert_eq!(state.projects["alpha"].reported_revision, 1);
    assert!(state.pending_revision("alpha").is_none());
    assert_eq!(state.urgent_generation, 1);

    // An exact duplicate typed urgent after ack creates no revision/generation.
    assert!(!state.record_progress("alpha", content("alpha", "blocked"), true));
    assert_eq!(state.projects["alpha"].revision, 1);
    assert_eq!(state.urgent_generation, 1);
}

#[test]
fn urgent_generation_is_serde_default_for_older_state() {
    let legacy = r#"{"projects":{},"batches":{}}"#;
    let state: CeoReportState = serde_json::from_str(legacy).expect("legacy state decodes");
    assert_eq!(state.urgent_generation, 0);
    assert_eq!(state, CeoReportState::new());

    let mut advanced = CeoReportState::new();
    assert!(advanced.record_progress("beta", content("beta", "blocked"), true));
    assert_eq!(advanced.urgent_generation, 1);
    let encoded = serde_json::to_string(&advanced).expect("serialize generation");
    let restored: CeoReportState = serde_json::from_str(&encoded).expect("restore generation");
    assert_eq!(restored.urgent_generation, 1);
    assert_eq!(restored, advanced);
}
