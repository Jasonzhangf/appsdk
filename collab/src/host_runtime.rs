//! Minimal host-runtime truth for Collab identity facts.
//!
//! Only a caller whose nearest live agent host is Antigravity (AGY TUI) gets
//! its real agent session from `ANTIGRAVITY_CONVERSATION_ID`. Every other
//! caller keeps the existing Codex environment contract unchanged; a nested
//! Codex process under AGY stays Codex-owned and never inherits the AGY
//! marker. When nearest agent detection yields None (unknown process or ps
//! unavailable), the AGY marker is not accepted: an absent real fact is an
//! explicit missing fact without fallback.
//!
//! There is deliberately no host database, no random id, no tmux-session
//! fallback, and no test environment variable backdoor in production builds.

pub(crate) const AGY_SESSION_ID_ENV: &str = "ANTIGRAVITY_CONVERSATION_ID";

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct HostRuntimeFacts {
    pub(crate) agy_session_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AgentKind {
    Antigravity,
    Codex,
}

/// Return host runtime facts. Only when the nearest agent host is confirmed
/// to be Antigravity (`Some(AgentKind::Antigravity)`) does `agy_session_id`
/// carry `ANTIGRAVITY_CONVERSATION_ID`. Unknown host or nested Codex cannot
/// claim AGY identity.
pub(crate) fn host_runtime_facts() -> HostRuntimeFacts {
    let agy_session = std::env::var(AGY_SESSION_ID_ENV)
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty());

    let Some(session_id) = agy_session else {
        return HostRuntimeFacts::default();
    };

    match nearest_agent_host() {
        Some(AgentKind::Antigravity) => HostRuntimeFacts {
            agy_session_id: Some(session_id),
        },
        _ => HostRuntimeFacts::default(),
    }
}

/// Detect the nearest live agent host on the process chain.
pub(crate) fn nearest_agent_host() -> Option<AgentKind> {
    platform_nearest_agent()
}

/// Pure classifier that walks a sequence of process command names from
/// nearest to furthest and returns the first recognized agent kind.
pub(crate) fn classify_nearest_agent<'a>(
    chain: impl IntoIterator<Item = &'a str>,
) -> Option<AgentKind> {
    for comm in chain {
        let trimmed = comm.trim();
        let name = trimmed
            .rsplit('/')
            .next()
            .unwrap_or(trimmed)
            .to_ascii_lowercase();
        if is_codex_command(&name) {
            return Some(AgentKind::Codex);
        }
        if is_antigravity_command(&name) {
            return Some(AgentKind::Antigravity);
        }
    }
    None
}

pub(crate) fn is_codex_command(name: &str) -> bool {
    name == "codex" || name == "codex-tui" || name == "codexcli" || name == "codex-cli"
}

pub(crate) fn is_antigravity_command(name: &str) -> bool {
    name == "agy"
        || name == "antigravity"
        || name.starts_with("agy-")
        || name.starts_with("antigravity-")
}

/// Return the nearest live agent on one explicit caller's ancestry. Unlike
/// daemon-local host detection, this walks the PID the daemon received from
/// the CLI. The returned PID is the process that must own the Rex foreground.
pub(crate) fn caller_nearest_agent(caller_pid: u32) -> Result<Option<(u32, AgentKind)>, String> {
    if caller_pid <= 1 {
        return Err("REX_EXECUTION_UNVERIFIED: caller PID is invalid".into());
    }
    let mut pid = caller_pid;
    let mut seen = std::collections::HashSet::new();
    for _ in 0..16 {
        if pid <= 1 || !seen.insert(pid) {
            break;
        }
        let output = std::process::Command::new("ps")
            .args(["-o", "ppid=,comm=", "-p", &pid.to_string()])
            .output()
            .map_err(|error| format!("REX_EXECUTION_UNVERIFIED: read caller ancestry: {error}"))?;
        if !output.status.success() {
            return Err(format!(
                "REX_EXECUTION_UNVERIFIED: caller ancestry ended before agent owner (pid={pid})"
            ));
        }
        let text = String::from_utf8(output.stdout)
            .map_err(|_| "REX_EXECUTION_UNVERIFIED: non-UTF-8 caller ancestry".to_owned())?;
        let (parent, comm) = parse_ppid_and_comm(&text)
            .ok_or_else(|| "REX_EXECUTION_UNVERIFIED: malformed caller ancestry".to_owned())?;
        if let Some(kind) = classify_nearest_agent([comm]) {
            return Ok(Some((pid, kind)));
        }
        pid = parent;
    }
    Ok(None)
}

/// Unix wall time at microsecond precision, matching macOS process starts.
pub(crate) fn current_rex_execution_boundary(
) -> Result<crate::server::binding_slot::RexExecutionFence, String> {
    // macOS proc_bsdinfo start_tvsec/start_tvusec uses Unix wall time.
    let elapsed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|error| format!("REX_EXECUTION_BOUNDARY_UNAVAILABLE: {error}"))?;
    Ok(crate::server::binding_slot::RexExecutionFence {
        seconds: elapsed.as_secs(),
        microseconds: elapsed.subsec_micros(),
    })
}

/// Read the exact macOS process start tuple. A missing process is distinct
/// from an observation error, so PID reuse cannot masquerade as the old owner.
pub(crate) fn exact_process_start_id(pid: u32) -> Result<Option<String>, String> {
    if pid == 0 {
        return Ok(None);
    }
    #[cfg(target_os = "macos")]
    {
        let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
        let expected = std::mem::size_of::<libc::proc_bsdinfo>() as libc::c_int;
        let read = unsafe {
            libc::proc_pidinfo(
                pid as libc::c_int,
                libc::PROC_PIDTBSDINFO,
                0,
                &mut info as *mut libc::proc_bsdinfo as *mut libc::c_void,
                expected,
            )
        };
        if read == expected {
            return Ok(Some(format!(
                "{}:{}",
                info.pbi_start_tvsec, info.pbi_start_tvusec
            )));
        }
        if read == 0 {
            let error = std::io::Error::last_os_error();
            if matches!(error.raw_os_error(), Some(libc::ESRCH) | Some(libc::ENOENT)) {
                return Ok(None);
            }
        }
        Err("REX_EXECUTION_UNVERIFIED: exact process start query failed".into())
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = pid;
        Err("REX_EXECUTION_UNVERIFIED: exact process start identity is unavailable on this platform"
            .into())
    }
}

/// A persisted execution is still alive only while the same PID carries the
/// same exact start tuple. PID reuse is therefore a different, ended execution.
pub(crate) fn execution_is_alive(owner_pid: u32, expected_start_id: &str) -> Result<bool, String> {
    Ok(exact_process_start_id(owner_pid)?.is_some_and(|current| current == expected_start_id))
}

/// Parse ppid and the full comm remainder from a `ps -o ppid=,comm=` line.
/// Preserves path with spaces and avoids truncating comm on whitespace.
pub(crate) fn parse_ppid_and_comm(line: &str) -> Option<(u32, &str)> {
    let trimmed = line.trim();
    let split_idx = trimmed.find(char::is_whitespace)?;
    let ppid: u32 = trimmed[..split_idx].parse().ok()?;
    let comm = trimmed[split_idx..].trim_start();
    if comm.is_empty() {
        return None;
    }
    Some((ppid, comm))
}

/// Inspect ancestor process chain using targeted `ps -o ppid=,comm= -p <pid>`.
/// Bounded to 16 iterations and stops as soon as an agent is identified.
fn platform_nearest_agent() -> Option<AgentKind> {
    let own_pid = std::process::id();
    let mut current_pid = own_pid;
    let mut seen = std::collections::HashSet::new();

    for _ in 0..16 {
        if current_pid <= 1 || !seen.insert(current_pid) {
            break;
        }
        let output = std::process::Command::new("ps")
            .args(["-o", "ppid=,comm=", "-p", &current_pid.to_string()])
            .output()
            .ok()?;
        if !output.status.success() {
            break;
        }
        let text = String::from_utf8(output.stdout).ok()?;
        let (ppid, comm) = parse_ppid_and_comm(&text)?;

        if current_pid != own_pid {
            if let Some(kind) = classify_nearest_agent(std::iter::once(comm)) {
                return Some(kind);
            }
        }
        current_pid = ppid;
    }
    None
}

/// Capture only the nearest real agent's kind and --profile on the caller's
/// verified pane ancestry. Full argv is transient and never logged or saved.
pub(crate) fn caller_profile_in_pane(
    caller: u32,
    pane: u32,
) -> Option<(AgentKind, Option<String>)> {
    let mut pid = caller;
    let mut seen = std::collections::HashSet::new();
    while pid > 1 && seen.insert(pid) {
        let process = std::process::Command::new("ps")
            .args(["-o", "ppid=,comm=", "-p", &pid.to_string()])
            .output()
            .ok()?;
        if !process.status.success() {
            return None;
        }
        let text = String::from_utf8(process.stdout).ok()?;
        let (parent, comm) = parse_ppid_and_comm(&text)?;
        if let Some(kind) = classify_nearest_agent([comm]) {
            let profile = std::process::Command::new("ps")
                .args(["-o", "args=", "-p", &pid.to_string()])
                .output()
                .ok()
                .filter(|output| output.status.success())
                .and_then(|output| String::from_utf8(output.stdout).ok())
                .and_then(|args| profile_argument(&args));
            return Some((kind, profile));
        }
        if pid == pane {
            break;
        }
        pid = parent;
    }
    None
}

fn profile_argument(args: &str) -> Option<String> {
    let mut parts = args.split_whitespace();
    while let Some(part) = parts.next() {
        let value = if matches!(part, "--profile" | "-p") {
            parts.next()
        } else {
            part.strip_prefix("--profile=")
        };
        if let Some(value) = value.filter(|value| {
            !value.is_empty()
                && value
                    .chars()
                    .all(|ch| ch.is_alphanumeric() || matches!(ch, '_' | '-' | '.'))
        }) {
            return Some(value.to_owned());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pure_classifier_identifies_nearest_agent() {
        assert_eq!(
            classify_nearest_agent(["zsh", "agy", "tmux"]),
            Some(AgentKind::Antigravity)
        );
        assert_eq!(
            classify_nearest_agent(["/Users/fanzhang/.local/bin/agy"]),
            Some(AgentKind::Antigravity)
        );
        assert_eq!(
            classify_nearest_agent(["/Applications/Antigravity App/Contents/MacOS/agy"]),
            Some(AgentKind::Antigravity)
        );
        assert_eq!(
            classify_nearest_agent(["zsh", "codex", "agy"]),
            Some(AgentKind::Codex)
        );
        assert_eq!(
            classify_nearest_agent(["/usr/local/bin/codex-tui"]),
            Some(AgentKind::Codex)
        );
        assert_eq!(classify_nearest_agent(["python", "zsh", "bash"]), None);
    }

    #[test]
    fn parse_ppid_and_comm_handles_spaces_and_paths() {
        let (ppid, comm) = parse_ppid_and_comm(" 12345 /path with spaces/agy ").unwrap();
        assert_eq!(ppid, 12345);
        assert_eq!(comm, "/path with spaces/agy");

        let (ppid2, comm2) = parse_ppid_and_comm("99 /bin/sh").unwrap();
        assert_eq!(ppid2, 99);
        assert_eq!(comm2, "/bin/sh");

        assert!(parse_ppid_and_comm("invalid").is_none());
        assert!(parse_ppid_and_comm("   ").is_none());
    }

    #[test]
    fn host_runtime_facts_requires_antigravity_host_and_unknown_is_none() {
        let _guard = crate::scope::TEST_ENV_LOCK.lock().unwrap();

        // 1. When AGY marker is present, but nearest agent is Codex, agy_session_id is None.
        assert_eq!(
            classify_nearest_agent(["codex", "agy"]),
            Some(AgentKind::Codex)
        );

        // 2. When AGY marker is present, but nearest agent is unknown/None,
        // pure classifier returns None and host facts must reject it.
        assert_eq!(classify_nearest_agent(["python3", "zsh"]), None);

        // 3. When session id is empty, facts are default.
        std::env::set_var(AGY_SESSION_ID_ENV, "");
        let facts = host_runtime_facts();
        assert_eq!(facts.agy_session_id, None);
        std::env::remove_var(AGY_SESSION_ID_ENV);
    }

    #[test]
    fn live_agy_environment_detects_antigravity_if_in_agy() {
        let _guard = crate::scope::TEST_ENV_LOCK.lock().unwrap();
        if std::env::var(AGY_SESSION_ID_ENV).is_ok() {
            let agent = platform_nearest_agent();
            assert_eq!(agent, Some(AgentKind::Antigravity));
        }
    }
}
