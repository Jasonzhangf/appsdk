//! Rex daemon monitor.
//!
//! One official `events` stream follows each current Rex binding. The daemon
//! keeps only cancellation/task resources; identity and activity stay in the
//! existing `AgentPresenceObserved` / `AgentActivityObserved` owner.

use super::name_state::{AgentActivity, AgentActivityRecord, AgentPresence, AgentPresenceRecord};
use super::state::{now_ms, Event};
use super::{GlobalState, Server};
use crate::client::adapters::rex::{
    observe_current, spawn_events, RexEvent, RexEventBatch, RexObservation, RexProgramStatusRecord,
    TERMINAL_CREATOR,
};
use crate::proto::{RexEndpoint, TransportKind};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;

const PROGRAM_STATUS_SOURCE: &str = "com.superlogical.terminal.program_status";
const RELEVANT_EVENTS: [&str; 4] = [
    "program_status_changed",
    "program_status_removed",
    "process_changed",
    "child_exited",
];
const RETIRE_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Default)]
pub struct RexMonitor {
    inner: Mutex<RexMonitorInner>,
}

#[derive(Default)]
struct RexMonitorInner {
    handle: Option<tokio::runtime::Handle>,
    tasks: HashMap<String, RexTask>,
}

#[derive(Clone, PartialEq, Eq)]
struct MonitorTarget {
    endpoint: RexEndpoint,
    agent_session_id: String,
}

struct RexTask {
    target: MonitorTarget,
    cancel: watch::Sender<bool>,
    task: JoinHandle<()>,
}

impl RexMonitor {
    /// Bind the monitor to the daemon runtime once. The timer path runs on a
    /// blocking thread, so task creation uses this stored handle.
    pub(crate) fn install_runtime_handle(&self, handle: tokio::runtime::Handle) {
        self.inner.lock().unwrap().handle = Some(handle);
    }

    pub(crate) fn runtime_handle(&self) -> Option<tokio::runtime::Handle> {
        self.inner.lock().unwrap().handle.clone()
    }

    /// Stop each owned stream. Cancellation is sent to the adapter watch;
    /// the adapter kills and reaps its own events CLI child.
    pub(crate) async fn shutdown(&self) {
        let tasks: Vec<RexTask> = {
            let mut inner = self.inner.lock().unwrap();
            inner.tasks.drain().map(|(_, task)| task).collect()
        };
        for task in tasks {
            let RexTask {
                cancel, mut task, ..
            } = task;
            let _ = cancel.send(true);
            if tokio::time::timeout(RETIRE_TIMEOUT, &mut task)
                .await
                .is_err()
            {
                task.abort();
            }
        }
    }
}

/// Reconcile the monitor resource table with the current Rex bindings. This is
/// called from the existing maintenance cadence; it does not create a poller.
pub(crate) fn reconcile(server: &Arc<Server>, _now: i64) {
    let desired: HashMap<String, MonitorTarget> = {
        let state = server.state.lock().unwrap();
        state
            .workers
            .values()
            .filter_map(|worker| {
                current_target(&state, &worker.id).map(|target| (worker.id.clone(), target))
            })
            .collect()
    };
    let mut inner = server.rex_monitor.inner.lock().unwrap();
    let Some(handle) = inner.handle.clone() else {
        return;
    };
    let retired = reconcile_table(&mut inner.tasks, desired, |worker_id, target| {
        spawn_task(server, &handle, worker_id, target)
    });
    drop(inner);
    for task in retired {
        retire(&handle, task);
    }
}

fn reconcile_table(
    table: &mut HashMap<String, RexTask>,
    mut desired: HashMap<String, MonitorTarget>,
    mut spawn: impl FnMut(String, MonitorTarget) -> RexTask,
) -> Vec<RexTask> {
    let mut retired = Vec::new();
    let current: Vec<String> = table.keys().cloned().collect();
    for worker_id in current {
        let same = desired
            .get(&worker_id)
            .is_some_and(|target| same_binding(&table[&worker_id].target, target));
        let finished = table[&worker_id].task.is_finished();
        if !same || finished {
            if let Some(task) = table.remove(&worker_id) {
                retired.push(task);
            }
        }
    }
    for (worker_id, target) in desired.drain() {
        if table.contains_key(&worker_id) {
            continue;
        }
        table.insert(worker_id.clone(), spawn(worker_id, target));
    }
    retired
}

fn spawn_task(
    server: &Arc<Server>,
    handle: &tokio::runtime::Handle,
    worker_id: String,
    target: MonitorTarget,
) -> RexTask {
    let (events_tx, events_rx) = mpsc::channel(8);
    let endpoint = target.endpoint.clone();
    let stream = handle.block_on(async move { spawn_events(endpoint, events_tx) });
    let task_target = target.clone();
    let server = Arc::clone(server);
    let task = handle.spawn(async move {
        run_stream(server, worker_id, task_target, events_rx, stream.task).await;
    });
    RexTask {
        target,
        cancel: stream.cancel,
        task,
    }
}

async fn run_stream(
    server: Arc<Server>,
    worker_id: String,
    target: MonitorTarget,
    mut events: mpsc::Receiver<RexEvent>,
    mut stream: JoinHandle<RexEventBatch>,
) {
    refresh_snapshot(&server, &worker_id, &target).await;
    loop {
        tokio::select! {
            event = events.recv() => match event {
                Some(event) if event_matches_target(&event, &target) => {
                    refresh_snapshot(&server, &worker_id, &target).await;
                }
                Some(_) => {}
                None => break,
            },
            finished = &mut stream => {
                if let Some((activity, reason)) = project_stream_join(finished) {
                    submit_activity(&server, &worker_id, &target, activity, reason);
                }
                return;
            }
        }
    }
    if let Ok(batch) = stream.await {
        if let Some((activity, reason)) = project_stream_end(&batch) {
            submit_activity(&server, &worker_id, &target, activity, reason);
        }
    }
}

async fn refresh_snapshot(server: &Arc<Server>, worker_id: &str, target: &MonitorTarget) {
    let (activity, reason) = match observe_current(&target.endpoint).await {
        Ok(observation) => project_observation(&observation),
        Err(error) => (
            AgentActivity::Unknown,
            format!("rex.osc7501 observe failed: {error}"),
        ),
    };
    submit_activity(server, worker_id, target, activity, reason);
}

fn project_observation(observation: &RexObservation) -> (AgentActivity, String) {
    if observation.gap_possible || !observation.source_errors.is_empty() {
        let detail = observation.source_errors.join("; ");
        return (
            AgentActivity::Unknown,
            format!("rex.osc7501 query incomplete gap_possible=true source_errors={detail}"),
        );
    }
    let Some((foreground_pid, foreground_name)) =
        observation.process.as_ref().and_then(|process| {
            let pid = process.pid.filter(|pid| *pid > 0)?;
            let name = process
                .name
                .as_deref()
                .filter(|name| !name.trim().is_empty())?;
            Some((pid, name))
        })
    else {
        return (
            AgentActivity::Unknown,
            format!(
                "rex.osc7501 foreground process identity unavailable source={PROGRAM_STATUS_SOURCE}"
            ),
        );
    };
    let current: Vec<&RexProgramStatusRecord> = observation
        .program_status
        .iter()
        .filter(|record| {
            record
                .owner
                .as_ref()
                .is_some_and(|owner| owner.pid == foreground_pid && owner.name == foreground_name)
        })
        .collect();
    if current.is_empty() {
        return (
            AgentActivity::Unknown,
            format!(
                "rex.osc7501 no current-owner program_status records total={} foreground_pid={foreground_pid} foreground_name={foreground_name} source={PROGRAM_STATUS_SOURCE}",
                observation.program_status.len()
            ),
        );
    }
    // Any blocked record means the foreground program is waiting for the user.
    // A stale working record from a previous foreground process is already
    // filtered above, so a current blocked/idle record is not overridden.
    let (activity, projection) = if current
        .iter()
        .any(|record| record.state.as_deref() == Some("blocked"))
    {
        (AgentActivity::WaitingUser, "waiting_user")
    } else if current
        .iter()
        .any(|record| record.state.as_deref() == Some("working"))
    {
        (AgentActivity::Busy, "busy")
    } else {
        // idle, done, error and any state Rex does not recognize all project
        // as non-working. Rex's API defines unknown states as idle.
        (AgentActivity::Idle, "idle")
    };
    let reason = format!(
        "rex.osc7501 projection={projection} records={} foreground_pid={foreground_pid} foreground_name={foreground_name} source={PROGRAM_STATUS_SOURCE}",
        current.len()
    );
    (activity, reason)
}

fn project_stream_join(
    finished: Result<RexEventBatch, tokio::task::JoinError>,
) -> Option<(AgentActivity, String)> {
    match finished {
        Ok(batch) => project_stream_end(&batch),
        Err(error) => Some((
            AgentActivity::Unknown,
            format!("rex.events task failed: {error}"),
        )),
    }
}

fn project_stream_end(batch: &RexEventBatch) -> Option<(AgentActivity, String)> {
    if !batch.gap_possible && batch.detail.is_none() {
        return None;
    }
    let detail = batch
        .detail
        .as_deref()
        .unwrap_or("events stream gap without detail");
    Some((
        AgentActivity::Unknown,
        format!("rex.events disconnected gap_possible=true detail={detail}"),
    ))
}

fn event_matches_target(event: &RexEvent, target: &MonitorTarget) -> bool {
    event.session_id == target.endpoint.session_id
        && event.block_id == target.endpoint.block_id
        && event.creator_name == TERMINAL_CREATOR
        && RELEVANT_EVENTS.contains(&event.name.as_str())
}

fn same_binding(left: &MonitorTarget, right: &MonitorTarget) -> bool {
    left == right
}

fn current_target(state: &super::state::State, worker_id: &str) -> Option<MonitorTarget> {
    let worker = state.workers.get(worker_id)?;
    let transport = worker.transport.as_ref()?;
    if transport.kind != TransportKind::Rex {
        return None;
    }
    let endpoint = transport.rex_endpoint.clone()?;
    let agent_session_id = transport
        .session_id
        .as_deref()
        .filter(|value| !value.trim().is_empty())?
        .to_owned();
    Some(MonitorTarget {
        endpoint,
        agent_session_id,
    })
}

fn submit_activity(
    server: &Server,
    worker_id: &str,
    target: &MonitorTarget,
    activity: AgentActivity,
    reason: String,
) {
    let Ok(scope) = GlobalState::canonical_project_scope(&server.root) else {
        super::presence::append_log(
            &server.log_path(),
            &format!("REX_MONITOR_SCOPE_UNKNOWN: {worker_id}"),
        );
        return;
    };
    let observed_ms = now_ms();
    let mut state = server.state.lock().unwrap();
    if current_target(&state, worker_id).as_ref() != Some(target) {
        return;
    }
    let presence = AgentPresenceRecord {
        project_scope: scope.as_str().into(),
        worker_id: worker_id.to_owned(),
        session_id: target.agent_session_id.clone(),
        presence: AgentPresence::Unknown,
        reason: format!("rex.osc7501; agent presence independent; {reason}"),
        observed_ms,
    };
    let activity = AgentActivityRecord {
        project_scope: scope.as_str().into(),
        worker_id: worker_id.to_owned(),
        session_id: target.agent_session_id.clone(),
        activity,
        background: false,
        reason,
        observed_ms,
        idle_since_ms: None,
    };
    let mut events = Vec::new();
    let old_presence = state
        .agent_presence
        .get(scope.as_str())
        .and_then(|workers| workers.get(worker_id));
    if old_presence.is_none_or(|old| {
        old.session_id != presence.session_id
            || old.presence != presence.presence
            || old.reason != presence.reason
    }) {
        events.push(Event::AgentPresenceObserved { presence });
    }
    let old_activity = state
        .agent_activity
        .get(scope.as_str())
        .and_then(|workers| workers.get(worker_id));
    if old_activity.is_none_or(|old| {
        old.session_id != activity.session_id
            || old.activity != activity.activity
            || old.background != activity.background
            || old.reason != activity.reason
            || old.idle_since_ms.is_some()
    }) {
        events.push(Event::AgentActivityObserved { activity });
    }
    if !events.is_empty() {
        server.commit_locked_reporting(&mut state, &events);
    }
}

fn retire(handle: &tokio::runtime::Handle, task: RexTask) {
    let RexTask {
        cancel, mut task, ..
    } = task;
    let _ = cancel.send(true);
    handle.spawn(async move {
        if tokio::time::timeout(RETIRE_TIMEOUT, &mut task)
            .await
            .is_err()
        {
            task.abort();
        }
    });
}

#[cfg(test)]
#[path = "rex_monitor_tests.rs"]
mod tests;
