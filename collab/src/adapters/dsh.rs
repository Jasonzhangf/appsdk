//! dsh-gateway adapter: collab's third transport channel.
//!
//! Collab speaks to the dsh-gateway control plane over its NDJSON UNIX socket.
//! Per the frozen interface (`docs/COLLAB-DSH-CHANNEL-DESIGN.md` section 3.7)
//! only two methods are ever issued from here:
//!
//! - `agent-facts`: the read-only single-response challenge that admits a
//!   `DshCandidate`, and re-verifies it when a route is resolved.
//! - `enqueue`: delivers one wake to one agent.
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

impl ControlError {
    /// True only when the gateway explicitly denied knowing this runtime or
    /// agent. A timeout, an unreachable socket or a failed nonce check all
    /// leave the peer's state unknown, and unknown must never be read as gone.
    pub fn is_definitely_absent(&self) -> bool {
        matches!(
            self,
            Self::Rejected { code, .. } if code == "unknown-runtime" || code == "unknown-agent"
        )
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
    /// The gateway explicitly denies knowing this runtime or agent.
    Absent,
    /// Everything else: unreachable socket, timeout, malformed reply, or a
    /// reply that failed the challenge. Never treated as gone.
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

/// Classifies one peer's liveness. `Absent` requires the gateway to say so
/// explicitly; every other failure is `Unknown`.
pub fn probe(endpoint: &str, runtime_id: &str, agent_id: &str) -> PeerPresence {
    match facts(endpoint, runtime_id, agent_id) {
        Ok(_) => PeerPresence::Live,
        Err(error) if error.is_definitely_absent() => PeerPresence::Absent,
        Err(_) => PeerPresence::Unknown,
    }
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
