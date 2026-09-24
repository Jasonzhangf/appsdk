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
}

/// A starting agent legitimately flaps between idle and working while it boots
/// and renders. Reporting each flap floods the master, so a state must hold
/// before it is worth one scheduling notification.
const SUBAGENT_STATE_SETTLE_MS: i64 = 60_000;

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
    !matches!(status, "closed" | "cancelled")
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
        super::live_master_id(server, &state).ok().flatten()
    };
    for worker in workers {
        // App Server transport exposes thread liveness, not the agent's
        // execution state. Managed children report that state through their
        // typed lifecycle; ordinary peers are left unknown until they do.
        let agent = {
            let state = server.state.lock().unwrap();
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
        };
        if agent == AgentState::Unknown {
            continue;
        }

        let mut state = server.state.lock().unwrap();
        if state.admission_frozen() {
            return;
        }
        if !state.workers.contains_key(&worker.id) {
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
                server.commit_locked(&mut state, &events);
            }
        }
    }
    flush_idle_batches_if_ready(server, now);
}

fn flush_idle_batches_if_ready(server: &Server, now: i64) {
    if !server.config.notifications.enabled {
        return;
    }
    let mut state = server.state.lock().unwrap();
    if state.admission_frozen() {
        return;
    }
    let Some(master_id) = super::live_master_id(server, &state).ok().flatten() else {
        return;
    };
    let master_record = state.keepalives.get(&master_id).cloned();
    if master_record
        .as_ref()
        .is_some_and(|record| record.observed == "working")
    {
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
    let subject = if newly_idle.len() == 1 && newly_idle[0].starts_with("subagent:") {
        "subagent-status".to_string()
    } else if newly_idle.len() == 1 {
        "worker-idle".to_string()
    } else {
        "master-wake-batch".to_string()
    };
    let body = format!(
        "newly_idle={} live_idle={} blocked={} goal_due={}. Scheduling continues: saturate live present peers first, then schedule managed subagents within the configured cap. Never stay idle while eligible capacity remains. Inspect the task graph, every live peer and managed subagent, liveness, saturation, and blockers; dispatch the next ready non-overlapping P0/P1 task to each idle eligible worker, or resolve and reassign blockers. Cancel only iff no actionable task, dependency, resolvable blocker, or authorized open bug remains, using collab notify unsubscribe {} and record the receipt. If the goal is complete, report the evidence to the user.",
        newly_idle.join(","),
        live_idle.join(","),
        blocked.join(","),
        state.master_wake.goal_due,
        subscription.id
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
    server.commit_locked(&mut state, &events);
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
}
