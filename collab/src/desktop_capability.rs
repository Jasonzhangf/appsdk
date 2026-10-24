//! Read current publisher-owned Desktop control, never identity/route history.
use crate::proto::AppServerCandidate;
use std::ffi::CString;
use std::fs::File;
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::os::unix::net::UnixStream;
use std::path::{Component, Path, PathBuf};
use std::time::Duration;

const FRAME_LIMIT: usize = 65536;

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Declaration {
    schema: String,
    desktop_bridge: AppServerCandidate,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct RuntimeDescriptor {
    schema: String,
    runtime_key: String,
    endpoint: String,
    namespace: String,
    publisher_id: String,
}

/// Publication absence is not evidence that a registered native capability
/// disappeared. Keep this typed so lifecycle callers never classify logs.
#[derive(Debug)]
pub struct DeclarationMissing;

impl std::fmt::Display for DeclarationMissing {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("DESKTOP_BRIDGE_CAPABILITY_REQUIRED: no current enrolled bridge declaration")
    }
}

impl std::error::Error for DeclarationMissing {}

fn owned(file: &File, directory: bool) -> anyhow::Result<()> {
    let metadata = file.metadata()?;
    if metadata.uid() != unsafe { libc::geteuid() }
        || metadata.mode() & (if directory { 0o022 } else { 0o077 }) != 0
        || (directory && !metadata.is_dir())
        || (!directory && !metadata.is_file())
    {
        anyhow::bail!("DESKTOP_BRIDGE_DECLARATION_UNSAFE: expected user-owned private control");
    }
    Ok(())
}

fn open_child(parent: &File, name: &std::ffi::OsStr, directory: bool) -> anyhow::Result<File> {
    let name = CString::new(name.as_bytes())?;
    let flags = libc::O_RDONLY
        | libc::O_CLOEXEC
        | libc::O_NOFOLLOW
        | if directory {
            libc::O_DIRECTORY
        } else {
            libc::O_NONBLOCK
        };
    let descriptor = unsafe { libc::openat(parent.as_raw_fd(), name.as_ptr(), flags) };
    if descriptor < 0 {
        let error = std::io::Error::last_os_error();
        if error.kind() == std::io::ErrorKind::NotFound {
            return Err(DeclarationMissing.into());
        }
        anyhow::bail!("DESKTOP_BRIDGE_DECLARATION_UNAVAILABLE: {error}");
    }
    let file = unsafe { File::from_raw_fd(descriptor) };
    owned(&file, directory)?;
    Ok(file)
}

fn open_runtime_desktop(state_root: &Path) -> anyhow::Result<Option<(PathBuf, File)>> {
    let root = match state_root.canonicalize() {
        Ok(root) => root,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let mut directory = File::open(&root)?;
    owned(&directory, true)?;
    for name in ["runtime", "desktop"] {
        directory = match open_child(&directory, std::ffi::OsStr::new(name), true) {
            Ok(directory) => directory,
            Err(error) if error.is::<DeclarationMissing>() => return Ok(None),
            Err(error) => return Err(error),
        };
    }
    Ok(Some((root, directory)))
}

fn unix_path(endpoint: &str) -> anyhow::Result<&Path> {
    let path = Path::new(endpoint.strip_prefix("unix://").ok_or_else(|| {
        anyhow::anyhow!("DESKTOP_BRIDGE_DECLARATION_INVALID: expected Unix endpoint")
    })?);
    if !path.is_absolute()
        || path
            .components()
            .any(|component| matches!(component, Component::ParentDir | Component::CurDir))
    {
        anyhow::bail!(
            "DESKTOP_BRIDGE_DECLARATION_INVALID: endpoint is not a private absolute Unix path"
        );
    }
    Ok(path)
}

fn endpoint_in_runtime(root: &Path, endpoint: &str) -> anyhow::Result<PathBuf> {
    let path = unix_path(endpoint)?;
    if path
        .parent()
        .and_then(|parent| parent.canonicalize().ok())
        .as_deref()
        != Some(root.join("runtime/desktop").as_path())
    {
        anyhow::bail!("DESKTOP_BRIDGE_DECLARATION_INVALID: endpoint is outside the bridge runtime");
    }
    Ok(path.to_path_buf())
}

fn connect(socket: &Path, timeout: Duration) -> std::io::Result<UnixStream> {
    let stream = UnixStream::connect(socket)?;
    stream.set_read_timeout(Some(timeout))?;
    stream.set_write_timeout(Some(timeout))?;
    Ok(stream)
}

fn exchange(
    stream: &mut UnixStream,
    request: &serde_json::Value,
) -> anyhow::Result<serde_json::Value> {
    let bytes = serde_json::to_vec(request)?;
    if bytes.len() > FRAME_LIMIT {
        anyhow::bail!("DESKTOP_BRIDGE_RESULT_UNKNOWN: request exceeds 64 KiB");
    }
    stream.write_all(&(bytes.len() as u32).to_le_bytes())?;
    stream.write_all(&bytes)?;
    let mut header = [0; 4];
    stream.read_exact(&mut header)?;
    let length = u32::from_le_bytes(header) as usize;
    if length > FRAME_LIMIT {
        anyhow::bail!("DESKTOP_BRIDGE_ADMISSION_FAILED: response exceeds 64 KiB");
    }
    let mut body = vec![0; length];
    stream.read_exact(&mut body)?;
    Ok(serde_json::from_slice(&body)?)
}

fn next_id(prefix: &str) -> String {
    static NEXT_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    format!(
        "{prefix}-{}-{}",
        std::process::id(),
        NEXT_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    )
}

fn ping_socket(socket: &Path, expected_runtime_key: Option<&str>) -> anyhow::Result<()> {
    let mut stream = connect(socket, Duration::from_secs(1))?;
    let id = next_id("ping");
    let response = exchange(&mut stream, &serde_json::json!({"id":id,"op":"ping"}))?;
    if response["id"] != id || response["ok"] != true {
        anyhow::bail!("DESKTOP_BRIDGE_RUNTIME_UNAVAILABLE: ping refused: {response}");
    }
    if let Some(expected) = expected_runtime_key {
        if response["runtime_key"] != expected {
            anyhow::bail!("DESKTOP_BRIDGE_RUNTIME_INVALID: publisher runtime key differs");
        }
    }
    Ok(())
}

/// Confirm that a declaration still names a live local publisher.
pub fn ping_candidate(candidate: &AppServerCandidate) -> anyhow::Result<()> {
    ping_socket(unix_path(&candidate.endpoint)?, None)
}

fn admit_stream(
    stream: &mut UnixStream,
    session: &str,
    thread: &str,
    cwd: &str,
) -> anyhow::Result<()> {
    let id = next_id("admit");
    let response = exchange(
        stream,
        &serde_json::json!({"id":id,"op":"admit","session_id":session,"thread_id":thread,"cwd":cwd}),
    )?;
    if response["id"] != id || response["ok"] != true {
        anyhow::bail!("DESKTOP_BRIDGE_ADMISSION_FAILED: {response}");
    }
    Ok(())
}

fn admit_socket(socket: &Path, session: &str, thread: &str, cwd: &str) -> anyhow::Result<()> {
    let mut stream = connect(socket, Duration::from_secs(10))?;
    admit_stream(&mut stream, session, thread, cwd)
}

fn admit_running_runtime(
    state_root: &Path,
    pipe: &str,
    cwd: &str,
    session: &str,
    thread: &str,
) -> anyhow::Result<bool> {
    use sha2::{Digest, Sha256};
    let Some((root, _desktop)) = open_runtime_desktop(state_root)? else {
        return Ok(false);
    };
    let key = format!("{:x}", Sha256::digest(pipe.as_bytes()));
    let socket = root
        .join("runtime/desktop")
        .join(format!("pipe-{}.sock", &key[..24]));
    let mut stream = match UnixStream::connect(socket) {
        Ok(stream) => stream,
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused
            ) =>
        {
            return Ok(false)
        }
        Err(error) => return Err(error.into()),
    };
    stream.set_read_timeout(Some(Duration::from_secs(10)))?;
    stream.set_write_timeout(Some(Duration::from_secs(10)))?;
    admit_stream(&mut stream, session, thread, cwd)?;
    Ok(true)
}

fn read_current(
    state_root: &Path,
    cwd_text: &str,
    session: &str,
    thread: &str,
) -> anyhow::Result<AppServerCandidate> {
    let root = state_root.canonicalize()?;
    let mut directory = File::open(&root)?;
    owned(&directory, true)?;
    let mut relative = PathBuf::from("runtime/desktop/capabilities/v1");
    for (label, value) in [("cwd", cwd_text), ("session", session), ("thread", thread)] {
        relative.push(label);
        let encoded: String = value
            .as_bytes()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        for chunk in encoded.as_bytes().chunks(120) {
            relative.push(std::str::from_utf8(chunk)?);
        }
    }
    for component in relative.components() {
        directory = open_child(&directory, component.as_os_str(), true)?;
    }
    let file = open_child(&directory, std::ffi::OsStr::new("current.json"), false)?;
    let mut bytes = Vec::new();
    file.take(FRAME_LIMIT as u64 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > FRAME_LIMIT {
        anyhow::bail!("DESKTOP_BRIDGE_DECLARATION_INVALID: exceeds 64 KiB");
    }
    let declaration: Declaration = serde_json::from_slice(&bytes)
        .map_err(|error| anyhow::anyhow!("DESKTOP_BRIDGE_DECLARATION_INVALID: {error}"))?;
    let candidate = declaration.desktop_bridge;
    if declaration.schema != "collab-desktop-capability/v1"
        || candidate.session_id != session
        || candidate.thread_id != thread
        || candidate.cwd != cwd_text
        || candidate.namespace != "codex_app"
    {
        anyhow::bail!(
            "DESKTOP_BRIDGE_SCOPE_MISMATCH: current declaration differs from caller scope"
        );
    }
    endpoint_in_runtime(&root, &candidate.endpoint)?;
    Ok(candidate)
}

fn runtime_descriptors(state_root: &Path) -> anyhow::Result<Vec<(PathBuf, RuntimeDescriptor)>> {
    let Some((root, desktop)) = open_runtime_desktop(state_root)? else {
        return Ok(Vec::new());
    };
    let services = match open_child(&desktop, std::ffi::OsStr::new("services"), true) {
        Ok(directory) => directory,
        Err(error) if error.is::<DeclarationMissing>() => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    let services_path = root.join("runtime/desktop/services");
    let entries = match std::fs::read_dir(&services_path) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.into()),
    };
    let mut descriptors = Vec::new();
    for entry in entries {
        let entry = entry?;
        let name = entry.file_name();
        if Path::new(&name)
            .extension()
            .and_then(|value| value.to_str())
            != Some("json")
        {
            continue;
        }
        let file = match open_child(&services, &name, false) {
            Ok(file) => file,
            Err(error) if error.is::<DeclarationMissing>() => continue,
            Err(error) => return Err(error),
        };
        let mut bytes = Vec::new();
        file.take(FRAME_LIMIT as u64 + 1).read_to_end(&mut bytes)?;
        if bytes.len() > FRAME_LIMIT {
            anyhow::bail!(
                "DESKTOP_BRIDGE_RUNTIME_DESCRIPTOR_INVALID: {} exceeds 64 KiB",
                name.to_string_lossy()
            );
        }
        let descriptor: RuntimeDescriptor = serde_json::from_slice(&bytes).map_err(|error| {
            anyhow::anyhow!(
                "DESKTOP_BRIDGE_RUNTIME_DESCRIPTOR_INVALID: {}: {error}",
                name.to_string_lossy()
            )
        })?;
        if descriptor.schema != "collab-desktop-runtime/v1"
            || descriptor.runtime_key.is_empty()
            || descriptor.publisher_id.is_empty()
            || descriptor.namespace != "codex_app"
        {
            anyhow::bail!(
                "DESKTOP_BRIDGE_RUNTIME_DESCRIPTOR_INVALID: {} has invalid schema or scope",
                name.to_string_lossy()
            );
        }
        let socket = endpoint_in_runtime(&root, &descriptor.endpoint)?;
        descriptors.push((socket, descriptor));
    }
    Ok(descriptors)
}

pub fn discover(
    state_root: &Path,
    cwd: &Path,
    session: Option<&str>,
    thread: Option<&str>,
) -> anyhow::Result<AppServerCandidate> {
    let session = session.filter(|s| !s.is_empty()).ok_or_else(|| {
        anyhow::anyhow!("DESKTOP_BRIDGE_SESSION_REQUIRED: current agent sessionID is missing")
    })?;
    let thread = thread.filter(|s| !s.is_empty()).ok_or_else(|| {
        anyhow::anyhow!("DESKTOP_BRIDGE_THREAD_REQUIRED: current native thread ID is missing")
    })?;
    let cwd = cwd.canonicalize()?;
    let cwd_text = cwd
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("DESKTOP_BRIDGE_SCOPE_INVALID: cwd is not UTF-8"))?;
    if let Ok(pipe) = std::env::var("CODEX_APP_TOOLS_PIPE_PATH") {
        if !admit_running_runtime(state_root, &pipe, cwd_text, session, thread)? {
            // Native host access stays in its signed Node bridge. The CLI only
            // asks that runtime service to admit this exact current scope.
            let node = std::env::var_os("CODEX_MCP_NODE_PATH").ok_or_else(|| {
                anyhow::anyhow!("DESKTOP_BRIDGE_HOST_REQUIRED: CODEX_MCP_NODE_PATH is missing")
            })?;
            let script = match std::env::var_os("COLLAB_DESKTOP_BRIDGE_SCRIPT") {
                Some(script) => PathBuf::from(script),
                None => std::env::current_exe()?
                    .parent()
                    .ok_or_else(|| anyhow::anyhow!("DESKTOP_BRIDGE_INSTALLATION_INVALID"))?
                    .join("collab-desktop-mcp.mjs"),
            };
            let output = std::process::Command::new(node)
                .arg(script)
                .arg("--admit")
                .arg(
                    serde_json::json!({
                        "session_id": session, "thread_id": thread, "cwd": cwd_text
                    })
                    .to_string(),
                )
                .env("COLLAB_STATE_DIR", state_root)
                .output()
                .map_err(|error| anyhow::anyhow!("DESKTOP_BRIDGE_UNAVAILABLE: {error}"))?;
            if !output.status.success() {
                anyhow::bail!(
                    "DESKTOP_BRIDGE_ADMISSION_FAILED: {}",
                    String::from_utf8_lossy(&output.stderr).trim()
                );
            }
        }
        return read_current(state_root, cwd_text, session, thread);
    }

    // Daemons do not inherit the host pipe or Node path. Use the current live
    // declaration when possible, otherwise ask a publisher-owned runtime
    // service to re-verify this exact scope. Never infer a service from logs,
    // journals, snapshots, or socket globs.
    let current = read_current(state_root, cwd_text, session, thread);
    match &current {
        Ok(candidate) => {
            if ping_candidate(candidate).is_ok() {
                return Ok(candidate.clone());
            }
        }
        Err(error) if error.is::<DeclarationMissing>() => {}
        Err(_) => return current,
    }
    let mut live = Vec::new();
    let mut problems = Vec::new();
    for (socket, descriptor) in runtime_descriptors(state_root)? {
        if let Err(error) = ping_socket(&socket, Some(&descriptor.runtime_key)) {
            problems.push(format!(
                "stale or unreachable {}: {error}",
                socket.display()
            ));
            continue;
        }
        if !live.contains(&socket) {
            live.push(socket);
        }
    }
    // Admission publishes scoped control. Refuse ambiguous ownership before
    // mutating it; otherwise the next call silently uses the last publisher.
    if live.len() > 1 {
        anyhow::bail!(
            "DESKTOP_BRIDGE_RUNTIME_AMBIGUOUS: {} live publishers with no current live scope; refusing to choose",
            live.len()
        );
    }
    if let Some(socket) = live.first() {
        admit_socket(socket, session, thread, cwd_text)?;
        return read_current(state_root, cwd_text, session, thread);
    }
    if !problems.is_empty() {
        anyhow::bail!(
            "DESKTOP_BRIDGE_RUNTIME_UNAVAILABLE: {}",
            problems.join("; ")
        );
    }
    if let Ok(candidate) = current {
        return Ok(candidate);
    }
    Err(DeclarationMissing.into())
}
