use super::{
    presence::AgentState,
    state::{Event, Message, State},
    Server,
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Record {
    pub observed: String,
    pub idle_since_ms: i64,
    pub activity_ms: i64,
    pub last_notice_ms: i64,
    pub last_notice_id: Option<String>,
    pub unacked: u8,
    pub suspected_offline: bool,
    /// Last state actually reported to the master. Durable state advances every
    /// tick; the master is told only when a state settles into something new.
    #[serde(default)]
    pub notified_state: String,
    /// Last ordinary-peer transport presence observed by the status producer.
    /// The first observation is a baseline; later online/offline edges notify
    /// the live master exactly once per transition.
    #[serde(default)]
    pub notified_presence: String,
    /// When the current not-yet-reported observation first appeared.
    #[serde(default)]
    pub pending_since_ms: i64,
    #[serde(default)]
    pub idle_episode_notices: u8,
    #[serde(default)]
    pub idle_episode_reason: String,
    /// A valid Working observation survives inconclusive probes and an
    /// unavailable master subscription until its idle transition is reported.
    #[serde(default)]
    pub working_seen: bool,
    #[serde(default)]
    pub idle_episode_stopped: bool,
    /// Scheduler-owned actionable task identities/statuses delimit episodes;
    /// ACKs, monitoring messages, and runtime flaps do not.
    #[serde(default)]
    pub idle_episode_tasks: Vec<(String, String)>,
    /// Last time this worker received a reply-pending reminder. A working
    /// observation caused by the reminder's own wake must not immediately
    /// re-arm the same obligations; this floor breaks the notify-wake-notify
    /// loop that would otherwise repeat every keepalive tick.
    #[serde(default)]
    pub last_reply_reminder_ms: i64,
}

/// A starting agent legitimately flaps between idle and working while it boots
/// and renders. Reporting each flap floods the master, so a state must hold
/// before it is worth one scheduling notification.
const SUBAGENT_STATE_SETTLE_MS: i64 = 60_000;

/// Minimum interval between reply-pending reminder bursts for one worker.
/// A short Working episode (including the wake produced by the reminder
/// itself) does not re-arm obligations until this floor elapses.
const REPLY_REMINDER_REARM_MS: i64 = 10 * 60 * 1000;

pub(crate) fn actionable(status: &str) -> bool {
    matches!(
        status,
        "assigned" | "working" | "verifying" | "reviewed" | "rework"
    )
}

/// A task that has not reached a terminal lifecycle state still owns its
/// worktree, branch, and delivery obligation, so its owner cannot be retired.
/// This is deliberately wider than `actionable`: a blocked, waiting, or
/// already-delivered task raises no keepalive nudge, but it is still unfinished
/// responsibility that must be resolved before worker close.
pub(crate) fn unfinished(status: &str) -> bool {
    !matches!(status, "pending" | "invited" | "closed" | "cancelled")
}

fn observed_label(agent: AgentState) -> &'static str {
    match agent {
        AgentState::Waiting => "idle",
        AgentState::Working => "working",
        AgentState::Unknown => "unknown",
    }
}

pub fn view(state: &State, worker: &str) -> serde_json::Value {
    let mut history: Vec<_> = state.msgs.values().filter(|m| m.to == worker && m.mtype == "keepalive")
        .map(|m| serde_json::json!({"id":m.id,"subject":m.subject,"created_ms":m.created_ms,
            "state":m.state,"acked":m.state=="read","delivery_confirmed":matches!(m.state.as_str(),"delivered"|"read"),"wake_attempts":m.wake_attempt_count})).collect();
    history.sort_by_key(|m| m["created_ms"].as_i64());
    serde_json::json!({"state":state.keepalives.get(worker),"notification_history":history,
        "has_actionable_tasks":state.tasks.values().any(|t|t.owner==worker && actionable(&t.status))})
}

pub fn tick(server: &Server) {
    tick_at(server, super::state::now_ms());
}

/// App Server transport liveness is verified at registration and at each
/// notification attempt. The keepalive coordinator owns only the durable
/// worker-idle transition; it does not infer agent state from terminal text.
pub(crate) fn tick_at(server: &Server, now: i64) {
    if !server.config.keepalive.enabled || !server.config.timers.enabled {
        return;
    }
    let workers: Vec<_> = {
        let state = server.state.lock().unwrap();
        if state.admission_frozen() {
            return;
        }
        state.workers.values().cloned().collect()
    };
    let master_id = {
        let state = server.state.lock().unwrap();
        super::current_master_holder(server, &state).ok().flatten()
    };
    for worker in workers {
        // Periodic agent activity is distinct from endpoint/identity validity.
        // Managed lifecycle records remain a compatibility source only.
        let agent = {
            let mut state = server.state.lock().unwrap();
            if state.admission_frozen() {
                return;
            }
            if state.workers.get(&worker.id) != Some(&worker) {
                continue;
            }
            let (presence, activity) = super::worker_agent_facts(server, &state, &worker);
            if let Some(activity) = activity {
                let label = match (presence.map(|fact| fact.presence), activity.activity) {
                    (Some(super::name_state::AgentPresence::Offline), _) => "unknown",
                    (_, super::name_state::AgentActivity::Busy) => "working",
                    (_, super::name_state::AgentActivity::Idle) => "idle",
                    (_, super::name_state::AgentActivity::WaitingUser) => "waiting_user",
                    _ => "unknown",
                };
                match label {
                    "working" => AgentState::Working,
                    "idle" => AgentState::Waiting,
                    _ => {
                        // An inconclusive or user-blocked peer is not idle capacity.
                        // Retain working_seen so its next real idle can close the episode.
                        let mut record = state
                            .keepalives
                            .get(&worker.id)
                            .cloned()
                            .unwrap_or_default();
                        let mut events = Vec::new();
                        if record.observed != label {
                            record.observed = label.into();
                            events.push(Event::KeepaliveUpdated {
                                worker_id: worker.id.clone(),
                                record,
                            });
                        }
                        let mut wake = state.master_wake.clone();
                        wake.idle_workers.retain(|id| id != &worker.id);
                        wake.newly_idle_workers.retain(|id| id != &worker.id);
                        if wake != state.master_wake {
                            events.push(Event::MasterWakeUpdated { accumulator: wake });
                        }
                        if !events.is_empty() {
                            server.commit_locked_reporting(&mut state, &events);
                        }
                        AgentState::Unknown
                    }
                }
            } else {
                match state
                    .subagents
                    .values()
                    .find(|child| child.peer == worker.id)
                    .map(|child| child.status.as_str())
                {
                    Some("working") => AgentState::Working,
                    Some("idle") => AgentState::Waiting,
                    _ => AgentState::Unknown,
                }
            }
        };
        if agent == AgentState::Unknown {
            continue;
        }

        let mut state = server.state.lock().unwrap();
        if state.admission_frozen() {
            return;
        }
        if state.workers.get(&worker.id) != Some(&worker) {
            continue;
        }
        let mut tasks: Vec<_> = state
            .tasks
            .values()
            .filter(|t| t.owner == worker.id && actionable(&t.status))
            .map(|t| t.id.clone())
            .collect();
        tasks.sort();

        let old = state
            .keepalives
            .get(&worker.id)
            .cloned()
            .unwrap_or_default();
        let mut record = old.clone();

        let task_revision: Vec<_> = tasks
            .iter()
            .map(|id| (id.clone(), state.tasks[id].status.clone()))
            .collect();
        let task_revision_changed = record.idle_episode_tasks != task_revision;
        if task_revision_changed {
            record.idle_episode_notices = 0;
            record.idle_episode_reason.clear();
            record.idle_episode_stopped = false;
            record.idle_episode_tasks = task_revision;
            if !tasks.is_empty() {
                record.working_seen = false;
            }
        }
        let managed = state
            .subagents
            .values()
            .find(|child| child.peer == worker.id)
            .cloned();
        let master_id = master_id.clone();
        let is_live_master = master_id.as_deref() == Some(worker.id.as_str());
        if agent == AgentState::Working && record.pending_since_ms == 0 {
            record.pending_since_ms = now;
        }
        if old.observed == "working"
            || agent == AgentState::Working
            || (old.observed.is_empty()
                && !tasks.is_empty()
                && managed
                    .as_ref()
                    .is_some_and(|child| child.status == "working"))
        {
            record.working_seen = true;
        }
        if is_live_master && agent == AgentState::Working && record.idle_episode_notices > 0 {
            record.idle_episode_stopped = true;
        }
        {
            let is_idle = agent == AgentState::Waiting;
            let idle_reason = if tasks.is_empty() {
                "no-actionable-tasks"
            } else {
                "actionable-tasks-idle"
            };
            let observed_str = observed_label(agent);
            let prior_observed = record.observed.clone();
            let observed_changed = prior_observed != observed_str;
            let idle_report_settled = if !is_idle || managed.is_none() {
                true
            } else if prior_observed != "working" {
                true
            } else {
                now.saturating_sub(record.pending_since_ms) >= SUBAGENT_STATE_SETTLE_MS
            };
            let idle_record_ready = is_idle && idle_report_settled;
            let idle_signal_due = idle_record_ready
                && record.idle_episode_notices == 0
                && !record.idle_episode_stopped
                && (record.working_seen || task_revision_changed);
            let working_signal_due = !is_idle && prior_observed == "idle" && record.working_seen;
            if observed_changed {
                record.observed = observed_str.into();
                record.idle_since_ms = now;
                if agent == AgentState::Working {
                    record.activity_ms = now;
                    if !is_live_master {
                        record.idle_episode_notices = 0;
                        record.idle_episode_reason.clear();
                        record.idle_episode_stopped = false;
                        record.working_seen = true;
                    }
                }
            }
            record.unacked = 0;
            record.last_notice_id = None;
            let mut events = Vec::new();
            // Reply obligations are durable message control state. Keepalive
            // only projects the typed agent observation: a new Working episode
            // re-arms an already reminded obligation, and a settled Waiting
            // episode sends one reminder through the existing direct-message
            // path. Unknown/WaitingUser never reach this block.
            if agent == AgentState::Working {
                for obligation in state.reply_obligation_view(&worker.id) {
                    if obligation["reminded_idle_episode"].as_bool() == Some(true) {
                        if now.saturating_sub(record.last_reply_reminder_ms)
                            < REPLY_REMINDER_REARM_MS
                        {
                            continue;
                        }
                        if let Some(message_id) = obligation["message_id"].as_str() {
                            events.push(Event::ReplyObligationReminded {
                                message_id: message_id.to_owned(),
                                reminded: false,
                            });
                        }
                    }
                }
            } else if agent == AgentState::Waiting && idle_record_ready {
                let pending: Vec<_> = state
                    .reply_obligation_view(&worker.id)
                    .into_iter()
                    .filter(|obligation| {
                        obligation["reminded_idle_episode"].as_bool() != Some(true)
                    })
                    .collect();
                if !pending.is_empty() {
                    record.last_reply_reminder_ms = now;
                    let alert_id = super::gen_msg_id();
                    events.push(Event::Sent {
                        msg: Message {
                            id: alert_id.clone(),
                            from: "collab-server".into(),
                            to: worker.id.clone(),
                            mtype: "notify".into(),
                            subject: Some("reply-pending".into()),
                            body: reply_reminder_body(&pending),
                            in_reply_to: None,
                            created_ms: now,
                            state: "pending".into(),
                            wake_attempt_count: 0,
                            last_wake_attempt_ms: 0,
                            retry_attempted: false,
                        },
                    });
                    if let Some(subscription) =
                        state.matching_subscription(&worker.id, "direct-message", None, now)
                    {
                        events.push(Event::WakeBound {
                            message_id: alert_id,
                            subscription_id: subscription.id.clone(),
                        });
                    }
                    for obligation in &pending {
                        if let Some(message_id) = obligation["message_id"].as_str() {
                            events.push(Event::ReplyObligationReminded {
                                message_id: message_id.to_owned(),
                                reminded: true,
                            });
                        }
                    }
                }
            }
            if idle_signal_due {
                let signal = if is_live_master {
                    super::state::MasterWakeSignal::MasterIdle {
                        worker_id: worker.id.clone(),
                    }
                } else if let Some(child) = &managed {
                    super::state::MasterWakeSignal::SubagentStatus {
                        subagent_id: child.id.clone(),
                    }
                } else {
                    super::state::MasterWakeSignal::WorkerIdle {
                        worker_id: worker.id.clone(),
                    }
                };
                events.push(Event::MasterWakeSignal { signal, at_ms: now });
            } else if working_signal_due {
                let signal = if let Some(child) = &managed {
                    super::state::MasterWakeSignal::SubagentWorking {
                        subagent_id: child.id.clone(),
                    }
                } else {
                    super::state::MasterWakeSignal::WorkerWorking {
                        worker_id: worker.id.clone(),
                    }
                };
                events.push(Event::MasterWakeSignal { signal, at_ms: now });
            }
            if let Some(mut child) = managed.clone() {
                if matches!(agent, AgentState::Working | AgentState::Waiting)
                    && child.status != observed_str
                {
                    child.status = observed_str.into();
                    events.push(Event::SubagentUpdated { subagent: child });
                }
            }
            if is_idle && managed.is_some() && prior_observed != "working" && !record.working_seen {
                record.pending_since_ms = now;
            } else if !is_idle && record.pending_since_ms == 0 {
                record.pending_since_ms = now;
            } else if record.pending_since_ms == 0 {
                record.pending_since_ms = now;
            }
            let new_idle_reason = is_idle && record.idle_episode_reason != idle_reason;
            if new_idle_reason {
                record.idle_episode_reason = idle_reason.into();
            }
            if record != old {
                events.insert(
                    0,
                    Event::KeepaliveUpdated {
                        worker_id: worker.id.clone(),
                        record,
                    },
                );
            }
            if !events.is_empty() {
                server.commit_locked_reporting(&mut state, &events);
            }
        }
    }
    flush_idle_batches_if_ready(server, now);
    board_patrol_pass_at(server, now);
}

/// Periodic board-idle patrol.
///
/// The due gate is evaluated before the transport read. A read then happens
/// outside the reducer lock, and the same typed carrier, task/cancellation set
/// and gate are revalidated before any wake is committed. Unknown or waiting
/// hosts advance the periodic window; a transport error is logged and treated
/// as Unknown so a failed read cannot leave the gate due for every tick.
fn board_patrol_pass_at(server: &Server, now: i64) {
    if !server.config.notifications.enabled {
        return;
    }
    let workers: Vec<_> = {
        let state = server.state.lock().unwrap();
        if state.admission_frozen() {
            return;
        }
        state.workers.values().cloned().collect()
    };
    for worker in workers {
        board_patrol_worker_at(server, &worker, now);
    }
}

fn board_patrol_worker_at(server: &Server, worker: &super::state::WorkerRec, now: i64) {
    use super::name_state::AgentActivity;
    use crate::board_patrol_policy::{evaluate_board_patrol, PatrolDecision, PatrolTriggerMode};

    let candidate = {
        let state = server.state.lock().unwrap();
        if state.admission_frozen() {
            return;
        }
        board_patrol_candidate(&state, worker, now)
    };
    let Some(candidate) = candidate else {
        return;
    };
    let activity = match current_board_patrol_activity(server, worker) {
        Ok(activity) => activity,
        Err(error) => {
            super::presence::append_log(
                &server.log_path(),
                &format!("BOARD_PATROL_ACTIVITY_READ_FAILED: {}: {error}", worker.id),
            );
            AgentActivity::Unknown
        }
    };

    let mut state = server.state.lock().unwrap();
    if state.admission_frozen() || state.workers.get(&worker.id) != Some(worker) {
        return;
    }
    if super::state::board_patrol_current_owner_key(&state, &worker.id).as_deref()
        != Some(candidate.owner_key.as_str())
    {
        return;
    }
    if super::state::board_patrol_eligible_task_ids(&state, &worker.id).as_deref()
        != Some(candidate.eligible.as_slice())
    {
        return;
    }
    if state.board_patrol_gates.get(&worker.id) != candidate.gate.as_ref() {
        return;
    }
    if board_patrol_name_address(&state, worker, &candidate.owner_key) != candidate.address {
        return;
    }
    // Board patrol owns its own trusted episode, tracked on the gate. A fresh
    // idle poll seeds the start; a continuous idle poll extends it; a
    // non-idle poll contributes no trusted start. The gate is the durable
    // typed owner: a persisted `idle_episode_start_ms` survives a daemon
    // restart because it was recorded by the new schema and is never
    // backfilled from an older snapshot.
    let idle_since_ms = if activity == AgentActivity::Idle {
        candidate
            .gate
            .as_ref()
            .filter(|gate| gate.last_poll_idle)
            .and_then(|gate| gate.idle_episode_start_ms)
            .filter(|start| *start > 0 && *start <= now)
            .unwrap_or(now)
    } else {
        0
    };
    let decision = evaluate_board_patrol(
        &candidate.eligible,
        &candidate.owner_key,
        PatrolTriggerMode::PeriodicPoll,
        activity == AgentActivity::Idle,
        activity == AgentActivity::Busy,
        idle_since_ms,
        now,
        candidate.gate.as_ref(),
    );
    match decision {
        PatrolDecision::NoEligibleTasks => {}
        PatrolDecision::NotDueYet { gate } => {
            if candidate.gate.as_ref() != Some(&gate) {
                server.commit_locked_reporting(
                    &mut state,
                    &[Event::BoardPatrolGateUpdated {
                        worker_id: worker.id.clone(),
                        gate,
                    }],
                );
            }
        }
        PatrolDecision::SkipBusy { gate } | PatrolDecision::SkipUnknown { gate } => {
            server.commit_locked_reporting(
                &mut state,
                &[Event::BoardPatrolGateUpdated {
                    worker_id: worker.id.clone(),
                    gate,
                }],
            );
        }
        PatrolDecision::WakeDue { tasks, gate } => {
            let Some((message_id, subscription_id, events)) = board_patrol_wake_events(
                &state,
                worker,
                candidate.address.as_ref(),
                &tasks,
                gate,
                now,
            ) else {
                return;
            };
            if server.commit_locked_checked(&mut state, &events).is_err() {
                return;
            }
            drop(state);
            let _ = super::attempt_notification_detailed_with_at(
                server,
                &message_id,
                &subscription_id,
                now,
            );
        }
    }
}

fn board_patrol_wake_events(
    state: &State,
    worker: &super::state::WorkerRec,
    address: Option<&super::name_mailbox::MailboxNameAddress>,
    tasks: &[String],
    gate: crate::board_patrol_policy::PeerBoardPatrolGate,
    now: i64,
) -> Option<(String, String, Vec<Event>)> {
    let subscription = board_patrol_subscription(state, worker, now)?;
    let message_id = super::gen_msg_id();
    let message = Message {
        id: message_id.clone(),
        from: "collab-server".into(),
        to: worker.id.clone(),
        mtype: "notify".into(),
        subject: Some("board-idle-patrol".into()),
        body: board_patrol_body(tasks),
        in_reply_to: None,
        created_ms: now,
        state: "pending".into(),
        wake_attempt_count: 0,
        last_wake_attempt_ms: 0,
        retry_attempted: false,
    };
    let mut events = vec![
        Event::BoardPatrolGateUpdated {
            worker_id: worker.id.clone(),
            gate,
        },
        Event::DeliveryMode {
            msg_id: message_id.clone(),
            mode: "explicit-notification".into(),
            source_thread_id: None,
        },
        Event::BoardPatrolNoticeBound {
            message_id: message_id.clone(),
        },
        Event::Sent { msg: message },
    ];
    if let Some(address) = address {
        events.push(Event::MailboxNameAddressed {
            message_id: message_id.clone(),
            address: address.clone(),
        });
    }
    events.push(Event::WakeBound {
        message_id: message_id.clone(),
        subscription_id: subscription.id.clone(),
    });
    Some((message_id, subscription.id, events))
}

struct BoardPatrolCandidate {
    owner_key: String,
    eligible: Vec<String>,
    gate: Option<crate::board_patrol_policy::PeerBoardPatrolGate>,
    address: Option<super::name_mailbox::MailboxNameAddress>,
}

fn board_patrol_candidate(
    state: &State,
    worker: &super::state::WorkerRec,
    now: i64,
) -> Option<BoardPatrolCandidate> {
    worker.transport.as_ref()?;
    let owner_key = super::state::board_patrol_current_owner_key(state, &worker.id)?;
    let eligible = super::state::board_patrol_eligible_task_ids(state, &worker.id)?;
    if eligible.is_empty() {
        return None;
    }
    let gate = state.board_patrol_gates.get(&worker.id).cloned();
    // The periodic window alone decides whether a fresh activity read is
    // worth doing. The trusted idle duration is evaluated after that read.
    if gate.as_ref().is_some_and(|gate| now < gate.next_due_ms) {
        return None;
    }
    Some(BoardPatrolCandidate {
        address: board_patrol_name_address(state, worker, &owner_key),
        owner_key,
        eligible,
        gate,
    })
}

fn board_patrol_name_address(
    state: &State,
    worker: &super::state::WorkerRec,
    owner_key: &str,
) -> Option<super::name_mailbox::MailboxNameAddress> {
    let session_id = worker.transport.as_ref()?.session_id.as_deref();
    let holder_matches = |holder: &super::name_state::NameOccupancy| {
        holder.name == owner_key
            && holder.worker_id == worker.id
            && session_id.is_none_or(|session| session == holder.session_id)
    };
    if let Some(holder) = state.name_occupancy(&worker.cwd, owner_key) {
        if holder_matches(holder) {
            return Some(super::name_mailbox::MailboxNameAddress {
                project_scope: worker.cwd.clone(),
                name: owner_key.to_owned(),
            });
        }
    }
    state
        .name_occupancies
        .iter()
        .find_map(|(project_scope, names)| {
            names
                .get(owner_key)
                .filter(|holder| holder_matches(holder))
                .map(|_| super::name_mailbox::MailboxNameAddress {
                    project_scope: project_scope.clone(),
                    name: owner_key.to_owned(),
                })
        })
}

fn board_patrol_subscription(
    state: &State,
    worker: &super::state::WorkerRec,
    now: i64,
) -> Option<super::state::NotificationSubscription> {
    let transport = worker.transport.as_ref()?;
    state
        .notification_subscriptions
        .values()
        .filter(|subscription| {
            subscription.matches(&worker.id, "direct-message", None, now)
                && super::subscription_matches_transport(subscription, transport)
        })
        .min_by_key(|subscription| (subscription.created_ms, subscription.id.as_str()))
        .cloned()
}

fn board_patrol_body(tasks: &[String]) -> String {
    format!(
        "Board idle patrol: you are idle while still owning open board work. Tasks: {}.\nRe-read the current tasks and blockers with `collab board show`, then continue the most useful next action. This notice is not a consumption receipt and does not close or complete any task.\nCancel patrol for a task only while waiting on the user, an external dependency, or an intentional pause: `collab notify cancel --task-id <task-id> --reason '<reason>'`.\nRearm a task after you resume or its blocker clears: `collab notify subscribe --event board-idle --task-id <task-id>`.",
        tasks.join(", ")
    )
}

fn reply_reminder_body(obligations: &[serde_json::Value]) -> String {
    let mut body = String::from(
        "Reply pending. Read the original message with `collab msg <message_id>` and use its reply entry below. For on_completion, keep this marker until actual completion; ordinary progress must omit --in-reply-to. Do not reply to this reminder.\n",
    );
    for obligation in obligations {
        body.push_str(&format!(
            "- message_id={} from={} policy={}\n",
            obligation["message_id"].as_str().unwrap_or(""),
            obligation["from"].as_str().unwrap_or(""),
            obligation["policy"].as_str().unwrap_or(""),
        ));
        if obligation["from_ceo"].as_bool() == Some(true) {
            body.push_str("  Reply through `collab ceo reply --subject '<subject>' --in-reply-to '<message_id>' '<actual completed result>'`.\n");
        } else {
            body.push_str("  Reply through `collab sendmessage --to '<from>' --subject '<subject>' --in-reply-to '<message_id>' '<your answer>'`.\n");
        }
    }
    body
}

fn flush_idle_batches_if_ready(server: &Server, now: i64) {
    if !server.config.notifications.enabled {
        return;
    }
    let mut state = server.state.lock().unwrap();
    if state.admission_frozen() {
        return;
    }
    let Some(master_id) = super::current_master_holder(server, &state).ok().flatten() else {
        return;
    };
    let master_record = state.keepalives.get(&master_id).cloned();
    if master_record.as_ref().is_some_and(|record| {
        matches!(
            record.observed.as_str(),
            "working" | "waiting_user" | "unknown"
        )
    }) {
        return;
    }
    if state.master_wake.newly_idle_workers.is_empty() {
        return;
    }
    let Some(subscription) = state
        .matching_subscription(&master_id, "direct-message", None, now)
        .cloned()
    else {
        return;
    };
    let newly_idle = state.master_wake.newly_idle_workers.clone();
    let live_idle = state.master_wake.idle_workers.clone();
    let blocked = state.master_wake.blocked_or_timed_out_tasks.clone();
    let mut pending_merges: Vec<_> = state.pending_merges.keys().cloned().collect();
    pending_merges.sort();
    let subject = if newly_idle.len() == 1 && newly_idle[0].starts_with("subagent:") {
        "subagent-status".to_string()
    } else if newly_idle.len() == 1 {
        "worker-idle".to_string()
    } else {
        "master-wake-batch".to_string()
    };
    let body = format!(
        "newly_idle={} live_idle={} blocked={} pending_merges={} goal_due={}. Long-horizon orchestration: assign suitable long-horizon goals through the public board to authorized peers; if no peer is available, retain and split the goal. Prefer assigning code implementation to subagents; orchestrate scheduling, acceptance, delivery, integration, and owned-resource closure instead of implementing the mainline yourself. Dispatch one-shot execution or observation directly to independent `codex exec --profile gcm` workers regardless of whether other peers are busy. For every assignment, state goal, scope, authorized actions, resource owner, delivery/test evidence, report and stop conditions. Master finite patrol: inspect open P0/P1 work, actual peer progress, owned worktrees and runtime resources, project-canonical issue intake, and required rearm. Subscription presence does not gate identity; do not invent a polling interval. A peer-owned goal patrols goal progress, blockers, and owned resources, and asks the current master first when blocked. Remove unneeded owned worktrees after recoverable extraction; reclaim owned stale resources; close each open P0/P1 with evidence or explicitly park it with owner, cause, and unblock condition; preserve and report protected holds (dirty, active, shared, or unknown resources). Verify evidence and integrate accepted pending candidates. AppSDK/Collab and tmux lifecycles remain independent; never stop or restart a tmux server or terminate host panes during patrol or cleanup. Before ending the patrol, re-arm a consumed one-shot goal or deadline wake with `appsdk goal subscribe` when continued scheduling is required; a read, ACK, summary, or dispatch receipt is not completion. Managed subagent records and `collab subagent dispatch` are legacy compatibility for diagnosis or migration only. When no actionable task, dependency, resolvable blocker, or authorized open issue remains, finish the patrol and keep the reusable direct-message subscription for new assignments; cancel only finite timed patrol or deadline subscriptions. If the goal is complete, report the evidence to the user.",
        newly_idle.join(","),
        live_idle.join(","),
        blocked.join(","),
        pending_merges.join(","),
        state.master_wake.goal_due
    );
    let alert_id = super::gen_msg_id();
    let mut events = vec![
        Event::Sent {
            msg: Message {
                id: alert_id.clone(),
                from: "collab-server".into(),
                to: master_id.clone(),
                mtype: if subject == "subagent-status" {
                    "subagent-status".into()
                } else {
                    "notify".into()
                },
                subject: Some(subject.clone()),
                body,
                in_reply_to: None,
                created_ms: now,
                state: "pending".into(),
                wake_attempt_count: 0,
                last_wake_attempt_ms: 0,
                retry_attempted: false,
            },
        },
        Event::WakeBound {
            message_id: alert_id,
            subscription_id: subscription.id,
        },
    ];
    for idle in &newly_idle {
        let worker_id = idle.strip_prefix("subagent:").map_or(idle.as_str(), |id| {
            state
                .subagents
                .get(id)
                .map(|record| record.peer.as_str())
                .unwrap_or(id)
        });
        let mut record = state.keepalives.get(worker_id).cloned().unwrap_or_default();
        record.idle_episode_notices = record.idle_episode_notices.max(1);
        record.last_notice_ms = now;
        record.notified_state = "idle".into();
        record.working_seen = false;
        record.pending_since_ms = now;
        record.observed = "idle".into();
        events.push(Event::KeepaliveUpdated {
            worker_id: worker_id.into(),
            record,
        });
    }
    crate::server::notification_state::mark_idle_batch_delivered(&mut state.master_wake);
    events.push(Event::MasterWakeUpdated {
        accumulator: state.master_wake.clone(),
    });
    server.commit_locked_reporting(&mut state, &events);
}

/// Read one worker's current activity for board-patrol wake decisions.
///
/// This is a single fresh read through the selected transport. It does not
/// touch state, timers or notifications, and an Unknown or error is never
/// capacity: the caller must verify the binding after the read before acting.
pub(super) fn current_board_patrol_activity(
    server: &Server,
    worker: &super::state::WorkerRec,
) -> Result<super::name_state::AgentActivity, String> {
    use super::name_state::AgentActivity;
    let Some(transport) = super::selected_transport_for_worker(worker) else {
        return Ok(AgentActivity::Unknown);
    };
    match transport.kind {
        crate::proto::TransportKind::Tmux => {
            let Some(endpoint) = transport.tmux_endpoint.as_ref() else {
                return Ok(AgentActivity::Unknown);
            };
            let observed = crate::client::adapters::tmux::observe_agent(endpoint)?;
            Ok(match (observed.agent_presence, observed.activity) {
                (
                    crate::client::adapters::tmux::AgentPresence::Online,
                    crate::client::adapters::tmux::AgentActivity::Idle,
                ) => AgentActivity::Idle,
                (
                    crate::client::adapters::tmux::AgentPresence::Online,
                    crate::client::adapters::tmux::AgentActivity::Busy,
                ) => AgentActivity::Busy,
                (
                    crate::client::adapters::tmux::AgentPresence::Online,
                    crate::client::adapters::tmux::AgentActivity::WaitingUser,
                ) => AgentActivity::WaitingUser,
                _ => AgentActivity::Unknown,
            })
        }
        crate::proto::TransportKind::AppServer | crate::proto::TransportKind::DesktopBridge => {
            let Some(thread_id) = transport.thread_id.as_deref() else {
                return Ok(AgentActivity::Unknown);
            };
            let raw = (server.appserver_thread_status)(&transport, thread_id)?;
            Ok(native_thread_activity(&raw))
        }
        crate::proto::TransportKind::Dsh => {
            let (Some(endpoint), Some(runtime_id), Some(agent_id)) = (
                transport.endpoint.as_deref(),
                transport.namespace.as_deref(),
                transport.thread_id.as_deref(),
            ) else {
                return Ok(AgentActivity::Unknown);
            };
            let facts = crate::client::adapters::dsh::activity(endpoint, runtime_id, agent_id)
                .map_err(|error| error.to_string())?;
            Ok(if !facts.live {
                AgentActivity::Unknown
            } else {
                match facts.status.as_str() {
                    "inactive" => AgentActivity::Idle,
                    "running" => AgentActivity::Busy,
                    _ => AgentActivity::Unknown,
                }
            })
        }
    }
}

/// Classify one native `thread/read` reply. Only a present, well-formed idle
/// thread with no active flag or in-flight turn is positive idle.
fn native_thread_activity(raw: &serde_json::Value) -> super::name_state::AgentActivity {
    use super::name_state::AgentActivity;
    let Some(kind) = raw
        .pointer("/thread/status/type")
        .and_then(serde_json::Value::as_str)
    else {
        return AgentActivity::Unknown;
    };
    if kind == "waiting_user" {
        return AgentActivity::WaitingUser;
    }
    // Optional activity fields used for idle decisions. A present field with
    // the wrong shape is a malformed native reply and must not be guessed into
    // Idle/Busy/WaitingUser. An absent field (or explicit null) means "no
    // value" and is not a new error.
    let active_flags: Vec<&str> = match raw
        .pointer("/thread/status/activeFlags")
        .or_else(|| raw.pointer("/activeFlags"))
    {
        None | Some(serde_json::Value::Null) => Vec::new(),
        Some(serde_json::Value::Array(flags)) => {
            let mut strings = Vec::with_capacity(flags.len());
            for flag in flags {
                match flag.as_str() {
                    Some(text) => strings.push(text),
                    // A non-string array member makes the field malformed.
                    None => return AgentActivity::Unknown,
                }
            }
            strings
        }
        // Present but not a string array: malformed.
        Some(_) => return AgentActivity::Unknown,
    };
    // Only the exact known native waiting flags mean WaitingUser. Any other
    // flag string, including an empty one, is unsupported and makes the reply
    // Unknown rather than a guessed state.
    let mut waiting = false;
    let mut unsupported_flag = false;
    for flag in &active_flags {
        if matches!(*flag, "waitingOnApproval" | "waitingOnUserInput") {
            waiting = true;
        } else {
            unsupported_flag = true;
        }
    }
    if unsupported_flag {
        return AgentActivity::Unknown;
    }
    if waiting {
        return AgentActivity::WaitingUser;
    }
    let active_turns = match raw.pointer("/activeTurns") {
        None | Some(serde_json::Value::Null) => false,
        Some(serde_json::Value::Array(turns)) => !turns.is_empty(),
        Some(_) => return AgentActivity::Unknown,
    };
    let turn_in_progress = match raw
        .pointer("/thread/latestTurnStatus")
        .or_else(|| raw.pointer("/turns/0/status"))
    {
        None | Some(serde_json::Value::Null) => false,
        Some(serde_json::Value::String(status)) => {
            matches!(status.as_str(), "inProgress" | "active")
        }
        Some(_) => return AgentActivity::Unknown,
    };
    match kind {
        "idle" if !active_turns && !turn_in_progress => AgentActivity::Idle,
        "idle" | "active" | "inProgress" => AgentActivity::Busy,
        _ => AgentActivity::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::super::peer_tests::{register, test_server};
    use super::*;
    use crate::subagent::Record as SubagentRecord;

    fn managed(id: &str, parent: &str, peer: &str, status: &str, now: i64) -> SubagentRecord {
        SubagentRecord {
            id: id.into(),
            parent: parent.into(),
            peer: peer.into(),
            status: status.into(),
            thread_id: Some(format!("thread-{peer}")),
            profile: None,
            created_ms: now,
            ready_deadline_ms: now + 90_000,
            last_message: None,
            error: None,
            probe_failures: Vec::new(),
            runtime: Some("codex".into()),
            create_operation_id: None,
            binding_id: None,
            endpoint_generation: None,
        }
    }

    fn task(id: &str, owner: &str, status: &str, now: i64) -> crate::server::state::TaskRec {
        crate::server::state::TaskRec {
            id: id.into(),
            owner: owner.into(),
            created_by: "master".into(),
            feature_id: None,
            worktree_path: None,
            branch: None,
            base_commit: None,
            priority: "p2".into(),
            status: status.into(),
            next_step: Some("work".into()),
            wait: None,
            created_ms: now,
            updated_ms: now,
        }
    }

    fn status_count(server: &Server) -> usize {
        server
            .state
            .lock()
            .unwrap()
            .msgs
            .values()
            .filter(|message| message.mtype == "subagent-status")
            .count()
    }

    #[test]
    fn appserver_worker_without_managed_state_stays_unknown() {
        let (server, root) = test_server();
        register(&server, "worker", "thread-worker");
        let base = super::super::state::now_ms();
        tick_at(&server, base);
        let state = server.state.lock().unwrap();
        assert!(!state.keepalives.contains_key("worker"));
        assert!(state.msgs.is_empty());
        drop(state);
        std::fs::remove_dir_all(root).unwrap();
    }

    fn observe(server: &Server, activity: super::super::name_state::AgentActivity, session: &str) {
        use super::super::name_state::*;
        let scope = server
            .root
            .canonicalize()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        server.commit(&[
            Event::AgentPresenceObserved {
                presence: AgentPresenceRecord {
                    project_scope: scope.clone(),
                    worker_id: "worker".into(),
                    session_id: session.into(),
                    presence: AgentPresence::Online,
                    reason: "fixture".into(),
                    observed_ms: 1,
                },
            },
            Event::AgentActivityObserved {
                activity: AgentActivityRecord {
                    project_scope: scope,
                    worker_id: "worker".into(),
                    session_id: session.into(),
                    activity,
                    background: false,
                    reason: "fixture".into(),
                    observed_ms: 1,
                    idle_since_ms: None,
                },
            },
        ]);
    }

    #[test]
    fn ordinary_typed_activity_drives_one_idle_episode() {
        use super::super::name_state::AgentActivity::*;
        let (server, root) = test_server();
        super::super::peer_tests::register_synthetic_peer(&server, "worker", "thread-worker");
        server
            .state
            .lock()
            .unwrap()
            .workers
            .get_mut("worker")
            .unwrap()
            .transport
            .as_mut()
            .unwrap()
            .session_id = Some("current".into());
        let now = super::super::state::now_ms();
        observe(&server, Busy, "current");
        tick_at(&server, now);
        assert_eq!(
            server.state.lock().unwrap().keepalives["worker"].observed,
            "working"
        );
        for activity in [WaitingUser, Unknown] {
            observe(&server, activity, "current");
            tick_at(&server, now + 1);
            assert!(server
                .state
                .lock()
                .unwrap()
                .master_wake
                .newly_idle_workers
                .is_empty());
        }
        observe(&server, Idle, "current");
        tick_at(&server, now + 2);
        {
            let state = server.state.lock().unwrap();
            assert_eq!(state.keepalives["worker"].observed, "idle");
            assert!(!state.master_wake.newly_idle_workers.is_empty());
        }
        let wake = server.state.lock().unwrap().master_wake.clone();
        tick_at(&server, now + 3);
        assert_eq!(
            serde_json::to_value(&server.state.lock().unwrap().master_wake).unwrap(),
            serde_json::to_value(wake).unwrap()
        );
        observe(&server, WaitingUser, "current");
        tick_at(&server, now + 4);
        assert!(server
            .state
            .lock()
            .unwrap()
            .master_wake
            .idle_workers
            .is_empty());
        assert!(server
            .state
            .lock()
            .unwrap()
            .master_wake
            .newly_idle_workers
            .is_empty());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn ordinary_idle_sends_one_existing_batch_notification() {
        use super::super::name_state::AgentActivity::*;
        use super::super::peer_tests::register_synthetic_peer;
        let (server, root) = test_server();
        register_synthetic_peer(&server, "worker", "thread-worker");
        register_synthetic_peer(&server, "master", "thread-master");
        assert!(
            super::super::handle_master_promote(
                &server,
                "master".into(),
                "token-master".into(),
                "user approved master".into()
            )
            .ok
        );
        server
            .state
            .lock()
            .unwrap()
            .workers
            .get_mut("worker")
            .unwrap()
            .transport
            .as_mut()
            .unwrap()
            .session_id = Some("current".into());
        let now = super::super::state::now_ms();
        observe(&server, Busy, "current");
        tick_at(&server, now);
        observe(&server, Idle, "current");
        tick_at(&server, now + 1);
        tick_at(&server, now + 2);
        let state = server.state.lock().unwrap();
        let notices: Vec<_> = state
            .msgs
            .values()
            .filter(|m| m.to == "master" && m.subject.as_deref() == Some("worker-idle"))
            .collect();
        assert_eq!(notices.len(), 1);
        assert!(notices[0].body.contains("newly_idle=worker"));
        assert!(!state.subagents.values().any(|child| child.peer == "worker"));
        drop(state);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn ordinary_stale_activity_does_not_override_current_session() {
        use super::super::name_state::AgentActivity::*;
        let (server, root) = test_server();
        super::super::peer_tests::register_synthetic_peer(&server, "worker", "thread-worker");
        server
            .state
            .lock()
            .unwrap()
            .workers
            .get_mut("worker")
            .unwrap()
            .transport
            .as_mut()
            .unwrap()
            .session_id = Some("current".into());
        observe(&server, Idle, "previous");
        tick_at(&server, super::super::state::now_ms());
        assert!(!server
            .state
            .lock()
            .unwrap()
            .keepalives
            .contains_key("worker"));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn ordinary_projection_separates_presence_activity_and_session() {
        use super::super::name_state::{AgentActivity::*, AgentPresence};
        let (server, root) = test_server();
        super::super::peer_tests::register_synthetic_peer(&server, "worker", "thread-worker");
        server
            .state
            .lock()
            .unwrap()
            .workers
            .get_mut("worker")
            .unwrap()
            .transport
            .as_mut()
            .unwrap()
            .session_id = Some("current".into());
        let worker = server.state.lock().unwrap().workers["worker"].clone();
        for (activity, label) in [
            (Busy, "busy"),
            (Idle, "idle"),
            (WaitingUser, "waiting_user"),
            (Unknown, "unknown"),
        ] {
            observe(&server, activity, "current");
            let (presence, view) = super::super::worker_presence_with_view(&server, &worker);
            assert_eq!(presence, super::super::IdentityPresence::Present);
            assert_eq!(view["agent_presence"], "online");
            assert_eq!(view["activity"], label);
        }
        let mut presence = {
            let state = server.state.lock().unwrap();
            super::super::worker_agent_facts(&server, &state, &worker)
                .0
                .unwrap()
                .clone()
        };
        for (fact, expected, label) in [
            (
                AgentPresence::Unknown,
                super::super::IdentityPresence::Unknown,
                "unknown",
            ),
            (
                AgentPresence::Offline,
                super::super::IdentityPresence::Missing,
                "offline",
            ),
        ] {
            presence.presence = fact;
            server.commit(&[Event::AgentPresenceObserved {
                presence: presence.clone(),
            }]);
            let (actual, view) = super::super::worker_presence_with_view(&server, &worker);
            assert_eq!(actual, expected);
            assert_eq!(view["agent_presence"], label);
            assert_eq!(server.state.lock().unwrap().workers["worker"], worker);
        }
        observe(&server, Idle, "old");
        assert!(super::super::worker_agent_view(&server, &worker).is_none());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn ordinary_presence_edges_use_typed_online_and_offline_once() {
        use super::super::name_state::{AgentActivity, AgentPresence};
        use super::super::peer_tests::register_synthetic_peer;
        let (server, root) = test_server();
        register_synthetic_peer(&server, "worker", "thread-worker");
        register_synthetic_peer(&server, "master", "thread-master");
        assert!(
            super::super::handle_master_promote(
                &server,
                "master".into(),
                "token-master".into(),
                "user approved master".into()
            )
            .ok
        );
        server
            .state
            .lock()
            .unwrap()
            .workers
            .get_mut("worker")
            .unwrap()
            .transport
            .as_mut()
            .unwrap()
            .session_id = Some("current".into());
        observe(&server, AgentActivity::Unknown, "current");
        super::super::record_ordinary_peer_presence_edges(&server, Some("worker"));
        assert_eq!(
            server.state.lock().unwrap().keepalives["worker"].notified_presence,
            "online"
        );
        let mut presence = {
            let state = server.state.lock().unwrap();
            let worker = &state.workers["worker"];
            super::super::worker_agent_facts(&server, &state, worker)
                .0
                .unwrap()
                .clone()
        };
        for fact in [
            AgentPresence::Unknown,
            AgentPresence::Offline,
            AgentPresence::Offline,
            AgentPresence::Online,
            AgentPresence::Online,
        ] {
            presence.presence = fact;
            server.commit(&[Event::AgentPresenceObserved {
                presence: presence.clone(),
            }]);
            super::super::record_ordinary_peer_presence_edges(&server, Some("worker"));
        }
        let state = server.state.lock().unwrap();
        let subjects: Vec<_> = state
            .msgs
            .values()
            .filter(|m| m.to == "master")
            .filter_map(|m| m.subject.as_deref())
            .collect();
        assert_eq!(
            subjects
                .iter()
                .filter(|s| **s == "worker-unresponsive: worker")
                .count(),
            1
        );
        assert_eq!(
            subjects
                .iter()
                .filter(|s| **s == "worker-recovered: worker")
                .count(),
            1
        );
        assert_eq!(state.keepalives["worker"].notified_presence, "online");
        drop(state);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn managed_working_to_idle_reports_once_after_settle() {
        let (server, root) = test_server();
        register(&server, "master", "thread-master");
        register(&server, "child", "thread-child");
        assert!(
            super::super::handle_master_promote(
                &server,
                "master".into(),
                "token-master".into(),
                "user approved master".into(),
            )
            .ok
        );
        let now = super::super::state::now_ms();
        server.commit(&[
            Event::SubagentUpdated {
                subagent: managed("managed", "master", "child", "working", now),
            },
            Event::TaskCreated {
                task: task("task-child", "child", "working", now),
            },
        ]);
        tick_at(&server, now + 900_000);
        assert_eq!(status_count(&server), 0);
        {
            let mut state = server.state.lock().unwrap();
            let mut child = state.subagents["managed"].clone();
            child.status = "idle".into();
            state.subagents.insert(child.id.clone(), child.clone());
        }
        tick_at(&server, now + 1_800_000);
        assert_eq!(status_count(&server), 1);
        tick_at(&server, now + 2_700_000);
        assert_eq!(status_count(&server), 1);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn managed_idle_without_actionable_task_stays_quiet() {
        let (server, root) = test_server();
        register(&server, "master", "thread-master");
        register(&server, "child", "thread-child");
        assert!(
            super::super::handle_master_promote(
                &server,
                "master".into(),
                "token-master".into(),
                "user approved master".into(),
            )
            .ok
        );
        let now = super::super::state::now_ms();
        server.commit(&[Event::SubagentUpdated {
            subagent: managed("managed", "master", "child", "idle", now),
        }]);
        tick_at(&server, now + 1_000);
        tick_at(&server, now + 200_000);
        tick_at(&server, now + 400_000);
        assert_eq!(status_count(&server), 0);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn managed_working_stops_master_reminder_episode() {
        let (server, root) = test_server();
        register(&server, "master", "thread-master");
        register(&server, "child", "thread-child");
        assert!(
            super::super::handle_master_promote(
                &server,
                "master".into(),
                "token-master".into(),
                "user approved master".into(),
            )
            .ok
        );
        let now = super::super::state::now_ms();
        server.commit(&[
            Event::SubagentUpdated {
                subagent: managed("managed", "master", "child", "working", now),
            },
            Event::TaskCreated {
                task: task("task-child", "child", "working", now),
            },
        ]);
        tick_at(&server, now + 1_000);
        tick_at(&server, now + 2_000);
        {
            let mut state = server.state.lock().unwrap();
            let mut child = state.subagents["managed"].clone();
            child.status = "idle".into();
            state.subagents.insert(child.id.clone(), child.clone());
        }
        tick_at(&server, now + 3_000);
        let state = server.state.lock().unwrap();
        assert_eq!(
            state
                .msgs
                .values()
                .filter(|message| message.mtype == "subagent-status")
                .count(),
            0
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn duplicate_idle_signal_records_once_and_does_not_rearm() {
        let mut state = State::default();
        let signal = super::super::state::MasterWakeSignal::WorkerIdle {
            worker_id: "peer".into(),
        };
        state.apply(&Event::MasterWakeSignal {
            signal: signal.clone(),
            at_ms: 10,
        });
        state.apply(&Event::MasterWakeSignal { signal, at_ms: 20 });
        assert_eq!(state.master_wake.generation, 1);
        assert_eq!(state.master_wake.idle_workers, vec!["peer"]);
        assert_eq!(state.master_wake.newly_idle_workers, vec!["peer"]);
    }

    #[test]
    fn closed_subagent_and_removed_peer_leave_both_idle_lists() {
        let mut state = State::default();
        state.apply(&Event::MasterWakeSignal {
            signal: super::super::state::MasterWakeSignal::WorkerIdle {
                worker_id: "lost-peer".into(),
            },
            at_ms: 10,
        });
        state.apply(&Event::MasterWakeSignal {
            signal: super::super::state::MasterWakeSignal::SubagentStatus {
                subagent_id: "closed-child".into(),
            },
            at_ms: 11,
        });
        assert_eq!(
            state.master_wake.idle_workers,
            vec!["lost-peer", "subagent:closed-child"]
        );

        state.apply(&Event::LegacyWorkerRemoved {
            worker_id: "lost-peer".into(),
        });
        state.apply(&Event::SubagentUpdated {
            subagent: crate::subagent::Record {
                id: "closed-child".into(),
                parent: "master".into(),
                peer: "closed-peer".into(),
                status: "closed".into(),
                thread_id: Some("thread-closed-peer".into()),
                profile: None,
                created_ms: 1,
                ready_deadline_ms: 2,
                last_message: None,
                error: None,
                probe_failures: Vec::new(),
                runtime: Some("codex".into()),
                create_operation_id: None,
                binding_id: None,
                endpoint_generation: None,
            },
        });

        assert!(state.master_wake.idle_workers.is_empty());
        assert!(state.master_wake.newly_idle_workers.is_empty());
    }

    #[test]
    fn master_busy_defers_managed_idle_until_one_batch() {
        let (server, root) = test_server();
        register(&server, "master", "thread-master");
        register(&server, "child", "thread-child");
        assert!(
            super::super::handle_master_promote(
                &server,
                "master".into(),
                "token-master".into(),
                "user approved master".into(),
            )
            .ok
        );
        let now = super::super::state::now_ms();
        server.commit(&[
            Event::SubagentUpdated {
                subagent: managed("managed", "master", "child", "working", now),
            },
            Event::KeepaliveUpdated {
                worker_id: "master".into(),
                record: Record {
                    observed: "working".into(),
                    idle_since_ms: now - 1,
                    working_seen: true,
                    ..Default::default()
                },
            },
            Event::TaskCreated {
                task: task("task-child", "child", "working", now),
            },
        ]);
        tick_at(&server, now + 1_000);

        {
            let mut state = server.state.lock().unwrap();
            let mut child = state.subagents["managed"].clone();
            child.status = "idle".into();
            state.subagents.insert(child.id.clone(), child);
        }
        tick_at(&server, now + 61_002);
        {
            let state = server.state.lock().unwrap();
            assert!(
                state
                    .msgs
                    .values()
                    .all(|message| message.mtype != "subagent-status"),
                "master busy must not receive per-event subagent idle notices"
            );
            assert!(state
                .master_wake
                .idle_workers
                .contains(&"subagent:managed".into()));
        }

        server.commit(&[Event::KeepaliveUpdated {
            worker_id: "master".into(),
            record: Record {
                observed: "idle".into(),
                idle_since_ms: now + 61_003,
                working_seen: true,
                ..Default::default()
            },
        }]);
        tick_at(&server, now + 61_004);

        let state = server.state.lock().unwrap();
        let batch: Vec<_> = state
            .msgs
            .values()
            .filter(|message| message.to == "master")
            .collect();
        assert_eq!(batch.len(), 1, "idle-time consumption must be one batch");
        let body = &batch[0].body;
        assert!(body.contains("newly_idle=subagent:managed"), "{body}");
        assert!(body.contains("live_idle=subagent:managed"), "{body}");
        assert!(
            body.contains("regardless of whether other peers are busy"),
            "{body}"
        );
        assert!(body.contains("protected holds"), "{body}");
        assert!(body.contains("one-shot goal or deadline wake"), "{body}");
        drop(state);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn transport_identity_is_the_only_liveness_signal() {
        let (server, root) = test_server();
        register(&server, "worker", "thread-worker");
        let state = server.state.lock().unwrap();
        let worker = &state.workers["worker"];
        let transport = worker.transport.as_ref().unwrap();
        assert_eq!(transport.kind, crate::proto::TransportKind::Tmux);
        assert_eq!(transport.thread_id.as_deref(), Some("thread-worker"));
        drop(state);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn master_idle_batch_surfaces_pending_merges() {
        let (server, root) = test_server();
        register(&server, "master", "thread-master");
        register(&server, "child", "thread-child");
        assert!(
            super::super::handle_master_promote(
                &server,
                "master".into(),
                "token-master".into(),
                "user approved master".into(),
            )
            .ok
        );
        let now = super::super::state::now_ms();
        server.commit(&[
            Event::MergeRequested {
                request: crate::server::state::PendingMerge {
                    task_id: "task-merge".into(),
                    owner: "child".into(),
                    requested_by: "child".into(),
                    requested_ms: now,
                    candidate_commit: None,
                },
            },
            Event::MasterWakeSignal {
                signal: super::super::state::MasterWakeSignal::WorkerIdle {
                    worker_id: "child".into(),
                },
                at_ms: now,
            },
            Event::KeepaliveUpdated {
                worker_id: "master".into(),
                record: Record {
                    observed: "working".into(),
                    idle_since_ms: now - 1,
                    working_seen: true,
                    ..Default::default()
                },
            },
        ]);

        server.commit(&[Event::KeepaliveUpdated {
            worker_id: "master".into(),
            record: Record {
                observed: "idle".into(),
                idle_since_ms: now + 1,
                working_seen: true,
                ..Default::default()
            },
        }]);
        tick_at(&server, now + 2);

        let state = server.state.lock().unwrap();
        let batch = state
            .msgs
            .values()
            .find(|message| {
                message.to == "master" && message.body.contains("Long-horizon orchestration")
            })
            .expect("master idle batch");
        assert!(
            batch.body.contains("pending_merges=task-merge"),
            "{}",
            batch.body
        );
        assert!(
            batch
                .body
                .contains("keep the reusable direct-message subscription"),
            "{}",
            batch.body
        );
        assert!(
            !batch.body.contains("collab notify unsubscribe"),
            "idle guidance must not cancel the receive subscription: {}",
            batch.body
        );
        drop(state);
        std::fs::remove_dir_all(root).unwrap();
    }

    /// Seed a synthetic peer whose selected transport is a native thread read
    /// callback. No tmux server is started and the reducer lock is released
    /// before the caller probed anything.
    fn patrol_appserver_worker(
        server: &mut Server,
        thread_id: &str,
    ) -> super::super::state::WorkerRec {
        super::super::peer_tests::register_synthetic_peer(server, "worker", thread_id);
        let mut state = server.state.lock().unwrap();
        state.workers.get_mut("worker").unwrap().transport = Some(
            super::super::peer_tests::test_appserver_transport(thread_id),
        );
        let worker = state.workers["worker"].clone();
        drop(state);
        worker
    }

    #[test]
    fn board_idle_patrol_activity_native_thread_status_table() {
        use super::super::name_state::AgentActivity;
        let (mut server, root) = test_server();
        let worker = patrol_appserver_worker(&mut server, "thread-patrol-table");
        for (status, active_turns, expected) in [
            (
                serde_json::json!({"type": "idle"}),
                serde_json::json!([]),
                AgentActivity::Idle,
            ),
            (
                serde_json::json!({"type": "notLoaded"}),
                serde_json::json!([]),
                AgentActivity::Unknown,
            ),
            (
                serde_json::json!({"type": "systemError"}),
                serde_json::json!([]),
                AgentActivity::Unknown,
            ),
            (
                serde_json::json!({"type": "surprise"}),
                serde_json::json!([]),
                AgentActivity::Unknown,
            ),
        ] {
            server.appserver_thread_status = std::sync::Arc::new(move |_, thread| {
                Ok(serde_json::json!({
                    "thread": {"id": thread, "status": status.clone()},
                    "activeTurns": active_turns.clone(),
                }))
            });
            assert_eq!(
                super::current_board_patrol_activity(&server, &worker).unwrap(),
                expected,
            );
        }
        // An idle thread that still reports an in-flight turn is not idle.
        server.appserver_thread_status = std::sync::Arc::new(|_, thread| {
            Ok(serde_json::json!({
                "thread": {"id": thread, "status": {"type": "idle"}},
                "activeTurns": [{"id": "turn-active"}],
            }))
        });
        assert_eq!(
            super::current_board_patrol_activity(&server, &worker).unwrap(),
            AgentActivity::Busy,
        );
        // A reply without the native status type is malformed, never idle.
        server.appserver_thread_status =
            std::sync::Arc::new(|_, thread| Ok(serde_json::json!({"thread": {"id": thread}})));
        assert_eq!(
            super::current_board_patrol_activity(&server, &worker).unwrap(),
            AgentActivity::Unknown,
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn board_idle_patrol_activity_fresh_busy_overrides_stale_cached_idle() {
        use super::super::name_state::AgentActivity;
        let (mut server, root) = test_server();
        let worker = patrol_appserver_worker(&mut server, "thread-patrol-stale");
        observe(&server, AgentActivity::Idle, "session-thread-patrol-stale");
        let cached = {
            let state = server.state.lock().unwrap();
            super::super::worker_agent_facts(&server, &state, &worker)
                .1
                .map(|fact| fact.activity)
        };
        assert_eq!(cached, Some(AgentActivity::Idle));
        server.appserver_thread_status = std::sync::Arc::new(|_, thread| {
            Ok(serde_json::json!({
                "thread": {"id": thread, "status": {"type": "active", "activeFlags": []}},
            }))
        });
        assert_eq!(
            super::current_board_patrol_activity(&server, &worker).unwrap(),
            AgentActivity::Busy,
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn board_idle_patrol_activity_native_read_error_is_preserved() {
        let (mut server, root) = test_server();
        let worker = patrol_appserver_worker(&mut server, "thread-patrol-error");
        server.appserver_thread_status =
            std::sync::Arc::new(|_, _| Err("native thread read exploded".to_owned()));
        let error = super::current_board_patrol_activity(&server, &worker).unwrap_err();
        assert!(error.contains("native thread read exploded"), "{error}");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn board_idle_patrol_activity_waiting_flags_are_never_idle() {
        use super::super::name_state::AgentActivity;
        let (mut server, root) = test_server();
        let worker = patrol_appserver_worker(&mut server, "thread-patrol-flags");
        for (flags, expected) in [
            (serde_json::json!([]), AgentActivity::Busy),
            (
                serde_json::json!(["waitingOnApproval"]),
                AgentActivity::WaitingUser,
            ),
            (
                serde_json::json!(["waitingOnUserInput"]),
                AgentActivity::WaitingUser,
            ),
            (serde_json::json!([""]), AgentActivity::Unknown),
            (
                serde_json::json!(["waitingOnApproval", "unsupportedFlag"]),
                AgentActivity::Unknown,
            ),
        ] {
            server.appserver_thread_status = std::sync::Arc::new(move |_, thread| {
                Ok(serde_json::json!({
                    "thread": {
                        "id": thread,
                        "status": {"type": "active", "activeFlags": flags.clone()},
                    },
                }))
            });
            let actual = super::current_board_patrol_activity(&server, &worker).unwrap();
            assert_eq!(actual, expected);
            assert_ne!(actual, AgentActivity::Idle);
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    /// Malformed optional activity fields on an otherwise idle thread must be
    /// Unknown, never Idle or a guessed Busy/WaitingUser. Only absent fields
    /// (or explicit null) mean "no value"; a present field of the wrong shape
    /// is a malformed native reply.
    #[test]
    fn board_idle_patrol_activity_malformed_activity_fields_are_unknown() {
        use super::super::name_state::AgentActivity;
        let (mut server, root) = test_server();
        let worker = patrol_appserver_worker(&mut server, "thread-patrol-strict");
        for (label, raw) in [
            (
                "activeFlags object",
                serde_json::json!({
                    "thread": {
                        "id": "thread-patrol-strict",
                        "status": {"type": "idle", "activeFlags": {"waiting": true}},
                    },
                    "activeTurns": [],
                }),
            ),
            (
                "activeFlags non-string member",
                serde_json::json!({
                    "thread": {
                        "id": "thread-patrol-strict",
                        "status": {"type": "idle", "activeFlags": ["waitingOnApproval", 7]},
                    },
                    "activeTurns": [],
                }),
            ),
            (
                "unsupported flag keyword",
                serde_json::json!({
                    "thread": {
                        "id": "thread-patrol-strict",
                        "status": {"type": "idle", "activeFlags": ["waitingForApproval"]},
                    },
                    "activeTurns": [],
                }),
            ),
            (
                "activeTurns non-array",
                serde_json::json!({
                    "thread": {
                        "id": "thread-patrol-strict",
                        "status": {"type": "idle", "activeFlags": []},
                    },
                    "activeTurns": {"count": 1},
                }),
            ),
            (
                "latestTurnStatus non-string",
                serde_json::json!({
                    "thread": {
                        "id": "thread-patrol-strict",
                        "status": {"type": "idle", "activeFlags": []},
                        "latestTurnStatus": {"type": "inProgress"},
                    },
                    "activeTurns": [],
                }),
            ),
        ] {
            server.appserver_thread_status = std::sync::Arc::new(move |_, _| Ok(raw.clone()));
            let actual = super::current_board_patrol_activity(&server, &worker).unwrap();
            assert_eq!(actual, AgentActivity::Unknown, "{label}: got {actual:?}");
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn board_idle_patrol_activity_missing_transport_is_unknown() {
        use super::super::name_state::AgentActivity;
        let (server, root) = test_server();
        super::super::peer_tests::register_synthetic_peer(&server, "worker", "thread-none");
        let worker = {
            let mut state = server.state.lock().unwrap();
            state.workers.get_mut("worker").unwrap().transport = None;
            state.workers["worker"].clone()
        };
        assert_eq!(
            super::current_board_patrol_activity(&server, &worker).unwrap(),
            AgentActivity::Unknown,
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    fn seed_board_patrol_owner(
        server: &Server,
        id: &str,
        thread_id: &str,
    ) -> super::super::state::WorkerRec {
        let cwd = super::super::global_state::GlobalState::canonical_project_scope(&server.root)
            .unwrap()
            .as_str()
            .to_owned();
        let app = crate::identity::AppServerId::new(crate::identity::CLI_APP_SERVER_ID).unwrap();
        let transport = super::super::peer_tests::test_appserver_transport(thread_id);
        let response = super::super::peer_tests::seed_committed_owner(
            server,
            id,
            &format!("token-{id}"),
            &transport,
            &cwd,
            Some(app.clone()),
        );
        assert!(response.ok, "seed {id}: {response:?}");
        super::super::commit_current_thread_route_for_runtime(server, server, id, &cwd, Some(&app))
            .unwrap();
        super::super::restore_registered_peer_default_leases(server);
        server.state.lock().unwrap().workers[id].clone()
    }

    fn occupy_board_patrol_name(
        server: &Server,
        worker: &super::super::state::WorkerRec,
        name: &str,
    ) {
        let project_scope = super::super::global_state::GlobalState::canonical_project_scope(
            std::path::Path::new(&worker.cwd),
        )
        .unwrap()
        .as_str()
        .to_owned();
        let session_id = worker
            .transport
            .as_ref()
            .and_then(|transport| transport.session_id.clone())
            .expect("seeded appserver transport has a session id");
        let revision = {
            let state = server.state.lock().unwrap();
            state.name_occupancy_revision(&project_scope, name) + 1
        };
        server.commit(&[Event::NameResolutionCommitted {
            project_scope,
            occupancy: super::super::name_state::NameOccupancy {
                name: name.to_owned(),
                worker_id: worker.id.clone(),
                session_id,
                occupancy_revision: revision,
                updated_ms: 1,
            },
            assignments: vec![],
        }]);
    }

    fn add_board_patrol_task(
        server: &Server,
        task_id: &str,
        owner_id: &str,
        name: &str,
        session_id: &str,
        now: i64,
    ) {
        let mut state = server.state.lock().unwrap();
        state
            .tasks
            .insert(task_id.to_owned(), task(task_id, owner_id, "working", now));
        state.task_assignments.insert(
            task_id.to_owned(),
            super::super::name_state::TaskAssignment {
                task_id: task_id.to_owned(),
                name: name.to_owned(),
                session_id: session_id.to_owned(),
                assignment_revision: 1,
                reflow: None,
                updated_ms: now,
            },
        );
    }

    fn seed_board_patrol_trusted_gate(
        server: &Server,
        worker_id: &str,
        owner_key: &str,
        tasks: &[&str],
        now: i64,
    ) {
        // The seeded episode models a poll this process already made, so the
        // restart guard must not discard it.
        super::super::observation_epoch::first_observation_since_start(
            &server.root,
            &format!("board-patrol:{worker_id}"),
        );
        server.commit(&[Event::BoardPatrolGateUpdated {
            worker_id: worker_id.into(),
            gate: crate::board_patrol_policy::PeerBoardPatrolGate {
                owner_key: owner_key.into(),
                last_wake_ms: 0,
                last_realtime_idle_since_ms: i64::MIN,
                last_wake_tasks: tasks.iter().map(|task| (*task).to_owned()).collect(),
                next_due_ms: now,
                idle_episode_start_ms: Some(now - 1_800_000 - 1),
                last_poll_idle: true,
            },
        }]);
    }

    fn install_appserver_statuses(
        server: &mut Server,
        statuses: Vec<(String, String)>,
    ) -> std::sync::Arc<std::sync::atomic::AtomicUsize> {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc;
        let calls = Arc::new(AtomicUsize::new(0));
        let counter = calls.clone();
        server.appserver_thread_status = Arc::new(move |_, thread| {
            counter.fetch_add(1, Ordering::SeqCst);
            let status = statuses
                .iter()
                .find(|(id, _)| id == thread)
                .map(|(_, status)| status.clone())
                .unwrap_or_else(|| "idle".into());
            Ok(serde_json::json!({
                "thread": {"id": thread, "status": {"type": status}},
            }))
        });
        calls
    }

    #[test]
    fn board_idle_patrol_first_poll_after_restart_starts_a_fresh_episode() {
        let (mut server, root) = test_server();
        let worker = seed_board_patrol_owner(&server, "alice", "thread-alice");
        occupy_board_patrol_name(&server, &worker, "Alice");
        add_board_patrol_task(
            &server,
            "T1",
            "alice",
            "Alice",
            worker
                .transport
                .as_ref()
                .unwrap()
                .session_id
                .as_deref()
                .unwrap(),
            1,
        );
        install_appserver_statuses(&mut server, vec![("thread-alice".into(), "idle".into())]);
        let now = super::super::state::now_ms();
        // A legacy gate persisted before this daemon started has no typed
        // episode field. It must not be backfilled from an older snapshot;
        // the first post-restart poll starts a fresh trusted interval.
        server.commit(&[Event::BoardPatrolGateUpdated {
            worker_id: "alice".into(),
            gate: crate::board_patrol_policy::PeerBoardPatrolGate {
                owner_key: "Alice".into(),
                last_wake_ms: 0,
                last_realtime_idle_since_ms: i64::MIN,
                last_wake_tasks: vec!["T1".into()],
                next_due_ms: now,
                idle_episode_start_ms: None,
                last_poll_idle: false,
            },
        }]);
        tick_at(&server, now);
        {
            let state = server.state.lock().unwrap();
            assert!(
                state.board_patrol_notice_ids.is_empty(),
                "a pre-restart episode must not fire on the first poll"
            );
            let gate = &state.board_patrol_gates["alice"];
            assert_eq!(
                gate.idle_episode_start_ms,
                Some(now),
                "the first post-restart poll starts a fresh trusted episode"
            );
            assert!(gate.last_poll_idle);
        }
        tick_at(&server, now + 1_800_001);
        assert_eq!(
            server.state.lock().unwrap().board_patrol_notice_ids.len(),
            1,
            "the fresh episode becomes due only after its own 30 minutes"
        );
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn board_idle_patrol_dispatch_creates_fresh_periodic_notice_without_retrying_old() {
        let (mut server, root) = test_server();
        let worker = seed_board_patrol_owner(&server, "alice", "thread-alice");
        occupy_board_patrol_name(&server, &worker, "Alice");
        add_board_patrol_task(
            &server,
            "T1",
            "alice",
            "Alice",
            worker
                .transport
                .as_ref()
                .unwrap()
                .session_id
                .as_deref()
                .unwrap(),
            1,
        );
        install_appserver_statuses(&mut server, vec![("thread-alice".into(), "idle".into())]);
        let now = super::super::state::now_ms();
        seed_board_patrol_trusted_gate(&server, "alice", "Alice", &["T1"], now);
        tick_at(&server, now);

        let (first_id, first_state, first_wake_attempts) = {
            let state = server.state.lock().unwrap();
            let ids: Vec<_> = state.board_patrol_notice_ids.iter().cloned().collect();
            assert_eq!(ids.len(), 1, "one due occurrence: {ids:?}");
            let id = ids[0].clone();
            let message = &state.msgs[&id];
            assert_eq!(message.to, "alice");
            assert_eq!(message.mtype, "notify");
            assert_eq!(message.subject.as_deref(), Some("board-idle-patrol"));
            assert!(message.body.contains("collab board show"));
            assert!(message.body.contains("collab notify cancel --task-id"));
            assert!(message
                .body
                .contains("collab notify subscribe --event board-idle"));
            assert_eq!(state.delivery_modes[&id], "explicit-notification");
            assert_eq!(
                state.wake_bindings.get(&id),
                Some(&super::super::default_direct_message_id("alice"))
            );
            assert!(state.notification_delivery_accepted.contains_key(&id));
            assert_eq!(
                state.board_patrol_gates["alice"].next_due_ms,
                now + crate::board_patrol_policy::BOARD_IDLE_PATROL_INTERVAL_MS
            );
            (id, message.state.clone(), message.wake_attempt_count)
        };

        tick_at(
            &server,
            now + 2 * crate::board_patrol_policy::BOARD_IDLE_PATROL_INTERVAL_MS,
        );
        let state = server.state.lock().unwrap();
        assert_eq!(state.board_patrol_notice_ids.len(), 2);
        let second_id = state
            .board_patrol_notice_ids
            .iter()
            .find(|id| **id != first_id)
            .expect("later occurrence has a new message id")
            .clone();
        assert_ne!(second_id, first_id);
        assert!(state
            .notification_delivery_accepted
            .contains_key(&second_id));
        let old = &state.msgs[&first_id];
        assert_eq!(old.state, first_state, "old unconsumed notice is unchanged");
        assert_eq!(old.wake_attempt_count, first_wake_attempts);
        assert_eq!(old.wake_attempt_count, 1, "old notice is not retried");
        drop(state);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn board_idle_patrol_dispatch_busy_skips_and_advances_without_early_probe() {
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::Arc;
        let (mut server, root) = test_server();
        let worker = seed_board_patrol_owner(&server, "alice", "thread-alice");
        occupy_board_patrol_name(&server, &worker, "Alice");
        add_board_patrol_task(
            &server,
            "T1",
            "alice",
            "Alice",
            worker
                .transport
                .as_ref()
                .unwrap()
                .session_id
                .as_deref()
                .unwrap(),
            1,
        );
        let busy = Arc::new(AtomicBool::new(true));
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        server.appserver_thread_status = Arc::new({
            let busy = busy.clone();
            let calls = calls.clone();
            move |_, thread| {
                calls.fetch_add(1, Ordering::SeqCst);
                Ok(serde_json::json!({
                    "thread": {
                        "id": thread,
                        "status": {"type": if busy.load(Ordering::SeqCst) { "active" } else { "idle" }},
                    },
                }))
            }
        });
        let now = super::super::state::now_ms();
        seed_board_patrol_trusted_gate(&server, "alice", "Alice", &["T1"], now);
        tick_at(&server, now);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        {
            let state = server.state.lock().unwrap();
            assert!(state.board_patrol_notice_ids.is_empty());
            assert_eq!(
                state.board_patrol_gates["alice"].next_due_ms,
                now + crate::board_patrol_policy::BOARD_IDLE_PATROL_INTERVAL_MS
            );
        }

        busy.store(false, Ordering::SeqCst);
        calls.store(0, Ordering::SeqCst);
        tick_at(&server, now + 1);
        assert_eq!(calls.load(Ordering::SeqCst), 0, "not due means no RPC");
        {
            let state = server.state.lock().unwrap();
            assert!(state.board_patrol_notice_ids.is_empty());
        }

        tick_at(
            &server,
            // The busy skip cleared the trusted episode, so this idle read
            // starts a fresh interval.
            now + 4 * crate::board_patrol_policy::BOARD_IDLE_PATROL_INTERVAL_MS + 2,
        );
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert!(server
            .state
            .lock()
            .unwrap()
            .board_patrol_notice_ids
            .is_empty());
        tick_at(
            &server,
            now + 6 * crate::board_patrol_policy::BOARD_IDLE_PATROL_INTERVAL_MS + 3,
        );
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert_eq!(
            server.state.lock().unwrap().board_patrol_notice_ids.len(),
            1
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn board_idle_patrol_dispatch_read_error_skips_and_advances_without_early_probe() {
        use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
        use std::sync::Arc;
        let (mut server, root) = test_server();
        let worker = seed_board_patrol_owner(&server, "alice", "thread-alice");
        occupy_board_patrol_name(&server, &worker, "Alice");
        add_board_patrol_task(
            &server,
            "T1",
            "alice",
            "Alice",
            worker
                .transport
                .as_ref()
                .unwrap()
                .session_id
                .as_deref()
                .unwrap(),
            1,
        );
        let failing = Arc::new(AtomicBool::new(true));
        let calls = Arc::new(AtomicUsize::new(0));
        server.appserver_thread_status = Arc::new({
            let failing = failing.clone();
            let calls = calls.clone();
            move |_, thread| {
                calls.fetch_add(1, Ordering::SeqCst);
                if failing.load(Ordering::SeqCst) {
                    return Err("native thread read failed".to_owned());
                }
                Ok(serde_json::json!({
                    "thread": {"id": thread, "status": {"type": "idle"}},
                }))
            }
        });
        let now = super::super::state::now_ms();
        seed_board_patrol_trusted_gate(&server, "alice", "Alice", &["T1"], now);
        tick_at(&server, now);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        {
            let state = server.state.lock().unwrap();
            assert!(state.board_patrol_notice_ids.is_empty());
            assert_eq!(
                state
                    .board_patrol_gates
                    .get("alice")
                    .expect("failed read still advances the gate")
                    .next_due_ms,
                now + crate::board_patrol_policy::BOARD_IDLE_PATROL_INTERVAL_MS
            );
        }
        let log = std::fs::read_to_string(server.log_path()).unwrap_or_default();
        assert!(
            log.contains("BOARD_PATROL_ACTIVITY_READ_FAILED: alice: native thread read failed"),
            "failed activity read is logged explicitly: {log}"
        );

        failing.store(false, Ordering::SeqCst);
        calls.store(0, Ordering::SeqCst);
        tick_at(&server, now + 1);
        assert_eq!(calls.load(Ordering::SeqCst), 0, "not due means no RPC");
        {
            let state = server.state.lock().unwrap();
            assert!(state.board_patrol_notice_ids.is_empty());
        }

        tick_at(
            &server,
            now + 4 * crate::board_patrol_policy::BOARD_IDLE_PATROL_INTERVAL_MS + 2,
        );
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert!(server
            .state
            .lock()
            .unwrap()
            .board_patrol_notice_ids
            .is_empty());
        tick_at(
            &server,
            now + 6 * crate::board_patrol_policy::BOARD_IDLE_PATROL_INTERVAL_MS + 3,
        );
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert_eq!(
            server.state.lock().unwrap().board_patrol_notice_ids.len(),
            1
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn board_idle_patrol_dispatch_cancel_before_attempt_blocks_delivery() {
        let (mut server, root) = test_server();
        let worker = seed_board_patrol_owner(&server, "alice", "thread-alice");
        occupy_board_patrol_name(&server, &worker, "Alice");
        add_board_patrol_task(
            &server,
            "T1",
            "alice",
            "Alice",
            worker
                .transport
                .as_ref()
                .unwrap()
                .session_id
                .as_deref()
                .unwrap(),
            1,
        );
        install_appserver_statuses(&mut server, vec![("thread-alice".into(), "idle".into())]);
        let now = super::super::state::now_ms();
        let (message_id, subscription_id, events) = {
            let state = server.state.lock().unwrap();
            let gate = crate::board_patrol_policy::PeerBoardPatrolGate {
                owner_key: "Alice".into(),
                last_wake_ms: now,
                last_realtime_idle_since_ms: now,
                last_wake_tasks: vec!["T1".into()],
                next_due_ms: now + crate::board_patrol_policy::BOARD_IDLE_PATROL_INTERVAL_MS,
                idle_episode_start_ms: Some(now),
                last_poll_idle: true,
            };
            let address = super::board_patrol_name_address(&state, &worker, "Alice");
            super::board_patrol_wake_events(
                &state,
                &worker,
                address.as_ref(),
                &["T1".to_owned()],
                gate,
                now,
            )
            .expect("default direct-message lease exists")
        };
        let kinds: Vec<String> = events
            .iter()
            .map(|event| {
                serde_json::to_value(event).unwrap()["ev"]
                    .as_str()
                    .unwrap()
                    .to_owned()
            })
            .collect();
        assert_eq!(
            kinds,
            [
                "BoardPatrolGateUpdated",
                "DeliveryMode",
                "BoardPatrolNoticeBound",
                "Sent",
                "MailboxNameAddressed",
                "WakeBound",
            ]
        );
        server.commit(&events);
        server.commit(&[Event::BoardPatrolCancelled {
            record: crate::board_patrol_policy::TaskPatrolCancelRecord {
                task_id: "T1".into(),
                owner_key: "Alice".into(),
                reason: Some("waiting on the user".into()),
                resume_condition: Some("user answers".into()),
                cancelled_ms: now + 1,
            },
        }]);
        let attempt = super::super::attempt_notification_detailed_with_at(
            &server,
            &message_id,
            &subscription_id,
            now,
        );
        assert!(
            !attempt.accepted(),
            "cancelled patrol must not be sent: {attempt:?}"
        );
        let state = server.state.lock().unwrap();
        assert!(!state
            .notification_delivery_accepted
            .contains_key(&message_id));
        assert_eq!(state.msgs[&message_id].wake_attempt_count, 0);
        drop(state);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn board_idle_patrol_dispatch_only_current_carrier_wakes() {
        let (mut server, root) = test_server();
        let old = seed_board_patrol_owner(&server, "old", "thread-old");
        let current = seed_board_patrol_owner(&server, "current", "thread-current");
        occupy_board_patrol_name(&server, &current, "Alice");
        let current_name_occupancies: Vec<_> = {
            let state = server.state.lock().unwrap();
            state
                .name_occupancies
                .values()
                .flat_map(|names| names.values())
                .filter(|occupancy| occupancy.worker_id == current.id)
                .cloned()
                .collect()
        };
        assert_eq!(
            current_name_occupancies.len(),
            1,
            "fixture invariant: current worker must hold exactly one name across alias scopes, got {current_name_occupancies:?}"
        );
        {
            let mut state = server.state.lock().unwrap();
            state.worker_names.insert("old".into(), "Alice".into());
        }
        add_board_patrol_task(
            &server,
            "T1",
            "old",
            "Alice",
            current
                .transport
                .as_ref()
                .unwrap()
                .session_id
                .as_deref()
                .unwrap(),
            1,
        );
        seed_board_patrol_trusted_gate(
            &server,
            "current",
            "Alice",
            &["T1"],
            super::super::state::now_ms(),
        );
        install_appserver_statuses(
            &mut server,
            vec![
                ("thread-old".into(), "idle".into()),
                ("thread-current".into(), "idle".into()),
            ],
        );
        tick_at(&server, super::super::state::now_ms());
        let state = server.state.lock().unwrap();
        assert!(super::super::state::board_patrol_current_owner_key(&state, "old").is_none());
        assert_eq!(state.board_patrol_notice_ids.len(), 1);
        let message_id = state.board_patrol_notice_ids.iter().next().unwrap();
        assert_eq!(state.msgs[message_id].to, "current");
        assert!(!state.msgs.values().any(|message| message.to == "old"));
        assert!(!state.board_patrol_gates.contains_key("old"));
        assert!(state.board_patrol_gates.contains_key("current"));
        drop(state);
        drop(old);
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[cfg(test)]
#[path = "reply_idle_tests.rs"]
mod reply_idle_tests;
