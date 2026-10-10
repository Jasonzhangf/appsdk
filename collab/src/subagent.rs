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

#[cfg(test)]
fn role_brief_prompt(role_brief: &serde_json::Value) -> Result<String> {
    let required = |field: &str| {
        role_brief
            .get(field)
            .and_then(serde_json::Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| anyhow::anyhow!("role_brief is missing {field}"))
    };
    let role = required("role")?;
    let role_task = required("role_task")?;
    let responsibilities = role_brief
        .get("responsibilities")
        .and_then(serde_json::Value::as_array)
        .filter(|items| !items.is_empty())
        .ok_or_else(|| anyhow::anyhow!("role_brief is missing responsibilities"))?
        .iter()
        .map(|item| {
            item.as_str()
                .filter(|value| !value.trim().is_empty())
                .map(|value| format!("- {value}"))
                .ok_or_else(|| anyhow::anyhow!("role_brief responsibility is invalid"))
        })
        .collect::<Result<Vec<_>>>()?
        .join("\n");
    let authority = role_brief
        .get("authority")
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("role_brief is missing authority"))?;
    let derivation = role_brief
        .get("derivation")
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("role_brief is missing derivation"))?;
    let blocked_boundary = required("blocked_boundary")?;
    let completion_action = required("completion_action")?;
    let next_action = required("next_action")?;
    let notification_rule = required("notification_rule")?;
    Ok(format!(
        "Role: {role}\nRole task: {role_task}\nResponsibilities:\n{responsibilities}\nAuthority: {authority}\nDerivation: {derivation}\nBlocked boundary: {blocked_boundary}\nCompletion action: {completion_action}\nNext action: {next_action}\nNotification rule: {notification_rule}"
    ))
}

#[cfg(test)]
fn child_prompt(record: &Record, role_brief: &serde_json::Value) -> Result<String> {
    let role_contract = role_brief_prompt(role_brief)?;
    Ok(format!(
        "You are a persistent AppSDK subagent. Your managed ID is {}. Your parent peer is {}. Your Collab identity is already registered on this Codex App Server thread. Do not self-register, recover a worker, or ask the user to grant identity. First report ready {}. Wait quietly for Collab messages. When assigned a task, read it, report working {}, and use the project's task/worktree workflow. Preserve others' files; code changes require your own worktree. Report progress through collab task records and send results to the parent with collab sendmessage --to {} --subject <topic> <body>. After completing a task report ready {} and remain available. Do not close this thread automatically, repeatedly poll, send ACK loops, or create other subagents without a user request.\n\
Active role contract (from the registration receipt):\n{role_contract}\n\
 collab-mcp is the shared Collab MCP for every agent. Use collab_* tools when this session lists them. The collab CLI in this cwd is also valid. If MCP is missing, unsupported, aborted, or unknown, use the CLI. Missing MCP is not a reason to skip receive, ready, or send.\n\
CLI: collab subagent ready {}; collab subagent working {}; collab recv; collab ack <message-id>; collab msg <message-id>; collab inbox; collab sendmessage --to {} --subject <topic> \"<body>\"; collab task relocate <task-id> --worktree <configured-path>.\n\
Each dispatched message has a canonical task named task-<message-id>. working claims that task; do not register a duplicate. Bind a clean worktree before code edits. ready only means thread idle. Use collab recv to read and consume a notification; use explicit ack only for legacy or already-delivered recovery. Never ACK an ACK or request automatic rearm after exhaustion.",
        record.id,
        record.parent,
        record.id,
        record.id,
        record.parent,
        record.id,
        record.id,
        record.id,
        record.parent
    ))
}

#[cfg(test)]
fn launch_args(
    runtime: &str,
    profile: &config::Profile,
    workspace: &std::path::Path,
    prompt: &str,
    mcp: &std::path::Path,
) -> Result<(String, Vec<String>)> {
    if runtime != "codex" {
        bail!("subagent.runtime must be codex");
    }
    let _ = workspace;
    let mut args = vec![
        "--profile".into(),
        profile.codex_profile.clone(),
        "--approve-for-me".into(),
    ];
    if let Some(model) = &profile.model {
        args.extend(["--model".into(), model.clone()]);
    }
    args.extend([
        "-c".into(),
        format!(
            "mcp_servers.appsdk-subagent.command={}",
            serde_json::to_string(&mcp.to_string_lossy())?
        ),
        "-c".into(),
        "mcp_servers.appsdk-subagent.env_vars=[\"CODEX_THREAD_ID\",\"PATH\",\"HOME\"]".into(),
    ]);
    for tool in [
        "collab_init",
        "collab_subagent",
        "collab_msg",
        "collab_inbox",
        "collab_ack",
        "collab_context",
        "collab_sendmessage",
        "collab_notify_status",
        "collab_task_status",
        "collab_task_register",
        "collab_task_relocate",
        "collab_task_update",
        "collab_task_block",
        "collab_task_deliver",
        "collab_task_close",
        "collab_master",
    ] {
        args.extend([
            "-c".into(),
            format!("mcp_servers.appsdk-subagent.tools.{tool}.approval_mode=\"approve\""),
        ]);
    }
    args.push(format!(
        "{prompt}\nThis session may list the shared collab-mcp tools as appsdk-subagent. Use those tools when present. If collab_ack, collab_msg, or collab_init is missing, unsupported, or aborted, use the collab CLI in this cwd. That is protocol, not a bypass."
    ));
    Ok(("codex".into(), args))
}

#[cfg(test)]
fn finish_probe(child: &mut std::process::Child, timeout: Duration) -> Result<()> {
    let started = Instant::now();
    loop {
        if let Some(status) = child.try_wait()? {
            if !status.success() {
                bail!("probe exited {status}");
            }
            return Ok(());
        }
        if started.elapsed() >= timeout {
            // The probe owns this newly-created process group, including
            // its MCP children. Never signal unrelated named processes.
            let result = unsafe { libc::kill(-(child.id() as i32), libc::SIGKILL) };
            if result != 0 && child.try_wait()?.is_none() {
                return Err(std::io::Error::last_os_error().into());
            }
            child.wait()?;
            bail!("probe timed out");
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[cfg(test)]
fn probe_with(
    executable: &std::path::Path,
    runtime: &str,
    profile: &config::Profile,
    settings: &config::Health,
    environment: &std::collections::BTreeMap<String, String>,
) -> Result<()> {
    if runtime != "codex" {
        bail!("subagent.runtime must be codex");
    }
    let directory =
        std::env::temp_dir().join(format!("appsdk-probe-{:016x}", rand::random::<u64>()));
    std::fs::create_dir(&directory)?;
    let result = (|| {
        let output = directory.join("result.txt");
        let prompt = format!(
            "Connectivity probe only. Do not use tools or read files. Reply exactly: {}",
            settings.expected_response
        );
        let mut command = Command::new(executable);
        command.env_clear().envs(environment);
        command
            .args([
                "exec",
                "--profile",
                &profile.codex_profile,
                "--ephemeral",
                "--skip-git-repo-check",
                "--sandbox",
                "read-only",
                "--output-last-message",
            ])
            .arg(&output)
            .arg(&prompt);
        if let Some(model) = &profile.model {
            command.args(["--model", model]);
        }
        command
            .current_dir(&directory)
            .env_remove("TMUX")
            .env_remove("TMUX_PANE")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        use std::os::unix::process::CommandExt;
        command.process_group(0);
        let mut child = command.spawn().context("cannot start health probe")?;
        finish_probe(&mut child, Duration::from_secs(settings.timeout_seconds))?;
        let body = std::fs::read_to_string(output)?;
        if body.trim() != settings.expected_response {
            bail!(
                "probe response did not match expected response: {:?}",
                body.trim()
            );
        }
        Ok(())
    })();
    let cleanup = std::fs::remove_dir_all(&directory);
    result.and_then(|_| {
        cleanup?;
        Ok(())
    })
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
fn child_appserver_candidate(
    parent_transport: &crate::proto::SelectedTransport,
    root: &std::path::Path,
    thread_id: &crate::identity::NativeThreadId,
) -> Result<crate::proto::AppServerCandidate> {
    let child_status = crate::client::adapters::codex_app_server::read_thread_status(
        parent_transport,
        thread_id.as_str(),
    )
    .map_err(|error| anyhow::anyhow!("{error}"))?;
    let child_session_id = child_session_id_from_thread_status(&child_status)?;
    child_appserver_candidate_from_session(parent_transport, root, &child_session_id, thread_id)
}

#[cfg(test)]
fn child_session_id_from_thread_status(thread_status: &serde_json::Value) -> Result<String> {
    thread_status
        .pointer("/thread/sessionId")
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(str::to_owned)
        .context("child App Server thread/read response is missing thread.sessionId")
}

#[cfg(test)]
fn child_appserver_candidate_from_session(
    parent_transport: &crate::proto::SelectedTransport,
    root: &std::path::Path,
    child_session_id: &str,
    thread_id: &crate::identity::NativeThreadId,
) -> Result<crate::proto::AppServerCandidate> {
    Ok(crate::proto::AppServerCandidate {
        endpoint: parent_transport
            .endpoint
            .clone()
            .context("parent App Server transport has no endpoint")?,
        namespace: parent_transport
            .namespace
            .clone()
            .context("parent App Server transport has no namespace")?,
        session_id: child_session_id.to_owned(),
        thread_id: thread_id.to_string(),
        cwd: root.display().to_string(),
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg(test)]
enum ChildNotificationFailureAction {
    PreserveRouteBinding,
    RetireRouteBinding,
}

#[cfg(test)]
fn child_notification_failure_action(
    error: &crate::client::adapters::AdapterError,
) -> ChildNotificationFailureAction {
    match error {
        crate::client::adapters::AdapterError::Unknown {
            operation: "rpc",
            detail,
        } if detail.contains("no rollout found for thread id") => {
            ChildNotificationFailureAction::PreserveRouteBinding
        }
        _ => ChildNotificationFailureAction::RetireRouteBinding,
    }
}

#[allow(clippy::too_many_arguments)]
#[cfg(test)]
fn child_notification_failure_result(
    route_owner: &Server,
    server: &Server,
    record: &mut Record,
    parent_transport: &crate::proto::SelectedTransport,
    thread_id: &crate::identity::NativeThreadId,
    worker_id: &str,
    child_cwd: &str,
    app_scope: Option<&AppServerId>,
    error: crate::client::adapters::AdapterError,
) -> Result<()> {
    if child_notification_failure_action(&error)
        == ChildNotificationFailureAction::PreserveRouteBinding
    {
        record.thread_id = Some(thread_id.to_string());
        bail!(
            "child notification failed: {error}; thread archive skipped: missing App Server rollout; route preserved; binding preserved"
        );
    }
    record.thread_id = None;
    let archive_result = crate::client::adapters::codex_app_server::archive_thread(
        parent_transport,
        thread_id.as_str(),
    );
    let route_cleanup = crate::server::retire_current_thread_route_after_launch_failure(
        route_owner,
        server,
        worker_id,
        child_cwd,
        app_scope,
    );
    let binding_cleanup = crate::server::retire_runtime_binding_after_route_failure(
        server,
        worker_id,
        child_cwd,
        app_scope,
        &record.parent,
        "child notification failed",
    );
    let archive_status = archive_result
        .map(|_| "archived".to_owned())
        .unwrap_or_else(|archive_error| format!("archive failed: {archive_error}"));
    let route_status = route_cleanup
        .map(|_| "route retired".to_owned())
        .unwrap_or_else(|cleanup_error| format!("route cleanup failed: {cleanup_error}"));
    let binding_status = binding_cleanup
        .map(|_| "binding retired".to_owned())
        .unwrap_or_else(|cleanup_error| format!("binding cleanup failed: {cleanup_error}"));
    bail!(
        "child notification failed: {error}; thread {archive_status}; {route_status}; {binding_status}"
    );
}

#[cfg(test)]
fn launch(
    server: &Server,
    route_owner: &Server,
    record: &mut Record,
    settings: &config::Subagent,
    environment: std::collections::BTreeMap<String, String>,
    app_scope: Option<&AppServerId>,
) -> Result<()> {
    crate::scope::init(&server.root).context("cannot write project MCP and CLI permissions")?;
    record.runtime = Some(settings.runtime.clone());
    let mut errors = Vec::new();
    let executable = std::path::Path::new("codex");
    let names = settings.profile_priority.clone();
    for name in &names {
        let profile = &settings.profiles[name];
        match probe_with(
            executable,
            &settings.runtime,
            profile,
            &settings.health,
            &environment,
        ) {
            Ok(()) => {
                record.profile = Some(profile.clone());
                break;
            }
            Err(error) => errors.push(format!("{name}: {error}")),
        }
    }
    record.probe_failures = errors.clone();
    let profile = record
        .profile
        .as_ref()
        .context(format!("no healthy profile: {}", errors.join("; ")))?;
    let parent_transport = {
        let state = server.state.lock().unwrap();
        state
            .workers
            .get(&record.parent)
            .and_then(|worker| worker.transport.clone())
            .context("parent has no registered App Server transport")?
    };
    let thread_id = crate::client::adapters::codex_app_server::start_thread(
        &parent_transport,
        &server.root,
        profile.model.as_deref(),
    )
    .map_err(|error| anyhow::anyhow!("{error}"))?;
    let candidate = match child_appserver_candidate(&parent_transport, &server.root, &thread_id) {
        Ok(candidate) => candidate,
        Err(error) => {
            let archive_result = crate::client::adapters::codex_app_server::archive_thread(
                &parent_transport,
                thread_id.as_str(),
            );
            let archive_status = archive_result
                .map(|_| "archived".to_owned())
                .unwrap_or_else(|archive_error| format!("archive failed: {archive_error}"));
            record.thread_id = None;
            bail!(
                "cannot build child App Server registration candidate: {error}; thread {archive_status}"
            );
        }
    };
    let scope = crate::scope::Scope {
        root: server.root.clone(),
    };
    let mut ident = crate::identity::load_or_create(&scope, Some(record.peer.clone()), None)?;
    let registered = crate::server::handle_register_with_app_scope_unfinalized(
        server,
        ident.worker_id.clone(),
        ident.token.clone(),
        server.root.display().to_string(),
        app_scope.cloned(),
        Some(crate::proto::TransportCandidates {
            appserver: Some(candidate),
            tmux: None,
            // A subagent is a child App Server thread. dsh peers register as
            // independent peers, never through this path.
            dsh: None,
        }),
    );
    if !registered.ok {
        record.thread_id = None;
        let _ = crate::client::adapters::codex_app_server::archive_thread(
            &parent_transport,
            thread_id.as_str(),
        );
        bail!(
            "cannot register child identity: {}",
            registered.error.unwrap_or_default()
        );
    }
    let child_cwd = server.root.display().to_string();
    let (runtime, transport) = match crate::identity::registration_from_receipt(
        &registered.data,
        &ident.worker_id,
        &scope.root,
    ) {
        Ok(registration) => registration,
        Err(error) => {
            record.thread_id = None;
            let archive_result = crate::client::adapters::codex_app_server::archive_thread(
                &parent_transport,
                thread_id.as_str(),
            );
            let cleanup_result = crate::server::retire_runtime_binding_after_route_failure(
                server,
                &ident.worker_id,
                &child_cwd,
                app_scope,
                &record.parent,
                "child registration receipt invalid",
            );
            let archive_status = archive_result
                .map(|_| "archived".to_owned())
                .unwrap_or_else(|archive_error| format!("archive failed: {archive_error}"));
            let cleanup_status = cleanup_result
                .map(|_| "binding retired".to_owned())
                .unwrap_or_else(|cleanup_error| format!("binding cleanup failed: {cleanup_error}"));
            bail!(
                    "child registration receipt invalid: {error}; thread {archive_status}; {cleanup_status}"
            );
        }
    };
    let role_brief = match crate::identity::role_brief_from_registration_receipt(&registered.data) {
        Ok(role_brief) => role_brief,
        Err(error) => {
            record.thread_id = None;
            let archive_result = crate::client::adapters::codex_app_server::archive_thread(
                &parent_transport,
                thread_id.as_str(),
            );
            let cleanup_result = crate::server::retire_runtime_binding_after_route_failure(
                server,
                &ident.worker_id,
                &child_cwd,
                app_scope,
                &record.parent,
                "child registration role brief invalid",
            );
            let archive_status = archive_result
                .map(|_| "archived".to_owned())
                .unwrap_or_else(|archive_error| format!("archive failed: {archive_error}"));
            let cleanup_status = cleanup_result
                .map(|_| "binding retired".to_owned())
                .unwrap_or_else(|cleanup_error| format!("binding cleanup failed: {cleanup_error}"));
            bail!(
                "child registration role brief invalid: {error}; thread {archive_status}; {cleanup_status}"
            );
        }
    };
    let prompt = child_prompt(record, &role_brief)?;
    if let Err(error) =
        crate::identity::persist_registration(&scope, &mut ident, runtime, transport.clone())
    {
        record.thread_id = None;
        let archive_result = crate::client::adapters::codex_app_server::archive_thread(
            &parent_transport,
            thread_id.as_str(),
        );
        let cleanup_result = crate::server::retire_runtime_binding_after_route_failure(
            server,
            &ident.worker_id,
            &child_cwd,
            app_scope,
            &record.parent,
            "child runtime persistence failed",
        );
        let archive_status = archive_result
            .map(|_| "archived".to_owned())
            .unwrap_or_else(|archive_error| format!("archive failed: {archive_error}"));
        let cleanup_status = cleanup_result
            .map(|_| "binding retired".to_owned())
            .unwrap_or_else(|cleanup_error| format!("binding cleanup failed: {cleanup_error}"));
        bail!(
            "cannot persist child runtime binding: {error}; thread {archive_status}; {cleanup_status}"
        );
    }
    if let Err(route_error) = crate::server::commit_current_thread_route_for_runtime(
        route_owner,
        server,
        &ident.worker_id,
        &child_cwd,
        app_scope,
    ) {
        record.thread_id = None;
        let archive_result = crate::client::adapters::codex_app_server::archive_thread(
            &parent_transport,
            thread_id.as_str(),
        );
        let cleanup_result = crate::server::retire_runtime_binding_after_route_failure(
            server,
            &ident.worker_id,
            &child_cwd,
            app_scope,
            &record.parent,
            "child route publication failed",
        );
        let archive_status = archive_result
            .map(|_| "archived".to_owned())
            .unwrap_or_else(|error| format!("archive failed: {error}"));
        let cleanup_status = cleanup_result
            .map(|_| "binding retired".to_owned())
            .unwrap_or_else(|error| format!("binding cleanup failed: {error}"));
        bail!(
            "child route publication failed: {route_error}; thread {archive_status}; {cleanup_status}"
        );
    }
    if let Err(error) = crate::client::adapters::codex_app_server::immediate_notify(
        &transport,
        Some(
            parent_transport
                .thread_id
                .as_deref()
                .context("parent App Server transport has no thread_id")?,
        ),
        &prompt,
        &format!("collab-subagent-start-{}", record.id),
    ) {
        return child_notification_failure_result(
            route_owner,
            server,
            record,
            &parent_transport,
            &thread_id,
            &ident.worker_id,
            &child_cwd,
            app_scope,
            error,
        );
    }
    let _ = environment;
    record.thread_id = Some(thread_id.to_string());
    record.status = "starting".into();
    record.ready_deadline_ms = now_ms() + settings.startup.ready_timeout_seconds as i64 * 1000;
    Ok(())
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
