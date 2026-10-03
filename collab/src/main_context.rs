use super::*;
use crate::identity::{AppServerId, CommandId, OperationId};
use crate::proto::ProjectContext;
use std::path::Path;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

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

/// Only the explicit `--worker` override is the adjudication channel, and it is
/// the one path the cross-project restore error points the user at. The implicit
/// `collab context` path must never ask the daemon to retire another peer's
/// anchor: the ledger contract keeps the implicit path inside the current scope
/// and fails closed on a foreign record.
pub(crate) fn context_may_retire_foreign_anchor(worker: Option<&str>) -> bool {
    worker.is_some()
}

/// Environment keys that describe the agent's own Collab/Codex runtime. The
/// daemon cannot see the caller's environment, so this projection is built
/// client-side and replaces the manual `env | rg` probe with one snapshot
/// field.
const CONTEXT_ENV_KEYS: &[&str] = &["HOME", "USER", "LOGNAME", "CARGO_HOME"];
const CONTEXT_ENV_PREFIXES: &[&str] = &["COLLAB_", "APPSDK_", "CODEX_"];

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

/// The closed set of failures that mean this peer's identity cannot be proven,
/// restored, or authenticated. These — and only these — are answered with the
/// identity terminal. A route, runtime-binding, or transport failure is a
/// different problem, so it keeps failing closed with a non-zero exit instead
/// of being relabelled as an identity request.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum IdentityFailure {
    TokenMismatch,
    RebindUnproven,
    CrossProjectRestore,
    AnchorMissing,
}

impl IdentityFailure {
    /// Classify against the exact strings the identity layer and the daemon
    /// actually produce. An unrecognised failure returns `None` on purpose: an
    /// open-ended default would silently absorb unrelated failures.
    pub(crate) fn classify(error: &str) -> Option<Self> {
        if error.starts_with("token mismatch")
            || error.starts_with("RUNTIME_BINDING_REJECTED: worker token does not match")
        {
            return Some(Self::TokenMismatch);
        }
        if error.starts_with("IDENTITY_REBIND_UNPROVEN:") {
            return Some(Self::RebindUnproven);
        }
        if error.starts_with("IDENTITY_RESTORE_CROSS_PROJECT:") {
            return Some(Self::CrossProjectRestore);
        }
        if error.starts_with("COLLAB_IDENTITY_ANCHOR_MISSING:") {
            return Some(Self::AnchorMissing);
        }
        None
    }

    pub(crate) fn code(self) -> &'static str {
        match self {
            Self::TokenMismatch => "TOKEN_MISMATCH",
            Self::RebindUnproven => "IDENTITY_REBIND_UNPROVEN",
            Self::CrossProjectRestore => "IDENTITY_RESTORE_CROSS_PROJECT",
            Self::AnchorMissing => "COLLAB_IDENTITY_ANCHOR_MISSING",
        }
    }

    /// Every repair in this set needs an operator to declare which durable
    /// identity to adopt, because `collab context` must not infer one.
    /// `TokenMismatch` is the exception: that identity is already declared and
    /// was rejected, so the recovery is to escalate it, not to approve a new
    /// declaration.
    pub(crate) fn requires_approval(self) -> bool {
        !matches!(self, Self::TokenMismatch)
    }

    /// The action must never be the invocation that just failed, or an agent
    /// following it loops. `IDENTITY_REBIND_UNPROVEN` and
    /// `COLLAB_IDENTITY_ANCHOR_MISSING` are only reached without `--worker`, and
    /// the identity layer documents `--worker` as their recovery, so the action
    /// names the missing argument with a placeholder: the operator picks the
    /// durable identity. A rejected token cannot be re-run into validity, and
    /// every `collab` command run as that worker re-sends the same rejected
    /// token through `me()`, so `TokenMismatch` escalates out of band with the
    /// concrete worker instead of naming a command that cannot work.
    pub(crate) fn action(self, worker: Option<&str>) -> String {
        let named = worker.unwrap_or("<worker_id>");
        match self {
            Self::TokenMismatch => format!(
                "escalate out of band to the project owner, or to the live master through a healthy peer: worker {named} is already declared and its token was rejected, so no collab command run as {named} can re-authenticate; preserve exact_error and worker_id={named} and wait for the owner to declare which durable identity to adopt; do not copy tokens, mint a new identity, or edit routes"
            ),
            Self::RebindUnproven | Self::CrossProjectRestore | Self::AnchorMissing => {
                "collab context --worker <worker_id>".to_owned()
            }
        }
    }
}

/// The single, copyable instruction an agent needs when `collab context`
/// cannot prove or restore its identity. The reason is the classified code so a
/// caller can branch without parsing prose, and `requires_approval` is derived
/// per reason so no caller has to guess whether a human gate applies.
pub(crate) fn identity_update_view(
    error: &anyhow::Error,
    failure: IdentityFailure,
    requested_worker: Option<&str>,
) -> serde_json::Value {
    json!({
        "required": true,
        "reason": failure.code(),
        "exact_error": error.to_string(),
        "worker_id": requested_worker,
        "action": failure.action(requested_worker),
        "requires_approval": failure.requires_approval(),
        "next": "run the action from the canonical project main checkout with a live runtime anchor; if the same error persists, preserve exact_error and worker_id and report them to the live master; do not edit routes, copy tokens, or start a second daemon",
    })
}

/// The identity terminal is entered only for a classified identity failure.
/// Everything else keeps its original error and a non-zero exit.
pub(crate) fn identity_terminal(
    bootstrap: &ContextBootstrap,
    error: anyhow::Error,
    requested_worker: Option<&str>,
) -> anyhow::Result<serde_json::Value> {
    match IdentityFailure::classify(&error.to_string()) {
        Some(failure) => Ok(identity_update_snapshot(
            bootstrap,
            &error,
            failure,
            requested_worker,
        )),
        None => Err(error),
    }
}

/// Read-only daemon projections that need no worker token. They stay available
/// while the identity path fails closed, so one `collab context` still answers
/// both questions an agent has at bootstrap: what is the durable project state,
/// and what must I do about my identity.
fn read_only_project_state(scope: &Scope) -> anyhow::Result<serde_json::Value> {
    let context = Some(cli_project_context(&scope.root)?);
    let workers: serde_json::Value =
        client::call_with_context(&scope.sock_path(), &Req::Workers, context.clone())?;
    let status: serde_json::Value =
        client::call_with_context(&scope.sock_path(), &Req::StatusAll, context.clone())?;
    let master: serde_json::Value =
        client::call_with_context(&scope.sock_path(), &Req::MasterStatus, context)?;
    Ok(json!({
        "peers": workers["workers"].clone(),
        "peer_count": workers["count"].clone(),
        "tasks": status["tasks"].clone(),
        "subagents": status["subagents"].clone(),
        "master_wake": status["master_wake"].clone(),
        "summary": status["summary"].clone(),
        "pending_merges": status["pending_merges"].clone(),
        "master": master["master"].clone(),
        "recorded_unusable": master["recorded_unusable"].clone(),
    }))
}

/// The identity terminal of the `collab context` state machine. It never
/// fabricates a registered snapshot: `registered` stays false, `identity`
/// stays null, and `requires_identity_update` carries the exact reason plus the
/// one action that can restore the identity.
pub(crate) fn identity_update_snapshot(
    bootstrap: &ContextBootstrap,
    error: &anyhow::Error,
    failure: IdentityFailure,
    requested_worker: Option<&str>,
) -> serde_json::Value {
    let mut snapshot = read_only_project_state(&bootstrap.scope).unwrap_or_else(|read_error| {
        json!({
            "read_only_state_unavailable": true,
            "exact_error": read_error.to_string(),
        })
    });
    let requires_identity_update = identity_update_view(error, failure, requested_worker);
    if let Some(object) = snapshot.as_object_mut() {
        object.insert("schema_version".to_owned(), json!(1));
        object.insert("project_root".to_owned(), json!(bootstrap.scope.root));
        object.insert("registered".to_owned(), json!(false));
        object.insert("identity".to_owned(), serde_json::Value::Null);
        object.insert("env".to_owned(), context_env_view());
        object.insert(
            "bootstrap".to_owned(),
            json!({
                "project_root_resolution": bootstrap.project_root_resolution,
                "baseline_created": bootstrap.baseline_created,
                "daemon_started": bootstrap.daemon_started,
                "identity": "unresolved",
                "registered": false,
            }),
        );
        object.insert(
            "requires_identity_update".to_owned(),
            requires_identity_update,
        );
    }
    snapshot
}

pub(crate) fn context_snapshot(worker: Option<String>) -> anyhow::Result<serde_json::Value> {
    let host_paths = scope::HostPaths::resolve()?;
    let cwd = std::env::current_dir()?;
    let bootstrap = context_bootstrap(&host_paths, &cwd)?;
    let scope = bootstrap.scope.clone();
    let requested_worker = worker.clone();
    let requested_worker = requested_worker.as_deref();
    let retire_foreign_anchor = context_may_retire_foreign_anchor(requested_worker);
    let mut ident = match identity::load_or_create_for_context(&scope, worker) {
        Ok(ident) => ident,
        Err(error) => return identity_terminal(&bootstrap, error, requested_worker),
    };
    if retire_foreign_anchor {
        crate::set_context_registration_requested(true);
    }
    let registration = ensure_registration_with_outcome(&scope, &mut ident);
    crate::set_context_registration_requested(false);
    let (_, identity_state) = match registration {
        Ok(outcome) => outcome,
        // The durable identity is loaded here, so the terminal can name the
        // concrete worker instead of leaving the caller with a placeholder.
        Err(error) => {
            return identity_terminal(&bootstrap, error, Some(ident.worker_id.as_str()));
        }
    };
    let identity_state = match identity_state {
        RegistrationOutcome::Created => "created",
        RegistrationOutcome::Reused => "reused",
        RegistrationOutcome::Recovered => "recovered",
        RegistrationOutcome::Recreated => "recreated",
    };
    // A registered identity can still be rejected by the daemon when its token
    // no longer owns the worker id. `identity_terminal` decides whether that
    // rejection is a classified identity failure or an unrelated one that must
    // keep failing closed. The rejected worker is the loaded identity, not the
    // caller's optional `--worker`, so the escalation names a concrete id even
    // on the implicit `collab context` path.
    let mut v: serde_json::Value = match call_project(
        &scope,
        &ident,
        &Req::Context {
            worker_id: ident.worker_id.clone(),
            token: ident.token.clone(),
        },
    ) {
        Ok(value) => value,
        Err(error) => {
            return identity_terminal(&bootstrap, error, Some(ident.worker_id.as_str()));
        }
    };
    if let Some(value) = v.as_object_mut() {
        value.insert(
            "bootstrap".to_string(),
            json!({
                "project_root_resolution": bootstrap.project_root_resolution,
                "baseline_created": bootstrap.baseline_created,
                "daemon_started": bootstrap.daemon_started,
                "identity": identity_state,
                "registered": true,
            }),
        );
        value.insert("env".to_string(), context_env_view());
    }
    Ok(v)
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
