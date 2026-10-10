//! Compile-time-only cancellation barriers.
//!
//! This module exists only when `context-cancel-test-hooks` is enabled. It
//! never activates from a production environment variable because the feature
//! is default-off and the module is absent from default/release binaries.

use serde_json::json;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::time::Duration;

pub(crate) fn barrier(
    boundary: &str,
    operation_id: &str,
    pid: u32,
    nested_command_id: Option<&str>,
    nested_operation_id: Option<&str>,
    cancellation_requested: fn() -> bool,
) {
    let _ = barrier_reply(
        boundary,
        operation_id,
        pid,
        nested_command_id,
        nested_operation_id,
        cancellation_requested,
    );
}

pub(crate) fn barrier_reply(
    boundary: &str,
    operation_id: &str,
    pid: u32,
    nested_command_id: Option<&str>,
    nested_operation_id: Option<&str>,
    cancellation_requested: fn() -> bool,
) -> Option<String> {
    let Ok(socket) = std::env::var("COLLAB_CONTEXT_CANCEL_HOOK_SOCKET") else {
        return None;
    };
    let Ok(mut stream) = UnixStream::connect(&socket) else {
        return None;
    };
    let _ = stream.set_read_timeout(Some(Duration::from_millis(50)));
    let frame = json!({
        "boundary": boundary,
        "operation_id": operation_id,
        "pid": pid,
        "nested_command_id": nested_command_id,
        "nested_operation_id": nested_operation_id,
    });
    let Ok(mut encoded) = serde_json::to_vec(&frame) else {
        return None;
    };
    encoded.push(b'\n');
    if stream.write_all(&encoded).is_err() || stream.flush().is_err() {
        return None;
    }
    let mut reader = BufReader::new(stream);
    let mut release = String::new();
    let mut signal_acknowledged = false;
    loop {
        match reader.read_line(&mut release) {
            Ok(0) => return None,
            Ok(_) => return Some(release.trim().to_owned()),
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::Interrupted
                        | std::io::ErrorKind::WouldBlock
                        | std::io::ErrorKind::TimedOut
                ) =>
            {
                // Buffered read may retry EINTR internally. Observe the
                // production signal flag at this feature-only boundary so the
                // fixture can release only after cancellation was consumed.
                if boundary == "client_pre_send" && !signal_acknowledged && cancellation_requested()
                {
                    acknowledge_signal(&socket, boundary, operation_id, pid);
                    signal_acknowledged = true;
                }
                continue;
            }
            Err(_) => return None,
        }
    }
}

/// Best-effort secondary frame recording that a cancellation signal was
/// consumed at the pre-send boundary. It carries no capability or credential
/// and is only meaningful to the private test harness.
fn acknowledge_signal(socket: &str, boundary: &str, operation_id: &str, pid: u32) {
    let Ok(mut stream) = UnixStream::connect(socket) else {
        return;
    };
    let frame = json!({
        "boundary": format!("{boundary}_signalled"),
        "operation_id": operation_id,
        "pid": pid,
        "nested_command_id": Option::<String>::None,
        "nested_operation_id": Option::<String>::None,
    });
    let Ok(mut encoded) = serde_json::to_vec(&frame) else {
        return;
    };
    encoded.push(b'\n');
    let _ = stream.write_all(&encoded);
    let _ = stream.flush();
}
