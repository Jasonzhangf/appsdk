//! dsh-gateway adapter: collab's third transport channel.
//!
//! Collab speaks to the dsh-gateway control plane over its NDJSON UNIX socket.
//! Per the frozen interface (`docs/COLLAB-DSH-CHANNEL-DESIGN.md` section 3.7)
//! Scoped methods are issued from here:
//!
//! - `agent-facts`: the read-only single-response challenge that admits a
//!   `DshCandidate`, and re-verifies it when a route is resolved.
//! - `enqueue`: delivers one wake to one agent.
//! - `register-peer`: hands the already-committed collab identity and binding
//!   to the selected gateway so it can start or restore the canonical Poll
//!   receive loop. It is not a second registration owner: the gateway adopts
//!   the binding instead of calling `Register` back.
//! - `resolve-agent`: locates only the already-bound session in its canonical
//!   project when a gateway connection UUID changes. Ambiguity is an error.
//!
//! The same control plane also exposes `runtimes`, `agents`, `agent-get`,
//! `queue`, `hold-ack`, `release-ack`, `interrupt` and `shutdown`. Those are
//! operator surfaces and stay unreachable from this adapter: collab must never
//! read the whole registry, and must never drive an agent's lifecycle.
//!
//! The three failure classes are kept apart on purpose. "The gateway says it
//! does not know this agent" is proof of absence; "the socket is down" and
//! "the reply did not answer my challenge" are not. Collapsing them would let a
//! gateway restart retire a live peer, which is the one mistake this channel
//! must not make.

use sha2::{Digest, Sha256};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// A challenge must not stall a registration indefinitely. The gateway answers
/// from its in-memory registry, so two seconds is already generous.
const CONTROL_TIMEOUT: Duration = Duration::from_secs(2);

/// The facts one `agent-facts` response carries. All five fields arrive in the
/// same response, which is what keeps them a single evidence group (design D3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentFacts {
    pub runtime_id: String,
    pub agent_id: String,
    pub session_id: String,
    pub cwd: String,
    pub status: String,
}

/// The `runtime_context` is the authoritative Collab runtime identity carried
/// verbatim from the committed `RuntimeBinding`. Its field names match
/// `ProjectContext.runtime_context`, so the gateway presents exactly what Collab
/// issued on `Poll`, `Ack`, and `Send`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DshRuntimeContext<'a> {
    pub agent_id: &'a str,
    pub runtime_id: &'a str,
    pub appserver_id: &'a str,
    pub binding_id: &'a str,
    pub endpoint_generation: u64,
    pub session_id: Option<&'a str>,
    pub native_thread_id: Option<&'a str>,
}

impl<'a> DshRuntimeContext<'a> {
    /// Serialize only the canonical Collab runtime-context fields. Gateway
    /// transport addressing (`runtimeId`/`agentId`) stays outside this object.
    fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "agent_id": self.agent_id,
            "runtime_id": self.runtime_id,
            "appserver_id": self.appserver_id,
            "binding_id": self.binding_id,
            "endpoint_generation": self.endpoint_generation,
            "session_id": self.session_id,
            "native_thread_id": self.native_thread_id,
        })
    }
}

/// The committed identity and binding collab hands to the selected gateway.
///
/// `worker_id`/`token` are the gateway's poll credentials, so this struct is
/// only ever serialized into the same-user control socket request. It is not a
/// log, receipt, or business-payload shape.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DshLoopEnrollment<'a> {
    /// DSH gateway WebSocket runtime address (`SelectedTransport.namespace`).
    pub runtime_id: &'a str,
    /// DSH gateway agent address (`RuntimeBinding.native_thread_id`).
    pub agent_id: &'a str,
    pub worker_id: &'a str,
    pub token: &'a str,
    pub project_scope: &'a str,
    pub runtime_context: DshRuntimeContext<'a>,
}

/// Why a control-plane exchange did not produce a usable answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ControlError {
    /// A definite refusal. Either the candidate endpoint is malformed, the
    /// gateway answered with its own error code, or the reply did not satisfy
    /// the challenge. `code` carries the gateway's stable code when there was
    /// one, so callers can tell "the gateway denies knowing this agent" apart
    /// from "the gateway refused for some other reason".
    Rejected { code: String, detail: String },
    /// No usable answer: a timeout, a socket closed before replying, or a
    /// frame that is not the expected NDJSON object.
    Unusable { detail: String },
    /// The control socket itself could not be reached.
    Unreachable { detail: String },
}

impl std::error::Error for ControlError {}

impl ControlError {
    /// True only when the gateway explicitly denied knowing this agent.
    ///
    /// `unknown-agent` is the gateway saying it does not know this agent at all,
    /// which is the only answer that may retire a peer: the retirement path
    /// deletes the route, and that is not reversible.
    ///
    /// `unknown-runtime` must **not** retire anything. It means "this gateway
    /// cannot observe the agent right now": the runtime is not connected,
    /// `agent/get` timed out, the runtime dropped mid-request, the reply was
    /// malformed, or it carried no `agent`/`cwd`. A single transient failure
    /// would otherwise delete a live peer's route. Timeouts, unreachable
    /// sockets, failed nonce checks and every other refusal are likewise
    /// `Unknown`, never `Absent`.
    pub fn is_definitely_absent(&self) -> bool {
        matches!(self, Self::Rejected { code, .. } if code == "unknown-agent")
    }

    /// True only when a wake provably did not reach the gateway's queue: the
    /// control socket could not be opened, or the gateway answered with a
    /// refusal. A write that failed mid-frame, a timeout, or a malformed reply
    /// all leave the outcome unknown — the message may already be queued — so
    /// they must never be resent.
    pub fn is_definitely_undelivered(&self) -> bool {
        matches!(self, Self::Rejected { .. } | Self::Unreachable { .. })
    }
}

impl std::fmt::Display for ControlError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Rejected { detail, .. } => write!(f, "DSH_ENDPOINT_REJECTED: {detail}"),
            Self::Unusable { detail } => write!(f, "DSH_ENDPOINT_UNKNOWN: {detail}"),
            Self::Unreachable { detail } => write!(f, "DSH_ENDPOINT_BLOCKED: {detail}"),
        }
    }
}

/// Liveness of one dsh peer, in the vocabulary `identity.rs` already uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PeerPresence {
    Live,
    /// The gateway explicitly denies knowing this agent. `unknown-runtime` does
    /// **not** qualify: it says the gateway cannot observe the agent right now.
    Absent,
    /// Everything else: unreachable socket, timeout, malformed reply, a reply
    /// that failed the challenge, or an `unknown-runtime` refusal. Never treated
    /// as gone.
    Unknown,
}

/// Splits `unix://<absolute path>` into the control socket path.
///
/// The judgement deliberately mirrors the App Server endpoint check in
/// `identity.rs`: an absolute path behind the `unix://` scheme. The new kind
/// does not get a relaxed check.
pub fn control_socket(endpoint: &str) -> Result<PathBuf, ControlError> {
    let path = endpoint
        .strip_prefix("unix://")
        .ok_or_else(|| ControlError::Rejected {
            code: "bad-candidate".into(),
            detail: format!("dsh endpoint must use the unix:// scheme, got {endpoint}"),
        })?;
    if !path.starts_with('/') {
        return Err(ControlError::Rejected {
            code: "bad-candidate".into(),
            detail: format!("dsh endpoint path must be absolute, got {path}"),
        });
    }
    Ok(PathBuf::from(path))
}

/// A fresh single-use challenge nonce. Never reused, so a captured reply from an
/// earlier challenge can never satisfy a later one; the gateway keeps no nonce
/// history.
fn fresh_nonce() -> String {
    use rand::Rng;
    let mut bytes = [0u8; 16];
    rand::thread_rng().fill(&mut bytes);
    let mut nonce = String::with_capacity(32);
    for byte in bytes {
        nonce.push_str(&format!("{byte:02x}"));
    }
    nonce
}

fn round_trip(
    socket: &Path,
    method: &str,
    params: serde_json::Value,
) -> Result<serde_json::Value, ControlError> {
    let mut stream = UnixStream::connect(socket).map_err(|error| ControlError::Unreachable {
        detail: format!(
            "gateway control socket {} is unreachable: {error}",
            socket.display()
        ),
    })?;
    stream
        .set_read_timeout(Some(CONTROL_TIMEOUT))
        .map_err(|error| ControlError::Unusable {
            detail: format!("cannot bound {method} read: {error}"),
        })?;
    stream
        .set_write_timeout(Some(CONTROL_TIMEOUT))
        .map_err(|error| ControlError::Unusable {
            detail: format!("cannot bound {method} write: {error}"),
        })?;
    let line = serde_json::to_string(&serde_json::json!({
        "method": method,
        "params": params,
    }))
    .map_err(|error| ControlError::Unusable {
        detail: format!("cannot encode {method}: {error}"),
    })?;
    stream
        .write_all(line.as_bytes())
        .and_then(|()| stream.write_all(b"\n"))
        .and_then(|()| stream.flush())
        .map_err(|error| ControlError::Unusable {
            detail: format!(
                "cannot send {method} to {}: {error}; the request may already have reached the gateway",
                socket.display()
            ),
        })?;
    let mut reader = BufReader::new(stream);
    let mut buf = String::new();
    let read = reader
        .read_line(&mut buf)
        .map_err(|error| match error.kind() {
            std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut => {
                ControlError::Unusable {
                    detail: format!(
                        "gateway did not answer {method} within {}s",
                        CONTROL_TIMEOUT.as_secs()
                    ),
                }
            }
            _ => ControlError::Unreachable {
                detail: format!("reading {method} reply: {error}"),
            },
        })?;
    if read == 0 {
        return Err(ControlError::Unusable {
            detail: format!("gateway closed the control socket before answering {method}"),
        });
    }
    let parsed: serde_json::Value =
        serde_json::from_str(buf.trim()).map_err(|error| ControlError::Unusable {
            detail: format!("malformed {method} reply: {error}"),
        })?;
    if parsed.get("ok").and_then(serde_json::Value::as_bool) != Some(true) {
        let code = parsed
            .pointer("/error/code")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("unknown-error")
            .to_owned();
        let message = parsed
            .pointer("/error/message")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("");
        return Err(ControlError::Rejected {
            detail: format!("gateway refused {method} with {code}: {message}"),
            code,
        });
    }
    parsed
        .get("result")
        .cloned()
        .ok_or_else(|| ControlError::Unusable {
            detail: format!("{method} reply carries no result"),
        })
}

fn rejected(detail: String) -> ControlError {
    ControlError::Rejected {
        code: "challenge-mismatch".into(),
        detail,
    }
}

fn receipt_mismatch(detail: String) -> ControlError {
    ControlError::Rejected {
        code: "receipt-mismatch".into(),
        detail,
    }
}

/// Runs one single-use challenge against the gateway and returns the facts it
/// reported. The nonce is minted here, echoed by the gateway and checked here,
/// so the reply is bound to this one challenge.
pub fn facts(endpoint: &str, runtime_id: &str, agent_id: &str) -> Result<AgentFacts, ControlError> {
    let socket = control_socket(endpoint)?;
    let nonce = fresh_nonce();
    let result = round_trip(
        &socket,
        "agent-facts",
        serde_json::json!({
            "nonce": nonce,
            "runtimeId": runtime_id,
            "agentId": agent_id,
        }),
    )?;
    let echoed = result
        .get("nonce")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| rejected("agent-facts reply carries no nonce".into()))?;
    if echoed != nonce {
        return Err(rejected(format!(
            "agent-facts echoed nonce {echoed} for challenge {nonce}"
        )));
    }
    let field = |name: &str| -> Result<String, ControlError> {
        result
            .get(name)
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| rejected(format!("agent-facts reply is missing {name}")))
    };
    Ok(AgentFacts {
        runtime_id: field("runtimeId")?,
        agent_id: field("agentId")?,
        session_id: field("sessionId")?,
        cwd: field("cwd")?,
        status: field("status")?,
    })
}

/// Resolve one existing session through its committed control endpoint. This
/// exposes no registry and grants no new identity or lifecycle authority.
pub fn resolve_agent(
    endpoint: &str,
    agent_id: &str,
    session_id: &str,
    cwd: &Path,
) -> Result<AgentFacts, ControlError> {
    let nonce = fresh_nonce();
    let result = round_trip(
        &control_socket(endpoint)?,
        "resolve-agent",
        serde_json::json!({
            "nonce": nonce, "agentId": agent_id, "cwd": cwd,
        }),
    )?;
    let field = |name: &str| {
        result[name]
            .as_str()
            .map(str::to_owned)
            .ok_or_else(|| rejected(format!("resolve-agent reply is missing {name}")))
    };
    if field("nonce")? != nonce
        || field("agentId")? != agent_id
        || field("sessionId")? != session_id
    {
        return Err(rejected("resolve-agent session or nonce mismatch".into()));
    }
    let observed_cwd = field("cwd")?;
    let canonical = |p: &Path| {
        std::fs::canonicalize(p)
            .map_err(|error| rejected(format!("resolve-agent cwd is unavailable: {error}")))
    };
    if canonical(Path::new(&observed_cwd))? != canonical(cwd)? {
        return Err(rejected("resolve-agent project mismatch".into()));
    }
    let runtime_id = field("runtimeId")?;
    let status = field("status")?;
    if runtime_id.is_empty() || !matches!(status.as_str(), "running" | "inactive") {
        return Err(rejected(
            "resolve-agent returned invalid runtime or status".into(),
        ));
    }
    Ok(AgentFacts {
        runtime_id,
        agent_id: agent_id.into(),
        session_id: session_id.into(),
        cwd: observed_cwd,
        status,
    })
}

/// Hands the committed collab binding to the selected gateway.
///
/// The gateway is the only owner of the canonical Poll receive loop. Collab
/// remains the only `Register` owner: this call adopts the already-committed
/// identity instead of minting one. Repeating it is the ordinary recovery
/// path, so the gateway must answer idempotently.
pub fn enroll_receive_loop(
    endpoint: &str,
    enrollment: &DshLoopEnrollment<'_>,
) -> Result<(), ControlError> {
    let result = round_trip(
        &control_socket(endpoint)?,
        "register-peer",
        serde_json::json!({
            "runtimeId": enrollment.runtime_id,
            "agentId": enrollment.agent_id,
            "binding": {
                "workerId": enrollment.worker_id,
                "token": enrollment.token,
                "projectScope": enrollment.project_scope,
                "runtimeContext": enrollment.runtime_context.to_json(),
            }
        }),
    )?;
    validate_enrollment_receipt(&result, enrollment)
}

fn validate_enrollment_receipt(
    result: &serde_json::Value,
    enrollment: &DshLoopEnrollment<'_>,
) -> Result<(), ControlError> {
    let string_field = |name: &str| -> Result<&str, ControlError> {
        result
            .get(name)
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| receipt_mismatch(format!("register-peer receipt is missing {name}")))
    };
    for (name, expected) in [
        ("runtimeId", enrollment.runtime_id),
        ("agentId", enrollment.agent_id),
        ("workerId", enrollment.worker_id),
        ("projectScope", enrollment.project_scope),
        ("bindingId", enrollment.runtime_context.binding_id),
    ] {
        let actual = string_field(name)?;
        if actual != expected {
            return Err(receipt_mismatch(format!(
                "register-peer receipt {name} {actual} does not match committed {expected}"
            )));
        }
    }
    let generation = result
        .get("endpointGeneration")
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| {
            receipt_mismatch("register-peer receipt is missing endpointGeneration".into())
        })?;
    if generation != enrollment.runtime_context.endpoint_generation {
        return Err(receipt_mismatch(format!(
            "register-peer receipt endpointGeneration {generation} does not match committed {}",
            enrollment.runtime_context.endpoint_generation
        )));
    }
    if result.get("token").is_some() {
        return Err(receipt_mismatch(
            "register-peer receipt must not echo the credential".into(),
        ));
    }
    Ok(())
}

/// Classifies one peer's liveness. `Absent` requires the gateway to say so
/// explicitly; every other failure is `Unknown`.
pub fn probe(endpoint: &str, runtime_id: &str, agent_id: &str) -> PeerPresence {
    match facts(endpoint, runtime_id, agent_id) {
        Ok(_) => PeerPresence::Live,
        Err(error) if error.is_definitely_absent() => PeerPresence::Absent,
        Err(_) => PeerPresence::Unknown,
    }
}

/// The `messageId` the gateway deduplicates wakes on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentReply {
    Pending,
    Replied {
        agent_id: String,
        message_id: String,
        session_id: String,
        turn: u64,
        step: u64,
        input_seq: u64,
        output_seq: u64,
        completed_seq: u64,
        text: String,
    },
}

pub fn agent_reply(
    endpoint: &str,
    runtime_id: &str,
    agent_id: &str,
    message_id: &str,
) -> Result<AgentReply, ControlError> {
    let outer = round_trip(
        &control_socket(endpoint)?,
        "agent-reply",
        serde_json::json!({
            "runtimeId":runtime_id,"agentId":agent_id,"messageId":message_id
        }),
    )?;
    let result = outer
        .get("result")
        .ok_or_else(|| rejected("agent-reply answer carries no gateway result".into()))?;
    match result["state"].as_str() {
        Some("pending") => Ok(AgentReply::Pending),
        Some("replied") => {
            let field = |name: &str| {
                result[name]
                    .as_str()
                    .map(str::to_owned)
                    .ok_or_else(|| rejected(format!("agent-reply missing {name}")))
            };
            let sequence = |name: &str| {
                result[name]
                    .as_u64()
                    .ok_or_else(|| rejected(format!("agent-reply missing {name}")))
            };
            Ok(AgentReply::Replied {
                agent_id: field("agentId")?,
                message_id: field("messageId")?,
                session_id: field("sessionId")?,
                turn: sequence("turn")?,
                step: sequence("step")?,
                input_seq: sequence("inputSeq")?,
                output_seq: sequence("outputSeq")?,
                completed_seq: sequence("completedSeq")?,
                text: field("text")?,
            })
        }
        _ => Err(rejected(
            "agent-reply state must be pending or replied".into(),
        )),
    }
}

pub fn registration_message_id(operation_id: &str) -> Result<String, ControlError> {
    let paths = crate::scope::HostPaths::resolve().map_err(|error| ControlError::Unusable {
        detail: error.to_string(),
    })?;
    let digest = Sha256::digest(paths.state_root().to_string_lossy().as_bytes());
    let prefix: String = digest
        .iter()
        .take(8)
        .map(|byte| format!("{byte:02x}"))
        .collect();
    Ok(format!("collab:{prefix}:register-{operation_id}"))
}

/// The `messageId` the gateway deduplicates wakes on.
///
/// The gateway folds every `enqueue` on this id, and collab replays its own
/// wakes (`daemon-live-closure`, `restart-replay-pending`), so the id must be
/// derived from collab's own `msg_id` rather than minted per attempt. The state
/// root namespaces it, because two collab instances on one host may otherwise
/// mint the same `m<ms>-<n>`; the digest is 64 bits wide because a narrower
/// prefix re-introduces birthday collisions, and a collision silently drops the
/// second message under "first enqueue wins".
///
/// The gateway validates the shape `^collab:[0-9a-f]{16}:m[0-9]+-[0-9]+$` and
/// answers `invalid-message-id` otherwise, so a wrong namespace is a loud
/// failure rather than a silent loss.
pub fn wake_message_id(msg_id: &str) -> Result<String, ControlError> {
    let paths = crate::scope::HostPaths::resolve().map_err(|error| ControlError::Unusable {
        detail: format!("collab state root is unavailable for the dsh messageId: {error}"),
    })?;
    let mut hasher = Sha256::new();
    hasher.update(paths.state_root().to_string_lossy().as_bytes());
    let digest = hasher.finalize();
    let mut prefix = String::with_capacity(16);
    for byte in digest.iter().take(8) {
        use std::fmt::Write;
        write!(&mut prefix, "{byte:02x}").expect("writing to String cannot fail");
    }
    Ok(format!("collab:{prefix}:{msg_id}"))
}

/// Delivers one wake to a single agent through `enqueue`.
///
/// The reply is a local admission receipt only: a `messageId` means the gateway
/// accepted the message into its durable queue. It never means the agent ran.
pub fn notify(
    endpoint: &str,
    runtime_id: &str,
    agent_id: &str,
    mode: &str,
    sender_id: &str,
    text: &str,
    msg_id: &str,
) -> Result<serde_json::Value, ControlError> {
    notify_with_message_id(
        endpoint,
        runtime_id,
        agent_id,
        mode,
        sender_id,
        text,
        &wake_message_id(msg_id)?,
    )
}

pub fn notify_with_message_id(
    endpoint: &str,
    runtime_id: &str,
    agent_id: &str,
    mode: &str,
    sender_id: &str,
    text: &str,
    message_id: &str,
) -> Result<serde_json::Value, ControlError> {
    let socket = control_socket(endpoint)?;
    let result = round_trip(
        &socket,
        "enqueue",
        serde_json::json!({
            "runtimeId": runtime_id,
            "agentId": agent_id,
            "mode": mode,
            "content": [{"type": "text", "text": text}],
            "sender": {"runtimeId": "collab", "id": sender_id, "name": "collab"},
            "messageId": message_id,
        }),
    )?;
    let message_id = result
        .get("messageId")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| ControlError::Unusable {
            detail: "enqueue reply carries no messageId".into(),
        })?;
    Ok(serde_json::json!({
        "transport": "dsh",
        "message_id": message_id,
        "queued": true,
        "consumed": false
    }))
}

#[cfg(test)]
mod enrollment_tests {
    use super::*;
    use serde_json::json;

    fn committed() -> DshLoopEnrollment<'static> {
        DshLoopEnrollment {
            runtime_id: "rt-gateway",
            agent_id: "native-thread-A",
            worker_id: "workerA",
            token: "tokenA",
            project_scope: "/project",
            runtime_context: DshRuntimeContext {
                agent_id: "workerA",
                runtime_id: "runtime-collab-A",
                appserver_id: "appserver-A",
                binding_id: "binding-1",
                endpoint_generation: 2,
                session_id: Some("session-A"),
                native_thread_id: Some("native-thread-A"),
            },
        }
    }

    fn receipt() -> serde_json::Value {
        json!({
            "runtimeId": "rt-gateway",
            "agentId": "native-thread-A",
            "workerId": "workerA",
            "projectScope": "/project",
            "bindingId": "binding-1",
            "endpointGeneration": 2
        })
    }

    fn is_receipt_mismatch(result: Result<(), ControlError>) -> bool {
        matches!(
            result,
            Err(ControlError::Rejected { code, .. }) if code == "receipt-mismatch"
        )
    }

    #[test]
    fn enrollment_receipt_must_match_the_committed_binding() {
        let committed = committed();
        assert!(validate_enrollment_receipt(&receipt(), &committed).is_ok());
        for (pointer, drifted) in [
            ("/runtimeId", json!("rt-2")),
            ("/agentId", json!("agentB")),
            ("/workerId", json!("workerB")),
            ("/projectScope", json!("/other")),
            ("/bindingId", json!("binding-2")),
            ("/endpointGeneration", json!(3)),
        ] {
            let mut result = receipt();
            *result.pointer_mut(pointer).expect("receipt field") = drifted;
            assert!(
                is_receipt_mismatch(validate_enrollment_receipt(&result, &committed)),
                "{pointer} drift must be rejected"
            );
        }
        let mut missing = receipt();
        missing.as_object_mut().unwrap().remove("runtimeId");
        assert!(is_receipt_mismatch(validate_enrollment_receipt(
            &missing, &committed
        )));
        let mut echoed = receipt();
        echoed["token"] = json!("tokenA");
        assert!(is_receipt_mismatch(validate_enrollment_receipt(
            &echoed, &committed
        )));
    }

    #[test]
    fn runtime_context_is_authoritative_and_separate_from_gateway_addressing() {
        let committed = committed();
        let context = committed.runtime_context.to_json();
        assert_eq!(context["agent_id"], "workerA");
        assert_eq!(context["runtime_id"], "runtime-collab-A");
        assert_eq!(context["appserver_id"], "appserver-A");
        assert_eq!(context["binding_id"], "binding-1");
        assert_eq!(context["endpoint_generation"], 2);
        assert_eq!(context["session_id"], "session-A");
        assert_eq!(context["native_thread_id"], "native-thread-A");
        assert_ne!(context["runtime_id"], committed.runtime_id);
        assert_ne!(context["agent_id"], committed.agent_id);
        assert_eq!(context["agent_id"], committed.worker_id);
    }
}
