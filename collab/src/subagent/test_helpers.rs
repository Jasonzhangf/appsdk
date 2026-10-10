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
