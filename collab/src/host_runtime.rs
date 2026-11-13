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

/// Boundary in the native clock used by the platform's process-start facts.
pub(crate) fn current_rex_execution_boundary(
) -> Result<crate::server::binding_slot::RexExecutionFence, String> {
    #[cfg(target_os = "macos")]
    {
        // macOS proc_bsdinfo start_tvsec/start_tvusec uses Unix wall time.
        let elapsed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|error| format!("REX_EXECUTION_BOUNDARY_UNAVAILABLE: {error}"))?;
        return crate::server::binding_slot::RexExecutionFence::unix_wall(
            elapsed.as_secs(),
            elapsed.subsec_micros(),
        )
        .ok_or_else(|| "REX_EXECUTION_BOUNDARY_UNAVAILABLE: invalid wall clock".into());
    }
    #[cfg(target_os = "linux")]
    {
        // Linux /proc stat field 22 exports task->start_boottime, not
        // task->start_time. CLOCK_BOOTTIME shares its suspend-inclusive clock.
        let boot_before = read_linux_boot_id()
            .map_err(|error| format!("REX_EXECUTION_BOUNDARY_UNAVAILABLE: {error}"))?;
        let ticks_per_second = linux_clock_ticks_per_second()
            .map_err(|error| format!("REX_EXECUTION_BOUNDARY_UNAVAILABLE: {error}"))?;
        let (seconds, nanoseconds) = linux_boottime()
            .map_err(|error| format!("REX_EXECUTION_BOUNDARY_UNAVAILABLE: {error}"))?;
        let boot_after = read_linux_boot_id()
            .map_err(|error| format!("REX_EXECUTION_BOUNDARY_UNAVAILABLE: {error}"))?;
        if boot_before != boot_after {
            return Err(
                "REX_EXECUTION_BOUNDARY_UNAVAILABLE: Linux boot identity changed during observation"
                    .into(),
            );
        }
        let ticks = (seconds as u128)
            .checked_mul(ticks_per_second as u128)
            .and_then(|whole| {
                let fractional = (nanoseconds as u128) * (ticks_per_second as u128) / 1_000_000_000;
                whole.checked_add(fractional)
            })
            .and_then(|ticks| u64::try_from(ticks).ok())
            .ok_or_else(|| {
                "REX_EXECUTION_BOUNDARY_UNAVAILABLE: Linux boot clock exceeds u64 ticks".to_owned()
            })?;
        return crate::server::binding_slot::RexExecutionFence::linux_boot(
            boot_before,
            ticks_per_second,
            ticks,
        )
        .ok_or_else(|| "REX_EXECUTION_BOUNDARY_UNAVAILABLE: invalid Linux clock facts".into());
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        Err("REX_EXECUTION_BOUNDARY_UNAVAILABLE: unsupported platform".into())
    }
}

/// Read the exact native process start identity. A missing process is distinct
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
    #[cfg(target_os = "linux")]
    {
        let boot_before =
            read_linux_boot_id().map_err(|error| format!("REX_EXECUTION_UNVERIFIED: {error}"))?;
        let ticks_per_second = linux_clock_ticks_per_second()
            .map_err(|error| format!("REX_EXECUTION_UNVERIFIED: {error}"))?;
        let start_ticks = read_linux_proc_start_ticks(pid)
            .map_err(|error| format!("REX_EXECUTION_UNVERIFIED: {error}"))?;
        let boot_after =
            read_linux_boot_id().map_err(|error| format!("REX_EXECUTION_UNVERIFIED: {error}"))?;
        if boot_before != boot_after {
            return Err(
                "REX_EXECUTION_UNVERIFIED: Linux boot identity changed during observation".into(),
            );
        }
        Ok(start_ticks.map(|ticks| format!("linux:{boot_before}:{ticks_per_second}:{ticks}")))
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = pid;
        Err("REX_EXECUTION_UNVERIFIED: exact process start identity is unavailable on this platform"
            .into())
    }
}

/// A persisted execution is still alive only while the same PID carries the
/// same exact start tuple. PID reuse is therefore a different, ended execution.
pub(crate) fn execution_is_alive(owner_pid: u32, expected_start_id: &str) -> Result<bool, String> {
    let Some(current_start_id) = exact_process_start_id(owner_pid)? else {
        return Ok(false);
    };
    if current_start_id == expected_start_id {
        return Ok(true);
    }
    let expected = crate::server::binding_slot::RexExecutionFence::from_start_id(expected_start_id)
        .ok_or_else(|| {
            "REX_EXECUTION_UNVERIFIED: persisted process start identity is malformed".to_owned()
        })?;
    let current = crate::server::binding_slot::RexExecutionFence::from_start_id(&current_start_id)
        .ok_or_else(|| {
            "REX_EXECUTION_UNVERIFIED: current process start identity is malformed".to_owned()
        })?;
    if expected.compare(&current).is_none() {
        return Err(
            "REX_EXECUTION_UNVERIFIED: process start identities use incomparable clocks".into(),
        );
    }
    Ok(false)
}

#[cfg(target_os = "linux")]
fn read_linux_boot_id() -> Result<String, String> {
    let boot_id = std::fs::read_to_string("/proc/sys/kernel/random/boot_id")
        .map_err(|error| format!("read Linux boot ID: {error}"))?
        .trim()
        .to_owned();
    if boot_id.is_empty()
        || boot_id.chars().any(|character| {
            character.is_whitespace() || character.is_control() || character == ':'
        })
    {
        return Err("malformed Linux boot ID".into());
    }
    Ok(boot_id)
}

#[cfg(target_os = "linux")]
fn linux_clock_ticks_per_second() -> Result<u64, String> {
    let ticks = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
    u64::try_from(ticks)
        .ok()
        .filter(|ticks| *ticks > 0)
        .ok_or_else(|| "invalid Linux CLK_TCK".into())
}

#[cfg(target_os = "linux")]
fn linux_boottime() -> Result<(u64, u64), String> {
    let mut value: libc::timespec = unsafe { std::mem::zeroed() };
    if unsafe { libc::clock_gettime(libc::CLOCK_BOOTTIME, &mut value) } != 0 {
        return Err(format!(
            "read CLOCK_BOOTTIME: {}",
            std::io::Error::last_os_error()
        ));
    }
    let seconds = u64::try_from(value.tv_sec).map_err(|_| "negative CLOCK_BOOTTIME".to_owned())?;
    let nanoseconds = u64::try_from(value.tv_nsec)
        .ok()
        .filter(|nanoseconds| *nanoseconds < 1_000_000_000)
        .ok_or_else(|| "invalid CLOCK_BOOTTIME".to_owned())?;
    Ok((seconds, nanoseconds))
}

#[cfg(target_os = "linux")]
fn read_linux_proc_start_ticks(pid: u32) -> Result<Option<u64>, String> {
    let path = format!("/proc/{pid}/stat");
    match std::fs::read_to_string(&path) {
        Ok(stat) => {
            parse_linux_proc_stat(&stat).ok_or_else(|| "malformed Linux /proc stat".to_owned())
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let pid =
                libc::pid_t::try_from(pid).map_err(|_| "Linux PID is out of range".to_owned())?;
            if unsafe { libc::kill(pid, 0) } == 0 {
                return Err("Linux /proc stat is absent for a live PID".into());
            }
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() == Some(libc::ESRCH) {
                Ok(None)
            } else {
                Err(format!("inspect absent Linux PID: {error}"))
            }
        }
        Err(error) => Err(format!("read Linux /proc stat: {error}")),
    }
}

#[cfg(any(target_os = "linux", test))]
fn parse_linux_proc_stat(stat: &str) -> Option<Option<u64>> {
    let open = stat.find('(')?;
    let close = stat.rfind(')')?;
    if open >= close {
        return None;
    }
    stat[..open].trim().parse::<u32>().ok()?;
    let mut fields = stat[close + 1..].split_whitespace();
    let state = fields.next()?;
    if state.chars().count() != 1 {
        return None;
    }
    let start_ticks = fields.nth(18)?.parse::<u64>().ok()?;
    Some((!matches!(state, "Z" | "X")).then_some(start_ticks))
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
    fn linux_proc_stat_parser_uses_field_twenty_two_and_process_state() {
        let mut fields = vec!["S".to_owned()];
        fields.extend((1..=18).map(|value| value.to_string()));
        fields.push("4242".to_owned());
        let fields = fields.join(" ");
        let stat = format!("321 (codex worker) (nested) {fields}");
        assert_eq!(parse_linux_proc_stat(&stat), Some(Some(4242)));

        let zombie = stat.replacen(" S ", " Z ", 1);
        assert_eq!(parse_linux_proc_stat(&zombie), Some(None));
        let dead = stat.replacen(" S ", " X ", 1);
        assert_eq!(parse_linux_proc_stat(&dead), Some(None));
        assert_eq!(parse_linux_proc_stat("malformed"), None);
        assert_eq!(
            parse_linux_proc_stat("321 (codex) S 1 2 3"),
            None,
            "a truncated process record must not become absence"
        );
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

    struct OwnedChild(std::process::Child);

    impl Drop for OwnedChild {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_process_start_facts_are_stable_and_distinguish_exit_from_query_errors() {
        let mut child = OwnedChild(
            std::process::Command::new("sleep")
                .arg("60")
                .spawn()
                .expect("spawn a Linux sleep fixture"),
        );
        let pid = child.0.id();
        let first = exact_process_start_id(pid)
            .expect("Linux exact start identity must be available")
            .expect("a live Linux process must have a start identity");
        assert!(
            first.starts_with("linux:"),
            "Linux start identity must declare its clock domain: {first}"
        );
        let second = exact_process_start_id(pid)
            .expect("repeated Linux exact start identity must be available")
            .expect("repeated Linux exact start identity must remain present");
        assert_eq!(
            first, second,
            "one live execution must keep one start token"
        );
        let boundary = current_rex_execution_boundary()
            .expect("Linux publication boundary must use native boot-clock facts");
        assert!(
            matches!(
                boundary,
                crate::server::binding_slot::RexExecutionFence::LinuxBoot {
                    ref boot_id,
                    ticks_per_second,
                    ..
                } if !boot_id.is_empty() && ticks_per_second > 0
            ),
            "Linux publication must carry boot ID and CLK_TCK"
        );

        let changed = match crate::server::binding_slot::RexExecutionFence::from_start_id(&first)
            .expect("native Linux start ID must parse")
        {
            crate::server::binding_slot::RexExecutionFence::LinuxBoot {
                boot_id,
                ticks_per_second,
                ticks,
            } => format!("linux:{boot_id}:{ticks_per_second}:{}", ticks + 1),
            _ => panic!("Linux start ID must use the Linux boot-clock form"),
        };
        assert!(
            !execution_is_alive(pid, &changed).expect("comparable replacement must be inspectable"),
            "a changed native start token on the same PID is a different execution"
        );
        assert!(
            execution_is_alive(pid, "12:7").is_err(),
            "a remembered Unix start ID is incomparable with a Linux start ID"
        );
        unsafe {
            assert_eq!(libc::kill(pid as libc::pid_t, libc::SIGSTOP), 0);
        }
        assert_eq!(
            exact_process_start_id(pid)
                .expect("stopped Linux exact start identity must be available")
                .as_deref(),
            Some(first.as_str()),
            "a suspended Linux execution is still the same live execution"
        );

        unsafe {
            assert_eq!(libc::kill(pid as libc::pid_t, libc::SIGCONT), 0);
        }
        child.0.kill().expect("terminate the Linux sleep fixture");
        let zombie_deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while exact_process_start_id(pid)
            .expect("zombie inspection must remain available")
            .is_some()
        {
            assert!(
                std::time::Instant::now() < zombie_deadline,
                "the terminated Linux fixture did not reach a terminal process state"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        child.0.wait().expect("reap the Linux sleep fixture");
        assert_eq!(
            exact_process_start_id(pid).expect("confirmed absence must be available"),
            None,
            "a reaped Linux process must be an ended execution, not a query error"
        );
    }
}
