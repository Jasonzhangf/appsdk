use super::*;
use crate::identity::{AppServerId, CommandId, OperationId};
use crate::proto::{IdentityFacts, ProjectContext};
use serde_json::Value;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

static CONTEXT_CANCEL_REQUESTED: AtomicBool = AtomicBool::new(false);

extern "C" fn context_cancel_signal(_signal: libc::c_int) {
    CONTEXT_CANCEL_REQUESTED.store(true, Ordering::SeqCst);
}

fn install_context_cancel_handler() -> anyhow::Result<()> {
    unsafe {
        // Install the handler without SA_RESTART so a signal interrupts the
        // blocking response read and the CLI can send the scoped control
        // request on a second connection.
        let mut action: libc::sigaction = std::mem::zeroed();
        action.sa_sigaction = context_cancel_signal as *const () as libc::sighandler_t;
        action.sa_flags = 0;
        libc::sigemptyset(&mut action.sa_mask);
        if libc::sigaction(libc::SIGINT, &action, std::ptr::null_mut()) != 0 {
            anyhow::bail!(
                "IDENTITY_OPERATION_CANCEL_SIGNAL_FAILED: {}",
                std::io::Error::last_os_error()
            );
        }
        // The MCP adapter blocks SIGINT across exec so the CLI installs this
        // handler before an early cancellation can terminate the child. Unblock
        // it now that the handler is in place.
        let mut set: libc::sigset_t = std::mem::zeroed();
        libc::sigemptyset(&mut set);
        libc::sigaddset(&mut set, libc::SIGINT);
        if libc::sigprocmask(libc::SIG_UNBLOCK, &set, std::ptr::null_mut()) != 0 {
            anyhow::bail!(
                "IDENTITY_OPERATION_CANCEL_SIGNAL_FAILED: {}",
                std::io::Error::last_os_error()
            );
        }
    }
    Ok(())
}

fn take_context_cancel_requested() -> bool {
    CONTEXT_CANCEL_REQUESTED.swap(false, Ordering::SeqCst)
}

#[cfg(feature = "context-cancel-test-hooks")]
pub(crate) fn context_cancel_requested() -> bool {
    CONTEXT_CANCEL_REQUESTED.load(Ordering::SeqCst)
}

fn local_cancelled_payload(request: &crate::proto::IdentityContextRequest) -> Value {
    json!({
        "ok": false,
        "result": {
            "operation_id": request.operation_id,
            "invocation": request.invocation,
            "action": request.action,
            "phase": null,
            "outcome": "cancelled",
            "committed_phases": [],
            "failed_phase": null,
            "requires": {
                "kind": null,
                "fields": [],
                "sources": {},
                "approval": null,
                "repair_invocation": null
            },
            "snapshot": null,
            "owner_readback": {}
        }
    })
}

/// Internal CLI-to-adapter classification of one mutating context call. The
/// local pre-send cancellation is never confused with a daemon-acknowledged
/// cancellation that also has `phase: null` but a durable operation.
enum ContextCallOutcome {
    LocalCancelled,
    Response(proto::Resp),
}

fn context_call_with_cancel(
    sock: &Path,
    request: &crate::proto::IdentityContextRequest,
    project_context: &ProjectContext,
) -> anyhow::Result<ContextCallOutcome> {
    install_context_cancel_handler()?;
    #[cfg(feature = "context-cancel-test-hooks")]
    crate::context_cancel_test_hooks::barrier(
        "client_pre_send",
        &request.operation_id,
        std::process::id(),
        None,
        None,
        context_cancel_requested,
    );
    if take_context_cancel_requested() {
        return Ok(ContextCallOutcome::LocalCancelled);
    }
    let mut stream =
        UnixStream::connect(sock).map_err(|error| anyhow::anyhow!("DAEMON_UNKNOWN: {error}"))?;
    let envelope = proto::RequestEnvelope::new(
        Req::IdentityContext {
            facts: IdentityFacts::default(),
            identity_context: Some(request.clone()),
        },
        Some(project_context.clone()),
    );
    let mut line = serde_json::to_vec(&envelope)?;
    line.push(b'\n');
    if let Err(error) = stream.write_all(&line).and_then(|()| stream.flush()) {
        if error.kind() == std::io::ErrorKind::Interrupted {
            // The signal arrived while the request bytes were being sent.
            // Preserve the original connection and use the separate control
            // path; never resend the mutation.
        } else {
            anyhow::bail!("DAEMON_UNKNOWN: failed to send request: {error}");
        }
    }
    stream.set_read_timeout(Some(Duration::from_millis(50)))?;
    let mut reader = BufReader::new(stream);
    let mut response = String::new();
    loop {
        match reader.read_line(&mut response) {
            Ok(0) => anyhow::bail!("DAEMON_UNKNOWN: daemon closed the connection before replying"),
            Ok(_) => {
                let parsed = serde_json::from_str(response.trim()).map_err(|error| {
                    anyhow::anyhow!("DAEMON_UNKNOWN: malformed response: {error}")
                })?;
                return Ok(ContextCallOutcome::Response(parsed));
            }
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::Interrupted
                        | std::io::ErrorKind::WouldBlock
                        | std::io::ErrorKind::TimedOut
                ) =>
            {
                if take_context_cancel_requested() {
                    let cancel = crate::proto::IdentityContextRequest {
                        operation_id: request.operation_id.clone(),
                        invocation: "cancel".into(),
                        action: "cancel".into(),
                        facts: IdentityFacts::default(),
                        approval: None,
                        grant_approval: None,
                        query: false,
                        query_capability: request.query_capability.clone(),
                        invocation_ticket: request.invocation_ticket.clone(),
                    };
                    let _ = client::call_with_context_response(
                        sock,
                        &Req::IdentityContext {
                            facts: IdentityFacts::default(),
                            identity_context: Some(cancel),
                        },
                        Some(project_context.clone()),
                    );
                }
            }
            Err(error) => {
                anyhow::bail!("DAEMON_UNKNOWN: failed to read response: {error}");
            }
        }
    }
}

pub(crate) fn cli_project_context(root: &std::path::Path) -> anyhow::Result<ProjectContext> {
    ProjectContext::for_registered_root_with_app(
        root,
        AppServerId::new(identity::CLI_APP_SERVER_ID)?,
    )
}

pub(crate) struct ContextBootstrap {
    pub(crate) scope: Scope,
    pub(crate) project_root_resolution: &'static str,
    pub(crate) baseline_created: bool,
    pub(crate) daemon_started: bool,
}

pub(crate) fn resolve_context_root(
    host_paths: &scope::HostPaths,
    cwd: &Path,
) -> anyhow::Result<(Scope, &'static str)> {
    if let Ok(scope) = Scope::resolve() {
        return Ok((scope, "route"));
    }
    if let Ok(route) = scope::canonical_route_for_cwd(host_paths, cwd) {
        return Ok((Scope { root: route.root }, "canonical-route"));
    }
    let canonical_cwd = std::fs::canonicalize(cwd)?;
    let is_git_root = Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .current_dir(&canonical_cwd)
        .output()
        .ok()
        .and_then(|output| {
            if !output.status.success() {
                return None;
            }
            std::fs::canonicalize(String::from_utf8_lossy(&output.stdout).trim()).ok()
        })
        .is_some_and(|toplevel| toplevel == canonical_cwd)
        && !canonical_cwd.ancestors().skip(1).any(|ancestor| {
            ancestor
                .file_name()
                .is_some_and(|name| name == "playground")
        });
    if canonical_cwd.join(".agent-collab").is_dir() || is_git_root {
        return Ok((
            Scope {
                root: canonical_cwd,
            },
            "cwd",
        ));
    }
    anyhow::bail!(
        "COLLAB_CONTEXT_UNRESOLVED: no registered Collab route and no local .agent-collab baseline for {}; run `collab context` from the canonical project main checkout",
        canonical_cwd.display()
    )
}

pub(crate) fn context_bootstrap(
    host_paths: &scope::HostPaths,
    cwd: &Path,
) -> anyhow::Result<ContextBootstrap> {
    let (scope, project_root_resolution) = resolve_context_root(host_paths, cwd)?;
    let baseline_created = if scope.root.join(".agent-collab").is_dir() {
        false
    } else {
        if scope.root.ancestors().skip(1).any(|ancestor| {
            ancestor
                .file_name()
                .is_some_and(|name| name == "playground")
        }) {
            anyhow::bail!(
                "collab context refuses to create a baseline inside a ./playground worktree"
            );
        }
        scope::init(&scope.root)?;
        true
    };
    let daemon_started = !client::alive(&scope.sock_path());
    client::ensure_server(&scope.sock_path())?;
    Ok(ContextBootstrap {
        scope,
        project_root_resolution,
        baseline_created,
        daemon_started,
    })
}

/// Environment keys that describe the agent's own Collab/Codex runtime. The
/// daemon cannot see the caller's environment, so this projection is built
/// client-side and replaces the manual `env | rg` probe with one snapshot
/// field.
const CONTEXT_ENV_KEYS: &[&str] = &["HOME", "USER", "LOGNAME", "CARGO_HOME"];
const CONTEXT_ENV_PREFIXES: &[&str] = &["COLLAB_", "APPSDK_", "CODEX_", "DSH_"];

/// Name fragments that mark a credential. `collab context` is the one place the
/// caller's environment enters a Collab response, and `CODEX_API_KEY`-shaped
/// variables are real, so a name-selected key is still dropped when it looks
/// like a credential instead of being copied into every snapshot.
const CONTEXT_ENV_SECRET_MARKERS: &[&str] = &["TOKEN", "KEY", "SECRET", "PASSWORD", "CREDENTIAL"];

fn context_env_key_is_secret(key: &str) -> bool {
    let upper = key.to_ascii_uppercase();
    CONTEXT_ENV_SECRET_MARKERS
        .iter()
        .any(|marker| upper.contains(marker))
}

pub(crate) fn context_env_view() -> serde_json::Value {
    let mut selected = std::collections::BTreeMap::new();
    // `std::env::vars()` panics when any key or value is not valid Unicode, and
    // this projection runs on every `collab context` response, including the
    // identity terminal. A non-Unicode entry is skipped instead of aborting the
    // single bootstrap entry.
    for (key, value) in std::env::vars_os() {
        let (Some(key), Some(value)) = (key.to_str(), value.to_str()) else {
            continue;
        };
        let named = CONTEXT_ENV_KEYS.contains(&key)
            || CONTEXT_ENV_PREFIXES
                .iter()
                .any(|prefix| key.starts_with(prefix));
        if named && !context_env_key_is_secret(key) {
            selected.insert(key.to_owned(), value.to_owned());
        }
    }
    serde_json::to_value(selected).unwrap_or(serde_json::Value::Null)
}

#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct IdentitySupplement {
    #[serde(default, deserialize_with = "provided_string")]
    session_id: Option<String>,
    #[serde(default, deserialize_with = "provided_string")]
    thread_id: Option<String>,
    #[serde(default, deserialize_with = "provided_string")]
    endpoint: Option<String>,
    #[serde(default, deserialize_with = "provided_string")]
    namespace: Option<String>,
}

fn provided_string<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<String>, D::Error> {
    <String as serde::Deserialize>::deserialize(deserializer).map(Some)
}

fn validate_supplement_string(field: &str, value: &str) -> anyhow::Result<()> {
    if value.trim().is_empty() {
        anyhow::bail!("IDENTITY_FACT_INVALID: {field} must not be empty");
    }
    if value.chars().any(char::is_control) {
        anyhow::bail!("IDENTITY_FACT_INVALID: {field} must not contain control characters");
    }
    Ok(())
}

fn parse_identity_supplement(raw: &str) -> anyhow::Result<IdentityFacts> {
    let supplement: IdentitySupplement = serde_json::from_str(raw).map_err(|error| {
        anyhow::anyhow!("IDENTITY_FACT_INVALID: invalid --provide JSON: {error}")
    })?;
    for (field, value) in [
        ("session_id", supplement.session_id.as_deref()),
        ("thread_id", supplement.thread_id.as_deref()),
        ("endpoint", supplement.endpoint.as_deref()),
        ("namespace", supplement.namespace.as_deref()),
    ] {
        if let Some(value) = value {
            validate_supplement_string(field, value)?;
        }
    }
    if supplement.session_id.is_none()
        && supplement.thread_id.is_none()
        && supplement.endpoint.is_none()
        && supplement.namespace.is_none()
    {
        anyhow::bail!("IDENTITY_FACT_INVALID: --provide must contain at least one identity fact");
    }
    Ok(IdentityFacts {
        session_id: supplement.session_id,
        thread_id: supplement.thread_id,
        endpoint: supplement.endpoint,
        namespace: supplement.namespace,
        tmux: None,
        dsh_session_id: None,
    })
}

fn merge_supplied_fact(
    field: &str,
    observed: &mut Option<String>,
    supplied: Option<String>,
) -> anyhow::Result<()> {
    let Some(supplied) = supplied else {
        return Ok(());
    };
    if observed.as_deref().is_some_and(|value| value != supplied) {
        anyhow::bail!(
            "IDENTITY_FACT_CONFLICT: supplied {field} conflicts with the automatically observed value"
        );
    }
    *observed = Some(supplied);
    Ok(())
}

pub(crate) fn collect_identity_facts(provide: Option<&str>) -> anyhow::Result<IdentityFacts> {
    let supplied = provide.map(parse_identity_supplement).transpose()?;
    let mut facts = crate::client::adapters::codex_app_server::identity_facts_from_env()
        .map_err(|error| anyhow::anyhow!(error.to_string()))?;
    let Some(supplied) = supplied else {
        return Ok(facts);
    };
    merge_supplied_fact("session_id", &mut facts.session_id, supplied.session_id)?;
    merge_supplied_fact("thread_id", &mut facts.thread_id, supplied.thread_id)?;
    merge_supplied_fact("endpoint", &mut facts.endpoint, supplied.endpoint)?;
    merge_supplied_fact("namespace", &mut facts.namespace, supplied.namespace)?;
    Ok(facts)
}

pub(crate) fn identity_context_response(
    scope: &Scope,
    facts: IdentityFacts,
) -> anyhow::Result<(Value, Option<Identity>)> {
    let response: Value = client::call_with_context(
        &scope.sock_path(),
        &Req::IdentityContext {
            facts,
            identity_context: None,
        },
        Some(cli_project_context(&scope.root)?),
    )?;
    let snapshot = response
        .get("snapshot")
        .cloned()
        .filter(Value::is_object)
        .ok_or_else(|| anyhow::anyhow!("IDENTITY_CONTEXT_INVALID: response is missing snapshot"))?;
    let receipt = response
        .get("identity_receipt")
        .filter(|value| !value.is_null())
        .cloned()
        .map(serde_json::from_value)
        .transpose()
        .map_err(|error| {
            anyhow::anyhow!("IDENTITY_CONTEXT_INVALID: invalid identity receipt: {error}")
        })?;
    Ok((snapshot, receipt))
}

pub(crate) fn identity_context_required(snapshot: &Value) -> anyhow::Error {
    let mut response = proto::Resp::err("IDENTITY_INFORMATION_REQUIRED: provide the missing runtime facts once through collab context --provide");
    response.data = snapshot.clone();
    anyhow::Error::new(client::ServerResponseError { response })
}

pub(crate) fn context_snapshot(provide: Option<String>) -> anyhow::Result<serde_json::Value> {
    context_operation(None, provide.as_deref(), false, None)
}

fn parse_approval_json(flag: &str, raw: &str) -> anyhow::Result<Value> {
    let approval: Value = serde_json::from_str(raw).map_err(|error| {
        anyhow::anyhow!("IDENTITY_APPROVAL_METADATA_INVALID: invalid {flag} JSON: {error}")
    })?;
    if !approval.is_object() {
        anyhow::bail!("IDENTITY_APPROVAL_METADATA_INVALID: {flag} must be a JSON object");
    }
    Ok(approval)
}

fn context_project_context(
    explicit_project_root: Option<&Path>,
    app_scope: Option<&str>,
    scope: &Scope,
) -> anyhow::Result<ProjectContext> {
    let app_scope_id = match app_scope {
        Some(app_scope) => AppServerId::new(app_scope.to_owned())?,
        None => AppServerId::new(identity::CLI_APP_SERVER_ID)?,
    };
    match explicit_project_root {
        Some(root) => crate::context_operation::cli_project_context_for_root(root, &app_scope_id),
        None if app_scope.is_some() => {
            crate::context_operation::cli_project_context_for_root(&scope.root, &app_scope_id)
        }
        None => crate::context_operation::default_project_context(scope),
    }
}

pub(crate) fn context_operation(
    operation_id: Option<String>,
    provide: Option<&str>,
    query: bool,
    explicit_project_root: Option<&std::path::Path>,
) -> anyhow::Result<serde_json::Value> {
    context_operation_with_options(
        operation_id,
        explicit_project_root,
        None,
        None,
        None,
        provide,
        query,
    )
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn context_operation_with_options(
    operation_id: Option<String>,
    explicit_project_root: Option<&Path>,
    app_scope: Option<&str>,
    approve_identity: Option<&str>,
    approve_grant: Option<&str>,
    provide: Option<&str>,
    query: bool,
) -> anyhow::Result<serde_json::Value> {
    let approval = approve_identity
        .map(|raw| parse_approval_json("--approve-identity", raw))
        .transpose()?;
    let grant_approval = approve_grant
        .map(|raw| parse_approval_json("--approve-grant", raw))
        .transpose()?;
    if query && (approval.is_some() || grant_approval.is_some()) {
        anyhow::bail!("IDENTITY_OPERATION_QUERY_SHAPE_INVALID: query cannot carry approvals");
    }
    let host_paths = scope::HostPaths::resolve()?;
    let cwd = std::env::current_dir()?;
    let (scope, bootstrap) = if query {
        let (scope, _project_root_resolution) = resolve_context_root(&host_paths, &cwd)?;
        (scope, None)
    } else {
        let bootstrap = context_bootstrap(&host_paths, &cwd)?;
        (bootstrap.scope.clone(), Some(bootstrap))
    };
    let project_context = context_project_context(explicit_project_root, app_scope, &scope)?;
    let mut prepared = if query {
        let operation_id = operation_id.ok_or_else(|| {
            anyhow::anyhow!("IDENTITY_OPERATION_QUERY_INVALID: --query requires --op")
        })?;
        crate::context_operation::prepare_query_operation(
            &host_paths,
            &project_context,
            operation_id,
        )?
    } else {
        let facts = collect_identity_facts(provide)?;
        crate::context_operation::prepare_mutating_operation(
            &host_paths,
            &project_context,
            operation_id,
            if approval.is_some() || grant_approval.is_some() {
                "approved_recovery"
            } else if provide.is_some() {
                "supplement"
            } else {
                "automatic"
            },
            facts,
        )?
    };
    if !query {
        prepared.request.approval = approval;
        prepared.request.grant_approval = grant_approval;
        let prepare_request = crate::proto::IdentityContextRequest {
            operation_id: prepared.request.operation_id.clone(),
            invocation: prepared.request.invocation.clone(),
            action: "prepare_invocation".into(),
            facts: prepared.request.facts.clone(),
            approval: prepared.request.approval.clone(),
            grant_approval: prepared.request.grant_approval.clone(),
            query: false,
            query_capability: prepared.request.query_capability.clone(),
            invocation_ticket: String::new(),
        };
        let prepared_response = client::call_with_context_response(
            &scope.sock_path(),
            &Req::IdentityContext {
                facts: IdentityFacts::default(),
                identity_context: Some(prepare_request),
            },
            Some(project_context.clone()),
        )?;
        if !prepared_response.ok {
            return Err(anyhow::Error::new(client::ServerResponseError {
                response: prepared_response,
            }));
        }
        prepared.request.invocation_ticket = prepared_response
            .data
            .get("invocation_ticket")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned)
            .ok_or_else(|| {
                anyhow::anyhow!("IDENTITY_OPERATION_PREPARE_SHAPE_INVALID: no ticket returned")
            })?;
    }
    let response = if query {
        client::call_with_context_response(
            &scope.sock_path(),
            &Req::IdentityContext {
                facts: IdentityFacts::default(),
                identity_context: Some(prepared.request),
            },
            Some(project_context),
        )?
    } else {
        match context_call_with_cancel(&scope.sock_path(), &prepared.request, &project_context)? {
            ContextCallOutcome::LocalCancelled => {
                // The classification is a transport concern: record it on
                // stderr so the MCP adapter suppresses only the local pre-send
                // cancellation while the public stdout payload is unchanged.
                eprintln!("COLLAB_CONTEXT_LOCAL_CANCELLATION");
                return Ok(local_cancelled_payload(&prepared.request));
            }
            ContextCallOutcome::Response(response) => response,
        }
    };
    let mut v = public_context_response(response, query, &prepared.proof.operation_id)?;
    if let Some(bootstrap) = bootstrap {
        if let Some(value) = v.as_object_mut() {
            let snapshot = value
                .get("result")
                .and_then(|result| result.get("snapshot"));
            let registered = snapshot
                .and_then(|snapshot| snapshot.get("registered"))
                .filter(|value| Value::is_boolean(value))
                .cloned()
                .unwrap_or(json!(false));
            let identity = if registered == json!(true)
                && snapshot
                    .and_then(|snapshot| snapshot.get("identity"))
                    .is_some_and(Value::is_object)
            {
                "daemon-owned"
            } else {
                "unresolved"
            };
            value.insert(
                "operation_id".to_string(),
                json!(prepared.proof.operation_id),
            );
            value.insert(
                "bootstrap".to_string(),
                json!({
                    "project_root_resolution": bootstrap.project_root_resolution,
                    "baseline_created": bootstrap.baseline_created,
                    "daemon_started": bootstrap.daemon_started,
                    "identity": identity,
                    "registered": registered,
                }),
            );
            value.insert("env".to_string(), context_env_view());
        }
    }
    Ok(v)
}

pub(crate) fn public_context_response(
    response: proto::Resp,
    query: bool,
    operation_id: &str,
) -> anyhow::Result<Value> {
    // `Resp.ok` owns transport/operation success on the wire; the typed
    // operation payload is the flattened `result` object. A typed incomplete
    // result arrives as `ok:false` with a `result`; a genuine transport
    // failure arrives without one and is surfaced as a server response error.
    let result = response.data.get("result").cloned();
    if !response.ok {
        let Some(result) = result else {
            return Err(anyhow::Error::new(client::ServerResponseError { response }));
        };
        return Ok(json!({"ok": false, "result": result}));
    }
    let result = result.ok_or_else(|| {
        anyhow::anyhow!("IDENTITY_CONTEXT_INVALID: daemon returned an unrecognized response shape")
    })?;
    if !query {
        return Ok(json!({"ok": true, "result": result}));
    }
    let projection = result;
    let mut queried = projection.as_object().cloned().unwrap_or_default();
    let requires = json!({
        "kind": null,
        "fields": [],
        "sources": {},
        "approval": null,
        "repair_invocation": null
    });
    queried
        .entry("failed_phase".to_owned())
        .or_insert(Value::Null);
    queried.insert("requires".to_owned(), requires);
    queried.insert("owner_readback".to_owned(), json!({}));
    Ok(json!({
        "ok": true,
        "result": {
            "operation_id": operation_id,
            "invocation": "query",
            "action": "query",
            "phase": null,
            "outcome": "completed",
            "committed_phases": [],
            "failed_phase": null,
            "requires": {
                "kind": null,
                "fields": [],
                "sources": {},
                "approval": null,
                "repair_invocation": null
            },
            "snapshot": null,
            "owner_readback": {},
            "queried_operation": queried
        }
    }))
}

pub(crate) fn lifecycle_identity(scope: &scope::Scope) -> anyhow::Result<Identity> {
    let identity =
        identity::load_existing_at(&scope.host_paths()?, scope, None)?.ok_or_else(|| {
            anyhow::anyhow!(
                "PEER_LIFECYCLE_IDENTITY_REQUIRED: no local peer identity exists for this project"
            )
        })?;
    runtime_for_request(&identity)?;
    Ok(identity)
}

fn lifecycle_prep_read(
    scope: &scope::Scope,
    ident: &Identity,
    target_id: &str,
) -> anyhow::Result<proto::Resp> {
    peer_lifecycle_read(scope, ident, Some(target_id.to_owned()))
}

fn lifecycle_context(scope: &scope::Scope, ident: &Identity) -> anyhow::Result<ProjectContext> {
    let runtime = runtime_for_request(ident)?;
    ProjectContext::for_registered_route(&scope.root, runtime)
}

pub(crate) fn peer_lifecycle_read(
    scope: &scope::Scope,
    ident: &Identity,
    target_id: Option<String>,
) -> anyhow::Result<proto::Resp> {
    client::call_with_context_response(
        &scope.sock_path(),
        &proto::Req::PeerLifecycle {
            request: proto::PeerLifecycleRequest::Read {
                worker_id: ident.worker_id.clone(),
                token: ident.token.clone(),
                target_id,
            },
        },
        Some(lifecycle_context(scope, ident)?),
    )
}

pub(crate) fn peer_lifecycle_create(
    scope: &scope::Scope,
    ident: &Identity,
    peer_id: String,
    cwd: String,
    model: Option<String>,
    operation_id: Option<String>,
) -> anyhow::Result<proto::Resp> {
    let cwd = std::fs::canonicalize(&cwd)?.to_string_lossy().into_owned();
    let context = lifecycle_context(scope, ident)?;
    let proof = crate::context_operation::prepare_mutating_operation(
        &scope.host_paths()?,
        &context,
        operation_id,
        "worker_create",
        IdentityFacts::default(),
    )?
    .proof;
    client::call_with_context_response(
        &scope.sock_path(),
        &proto::Req::PeerLifecycle {
            request: proto::PeerLifecycleRequest::Create {
                worker_id: ident.worker_id.clone(),
                token: ident.token.clone(),
                operation_id: proof.operation_id,
                query_capability: proof.query_capability,
                peer_id,
                cwd,
                model,
            },
        },
        Some(context),
    )
}

pub(crate) fn peer_lifecycle_update(
    scope: &scope::Scope,
    ident: &Identity,
    target_id: &str,
    cwd: &str,
    operation_id: Option<String>,
) -> anyhow::Result<proto::Resp> {
    if target_id.trim().is_empty() || cwd.trim().is_empty() {
        anyhow::bail!("PEER_LIFECYCLE_INPUT_INVALID: target_id and cwd must be non-empty");
    }
    let canonical_cwd = std::fs::canonicalize(cwd)
        .map_err(|error| anyhow::anyhow!("PEER_LIFECYCLE_CWD_INVALID: {cwd}: {error}"))?;
    let cwd = canonical_cwd.to_string_lossy().into_owned();
    let (proof, target) = if let Some(operation_id) = operation_id {
        let context = lifecycle_context(scope, ident)?;
        let prepared = crate::context_operation::prepare_query_operation(
            &scope.host_paths()?,
            &context,
            operation_id,
        )?;
        let response = client::call_with_context_response(
            &scope.sock_path(),
            &proto::Req::PeerLifecycle {
                request: proto::PeerLifecycleRequest::Query {
                    operation_id: prepared.proof.operation_id.clone(),
                    query_capability: prepared.proof.query_capability.clone(),
                },
            },
            Some(context),
        )?;
        let result = peer_lifecycle_result(&response)?;
        if result.action != proto::PeerLifecycleAction::Update {
            anyhow::bail!("PEER_LIFECYCLE_ACTION_CONFLICT: retained operation is not an update");
        }
        if let Some(update) = result.update.as_ref() {
            if update.intended_cwd != cwd {
                anyhow::bail!(
                    "PEER_LIFECYCLE_INTENT_CONFLICT: retained update intended {}",
                    update.intended_cwd
                );
            }
        }
        let target = result.target.ok_or_else(|| {
            anyhow::anyhow!("PEER_LIFECYCLE_OPERATION_UNKNOWN: retained update has no target")
        })?;
        if target.worker_id != target_id {
            anyhow::bail!(
                "PEER_LIFECYCLE_INTENT_CONFLICT: retained update target is {}",
                target.worker_id
            );
        }
        (prepared.proof, target)
    } else {
        let prepared = lifecycle_prep_read(scope, ident, target_id)?;
        if !prepared.ok || prepared.data.get("result").is_none() {
            return Ok(prepared);
        }
        let proof = crate::context_operation::prepare_mutating_operation(
            &scope.host_paths()?,
            &lifecycle_context(scope, ident)?,
            None,
            "worker_update",
            IdentityFacts::default(),
        )?;
        let target = peer_lifecycle_result(&prepared)?.target.ok_or_else(|| {
            anyhow::anyhow!("PEER_LIFECYCLE_INVALID: daemon returned no exact target")
        })?;
        (proof.proof, target)
    };
    client::call_with_context_response(
        &scope.sock_path(),
        &proto::Req::PeerLifecycle {
            request: proto::PeerLifecycleRequest::Update {
                worker_id: ident.worker_id.clone(),
                token: ident.token.clone(),
                operation_id: proof.operation_id,
                query_capability: proof.query_capability,
                target,
                cwd,
            },
        },
        Some(lifecycle_context(scope, ident)?),
    )
}

pub(crate) fn peer_lifecycle_close(
    scope: &scope::Scope,
    ident: &Identity,
    target_id: &str,
    reason: &str,
    operation_id: Option<String>,
) -> anyhow::Result<proto::Resp> {
    if target_id.trim().is_empty() || reason.trim().is_empty() {
        anyhow::bail!("PEER_LIFECYCLE_INPUT_INVALID: target_id and reason must be non-empty");
    }
    let (proof, target) = if let Some(operation_id) = operation_id {
        let context = lifecycle_context(scope, ident)?;
        let prepared = crate::context_operation::prepare_query_operation(
            &scope.host_paths()?,
            &context,
            operation_id,
        )?;
        let response = client::call_with_context_response(
            &scope.sock_path(),
            &proto::Req::PeerLifecycle {
                request: proto::PeerLifecycleRequest::Query {
                    operation_id: prepared.proof.operation_id.clone(),
                    query_capability: prepared.proof.query_capability.clone(),
                },
            },
            Some(context),
        )?;
        let result = peer_lifecycle_result(&response)?;
        if result.action != proto::PeerLifecycleAction::Close {
            anyhow::bail!("PEER_LIFECYCLE_ACTION_CONFLICT: retained operation is not a close");
        }
        let target = result.target.ok_or_else(|| {
            anyhow::anyhow!("PEER_LIFECYCLE_OPERATION_UNKNOWN: retained close has no target")
        })?;
        if target.worker_id != target_id {
            anyhow::bail!(
                "PEER_LIFECYCLE_INTENT_CONFLICT: retained close target is {}",
                target.worker_id
            );
        }
        (prepared.proof, target)
    } else {
        let prepared = lifecycle_prep_read(scope, ident, target_id)?;
        if !prepared.ok || prepared.data.get("result").is_none() {
            return Ok(prepared);
        }
        let proof = crate::context_operation::prepare_mutating_operation(
            &scope.host_paths()?,
            &lifecycle_context(scope, ident)?,
            None,
            "worker_close",
            IdentityFacts::default(),
        )?;
        let target = peer_lifecycle_result(&prepared)?.target.ok_or_else(|| {
            anyhow::anyhow!("PEER_LIFECYCLE_INVALID: daemon returned no exact target")
        })?;
        (proof.proof, target)
    };
    client::call_with_context_response(
        &scope.sock_path(),
        &proto::Req::PeerLifecycle {
            request: proto::PeerLifecycleRequest::Close {
                worker_id: ident.worker_id.clone(),
                token: ident.token.clone(),
                operation_id: proof.operation_id,
                query_capability: proof.query_capability,
                target,
                reason: reason.to_owned(),
            },
        },
        Some(lifecycle_context(scope, ident)?),
    )
}

pub(crate) fn peer_lifecycle_query(
    scope: &scope::Scope,
    ident: &Identity,
    operation_id: &str,
) -> anyhow::Result<proto::Resp> {
    let context = lifecycle_context(scope, ident)?;
    let prepared = crate::context_operation::prepare_query_operation(
        &scope.host_paths()?,
        &context,
        operation_id.to_owned(),
    )?;
    client::call_with_context_response(
        &scope.sock_path(),
        &proto::Req::PeerLifecycle {
            request: proto::PeerLifecycleRequest::Query {
                operation_id: prepared.proof.operation_id,
                query_capability: prepared.proof.query_capability,
            },
        },
        Some(context),
    )
}

fn peer_lifecycle_result(response: &proto::Resp) -> anyhow::Result<proto::PeerLifecycleResult> {
    let result = response.data.get("result").ok_or_else(|| {
        anyhow::anyhow!("PEER_LIFECYCLE_INVALID: daemon returned no lifecycle result")
    })?;
    serde_json::from_value(result.clone()).map_err(|error| {
        anyhow::anyhow!(
            "PEER_LIFECYCLE_INVALID: daemon returned an invalid lifecycle result: {error}"
        )
    })
}

pub(crate) fn command_envelope(
    scope: &Scope,
    ident: &Identity,
) -> anyhow::Result<proto::CommandEnvelope> {
    let runtime = runtime_for_request(ident)?;
    let route = scope.route_scope(runtime.appserver_id.clone())?;
    let nonce = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
    Ok(proto::CommandEnvelope::new(
        CommandId::new(format!("command-{}-{nonce}", std::process::id()))?,
        OperationId::new(format!("operation-{}-{nonce}", std::process::id()))?,
        runtime.binding_id.clone(),
        runtime.endpoint_generation,
        route,
        None,
        None,
        None,
        None,
    ))
}

/// One stable, caller-owned identity per `recv` invocation. It is generated
/// before the request so a lost response can be replayed by the same caller
/// instead of stranding an already committed batch.
pub(crate) fn new_receive_id() -> String {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    format!("receive-{}-{nonce}", std::process::id())
}

/// The exact, copyable recovery instruction the default `recv` publishes
/// before it consumes anything.
pub(crate) fn receive_recovery_banner(receive_id: &str) -> String {
    format!("recv receive_id={receive_id} recovery: collab recv --receive-id {receive_id}")
}

pub(crate) fn recv_request(
    worker_id: String,
    token: String,
    timeout_seconds: u64,
    receive_id: String,
) -> Req {
    Req::Poll {
        worker_id,
        token,
        timeout_ms: timeout_seconds.saturating_mul(1000),
        receive_id: Some(receive_id),
    }
}

/// Publish the caller-owned receive identity and then dispatch the request.
///
/// Claiming `receive_id` only after a successful reply cannot recover a route
/// whose response was lost, so the default CLI must make the replay identity
/// visible before the daemon consumes the batch. A failed publication returns
/// before dispatch: consuming without a recoverable identity would recreate
/// the stranded batch the identity exists to prevent.
pub(crate) fn recv_with_published_identity<D>(
    publish: &mut dyn std::io::Write,
    receive_id: &str,
    dispatch: D,
) -> anyhow::Result<serde_json::Value>
where
    D: FnOnce(&str) -> anyhow::Result<serde_json::Value>,
{
    writeln!(publish, "{}", receive_recovery_banner(receive_id))
        .and_then(|()| publish.flush())
        .map_err(|error| anyhow::anyhow!("RECEIVE_IDENTITY_PUBLISH_FAILED: {error}"))?;
    dispatch(receive_id)
}

pub(crate) fn subagent_observe_query(
    command: &subagent::Action,
) -> Option<(Option<String>, Option<usize>)> {
    match command {
        subagent::Action::List => Some((None, None)),
        subagent::Action::Status { id } => Some((Some(id.clone()), None)),
        _ => None,
    }
}
