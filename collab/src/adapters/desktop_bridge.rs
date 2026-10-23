//! Restricted callback into the Desktop-owned MCP process. This adapter does
//! not connect to the host tools pipe or select caller metadata.
use crate::proto::{AppServerCandidate, SelectedTransport, TransportKind};
use serde_json::{json, Value};
use std::io::{Read, Write};
use std::path::Path;
use std::time::Duration;

const LIMIT: usize = 8 * 1024 * 1024;

fn exchange(transport: &SelectedTransport, op: &str, text: Option<&str>) -> Result<Value, String> {
    let fail = |error: &str| format!("DESKTOP_BRIDGE_FAILED: {error}");
    let path = transport
        .endpoint
        .as_deref()
        .and_then(|s| s.strip_prefix("unix://"))
        .filter(|s| Path::new(s).is_absolute())
        .ok_or_else(|| fail("endpoint must be an absolute unix:// socket"))?;
    let session = transport
        .session_id
        .as_deref()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| fail("missing agent session"))?;
    let thread = transport
        .thread_id
        .as_deref()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| fail("missing Desktop thread"))?;
    static NEXT_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    let id = format!(
        "bridge-{}-{}",
        std::process::id(),
        NEXT_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    );
    let mut request = json!({"id":id,"op":op,"session_id":session,"thread_id":thread});
    if let Some(text) = text {
        request["text"] = json!(text);
    }
    let bytes = serde_json::to_vec(&request).map_err(|e| fail(&e.to_string()))?;
    if bytes.len() > LIMIT {
        return Err(fail("request exceeds frame limit"));
    }
    let mut socket =
        std::os::unix::net::UnixStream::connect(path).map_err(|e| fail(&e.to_string()))?;
    socket
        .set_read_timeout(Some(Duration::from_secs(15)))
        .map_err(|e| fail(&e.to_string()))?;
    socket
        .set_write_timeout(Some(Duration::from_secs(15)))
        .map_err(|e| fail(&e.to_string()))?;
    // A failed/unknown send is returned to the caller. Never replay here.
    socket
        .write_all(&(bytes.len() as u32).to_le_bytes())
        .and_then(|_| socket.write_all(&bytes))
        .map_err(|e| fail(&e.to_string()))?;
    let mut header = [0; 4];
    socket
        .read_exact(&mut header)
        .map_err(|e| fail(&e.to_string()))?;
    let length = u32::from_le_bytes(header) as usize;
    if length > LIMIT {
        return Err(fail("response exceeds frame limit"));
    }
    let mut body = vec![0; length];
    socket
        .read_exact(&mut body)
        .map_err(|e| fail(&e.to_string()))?;
    let reply: Value = serde_json::from_slice(&body).map_err(|e| fail(&e.to_string()))?;
    if reply["id"] != id {
        return Err(fail("response id mismatch"));
    }
    if reply["ok"] != true {
        return Err(format!("DESKTOP_BRIDGE_FAILED: {reply}"));
    }
    if reply["session_id"] != session || reply["thread_id"] != thread {
        return Err(fail("response target mismatch"));
    }
    if reply["native"]["success"] != true {
        return Err(format!(
            "DESKTOP_BRIDGE_FAILED: native result {}",
            reply["native"]
        ));
    }
    Ok(reply["native"].clone())
}

pub fn read_thread_status(transport: &SelectedTransport, thread: &str) -> Result<Value, String> {
    if transport.thread_id.as_deref() != Some(thread) {
        return Err(
            "DESKTOP_BRIDGE_FAILED: requested thread differs from registered target".into(),
        );
    }
    let native = exchange(transport, "probe", None)?;
    let payload = native["contentItems"]
        .as_array()
        .and_then(|items| {
            items.iter().find_map(|item| {
                (item["type"] == "inputText")
                    .then(|| item["text"].as_str())
                    .flatten()
            })
        })
        .ok_or_else(|| format!("DESKTOP_BRIDGE_FAILED: missing read_thread content: {native}"))?;
    let result: Value = serde_json::from_str(payload)
        .map_err(|e| format!("DESKTOP_BRIDGE_FAILED: invalid read_thread: {e}"))?;
    if result["thread"]["id"] != thread || !result["thread"]["status"].is_object() {
        return Err(format!(
            "DESKTOP_BRIDGE_FAILED: invalid read_thread target/status: {result}"
        ));
    }
    Ok(result)
}

pub fn send(transport: &SelectedTransport, text: &str) -> Result<Value, String> {
    exchange(transport, "send", Some(text))
}

pub fn verify_candidate(candidate: &AppServerCandidate) -> Result<SelectedTransport, String> {
    if candidate.namespace != "codex_app"
        || candidate.session_id.trim().is_empty()
        || candidate.thread_id.trim().is_empty()
    {
        return Err("DESKTOP_BRIDGE_REJECTED: incomplete Desktop candidate".into());
    }
    let transport = SelectedTransport {
        kind: TransportKind::DesktopBridge,
        endpoint: Some(candidate.endpoint.clone()),
        namespace: Some(candidate.namespace.clone()),
        session_id: Some(candidate.session_id.clone()),
        thread_id: Some(candidate.thread_id.clone()),
        tmux_endpoint: None,
        capabilities: vec!["send_message_to_thread".into(), "read_thread".into()],
        self_check: "Desktop bridge verified registered thread and project".into(),
    };
    let result = read_thread_status(&transport, &candidate.thread_id)?;
    let cwd = result["thread"]["cwd"]
        .as_str()
        .ok_or("DESKTOP_BRIDGE_REJECTED: missing thread cwd")?;
    let native_root = std::fs::canonicalize(cwd)
        .map_err(|e| format!("DESKTOP_BRIDGE_REJECTED: native cwd: {e}"))?;
    let requested_root = std::fs::canonicalize(&candidate.cwd)
        .map_err(|e| format!("DESKTOP_BRIDGE_REJECTED: candidate cwd: {e}"))?;
    if native_root != requested_root {
        return Err(
            "DESKTOP_BRIDGE_REJECTED: native thread cwd differs from registered project".into(),
        );
    }
    Ok(transport)
}
