use super::*;
use crate::identity::{AppServerId, CommandId, OperationId};
use crate::proto::{IdentityFacts, ProjectContext};
use serde_json::Value;
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
        &Req::IdentityContext { facts },
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
    let host_paths = scope::HostPaths::resolve()?;
    let cwd = std::env::current_dir()?;
    let facts = collect_identity_facts(provide.as_deref())?;
    let bootstrap = context_bootstrap(&host_paths, &cwd)?;
    let scope = bootstrap.scope.clone();
    let (mut v, _receipt) = identity_context_response(&scope, facts)?;
    if let Some(value) = v.as_object_mut() {
        value.insert(
            "bootstrap".to_string(),
            json!({
                "project_root_resolution": bootstrap.project_root_resolution,
                "baseline_created": bootstrap.baseline_created,
                "daemon_started": bootstrap.daemon_started,
                "identity": if value.get("registered").and_then(Value::as_bool) == Some(true) {
                    "daemon-owned"
                } else {
                    "unresolved"
                },
                "registered": value.get("registered").cloned().unwrap_or(json!(false)),
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
