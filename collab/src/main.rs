mod board;
mod dashboard;
mod client;
mod config;
mod identity;
mod install_skills;
pub(crate) mod migration;
mod proto;
mod reset;
mod scope;
mod server;
mod subagent;

use clap::Parser;
use identity::{Identity, RuntimeIdentity};
use proto::{Req, Resp, TransportKind};
use scope::Scope;
use serde::de::DeserializeOwned;
use serde_json::json;
use std::cell::Cell;

mod main_live_closure;
use main_live_closure::*;

mod main_cli;
use main_cli::*;

mod main_context;
use main_context::*;

#[derive(Parser)]
#[command(
    name = "collab",
    version = env!("COLLAB_VERSION"),
    about = "Project-local coordination for multi-agent work"
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}


fn out<T: serde::Serialize>(v: &T) {
    println!("{}", serde_json::to_string_pretty(v).unwrap());
}

/// Register an identity with the server (idempotent for the same token).
fn register(scope: &Scope, ident: &mut Identity) -> anyhow::Result<serde_json::Value> {
    let context_runtime = match ident.runtime.as_ref() {
        Some(runtime) => {
            runtime.validate()?;
            runtime.clone()
        }
        None => RuntimeIdentity::cli_adapter(&ident.worker_id)?,
    };
    register_with_runtime(scope, ident, context_runtime)
}

thread_local! {
    /// Set for the duration of one `collab context` registration so the client
    /// asks the daemon to retire a provably dead cross-project anchor instead of
    /// failing closed. The getter and the setter below must share this single
    /// cell: a second `thread_local!` in either one would be a different cell,
    /// and `Req::Register` would then always carry
    /// `retire_cross_project_anchor: false`.
    static CONTEXT_RETIRE_CROSS_PROJECT: Cell<bool> = const { Cell::new(false) };
}

fn context_registration_requested() -> bool {
    CONTEXT_RETIRE_CROSS_PROJECT.with(Cell::get)
}

pub(crate) fn set_context_registration_requested(value: bool) {
    CONTEXT_RETIRE_CROSS_PROJECT.with(|flag| flag.set(value));
}

/// Bootstrap recovery with the only runtime identity accepted before the
/// daemon has restored this worker's registered route.
fn register_recovery(scope: &Scope, ident: &mut Identity) -> anyhow::Result<serde_json::Value> {
    register_with_runtime(
        scope,
        ident,
        RuntimeIdentity::cli_adapter(&ident.worker_id)?,
    )
}

fn register_with_runtime(
    scope: &Scope,
    ident: &mut Identity,
    context_runtime: RuntimeIdentity,
) -> anyhow::Result<serde_json::Value> {
    let cwd = scope.root.display().to_string();
    // Candidate collection yields at most one channel family.
    //
    // dsh is mutually exclusive with the pane transports (design F1), and a
    // persisted binding keeps its own channel: collecting a pane candidate for
    // a persisted dsh peer would silently migrate it onto another transport.
    //
    // This CLI never produces a dsh candidate. Every dsh field comes from the
    // gateway's own registry (design section 3.1) and DSH exposes no ambient
    // agent/session identity to derive it from, so the gateway supplies the
    // candidate through its own client path (design section 7.2) instead.
    let persisted_kind = ident
        .transport
        .as_ref()
        .map(|transport| transport.kind.clone());
    let (appserver_candidate, tmux_candidate) = match persisted_kind {
        Some(proto::TransportKind::Dsh) => (None, None),
        Some(proto::TransportKind::Tmux) => (
            None,
            crate::client::adapters::tmux::candidate_from_env().ok(),
        ),
        _ => (
            crate::client::adapters::candidate_from_env().map_err(anyhow::Error::msg)?,
            crate::client::adapters::tmux::candidate_from_env().ok(),
        ),
    };
    let response: serde_json::Value = client::call_with_runtime_identity_at_root_daemon(
        &scope.sock_path(),
        &Req::Register {
            worker_id: ident.worker_id.clone(),
            token: ident.token.clone(),
            cwd,
            candidates: Some(proto::TransportCandidates {
                appserver: appserver_candidate,
                tmux: tmux_candidate,
                dsh: None,
            }),
            retire_cross_project_anchor: context_registration_requested(),
        },
        &scope.root,
        &context_runtime,
    )?;
    let (runtime, transport) =
        identity::registration_from_receipt(&response, &ident.worker_id, &scope.root)?;
    if runtime.appserver_id != context_runtime.appserver_id {
        anyhow::bail!(
            "registration receipt app scope mismatch: expected {}, observed {}",
            context_runtime.appserver_id,
            runtime.appserver_id
        );
    }
    identity::persist_registration(scope, ident, runtime, transport)?;
    Ok(response)
}

/// Identity bootstrap used by every command that acts as a worker.
fn me(scope: &Scope, worker: Option<String>) -> anyhow::Result<Identity> {
    let worker = worker.or_else(|| std::env::var("COLLAB_WORKER").ok());
    let mut ident = identity::load_or_create(scope, worker, None)?;
    if ident.runtime.is_none() || ident.transport.is_none() {
        let _ = register(scope, &mut ident)?;
    } else if !persisted_runtime_matches_scope(scope, &ident)? {
        // A persisted binding that no longer matches this session, thread, or
        // canonical cwd must not be reused: commands would then be dispatched
        // under a stale App Server address.  Re-register through the same
        // owner used by `ensure_registration`.
        register_recovery(scope, &mut ident)?;
    }
    runtime_for_request(&ident)?;
    Ok(ident)
}

/// Board operations never bootstrap an identity or launch the daemon.
fn registered_board_identity(scope: &Scope) -> anyhow::Result<Identity> {
    let identity = identity::load_existing_at(&scope::HostPaths::resolve()?, scope, None)?
        .ok_or_else(|| anyhow::anyhow!("BOARD_IDENTITY_REQUIRED: run collab context from the owning TUI before opening the board"))?;
    if !persisted_runtime_matches_scope(scope, &identity)? {
        anyhow::bail!("BOARD_STALE_IDENTITY: run collab context to repair the project binding");
    }
    runtime_for_request(&identity)?;
    Ok(identity)
}

/// How `collab context` changed an identity during automatic bootstrap.
enum RegistrationOutcome {
    Created,
    Reused,
    Recovered,
    Recreated,
}

fn ensure_registration(scope: &Scope, ident: &mut Identity) -> anyhow::Result<serde_json::Value> {
    ensure_registration_with_outcome(scope, ident).map(|(value, _)| value)
}

fn ensure_registration_with_outcome(
    scope: &Scope,
    ident: &mut Identity,
) -> anyhow::Result<(serde_json::Value, RegistrationOutcome)> {
    if ident.runtime.is_none() || ident.transport.is_none() {
        let value = register(scope, ident)?;
        Ok((value, RegistrationOutcome::Created))
    } else if !persisted_runtime_matches_scope(scope, ident)? {
        match register_recovery(scope, ident) {
            Ok(value) => Ok((value, RegistrationOutcome::Recovered)),
            Err(error) => {
                if ident.transport.as_ref().is_some_and(|transport| {
                    transport.kind == TransportKind::Tmux
                        && transport.tmux_endpoint.as_ref().is_some_and(|old| {
                            crate::client::adapters::tmux::candidate_from_env()
                                .ok()
                                .is_some_and(|current| {
                                    crate::client::adapters::tmux::same_pane_route(
                                        old,
                                        &current.endpoint,
                                    )
                                })
                        })
                }) {
                    return Err(error);
                }
                // A stale binding that can no longer be recovered in place.
                // Re-register fresh with the same worker token so the server
                // supersedes the old transport; the previous identity is
                // dropped as the active peer without a daemon restart.
                ident.runtime = None;
                ident.transport = None;
                let value = register(scope, ident)?;
                Ok((value, RegistrationOutcome::Recreated))
            }
        }
    } else {
        runtime_for_request(ident)?;
        Ok((json!({"reused": true}), RegistrationOutcome::Reused))
    }
}

fn persisted_runtime_matches_scope(scope: &Scope, ident: &Identity) -> anyhow::Result<bool> {
    let runtime = ident
        .runtime
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("identity has no registered runtime binding"))?;
    // A missing transport cannot be reused, but it is a normal legacy shape
    // rather than an error: report "not reusable" so the caller re-registers.
    let Some(transport) = ident.transport.as_ref() else {
        return Ok(false);
    };
    if transport.kind == TransportKind::Tmux {
        let Some(persisted_endpoint) = transport.tmux_endpoint.as_ref() else {
            return Ok(false);
        };
        let Ok(current) = crate::client::adapters::tmux::candidate_from_env() else {
            return Ok(false);
        };
        if !crate::client::adapters::tmux::same_pane_route(&current.endpoint, persisted_endpoint) {
            return Ok(false);
        }
        if current.endpoint.codex_session_id.as_deref()
            != persisted_endpoint.codex_session_id.as_deref()
            || current.endpoint.codex_thread_id.as_deref()
                != persisted_endpoint.codex_thread_id.as_deref()
        {
            return Ok(false);
        }
    } else if runtime.native_thread_id.is_some() {
        let session = std::env::var("CODEX_SESSION_ID").ok();
        let thread = std::env::var("CODEX_THREAD_ID").ok();
        if let (Some(session), Some(thread)) = (session, thread) {
            let persisted_thread = runtime
                .native_thread_id
                .as_ref()
                .map(crate::identity::NativeThreadId::as_str);
            let persisted_session = runtime
                .session_id
                .as_ref()
                .map(crate::identity::SessionId::as_str);
            if persisted_thread != Some(thread.as_str())
                || persisted_session != Some(session.as_str())
            {
                return Ok(false);
            }
        }
    }
    let host_paths = scope.host_paths()?;
    let scope_root = std::fs::canonicalize(&scope.root)?;
    if ident
        .project_scope
        .as_ref()
        .is_none_or(|project_scope| project_scope.as_str() != scope_root.to_string_lossy())
    {
        return Ok(false);
    }
    Ok(
        scope::canonical_route_for_identity(&host_paths, &scope.root, &runtime.appserver_id)
            .is_ok_and(|route| route.root == scope_root),
    )
    .and_then(|file_says_reusable| {
        if !file_says_reusable {
            return Ok(false);
        }
        // The route journal on disk is not the live truth: after a daemon
        // restart or re-registration the running daemon may hold no route for
        // this address even though the file still lists one.  Reuse is only
        // valid when the live daemon resolves the same session/thread.
        match ident.transport.as_ref() {
            Some(transport) if transport.kind == crate::proto::TransportKind::Tmux => {
                let Some(endpoint) = transport.tmux_endpoint.as_ref() else {
                    return Ok(false);
                };
                Ok(client::resolve_route(&scope.sock_path(), endpoint).is_ok())
            }
            Some(transport) if transport.kind == crate::proto::TransportKind::AppServer => {
                let (Some(session), Some(thread)) = (
                    runtime
                        .session_id
                        .as_ref()
                        .map(crate::identity::SessionId::as_str),
                    runtime
                        .native_thread_id
                        .as_ref()
                        .map(crate::identity::NativeThreadId::as_str),
                ) else {
                    return Ok(false);
                };
                if transport.thread_id.as_deref() != Some(thread)
                    || transport.session_id.as_deref() != Some(session)
                {
                    return Ok(false);
                }
                // The persisted route file is not live truth: after a daemon
                // restart or re-registration the running daemon may hold no
                // route for this address. Reuse is only valid when the live
                // daemon resolves the same session/thread.
                Ok(client::resolve_native_route(&scope.sock_path(), session, thread).is_ok())
            }
            _ => Ok(false),
        }
    })
}

fn runtime_for_request<'a>(ident: &'a Identity) -> anyhow::Result<&'a RuntimeIdentity> {
    let runtime = ident.runtime.as_ref().ok_or_else(|| {
        anyhow::anyhow!(
            "identity has no registered runtime binding; register the current peer before making a project request"
        )
    })?;
    runtime.validate()?;
    if runtime.agent_id.as_str() != ident.worker_id {
        anyhow::bail!(
            "runtime binding agent does not match identity worker: expected {}, observed {}",
            ident.worker_id,
            runtime.agent_id
        );
    }
    Ok(runtime)
}

fn registered_runtime_projection(
    scope: &Scope,
    ident: &Identity,
    daemon_pid: u32,
) -> anyhow::Result<serde_json::Value> {
    let runtime = ident
        .runtime
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("registered identity is missing its runtime"))?;
    let transport = ident
        .transport
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("registered identity is missing its transport"))?;
    if daemon_pid == 0 {
        anyhow::bail!("registered daemon PID is zero");
    }
    let project_root = std::fs::canonicalize(&scope.root)?;
    match transport.kind {
        crate::proto::TransportKind::Tmux => {
            let endpoint = transport.tmux_endpoint.as_ref().ok_or_else(|| {
                anyhow::anyhow!("registered tmux transport is missing its endpoint")
            })?;
            let expected_session = endpoint
                .codex_session_id
                .as_deref()
                .unwrap_or(&endpoint.tmux_session_id);
            let expected_thread = endpoint
                .codex_thread_id
                .as_deref()
                .unwrap_or(&endpoint.pane_id);
            if runtime
                .session_id
                .as_ref()
                .map(ToString::to_string)
                .as_deref()
                != Some(expected_session)
                || runtime
                    .native_thread_id
                    .as_ref()
                    .map(ToString::to_string)
                    .as_deref()
                    != Some(expected_thread)
            {
                anyhow::bail!("registered tmux endpoint does not match its runtime route");
            }
            Ok(json!({
                "runtimeId": runtime.runtime_id,
                "appserverId": runtime.appserver_id,
                "transport": "tmux",
                "tmuxEndpoint": endpoint,
                "projectRoot": project_root,
                "capabilities": transport.capabilities,
                "processId": daemon_pid,
            }))
        }
        crate::proto::TransportKind::AppServer => {
            let session = transport.session_id.as_deref().ok_or_else(|| {
                anyhow::anyhow!("registered App Server transport is missing its session id")
            })?;
            let thread = transport.thread_id.as_deref().ok_or_else(|| {
                anyhow::anyhow!("registered App Server transport is missing its thread id")
            })?;
            if runtime
                .session_id
                .as_ref()
                .map(ToString::to_string)
                .as_deref()
                != Some(session)
                || runtime
                    .native_thread_id
                    .as_ref()
                    .map(ToString::to_string)
                    .as_deref()
                    != Some(thread)
            {
                anyhow::bail!("registered App Server transport does not match its runtime route");
            }
            Ok(json!({
                "runtimeId": runtime.runtime_id,
                "appserverId": runtime.appserver_id,
                "transport": "appserver",
                "endpoint": transport.endpoint,
                "namespace": transport.namespace,
                "sessionId": session,
                "threadId": thread,
                "projectRoot": project_root,
                "capabilities": transport.capabilities,
                "processId": daemon_pid,
            }))
        }
        crate::proto::TransportKind::Dsh => {
            let endpoint = transport.endpoint.as_deref().ok_or_else(|| {
                anyhow::anyhow!("registered dsh transport is missing its gateway endpoint")
            })?;
            let gateway_runtime = transport.namespace.as_deref().ok_or_else(|| {
                anyhow::anyhow!("registered dsh transport is missing its gateway runtime id")
            })?;
            let agent = transport.thread_id.as_deref().ok_or_else(|| {
                anyhow::anyhow!("registered dsh transport is missing its agent id")
            })?;
            let session = transport.session_id.as_deref().ok_or_else(|| {
                anyhow::anyhow!("registered dsh transport is missing its session id")
            })?;
            // The route key is reused, not new: the dsh agent id is carried as
            // `native_thread_id` and the dsh session id as `session_id`, so the
            // typed binding is checked the same way as the other transports.
            if runtime
                .session_id
                .as_ref()
                .map(ToString::to_string)
                .as_deref()
                != Some(session)
                || runtime
                    .native_thread_id
                    .as_ref()
                    .map(ToString::to_string)
                    .as_deref()
                    != Some(agent)
            {
                anyhow::bail!("registered dsh transport does not match its runtime route");
            }
            Ok(json!({
                "runtimeId": runtime.runtime_id,
                "appserverId": runtime.appserver_id,
                "transport": "dsh",
                "endpoint": endpoint,
                "gatewayRuntimeId": gateway_runtime,
                "agentId": agent,
                "sessionId": session,
                "projectRoot": project_root,
                "capabilities": transport.capabilities,
                "processId": daemon_pid,
            }))
        }
    }
}

fn call_project<T: DeserializeOwned>(
    scope: &Scope,
    ident: &Identity,
    request: &Req,
) -> anyhow::Result<T> {
    let runtime = runtime_for_request(ident)?;
    client::call_with_runtime_identity_at_root(&scope.sock_path(), request, &scope.root, runtime)
}

fn main() {
    let cli = Cli::parse();
    if let Err(e) = run(cli.cmd) {
        eprintln!("collab: {}", format_cli_error(&e.to_string()));
        if let Some(server_error) = e.downcast_ref::<client::ServerResponseError>() {
            eprintln!("collab response: {}", serde_json::json!(&server_error.response));
        }
        std::process::exit(1);
    }
}

const LEGACY_ROUTE_RESOLVE_NOT_FOUND_RECOVERY: &str = "recovery: run `collab context` from the canonical project main checkout to resolve the route and restore registration; do not re-register a worktree or edit routes.jsonl";
const LEGACY_ROUTE_RESOLVE_NOT_FOUND_UPGRADE: &str = "recovery: run `collab context` from the canonical project main checkout; preserve daemon state and do not start a second daemon or use mailbox state as transport delivery";
const IDENTITY_REBIND_UNPROVEN_RECOVERY: &str = "recovery: run `collab context` from the canonical project main checkout; if the same identity error persists, report the exact error and worker_id to the live master with `COLLAB_WORKER=<worker_id> collab sendmessage --from <worker_id> --to <master> --subject blocker \"<exact error; worker_id=<worker_id>; cause; decision needed>\"`; if that also fails, report out-of-band through a healthy peer or the human; do not edit routes, copy tokens, or start a second daemon";
const IDENTITY_RESTORE_CROSS_PROJECT_RECOVERY: &str = "recovery: run `collab context` from the canonical project main checkout for the current project; if it persists, preserve the exact error and worker_id and report out-of-band to the live master through a healthy peer or the human; do not try `collab sendmessage` through the same failing identity path; do not edit routes, copy tokens, or start a second daemon";

include!("main_error_format.rs");

fn run(cmd: Cmd) -> anyhow::Result<()> {
    match cmd {
        Cmd::Config => {
            crate::config::ensure_written()?;
            out(&crate::config::load(&scope::lifecycle_project_root()?)?);
            Ok(())
        }
        Cmd::Subagent { command } => {
            if matches!(&command, subagent::Action::Start { .. }) {
                anyhow::bail!("MANAGED_SUBAGENT_UNSUPPORTED: tmux cannot create a Codex thread; start the peer in its own tmux pane and register that pane");
            }
            let scope = Scope::resolve()?;
            let query = subagent_observe_query(&command);
            if let Some((id, snapshot_lines)) = query {
                let value: serde_json::Value = client::call_with_context(
                    &scope.sock_path(),
                    &Req::SubagentObserve { id, snapshot_lines },
                    Some(cli_project_context(&scope.root)?),
                )?;
                out(&value);
                return Ok(());
            }
            Ok(())
        }
        Cmd::Who => {
            let scope = Scope::resolve()?;
            let v: serde_json::Value = client::call_with_context(
                &scope.sock_path(),
                &Req::Workers,
                Some(cli_project_context(&scope.root)?),
            )?;
            out(&v);
            Ok(())
        }
        Cmd::Init { worker_id } => {
            let project_root = scope::project_root_for_init()?;
            if project_root.ancestors().skip(1).any(|ancestor| {
                ancestor
                    .file_name()
                    .is_some_and(|name| name == "playground")
            }) {
                anyhow::bail!(
                    "collab init must run from the project main tree, not a ./playground worktree"
                );
            }
            let _base = scope::init(&project_root)?;
            let scope = Scope { root: project_root };
            let started = !client::alive(&scope.sock_path());
            client::ensure_server(&scope.sock_path())?;
            let mut ident = identity::load_or_create_for_init(&scope, worker_id)?;
            let registration = ensure_registration(&scope, &mut ident)?;
            let daemon_pid = std::fs::read_to_string(scope.host_paths()?.pid_path())
                .map_err(|error| anyhow::anyhow!("registered daemon PID is unavailable: {error}"))?
                .trim()
                .parse::<u32>()
                .map_err(|error| anyhow::anyhow!("registered daemon PID is invalid: {error}"))?;
            let runtime = registered_runtime_projection(&scope, &ident, daemon_pid)?;
            let task_board: serde_json::Value =
                call_project(&scope, &ident, &Req::TaskStatus { task_id: None })?;
            out(&json!({
                "ok": true,
                "root": scope.root,
                "worker_id": ident.worker_id,
                "identity_kind": "peer",
                "runtime": runtime,
                "transport_selected": ident.transport,
                "daemon_started": started,
                "role_brief": registration["role_brief"],
                "task_board": task_board["tasks"],
                "recovery_action": registration["role_brief"]["communication_recovery"]
            }));
            Ok(())
        }
        Cmd::Serve => {
            let scope = Scope {
                root: scope::lifecycle_project_root()?,
            };
            let rt = tokio::runtime::Runtime::new()?;
            rt.block_on(server::run(scope))
        }
        Cmd::Up => {
            let project_root = scope::lifecycle_project_root()?;
            if !project_root.join(".agent-collab").is_dir() {
                scope::init(&project_root)?;
            }
            let scope = Scope { root: project_root };
            let host_server_dir = scope.host_server_dir();
            std::fs::create_dir_all(&host_server_dir)?;
            std::fs::remove_file(host_server_dir.join("DOWN")).ok();
            client::record_event(
                &scope.sock_path(),
                "daemon_up_requested",
                json!({"pid": std::process::id()}),
            );
            let sock = scope.sock_path();
            let was_running = client::alive(&sock);
            client::ensure_server(&sock)?;
            out(&json!({"ok": true, "server": sock, "started": !was_running}));
            Ok(())
        }
        Cmd::Down => {
            let scope = Scope {
                root: scope::lifecycle_project_root()?,
            };
            if client::alive(&scope.sock_path()) {
                let _: serde_json::Value = client::call_with_context(
                    &scope.sock_path(),
                    &Req::Shutdown { operator: true },
                    Some(cli_project_context(&scope.root)?),
                )?;
            }
            let server_dir = scope.host_server_dir();
            std::fs::create_dir_all(&server_dir)?;
            std::fs::write(server_dir.join("DOWN"), b"explicitly stopped\n")?;
            client::record_event(
                &scope.sock_path(),
                "daemon_down_requested",
                json!({"pid": std::process::id()}),
            );
            let pid_path = server_dir.join("server.pid");
            if client::alive(&scope.sock_path()) {
                let mut pids = Vec::new();
                let output = std::process::Command::new("lsof")
                    .args(["-t", scope.sock_path().to_str().unwrap_or_default()])
                    .output()?;
                for line in String::from_utf8_lossy(&output.stdout).lines() {
                    if let Ok(pid) = line.trim().parse::<i32>() {
                        pids.push(pid);
                    }
                }
                if pids.is_empty() {
                    if let Ok(pid_text) = std::fs::read_to_string(&pid_path) {
                        if let Ok(pid) = pid_text.trim().parse::<i32>() {
                            pids.push(pid);
                        }
                    }
                }
                pids.sort_unstable();
                pids.dedup();
                for pid in pids {
                    if pid > 1 && pid != std::process::id() as i32 {
                        let status = std::process::Command::new("kill")
                            .args(["-TERM", &pid.to_string()])
                            .status()?;
                        if !status.success() {
                            anyhow::bail!("failed to stop collab daemon pid {}", pid);
                        }
                    }
                }
                for _ in 0..40 {
                    if !client::alive(&scope.sock_path()) {
                        break;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }
                if client::alive(&scope.sock_path()) {
                    anyhow::bail!(
                        "collab daemon did not stop at {}",
                        scope.sock_path().display()
                    );
                }
            }
            out(&json!({"ok": true, "down": true, "server": scope.sock_path()}));
            Ok(())
        }
        Cmd::Status { all } => {
            let scope = Scope::resolve()?;
            let v: serde_json::Value = if all {
                client::call_with_context(
                    &scope.sock_path(),
                    &Req::StatusAll,
                    Some(cli_project_context(&scope.root)?),
                )?
            } else {
                client::call(&scope.sock_path(), &Req::Ping)?
            };
            out(&v);
            Ok(())
        }
        Cmd::Mailbox { cmd } => {
            let scope = Scope::resolve()?;
            match cmd {
                MailboxCmd::Read { all, sort, worker } => {
                    let explicit_worker = worker.is_some();
                    let actorless = all || explicit_worker;
                    let mut identity = None;
                    let worker_id = if actorless {
                        worker
                    } else {
                        let ident = me(&scope, None)?;
                        let worker_id = ident.worker_id.clone();
                        identity = Some(ident);
                        Some(worker_id)
                    };
                    let request = Req::MailboxRead {
                        all,
                        sort: Some(sort),
                        worker_id,
                    };
                    let v: serde_json::Value = if actorless {
                        client::call_with_context(
                            &scope.sock_path(),
                            &request,
                            Some(cli_project_context(&scope.root)?),
                        )?
                    } else {
                        let ident = identity
                            .as_ref()
                            .ok_or_else(|| anyhow::anyhow!("mailbox identity was not resolved"))?;
                        call_project(&scope, ident, &request)?
                    };
                    out(&v);
                    Ok(())
                }
            }
        }
        Cmd::Role => {
            anyhow::bail!("collab role is deprecated; declared roles were removed")
        }
        Cmd::Route {
            command:
                RouteCmd::Resolve {
                    session_id,
                    pane_id,
                },
        } => {
            let host_paths = scope::HostPaths::resolve()?;
            let candidate =
                crate::client::adapters::tmux::candidate_from_env().map_err(anyhow::Error::msg)?;
            if session_id
                .as_deref()
                .is_some_and(|value| value != candidate.endpoint.tmux_session_id)
                || pane_id
                    .as_deref()
                    .is_some_and(|value| value != candidate.endpoint.pane_id)
            {
                anyhow::bail!(
                    "ROUTE_RESOLVE_INVALID: requested identity does not match current tmux pane"
                );
            }
            let route =
                crate::client::resolve_route(&host_paths.socket_path(), &candidate.endpoint)?;
            out(&json!({
                "canonical_root": route.canonical_root,
                "storage_root": route.storage_root,
                "app_scope_id": route.app_scope_id,
                "project_scope": route.project_scope,
                "agent_id": route.agent_id,
                "binding_id": route.binding_id,
                "endpoint_generation": route.endpoint_generation,
                "session_id": route.session_id,
                "native_thread_id": route.native_thread_id,
                "tmux_endpoint": candidate.endpoint,
            }));
            Ok(())
        }
        Cmd::LiveClosure {
            command:
                LiveClosureCmd::Probe {
                    closure_id,
                    source_commit,
                    artifact_hash,
                    environment_id,
                    path,
                    to,
                    to_project,
                },
        } => {
            let scope = Scope::resolve()?;
            live_closure_probe(
                &scope,
                closure_id,
                source_commit,
                artifact_hash,
                environment_id,
                path,
                to,
                to_project,
            )
        }
        Cmd::LiveClosure {
            command:
                LiveClosureCmd::Observe {
                    to,
                    challenge,
                    message_id,
                },
        } => {
            let scope = Scope::resolve()?;
            live_closure_observe(&scope, to, challenge, message_id)
        }
        Cmd::Root { command } | Cmd::Master { command } => {
            if matches!(command, MasterCmd::Status) {
                let host_paths = scope::HostPaths::resolve()?;
                let route = scope::route_for_tmux_pane(&host_paths)?;
                let scope = Scope { root: route.root };
                let v: serde_json::Value = client::call_with_context(
                    &scope.sock_path(),
                    &Req::MasterStatus,
                    Some(proto::ProjectContext::for_registered_root_with_app(
                        &scope.root,
                        route.app_scope_id,
                    )?),
                )?;
                out(&v);
                return Ok(());
            }
            let scope = Scope::resolve()?;
            let ident = me(&scope, None)?;
            let req = match command {
                MasterCmd::Recover => {
                    anyhow::bail!(
                        "collab master recover is deprecated; use collab master promote or delegate"
                    )
                }
                MasterCmd::Promote { approval } => Req::MasterPromote {
                    worker_id: ident.worker_id.clone(),
                    token: ident.token.clone(),
                    approval,
                },
                MasterCmd::Delegate { target } => Req::MasterDelegate {
                    worker_id: ident.worker_id.clone(),
                    token: ident.token.clone(),
                    target_id: target,
                },
                MasterCmd::Send {
                    project,
                    to,
                    subject,
                    body,
                } => {
                    let local: serde_json::Value =
                        call_project(&scope, &ident, &Req::MasterStatus)?;
                    let Some(master) = local.get("master") else {
                        anyhow::bail!("cross-project send requires this peer to be a live master")
                    };
                    if master.get("worker_id").and_then(|v| v.as_str())
                        != Some(ident.worker_id.as_str())
                        || master.get("endpoint_live").and_then(|v| v.as_bool()) != Some(true)
                    {
                        anyhow::bail!("cross-project send requires this peer to be the live master")
                    }
                    let target = project.canonicalize()?;
                    if target == scope.root.canonicalize()? {
                        anyhow::bail!("cross-project send requires a different project")
                    }
                    if !target.join(".agent-collab").is_dir() {
                        anyhow::bail!("target project has no .agent-collab: {}", target.display())
                    }
                    let target_scope = Scope { root: target };
                    let value: serde_json::Value = client::call_with_context(
                        &target_scope.sock_path(),
                        &Req::CrossProjectSend {
                            from: ident.worker_id.clone(),
                            from_project: scope.root.display().to_string(),
                            source_master_assigned_by: master
                                .get("assigned_by")
                                .and_then(|v| v.as_str())
                                .unwrap_or_default()
                                .to_string(),
                            source_master_approval: master
                                .get("approval")
                                .and_then(|v| v.as_str())
                                .map(str::to_owned),
                            source_master_assigned_ms: master
                                .get("assigned_ms")
                                .and_then(|v| v.as_i64())
                                .unwrap_or_default(),
                            to,
                            subject,
                            body: body.join(" "),
                            in_reply_to: None,
                        },
                        Some(cli_project_context(&target_scope.root)?),
                    )?;
                    out(&value);
                    return Ok(());
                }
                MasterCmd::Status => unreachable!("handled above"),
            };
            let v: serde_json::Value = call_project(&scope, &ident, &req)?;
            out(&v);
            Ok(())
        }
        Cmd::Worker { cmd } => {
            let scope = match cmd {
                WorkerCmd::Recover => scope::resolve_for_recovery()?,
                _ => Scope::resolve()?,
            };
            match cmd {
                WorkerCmd::Recover => {
                    let mut ident = identity::load_or_create(&scope, None, None)?;
                    let _ = register_recovery(&scope, &mut ident)?;
                    out(&json!({
                        "recovered": true,
                        "worker_id": ident.worker_id,
                        "transport": ident.transport,
                        "identity_kind": "peer",
                        "next": "run collab who and collab task status; task ownership is unchanged"
                    }));
                    Ok(())
                }
                WorkerCmd::Status { id } => {
                    let ident = me(&scope, None)?;
                    let v: serde_json::Value =
                        call_project(&scope, &ident, &Req::WorkerStatus { worker_id: id })?;
                    out(&v);
                    Ok(())
                }
                WorkerCmd::Snapshot { id, lines } => {
                    let ident = me(&scope, None)?;
                    let v: serde_json::Value = call_project(
                        &scope,
                        &ident,
                        &Req::WorkerSnapshot {
                            worker_id: ident.worker_id.clone(),
                            token: ident.token.clone(),
                            target_id: id,
                            lines,
                        },
                    )?;
                    out(&v);
                    Ok(())
                }
                WorkerCmd::Close { id, reason } => {
                    let ident = me(&scope, None)?;
                    let v: serde_json::Value = call_project(
                        &scope,
                        &ident,
                        &Req::WorkerClose {
                            worker_id: ident.worker_id.clone(),
                            token: ident.token.clone(),
                            target_id: id,
                            reason,
                        },
                    )?;
                    out(&v);
                    Ok(())
                }
            }
        }
        Cmd::TransferMaster { target } => {
            let _ = target;
            anyhow::bail!("collab transfer-master is deprecated; use collab master delegate")
        }
        Cmd::RemoveWorker { target, force } => {
            let _ = (target, force);
            anyhow::bail!(
                "collab remove-worker is deprecated; use owner cleanup and migration verify"
            )
        }
        Cmd::Reset {
            approval,
            discard_legacy,
            routes,
            project,
            host,
            storage_root,
            keep,
            include_runs,
        } => {
            let level = reset::ResetLevel::select(routes, project, host)?;
            let root = scope::project_root_for_init()?;
            let scope = Scope { root };
            let host_paths = scope::HostPaths::resolve()?;
            reset::run(
                &scope,
                &host_paths,
                reset::ResetRequest {
                    approval: approval.unwrap_or_default(),
                    discard_legacy,
                    level,
                    storage_root,
                    keep,
                    include_runs,
                },
            )
        }
        Cmd::Whoami { worker } => {
            let scope = Scope::resolve()?;
            let mut ident = identity::load_or_create(&scope, worker, None)?;
            let registration = ensure_registration(&scope, &mut ident)?;
            let mut response = serde_json::to_value(&ident)?;
            response["role_brief"] = registration["role_brief"].clone();
            out(&response);
            Ok(())
        }
        Cmd::Send {
            from,
            to,
            subject,
            r#type,
            in_reply_to,
            delivery,
            body,
        } => {
            let scope = Scope::resolve()?;
            let ident = me(&scope, None)?;
            if from
                .as_deref()
                .is_some_and(|requested| requested != ident.worker_id)
            {
                anyhow::bail!("--from must match the authenticated worker identity");
            }
            let body = body.join(" ");
            if body.is_empty() {
                anyhow::bail!("empty message body");
            }
            let command = command_envelope(&scope, &ident)?;
            let v: serde_json::Value = call_project(
                &scope,
                &ident,
                &Req::Send {
                    from: ident.worker_id.clone(),
                    worker_id: Some(ident.worker_id.clone()),
                    token: Some(ident.token.clone()),
                    command: Some(command),
                    to,
                    mtype: r#type,
                    subject: Some(subject),
                    body,
                    in_reply_to,
                    delivery,
                },
            )?;
            out(&v);
            Ok(())
        }
        Cmd::Notify { cmd } => {
            let scope = Scope::resolve()?;
            let ident = me(&scope, None)?;
            let request = match cmd {
                NotifyCmd::Methods => Req::NotificationMethods,
                NotifyCmd::Subscribe {
                    event,
                    subject,
                    at_ms,
                    every_ms,
                    repeat_count,
                    trigger_ms,
                    ttl_seconds,
                } => Req::NotificationSubscribe {
                    worker_id: ident.worker_id.clone(),
                    token: ident.token.clone(),
                    event,
                    subject,
                    trigger_ms,
                    trigger_times_ms: at_ms,
                    interval_ms: every_ms,
                    repeat_count,
                    ttl_seconds,
                },
                NotifyCmd::Status => Req::NotificationStatus {
                    worker_id: ident.worker_id.clone(),
                    token: ident.token.clone(),
                },
                NotifyCmd::Unsubscribe { subscription_id } => Req::NotificationUnsubscribe {
                    worker_id: ident.worker_id.clone(),
                    token: ident.token.clone(),
                    subscription_id,
                },
            };
            let value: serde_json::Value = call_project(&scope, &ident, &request)?;
            out(&value);
            Ok(())
        }
        Cmd::Recv {
            timeout,
            worker,
            receive_id,
        } => {
            let scope = Scope::resolve()?;
            let ident = me(&scope, worker)?;
            let receive_id = receive_id.unwrap_or_else(new_receive_id);
            // The caller must know the receive identity before consumption, not
            // only after a successful reply: a lost socket response still
            // leaves the exact replay command available on stderr.
            let v =
                recv_with_published_identity(&mut std::io::stderr(), &receive_id, |receive_id| {
                    call_project(
                        &scope,
                        &ident,
                        &recv_request(
                            ident.worker_id.clone(),
                            ident.token.clone(),
                            timeout,
                            receive_id.to_owned(),
                        ),
                    )
                })?;
            out(&v);
            Ok(())
        }
        Cmd::Inbox { worker } => {
            let scope = Scope::resolve()?;
            let ident = me(&scope, worker)?;
            let v: serde_json::Value = call_project(
                &scope,
                &ident,
                &Req::Inbox {
                    worker_id: ident.worker_id.clone(),
                    token: ident.token.clone(),
                },
            )?;
            out(&v);
            Ok(())
        }
        Cmd::Context { worker } => {
            out(&context_snapshot(worker)?);
            Ok(())
        }
        Cmd::Ack { ids, worker, all } => {
            if ids.is_empty() && !all {
                anyhow::bail!("usage: collab ack <msg_id>... [--all] [--worker <id>]");
            }
            let scope = Scope::resolve()?;
            let ident = me(&scope, worker)?;
            let v: serde_json::Value = call_project(
                &scope,
                &ident,
                &Req::Ack {
                    worker_id: ident.worker_id.clone(),
                    token: ident.token.clone(),
                    ids,
                },
            )?;
            out(&v);
            Ok(())
        }
        Cmd::Msg { msg_id } => {
            let scope = Scope::resolve()?;
            let ident = me(&scope, None)?;
            let v: serde_json::Value = call_project(&scope, &ident, &Req::MsgStatus { msg_id })?;
            out(&v);
            Ok(())
        }
        Cmd::Dashboard { port } => {
            let scope = Scope::resolve()?;
            let identity = registered_board_identity(&scope)?;
            let runtime = runtime_for_request(&identity)?;
            let context = proto::ProjectContext::for_registered_root_with_app(&scope.root, runtime.appserver_id.clone())?;
            dashboard::run(scope.sock_path(), context, port)
        }
        Cmd::Board { cmd } => {
            let scope = Scope::resolve()?;
            let ident = registered_board_identity(&scope)?;
            let request = match cmd {
                board::BoardCommand::Show => Req::BoardShow,
                command => Req::Board { worker_id: ident.worker_id.clone(), token: ident.token.clone(), command },
            };
            let value: serde_json::Value = call_project(&scope, &ident, &request)?;
            out(&value);
            Ok(())
        }
        Cmd::Task { cmd } => {
            let scope = Scope::resolve()?;
            let ident = me(&scope, None)?;
            let worker_id = ident.worker_id.clone();
            let token = ident.token.clone();
            let req = match cmd {
                TaskCmd::Register {
                    id,
                    owner,
                    feature,
                    worktree,
                    branch,
                    base_commit,
                    priority,
                    next,
                    goal,
                } => Req::TaskRegister {
                    worker_id: worker_id.clone(),
                    token: token.clone(),
                    task_id: id,
                    owner,
                    feature_id: feature,
                    worktree_path: worktree,
                    branch,
                    base_commit,
                    priority: priority.unwrap_or_else(crate::server::state::default_priority),
                    next_step: next,
                    goal_prompt: goal,
                },
                TaskCmd::Update { id, status, next } => Req::TaskUpdate {
                    worker_id: worker_id.clone(),
                    token: token.clone(),
                    task_id: id,
                    status,
                    next_step: next,
                },
                TaskCmd::Accept { id, expected_revision } => match expected_revision {
                    Some(expected_revision) => Req::Board {
                        worker_id: worker_id.clone(), token: token.clone(),
                        command: board::BoardCommand::Respond { id, accept: true, decline: false, expected_revision, reason: None },
                    },
                    None => Req::TaskAccept { worker_id: worker_id.clone(), token: token.clone(), task_id: id },
                },
                TaskCmd::Decline { id, expected_revision, reason, legacy_assignment } => Req::Board {
                    worker_id: worker_id.clone(), token: token.clone(),
                    command: if legacy_assignment {
                        board::BoardCommand::DeclineAssigned { id, expected_revision, reason }
                    } else {
                        board::BoardCommand::Respond { id, accept: false, decline: true, expected_revision, reason: Some(reason) }
                    },
                },
                TaskCmd::Relocate {
                    id,
                    worktree,
                    branch,
                    base_commit,
                } => Req::TaskRelocate {
                    worker_id: worker_id.clone(),
                    token: token.clone(),
                    task_id: id,
                    worktree_path: worktree,
                    branch,
                    base_commit,
                },
                TaskCmd::Claim { id } => Req::TaskClaim {
                    worker_id: worker_id.clone(),
                    token: token.clone(),
                    task_id: id,
                },
                TaskCmd::Wait { id, blocking_task } => Req::TaskWait {
                    worker_id: worker_id.clone(),
                    token: token.clone(),
                    task_id: id,
                    blocking_task_id: blocking_task,
                },
                TaskCmd::Deliver {
                    id,
                    evidence,
                    worktree,
                } => Req::TaskDeliver {
                    worker_id: worker_id.clone(),
                    token: token.clone(),
                    task_id: id,
                    evidence: Some(evidence),
                    worktree: Some(worktree),
                },
                TaskCmd::Review {
                    id,
                    accept,
                    rework,
                    evidence,
                } => Req::TaskReview {
                    worker_id: worker_id.clone(),
                    token: token.clone(),
                    task_id: id,
                    accept,
                    rework,
                    evidence,
                },
                TaskCmd::Integrated {
                    id,
                    commit,
                    evidence,
                } => Req::TaskIntegrated {
                    worker_id: worker_id.clone(),
                    token: token.clone(),
                    task_id: id,
                    commit,
                    evidence,
                },
                TaskCmd::Block { id, next } => Req::TaskUpdate {
                    worker_id: worker_id.clone(),
                    token: token.clone(),
                    task_id: id,
                    status: Some("blocked".into()),
                    next_step: next,
                },
                TaskCmd::Close { id, force, reason } => Req::TaskClose {
                    worker_id: worker_id.clone(),
                    token: token.clone(),
                    task_id: id,
                    force,
                    reason,
                },
                TaskCmd::FinalizeCleanup { id } => Req::TaskFinalizeCleanup {
                    worker_id: worker_id.clone(),
                    token: token.clone(),
                    task_id: id,
                },
                TaskCmd::Dispatch => Req::TaskDispatch {
                    worker_id: worker_id.clone(),
                    token: token.clone(),
                },
                TaskCmd::Status { id } => Req::TaskStatus { task_id: id },
            };
            let v: serde_json::Value = call_project(&scope, &ident, &req)?;
            out(&v);
            Ok(())
        }
        Cmd::Migrate { cmd } => {
            let scope = Scope::resolve()?;
            let ident = me(&scope, None)?;
            let worker_id = ident.worker_id.clone();
            let token = ident.token.clone();
            let req = match cmd {
                MigrateCmd::Inspect => Req::MigrationInspect {
                    worker_id: worker_id.clone(),
                    token: token.clone(),
                },
                MigrateCmd::Plan => Req::MigrationPlan {
                    worker_id: worker_id.clone(),
                    token: token.clone(),
                },
                MigrateCmd::Apply => Req::MigrationApply {
                    worker_id: worker_id.clone(),
                    token: token.clone(),
                },
                MigrateCmd::Verify => Req::MigrationVerify {
                    worker_id: worker_id.clone(),
                    token: token.clone(),
                },
            };
            let v: serde_json::Value = call_project(&scope, &ident, &req)?;
            out(&v);
            Ok(())
        }
        Cmd::InstallSkills { target, force } => {
            let target = match target {
                Some(path) => path,
                None => match std::env::var_os("HOME") {
                    Some(home) => std::path::PathBuf::from(home)
                        .join(".agents")
                        .join("skills")
                        .join("collab"),
                    None => {
                        anyhow::bail!("install-skills default target requires $HOME; pass --target")
                    }
                },
            };
            let (outcomes, bytes, count) =
                install_skills::install(&target, force).map_err(|error| anyhow::anyhow!(error))?;
            let written = outcomes
                .iter()
                .filter(|(_, o)| *o == install_skills::InstallOutcome::Written)
                .count();
            let skipped = count - written;
            let files: Vec<&str> = outcomes.iter().map(|(r, _)| *r).collect();
            out(&serde_json::json!({
                "target": target,
                "files": files,
                "written": written,
                "skipped": skipped,
                "bytes": bytes,
                "force": force,
                "next": "the collab skill is now visible to any agent that loads ~/.agents/skills; restart the agent or rerun its skill discovery to pick up the bundle",
            }));
            Ok(())
        }
    }
}

// keep Resp referenced so the type stays part of the public surface for tests
#[allow(dead_code)]
fn _unused(_r: Resp) {}

#[cfg(test)]
#[path = "main_tests.rs"]
mod tests;
