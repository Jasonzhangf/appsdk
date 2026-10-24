//! Read current publisher-owned Desktop control, never identity/route history.
use crate::proto::AppServerCandidate;
use std::ffi::CString;
use std::fs::File;
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Component, Path, PathBuf};

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Declaration {
    schema: String,
    desktop_bridge: AppServerCandidate,
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

fn admit_running_runtime(
    state_root: &Path,
    pipe: &str,
    cwd: &str,
    session: &str,
    thread: &str,
) -> anyhow::Result<bool> {
    use sha2::{Digest, Sha256};
    let root = match state_root.canonicalize() {
        Ok(root) => root,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error.into()),
    };
    let mut directory = File::open(&root)?;
    owned(&directory, true)?;
    for name in ["runtime", "desktop"] {
        directory = match open_child(&directory, std::ffi::OsStr::new(name), true) {
            Ok(directory) => directory,
            Err(error) if error.is::<DeclarationMissing>() => return Ok(false),
            Err(error) => return Err(error),
        };
    }
    let key = format!("{:x}", Sha256::digest(pipe.as_bytes()));
    let socket = root
        .join("runtime/desktop")
        .join(format!("pipe-{}.sock", &key[..24]));
    let mut stream = match std::os::unix::net::UnixStream::connect(socket) {
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
    stream.set_read_timeout(Some(std::time::Duration::from_secs(10)))?;
    stream.set_write_timeout(Some(std::time::Duration::from_secs(10)))?;
    static NEXT_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    let id = format!(
        "admit-{}-{}",
        std::process::id(),
        NEXT_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    );
    let request = serde_json::to_vec(
        &serde_json::json!({"id":id,"op":"admit","session_id":session,"thread_id":thread,"cwd":cwd}),
    )?;
    stream.write_all(&(request.len() as u32).to_le_bytes())?;
    stream.write_all(&request)?;
    let mut header = [0; 4];
    stream.read_exact(&mut header)?;
    let length = u32::from_le_bytes(header) as usize;
    if length > 65536 {
        anyhow::bail!("DESKTOP_BRIDGE_ADMISSION_FAILED: response exceeds 64 KiB");
    }
    let mut bytes = vec![0; length];
    stream.read_exact(&mut bytes)?;
    let response: serde_json::Value = serde_json::from_slice(&bytes)?;
    if response["id"] != id || response["ok"] != true {
        anyhow::bail!("DESKTOP_BRIDGE_ADMISSION_FAILED: {response}");
    }
    Ok(true)
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
    }
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
    file.take(65537).read_to_end(&mut bytes)?;
    if bytes.len() > 65536 {
        anyhow::bail!("DESKTOP_BRIDGE_DECLARATION_INVALID: exceeds 64 KiB");
    }
    let declaration: Declaration = serde_json::from_slice(&bytes)
        .map_err(|e| anyhow::anyhow!("DESKTOP_BRIDGE_DECLARATION_INVALID: {e}"))?;
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
    let endpoint = candidate
        .endpoint
        .strip_prefix("unix://")
        .map(Path::new)
        .ok_or_else(|| {
            anyhow::anyhow!("DESKTOP_BRIDGE_DECLARATION_INVALID: expected Unix endpoint")
        })?;
    if !endpoint.is_absolute()
        || endpoint
            .components()
            .any(|c| matches!(c, Component::ParentDir | Component::CurDir))
        || endpoint
            .parent()
            .and_then(|p| p.canonicalize().ok())
            .as_deref()
            != Some(root.join("runtime/desktop").as_path())
    {
        anyhow::bail!("DESKTOP_BRIDGE_DECLARATION_INVALID: endpoint is outside the bridge runtime");
    }
    Ok(candidate)
}
