fn parse_wire_request(line: &str) -> Result<(Option<ProjectContext>, Req), String> {
    match serde_json::from_str::<RequestEnvelope>(line) {
        Ok(mut envelope) => {
            envelope
                .normalize_identity_context()
                .map_err(|error| format!("bad request: {error}"))?;
            Ok(envelope.into_parts())
        }
        Err(envelope_error) => {
            // An invalid project_context must not be reinterpreted as a
            // legacy unscoped request merely because serde ignores unknown
            // fields when decoding Req.  That would turn a forged envelope
            // into a context-free Ping or a later default route.
            let has_project_context = serde_json::from_str::<serde_json::Value>(line)
                .ok()
                .and_then(|value| {
                    value
                        .as_object()
                        .map(|object| object.contains_key("project_context"))
                })
                .unwrap_or(false);
            if has_project_context {
                return Err(format!(
                    "bad request: invalid project context envelope: {envelope_error}"
                ));
            }
            serde_json::from_str::<Req>(line)
                .map(|request| (None, request))
                .map_err(|request_error| {
                    format!("bad request: {request_error}; wire envelope parse: {envelope_error}")
                })
        }
    }
}

async fn dispatch_wire(
    server: Arc<Server>,
    project_context: Option<ProjectContext>,
    req: Req,
) -> Resp {
    match req {
        Req::Poll {
            worker_id,
            token,
            timeout_ms,
            receive_id,
        } => {
            let poll_req = Req::Poll {
                worker_id: worker_id.clone(),
                token: token.clone(),
                timeout_ms,
                receive_id: receive_id.clone(),
            };
            if let Err(error) =
                validate_request_context(&server, &poll_req, project_context.as_ref())
            {
                return Resp::err(error);
            }
            let admission = {
                let check = server.state.lock().unwrap();
                if let Err(error) = verify(&check, &worker_id, &token) {
                    Some(error)
                } else if check.admission_frozen() {
                    Some(Resp::err("PROJECT_RESET_PENDING: this scope is completing an already committed project reset"))
                } else {
                    None
                }
            };
            match admission {
                Some(response) => response,
                None => {
                    handle_poll_async_with_context(
                        server,
                        worker_id,
                        Some(token),
                        timeout_ms,
                        project_context,
                        receive_id,
                    )
                    .await
                }
            }
        }
        req => tokio::task::spawn_blocking(move || {
            let route_gate = if mutation_blocked_during_project_reset(&req)
                || wire_mutation_principal(&req).is_some()
            {
                Some(wire_route_mutation_gate(&server))
            } else {
                None
            };
            let _route_gate_guard = route_gate.as_ref().map(|gate| gate.lock().unwrap());
            if let Err(error) = validate_request_context(&server, &req, project_context.as_ref()) {
                return Resp::err(error);
            }
            dispatch_with_route_context(&server, req, project_context)
        })
        .await
        .unwrap_or_else(|e| Resp::err(format!("handler join error: {}", e))),
    }
}

/// Route a host-daemon connection to the reducer that owns its exact
/// `(app_scope_id, project_scope_id)` pair.  Registration is handled inside a
/// blocking transaction so the route table is published only after the target
/// owner has committed the current identity binding.
async fn dispatch_wire_routed(
    manager: Arc<ProjectRuntimeManager>,
    project_context: Option<ProjectContext>,
    req: Req,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
) -> (Arc<Server>, Resp) {
    let is_reply = matches!(&req, Req::Send { in_reply_to: Some(_), .. }
        | Req::AddressSend { in_reply_to: Some(_), .. });
    let local_machine = manager.host.config.federation.as_ref()
        .map(|configuration| configuration.machine.clone()).unwrap_or_else(|| "local".into());
    let (server, mut response) = match req {
        Req::DirectoryAll => (manager.host.clone(), manager.directory_all()),
        req @ Req::DefaultSet { .. } => {
            federation_defaults::dispatch(manager, project_context, req).await
        }
        req @ Req::ChannelInbound { .. } => {
            federation_runtime::channel_dispatch(manager, project_context, req).await
        }
        req @ (Req::AddressSend { .. } | Req::FederationDirectory { .. } | Req::MessageRefStatus { .. }) => {
            federation_runtime::dispatch(manager, project_context, req).await
        }
        Req::ProjectReset { worker_id, token } => {
            let reset_manager = manager.clone();
            let response = tokio::task::spawn_blocking(move || match project_context {
                Some(context) => reset_manager.reset_project(&context, &worker_id, &token),
                None => Resp::err("PROJECT_CONTEXT_REQUIRED"),
            })
            .await
            .unwrap_or_else(|error| {
                Resp::err(format!("RESET_INCOMPLETE: handler failed: {error}"))
            });
            (manager.host.clone(), response)
        }
        Req::Shutdown { password } => {
            let response = handle_user_shutdown(&manager.host, password.as_ref());
            (manager.host.clone(), response)
        }
        Req::RouteResolve { tmux_endpoint } => {
            let response = match manager.resolve_route_by_tmux_endpoint(&tmux_endpoint) {
                Ok(route) => match serde_json::to_value(route) {
                    Ok(value) => Resp::data(value),
                    Err(error) => {
                        Resp::err(format!("ROUTE_RESOLVE_INVALID: serialize route: {error}"))
                    }
                },
                Err(error) => Resp::err(error),
            };
            (manager.host.clone(), response)
        }
        Req::RouteResolveNative {
            session_id,
            native_thread_id,
        } => {
            let _guard = manager.identity_gate.lock().unwrap();
            let resolved = match native_thread_id.as_deref() {
                Some(thread) => manager.resolve_route_by_native_thread(&session_id, thread),
                None => project_context
                    .as_ref()
                    .ok_or_else(|| {
                        "PROJECT_CONTEXT_REQUIRED: session route reads require project/app scope"
                            .to_owned()
                    })
                    .and_then(|context| {
                        manager.resolve_route_by_native_session(context, &session_id)
                    }),
            };
            let response = match resolved {
                Ok(route) => match serde_json::to_value(route) {
                    Ok(value) => Resp::data(value),
                    Err(error) => {
                        Resp::err(format!("ROUTE_RESOLVE_INVALID: serialize route: {error}"))
                    }
                },
                Err(error) => Resp::err(error),
            };
            (manager.host.clone(), response)
        }
        Req::Poll {
            worker_id,
            token,
            timeout_ms,
            receive_id,
        } => {
            let Some(context) = project_context else {
                return (
                    manager.host.clone(),
                    Resp::err(
                        "PROJECT_CONTEXT_REQUIRED: canonical project root and scope are required",
                    ),
                );
            };
            let poll_req = Req::Poll {
                worker_id: worker_id.clone(),
                token: token.clone(),
                timeout_ms,
                receive_id: receive_id.clone(),
            };
            let server = match manager.select_runtime(&context) {
                Ok(server) => server,
                Err(error) => return (manager.host.clone(), Resp::err(error)),
            };
            if let Err(error) = manager.recover_pending_effects_for_scope(&context) {
                return (server, Resp::err(error));
            }
            if let Err(error) = validate_request_context(&server, &poll_req, Some(&context)) {
                return (server, Resp::err(error));
            }
            let admission = {
                let check = server.state.lock().unwrap();
                if let Err(error) = verify(&check, &worker_id, &token) {
                    Some(error)
                } else if check.admission_frozen() {
                    Some(Resp::err("PROJECT_RESET_PENDING: this scope is completing an already committed project reset"))
                } else {
                    None
                }
            };
            let response = match admission {
                Some(response) => response,
                None => {
                    let poll = handle_poll_async_with_context(
                        server.clone(),
                        worker_id,
                        Some(token),
                        timeout_ms,
                        Some(context),
                        receive_id,
                    );
                    tokio::pin!(poll);
                    tokio::select! {
                        response = poll.as_mut() => response,
                        _ = shutdown.changed() => Resp::err("DAEMON_SHUTTING_DOWN"),
                    }
                }
            };
            (server, response)
        }
        req => {
            let gate_effects = mutation_blocked_during_project_reset(&req)
                && !matches!(&req, Req::Register { .. } | Req::IdentityContext { .. });
            let result = tokio::task::spawn_blocking({
                let manager = manager.clone();
                move || {
                    if gate_effects {
                        if let Some(context) = project_context.as_ref() {
                            if let Err(error) = manager.recover_pending_effects_for_scope(context) {
                                return (manager.host.clone(), Resp::err(error));
                            }
                        }
                    }
                    manager.dispatch_sync(project_context, req)
                }
            })
            .await;
            match result {
                Ok(result) => result,
                Err(error) => (
                    manager.host.clone(),
                    Resp::err(format!("handler join error: {error}")),
                ),
            }
        }
    };
    let local_receipt = response.data.get("reference").and_then(|reference| reference.get("machine"))
        .and_then(serde_json::Value::as_str).is_none_or(|machine| machine.eq_ignore_ascii_case(&local_machine));
    if is_reply && response.ok && local_receipt {
        let message_id = response.data.get("msg_id").and_then(serde_json::Value::as_str).map(str::to_owned);
        if let Some(message_id) = message_id {
            let has_channel_route = server.state.lock().unwrap().channel_reply_routes.contains_key(&message_id);
            if has_channel_route {
                match channel_delivery::deliver_committed_reply(server.clone(), message_id.clone()).await {
                    Ok(result) => response.data["channel_delivery"] = json!(result),
                    Err(error) => {
                        response.data["channel_delivery_error"] = json!(error);
                        response.data["channel_delivery"] = json!(channel_delivery::current_result(
                            &server.state.lock().unwrap(), &message_id));
                    }
                }
            }
        }
    }
    (server, response)
}

async fn conn_task(server: Arc<Server>, stream: tokio::net::UnixStream) {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    let (reader, mut writer) = stream.into_split();
    let mut lines = BufReader::new(reader).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        if line.trim().is_empty() {
            continue;
        }
        let resp = match parse_wire_request(&line) {
            Ok((project_context, req)) => {
                let resp = dispatch_wire(server.clone(), project_context, req).await;
                resp
            }
            Err(error) => {
                let resp = Resp::err(error);
                resp
            }
        };
        let mut out = serde_json::to_string(&resp).expect("serialize resp");
        out.push('\n');
        if writer.write_all(out.as_bytes()).await.is_err() {
            break;
        }
    }
}

async fn conn_task_routed(
    manager: Arc<ProjectRuntimeManager>,
    stream: tokio::net::UnixStream,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
    user_shutdown: Option<tokio::sync::mpsc::Sender<()>>,
) {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    let (reader, mut writer) = stream.into_split();
    let mut lines = BufReader::new(reader).lines();
    loop {
        let line = tokio::select! {
            _ = shutdown.changed() => break,
            line = lines.next_line() => match line {
                Ok(Some(line)) => line,
                _ => break,
            },
        };
        if line.trim().is_empty() {
            continue;
        }
        let mut close_daemon = false;
        let resp = match parse_wire_request(&line) {
            Ok((project_context, req)) => {
                let maintenance = matches!(&req, Req::Shutdown { .. });
                let (_, resp) =
                    dispatch_wire_routed(manager.clone(), project_context, req, shutdown.clone())
                        .await;
                close_daemon = maintenance && resp.ok;
                resp
            }
            Err(error) => {
                let resp = Resp::err(error);
                resp
            }
        };
        let mut out = serde_json::to_string(&resp).expect("serialize resp");
        out.push('\n');
        if *shutdown.borrow() {
            break;
        }
        let written = tokio::select! {
            biased;
            _ = shutdown.changed() => break,
            written = writer.write_all(out.as_bytes()) => written,
        };
        if written.is_err() {
            break;
        }
        if close_daemon {
            if let Some(sender) = &user_shutdown {
                let _ = sender.try_send(());
            }
            break;
        }
    }
}

fn is_identity_query_intent(request: &Req) -> bool {
    matches!(
        request,
        Req::IdentityContext {
            identity_context: Some(identity_context),
            ..
        } if is_identity_query_request(identity_context)
    )
}

fn is_identity_query_request(request: &IdentityContextRequest) -> bool {
    request.query || request.invocation == "query" || request.action == "query"
}

fn is_valid_identity_query_shape(request: &IdentityContextRequest) -> bool {
    request.query && request.invocation == "query" && request.action == "query"
}


fn acquire_daemon_lock(lock_path: &Path, socket_path: &Path) -> anyhow::Result<std::fs::File> {
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .open(&lock_path)?;
    use std::os::unix::io::AsRawFd;
    let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if rc == 0 {
        return Ok(file);
    }
    let error = std::io::Error::last_os_error();
    match error.raw_os_error() {
        Some(code) if code == libc::EAGAIN || code == libc::EWOULDBLOCK => anyhow::bail!(
            "server already running at {}: {}",
            socket_path.display(),
            error
        ),
        Some(code) if code == libc::EPERM => anyhow::bail!(
            "cannot acquire daemon lock at {}: flock is unavailable or denied; refusing PID fallback: {}",
            lock_path.display(),
            error
        ),
        _ => Err(error.into()),
    }
}

/// Acquire the same host writer lock used by the resident daemon. Offline
/// maintenance commands use this to prove no daemon writer exists before they
/// mutate durable state.
pub(crate) fn acquire_reset_lock(
    lock_path: &Path,
    socket_path: &Path,
) -> anyhow::Result<std::fs::File> {
    acquire_daemon_lock(lock_path, socket_path)
}

/// True when some other host daemon still owns the pre-host-state writer lock.
///
/// Every daemon keeps this legacy lock for its whole lifetime as a fence against
/// binaries released before the host-scoped state directory existed. A second
/// daemon cannot start while the fence is held, so a supervised start must report
/// the conflict and exit instead of letting launchd restart it forever.
pub fn legacy_host_daemon_present(host_paths: &HostPaths) -> bool {
    let lock_path = legacy_host_daemon_lock_path(host_paths);
    let Ok(file) = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .open(&lock_path)
    else {
        // Fail closed: an unreadable fence is treated as held so the service
        // does not start a second writer.
        return true;
    };
    use std::os::unix::io::AsRawFd;
    let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if rc != 0 {
        return true;
    }
    unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_UN) };
    false
}

fn acquire_legacy_writer_lock(
    lock_path: &Path,
    description: &str,
) -> anyhow::Result<std::fs::File> {
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .open(lock_path)?;
    use std::os::unix::io::AsRawFd;
    let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if rc == 0 {
        return Ok(file);
    }
    let error = std::io::Error::last_os_error();
    if matches!(
        error.raw_os_error(),
        Some(code) if code == libc::EAGAIN || code == libc::EWOULDBLOCK
    ) {
        anyhow::bail!(
            "DAEMON_MIGRATION_REQUIRED: legacy {description} lock is held at {}; the user must resolve the existing writer through formal maintenance before starting the host daemon",
            lock_path.display()
        );
    }
    Err(error.into())
}

fn legacy_host_daemon_lock_path(host_paths: &HostPaths) -> PathBuf {
    legacy_host_daemon_lock_path_for(host_paths.state_root(), &default_state_root())
}

fn legacy_host_daemon_lock_path_for(state_root: &Path, default_root: &Path) -> PathBuf {
    if state_root == default_root {
        PathBuf::from(LEGACY_HOST_DAEMON_LOCK_PATH)
    } else {
        state_root.join("legacy-host.lock")
    }
}

fn default_state_root() -> PathBuf {
    std::env::var_os(crate::scope::HOME_ENV)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .map(|home| home.join(".collab"))
        .unwrap_or_else(|| PathBuf::from("/nonexistent/collab-state"))
}

fn probe_legacy_socket(socket_path: &Path) -> anyhow::Result<bool> {
    match crate::client::connect(socket_path) {
        Ok(stream) => {
            drop(stream);
            Ok(true)
        }
        Err(error) if crate::client::stale_socket_error(&error) => Ok(false),
        Err(error) => Err(anyhow::Error::new(error)),
    }
}

pub(crate) struct LegacyWriterFence {
    // These guards intentionally stay alive for the complete daemon lifetime.
    // The legacy binary does not know the new host lock, so a one-shot probe is
    // insufficient to prevent it from writing the same storage after startup.
    _host_lock: Option<std::fs::File>,
    _project_lock: Option<std::fs::File>,
    project_error: Option<String>,
}

/// Reject a reachable pre-host-endpoint socket after all compatible legacy
/// locks are held. A stale socket is safe to classify as absent only for the
/// platform's explicit stale-socket errors; any other probe error is unknown
/// and fails closed.
fn ensure_legacy_socket_absent(scope: &Scope, host_paths: &HostPaths) -> anyhow::Result<()> {
    let project_server_dir = scope.server_dir();
    let legacy_socket = project_server_dir.join("server.sock");
    if legacy_socket != host_paths.socket_path() && probe_legacy_socket(&legacy_socket)? {
        anyhow::bail!(
            "DAEMON_MIGRATION_REQUIRED: legacy project daemon is reachable at {}; the user must resolve the existing writer through formal maintenance before starting the host daemon",
            legacy_socket.display()
        );
    }
    Ok(())
}

/// Hold every lock understood by the pre-host-endpoint daemon until the new
/// daemon exits. Acquiring these locks before current-state loading closes the
/// check-to-open race: an old writer can neither start after the check nor
/// acquire the same project lock while this process owns the state.
pub(crate) fn acquire_legacy_writer_fence(
    scope: &Scope,
    host_paths: &HostPaths,
) -> anyhow::Result<LegacyWriterFence> {
    acquire_legacy_writer_fence_mode(scope, host_paths, true)
}

fn acquire_legacy_writer_fence_mode(
    scope: &Scope,
    host_paths: &HostPaths,
    strict_project: bool,
) -> anyhow::Result<LegacyWriterFence> {
    let legacy_host_lock = legacy_host_daemon_lock_path(host_paths);
    let project_server_dir = scope.server_dir();
    let host_lock = if legacy_host_lock != host_paths.lock_path() {
        Some(acquire_legacy_writer_lock(
            &legacy_host_lock,
            "host daemon",
        )?)
    } else {
        None
    };

    let legacy_project_lock = project_server_dir.join("daemon.lock");
    let project_guard_required = legacy_project_lock != host_paths.lock_path()
        && legacy_project_lock != legacy_host_lock
        && project_server_dir.is_dir();
    let (project_lock, project_error) = if project_guard_required {
        match acquire_legacy_writer_lock(&legacy_project_lock, "project daemon") {
            Ok(lock) => (Some(lock), None),
            Err(error) if strict_project => return Err(error),
            Err(error) => (None, Some(format!("{error:#}"))),
        }
    } else {
        (None, None)
    };

    let project_error = match ensure_legacy_socket_absent(scope, host_paths) {
        Ok(()) => project_error,
        Err(error) if strict_project => return Err(error),
        Err(error) => Some(match project_error {
            Some(previous) => format!("{previous}; {error:#}"),
            None => format!("{error:#}"),
        }),
    };
    Ok(LegacyWriterFence {
        _host_lock: host_lock,
        _project_lock: project_lock,
        project_error,
    })
}

fn is_provisional_cli_runtime(project_context: &ProjectContext, worker_id: &str) -> bool {
    project_context
        .runtime_context
        .as_ref()
        .is_some_and(|runtime| {
            crate::identity::RuntimeIdentity::cli_adapter(worker_id)
                .is_ok_and(|provisional| runtime == &provisional)
        })
}

fn same_inode(left: &std::fs::Metadata, right: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    left.dev() == right.dev() && left.ino() == right.ino()
}

fn prepare_socket_path(sock_path: &Path) -> anyhow::Result<()> {
    use std::os::unix::fs::FileTypeExt;

    let before = match std::fs::symlink_metadata(sock_path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    if !before.file_type().is_socket() {
        anyhow::bail!(
            "server socket path is occupied by {}; refusing to remove it",
            sock_path.display()
        );
    }
    match crate::client::connect(sock_path) {
        Ok(_) => anyhow::bail!("server already running at {}", sock_path.display()),
        Err(error) if crate::client::stale_socket_error(&error) => {}
        Err(error) => {
            return Err(anyhow::Error::new(error).context(format!(
                "cannot determine whether stale server socket {} can be removed",
                sock_path.display()
            )))
        }
    }
    let after = match std::fs::symlink_metadata(sock_path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    if !same_inode(&before, &after) {
        anyhow::bail!(
            "server socket changed while checking {}; refusing to remove it",
            sock_path.display()
        );
    }
    std::fs::remove_file(sock_path)?;
    Ok(())
}

fn remove_listener_socket(sock_path: &Path, captured: &std::fs::Metadata) -> anyhow::Result<()> {
    let current = match std::fs::symlink_metadata(sock_path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    if same_inode(captured, &current) {
        std::fs::remove_file(sock_path)?;
    }
    Ok(())
}

pub async fn run(scope: Scope) -> anyhow::Result<()> {
    let host_paths = scope.host_paths()?;
    run_with_host_paths(scope, host_paths).await
}

async fn run_timer_scheduler<T, R, F>(
    mut stop_rx: tokio::sync::mpsc::Receiver<()>,
    tick_interval_ms: u64,
    runtimes: R,
    spawn_tick: F,
) where
    T: Send + 'static,
    R: Fn() -> Vec<T> + Send + 'static,
    F: Fn(Vec<T>) -> tokio::task::JoinHandle<()> + Send + 'static,
{
    let mut interval = tokio::time::interval(Duration::from_millis(tick_interval_ms));
    let mut tick = None;
    loop {
        tokio::select! {
            _ = stop_rx.recv() => break,
            _ = interval.tick() => {
                if tick.as_ref().is_some_and(|task: &tokio::task::JoinHandle<()>| !task.is_finished()) {
                    continue;
                }
                if let Some(completed) = tick.take() {
                    let _ = completed.await;
                }
                tick = Some(spawn_tick(runtimes()));
            }
        }
    }
    if let Some(tick) = tick {
        tick.abort();
        let _ = tick.await;
    }
}

async fn run_with_host_paths(scope: Scope, host_paths: HostPaths) -> anyhow::Result<()> {
    raise_open_file_limit();
    let scope = Scope {
        root: std::fs::canonicalize(&scope.root)?,
    };
    host_paths.ensure_root()?;
    let sock_path = host_paths.socket_path();
    // The host lock is the daemon writer admission gate.
    let _lock_file = acquire_daemon_lock(&host_paths.lock_path(), &sock_path)?;

    let project_scope = GlobalState::canonical_project_scope(&scope.root)
        .map_err(|error| anyhow::anyhow!("PROJECT_SCOPE_UNKNOWN: {error}"))?;
    let resident_key = (
        crate::identity::CLI_APP_SERVER_ID.to_owned(),
        project_scope.as_str().to_owned(),
    );
    let route_file = host_paths.state_root().join("routes.json");
    let runtime_storage_root = select_central_runtime_storage(&host_paths, &route_file, &resident_key, None)
        .map_err(anyhow::Error::msg)?;
    let project_server_dir = runtime_storage_root.join(".agent-collab").join("server");

    prepare_socket_path(&sock_path)?;

    let state_path = project_server_dir.join("state.json");
    let state = current_state::load(&state_path, false).map_err(anyhow::Error::msg)?;


    append_log(&host_paths.log_path(), "server starting");

    let server = Arc::new(Server {
        config: crate::config::load(&scope.root)?,
        root: scope.root.clone(),
        storage_root: runtime_storage_root,
        state_path,
        host_paths: host_paths.clone(),
        state: Mutex::new(state),
        appserver_candidate_check: default_appserver_candidate_check(),
        appserver_notification_sink: default_appserver_notification_sink(),
        appserver_thread_status: default_appserver_thread_status(),
        appserver_thread_archive: default_appserver_thread_archive(),
        #[cfg(not(test))]
        tmux_notification_sink: default_tmux_notification_sink(),
        rex_monitor: Default::default(),
        mailbox_notify: Notify::new(),
    });
    restore_registered_peer_default_leases(&server);
    reconcile_master_goal_due(&server);
    server
        .rex_monitor
        .install_runtime_handle(tokio::runtime::Handle::current());
    // The monitor creates each Rex events task through `Handle::block_on`,
    // matching the timer path, so the initial reconcile must also run off the
    // async worker thread.
    let initial_monitor = Arc::clone(&server);
    if let Err(error) = tokio::task::spawn_blocking(move || {
        crate::server::rex_monitor::reconcile(&initial_monitor, now_ms());
    })
    .await
    {
        append_log(
            &host_paths.log_path(),
            &format!("REX_MONITOR_RECONCILE_FAILED: {error}"),
        );
    }
    purge_expired_storage(&server, now_ms());
    let runtime_manager = ProjectRuntimeManager::new(server.clone(), &host_paths)
    .map_err(|error| anyhow::anyhow!(error))?;
    let federation_ingress = match server.config.federation.clone() {
        Some(configuration) => Some(federation_listener::Ingress::open(
            host_paths.state_root().join("federation").join("rpc.sock"), configuration,
        ).map_err(anyhow::Error::msg)?),
        None => None,
    };
    // Startup convergence is background work: a slow or unreachable gateway
    // must not delay the listener or the first request.
    crate::server::binding_lifecycle::kick(&runtime_manager);
    let listener = UnixListener::bind(&sock_path)?;
    let socket_metadata = std::fs::symlink_metadata(&sock_path)?;
    use std::os::unix::fs::PermissionsExt;
    if let Err(error) = std::fs::set_permissions(&sock_path, std::fs::Permissions::from_mode(0o600))
    {
        let cleanup = remove_listener_socket(&sock_path, &socket_metadata);
        return match cleanup {
            Ok(()) => Err(error.into()),
            Err(cleanup_error) => Err(anyhow::Error::new(error).context(format!(
                "failed to clean up startup socket: {cleanup_error}"
            ))),
        };
    }
    let pid_path = host_paths.pid_path();
    std::fs::write(&pid_path, std::process::id().to_string()).map_err(|error| {
        match remove_listener_socket(&sock_path, &socket_metadata) {
            Ok(()) => anyhow::Error::new(error),
            Err(cleanup_error) => anyhow::Error::new(error).context(format!(
                "failed to clean up startup socket: {cleanup_error}"
            )),
        }
    })?;
    let pid_metadata = std::fs::symlink_metadata(&pid_path)?;
    let state_root_metadata = std::fs::symlink_metadata(host_paths.state_root())?;

    let mut interrupt = Box::pin(tokio::signal::ctrl_c());
    let mut terminate = Box::pin(async {
        let mut signal = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
        signal.recv().await;
        Ok::<(), std::io::Error>(())
    });
    let mut state_root_removed = Box::pin(async {
        let mut interval = tokio::time::interval(Duration::from_millis(250));
        interval.tick().await;
        loop {
            interval.tick().await;
            if !std::fs::symlink_metadata(host_paths.state_root())
                .is_ok_and(|current| same_inode(&state_root_metadata, &current))
            {
                return;
            }
        }
    });

    // Background scheduler: bounded waits and explicitly registered notifications.
    let sched = runtime_manager.clone();
    let cycle_manager = sched.clone();
    let (scheduler_stop, scheduler_stop_rx) = tokio::sync::mpsc::channel::<()>(1);
    let scheduler_interval_ms = sched.host.config.timers.tick_interval_ms;
    let scheduler = tokio::spawn(run_timer_scheduler(
        scheduler_stop_rx,
        scheduler_interval_ms,
        move || {
            crate::server::binding_lifecycle::kick(&sched);
            sched.runtimes()
        },
        move |runtimes| {
            let cycle_manager = cycle_manager.clone();
            tokio::task::spawn_blocking(move || {
                for runtime in runtimes {
                    crate::server::timers::tick(&runtime);
                }
                if let Err(error) = cycle_manager.tick_ceo_report_patrol(now_ms()) {
                    append_log(
                        &cycle_manager.host.log_path(),
                        &format!("CEO_REPORT_PATROL_TICK_FAILED: {error}"),
                    );
                }
            })
        },
    ));

    let mut connection_tasks = tokio::task::JoinSet::new();
    let (connection_shutdown, connection_shutdown_rx) = tokio::sync::watch::channel(false);
    let (user_shutdown_tx, mut user_shutdown_rx) = tokio::sync::mpsc::channel::<()>(1);
    tokio::select! {
        _ = user_shutdown_rx.recv() => {}
        _ = interrupt.as_mut() => {}
        _ = terminate.as_mut() => {}
        _ = state_root_removed.as_mut() => {}
        result = async {
            loop {
                while let Some(result) = connection_tasks.try_join_next() {
                    if let Err(error) = result {
                        append_log(&host_paths.log_path(), &format!("connection task failed: {error}"));
                    }
                }
                let (federated, accepted) = tokio::select! {
                    accepted = listener.accept() => (false, accepted.map(|(stream, _)| stream)),
                    accepted = async {
                        match federation_ingress.as_ref() {
                            Some(ingress) => ingress.accept().await,
                            None => std::future::pending().await,
                        }
                    } => (true, accepted),
                };
                match accepted {
                    Ok(stream) if federated => {
                        let ingress = federation_ingress.as_ref().expect("federated acceptance has an ingress");
                        connection_tasks.spawn(federation_listener::connection(
                            runtime_manager.clone(), stream, ingress.configuration(), connection_shutdown_rx.clone(),
                        ));
                    }
                    Ok(stream) => {
                        let manager = runtime_manager.clone();
                        connection_tasks.spawn(conn_task_routed(
                            manager,
                            stream,
                            connection_shutdown_rx.clone(),
                            Some(user_shutdown_tx.clone()),
                        ));
                    }
                    Err(error) => {
                        append_log(&host_paths.log_path(), &format!("accept error: {error}"));
                        tokio::time::sleep(Duration::from_secs(1)).await;
                    }
                }
            }
        } => result,
    }

    let _ = scheduler_stop.try_send(());
    let _ = scheduler.await;
    connection_shutdown.send_replace(true);
    while connection_tasks.join_next().await.is_some() {}
    for runtime in runtime_manager.runtimes() {
        runtime.rex_monitor.shutdown().await;
    }

    let _ = remove_listener_socket(&sock_path, &socket_metadata);
    if std::fs::symlink_metadata(&pid_path).is_ok_and(|current| same_inode(&pid_metadata, &current))
    {
        let _ = std::fs::remove_file(&pid_path);
    }
    Ok(())
}

#[cfg(unix)]
fn raise_open_file_limit() {
    // launchd (and an ordinary shell) gives the daemon a 256-file soft limit.
    // A host with many registered workers plus their keepalive connections
    // exhausts that ceiling and every later binding fails with EMFILE. Raise
    // the soft limit toward the hard ceiling here so a manually started daemon
    // is protected too, not only the LaunchAgent path.
    const TARGET: libc::rlim_t = 4096;
    unsafe {
        let mut limit: libc::rlimit = std::mem::zeroed();
        if libc::getrlimit(libc::RLIMIT_NOFILE, &mut limit) != 0 {
            return;
        }
        if limit.rlim_cur >= TARGET {
            return;
        }
        limit.rlim_cur = if limit.rlim_max >= TARGET {
            TARGET
        } else {
            limit.rlim_max
        };
        libc::setrlimit(libc::RLIMIT_NOFILE, &limit);
    }
}

#[cfg(not(unix))]
fn raise_open_file_limit() {}

fn reconcile_master_goal_due(server: &Server) {
    let events = {
        let mut state = server.state.lock().unwrap();
        if state.reconcile_stale_master_goal_due(now_ms()) {
            vec![Event::MasterWakeUpdated {
                accumulator: state.master_wake.clone(),
            }]
        } else {
            Vec::new()
        }
    };
    if !events.is_empty() {
        server.commit(&events);
    }
}
