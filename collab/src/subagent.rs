use crate::{
    config,
    identity::{AppServerId, BindingId},
    proto::{PeerLifecycleAction, PeerLifecyclePhase, PeerLifecycleStage, Resp},
    scope::RouteScope,
    server::{
        global_state::MasterGrant,
        state::{now_ms, Event, State},
        Server,
    },
};
use anyhow::{bail, Context, Result};
use clap::Subcommand;
use serde::{Deserialize, Serialize};
use serde_json::json;
#[cfg(test)]
use std::{
    process::{Command, Stdio},
    time::{Duration, Instant},
};

#[derive(Clone, Debug, Serialize, Deserialize, Subcommand)]
pub enum Action {
    Start {
        #[arg(long)]
        id: Option<String>,
        /// Override ~/.appsdk/config.toml [subagent].runtime for this child only
        #[arg(long)]
        #[serde(default)]
        runtime: Option<String>,
    },
    /// Dispatch a task through the live master scheduler.
    Dispatch {
        #[arg(long)]
        request_id: String,
        #[arg(long)]
        subject: String,
        body: String,
        #[arg(long)]
        feature_id: Option<String>,
        #[arg(long)]
        worktree_path: Option<String>,
        #[arg(long)]
        branch: Option<String>,
        #[arg(long)]
        base_commit: Option<String>,
        #[arg(long, default_value = "p2")]
        priority: String,
        #[arg(long)]
        next_step: Option<String>,
    },
    List,
    Status {
        id: String,
    },
    Snapshot {
        id: String,
        #[arg(long, default_value_t = 40)]
        lines: usize,
    },
    Rearm {
        id: String,
    },
    Send {
        id: String,
        #[arg(long)]
        subject: String,
        body: String,
    },
    Ready {
        id: String,
    },
    Working {
        id: String,
    },
    Close {
        id: String,
    },
    /// Bind a completed, verified ordinary Create result as this master's
    /// managed child. All association facts (parent, child thread, binding,
    /// generation, scope) are derived from authenticated daemon state and the
    /// retained Create operation; the caller only supplies the managed id and
    /// the retained Create operation id.
    Bind {
        id: String,
        #[arg(long = "create-op")]
        create_operation_id: String,
    },
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Record {
    pub id: String,
    pub parent: String,
    pub peer: String,
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<String>,
    pub profile: Option<config::Profile>,
    pub created_ms: i64,
    pub ready_deadline_ms: i64,
    pub last_message: Option<String>,
    pub error: Option<String>,
    #[serde(default)]
    pub probe_failures: Vec<String>,
    #[serde(default)]
    pub runtime: Option<String>,
    /// Provenance for a managed child committed by explicit `subagent bind`.
    ///
    /// These pin the retained Create operation and the child's exact binding
    /// and endpoint generation observed at bind time. They are committed
    /// durably in the same `SubagentUpdated` record. Records created before
    /// explicit Bind deserialize with absent provenance; they remain
    /// observable, but Send/Ready refuse to mutate them because they never
    /// gain binding authority by inference.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub create_operation_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub binding_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint_generation: Option<u64>,
}

/// How a managed child was finalized. Archiving is the preferred outcome, but
/// a session root on another volume cannot be archived in place; that case is
/// reported as a named terminal cleanup instead of an unbounded error.
const CLOSE_OUTCOME_RECORD_ONLY: &str = "closed_record_only";

fn close_archive_error(outcome: &str, error: &str) -> String {
    format!("CLOSE_OUTCOME={outcome}: {error}")
}

fn recorded_close_outcome(_record: &Record) -> &'static str {
    CLOSE_OUTCOME_RECORD_ONLY
}
pub(crate) fn observe(
    server: &Server,
    id: Option<&str>,
    lines: Option<usize>,
) -> Result<serde_json::Value> {
    let (record, transport, mailbox, tasks, keepalive) = {
        let state = server.state.lock().unwrap();
        let Some(id) = id else {
            return Ok(json!({"subagents":state.subagents.values().collect::<Vec<_>>()}));
        };
        let record = state.subagents.get(id).context("unknown subagent")?.clone();
        let transport = state
            .workers
            .get(&record.peer)
            .and_then(|worker| worker.transport.clone());
        let mut mailbox: Vec<_> = state
            .msgs
            .values()
            .filter(|message| message.from == record.peer && message.to == record.parent)
            .cloned()
            .collect();
        mailbox.sort_by_key(|message| message.created_ms);
        let tasks = state
            .tasks
            .values()
            .filter(|task| task.owner == record.peer)
            .cloned()
            .collect::<Vec<_>>();
        let keepalive = crate::server::keepalive::view(&state, &record.peer);
        (record, transport, mailbox, tasks, keepalive)
    };
    if let Some(lines) = lines {
        if !(1..=200).contains(&lines) {
            bail!("snapshot lines must be 1..200");
        }
        bail!("SUBAGENT_SNAPSHOT_UNSUPPORTED: tmux panes do not expose durable Codex thread history; inspect the peer's durable mailbox and task state");
    }
    let transport_view = match transport.as_ref() {
        Some(transport) if transport.kind == crate::proto::TransportKind::Tmux => {
            let endpoint = transport
                .tmux_endpoint
                .as_ref()
                .context("TMUX_ENDPOINT_MISSING: registered subagent has no pane binding")?;
            crate::client::adapters::tmux::view(endpoint).map_err(anyhow::Error::msg)?
        }
        Some(_) => bail!("TRANSPORT_UNSUPPORTED: subagent status requires a tmux pane binding"),
        None => serde_json::Value::Null,
    };
    let observed = if transport_view.is_null() {
        "unknown"
    } else {
        record.status.as_str()
    };
    let mut value = json!({
        "subagent": record,
        "observed_status": observed,
        "observed_ms": now_ms(),
        "transport_view": transport_view,
        "keepalive": keepalive,
        "mailbox": mailbox,
        "tasks": tasks
    });
    merge_follow_up(&mut value);
    Ok(value)
}

fn merge_follow_up(value: &mut serde_json::Value) {
    value["retry_allowed"] = json!(true);
    value["close_required"] = json!(false);
    value["next_check"] = json!("status");
    value["progress"] = json!("snapshot");
}

fn follow_up(record: &Record, reused: bool) -> serde_json::Value {
    let mut value = json!({"subagent": record, "reused": reused});
    merge_follow_up(&mut value);
    value
}

fn can_reuse_existing(record: &Record) -> bool {
    record.thread_id.is_some() && record.status != "failed"
}

pub(crate) fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 80
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}
pub(crate) fn valid_runtime(runtime: &str) -> bool {
    runtime == "codex"
}

fn notify(
    server: &Server,
    from: &str,
    to: &str,
    subject: &str,
    body: String,
    assign_task: bool,
    managed_subagent_id: Option<&str>,
) -> Result<serde_json::Value> {
    let response = crate::server::handle_send_with_task(
        server,
        from.into(),
        to.into(),
        "notify".into(),
        Some(subject.into()),
        body,
        None,
        "immediate".into(),
        assign_task,
        managed_subagent_id,
    );
    if !response.ok {
        return Err(crate::client::ServerResponseError { response }.into());
    }
    Ok(response.data)
}

#[derive(Debug)]
struct SubagentOutcomeError {
    response: Resp,
    action: serde_json::Value,
    follow_up_error: Option<String>,
}

impl std::fmt::Display for SubagentOutcomeError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let message = self
            .response
            .error
            .as_deref()
            .unwrap_or("subagent notification failed");
        if let Some(follow_up) = &self.follow_up_error {
            write!(
                formatter,
                "{message}; follow-up state commit failed: {follow_up}"
            )
        } else {
            formatter.write_str(message)
        }
    }
}

impl std::error::Error for SubagentOutcomeError {}

fn failure_response(error: anyhow::Error) -> Resp {
    if let Some(outcome) = error.downcast_ref::<SubagentOutcomeError>() {
        let mut data = outcome.response.data.clone();
        if !data.is_object() {
            data = json!({"notification_response_data": data});
        }
        data["subagent_action"] = outcome.action.clone();
        if let Some(follow_up_error) = &outcome.follow_up_error {
            data["subagent_action"]["follow_up_error"] = json!(follow_up_error);
        }
        return Resp::err_data(error.to_string(), data);
    }
    if let Some(response) = error.downcast_ref::<crate::client::ServerResponseError>() {
        return Resp::err_data(
            response
                .response
                .error
                .clone()
                .unwrap_or_else(|| error.to_string()),
            response.response.data.clone(),
        );
    }
    Resp::err(error.to_string())
}

#[cfg(test)]
pub fn handle(server: &Server, actor: &str, token: &str, action: Action) -> Resp {
    handle_with_env(
        server,
        actor,
        token,
        action,
        std::collections::BTreeMap::new(),
    )
}
pub fn handle_with_env(
    server: &Server,
    actor: &str,
    token: &str,
    action: Action,
    _environment: std::collections::BTreeMap<String, String>,
) -> Resp {
    if let Action::Dispatch {
        request_id,
        subject,
        body,
        feature_id,
        worktree_path,
        branch,
        base_commit,
        priority,
        next_step,
    } = action
    {
        return crate::server::handle_scheduler_dispatch(
            server,
            actor.into(),
            token.into(),
            request_id,
            subject,
            body,
            feature_id,
            worktree_path,
            branch,
            base_commit,
            priority,
            next_step,
        );
    }
    if matches!(&action, Action::Start { .. }) {
        let state = server.state.lock().unwrap();
        if !state
            .workers
            .get(actor)
            .is_some_and(|worker| worker.token == token)
        {
            return Resp::err("subagent authentication failed");
        }
        return Resp::err("MANAGED_SUBAGENT_UNSUPPORTED: tmux cannot create a Codex thread; start the peer in its own tmux pane and register that pane");
    }
    match run(server, actor, token, action) {
        Ok(value) => Resp::data(value),
        Err(error) => failure_response(error),
    }
}
fn run(server: &Server, actor: &str, token: &str, action: Action) -> Result<serde_json::Value> {
    {
        let state = server.state.lock().unwrap();
        if !state.workers.get(actor).is_some_and(|w| w.token == token) {
            bail!("subagent authentication failed");
        }
    }
    if matches!(action, Action::List) {
        let state = server.state.lock().unwrap();
        return Ok(
            json!({"subagents": state.subagents.values().filter(|s| s.parent == actor).collect::<Vec<_>>() }),
        );
    }
    if let Action::Bind {
        id,
        create_operation_id,
    } = &action
    {
        return bind(server, actor, id, create_operation_id);
    }
    let id = match &action {
        Action::Status { id }
        | Action::Snapshot { id, .. }
        | Action::Rearm { id }
        | Action::Send { id, .. }
        | Action::Ready { id }
        | Action::Working { id }
        | Action::Close { id } => id,
        _ => unreachable!(),
    };
    let mut state = server.state.lock().unwrap();
    let mut record = state.subagents.get(id).context("unknown subagent")?.clone();
    let child_action = matches!(action, Action::Ready { .. } | Action::Working { .. });
    if child_action {
        let bound_thread = state
            .workers
            .get(actor)
            .and_then(|worker| worker.transport.as_ref())
            .and_then(|transport| transport.thread_id.as_deref());
        if record.peer != actor || record.thread_id.as_deref() != bound_thread {
            bail!("only the bound subagent may report readiness or work");
        }
    } else if record.parent != actor
        && crate::server::current_master_holder(server, &state)
            .map_err(anyhow::Error::msg)?
            .as_deref()
            != Some(actor)
    {
        bail!("only the creating parent or current master may manage this subagent");
    }
    // A managed mutation requires the record's committed provenance to match
    // the child's exact current binding. Records without provenance stay
    // observable but never gain mutation authority by inference.
    if matches!(action, Action::Send { .. } | Action::Ready { .. })
        && !record_provenance_matches_current_binding(&state, &record)
    {
        bail!("managed subagent provenance is absent or stale; re-bind before mutating");
    }
    match action {
        Action::Snapshot { .. } => bail!("SUBAGENT_SNAPSHOT_UNSUPPORTED: tmux panes do not expose durable Codex thread history; inspect the peer's durable mailbox and task state"),
        Action::Rearm { .. } => {
            server
                .commit_locked_checked(
                    &mut state,
                    &[Event::KeepaliveUpdated {
                        worker_id: record.peer.clone(),
                        record: crate::server::keepalive::Record::default(),
                    }],
                )
                .map_err(|error| anyhow::anyhow!("subagent rearm journal failure: {error}"))?;
            return Ok(
                json!({"subagent_id":record.id,"keepalive_rearmed":true,"notification":"none"}),
            );
        }
        Action::Status { .. } => {
            drop(state);
            return observe(server, Some(&record.id), None);
        }
        Action::Ready { .. } | Action::Working { .. } => {
            let ready = matches!(action, Action::Ready { .. });
            if matches!(
                record.status.as_str(),
                "closing" | "closed" | "failed" | "probing"
            ) {
                bail!("subagent is not running");
            }
            if ready && record.status == "idle" {
                return Ok(json!({
                    "subagent": record,
                    "reused": true,
                    "notification": "not-resent",
                    "subagent_action": {
                        "subagent_id": record.id,
                        "action": "ready",
                        "state_commit": "reused",
                        "status": "idle",
                        "reused": true
                    }
                }));
            }
            if ready && record.status == "assigned" {
                bail!("accept the assigned task before reporting completion");
            }
            let assigned_task = if !ready {
                // A keepalive thread probe may persist `working` before the
                // child gets a chance to claim its still-assigned task. Keep
                // the task binding as the source of truth and accept both
                // sides of that short race. A working task is accepted only
                // when the managed record is already working, which makes a
                // repeated claim idempotent without reopening other states.
                if !matches!(record.status.as_str(), "assigned" | "working") {
                    bail!("no assigned task to accept");
                }
                let task_id = record
                    .last_message
                    .as_ref()
                    .map(|id| format!("task-{id}"))
                    .ok_or_else(|| anyhow::anyhow!("assigned task message binding is missing"))?;
                let mut task = state
                    .tasks
                    .get(&task_id)
                    .cloned()
                    .ok_or_else(|| anyhow::anyhow!("assigned task {task_id} not found"))?;
                if task.owner != actor {
                    bail!("task owner mismatch");
                }
                if let Some(admission) = state
                    .scheduler_admissions
                    .values()
                    .find(|admission| admission.task_id == task.id)
                {
                    if admission.status != "succeeded" {
                        bail!(
                            "scheduler assignment admission is {}; cannot accept task {}",
                            admission.status,
                            task_id
                        );
                    }
                    if admission.managed_subagent_id.as_deref() != Some(record.id.as_str()) {
                        bail!("managed subagent does not own scheduler task {}", task_id);
                    }
                }
                if task.status != "assigned"
                    && !(record.status == "working" && task.status == "working")
                {
                    bail!(
                        "assigned task {task_id} is not in assigned state (status={})",
                        task.status
                    );
                }
                let task_was_assigned = task.status == "assigned";
                if task_was_assigned {
                    task.status = "working".into();
                    task.updated_ms = now_ms();
                }
                Some((task, task_was_assigned))
            } else {
                None
            };
            let next_status = if ready { "idle" } else { "working" };
            let mut events = Vec::new();
            if let Some((task, task_was_assigned)) = assigned_task {
                if task_was_assigned {
                    events.push(Event::TaskUpdated { task });
                }
            }
            if record.status != next_status {
                record.status = next_status.into();
                events.push(Event::SubagentUpdated {
                    subagent: record.clone(),
                });
            }
            let state_commit = if events.is_empty() {
                "unchanged"
            } else {
                // Persist the task and managed-record transition together so
                // replay cannot observe a half-claimed assignment.
                server
                    .commit_locked_checked(&mut state, &events)
                    .map_err(|error| {
                        anyhow::anyhow!("subagent working journal failure: {error}")
                    })?;
                "committed"
            };
            drop(state);
            let notification = if ready {
                match notify(
                    server,
                    actor,
                    &record.parent,
                    "subagent-idle",
                    format!("subagent={} is idle and available", record.id),
                    false,
                    None,
                ) {
                    Ok(value) => Some(value),
                    Err(error) => {
                        match error.downcast::<crate::client::ServerResponseError>() {
                            Ok(response) => return Err(SubagentOutcomeError {
                                response: response.response,
                                action: json!({
                                    "subagent_id": record.id,
                                    "action": "ready",
                                    "state_commit": state_commit,
                                    "status": record.status,
                                    "reused": false
                                }),
                                follow_up_error: None,
                            }.into()),
                            Err(error) => return Err(error),
                        }
                    }
                }
            } else {
                None
            };
            return Ok(json!({
                "subagent": record,
                "notification": notification,
                "subagent_action": {
                    "subagent_id": record.id,
                    "action": if ready { "ready" } else { "working" },
                    "state_commit": state_commit,
                    "status": record.status,
                    "reused": false
                }
            }));
        }
        Action::Send { subject, body, .. } => {
            if subject.trim().is_empty() || body.trim().is_empty() {
                bail!("subject and task body are required");
            }
            // A keepalive thread observation can race with the child's ready
            // report and leave the durable managed status at working even
            // though the child owns no actionable task. Reconcile that stale
            // state before asking the task sender to bind the next dispatch.
            let has_active_owned_task = state.tasks.values().any(|task| {
                task.owner == record.peer
                    && crate::server::state::task_resource_active(&task.status)
            });
            let stale_working_without_task = record.status == "working" && !has_active_owned_task;
            if record.status == "idle" && has_active_owned_task {
                bail!("managed subagent already has an active task");
            }
            if record.status != "idle" && !stale_working_without_task {
                bail!("subagent is not idle; query status instead of resending");
            }
            if stale_working_without_task {
                record.status = "idle".into();
                server
                    .commit_locked_checked(
                        &mut state,
                        &[Event::SubagentUpdated {
                            subagent: record.clone(),
                        }],
                    )
                    .map_err(|error| anyhow::anyhow!("subagent send journal failure: {error}"))?;
            }
            let pre_notification_state_commit = if stale_working_without_task {
                "committed"
            } else {
                "not-needed"
            };
            drop(state);
            let result = match notify(
                server,
                actor,
                &record.peer,
                &subject,
                body,
                true,
                Some(&record.id),
            ) {
                Ok(value) => value,
                Err(error) => {
                    let error_text = error.to_string();
                    let response = match error.downcast::<crate::client::ServerResponseError>() {
                        Ok(response) => response.response,
                        Err(error) => return Err(error),
                    };
                    let mut state = server.state.lock().unwrap();
                    let mut error_state_commit = "not-attempted";
                    let mut follow_up_error = None;
                    let mut current_status = "unknown".to_owned();
                    if let Some(mut current) = state.subagents.get(&record.id).cloned() {
                        current_status = current.status.clone();
                        if current.status == "idle" {
                            current.error = Some(error_text);
                            if let Err(journal_error) = server.commit_locked_checked(
                                &mut state,
                                &[Event::SubagentUpdated { subagent: current }],
                            ) {
                                error_state_commit = "failed";
                                follow_up_error = Some(format!(
                                    "subagent send outcome unknown: notification failed and journal commit failed: {journal_error}"
                                ));
                            } else {
                                error_state_commit = "committed";
                            }
                        }
                    } else {
                        error_state_commit = "unknown";
                    }
                    return Err(SubagentOutcomeError {
                        action: json!({
                            "subagent_id": record.id,
                            "action": "send",
                            "state_commit": pre_notification_state_commit,
                            "error_state_commit": error_state_commit,
                            "status": current_status,
                            "durable_msg_id": response.data.get("msg_id").cloned(),
                            "reused": false
                        }),
                        response,
                        follow_up_error,
                    }
                    .into());
                }
            };
            return Ok(json!({
                "subagent_id": record.id,
                "message": result,
                "subagent_action": {
                    "subagent_id": record.id,
                "action": "send",
                    "state_commit": pre_notification_state_commit,
                    "error_state_commit": "not-applicable",
                    "status": record.status,
                    "durable_msg_id": result.get("msg_id").cloned(),
                    "reused": false
                }
            }));
        }
        Action::Close { .. } => {
            if record.status == "closed" {
                let mut value = close_result(&record, None, recorded_close_outcome(&record))?;
                value["reused"] = json!(true);
                return Ok(value);
            }
            if record.status == "probing" && record.error.is_none() {
                bail!("startup probe is in progress; check status, or close after its bounded completion");
            }
            let snapshot = state
                .subagent_snapshots
                .get(&record.id)
                .filter(|receipt| record.thread_id.as_deref() == Some(receipt.thread_id.as_str()))
                .cloned();
            // A missing pane cannot provide a new snapshot, so close is
            // allowed only after its durable Collab responsibilities resolve.
            // Unknown and live panes still require an existing snapshot.
            let snapshot_captured_ms = match snapshot {
                Some(snapshot) => Some(snapshot.captured_ms),
                None => {
                    let presence = state
                        .workers
                        .get(&record.peer)
                        .map(|worker| crate::server::worker_presence(server, worker))
                        .unwrap_or(crate::server::presence::IdentityPresence::Missing);
                    if presence != crate::server::presence::IdentityPresence::Missing
                        || !subagent_responsibilities_resolved(&state, &record)
                    {
                        bail!(
                            "subagent {} requires a successful snapshot of its live tmux pane before close",
                            record.id
                        );
                    }
                    None
                }
            };
            record.status = "closing".into();
            record.error = None;
            server
                .commit_locked_checked(
                    &mut state,
                    &[Event::SubagentUpdated {
                        subagent: record.clone(),
                    }],
                )
                .map_err(|error| anyhow::anyhow!("subagent close journal failure: {error}"))?;
            drop(state);
            record.status = "closed".into();
            record.error = Some(close_archive_error(
                CLOSE_OUTCOME_RECORD_ONLY,
                "tmux cannot archive or terminate a peer pane; its registration and route remain active",
            ));
            server
                .commit_checked(&[Event::SubagentUpdated {
                    subagent: record.clone(),
                }])
                .map_err(|error| {
                    anyhow::anyhow!("subagent close outcome unknown: record-only close journal commit failed: {error}")
                })?;
            return close_result(&record, snapshot_captured_ms, CLOSE_OUTCOME_RECORD_ONLY);
        }
        _ => unreachable!(),
    }
    Ok(json!({"subagent": record}))
}

/// A missing thread may retire only after its responsibility set is empty:
/// no active owned task and no unread notification still addressed to it.
fn subagent_responsibilities_resolved(
    state: &crate::server::state::State,
    record: &Record,
) -> bool {
    let has_active_task = state.tasks.values().any(|task| {
        task.owner == record.peer && crate::server::state::task_resource_active(&task.status)
    });
    let has_unread_notification = state.msgs.values().any(|message| {
        message.to == record.peer && matches!(message.state.as_str(), "pending" | "delivered")
    });
    !has_active_task && !has_unread_notification
}

fn close_result(
    record: &Record,
    snapshot_captured_ms: Option<i64>,
    close_outcome: &str,
) -> Result<serde_json::Value> {
    Ok(json!({
        "subagent": record,
        "snapshot_captured_ms": snapshot_captured_ms,
        "close_outcome": close_outcome,
    }))
}

/// Bind one completed, verified ordinary Create result as this master's managed
/// child.
///
/// Every association fact comes from authenticated daemon state and the
/// retained Create operation; the caller supplies only the managed id and the
/// retained Create operation id. The association is committed as the single
/// canonical `subagent::Record` through the existing checked reducer/fence, so
/// Close and Bind cannot race past responsibility fencing.
fn bind(
    server: &Server,
    actor: &str,
    id: &str,
    create_operation_id: &str,
) -> Result<serde_json::Value> {
    if !valid_id(id) {
        bail!("BIND_INVALID_ID: managed id is invalid");
    }
    if create_operation_id.trim().is_empty() {
        bail!("BIND_CREATE_UNKNOWN: create operation id must not be empty");
    }
    let mut state = server.state.lock().unwrap();

    if crate::server::current_master_holder(server, &state)
        .map_err(anyhow::Error::msg)?
        .as_deref()
        != Some(actor)
    {
        bail!("BIND_REQUIRES_MASTER: only the current registered master may bind a managed child");
    }
    let grant = current_master_grant_for_actor(&state, actor).ok_or_else(|| {
        anyhow::anyhow!("BIND_REQUIRES_MASTER_BINDING: current master has no exact runtime binding")
    })?;

    let operation = state
        .peer_lifecycle_operations
        .get(create_operation_id)
        .cloned()
        .ok_or_else(|| {
            anyhow::anyhow!(
                "BIND_CREATE_UNKNOWN: no retained Create operation {create_operation_id}"
            )
        })?;
    if operation.action != PeerLifecycleAction::Create {
        bail!("BIND_CREATE_NOT_CREATE: {create_operation_id} is not a Create operation");
    }
    if operation.actor_id != actor {
        bail!("BIND_CREATE_FOREIGN: {create_operation_id} belongs to another caller");
    }
    if operation.phase != PeerLifecyclePhase::Complete {
        bail!("BIND_CREATE_INCOMPLETE: {create_operation_id} is not complete");
    }
    let create = operation.create.as_ref().ok_or_else(|| {
        anyhow::anyhow!("BIND_CREATE_INCOMPLETE: {create_operation_id} has no verified receipt")
    })?;
    if create.readiness.state != PeerLifecycleStage::Verified {
        bail!("BIND_CREATE_UNVERIFIED: {create_operation_id} readiness is not verified");
    }
    let child_thread = create.thread_id.clone().ok_or_else(|| {
        anyhow::anyhow!("BIND_CREATE_INCOMPLETE: {create_operation_id} has no verified thread id")
    })?;
    let target = operation.target.as_ref().ok_or_else(|| {
        anyhow::anyhow!("BIND_CREATE_INCOMPLETE: {create_operation_id} has no exact target")
    })?;

    // Parent and child must resolve to the same registered canonical main and
    // app scope. Different worktree directories are allowed; only the
    // registered scope is identity evidence.
    if operation.project_scope != grant.project_scope
        || operation.app_scope_id != grant.app_scope_id
    {
        bail!("BIND_SCOPE_MISMATCH: Create operation is outside the master's registered main/app scope");
    }
    let route_scope = RouteScope {
        app_scope_id: operation.app_scope_id.clone(),
        project_scope_id: operation.project_scope.clone(),
    };
    if state
        .global
        .lookup_project_for_route(&route_scope)
        .is_none()
    {
        bail!("BIND_SCOPE_UNREGISTERED: Create operation scope is not a registered route");
    }

    let child_peer = create.peer_id.clone();
    if child_peer == actor {
        bail!("BIND_SELF: parent and child must be distinct");
    }
    if target.worker_id != child_peer {
        bail!("BIND_TARGET_MISMATCH: Create target does not match the retained child");
    }
    if target.project_scope != operation.project_scope
        || target.app_scope_id != operation.app_scope_id
    {
        bail!("BIND_TARGET_SCOPE_MISMATCH: Create target scope does not match the operation");
    }
    if target.transport.thread_id.as_deref() != Some(child_thread.as_str()) {
        bail!("BIND_THREAD_MISMATCH: Create target thread does not match the retained thread");
    }
    if target.endpoint_generation == 0 {
        bail!("BIND_GENERATION_INVALID: child endpoint generation is zero");
    }

    // The child must still be registered with the exact retained thread.
    let worker = state.workers.get(&child_peer).cloned().ok_or_else(|| {
        anyhow::anyhow!("BIND_CHILD_UNREGISTERED: child {child_peer} is not currently registered")
    })?;
    let registered_thread = worker
        .transport
        .as_ref()
        .and_then(|transport| transport.thread_id.as_deref());
    if registered_thread != Some(child_thread.as_str()) {
        bail!("BIND_CHILD_THREAD_MISMATCH: current child registration does not match the retained thread");
    }

    // The child's exact current runtime binding must match the Create result.
    let binding = state
        .global
        .lookup_project(&operation.project_scope)
        .and_then(|project| project.lookup_binding(&target.binding_id))
        .ok_or_else(|| {
            anyhow::anyhow!("BIND_CHILD_BINDING_MISSING: child binding is not registered")
        })?;
    if binding.agent_id.as_str() != child_peer
        || binding.app_scope_id != operation.app_scope_id
        || binding.endpoint_generation != target.endpoint_generation
        || binding
            .native_thread_id
            .as_ref()
            .map(|thread| thread.as_str())
            != Some(child_thread.as_str())
    {
        bail!(
            "BIND_CHILD_BINDING_MISMATCH: child binding does not match the retained Create result"
        );
    }

    // No active responsibility fence or in-flight lifecycle operation may own
    // the exact child.
    if state
        .responsibility_fences
        .values()
        .any(|fence| fence.is_active() && fence.worker_id == child_peer)
    {
        bail!("BIND_CHILD_FENCED: child {child_peer} is under an active responsibility fence");
    }
    if state.peer_lifecycle_operations.values().any(|other| {
        other.action != PeerLifecycleAction::Create
            && other.phase.is_in_flight()
            && other
                .target
                .as_ref()
                .is_some_and(|target| target.worker_id == child_peer)
    }) {
        bail!("BIND_CHILD_LIFECYCLE_IN_FLIGHT: child {child_peer} has an in-flight lifecycle operation");
    }

    // An exact repeated Bind is reused; any other existing record is a conflict.
    if let Some(existing) = state.subagents.get(id).cloned() {
        if existing.parent == actor
            && existing.peer == child_peer
            && existing.thread_id.as_deref() == Some(child_thread.as_str())
            && existing.create_operation_id.as_deref() == Some(create_operation_id)
            && existing.binding_id.as_deref() == Some(target.binding_id.as_str())
            && existing.endpoint_generation == Some(target.endpoint_generation)
        {
            return Ok(json!({
                "subagent": existing,
                "create_operation_id": create_operation_id,
                "association_commit": "reused",
                "reused": true,
                "next_action": format!("child must report ready: collab subagent ready {id}"),
            }));
        }
        bail!("BIND_CONFLICT: managed id {id} already exists with different provenance");
    }
    if state
        .subagents
        .values()
        .any(|other| other.peer == child_peer)
    {
        bail!("BIND_CONFLICT: child {child_peer} is already a managed subagent");
    }

    // Recheck the exact facts at the checked commit boundary. The lock is held
    // across this build and the checked reducer re-validates the fence, so the
    // durable transition cannot observe a different child binding.
    let now = now_ms();
    let record = Record {
        id: id.to_owned(),
        parent: actor.to_owned(),
        peer: child_peer.clone(),
        status: "starting".into(),
        thread_id: Some(child_thread.clone()),
        profile: None,
        created_ms: now,
        ready_deadline_ms: now + server.config.subagent.startup.ready_timeout_seconds as i64 * 1000,
        last_message: None,
        error: None,
        probe_failures: Vec::new(),
        runtime: Some(server.config.subagent.runtime.clone()),
        create_operation_id: Some(create_operation_id.to_owned()),
        binding_id: Some(target.binding_id.as_str().to_owned()),
        endpoint_generation: Some(target.endpoint_generation),
    };
    server
        .commit_locked_checked(
            &mut state,
            &[Event::SubagentUpdated {
                subagent: record.clone(),
            }],
        )
        .map_err(|error| anyhow::anyhow!("BIND_DURABILITY_FAILED: {error}"))?;
    Ok(json!({
        "subagent": record,
        "create_operation_id": create_operation_id,
        "association_commit": "committed",
        "reused": false,
        "next_action": format!("child must report ready: collab subagent ready {id}"),
    }))
}

/// The current typed master grant held by `actor`, read only from the reducer.
fn current_master_grant_for_actor(state: &State, actor: &str) -> Option<MasterGrant> {
    let mut grants = state
        .global
        .projects
        .values()
        .flat_map(|project| project.master_grants.values())
        .filter(|grant| {
            grant.agent_id.as_str() == actor
                && state
                    .global
                    .lookup_master_grant(&grant.project_scope, &grant.binding_id)
                    .is_some_and(|current| current == *grant)
        });
    let grant = grants.next()?.clone();
    grants.next().is_none().then_some(grant)
}

/// The record's committed provenance must match the child's exact current
/// binding before a managed mutation. Legacy records carry no provenance and
/// never gain mutation authority by inference.
fn record_provenance_matches_current_binding(state: &State, record: &Record) -> bool {
    let (Some(create_operation_id), Some(binding_id), Some(endpoint_generation)) = (
        record.create_operation_id.as_deref(),
        record.binding_id.as_deref(),
        record.endpoint_generation,
    ) else {
        return false;
    };
    if create_operation_id.trim().is_empty()
        || binding_id.trim().is_empty()
        || endpoint_generation == 0
    {
        return false;
    }
    let Ok(binding_id) = BindingId::new(binding_id.to_owned()) else {
        return false;
    };
    let Some(binding) = state.global.lookup_binding(&binding_id) else {
        return false;
    };
    binding.agent_id.as_str() == record.peer
        && binding.endpoint_generation == endpoint_generation
        && binding
            .native_thread_id
            .as_ref()
            .map(|thread| thread.as_str())
            == record.thread_id.as_deref()
}

#[cfg(test)]
#[path = "subagent_tests.rs"]
mod tests;

#[cfg(test)]
include!("subagent/test_helpers.rs");
