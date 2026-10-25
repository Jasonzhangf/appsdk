//! Automatic CEO report-notice patrol.
//!
//! This module is mounted by `mod ceo_report_patrol;` in `server/mod.rs`; the
//! manager timer cycle in `mod_parts/part_12.rs` calls one
//! `tick_ceo_report_patrol`, and the grant-set reset keeps the gate in the
//! canonical report state.
//!
//! One evaluation has four separate phases:
//! 1. Snapshot the host grant, notice gate, generation and pending facts under
//!    one short host reducer lock. Suppressed or not-due ticks stop here, before
//!    any route lookup or native activity request.
//! 2. Resolve the current CEO target and read fresh positive idle outside every
//!    reducer lock. Busy, waiting, unknown, offline and probe errors skip the
//!    round and advance the 15-minute window without catch-up.
//! 3. Revalidate the snapshot and reserve one preallocated message ID durably on
//!    the host before the target mailbox sees it. Concurrent ticks therefore
//!    produce at most one occurrence.
//! 4. Commit the target mailbox idempotently, revalidate the current name
//!    binding, clear only the matching host reservation, and attempt the wake
//!    once outside all locks. An already queued reservation is cleared without a
//!    second wake attempt; an uncertain original notice is never replayed.
//!
//! Automatic notices never carry or pre-bind report content. The CEO prepares
//! the current body and revision coverage atomically in the progress handler.

use crate::ceo::CeoGrant;
use crate::ceo_report_state::CeoNoticeGate;
use crate::ceo_reporting_policy::CeoReportPolicy;
use crate::server::name_mailbox::MailboxNameAddress;
use crate::server::name_state::AgentActivity;
use crate::server::state::{Event, Message, WorkerRec};
use std::sync::Arc;

/// Automatic notice cadence used for both the periodic fallback and the skip
/// window after a busy or unknown target. A skip never creates a backlog.
const CEO_REPORT_PATROL_INTERVAL_MS: u64 = 900_000;

/// Stable subject for the daemon-generated notice. Report content is prepared
/// later by `ceo progress`, never bound to this message.
const CEO_REPORT_NOTICE_SUBJECT: &str = "ceo-report-due";

const CEO_REPORT_NOTICE_BODY: &str = "CEO_REPORT_DUE: pending project progress is ready to report. Run `collab ceo progress` to select the current batch and revision coverage, report the actual results, then run `collab ceo report-ack --batch-id <batch-id>` after the report is visible. This notice carries no prebound content.";

/// Host facts captured in one short reducer lock before any target lookup.
struct CeoNoticeSnapshot {
    grant: CeoGrant,
    gate: CeoNoticeGate,
    urgent_generation: u64,
    has_pending: bool,
    has_pending_urgent: bool,
    last_actual_report_ms: Option<u64>,
}

impl super::ProjectRuntimeManager {
    /// Evaluate the automatic CEO report notice once for the manager timer cycle.
    ///
    /// Ordinary eligibility requires pending revisions, policy allowed and an
    /// elapsed 15-minute window. A new meaningful typed urgent generation may
    /// assess once immediately; repeated duplicate urgent signals do not.
    pub(super) fn tick_ceo_report_patrol(&self, now_ms: i64) -> Result<(), String> {
        let now = now_ms.max(0) as u64;
        let policy = self
            .host
            .config
            .ceo_report_policy()
            .map_err(|error| format!("CEO_REPORT_PATROL_POLICY_INVALID: {error}"))?;

        let Some(snapshot) = self.ceo_notice_snapshot()? else {
            return Ok(());
        };

        // An interrupted reservation is completed or cleared before any new
        // occurrence. A queued reservation needs no native input at all.
        if let Some(message_id) = snapshot.gate.pending_message_id.clone() {
            return self.recover_ceo_notice_reservation(&snapshot, &message_id, now, policy);
        }

        if !snapshot.has_pending {
            return Ok(());
        }

        // Pending urgent progress may bypass ordinary quiet hours and cooldown,
        // but still obeys the 15-minute window unless its generation is new.
        let urgent_due = snapshot.urgent_generation > snapshot.gate.urgent_generation_seen
            && snapshot.has_pending_urgent;
        let decision = policy.evaluate(
            now,
            snapshot.last_actual_report_ms,
            snapshot.has_pending_urgent,
        );
        if !decision.allowed {
            return Ok(());
        }
        if now < snapshot.gate.next_due_ms && !urgent_due {
            return Ok(());
        }

        // Phase 2: current target and fresh activity, never under the host lock.
        let (target, holder_id) = match self.ceo_holder(&snapshot.grant) {
            Ok(holder) => holder,
            Err(error) => {
                self.commit_ceo_notice_skip(&snapshot, now)?;
                return Err(format!(
                    "CEO_REPORT_PATROL_TARGET_UNAVAILABLE: {}",
                    error
                        .error
                        .unwrap_or_else(|| "CEO_ROUTE_UNAVAILABLE".into())
                ));
            }
        };
        let Some(worker) = clone_ceo_worker(&target, &holder_id) else {
            return self.commit_ceo_notice_skip(&snapshot, now);
        };
        match ceo_worker_is_freshly_idle(&target, &worker) {
            Ok(true) => {}
            Ok(false) => return self.commit_ceo_notice_skip(&snapshot, now),
            Err(error) => {
                self.commit_ceo_notice_skip(&snapshot, now)?;
                return Err(error);
            }
        }

        // Phase 3: reserve one durable occurrence on the host.
        let message_id = super::gen_msg_id();
        if !self.reserve_ceo_notice(&snapshot, now, &message_id, &holder_id)? {
            return Ok(());
        }

        // Phase 4: commit the target mailbox, clear the matching reservation and
        // attempt the newly queued wake once outside all locks.
        self.commit_ceo_notice(&snapshot, &target, &holder_id, &worker, &message_id, now)
    }

    /// Read the host grant, gate and pending facts under one reducer lock.
    fn ceo_notice_snapshot(&self) -> Result<Option<CeoNoticeSnapshot>, String> {
        let state = self.host.state.lock().unwrap();
        if let Some(error) = &state.journal_poison {
            return Err(format!("CEO_REPORT_PATROL_STORE_UNAVAILABLE: {error}"));
        }
        let Some(grant) = state.ceo_grant.clone() else {
            return Ok(None);
        };
        let report = &state.ceo_report_state;
        Ok(Some(CeoNoticeSnapshot {
            grant,
            gate: report.notice_gate.clone(),
            urgent_generation: report.urgent_generation,
            has_pending: !report.pending_revisions().is_empty(),
            has_pending_urgent: report.has_pending_urgent(),
            last_actual_report_ms: report.last_actual_report_ms,
        }))
    }

    /// Reserve one preallocated message ID on the host after revalidating the
    /// exact snapshot. Returns `false` when another caller already changed the
    /// grant, gate or urgent generation.
    fn reserve_ceo_notice(
        &self,
        snapshot: &CeoNoticeSnapshot,
        now: u64,
        message_id: &str,
        worker_id: &str,
    ) -> Result<bool, String> {
        let mut state = self.host.state.lock().unwrap();
        if let Some(error) = &state.journal_poison {
            return Err(format!("CEO_REPORT_PATROL_STORE_UNAVAILABLE: {error}"));
        }
        if state.ceo_grant.as_ref() != Some(&snapshot.grant)
            || state.ceo_report_state.notice_gate != snapshot.gate
            || state.ceo_report_state.urgent_generation != snapshot.urgent_generation
            || state.ceo_report_state.pending_revisions().is_empty()
        {
            return Ok(false);
        }
        let gate = CeoNoticeGate {
            next_due_ms: now.saturating_add(CEO_REPORT_PATROL_INTERVAL_MS),
            urgent_generation_seen: snapshot.urgent_generation,
            pending_message_id: Some(message_id.to_owned()),
            pending_worker_id: Some(worker_id.to_owned()),
        };
        self.host
            .commit_locked_checked(&mut state, &[Event::CeoReportNoticeGateUpdated { gate }])
            .map_err(|error| format!("CEO_REPORT_PATROL_RESERVE_DURABILITY_FAILED: {error}"))?;
        Ok(true)
    }

    /// Clear an unqueued reservation or a non-idle evaluation and advance the
    /// 15-minute window. The evaluated urgent generation is consumed so a busy
    /// or unknown target is never caught up later for the same signal.
    fn commit_ceo_notice_skip(&self, snapshot: &CeoNoticeSnapshot, now: u64) -> Result<(), String> {
        let mut state = self.host.state.lock().unwrap();
        if let Some(error) = &state.journal_poison {
            return Err(format!("CEO_REPORT_PATROL_STORE_UNAVAILABLE: {error}"));
        }
        if state.ceo_grant.as_ref() != Some(&snapshot.grant)
            || state.ceo_report_state.notice_gate != snapshot.gate
            || state.ceo_report_state.urgent_generation != snapshot.urgent_generation
        {
            return Ok(());
        }
        let gate = CeoNoticeGate {
            next_due_ms: now.saturating_add(CEO_REPORT_PATROL_INTERVAL_MS),
            urgent_generation_seen: snapshot.urgent_generation,
            pending_message_id: None,
            pending_worker_id: None,
        };
        self.host
            .commit_locked_checked(&mut state, &[Event::CeoReportNoticeGateUpdated { gate }])
            .map(|_| ())
            .map_err(|error| format!("CEO_REPORT_PATROL_SKIP_DURABILITY_FAILED: {error}"))
    }

    /// Clear the matching reserved occurrence and advance the 15-minute window
    /// after a post-reservation target check fails. The target lock must be
    /// dropped before this host-only skip runs.
    fn commit_ceo_notice_skip_reserved(
        &self,
        snapshot: &CeoNoticeSnapshot,
        message_id: &str,
        now: u64,
    ) -> Result<(), String> {
        let mut state = self.host.state.lock().unwrap();
        if let Some(error) = &state.journal_poison {
            return Err(format!("CEO_REPORT_PATROL_STORE_UNAVAILABLE: {error}"));
        }
        if state.ceo_grant.as_ref() != Some(&snapshot.grant)
            || state
                .ceo_report_state
                .notice_gate
                .pending_message_id
                .as_deref()
                != Some(message_id)
        {
            return Ok(());
        }
        let gate = CeoNoticeGate {
            next_due_ms: now.saturating_add(CEO_REPORT_PATROL_INTERVAL_MS),
            urgent_generation_seen: snapshot.urgent_generation,
            pending_message_id: None,
            pending_worker_id: None,
        };
        self.host
            .commit_locked_checked(&mut state, &[Event::CeoReportNoticeGateUpdated { gate }])
            .map(|_| ())
            .map_err(|error| format!("CEO_REPORT_PATROL_SKIP_DURABILITY_FAILED: {error}"))
    }

    /// Clear only the matching host reservation, preserving the current deadline
    /// and urgent generation. This is the recovery receipt after the target
    /// mailbox already contains the message.
    fn clear_ceo_notice_reservation(&self, message_id: &str) -> Result<(), String> {
        let mut state = self.host.state.lock().unwrap();
        if let Some(error) = &state.journal_poison {
            return Err(format!("CEO_REPORT_PATROL_STORE_UNAVAILABLE: {error}"));
        }
        if state
            .ceo_report_state
            .notice_gate
            .pending_message_id
            .as_deref()
            != Some(message_id)
        {
            return Ok(());
        }
        let mut gate = state.ceo_report_state.notice_gate.clone();
        gate.pending_message_id = None;
        gate.pending_worker_id = None;
        self.host
            .commit_locked_checked(&mut state, &[Event::CeoReportNoticeGateUpdated { gate }])
            .map(|_| ())
            .map_err(|error| format!("CEO_REPORT_PATROL_CLEAR_DURABILITY_FAILED: {error}"))
    }

    /// Complete an interrupted reservation or clear it without fabricating input.
    ///
    /// Already queued means the target mailbox owns the message: clear the host
    /// reservation only and never retry an uncertain wake. Not yet queued means
    /// the target must still be freshly and positively idle; anything else clears
    /// the reservation and advances the window without catch-up.
    fn recover_ceo_notice_reservation(
        &self,
        snapshot: &CeoNoticeSnapshot,
        message_id: &str,
        now: u64,
        policy: CeoReportPolicy,
    ) -> Result<(), String> {
        let (target, holder_id) = match self.ceo_holder(&snapshot.grant) {
            Ok(holder) => holder,
            Err(error) => {
                self.commit_ceo_notice_skip(snapshot, now)?;
                return Err(format!(
                    "CEO_REPORT_PATROL_TARGET_UNAVAILABLE: {}",
                    error
                        .error
                        .unwrap_or_else(|| "CEO_ROUTE_UNAVAILABLE".into())
                ));
            }
        };

        let already_queued = {
            let state = target.state.lock().unwrap();
            state.msgs.contains_key(message_id)
        };
        if already_queued {
            return self.clear_ceo_notice_reservation(message_id);
        }
        if !snapshot.has_pending {
            return self.clear_ceo_notice_reservation(message_id);
        }

        // A reservation created before quiet hours or cooldown must not recover
        // while the current typed policy makes the pending work unreportable.
        // Clear it and advance the window without resolving the notice, probing
        // activity or sending input.
        let decision = policy.evaluate(
            now,
            snapshot.last_actual_report_ms,
            snapshot.has_pending_urgent,
        );
        if !decision.allowed {
            return self.commit_ceo_notice_skip(snapshot, now);
        }

        // Never input to an old CEO holder or stale name carrier.
        if snapshot.gate.pending_worker_id.as_deref() != Some(holder_id.as_str()) {
            return self.commit_ceo_notice_skip(snapshot, now);
        }

        let Some(worker) = clone_ceo_worker(&target, &holder_id) else {
            return self.commit_ceo_notice_skip(snapshot, now);
        };
        match ceo_worker_is_freshly_idle(&target, &worker) {
            Ok(true) => {}
            Ok(false) => return self.commit_ceo_notice_skip(snapshot, now),
            Err(error) => {
                self.commit_ceo_notice_skip(snapshot, now)?;
                return Err(error);
            }
        }
        self.commit_ceo_notice(snapshot, &target, &holder_id, &worker, message_id, now)
    }

    /// Commit the reserved notice idempotently to the target mailbox, clear the
    /// matching host reservation and attempt a newly queued wake exactly once.
    fn commit_ceo_notice(
        &self,
        snapshot: &CeoNoticeSnapshot,
        target: &Arc<super::Server>,
        holder_id: &str,
        probed_worker: &WorkerRec,
        message_id: &str,
        now: u64,
    ) -> Result<(), String> {
        // Re-check the host reservation before touching the target journal. The
        // host lock is released before the target lock is acquired.
        let reservation_is_current = {
            let state = self.host.state.lock().unwrap();
            state.ceo_grant.as_ref() == Some(&snapshot.grant)
                && state
                    .ceo_report_state
                    .notice_gate
                    .pending_message_id
                    .as_deref()
                    == Some(message_id)
                && state
                    .ceo_report_state
                    .notice_gate
                    .pending_worker_id
                    .as_deref()
                    == Some(holder_id)
        };
        if !reservation_is_current {
            return Ok(());
        }

        let target_project = target.root.to_string_lossy().into_owned();
        let mut queued_now = false;
        let mut wake_subscription = None;
        {
            let mut state = target.state.lock().unwrap();
            if state.msgs.contains_key(message_id) {
                // A concurrent or recovered commit already queued this exact
                // occurrence. Clearing below is the whole recovery.
            } else if state.admission_frozen() {
                drop(state);
                return self.commit_ceo_notice_skip_reserved(snapshot, message_id, now);
            } else {
                let binding_current = state
                    .name_occupancy(&target_project, &snapshot.grant.name)
                    .is_some_and(|occupancy| occupancy.worker_id == holder_id)
                    && state
                        .workers
                        .get(holder_id)
                        .is_some_and(|worker| worker.transport == probed_worker.transport);
                if !binding_current {
                    drop(state);
                    return self.commit_ceo_notice_skip_reserved(snapshot, message_id, now);
                }

                let subscription = state
                    .matching_subscription(holder_id, "direct-message", None, now as i64)
                    .cloned();
                let mut events = vec![
                    Event::DeliveryMode {
                        msg_id: message_id.to_owned(),
                        mode: "explicit-notification".into(),
                        source_thread_id: None,
                    },
                    Event::Sent {
                        msg: Message {
                            id: message_id.to_owned(),
                            from: "collab-server".into(),
                            to: holder_id.to_owned(),
                            mtype: "notify".into(),
                            subject: Some(CEO_REPORT_NOTICE_SUBJECT.into()),
                            body: CEO_REPORT_NOTICE_BODY.into(),
                            in_reply_to: None,
                            created_ms: now as i64,
                            state: "pending".into(),
                            wake_attempt_count: 0,
                            last_wake_attempt_ms: 0,
                            retry_attempted: false,
                        },
                    },
                    Event::MailboxNameAddressed {
                        message_id: message_id.to_owned(),
                        address: MailboxNameAddress {
                            project_scope: target_project.clone(),
                            name: snapshot.grant.name.clone(),
                        },
                    },
                ];
                if let Some(subscription) = &subscription {
                    events.push(Event::WakeBound {
                        message_id: message_id.to_owned(),
                        subscription_id: subscription.id.clone(),
                    });
                }
                if let Err(error) = target.commit_locked_checked(&mut state, &events) {
                    // Keep the durable host reservation. The next tick recovers
                    // the same message ID idempotently without inventing success.
                    return Err(format!(
                        "CEO_REPORT_PATROL_DELIVERY_DURABILITY_FAILED: {error}"
                    ));
                }
                queued_now = true;
                wake_subscription = subscription.map(|subscription| subscription.id);
            }
        }

        self.clear_ceo_notice_reservation(message_id)?;

        // A wake is attempted at most once, only for a message queued in this
        // call. Recovered or already queued occurrences are never re-attempted.
        if queued_now {
            if let Some(subscription_id) = wake_subscription {
                let attempt = super::attempt_notification_detailed_with_at(
                    target,
                    message_id,
                    &subscription_id,
                    now as i64,
                );
                if let super::NotificationAttempt::Rejected(error) = attempt {
                    crate::server::presence::append_log(
                        &target.log_path(),
                        &format!(
                            "CEO_REPORT_PATROL_WAKE_REJECTED: message={message_id} error={error}"
                        ),
                    );
                }
            }
        }
        Ok(())
    }
}

fn clone_ceo_worker(target: &super::Server, worker_id: &str) -> Option<WorkerRec> {
    let state = target.state.lock().unwrap();
    state.workers.get(worker_id).cloned()
}

fn ceo_worker_is_freshly_idle(target: &super::Server, worker: &WorkerRec) -> Result<bool, String> {
    match crate::server::keepalive::current_board_patrol_activity(target, worker) {
        Ok(AgentActivity::Idle) => Ok(true),
        Ok(_) => Ok(false),
        Err(error) => {
            crate::server::presence::append_log(
                &target.log_path(),
                &format!(
                    "CEO_REPORT_PATROL_ACTIVITY_PROBE_FAILED: worker={} error={error}",
                    worker.id
                ),
            );
            Err(error)
        }
    }
}
