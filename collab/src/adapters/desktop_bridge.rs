//! Restricted callback into the Desktop-owned MCP process. This adapter does
//! not connect to the host tools pipe or select caller metadata.
use super::dsh::{connect_with_deadline, remaining_time, DeadlineStream};
use crate::proto::{AppServerCandidate, SelectedTransport, TransportKind};
use serde_json::{json, Value};
use std::io::{Read, Write};
use std::path::Path;
use std::time::{Duration, Instant};

const LIMIT: usize = 8 * 1024 * 1024;
pub const NOT_SUBMITTED_PREFIX: &str = "DESKTOP_BRIDGE_NOT_SUBMITTED:";

fn exchange(transport: &SelectedTransport, op: &str, text: Option<&str>) -> Result<Value, String> {
    exchange_with_deadline(transport, op, text, None)
}

fn exchange_with_deadline(
    transport: &SelectedTransport,
    op: &str,
    text: Option<&str>,
    deadline: Option<Instant>,
) -> Result<Value, String> {
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
    let socket = connect_with_deadline(Path::new(path), deadline).map_err(|error| {
        if deadline.is_some()
            || matches!(
                error.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused
            )
        {
            format!("{NOT_SUBMITTED_PREFIX} {error}")
        } else {
            fail(&error.to_string())
        }
    })?;
    if deadline.is_none() {
        socket
            .set_read_timeout(Some(Duration::from_secs(15)))
            .map_err(|e| fail(&e.to_string()))?;
        socket
            .set_write_timeout(Some(Duration::from_secs(15)))
            .map_err(|e| fail(&e.to_string()))?;
    }
    let mut socket = DeadlineStream::new(socket, deadline);
    // A failed/unknown send is returned to the caller. Never replay here.
    socket
        .write_all(&(bytes.len() as u32).to_le_bytes())
        .and_then(|()| socket.write_all(&bytes))
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
    if let Some(until) = deadline {
        remaining_time(until).map_err(|e| fail(&e.to_string()))?;
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

pub fn guarded_send(
    transport: &SelectedTransport,
    text: &str,
    deadline: Duration,
) -> Result<Value, String> {
    exchange_with_deadline(
        transport,
        "send",
        Some(text),
        Some(Instant::now() + deadline),
    )
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
        rex_endpoint: None,
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

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::thread;

    static SEQ: AtomicU64 = AtomicU64::new(0);

    pub(crate) struct TrickleBridge {
        dir: std::path::PathBuf,
        socket: std::path::PathBuf,
        handle: Option<thread::JoinHandle<()>>,
    }

    impl TrickleBridge {
        pub(crate) fn serve() -> Self {
            Self::serve_with(|mut stream| {
                let mut header = [0; 4];
                if stream.read_exact(&mut header).is_err() {
                    return;
                }
                let length = u32::from_le_bytes(header) as usize;
                let mut request = vec![0; length];
                if stream.read_exact(&mut request).is_err() {
                    return;
                }
                let request: Value = serde_json::from_slice(&request).unwrap();
                let body = serde_json::json!({
                    "id": request["id"],
                    "ok": true,
                    "session_id": "session-1",
                    "thread_id": "thread-1",
                    "native": {"success": true}
                });
                let bytes = serde_json::to_vec(&body).unwrap();
                let _ = stream.write_all(&(bytes.len() as u32).to_le_bytes());
                let (prefix, rest) = bytes.split_at(12);
                for byte in prefix {
                    if stream.write_all(&[*byte]).is_err() {
                        return;
                    }
                    thread::sleep(Duration::from_millis(70));
                }
                let _ = stream.write_all(rest);
            })
        }

        fn serve_with(handle: impl FnOnce(UnixStream) + Send + 'static) -> Self {
            let id = SEQ.fetch_add(1, Ordering::Relaxed);
            let dir = std::env::temp_dir().join(format!(
                "collab-desktop-trickle-{}-{id}",
                std::process::id()
            ));
            std::fs::create_dir_all(&dir).expect("create fixture directory");
            let socket = dir.join("bridge.sock");
            let listener = UnixListener::bind(&socket).expect("bind bridge socket");
            let handle = thread::spawn(move || {
                if let Ok((stream, _)) = listener.accept() {
                    handle(stream);
                }
            });
            Self {
                dir,
                socket,
                handle: Some(handle),
            }
        }

        pub(crate) fn transport(&self) -> SelectedTransport {
            SelectedTransport {
                kind: TransportKind::DesktopBridge,
                endpoint: Some(format!("unix://{}", self.socket.display())),
                namespace: Some("codex_app".into()),
                session_id: Some("session-1".into()),
                thread_id: Some("thread-1".into()),
                tmux_endpoint: None,
                rex_endpoint: None,
                capabilities: vec!["send_message_to_thread".into()],
                self_check: "test".into(),
            }
        }
    }

    impl Drop for TrickleBridge {
        fn drop(&mut self) {
            if self.handle.is_some() {
                let _ = std::os::unix::net::UnixStream::connect(&self.socket);
                if let Some(handle) = self.handle.take() {
                    let _ = handle.join();
                }
            }
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    #[test]
    fn ordinary_send_accepts_a_complete_trickled_frame() {
        let bridge = TrickleBridge::serve();
        assert_eq!(send(&bridge.transport(), "wake").unwrap()["success"], true);
    }

    #[test]
    fn guarded_send_expired_before_connect_is_known_not_submitted() {
        let bridge = TrickleBridge::serve();
        let error = guarded_send(&bridge.transport(), "wake", Duration::ZERO).unwrap_err();
        assert!(error.starts_with(NOT_SUBMITTED_PREFIX), "{error}");
    }

    #[test]
    fn guarded_send_stops_a_blocked_request_write() {
        let (header_seen, seen) = std::sync::mpsc::channel();
        let (release, wait) = std::sync::mpsc::channel();
        let bridge = TrickleBridge::serve_with(move |mut stream| {
            let mut header = [0; 4];
            stream.read_exact(&mut header).unwrap();
            header_seen.send(u32::from_le_bytes(header)).unwrap();
            let _ = wait.recv_timeout(Duration::from_secs(1));
        });
        let text = "x".repeat(2 * 1024 * 1024);
        let started = Instant::now();
        let error = guarded_send(&bridge.transport(), &text, Duration::from_millis(200))
            .expect_err("a peer that does not read the request must exceed the write budget");
        assert!(seen.recv_timeout(Duration::from_millis(200)).unwrap() as usize > text.len());
        assert!(started.elapsed() < Duration::from_millis(800));
        assert_eq!(
            crate::client::adapters::AdapterError::notification_class_from_display(&error),
            crate::client::adapters::NotificationDeliveryClass::Unknown,
            "{error}"
        );
        release.send(()).unwrap();
    }

    #[test]
    fn guarded_send_uses_one_budget_for_a_partial_framed_reply() {
        let bridge = TrickleBridge::serve();
        let started = Instant::now();
        let error = guarded_send(&bridge.transport(), "wake", Duration::from_millis(150))
            .expect_err("partial Desktop reply must time out");
        assert_eq!(
            crate::client::adapters::AdapterError::notification_class_from_display(&error),
            crate::client::adapters::NotificationDeliveryClass::Unknown,
            "{error}"
        );
        assert!(
            started.elapsed() < Duration::from_millis(600),
            "continuous partial response extended the operation: {:?}",
            started.elapsed()
        );
    }
}
