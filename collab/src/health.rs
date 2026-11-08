//! Read-only release capability acceptance for `collab status --check`.
//!
//! This module deliberately reuses the existing retained-identity map and the
//! daemon's pure projections. It never bootstraps context, mutates a mailbox,
//! sends a message, restarts a daemon, or executes a host adapter.

use anyhow::Context;
use serde_json::{json, Value};
use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use crate::identity::{CommandId, OperationId};
use crate::main_context::collect_identity_facts;
use crate::proto::{CommandEnvelope, ProjectContext, Req};
use crate::scope::{HostPaths, RouteScope, Scope};

const CHECK_DEADLINE: Duration = Duration::from_secs(5);
const MCP_PROTOCOL_VERSION: &str = "2025-03-26";
const REQUIRED_MCP_TOOLS: [&str; 4] = [
    "collab_context",
    "collab_recv",
    "collab_sendmessage",
    "collab_board_show",
];

/// Pending activation is a typed exit-3 outcome, not a generic failure.
#[derive(Debug)]
pub(crate) struct HealthError {
    report: Value,
}

impl HealthError {
    pub(crate) fn pending(report: Value) -> Self {
        Self { report }
    }

    pub(crate) fn exit_code(&self) -> i32 {
        3
    }

    pub(crate) fn emit(&self) {
        println!("{}", serde_json::to_string_pretty(&self.report).unwrap());
    }
}

impl std::fmt::Display for HealthError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("collab capability check pending user activation")
    }
}

impl std::error::Error for HealthError {}

pub(crate) fn status_check(local_only: bool) -> anyhow::Result<()> {
    let deadline = Instant::now() + CHECK_DEADLINE;
    let scope = Scope::resolve()?;
    local_pair_check(&deadline)?;

    if local_only {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "outcome": "success",
                "checks": {
                    "local": {
                        "status": "passed",
                        "mcp_version": env!("COLLAB_VERSION")
                    }
                }
            }))
            .unwrap()
        );
        return Ok(());
    }

    let host_paths = HostPaths::resolve()?;
    let socket = host_paths.sock_path();
    let ping = ping_with_deadline(&socket, &deadline)?;
    let daemon_version = ping.get("daemon_version").and_then(Value::as_str);
    let capabilities = ping.get("capabilities").and_then(Value::as_array);

    match daemon_version {
        None => {
            return pending(
                "daemon_version_unavailable",
                json!({
                    "checks": {
                        "local": local_summary(),
                        "daemon": {"status": "passed", "daemon_version": null}
                    },
                    "socket": socket,
                }),
            );
        }
        Some(version) if version != env!("COLLAB_VERSION") => {
            return pending(
                "daemon_version_differs",
                json!({
                    "checks": {
                        "local": local_summary(),
                        "daemon": {"status": "passed", "daemon_version": version}
                    },
                    "expected_version": env!("COLLAB_VERSION"),
                    "observed_version": version,
                    "socket": socket,
                }),
            );
        }
        Some(version) => {
            let supported = capabilities.is_some_and(|list| {
                list.iter().any(|entry| {
                    entry.as_str() == Some(crate::proto::CAPABILITY_READINESS_PROTOCOL)
                })
            });
            if !supported {
                return pending(
                    "daemon_readiness_unsupported",
                    json!({
                        "checks": {
                            "local": local_summary(),
                            "daemon": {"status": "passed", "daemon_version": version}
                        },
                        "required_capability": crate::proto::CAPABILITY_READINESS_PROTOCOL,
                        "socket": socket,
                    }),
                );
            }
            let caller = retained_caller(&host_paths, &scope, &deadline)?;
            let Some(identity) = caller else {
                return pending(
                    "caller_context_required",
                    json!({
                        "checks": {
                            "local": local_summary(),
                            "daemon": {"status": "passed", "daemon_version": version},
                            "caller": {"status": "missing"}
                        },
                        "socket": socket,
                    }),
                );
            };
            let capabilities = readiness_request(&host_paths, &scope, &identity, &deadline)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({
                    "outcome": "success",
                    "checks": {
                        "local": local_summary(),
                        "daemon": {"status": "passed", "daemon_version": version},
                        "caller": {
                            "status": "passed",
                            "worker_id": identity.worker_id
                        },
                        "capabilities": capabilities
                    },
                    "socket": socket
                }))
                .unwrap()
            );
        }
    }
    Ok(())
}

fn local_summary() -> Value {
    json!({"status": "passed", "mcp_version": env!("COLLAB_VERSION")})
}

fn pending<T>(reason: &str, mut report: Value) -> anyhow::Result<T> {
    report["outcome"] = json!("pending_user_activation");
    report["required_action"] = json!(reason);
    Err(HealthError::pending(report).into())
}

fn ping_with_deadline(socket: &Path, deadline: &Instant) -> anyhow::Result<Value> {
    let stream = match UnixStream::connect(socket) {
        Ok(stream) => stream,
        Err(error) if daemon_definitely_absent(socket, &error) => {
            return pending(
                "daemon_absent",
                json!({
                    "checks": {
                        "local": local_summary(),
                        "daemon": {"status": "absent"}
                    },
                    "socket": socket,
                }),
            );
        }
        Err(error) => {
            return Err(error).with_context(|| {
                format!(
                    "DAEMON_UNAVAILABLE: cannot reach collab daemon at {}",
                    socket.display()
                )
            });
        }
    };
    crate::client::call_with_stream_deadline(socket, stream, &Req::Ping, None, *deadline)
}

fn daemon_definitely_absent(socket: &Path, error: &std::io::Error) -> bool {
    let _ = socket;
    crate::client::stale_socket_error(error)
}

fn retained_caller(
    host_paths: &HostPaths,
    scope: &Scope,
    _deadline: &Instant,
) -> anyhow::Result<Option<crate::identity::Identity>> {
    let facts = collect_identity_facts(None)?;
    crate::identity::registered_from_map(host_paths, scope, &facts)
}

fn readiness_request(
    host_paths: &HostPaths,
    scope: &Scope,
    identity: &crate::identity::Identity,
    deadline: &Instant,
) -> anyhow::Result<Value> {
    let runtime = identity.runtime.clone().ok_or_else(|| {
        anyhow::anyhow!("CALLER_IDENTITY_REQUIRED: retained caller has no runtime binding")
    })?;
    let project_scope = identity.project_scope.clone().ok_or_else(|| {
        anyhow::anyhow!("CALLER_IDENTITY_REQUIRED: retained caller has no project scope")
    })?;
    let project_context = ProjectContext::for_registered_route(&scope.root, &runtime)?;
    let command = CommandEnvelope::new(
        CommandId::new(format!("readiness-{}", runtime.binding_id.as_str()))
            .map_err(|error| anyhow::anyhow!("READINESS_COMMAND_ID_INVALID: {error}"))?,
        OperationId::new(format!("readiness-op-{}", runtime.binding_id.as_str()))
            .map_err(|error| anyhow::anyhow!("READINESS_OPERATION_ID_INVALID: {error}"))?,
        runtime.binding_id.clone(),
        RouteScope {
            app_scope_id: runtime.appserver_id.clone(),
            project_scope_id: project_scope,
        },
        None,
        None,
        None,
        None,
    );
    crate::client::call_with_deadline(
        &host_paths.sock_path(),
        &Req::CapabilityReadiness {
            worker_id: identity.worker_id.clone(),
            token: identity.token.clone(),
            command,
        },
        Some(project_context),
        *deadline,
    )
}

fn local_pair_check(deadline: &Instant) -> anyhow::Result<()> {
    let cli = std::env::current_exe().context(
        "COLLAB_HEALTH_LOCAL_CLI_RESOLVE_FAILED: cannot resolve the invoked collab binary",
    )?;
    let directory = cli.parent().ok_or_else(|| {
        anyhow::anyhow!("COLLAB_HEALTH_LOCAL_CLI_RESOLVE_FAILED: binary has no parent directory")
    })?;
    let mcp = directory.join("collab-mcp");
    if !mcp.is_file() {
        anyhow::bail!(
            "COLLAB_HEALTH_LOCAL_MCP_MISSING: expected sibling collab-mcp at {}",
            mcp.display()
        );
    }

    let mut child = Command::new(&mcp)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("COLLAB_HEALTH_LOCAL_MCP_SPAWN_FAILED: {}", mcp.display()))?;

    let mut stdin = child.stdin.take().expect("piped stdin");
    let mut stdout = child.stdout.take().expect("piped stdout");
    let mut output = Vec::new();

    write_jsonrpc(
        &mut stdin,
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {"protocolVersion": MCP_PROTOCOL_VERSION}
        }),
        deadline,
        "MCP initialize write",
    )?;
    write_jsonrpc(
        &mut stdin,
        json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}),
        deadline,
        "MCP tools/list write",
    )?;
    drop(stdin);

    let initialize = read_jsonrpc(
        &mut stdout,
        &mut output,
        deadline,
        "MCP initialize response",
    )?;
    let tools = read_jsonrpc(
        &mut stdout,
        &mut output,
        deadline,
        "MCP tools/list response",
    )?;
    validate_initialize(&initialize, &mcp)?;
    validate_tools(&tools, &mcp)?;

    reap_local_mcp(&mut child, deadline, &mcp)?;
    Ok(())
}

fn write_jsonrpc(
    writer: &mut impl Write,
    request: Value,
    deadline: &Instant,
    phase: &str,
) -> anyhow::Result<()> {
    let body = serde_json::to_vec(&request)?;
    writer
        .write_all(&body)
        .and_then(|()| writer.write_all(b"\n"))
        .and_then(|()| writer.flush())
        .with_context(|| {
            format!(
                "COLLAB_HEALTH_LOCAL_MCP_WRITE_FAILED: {phase} failed within {}s",
                CHECK_DEADLINE.as_secs()
            )
        })?;
    crate::client::remaining_before(*deadline, phase)?;
    Ok(())
}

fn read_jsonrpc(
    reader: &mut std::process::ChildStdout,
    buffer: &mut Vec<u8>,
    deadline: &Instant,
    phase: &str,
) -> anyhow::Result<Value> {
    let raw_fd = reader.as_raw_fd();
    set_nonblocking(raw_fd)?;
    loop {
        if let Some(value) = parse_frame(buffer)? {
            return Ok(value);
        }
        let remaining = crate::client::remaining_before(*deadline, phase)?;
        let poll_timeout_ms = remaining.as_millis().min(i32::MAX as u128) as i32;
        let mut poll_fd = libc::pollfd {
            fd: raw_fd,
            events: libc::POLLIN,
            revents: 0,
        };
        let ready = unsafe { libc::poll(&mut poll_fd, 1, poll_timeout_ms) };
        if ready == 0 {
            anyhow::bail!("COLLAB_HEALTH_TIMEOUT: {phase} exceeded the five-second check deadline");
        }
        if ready < 0 {
            return Err(std::io::Error::last_os_error())
                .with_context(|| format!("COLLAB_HEALTH_LOCAL_MCP_POLL_FAILED: {phase}"));
        }
        let mut chunk = [0u8; 4096];
        match reader.read(&mut chunk) {
            Ok(0) => {
                anyhow::bail!("COLLAB_HEALTH_LOCAL_MCP_CLOSED: sibling MCP closed before {phase}");
            }
            Ok(read) => buffer.extend_from_slice(&chunk[..read]),
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
                ) => {}
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("COLLAB_HEALTH_LOCAL_MCP_READ_FAILED: {phase}"));
            }
        }
    }
}

fn parse_frame(buffer: &mut Vec<u8>) -> anyhow::Result<Option<Value>> {
    let lower = String::from_utf8_lossy(buffer).to_ascii_lowercase();
    if lower.starts_with("content-length:") {
        let header_end = find_header_end(buffer);
        let Some(header_end) = header_end else {
            return Ok(None);
        };
        let header = std::str::from_utf8(&buffer[..header_end])
            .map_err(|error| anyhow::anyhow!("COLLAB_HEALTH_LOCAL_MCP_FRAME_INVALID: {error}"))?;
        let length = header
            .split(':')
            .nth(1)
            .and_then(|value| value.trim().parse::<usize>().ok())
            .ok_or_else(|| {
                anyhow::anyhow!("COLLAB_HEALTH_LOCAL_MCP_FRAME_INVALID: invalid Content-Length")
            })?;
        let body_start = header_end;
        if buffer.len() < body_start + length {
            return Ok(None);
        }
        let body = buffer[body_start..body_start + length].to_vec();
        buffer.drain(..body_start + length);
        let value = serde_json::from_slice(&body)
            .map_err(|error| anyhow::anyhow!("COLLAB_HEALTH_LOCAL_MCP_FRAME_INVALID: {error}"))?;
        return Ok(Some(value));
    }
    let Some(newline) = buffer.iter().position(|byte| *byte == b'\n') else {
        return Ok(None);
    };
    let line = buffer[..newline].to_vec();
    buffer.drain(..=newline);
    if line.iter().all(|byte| matches!(byte, b'\r' | b' ' | b'\t')) {
        return Ok(None);
    }
    serde_json::from_slice(&line)
        .map(Some)
        .map_err(|error| anyhow::anyhow!("COLLAB_HEALTH_LOCAL_MCP_FRAME_INVALID: {error}"))
}

fn find_header_end(buffer: &[u8]) -> Option<usize> {
    buffer
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .map(|index| index + 4)
}

fn set_nonblocking(fd: i32) -> anyhow::Result<()> {
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 {
        return Err(std::io::Error::last_os_error())
            .context("COLLAB_HEALTH_LOCAL_MCP_FCNTL_FAILED");
    }
    let result = unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) };
    if result < 0 {
        return Err(std::io::Error::last_os_error())
            .context("COLLAB_HEALTH_LOCAL_MCP_FCNTL_FAILED");
    }
    Ok(())
}

fn validate_initialize(result: &Value, mcp: &Path) -> anyhow::Result<()> {
    ensure_success(result)?;
    let version = result
        .pointer("/result/serverInfo/version")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            anyhow::anyhow!(
                "COLLAB_HEALTH_LOCAL_MCP_VERSION_UNAVAILABLE: {} did not report serverInfo.version",
                mcp.display()
            )
        })?;
    if version != env!("COLLAB_VERSION") {
        anyhow::bail!(
            "COLLAB_HEALTH_LOCAL_MCP_VERSION_DIFFERS: expected {}, observed {}",
            env!("COLLAB_VERSION"),
            version
        );
    }
    Ok(())
}

fn validate_tools(result: &Value, mcp: &Path) -> anyhow::Result<()> {
    ensure_success(result)?;
    let tools = result
        .pointer("/result/tools")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            anyhow::anyhow!(
                "COLLAB_HEALTH_LOCAL_MCP_TOOLS_UNAVAILABLE: {} did not report tools",
                mcp.display()
            )
        })?;
    for required in REQUIRED_MCP_TOOLS {
        if !tools
            .iter()
            .any(|tool| tool.get("name").and_then(Value::as_str) == Some(required))
        {
            anyhow::bail!(
                "COLLAB_HEALTH_LOCAL_MCP_TOOL_MISSING: {} did not declare {}",
                mcp.display(),
                required
            );
        }
    }
    Ok(())
}

fn ensure_success(result: &Value) -> anyhow::Result<()> {
    if let Some(error) = result.get("error") {
        anyhow::bail!(
            "COLLAB_HEALTH_LOCAL_MCP_ERROR: {}",
            serde_json::to_string(error)?
        );
    }
    if result.get("result").is_none() {
        anyhow::bail!("COLLAB_HEALTH_LOCAL_MCP_MALFORMED: response has no result");
    }
    Ok(())
}

fn reap_local_mcp(child: &mut Child, deadline: &Instant, mcp: &Path) -> anyhow::Result<()> {
    loop {
        if let Some(status) = child.try_wait()? {
            if !status.success() {
                anyhow::bail!(
                    "COLLAB_HEALTH_LOCAL_MCP_EXIT: {} exited with {}",
                    mcp.display(),
                    status
                );
            }
            return Ok(());
        }
        if Instant::now() >= *deadline {
            let _ = child.kill();
            let _ = child.wait();
            anyhow::bail!(
                "COLLAB_HEALTH_TIMEOUT: sibling MCP did not exit within the check deadline"
            );
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}
