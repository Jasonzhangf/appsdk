//! Rex daemon monitor.
//!
//! One official `events` stream follows each current Rex binding. The daemon
//! keeps only cancellation/task resources; identity and activity stay in the
//! existing `AgentPresenceObserved` / `AgentActivityObserved` owner.

use super::name_state::{AgentActivity, AgentActivityRecord, AgentPresence, AgentPresenceRecord};
use super::state::{now_ms, Event};
use super::{GlobalState, Server};
use crate::client::adapters::rex::{
    observe_current, spawn_events, RexEvent, RexEventBatch, RexObservation, RexProgramStatusOwner,
    RexProgramStatusRecord, TERMINAL_CREATOR,
};
use crate::proto::{RexEndpoint, TransportKind};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;

const PROCESS_SOURCE: &str = "com.superlogical.terminal.process";
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

struct RexProjection {
    presence: AgentPresence,
    activity: AgentActivity,
    reason: String,
}

/// The latest observed Codex process for this exact execution endpoint.
/// It supplies process-exit evidence after daemon restart and grants no identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ObservedRexExecution {
    pub(crate) endpoint: RexEndpoint,
    pub(crate) agent_session_id: String,
    pub(crate) owner: RexProgramStatusOwner,
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
    let mut execution = server
        .state
        .lock()
        .unwrap()
        .rex_executions
        .get(&worker_id)
        .filter(|record| {
            record.endpoint == target.endpoint && record.agent_session_id == target.agent_session_id
        })
        .map(|record| record.owner.clone());
    refresh_snapshot(&server, &worker_id, &target, &mut execution).await;
    loop {
        tokio::select! {
            event = events.recv() => match event {
                Some(event) if event_matches_target(&event, &target) => {
                    refresh_snapshot(&server, &worker_id, &target, &mut execution).await;
                }
                Some(_) => {}
                None => break,
            },
            finished = &mut stream => {
                if let Some(projection) = project_stream_join(finished) {
                    submit_activity(&server, &worker_id, &target, projection);
                }
                return;
            }
        }
    }
    if let Ok(batch) = stream.await {
        if let Some(projection) = project_stream_end(&batch) {
            submit_activity(&server, &worker_id, &target, projection);
        }
    }
}

async fn refresh_snapshot(
    server: &Arc<Server>,
    worker_id: &str,
    target: &MonitorTarget,
    execution: &mut Option<RexProgramStatusOwner>,
) {
    let projection = match observe_current(&target.endpoint).await {
        Ok(observation) => project_observation_with_execution(&observation, execution, &|pid| {
            current_process_exists(&target.endpoint, pid)
        }),
        Err(error) => RexProjection {
            presence: AgentPresence::Unknown,
            activity: AgentActivity::Unknown,
            reason: format!("rex.osc7501 observe failed: {error}"),
        },
    };
    submit_activity_with_execution(server, worker_id, target, projection, execution.as_ref());
}

/// A local PID probe belongs only to the Rex server that owns that PID.
/// Unsupported remote targets and permission failures remain explicit unknowns.
fn current_process_exists(endpoint: &RexEndpoint, pid: u64) -> Result<bool, String> {
    use std::os::unix::fs::FileTypeExt;
    let socket = endpoint
        .server
        .strip_prefix("unix://")
        .ok_or("local process probe unsupported for this Rex server")?;
    if !std::fs::metadata(socket)
        .map_err(|error| error.to_string())?
        .file_type()
        .is_socket()
    {
        return Err("local process probe requires the current Rex Unix socket".into());
    }
    let pid = libc::pid_t::try_from(pid)
        .ok()
        .filter(|pid| *pid > 0)
        .ok_or("invalid observed Rex process PID")?;
    // Signal zero observes existence without sending a signal to the process.
    if unsafe { libc::kill(pid, 0) } == 0 {
        return Ok(true);
    }
    let error = std::io::Error::last_os_error();
    match error.raw_os_error() {
        Some(libc::ESRCH) => Ok(false),
        Some(libc::EPERM) => Ok(true),
        _ => Err(error.to_string()),
    }
}

fn project_observation_with_execution(
    observation: &RexObservation,
    execution: &mut Option<RexProgramStatusOwner>,
    process_exists: &dyn Fn(u64) -> Result<bool, String>,
) -> RexProjection {
    let mut projection = project_observation(observation);
    if projection.presence == AgentPresence::Online {
        if let Some(process) = observation.process.as_ref() {
            *execution =
                process
                    .pid
                    .zip(process.name.as_ref())
                    .map(|(pid, name)| RexProgramStatusOwner {
                        pid,
                        name: name.clone(),
                    });
        }
        return projection;
    }
    let Some(previous) = execution.as_ref() else {
        return projection;
    };
    let Some(process) = observation.process.as_ref() else {
        return projection;
    };
    let foreground_is_shell = matches!(
        process.name.as_deref(),
        Some("sh" | "bash" | "zsh" | "fish" | "dash" | "ksh" | "nu")
    );
    if !foreground_is_shell
        || process.pid.is_none()
        || process.pid == Some(previous.pid)
        || observation.gap_possible
        || !observation.source_errors.is_empty()
        || !observation.program_status.is_empty()
        || ![PROCESS_SOURCE, PROGRAM_STATUS_SOURCE]
            .iter()
            .all(|source| observation.sources.iter().any(|present| present == source))
    {
        return projection;
    }
    match process_exists(previous.pid) {
        Ok(false) => {
            projection.presence = AgentPresence::Offline;
            projection.activity = AgentActivity::Unknown;
            projection.reason = format!("rex current Codex process exited pid={} name={}; refreshed block foreground pid={} name={}",
                previous.pid, previous.name, process.pid.unwrap(), process.name.as_deref().unwrap());
        }
        Ok(true) => {}
        Err(error) => {
            projection.reason = format!("{}; precise exit unavailable: {error}", projection.reason)
        }
    }
    projection
}

fn project_observation(observation: &RexObservation) -> RexProjection {
    let missing_sources = [PROCESS_SOURCE, PROGRAM_STATUS_SOURCE]
        .into_iter()
        .filter(|source| !observation.sources.iter().any(|present| present == source))
        .collect::<Vec<_>>();
    if !missing_sources.is_empty() {
        let detail = observation.source_errors.join("; ");
        return RexProjection {
            presence: AgentPresence::Unknown,
            activity: AgentActivity::Unknown,
            reason: format!(
                "rex.osc7501 query incomplete missing_sources={} gap_possible={} source_errors={detail}",
                missing_sources.join(","),
                observation.gap_possible
            ),
        };
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
        return RexProjection {
            presence: AgentPresence::Unknown,
            activity: AgentActivity::Unknown,
            reason: format!(
                "rex.osc7501 foreground process identity unavailable source={PROGRAM_STATUS_SOURCE}"
            ),
        };
    };
    let current: Vec<&RexProgramStatusRecord> = observation
        .program_status
        .iter()
        .filter(|record| {
            record.app.as_deref() == Some("codex")
                && record.owner.as_ref().is_some_and(|owner| {
                    owner.pid == foreground_pid && owner.name == foreground_name
                })
        })
        .collect();
    if current.is_empty() {
        return RexProjection {
            presence: AgentPresence::Unknown,
            activity: AgentActivity::Unknown,
            reason: format!(
                "rex.osc7501 no current-owner program_status records total={} foreground_pid={foreground_pid} foreground_name={foreground_name} source={PROGRAM_STATUS_SOURCE}",
                observation.program_status.len()
            ),
        };
    }
    let mut activity = None;
    let mut conflict = false;
    for record in current {
        let next = match record.state.as_deref().map(str::trim) {
            Some("working") => Some(AgentActivity::Busy),
            Some("blocked") => Some(AgentActivity::WaitingUser),
            Some("idle" | "done") => Some(AgentActivity::Idle),
            Some("error") => Some(AgentActivity::Error),
            _ => None,
        };
        match (activity, next) {
            (None, Some(next)) => activity = Some(next),
            (Some(current), Some(next)) if current != next => conflict = true,
            (Some(_), Some(_)) => {}
            (_, None) => conflict = true,
        }
    }
    let activity = if conflict {
        AgentActivity::Unknown
    } else {
        activity.unwrap_or(AgentActivity::Unknown)
    };
    let source_errors = if observation.source_errors.is_empty() {
        String::new()
    } else {
        format!(" source_errors={}", observation.source_errors.join("; "))
    };
    let reason = format!(
        "rex.osc7501 projection={} records={} foreground_pid={foreground_pid} foreground_name={foreground_name} source={PROGRAM_STATUS_SOURCE}{source_errors}",
        activity_label(activity),
        observation.program_status.len()
    );
    RexProjection {
        presence: AgentPresence::Online,
        activity,
        reason,
    }
}

fn project_stream_join(
    finished: Result<RexEventBatch, tokio::task::JoinError>,
) -> Option<RexProjection> {
    match finished {
        Ok(batch) => project_stream_end(&batch),
        Err(error) => Some(RexProjection {
            presence: AgentPresence::Unknown,
            activity: AgentActivity::Unknown,
            reason: format!("rex.events task failed: {error}"),
        }),
    }
}

fn project_stream_end(batch: &RexEventBatch) -> Option<RexProjection> {
    if !batch.gap_possible && batch.detail.is_none() {
        return None;
    }
    let detail = batch
        .detail
        .as_deref()
        .unwrap_or("events stream gap without detail");
    Some(RexProjection {
        presence: AgentPresence::Unknown,
        activity: AgentActivity::Unknown,
        reason: format!("rex.events disconnected gap_possible=true detail={detail}"),
    })
}

fn presence_label(presence: AgentPresence) -> &'static str {
    match presence {
        AgentPresence::Online => "online",
        AgentPresence::Offline => "offline",
        AgentPresence::Unknown => "unknown",
    }
}

fn activity_label(activity: AgentActivity) -> &'static str {
    match activity {
        AgentActivity::Busy => "busy",
        AgentActivity::Idle => "idle",
        AgentActivity::WaitingUser => "waiting_user",
        AgentActivity::Error => "error",
        AgentActivity::Unknown => "unknown",
    }
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
    projection: RexProjection,
) {
    submit_activity_with_execution(server, worker_id, target, projection, None);
}

fn submit_activity_with_execution(
    server: &Server,
    worker_id: &str,
    target: &MonitorTarget,
    projection: RexProjection,
    execution: Option<&RexProgramStatusOwner>,
) {
    let RexProjection {
        presence,
        activity,
        reason,
    } = projection;
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
        presence,
        reason: format!(
            "rex.osc7501 presence={} records_from_activity_projection={reason}",
            presence_label(presence)
        ),
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
    if let Some(owner) = execution {
        let record = ObservedRexExecution {
            endpoint: target.endpoint.clone(),
            agent_session_id: target.agent_session_id.clone(),
            owner: owner.clone(),
        };
        if state.rex_executions.get(worker_id) != Some(&record) {
            events.push(Event::RexExecutionObserved {
                worker_id: worker_id.into(),
                record,
            });
        }
    }
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
