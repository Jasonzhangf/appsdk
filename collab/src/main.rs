mod board;
pub mod board_patrol_policy;
mod ceo;
mod ceo_report_state;
mod ceo_reporting_policy;
mod ceo_workspace_guidance;
mod client;
mod config;
mod context_operation;
mod dashboard;
mod desktop_capability;
pub(crate) mod host_runtime;
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
use proto::{ReplyPolicy, Req, Resp, TransportKind};
use scope::Scope;
use serde::de::DeserializeOwned;
use serde_json::{json, Value};

mod main_live_closure;
use main_live_closure::*;

mod main_ceo;

pub mod main_cli;
use main_cli::*;

mod main_context;
use main_context::*;
mod main_maintenance;
mod main_autostart;

#[cfg(feature = "context-cancel-test-hooks")]
mod context_cancel_test_hooks;

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

/// Poll the daemon socket until it answers, bounded by `timeout`.
fn wait_for_daemon(sock: &std::path::Path, timeout: std::time::Duration) -> bool {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        if client::alive(sock) {
            return true;
        }
        if std::time::Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}

/// Use a retained credential for ordinary commands. Only context reconciles
/// current communication facts; reading an identity must not refresh a route.
fn me(scope: &Scope) -> anyhow::Result<Identity> {
    let facts = collect_identity_facts(None)?;
    if facts.dsh_session_id.is_none() || facts.endpoint.is_some() {
        return registered_project_identity(scope);
    }
    // DSH retains its gateway-owned identity path. It has no native endpoint
    // supplement and is outside the native context fact contract.
    let (snapshot, receipt) = identity_context_response(scope, facts)?;
    let ident = receipt.ok_or_else(|| identity_context_required(&snapshot))?;
    runtime_for_request(&ident)?;
    Ok(ident)
}

fn msg_caller(scope: &Scope) -> anyhow::Result<Identity> {
    me(scope).map_err(|error| {
        let detail = error.to_string();
        if detail.starts_with("ROUTE_RESOLVE_AMBIGUOUS:")
            || detail.starts_with("IDENTITY_RESTORE_AMBIGUOUS:")
        {
            anyhow::anyhow!(
                "CALLER_ROUTE_AMBIGUOUS: caller session matches multiple registered routes in this project/app; next: run `collab context` from the canonical current project root to reconcile the caller route before retrying. cause: {detail}"
            )
        } else if detail.starts_with("ROUTE_RESOLVE_NOT_FOUND:")
            || detail.starts_with("ROUTE_RESOLVE_INVALID:")
            || detail.starts_with("ROUTE_RESOLVE_STALE_INDEX:")
            || detail.starts_with("SESSION_THREAD_BINDING_STALE:")
            || detail.starts_with("BOARD_STALE_IDENTITY:")
            || detail.contains("identity has no registered runtime binding")
            || detail.starts_with("IDENTITY_RESTORE_CROSS_PROJECT:")
        {
            anyhow::anyhow!(
                "CALLER_ROUTE_NOT_FOUND: caller has no current registered route in this project/app; next: run `collab context` from the canonical current project root to restore the caller route, then retry. cause: {detail}"
            )
        } else if detail.starts_with("BOARD_IDENTITY_REQUIRED:")
            || detail.starts_with("IDENTITY_INFORMATION_REQUIRED:")
            || detail.starts_with("IDENTITY_CONTEXT_INVALID:")
            || detail.starts_with("COLLAB_CONTEXT_UNRESOLVED:")
        {
            anyhow::anyhow!(
                "CALLER_IDENTITY_REQUIRED: caller has no identifiable session or stored Collab identity for this project; next: run `collab context` from the current project root to establish the caller identity, then retry. cause: {detail}"
            )
        } else {
            error
        }
    })
}

/// Registered project operations never bootstrap identity or launch the daemon.
fn registered_project_identity(scope: &Scope) -> anyhow::Result<Identity> {
    registered_project_identity_with_provide(scope, None)
}

fn registered_project_identity_with_provide(
    scope: &Scope,
    provide: Option<&str>,
) -> anyhow::Result<Identity> {
    let paths = scope::HostPaths::resolve()?;
    let facts = collect_identity_facts(provide)?;
    let session = facts
        .session_id
        .as_deref()
        .or(facts.dsh_session_id.as_deref())
        .ok_or_else(|| {
            anyhow::anyhow!("BOARD_IDENTITY_REQUIRED: current agent session id is required")
        })?;
    // Names own work; the agent session identifies the current actor. Pane,
    // thread and endpoint belong to transport recovery, not credential reads.
    let route = client::resolve_native_session_route(
        &scope.sock_path(),
        session,
        cli_project_context(&scope.root)?,
    )?;
    let root = std::fs::canonicalize(&scope.root)?;
    if route.project_scope.as_str() != root.to_string_lossy() {
        anyhow::bail!("BOARD_STALE_IDENTITY: current route belongs to another project");
    }
    let identity = identity::load_existing_at(&paths, scope, Some(route.agent_id.to_string()))?
        .ok_or_else(|| {
            anyhow::anyhow!("BOARD_IDENTITY_REQUIRED: current route has no local credential")
        })?;
    let runtime = runtime_for_request(&identity)?;
    if identity.worker_id != route.agent_id.as_str()
        || identity.project_scope.as_ref() != Some(&route.project_scope)
        || runtime.appserver_id != route.app_scope_id
        || runtime.binding_id != route.binding_id
        || runtime.endpoint_generation != route.endpoint_generation
        || runtime.session_id != route.session_id
        || runtime.native_thread_id != route.native_thread_id
    {
        anyhow::bail!("BOARD_STALE_IDENTITY: credential does not match the current route");
    }
    Ok(identity)
}

fn require_cli_mailbox_consumer(identity: &Identity) -> anyhow::Result<()> {
    if identity
        .transport
        .as_ref()
        .is_some_and(|transport| transport.kind == TransportKind::Dsh)
    {
        anyhow::bail!(
            "DSH_NATIVE_DELIVERY_OWNED: the DSH gateway owns Poll/Ack and delivers the full message into the native conversation; act on that body and reply with collab sendmessage. Do not run collab recv or ack. Use collab msg <id> or collab inbox for read-only inspection"
        );
    }
    Ok(())
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
    if transport.kind == TransportKind::Tmux || runtime.native_thread_id.is_some() {
        let host_facts = host_runtime::host_runtime_facts();
        let (session, thread) = if let Some(agy_session) = host_facts.agy_session_id {
            (Some(agy_session), None)
        } else {
            (
                std::env::var("CODEX_SESSION_ID").ok(),
                std::env::var("CODEX_THREAD_ID").ok(),
            )
        };
        let Some(session) = session else {
            return Ok(false);
        };
        {
            let persisted_thread = runtime
                .native_thread_id
                .as_ref()
                .map(crate::identity::NativeThreadId::as_str);
            let persisted_session = runtime
                .session_id
                .as_ref()
                .map(crate::identity::SessionId::as_str);
            if thread
                .as_deref()
                .is_some_and(|thread| persisted_thread != Some(thread))
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

pub(crate) fn runtime_for_request<'a>(ident: &'a Identity) -> anyhow::Result<&'a RuntimeIdentity> {
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
                .ok_or_else(|| anyhow::anyhow!("registered tmux transport has no agent session"))?;
            let expected_thread = endpoint.codex_thread_id.as_deref();
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
                    != expected_thread
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
        crate::proto::TransportKind::AppServer | crate::proto::TransportKind::DesktopBridge => {
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
                "transport": transport.kind.as_str(),
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

/// One copyable delivery address resolved from the host-wide directory.
#[derive(Clone)]
struct GlobalDeliveryTarget {
    worker_id: String,
    project_root: String,
    address: String,
}

/// Host-wide directory entries carry one target per stable address. The
/// daemon already deduplicates route records by owner; this helper folds any
/// case variants back to one holder and rejects a genuinely ambiguous name.
fn unique_directory_targets(targets: Vec<GlobalDeliveryTarget>) -> Vec<GlobalDeliveryTarget> {
    let mut seen = std::collections::BTreeSet::new();
    let mut out = Vec::new();
    for target in targets {
        let key = (
            target.project_root.clone(),
            target.worker_id.clone(),
            target.address.clone(),
        );
        if seen.insert(key) {
            out.push(target);
        }
    }
    out
}

/// Read the host-wide directory without requiring a project scope. This is
/// the same read-only source as `collab list`, so send addresses and list
/// addresses can never drift from each other.
fn host_directory() -> anyhow::Result<Vec<GlobalDeliveryTarget>> {
    let host = scope::HostPaths::resolve()?;
    let value: Value = client::call(&host.socket_path(), &Req::DirectoryAll)?;
    let mut targets = Vec::new();
    for entry in value
        .get("targets")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let Some(address) = entry.get("address").and_then(Value::as_str) else {
            continue;
        };
        let Some(project_root) = entry.get("project_root").and_then(Value::as_str) else {
            continue;
        };
        let Some(worker_id) = entry.get("worker_id").and_then(Value::as_str) else {
            continue;
        };
        targets.push(GlobalDeliveryTarget {
            worker_id: worker_id.to_owned(),
            project_root: project_root.to_owned(),
            address: address.to_owned(),
        });
    }
    Ok(targets)
}

/// Resolve `ceo` through the typed host grant and the global directory.
fn resolve_ceo_delivery_target(
    scope: &Scope,
    directory: &[GlobalDeliveryTarget],
) -> anyhow::Result<GlobalDeliveryTarget> {
    let host = scope::HostPaths::resolve()?;
    let value: Value = client::call(
        &host.socket_path(),
        &Req::Ceo {
            worker_id: String::new(),
            token: String::new(),
            command: crate::ceo::CeoCommand::Status,
        },
    )?;
    let ceo = value.get("ceo").filter(|entry| !entry.is_null());
    let Some(worker_id) = ceo
        .and_then(|entry| entry.get("worker_id"))
        .or_else(|| value.get("worker_id"))
        .and_then(Value::as_str)
    else {
        anyhow::bail!("CHANNEL_CEO_GRANT_MISSING: no typed CEO grant is active");
    };
    let workspace = ceo
        .and_then(|entry| entry.get("workspace"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    let matches = unique_directory_targets(
        directory
            .iter()
            .filter(|target| {
                target.worker_id == worker_id
                    && (workspace.is_empty() || path_eq(&target.project_root, workspace))
            })
            .cloned()
            .collect(),
    );
    match matches.len() {
        1 => Ok(matches.into_iter().next().expect("one target")),
        0 => anyhow::bail!(
            "CHANNEL_CEO_HOLDER_UNAVAILABLE: the active CEO has no current deliverable address"
        ),
        _ => anyhow::bail!(
            "CHANNEL_CEO_ADDRESS_AMBIGUOUS: the active CEO maps to multiple deliverable addresses"
        ),
    }
}

fn path_eq(left: &str, right: &str) -> bool {
    match (
        std::path::Path::new(left).canonicalize(),
        std::path::Path::new(right).canonicalize(),
    ) {
        (Ok(left), Ok(right)) => left == right,
        _ => left == right,
    }
}

/// Classify one send target. A project-qualified address can name either the
/// current project or another project; `ceo` names the host-wide CEO. Bare
/// names and worker IDs stay on the existing local send path so project-local
/// behavior does not change.
struct SendTargetResolution {
    to: String,
    cross_project: Option<GlobalDeliveryTarget>,
}

fn resolve_send_target(scope: &Scope, requested: &str) -> anyhow::Result<SendTargetResolution> {
    let wanted = requested.trim();
    if wanted.is_empty() {
        return Ok(SendTargetResolution {
            to: wanted.to_owned(),
            cross_project: None,
        });
    }
    let directory = host_directory()?;
    if wanted.eq_ignore_ascii_case("ceo") {
        let target = resolve_ceo_delivery_target(scope, &directory)?;
        let cross = if path_eq(&target.project_root, &scope.root.to_string_lossy()) {
            None
        } else {
            Some(target.clone())
        };
        return Ok(SendTargetResolution {
            to: target.address,
            cross_project: cross,
        });
    }
    if !wanted.contains('.') {
        return Ok(SendTargetResolution {
            to: wanted.to_owned(),
            cross_project: None,
        });
    }
    let matches = unique_directory_targets(
        directory
            .into_iter()
            .filter(|target| target.address.eq_ignore_ascii_case(wanted))
            .collect(),
    );
    match matches.len() {
        1 => {
            let target = matches.into_iter().next().expect("one target");
            let cross = if path_eq(&target.project_root, &scope.root.to_string_lossy()) {
                None
            } else {
                Some(target.clone())
            };
            Ok(SendTargetResolution {
                to: target.address,
                cross_project: cross,
            })
        }
        0 => Ok(SendTargetResolution {
            to: wanted.to_owned(),
            cross_project: None,
        }),
        _ => anyhow::bail!(
            "ADDRESS_AMBIGUOUS: {} folds to multiple current holders",
            wanted
        ),
    }
}

/// Build a host-wide cross-project send while keeping `collab send` as the
/// only public command. Permission failures name the current project master,
/// so an unauthorized caller learns exactly who can send.
fn send_cross_project(
    scope: &Scope,
    ident: &Identity,
    target: &GlobalDeliveryTarget,
    subject: String,
    body: String,
    in_reply_to: Option<String>,
) -> anyhow::Result<()> {
    let local: Value = call_project(scope, ident, &Req::MasterStatus)?;
    let master = local
        .get("master")
        .filter(|entry| !entry.is_null())
        .ok_or_else(|| {
            anyhow::anyhow!(
                "PERMISSION_DENIED: cross-project send requires the current project master; no master is active for this project"
            )
        })?;
    let master_worker = master
        .get("worker_id")
        .and_then(Value::as_str)
        .unwrap_or("");
    if master_worker != ident.worker_id {
        let master_name = master
            .get("display_name")
            .and_then(Value::as_str)
            .unwrap_or(master_worker);
        anyhow::bail!(
            "PERMISSION_DENIED: cross-project send requires this project's current master; current master is {} ({}). You are not permitted to send to {}",
            master_name,
            master_worker,
            target.address
        );
    }
    let request = Req::CrossProjectSend {
        from: ident.worker_id.clone(),
        from_project: scope.root.to_string_lossy().into_owned(),
        source_master_assigned_by: master
            .get("assigned_by")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        source_master_approval: master
            .get("approval")
            .and_then(Value::as_str)
            .map(str::to_owned),
        source_master_assigned_ms: master
            .get("assigned_ms")
            .and_then(Value::as_i64)
            .unwrap_or_default(),
        to: target.worker_id.clone(),
        subject,
        body,
        in_reply_to,
    };
    let context = cli_project_context(std::path::Path::new(&target.project_root))?;
    let value: Value = client::call_with_context(&scope.sock_path(), &request, Some(context))?;
    out(&value);
    Ok(())
}

/// `ceo` is a host-wide address, not necessarily the master of the CEO's own
/// workspace project. Route it through the daemon-owned CEO reply path so the
/// same permission rule applies and no project-master substitution is made.
fn send_to_ceo(
    scope: &Scope,
    ident: &Identity,
    subject: String,
    body: String,
    in_reply_to: Option<String>,
) -> anyhow::Result<()> {
    let request = Req::Ceo {
        worker_id: ident.worker_id.clone(),
        token: ident.token.clone(),
        command: crate::ceo::CeoCommand::Reply {
            subject,
            body,
            in_reply_to,
            urgent: false,
        },
    };
    let value: Value = call_project(scope, ident, &request)?;
    out(&value);
    if value.get("ok") == Some(&Value::Bool(false)) {
        anyhow::bail!(
            "{}",
            value
                .get("error")
                .and_then(Value::as_str)
                .unwrap_or("CEO_SEND_FAILED")
        );
    }
    Ok(())
}

/// Resolve the same canonical scope the context bootstrap used for an
/// authenticated follow-up mutation.
fn context_mutation_scope(
    project: Option<&std::path::Path>,
    app_scope: Option<&str>,
) -> anyhow::Result<Scope> {
    if let Some(project) = project {
        let app = crate::identity::AppServerId::new(
            app_scope.unwrap_or(crate::identity::CLI_APP_SERVER_ID),
        )?;
        let context = crate::context_operation::cli_project_context_for_root(project, &app)?;
        Ok(Scope {
            root: context.canonical_root.into(),
        })
    } else {
        Scope::resolve()
    }
}

/// Attach one mutation receipt to both the flattened and enveloped context
/// shapes while preserving the original bootstrap/registration receipts.
fn attach_context_receipt(value: &mut Value, key: &str, receipt: &Value) {
    value[key] = receipt.clone();
    if let Some(snapshot) = value.pointer_mut("/result/snapshot") {
        if snapshot.is_object() {
            snapshot[key] = receipt.clone();
        }
    }
}

/// Replace the dynamic context projection after a mutation so the same
/// invocation returns the effective role_brief and derived operations.
fn refresh_context_projection(value: &mut Value, fresh: &Value) {
    const FIELDS: [&str; 5] = [
        "role_brief",
        "operations",
        "next_actions",
        "work_roles",
        "peers",
    ];
    for field in FIELDS {
        if let Some(projected) = fresh.get(field) {
            value[field] = projected.clone();
        }
    }
    for pointer in ["/result/snapshot", "/result"] {
        let Some(projection) = value.pointer_mut(pointer) else {
            continue;
        };
        if projection.get("role_brief").is_none() && projection.get("identity").is_none() {
            continue;
        }
        for field in FIELDS {
            if let Some(projected) = fresh.get(field) {
                projection[field] = projected.clone();
            }
        }
    }
}

/// Emit one peer lifecycle `Resp` exactly as the daemon serialized it. A typed
/// non-success result stays a payload and exits 2, matching `collab context`;
/// a transport failure has no `result` and remains an error.
fn emit_lifecycle_response(response: proto::Resp) -> anyhow::Result<()> {
    let typed = response.data.get("result").is_some();
    out(&response);
    if !response.ok && typed {
        std::process::exit(2);
    }
    Ok(())
}

fn main() {
    let cli = Cli::parse();
    if let Err(e) = run(cli.cmd) {
        eprintln!("collab: {}", format_cli_error(&e.to_string()));
        if let Some(server_error) = e.downcast_ref::<client::ServerResponseError>() {
            eprintln!(
                "collab response: {}",
                serde_json::json!(&server_error.response)
            );
        }
        std::process::exit(1);
    }
}

const LEGACY_ROUTE_RESOLVE_NOT_FOUND_RECOVERY: &str = "recovery: run `collab context` from the canonical project main checkout to resolve the route and restore registration; do not re-register a worktree or edit routes.jsonl";
const LEGACY_ROUTE_RESOLVE_NOT_FOUND_UPGRADE: &str = "recovery: run `collab context` from the canonical project main checkout; preserve daemon state and do not start a second daemon or use mailbox state as transport delivery";
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
            let ident = me(&scope)?;
            let request = Req::Subagent {
                worker_id: ident.worker_id.clone(),
                token: ident.token.clone(),
                command,
                launch_env: std::collections::BTreeMap::new(),
            };
            let value: serde_json::Value = call_project(&scope, &ident, &request)?;
            out(&value);
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
        Cmd::Directory => {
            let scope = Scope::resolve()?;
            let v: serde_json::Value = client::call_with_context(
                &scope.sock_path(),
                &Req::Directory,
                Some(cli_project_context(&scope.root)?),
            )?;
            out(&v);
            Ok(())
        }
        Cmd::List => {
            // Host-wide read-only directory. `list` must work from any cwd and
            // needs no project scope; its addresses and project_roots are the
            // single delivery source for channel adapters.
            let host = crate::scope::HostPaths::resolve()?;
            let v: serde_json::Value = client::call(&host.socket_path(), &Req::DirectoryAll)?;
            out(&v);
            Ok(())
        }
        Cmd::Default {
            target,
            channel,
            channel_instance,
            conversation,
        } => {
            let (Some(channel), Some(conversation)) = (channel, conversation) else {
                // A plain terminal has no channel-owned conversation context.
                // Refuse rather than mutate a global default for every user.
                anyhow::bail!(
                    "CONVERSATION_CONTEXT_MISSING: default requires --channel and --conversation from the active channel session"
                );
            };
            let channel_instance = channel_instance.unwrap_or_else(|| channel.clone());
            let scope = Scope::resolve()?;
            match target {
                None => {
                    let v: serde_json::Value = client::call_with_context(
                        &scope.sock_path(),
                        &Req::DefaultShow {
                            channel,
                            channel_instance,
                            conversation,
                        },
                        Some(cli_project_context(&scope.root)?),
                    )?;
                    out(&v);
                }
                Some(target_address) => {
                    let ident = me(&scope)?;
                    let v: serde_json::Value = call_project(
                        &scope,
                        &ident,
                        &Req::DefaultSet {
                            worker_id: ident.worker_id.clone(),
                            token: ident.token.clone(),
                            channel,
                            channel_instance,
                            conversation,
                            target_address,
                        },
                    )?;
                    out(&v);
                }
            }
            Ok(())
        }
        Cmd::Init => {
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
            let scope = Scope { root: project_root };
            // Ordinary init observes the user-managed daemon before any
            // baseline or identity write and never launches it.
            client::require_server(&scope.sock_path())?;
            scope::init(&scope.root)?;
            let facts = collect_identity_facts(None)?;
            let (snapshot, ident) = identity_context_response(&scope, facts)?;
            let ident = ident.ok_or_else(|| identity_context_required(&snapshot))?;
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
                "daemon_started": false,
                "role_brief": snapshot["role_brief"],
                "task_board": task_board["tasks"],
                "recovery_action": snapshot["role_brief"]["communication_recovery"]
            }));
            Ok(())
        }
        Cmd::Serve { service } => {
            let root = if service {
                // A login-supervised daemon (launchd RunAtLoad) has no project
                // cwd, so it resolves its scope from the host state root. Every
                // case that cannot start must still exit 0, or launchd would
                // relaunch us in a tight crash-loop:
                //   * no service record yet: start on the host default root, so
                //     a clean install autostarts without a manual `collab up`;
                //   * a record whose project directory is gone (cleaned-up
                //     worktree): fall back to the host default root, and say so;
                //   * an unparseable record: report and exit 0.
                let recorded = match scope::service_scope_root() {
                    Ok(record) => record,
                    Err(error) => {
                        // A malformed record cannot be repaired by relaunching;
                        // exit 0 so launchd does not spin, and surface the
                        // reason in the log.
                        out(&json!({
                            "ok": false,
                            "service": "record-invalid",
                            "reason": format!("{error:#}"),
                        }));
                        return Ok(());
                    }
                };
                let root = match recorded {
                    Some(root) if root.is_dir() => root,
                    stale => {
                        let fallback = scope::service_default_root()?;
                        out(&json!({
                            "ok": true,
                            "service": "default-root",
                            "reason": match &stale {
                                Some(root) => format!(
                                    "SERVICE_RECORD_STALE: recorded root {} is gone",
                                    root.display()
                                ),
                                None => {
                                    "SERVICE_RECORD_MISSING: no project binding recorded"
                                        .to_string()
                                }
                            },
                            "root": fallback,
                        }));
                        fallback
                    }
                };
                let probe = Scope { root: root.clone() };
                if client::alive(&probe.sock_path()) {
                    out(&json!({
                        "ok": true,
                        "service": "already-running",
                        "server": probe.sock_path(),
                    }));
                    return Ok(());
                }
                root
            } else {
                scope::lifecycle_project_root()?
            };
            let scope = Scope { root };
            if service {
                // Only one host daemon may run. When another daemon still owns
                // the legacy writer fence, starting here would fail and launchd
                // would restart us in a loop, so report the conflict and exit.
                // The fix is for the user to stop the running daemon once; the
                // supervised start then takes over at the next login or load.
                if server::legacy_host_daemon_present() {
                    out(&json!({
                        "ok": false,
                        "service": "another-daemon-running",
                        "reason": "DAEMON_MIGRATION_REQUIRED: another Collab daemon holds the host writer lock; stop it before the supervised daemon can start",
                    }));
                    return Ok(());
                }
                // The host DOWN marker records that the user stopped the daemon
                // in a previous session. A supervised start is a new session
                // (login, boot, or an explicit authorized load), so the marker
                // must not keep ordinary clients locked out until a manual
                // `collab up`. Only this entry point clears it; ordinary clients
                // still refuse to talk to a stopped daemon.
                if let Ok(host_paths) = scope.host_paths() {
                    if std::fs::remove_file(host_paths.state_root().join("DOWN")).is_ok() {
                        out(&json!({"ok": true, "service": "down-marker-cleared"}));
                    }
                }
            }
            let rt = tokio::runtime::Runtime::new()?;
            rt.block_on(server::run(scope))
        }
        Cmd::Up => {
            let _terminal = main_maintenance::user_terminal()?;
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
            // Bind the supervised daemon to this project, then let launchd own
            // the start. Loading the agent before spawning anything is what
            // makes the supervisor own the daemon: a detached `serve` spawned
            // first would make the supervised child exit as "already-running",
            // leaving crash recovery with no process to observe. Only when
            // launchd cannot run the agent (for example a non-macOS host, or an
            // isolated state root) does `up` start a detached daemon, and the
            // result says which happened.
            let service_record = main_autostart::write_service_record(&scope.root);
            let autostart = main_autostart::install_and_load();
            let supervised = autostart.is_ok()
                && wait_for_daemon(&sock, std::time::Duration::from_secs(5));
            if !supervised {
                client::ensure_server(&sock)?;
            }
            out(&json!({
                "ok": true,
                "server": sock,
                "started": !was_running,
                "supervised": supervised,
                "service_record": match &service_record {
                    Ok(path) => json!({"bound": true, "path": path}),
                    Err(error) => json!({"bound": false, "error": format!("{error:#}")}),
                },
                "autostart": match &autostart {
                    Ok(path) => json!({
                        "installed": true,
                        "plist": path,
                        "loaded": main_autostart::is_loaded(),
                    }),
                    Err(error) => json!({"installed": false, "error": format!("{error:#}")}),
                },
            }));
            Ok(())
        }
        Cmd::Down => {
            let scope = Scope {
                root: scope::lifecycle_project_root()?,
            };
            let configured = config::maintenance_password_opt()?;
            let password = match configured {
                None => {
                    // No password is configured, so there is nothing to verify.
                    // Still require a human terminal: a daemon that agents can
                    // stop without a password is a lifecycle safety hole. Only
                    // the password prompt is skipped, not the user-only gate.
                    let _terminal = main_maintenance::user_terminal()?;
                    None
                }
                Some(expected) => {
                    let entered = main_maintenance::read_password()?;
                    if entered.expose() != expected {
                        anyhow::bail!("MAINTENANCE_AUTHENTICATION_FAILED");
                    }
                    Some(entered)
                }
            };
            let _: Value = client::call(
                &scope.sock_path(),
                &Req::Shutdown {
                    password,
                },
            )?;
            // Do NOT `launchctl unload -w` here: that is a persistent disable
            // that would also cancel the next login. The daemon exits cleanly,
            // so the RunAtLoad/KeepAlive(SuccessfulExit=false) job stays
            // stopped for this session and resumes automatically on next login.
            out(&json!({"ok": true, "down": true, "server": scope.sock_path()}));
            Ok(())
        }
        Cmd::InstallAutostart => {
            let path = main_autostart::install_and_load()?;
            out(&json!({"ok": true, "plist": path, "loaded": main_autostart::is_loaded()}));
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
                MailboxCmd::Read { all, sort } => {
                    let actorless = all;
                    let mut identity = None;
                    let worker_id = if actorless {
                        None
                    } else {
                        let ident = me(&scope)?;
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
                let cwd = std::env::current_dir()?;
                let route = scope::canonical_route_for_cwd(&host_paths, &cwd)?;
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
            let ident = me(&scope)?;
            let req = match command {
                MasterCmd::Promote { approval } => Req::MasterPromote {
                    worker_id: ident.worker_id.clone(),
                    token: ident.token.clone(),
                    approval,
                },
                MasterCmd::Clear { approval } => Req::MasterClear {
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
                        anyhow::bail!(
                            "cross-project send requires this peer to be the project master"
                        )
                    };
                    if master.get("worker_id").and_then(|v| v.as_str())
                        != Some(ident.worker_id.as_str())
                    {
                        anyhow::bail!(
                            "cross-project send requires this peer to be the project master"
                        )
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
                MasterCmd::Wake { command } => match command {
                    MasterWakeCmd::Hold {
                        reason,
                        ttl_seconds,
                    } => Req::MasterWakeHold {
                        worker_id: ident.worker_id.clone(),
                        token: ident.token.clone(),
                        reason,
                        ttl_seconds,
                    },
                    MasterWakeCmd::Release => Req::MasterWakeRelease {
                        worker_id: ident.worker_id.clone(),
                        token: ident.token.clone(),
                    },
                },
                MasterCmd::Status => unreachable!("handled above"),
            };
            let v: serde_json::Value = call_project(&scope, &ident, &req)?;
            out(&v);
            Ok(())
        }
        Cmd::Ceo { command } => main_ceo::run(command),
        Cmd::Snapshot { target_id, lines } => {
            let scope = Scope::resolve()?;
            let ident = me(&scope)?;
            let v: serde_json::Value = call_project(
                &scope,
                &ident,
                &Req::WorkerSnapshot {
                    worker_id: ident.worker_id.clone(),
                    token: ident.token.clone(),
                    target_id,
                    lines,
                },
            )?;
            out(&v);
            Ok(())
        }
        Cmd::Worker { cmd } => {
            let scope = if matches!(
                &cmd,
                WorkerCmd::Close {
                    self_current: true,
                    ..
                }
            ) {
                resolve_context_root(&scope::HostPaths::resolve()?, &std::env::current_dir()?)?.0
            } else {
                Scope::resolve()?
            };
            match cmd {
                WorkerCmd::Name { name, reset: _ } => {
                    let ident = registered_project_identity(&scope)?;
                    let value: Value = call_project(
                        &scope,
                        &ident,
                        &Req::WorkerName {
                            worker_id: ident.worker_id.clone(),
                            token: ident.token.clone(),
                            name,
                        },
                    )?;
                    out(&value);
                    Ok(())
                }
                WorkerCmd::Status { id } => {
                    let ident = me(&scope)?;
                    let v: serde_json::Value =
                        call_project(&scope, &ident, &Req::WorkerStatus { worker_id: id })?;
                    out(&v);
                    Ok(())
                }
                WorkerCmd::Snapshot { target_id, lines } => {
                    let ident = me(&scope)?;
                    let v: serde_json::Value = call_project(
                        &scope,
                        &ident,
                        &Req::WorkerSnapshot {
                            worker_id: ident.worker_id.clone(),
                            token: ident.token.clone(),
                            target_id,
                            lines,
                        },
                    )?;
                    out(&v);
                    Ok(())
                }
                WorkerCmd::Create {
                    target_id,
                    cwd,
                    model,
                    operation_id,
                } => {
                    let ident = lifecycle_identity(&scope)?;
                    emit_lifecycle_response(peer_lifecycle_create(
                        &scope,
                        &ident,
                        target_id,
                        cwd,
                        model,
                        operation_id,
                    )?)
                }
                WorkerCmd::Read { target_id } => {
                    let ident = lifecycle_identity(&scope)?;
                    emit_lifecycle_response(peer_lifecycle_read(&scope, &ident, target_id)?)
                }
                WorkerCmd::Update {
                    target_id,
                    cwd,
                    operation_id,
                } => {
                    let ident = lifecycle_identity(&scope)?;
                    emit_lifecycle_response(peer_lifecycle_update(
                        &scope,
                        &ident,
                        &target_id,
                        &cwd,
                        operation_id,
                    )?)
                }
                WorkerCmd::Close {
                    target_id,
                    self_current,
                    reason,
                    operation_id,
                } => {
                    if self_current {
                        // Explicit self removal retains the lifecycle contract:
                        // reconcile current facts and repair its credential first.
                        let (snapshot, receipt) =
                            identity_context_response(&scope, collect_identity_facts(None)?)?;
                        let ident = receipt.ok_or_else(|| identity_context_required(&snapshot))?;
                        let response = client::call_with_context_response(
                            &scope.sock_path(),
                            &Req::UnregisterSelf {
                                worker_id: ident.worker_id.clone(),
                                token: ident.token.clone(),
                            },
                            Some(proto::ProjectContext::for_registered_route(
                                &scope.root,
                                runtime_for_request(&ident)?,
                            )?),
                        )?;
                        return emit_lifecycle_response(response);
                    }
                    let ident = lifecycle_identity(&scope)?;
                    emit_lifecycle_response(peer_lifecycle_close(
                        &scope,
                        &ident,
                        target_id.as_deref().expect("clap requires a target"),
                        reason.as_deref().expect("clap requires a reason"),
                        operation_id,
                    )?)
                }
                WorkerCmd::Query { operation_id } => {
                    let ident = lifecycle_identity(&scope)?;
                    emit_lifecycle_response(peer_lifecycle_query(&scope, &ident, &operation_id)?)
                }
            }
        }
        Cmd::Reset {
            approval,
            discard_legacy,
            project,
            host,
            storage_root,
            include_runs,
        } => {
            let level = reset::ResetLevel::select(project, host)?;
            if project {
                let scope =
                    resolve_context_root(&scope::HostPaths::resolve()?, &std::env::current_dir()?)?
                        .0;
                client::require_server(&scope.sock_path())?;
                let ident = registered_project_identity(&scope)?;
                let value: Value = call_project(
                    &scope,
                    &ident,
                    &Req::ProjectReset {
                        worker_id: ident.worker_id.clone(),
                        token: ident.token.clone(),
                    },
                )?;
                out(&value);
                return Ok(());
            }
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
                    include_runs,
                },
            )
        }
        Cmd::Send {
            to,
            unknown_sender,
            target_session,
            subject,
            content,
            r#type,
            in_reply_to,
            delivery,
            reply_required,
            reply_on_completion,
            body,
        } => {
            let mut body_words = body;
            let content_provided = content.is_some();
            let target = match to {
                Some(to) => to,
                None if body_words.is_empty() && content_provided && in_reply_to.is_some() => {
                    // The daemon derives the external channel route from the
                    // linked inbound message. The empty wire target is only a
                    // request to resolve that typed route.
                    String::new()
                }
                None => {
                    if body_words.is_empty() {
                        anyhow::bail!(
                            "SEND_TARGET_REQUIRED: sendmessage requires --to or a positional address"
                        );
                    }
                    body_words.remove(0)
                }
            };
            let positional_body = body_words.join(" ");
            if content_provided && !positional_body.is_empty() {
                anyhow::bail!(
                    "SEND_SYNTAX_CONFLICT: provide either --content or a positional body, not both"
                );
            }
            let body = match content {
                Some(content) => content,
                None => positional_body,
            };
            if body.trim().is_empty() && in_reply_to.is_none() {
                anyhow::bail!("empty message body");
            }
            let subject = match subject {
                Some(subject) if !subject.trim().is_empty() => subject,
                _ if content_provided => server::mailbox::notification_subject_from_body(&body)
                    .ok_or_else(|| {
                        anyhow::anyhow!("MESSAGE_SUBJECT_REQUIRED: content has no usable title")
                    })?,
                _ => anyhow::bail!("MESSAGE_SUBJECT_REQUIRED: sendmessage requires --subject"),
            };
            let scope = Scope::resolve()?;
            if unknown_sender {
                if target.is_empty() {
                    anyhow::bail!(
                        "ANONYMOUS_REPLY_REJECTED: an anonymous sender cannot use an original channel reply route"
                    );
                }
                if reply_required || reply_on_completion {
                    anyhow::bail!("anonymous send cannot establish reply obligations");
                }
                let v: serde_json::Value = client::call_with_context(
                    &scope.sock_path(),
                    &Req::Send {
                        from: "unknown".into(),
                        worker_id: None,
                        token: None,
                        command: None,
                        to: target,
                        mtype: r#type,
                        subject: Some(subject),
                        body,
                        in_reply_to,
                        delivery,
                        reply_policy: None,
                        target_session,
                    },
                    Some(cli_project_context(&scope.root)?),
                )?;
                out(&v);
                return Ok(());
            }
            let ident = me(&scope)?;
            let reply_policy = if reply_required {
                Some(ReplyPolicy::Required)
            } else if reply_on_completion {
                Some(ReplyPolicy::OnCompletion)
            } else {
                None
            };
            // One address owner for local and cross-project delivery. A
            // project-qualified address that names another project is routed
            // through the host-wide cross-project path; a target the caller
            // is not authorized to send to gets an explicit permission error
            // naming the current master.
            let resolved = resolve_send_target(&scope, &target)?;
            if let Some(cross) = resolved.cross_project {
                if reply_policy.is_some() {
                    anyhow::bail!(
                        "SEND_REPLY_POLICY_UNSUPPORTED: cross-project send has no reply obligation; ask the target project's master to assign the work"
                    );
                }
                if target_session.is_some() {
                    anyhow::bail!(
                        "SEND_TARGET_SESSION_UNSUPPORTED: cross-project send cannot pin a local session"
                    );
                }
                if target.eq_ignore_ascii_case("ceo") {
                    return send_to_ceo(&scope, &ident, subject, body, in_reply_to);
                }
                return send_cross_project(&scope, &ident, &cross, subject, body, in_reply_to);
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
                    to: resolved.to,
                    mtype: r#type,
                    subject: Some(subject),
                    body,
                    in_reply_to,
                    delivery,
                    reply_policy,
                    target_session,
                },
            )?;
            out(&v);
            Ok(())
        }
        Cmd::Notify { cmd } => {
            let scope = Scope::resolve()?;
            let ident = me(&scope)?;
            let request = match cmd {
                NotifyCmd::Methods => Req::NotificationMethods,
                NotifyCmd::Subscribe {
                    event,
                    task_ids,
                    subject,
                    at_ms,
                    every_ms,
                    repeat_count,
                    trigger_ms,
                    ttl_seconds,
                } => {
                    if event == "board-idle" {
                        if subject.is_some()
                            || !at_ms.is_empty()
                            || every_ms.is_some()
                            || trigger_ms.is_some()
                            || repeat_count != 1
                        {
                            anyhow::bail!(
                                "board-idle rearm does not accept subject or schedule options"
                            );
                        }
                        Req::NotificationBoardRearm {
                            worker_id: ident.worker_id.clone(),
                            token: ident.token.clone(),
                            task_ids,
                        }
                    } else {
                        if !task_ids.is_empty() {
                            anyhow::bail!("--task-id requires --event board-idle");
                        }
                        Req::NotificationSubscribe {
                            worker_id: ident.worker_id.clone(),
                            token: ident.token.clone(),
                            event,
                            subject,
                            trigger_ms,
                            trigger_times_ms: at_ms,
                            interval_ms: every_ms,
                            repeat_count,
                            ttl_seconds,
                        }
                    }
                }
                NotifyCmd::Cancel {
                    task_ids,
                    event,
                    reason,
                    resume_condition,
                } => Req::NotificationCancel {
                    worker_id: ident.worker_id.clone(),
                    token: ident.token.clone(),
                    task_ids,
                    event,
                    reason,
                    resume_condition,
                },
                NotifyCmd::Status => Req::NotificationStatus {
                    worker_id: ident.worker_id.clone(),
                    token: ident.token.clone(),
                },
            };
            let value: serde_json::Value = call_project(&scope, &ident, &request)?;
            out(&value);
            Ok(())
        }
        Cmd::Goal { cmd } => {
            let scope = Scope::resolve()?;
            let ident = me(&scope)?;
            let GoalCmd::Reconcile {
                goal_id,
                owner,
                desired,
                trigger_ms,
                ttl_seconds,
                goal_subject,
                reason,
            } = cmd;
            let value: serde_json::Value = call_project(
                &scope,
                &ident,
                &Req::GoalScheduleReconcile {
                    worker_id: ident.worker_id.clone(),
                    token: ident.token.clone(),
                    goal_id,
                    goal_owner: owner,
                    desired,
                    trigger_ms,
                    ttl_seconds,
                    goal_subject,
                    reason,
                },
            )?;
            out(&value);
            Ok(())
        }
        Cmd::Recv {
            timeout,
            receive_id,
        } => {
            let scope = Scope::resolve()?;
            let ident = me(&scope)?;
            require_cli_mailbox_consumer(&ident)?;
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
        Cmd::Inbox => {
            let scope = Scope::resolve()?;
            let ident = me(&scope)?;
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
        Cmd::Context {
            work_roles,
            for_name,
            workflow_target,
            workflow_template,
            reset_workflow_template,
            operation_id,
            project,
            app_scope,
            approve_identity,
            approve_grant,
            provide,
            query,
            return_challenge,
            recover_communication,
        } => {
            // Validate and parse the workflow mutation before any bootstrap or
            // identity write. A malformed spec or conflicting flag must leave
            // the host untouched.
            let workflow_request = match (
                workflow_target.as_deref(),
                workflow_template.as_deref(),
                reset_workflow_template,
            ) {
                (None, None, false) => None,
                (Some(_), _, _) | (_, Some(_), _) if work_roles.is_some() || for_name.is_some() => {
                    anyhow::bail!("CONTEXT_WORKFLOW_MUTATION_INVALID: workflow template flags cannot be combined with --work-roles/--for-name");
                }
                (Some(target), Some(raw), false) => {
                    let spec: crate::board::WorkflowTemplateSpec =
                        serde_json::from_str(raw).map_err(|error| {
                            anyhow::anyhow!("CONTEXT_WORKFLOW_TEMPLATE_INVALID: {error}")
                        })?;
                    Some((target.to_owned(), Some(spec)))
                }
                (Some(target), None, true) => Some((target.to_owned(), None)),
                (Some(_), None, false) => anyhow::bail!(
                    "CONTEXT_WORKFLOW_MUTATION_INVALID: --workflow-target requires exactly one of --workflow-template or --reset-workflow-template"
                ),
                (None, Some(_), _) | (None, None, true) => anyhow::bail!(
                    "CONTEXT_WORKFLOW_TARGET_REQUIRED: --workflow-template/--reset-workflow-template requires --workflow-target"
                ),
                _ => anyhow::bail!(
                    "CONTEXT_WORKFLOW_MUTATION_INVALID: pass exactly one of --workflow-template or --reset-workflow-template"
                ),
            };
            let result = if let Some(challenge) = return_challenge {
                main_context::return_registration_challenge(&challenge)
            } else {
                main_context::context_operation_with_intent(
                    operation_id,
                    project.as_deref(),
                    app_scope.as_deref(),
                    approve_identity.as_deref(),
                    approve_grant.as_deref(),
                    provide.as_deref(),
                    query,
                    recover_communication,
                )
            };
            match result {
                Ok(value) => {
                    if value.get("ok") == Some(&Value::Bool(false)) {
                        out(&value);
                        std::process::exit(2);
                    }
                    let mut updated = value;
                    if work_roles.is_some() || workflow_request.is_some() {
                        let scope =
                            context_mutation_scope(project.as_deref(), app_scope.as_deref())?;
                        let ident =
                            registered_project_identity_with_provide(&scope, provide.as_deref())?;
                        if let Some(roles) = work_roles {
                            let request = Req::WorkRolesSet {
                                worker_id: ident.worker_id.clone(),
                                token: ident.token.clone(),
                                name: for_name,
                                roles: if roles.is_empty() {
                                    Vec::new()
                                } else {
                                    roles.split(',').map(str::to_owned).collect()
                                },
                            };
                            let receipt: Value = call_project(&scope, &ident, &request)?;
                            attach_context_receipt(&mut updated, "work_roles_update", &receipt);
                        }
                        if let Some((target, template)) = workflow_request {
                            let request = Req::WorkflowTemplateSet {
                                worker_id: ident.worker_id.clone(),
                                token: ident.token.clone(),
                                target,
                                template,
                            };
                            let receipt: Value = call_project(&scope, &ident, &request)?;
                            attach_context_receipt(
                                &mut updated,
                                "workflow_template_update",
                                &receipt,
                            );
                        }
                        let fresh: Value = call_project(
                            &scope,
                            &ident,
                            &Req::Context {
                                worker_id: ident.worker_id.clone(),
                                token: ident.token.clone(),
                            },
                        )?;
                        refresh_context_projection(&mut updated, &fresh);
                    }
                    out(&updated);
                }
                Err(error) => {
                    if let Some(server_error) = error.downcast_ref::<client::ServerResponseError>()
                    {
                        let payload = &server_error.response.data;
                        if payload.get("ok") == Some(&Value::Bool(false))
                            && payload.get("result").is_some()
                        {
                            out(payload);
                            std::process::exit(2);
                        }
                    }
                    return Err(error);
                }
            }
            Ok(())
        }
        Cmd::Ack { ids, all } => {
            if ids.is_empty() && !all {
                anyhow::bail!("usage: collab ack <msg_id>... [--all]");
            }
            let scope = Scope::resolve()?;
            let ident = me(&scope)?;
            require_cli_mailbox_consumer(&ident)?;
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
            let ident = msg_caller(&scope)?;
            let v: serde_json::Value = call_project(&scope, &ident, &Req::MsgStatus { msg_id })?;
            out(&v);
            Ok(())
        }
        Cmd::Dashboard { port } => {
            let scope = Scope::resolve()?;
            let identity = registered_project_identity(&scope)?;
            let runtime = runtime_for_request(&identity)?;
            let context = proto::ProjectContext::for_registered_root_with_app(
                &scope.root,
                runtime.appserver_id.clone(),
            )?;
            dashboard::run(scope.sock_path(), context, port)
        }
        Cmd::Board { cmd } => {
            let scope = Scope::resolve()?;
            let ident = registered_project_identity(&scope)?;
            let request = match cmd {
                command => Req::Board {
                    worker_id: ident.worker_id.clone(),
                    token: ident.token.clone(),
                    command,
                },
            };
            let value: serde_json::Value = call_project(&scope, &ident, &request)?;
            out(&value);
            Ok(())
        }
        Cmd::Task { cmd } => {
            let scope = Scope::resolve()?;
            let ident = me(&scope)?;
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
                TaskCmd::Accept {
                    id,
                    expected_revision,
                } => match expected_revision {
                    Some(expected_revision) => Req::Board {
                        worker_id: worker_id.clone(),
                        token: token.clone(),
                        command: board::BoardCommand::Respond {
                            id,
                            accept: true,
                            decline: false,
                            expected_revision,
                            reason: None,
                        },
                    },
                    None => Req::TaskAccept {
                        worker_id: worker_id.clone(),
                        token: token.clone(),
                        task_id: id,
                    },
                },
                TaskCmd::Decline {
                    id,
                    expected_revision,
                    reason,
                    legacy_assignment,
                } => Req::Board {
                    worker_id: worker_id.clone(),
                    token: token.clone(),
                    command: if legacy_assignment {
                        board::BoardCommand::DeclineAssigned {
                            id,
                            expected_revision,
                            reason,
                        }
                    } else {
                        board::BoardCommand::Respond {
                            id,
                            accept: false,
                            decline: true,
                            expected_revision,
                            reason: Some(reason),
                        }
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
                    no_candidate,
                } => Req::TaskDeliver {
                    worker_id: worker_id.clone(),
                    token: token.clone(),
                    task_id: id,
                    evidence: Some(evidence),
                    worktree,
                    no_candidate,
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
                TaskCmd::FinalizeCleanup { id, commit } => Req::TaskFinalizeCleanup {
                    worker_id: worker_id.clone(),
                    token: token.clone(),
                    task_id: id,
                    commit,
                },
                TaskCmd::Status { id } => Req::TaskStatus { task_id: id },
            };
            let v: serde_json::Value = call_project(&scope, &ident, &req)?;
            out(&v);
            Ok(())
        }
        Cmd::Migrate { cmd } => {
            let scope = Scope::resolve()?;
            let ident = me(&scope)?;
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
