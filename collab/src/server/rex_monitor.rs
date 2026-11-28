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
use std::sync::atomic::{AtomicBool, Ordering};
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
    agent_kind: Option<crate::host_runtime::AgentKind>,
}

impl MonitorTarget {
    /// `current_target(&state, worker_id)` omits `agent_kind` because host agent kind lives in
    /// `BindingControlStore`; matching endpoint and session keeps state-only checks aligned.
    fn matches_state_target(&self, other: &Self) -> bool {
        self.endpoint == other.endpoint && self.agent_session_id == other.agent_session_id
    }
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
    sampled: Arc<AtomicBool>,
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

    pub(super) fn has_current_sample(&self, state: &super::State, worker_id: &str) -> bool {
        let Some(target) = current_target(state, worker_id) else {
            return false;
        };
        self.inner
            .lock()
            .unwrap()
            .tasks
            .get(worker_id)
            .is_some_and(|task| {
                task.target.matches_state_target(&target)
                    && !task.task.is_finished()
                    && task.sampled.load(Ordering::Acquire)
            })
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
        let route = super::server_route_scope(server, &state).ok().flatten();
        let control =
            super::binding_store::BindingControlStore::new(server.host_paths.state_root())
                .load()
                .ok();
        state
            .workers
            .values()
            .filter_map(|worker| {
                let mut target = current_target(&state, &worker.id)?;
                if let (Some(route), Some(control)) = (route.as_ref(), control.as_ref()) {
                    target.agent_kind = control
                        .current_for_worker(
                            &route.project_scope_id,
                            &route.app_scope_id,
                            &worker.id,
                        )
                        .ok()
                        .flatten()
                        .and_then(|slot| slot.active_binding())
                        .and_then(|value| value.host.agent_kind.as_deref())
                        .and_then(crate::host_runtime::AgentKind::from_str);
                }
                Some((worker.id.clone(), target))
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
    let sampled = Arc::new(AtomicBool::new(false));
    let stream_sampled = Arc::clone(&sampled);
    let task = handle.spawn(async move {
        run_stream(
            server,
            worker_id,
            task_target,
            events_rx,
            stream.task,
            stream_sampled,
        )
        .await;
    });
    RexTask {
        target,
        sampled,
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
    sampled: Arc<AtomicBool>,
) {
    let mut execution = {
        let server = Arc::clone(&server);
        let worker_id = worker_id.clone();
        let target = target.clone();
        let Ok(previous) = tokio::task::spawn_blocking(move || {
            server
                .state
                .lock()
                .unwrap()
                .rex_executions
                .get(&worker_id)
                .filter(|record| {
                    record.endpoint == target.endpoint
                        && record.agent_session_id == target.agent_session_id
                })
                .map(|record| record.owner.clone())
        })
        .await
        else {
            return;
        };
        previous
    };
    if refresh_snapshot(&server, &worker_id, &target, &mut execution).await {
        sampled.store(true, Ordering::Release);
    }
    loop {
        tokio::select! {
            event = events.recv() => match event {
                Some(event) if event_matches_target(&event, &target) => {
                    if refresh_snapshot(&server, &worker_id, &target, &mut execution).await {
                        sampled.store(true, Ordering::Release);
                    }
                }
                Some(_) => {}
                None => break,
            },
            finished = &mut stream => {
                if let Some(projection) = project_stream_join(finished) {
                    publish_projection(&server, &worker_id, &target, projection, None).await;
                }
                return;
            }
        }
    }
    if let Ok(batch) = stream.await {
        if let Some(projection) = project_stream_end(&batch) {
            publish_projection(&server, &worker_id, &target, projection, None).await;
        }
    }
}

async fn refresh_snapshot(
    server: &Arc<Server>,
    worker_id: &str,
    target: &MonitorTarget,
    execution: &mut Option<RexProgramStatusOwner>,
) -> bool {
    let projection = match observe_current(&target.endpoint).await {
        Ok(observation) => project_observation_with_execution_for(
            &observation,
            target.agent_kind,
            execution,
            &|pid| current_process_exists(&target.endpoint, pid),
        ),
        Err(error) => RexProjection {
            presence: AgentPresence::Unknown,
            activity: AgentActivity::Unknown,
            reason: format!("rex.osc7501 observe failed: {error}"),
        },
    };
    publish_projection(server, worker_id, target, projection, execution.clone()).await
}

async fn publish_projection(
    server: &Arc<Server>,
    worker_id: &str,
    target: &MonitorTarget,
    projection: RexProjection,
    execution: Option<RexProgramStatusOwner>,
) -> bool {
    // Native submission may hold the state lock while Rex I/O needs the
    // runtime's workers. A contending monitor must stay off those workers.
    let server = Arc::clone(server);
    let worker_id = worker_id.to_owned();
    let target = target.clone();
    tokio::task::spawn_blocking(move || {
        let (submitted, claim_notifications) = submit_activity_with_execution_and_claims(
            &server,
            &worker_id,
            &target,
            projection,
            execution.as_ref(),
        );
        for (message_id, subscription_id) in claim_notifications {
            super::attempt_notification_detailed_with_rex_recovery(
                &server,
                &message_id,
                &subscription_id,
                now_ms(),
            );
        }
        submitted
    })
    .await
    .unwrap_or(false)
}

/// Live-query one Rex endpoint for idle/busy patrol when cached state is missing
/// or stale, using the same foreground + OSC 7501 owner projection as `rex_monitor`.
pub(super) fn observe_current_patrol_activity(
    server: &Server,
    worker: &super::state::WorkerRec,
) -> Result<AgentActivity, String> {
    let Some(endpoint) = worker
        .transport
        .as_ref()
        .and_then(|transport| transport.rex_endpoint.as_ref())
        .cloned()
    else {
        return Ok(AgentActivity::Unknown);
    };
    let control = super::binding_store::BindingControlStore::new(server.host_paths.state_root())
        .load()
        .ok();
    let (expected_kind, mut execution) = {
        let state = server.state.lock().unwrap();
        let route = super::server_route_scope(server, &state).ok().flatten();
        let expected_kind = route
            .as_ref()
            .zip(control.as_ref())
            .and_then(|(route, control)| {
                control
                    .current_for_worker(&route.project_scope_id, &route.app_scope_id, &worker.id)
                    .ok()
                    .flatten()
                    .and_then(|slot| slot.active_binding())
                    .and_then(|value| value.host.agent_kind.as_deref())
                    .and_then(crate::host_runtime::AgentKind::from_str)
            });
        let execution = state
            .rex_executions
            .get(&worker.id)
            .filter(|record| record.endpoint == endpoint)
            .map(|record| record.owner.clone());
        (expected_kind, execution)
    };
    let future_endpoint = endpoint.clone();
    let observation = if tokio::runtime::Handle::try_current().is_ok() {
        let handle = server
            .rex_monitor
            .runtime_handle()
            .unwrap_or_else(tokio::runtime::Handle::current);
        std::thread::spawn(move || handle.block_on(observe_current(&future_endpoint)))
            .join()
            .map_err(|_| "REX_PATROL_OBSERVE_JOIN_FAILED: worker thread panicked".to_string())?
    } else if let Some(handle) = server.rex_monitor.runtime_handle() {
        handle.block_on(observe_current(&future_endpoint))
    } else {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|error| error.to_string())?
            .block_on(observe_current(&future_endpoint))
    }
    .map_err(|error| error.to_string())?;
    let projection = project_observation_with_execution_for(
        &observation,
        expected_kind,
        &mut execution,
        &|pid| current_process_exists(&endpoint, pid),
    );
    Ok(if projection.presence == AgentPresence::Online {
        projection.activity
    } else {
        AgentActivity::Unknown
    })
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

#[cfg(test)]
fn project_observation_with_execution(
    observation: &RexObservation,
    execution: &mut Option<RexProgramStatusOwner>,
    process_exists: &dyn Fn(u64) -> Result<bool, String>,
) -> RexProjection {
    project_observation_with_execution_for(observation, None, execution, process_exists)
}

fn project_observation_with_execution_for(
    observation: &RexObservation,
    expected_kind: Option<crate::host_runtime::AgentKind>,
    execution: &mut Option<RexProgramStatusOwner>,
    process_exists: &dyn Fn(u64) -> Result<bool, String>,
) -> RexProjection {
    let mut projection = project_observation_for(observation, expected_kind);
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
            projection.reason = format!("rex current agent process exited pid={} name={}; refreshed block foreground pid={} name={}",
                previous.pid, previous.name, process.pid.unwrap(), process.name.as_deref().unwrap());
        }
        Ok(true) => {}
        Err(error) => {
            projection.reason = format!("{}; precise exit unavailable: {error}", projection.reason)
        }
    }
    projection
}

#[cfg(test)]
fn project_observation(observation: &RexObservation) -> RexProjection {
    project_observation_for(observation, None)
}

fn project_observation_for(
    observation: &RexObservation,
    expected_kind: Option<crate::host_runtime::AgentKind>,
) -> RexProjection {
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
    let Some((process, foreground_pid, foreground_name)) =
        observation.process.as_ref().and_then(|process| {
            let pid = process.pid.filter(|pid| *pid > 0)?;
            let name = process
                .name
                .as_deref()
                .filter(|name| !name.trim().is_empty())?;
            Some((process, pid, name))
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
            let app_matches = match expected_kind {
                Some(kind) => record.app.as_deref() == Some(kind.rex_program_app()),
                None => matches!(record.app.as_deref(), Some("codex" | "antigravity")),
            };
            app_matches
                && record
                    .owner
                    .as_ref()
                    .is_some_and(|owner| process.matches_owner(owner))
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
        agent_kind: None,
    })
}

#[cfg(test)]
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
) -> bool {
    submit_activity_with_execution_and_claims(server, worker_id, target, projection, execution).0
}

fn submit_activity_with_execution_and_claims(
    server: &Server,
    worker_id: &str,
    target: &MonitorTarget,
    projection: RexProjection,
    execution: Option<&RexProgramStatusOwner>,
) -> (bool, Vec<(String, String)>) {
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
        return (false, Vec::new());
    };
    let positive_online_idle =
        presence == AgentPresence::Online && activity == AgentActivity::Idle;
    let control = if positive_online_idle {
        super::binding_store::BindingControlStore::new(server.host_paths.state_root())
            .load()
            .ok()
    } else {
        None
    };
    let observed_ms = now_ms();
    let mut state = server.state.lock().unwrap();
    if !current_target(&state, worker_id)
        .is_some_and(|current| current.matches_state_target(target))
    {
        return (false, Vec::new());
    }
    let presence_record = AgentPresenceRecord {
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
    let same_execution = match execution {
        Some(owner) => state.rex_executions.get(worker_id).is_some_and(|old| {
            old.endpoint == target.endpoint
                && old.agent_session_id == target.agent_session_id
                && old.owner == *owner
        }),
        None => state.rex_executions.get(worker_id).is_none_or(|old| {
            old.endpoint == target.endpoint && old.agent_session_id == target.agent_session_id
        }),
    };
    let old_activity = state
        .agent_activity
        .get(scope.as_str())
        .and_then(|workers| workers.get(worker_id))
        .cloned();
    let idle_since_ms = if positive_online_idle {
        // Never extend a pre-restart idle episode across daemon restarts.
        let is_first = super::observation_epoch::first_observation_since_start(
            &server.root,
            &format!("rex-activity:{worker_id}"),
        );
        match old_activity.as_ref() {
            Some(old)
                if !is_first
                    && same_execution
                    && old.session_id == target.agent_session_id
                    && old.activity == AgentActivity::Idle
                    && old.observed_ms <= observed_ms
                    && old.idle_since_ms.is_some_and(|start| observed_ms >= start) =>
            {
                old.idle_since_ms
            }
            _ => Some(observed_ms),
        }
    } else {
        None
    };
    let activity_record = AgentActivityRecord {
        project_scope: scope.as_str().into(),
        worker_id: worker_id.to_owned(),
        session_id: target.agent_session_id.clone(),
        activity,
        background: false,
        reason,
        observed_ms,
        idle_since_ms,
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
        old.session_id != presence_record.session_id
            || old.presence != presence_record.presence
            || old.reason != presence_record.reason
    }) {
        events.push(Event::AgentPresenceObserved {
            presence: presence_record,
        });
    }
    if old_activity.as_ref().is_none_or(|old| {
        old.session_id != activity_record.session_id
            || old.activity != activity_record.activity
            || old.background != activity_record.background
            || old.reason != activity_record.reason
            || old.idle_since_ms != activity_record.idle_since_ms
    }) {
        events.push(Event::AgentActivityObserved {
            activity: activity_record,
        });
    }
    let mut claim_notifications = Vec::new();
    let mut has_continuation = false;
    if positive_online_idle {
        if let (Ok(Some(route)), Some(control)) = (super::server_route_scope(server, &state), control.as_ref()) {
            let continuation = super::ticket_continuation_events(
                &state,
                control,
                &route,
                worker_id,
                &[],
                true,
            );
            if !continuation.is_empty() {
                has_continuation = true;
                claim_notifications = super::extract_wake_bindings(&continuation);
                events.extend(continuation);
            }
        }
    }
    if !events.is_empty() {
        let commit = if has_continuation {
            if let Some(control) = control.as_ref() {
                server.commit_locked_checked_with_control(&mut state, &events, control)
            } else {
                server.commit_locked_checked(&mut state, &events)
            }
        } else {
            server.commit_locked_checked(&mut state, &events)
        };
        if let Err(error) = commit {
            server.report_state_error(&error);
            return (false, Vec::new());
        }
    }
    (true, claim_notifications)
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
