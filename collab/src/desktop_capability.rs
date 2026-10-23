//! Read current publisher-owned Desktop control, never identity/route history.
use crate::proto::AppServerCandidate;
use std::ffi::CString;
use std::fs::File;
use std::io::Read;
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
            anyhow::bail!(
                "DESKTOP_BRIDGE_CAPABILITY_REQUIRED: no current enrolled bridge declaration"
            );
        }
        anyhow::bail!("DESKTOP_BRIDGE_DECLARATION_UNAVAILABLE: {error}");
    }
    let file = unsafe { File::from_raw_fd(descriptor) };
    owned(&file, directory)?;
    Ok(file)
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
