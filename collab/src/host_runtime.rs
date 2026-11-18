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

#[cfg(target_os = "linux")]
use std::os::fd::{FromRawFd, OwnedFd};

pub(crate) struct PublicationGuard {
    file: std::fs::File,
}

impl PublicationGuard {
    pub(crate) fn acquire(
        path: &std::path::Path,
        exclusive: bool,
        create: bool,
    ) -> anyhow::Result<Self> {
        let mut options = std::fs::OpenOptions::new();
        options.read(true).write(true);
        if create {
            options.create(true);
        }
        let file = options.open(path).map_err(|error| {
            anyhow::Error::new(error).context(format!(
                "DAEMON_OWNER_PUBLICATION_REFUSED: cannot open {}",
                path.display()
            ))
        })?;
        use std::os::fd::AsRawFd;
        let mode = if exclusive {
            libc::LOCK_EX
        } else {
            libc::LOCK_SH
        };
        if unsafe { libc::flock(file.as_raw_fd(), mode | libc::LOCK_NB) } != 0 {
            let error = std::io::Error::last_os_error();
            let detail = if error.raw_os_error() == Some(libc::EAGAIN) {
                "is busy"
            } else {
                "could not be acquired"
            };
            return Err(anyhow::Error::new(error).context(format!(
                "DAEMON_OWNER_PUBLICATION_REFUSED: {} lock {} {detail}",
                if exclusive { "exclusive" } else { "shared" },
                path.display()
            )));
        }
        Ok(Self { file })
    }

    #[cfg(test)]
    fn raw_fd(&self) -> std::os::fd::RawFd {
        use std::os::fd::AsRawFd;
        self.file.as_raw_fd()
    }
}

/// A retained process instance selected for daemon replacement. Holding this
/// target does not authorize signalling; it prevents a later PID/owner reuse
/// from becoming the signal target.
#[derive(Debug)]
pub(crate) enum RetainedProcessInstance {
    #[cfg(target_os = "linux")]
    Linux(OwnedFd),
    #[cfg(target_os = "macos")]
    MacOS(MacAuditToken),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ProcessSignalOutcome {
    Delivered,
    InstanceGone,
    Unavailable(String),
    Failed(String),
}

fn process_signal_error(error: libc::c_int, operation: &str) -> ProcessSignalOutcome {
    match error {
        libc::ESRCH => ProcessSignalOutcome::InstanceGone,
        libc::ENOSYS => ProcessSignalOutcome::Unavailable(format!(
            "{operation} is unavailable: {}",
            std::io::Error::from_raw_os_error(error)
        )),
        libc::EPERM | libc::EACCES => ProcessSignalOutcome::Failed(format!(
            "{operation} permission denied: {}",
            std::io::Error::from_raw_os_error(error)
        )),
        _ => ProcessSignalOutcome::Failed(format!(
            "{operation} failed: {}",
            std::io::Error::from_raw_os_error(error)
        )),
    }
}

#[cfg(target_os = "macos")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(C)]
pub(crate) struct MacAuditToken {
    val: [u32; 8],
}

#[cfg(target_os = "macos")]
impl MacAuditToken {
    fn parse(value: &str) -> Option<Self> {
        let words = value
            .split_whitespace()
            .map(|word| word.parse::<u32>().ok())
            .collect::<Option<Vec<_>>>()?;
        <[u32; 8]>::try_from(words.as_slice())
            .ok()
            .map(|val| Self { val })
    }

    fn serialize(&self) -> String {
        self.val
            .iter()
            .map(u32::to_string)
            .collect::<Vec<_>>()
            .join(" ")
    }
}

/// The daemon publishes the complete audit token. The client keeps the parsed
/// token as the immutable proof of one process instance.
#[cfg(target_os = "macos")]
pub(crate) fn own_audit_token_words() -> Result<MacAuditToken, String> {
    let mut token = MacAuditToken { val: [0; 8] };
    let mut count = 8;
    let result = unsafe {
        libc::task_info(
            libc::mach_task_self(),
            TASK_AUDIT_TOKEN,
            token.val.as_mut_ptr().cast(),
            &mut count,
        )
    };
    if result != 0 || count != 8 {
        return Err(format!(
            "self audit token unavailable: task_info returned {result}, word count {count}"
        ));
    }
    Ok(token)
}

/// Linux pidfds continue to identify the selected process after exit and are
/// not retargeted by numeric PID reuse.
#[cfg(target_os = "linux")]
pub(crate) fn retained_process_instance(pid: u32) -> Result<RetainedProcessInstance, String> {
    let pid = libc::pid_t::try_from(pid).map_err(|_| "PID is out of range".to_owned())?;
    let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0) };
    if fd < 0 {
        let error = std::io::Error::last_os_error();
        return Err(format!("pidfd_open failed: {error}"));
    }
    // SAFETY: a successful pidfd_open syscall returns a newly owned descriptor.
    Ok(RetainedProcessInstance::Linux(unsafe {
        OwnedFd::from_raw_fd(fd as libc::c_int)
    }))
}

/// macOS process-instance proof is immutable and is published by the daemon
/// that owns the writer lock. A token is not acquired from another process.
#[cfg(target_os = "macos")]
pub(crate) fn retained_process_instance(
    token: &str,
    expected_pid: u32,
) -> Result<RetainedProcessInstance, String> {
    let token =
        MacAuditToken::parse(token).ok_or_else(|| "malformed daemon audit token".to_owned())?;
    let token_pid = audit_token_pid(&token).map_err(|error| error.to_owned())?;
    let expected_pid = libc::pid_t::try_from(expected_pid)
        .map_err(|_| "recorded daemon PID is out of range".to_owned())?;
    if token_pid != expected_pid {
        return Err(format!(
            "daemon audit token names PID {token_pid}, not recorded daemon PID {expected_pid}"
        ));
    }
    Ok(RetainedProcessInstance::MacOS(token))
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub(crate) fn retained_process_instance(_pid: u32) -> Result<RetainedProcessInstance, String> {
    Err("retained process instances are unsupported on this platform".into())
}

pub(crate) fn retained_process_instance_is_alive(
    target: &RetainedProcessInstance,
) -> Result<bool, String> {
    match target {
        #[cfg(target_os = "linux")]
        RetainedProcessInstance::Linux(fd) => {
            use std::os::fd::AsRawFd;
            let mut pollfd = libc::pollfd {
                fd: fd.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            };
            let result = unsafe { libc::poll(&mut pollfd, 1, 0) };
            if result < 0 {
                return Err(format!(
                    "poll on retained pidfd failed: {}",
                    std::io::Error::last_os_error()
                ));
            }
            if pollfd.revents & libc::POLLNVAL != 0 {
                return Err("retained pidfd is invalid".into());
            }
            Ok(pollfd.revents & (libc::POLLIN | libc::POLLHUP) == 0)
        }
        #[cfg(target_os = "macos")]
        RetainedProcessInstance::MacOS(_) => Ok(true),
    }
}

/// The daemon obtains only its own task's kernel audit token.
#[cfg(target_os = "macos")]
pub(crate) fn own_audit_token() -> Result<String, String> {
    Ok(own_audit_token_words()?.serialize())
}

pub(crate) fn signal_retained_process_instance(
    target: &RetainedProcessInstance,
    signal: libc::c_int,
) -> ProcessSignalOutcome {
    match target {
        #[cfg(target_os = "linux")]
        RetainedProcessInstance::Linux(fd) => {
            use std::os::fd::AsRawFd;
            let result = unsafe {
                libc::syscall(
                    libc::SYS_pidfd_send_signal,
                    fd.as_raw_fd(),
                    signal,
                    std::ptr::null_mut::<libc::c_void>(),
                    0,
                )
            };
            if result == 0 {
                return ProcessSignalOutcome::Delivered;
            }
            let error = std::io::Error::last_os_error();
            match error.raw_os_error() {
                Some(error) => process_signal_error(error, "pidfd_send_signal"),
                None => ProcessSignalOutcome::Failed(format!(
                    "pidfd_send_signal failed without errno: {error}"
                )),
            }
        }
        #[cfg(target_os = "macos")]
        RetainedProcessInstance::MacOS(token) => {
            let Ok(signal_with_token) = proc_signal_with_audittoken() else {
                return ProcessSignalOutcome::Unavailable(
                    "proc_signal_with_audittoken is unavailable".into(),
                );
            };
            // The SDK takes a mutable pointer although the token is read-only.
            // Pass a private copy so the retained proof is not aliased mutably.
            let mut signal_token = *token;
            // SAFETY: `signal_token` is a valid complete audit token for this call.
            let result = unsafe { signal_with_token(&mut signal_token, signal) };
            if result == 0 {
                return ProcessSignalOutcome::Delivered;
            }
            process_signal_error(result, "proc_signal_with_audittoken")
        }
    }
}

#[cfg(target_os = "macos")]
const TASK_AUDIT_TOKEN: libc::task_flavor_t = 15;

#[cfg(target_os = "macos")]
type ProcSignalWithAuditToken =
    unsafe extern "C" fn(*mut MacAuditToken, libc::c_int) -> libc::c_int;

#[cfg(target_os = "macos")]
type AuditTokenToPid = unsafe extern "C" fn(MacAuditToken) -> libc::pid_t;

#[cfg(target_os = "macos")]
fn proc_signal_with_audittoken() -> Result<ProcSignalWithAuditToken, String> {
    // Resolve once so an absent optional interface reports refusal rather
    // than preventing normal startup or replacement classification.
    static SIGNAL: std::sync::OnceLock<Result<usize, String>> = std::sync::OnceLock::new();
    let address = match SIGNAL.get_or_init(|| {
        let pointer = unsafe {
            libc::dlsym(
                libc::RTLD_DEFAULT,
                b"proc_signal_with_audittoken\0".as_ptr().cast(),
            )
        };
        if pointer.is_null() {
            Err("proc_signal_with_audittoken is unavailable".into())
        } else {
            Ok(pointer as usize)
        }
    }) {
        Ok(address) => *address,
        Err(error) => return Err(error.clone()),
    };
    // SAFETY: the SDK declares this exact libproc signature.
    Ok(unsafe { std::mem::transmute::<usize, ProcSignalWithAuditToken>(address) })
}

#[cfg(target_os = "macos")]
fn audit_token_pid(token: &MacAuditToken) -> Result<libc::pid_t, String> {
    static TO_PID: std::sync::OnceLock<Result<usize, String>> = std::sync::OnceLock::new();
    let address = match TO_PID.get_or_init(|| {
        let pointer =
            unsafe { libc::dlsym(libc::RTLD_DEFAULT, b"audit_token_to_pid\0".as_ptr().cast()) };
        if pointer.is_null() {
            Err("audit_token_to_pid is unavailable".into())
        } else {
            Ok(pointer as usize)
        }
    }) {
        Ok(address) => *address,
        Err(error) => return Err(error.clone()),
    };
    let function: AuditTokenToPid =
        // SAFETY: the SDK declares this exact libbsm signature.
        unsafe { std::mem::transmute::<usize, AuditTokenToPid>(address) };
    Ok(unsafe { function(*token) })
}

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

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    static PUBLICATION_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

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

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn native_signal_errors_are_explicit_refusals_without_fallback() {
        assert_eq!(
            process_signal_error(libc::ESRCH, "retained signal"),
            ProcessSignalOutcome::InstanceGone
        );
        assert!(matches!(
            process_signal_error(libc::ENOSYS, "retained signal"),
            ProcessSignalOutcome::Unavailable(_)
        ));
        assert!(matches!(
            process_signal_error(libc::EPERM, "retained signal"),
            ProcessSignalOutcome::Failed(_)
        ));
        assert!(matches!(
            process_signal_error(libc::EACCES, "retained signal"),
            ProcessSignalOutcome::Failed(_)
        ));
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn publication_lock_enforces_reader_writer_exclusion_and_cloexec() {
        use std::os::fd::AsRawFd;

        let _test_guard = PUBLICATION_TEST_LOCK.lock().unwrap();
        let root = std::env::temp_dir().join(format!(
            "collab-publication-lock-{}-{:032x}",
            std::process::id(),
            rand::random::<u128>()
        ));
        std::fs::create_dir(&root).unwrap();
        let path = root.join("daemon.owner.lock");

        let first_reader = PublicationGuard::acquire(&path, false, true).unwrap();
        let second_reader = PublicationGuard::acquire(&path, false, false).unwrap();
        assert!(
            PublicationGuard::acquire(&path, true, false).is_err(),
            "an exclusive writer must not pass active readers"
        );
        drop(first_reader);
        drop(second_reader);

        let writer = PublicationGuard::acquire(&path, true, false).unwrap();
        assert!(
            PublicationGuard::acquire(&path, false, false).is_err(),
            "a shared reader must not pass an active writer"
        );
        let descriptor_flags = unsafe { libc::fcntl(writer.raw_fd(), libc::F_GETFD) };
        assert!(descriptor_flags >= 0);
        assert_ne!(descriptor_flags & libc::FD_CLOEXEC, 0);
        drop(writer);

        PublicationGuard::acquire(&path, false, false)
            .expect("dropping a guard must release its flock");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn publication_lock_is_not_inherited_after_exec() {
        const PROBE_ENV: &str = "COLLAB_PUBLICATION_LOCK_EXEC_PROBE";
        if let Some(path) = std::env::var_os(PROBE_ENV) {
            use std::io::BufRead;

            let mut command = String::new();
            std::io::stdin().lock().read_line(&mut command).unwrap();
            assert_eq!(command.trim(), "try-lock");
            PublicationGuard::acquire(std::path::Path::new(&path), true, false)
                .expect("exec child must not retain the parent's publication lock");
            return;
        }

        use std::io::Write;
        use std::os::fd::AsRawFd;

        let _test_guard = PUBLICATION_TEST_LOCK.lock().unwrap();
        let root = std::env::temp_dir().join(format!(
            "collab-publication-exec-{}-{:032x}",
            std::process::id(),
            rand::random::<u128>()
        ));
        std::fs::create_dir(&root).unwrap();
        let path = root.join("daemon.owner.lock");
        let reader = PublicationGuard::acquire(&path, false, true).unwrap();
        let descriptor_flags = unsafe { libc::fcntl(reader.raw_fd(), libc::F_GETFD) };
        assert!(descriptor_flags >= 0);
        assert_ne!(descriptor_flags & libc::FD_CLOEXEC, 0);

        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "host_runtime::tests::publication_lock_is_not_inherited_after_exec",
                "--nocapture",
            ])
            .env(PROBE_ENV, &path)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .expect("spawn publication-lock exec probe");
        drop(reader);
        child
            .stdin
            .take()
            .unwrap()
            .write_all(b"try-lock\n")
            .unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(
            output.status.success(),
            "exec child must acquire exclusive publication after parent drop: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn publication_reader_refuses_missing_protocol_file_without_creating_it() {
        let _test_guard = PUBLICATION_TEST_LOCK.lock().unwrap();
        let root = std::env::temp_dir().join(format!(
            "collab-publication-missing-{}-{:032x}",
            std::process::id(),
            rand::random::<u128>()
        ));
        std::fs::create_dir(&root).unwrap();
        let path = root.join("daemon.owner.lock");
        let error = match PublicationGuard::acquire(&path, false, false) {
            Ok(_) => panic!("missing publication protocol file must be refused"),
            Err(error) => error,
        };
        assert!(
            format!("{error:#}").contains("DAEMON_OWNER_PUBLICATION_REFUSED"),
            "publication refusal marker must remain visible: {error:#}"
        );
        assert_eq!(
            error
                .root_cause()
                .downcast_ref::<std::io::Error>()
                .and_then(std::io::Error::raw_os_error),
            Some(libc::ENOENT),
            "the OS acquisition error must remain in the error chain"
        );
        assert!(
            !path.exists(),
            "a replacement reader must not create the lock"
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_retained_pidfd_never_retargets_an_owned_successor() {
        let mut first = OwnedChild(
            std::process::Command::new("/bin/sleep")
                .arg("60")
                .spawn()
                .expect("spawn the first Linux retained-instance fixture"),
        );
        let first_pid = first.0.id();
        let target =
            retained_process_instance(first_pid).expect("retain the first Linux process instance");
        assert!(retained_process_instance_is_alive(&target).unwrap());

        first.0.kill().expect("terminate the first fixture");
        first.0.wait().expect("reap the first fixture");
        assert!(!retained_process_instance_is_alive(&target).unwrap());

        let mut successor = OwnedChild(
            std::process::Command::new("/bin/sleep")
                .arg("60")
                .spawn()
                .expect("spawn the successor Linux fixture"),
        );
        assert_eq!(
            signal_retained_process_instance(&target, libc::SIGTERM),
            ProcessSignalOutcome::InstanceGone
        );
        assert_eq!(
            signal_retained_process_instance(&target, libc::SIGKILL),
            ProcessSignalOutcome::InstanceGone
        );
        assert_eq!(
            first_pid,
            first.0.id(),
            "the retained target must remain bound to the original fixture PID"
        );
        assert!(
            successor.0.try_wait().unwrap().is_none(),
            "a retained pidfd must not signal the owned successor"
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_audit_token_helper_process() {
        if std::env::var_os("COLLAB_MACOS_AUDIT_TOKEN_HELPER").is_none() {
            return;
        }
        use std::io::Write;

        let token = own_audit_token_words().expect("helper must obtain its own audit token");
        println!("COLLAB_AUDIT_TOKEN {}", token.serialize());
        std::io::stdout().flush().unwrap();
        loop {
            std::thread::sleep(std::time::Duration::from_secs(60));
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_stale_generation_is_refused_and_valid_token_signals_owned_fixture() {
        use std::io::{BufRead, BufReader};
        use std::os::unix::process::ExitStatusExt;

        let child = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "host_runtime::tests::macos_audit_token_helper_process",
                "--nocapture",
            ])
            .env("COLLAB_MACOS_AUDIT_TOKEN_HELPER", "1")
            .stdout(std::process::Stdio::piped())
            .spawn()
            .expect("spawn the owned macOS token helper");
        let mut child = OwnedChild(child);
        let pid = child.0.id();
        let stdout = child.0.stdout.take().expect("capture helper stdout");
        let (sender, receiver) = std::sync::mpsc::sync_channel(1);
        std::thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            let mut line = String::new();
            while reader.read_line(&mut line).unwrap_or(0) > 0 {
                if let Some(token) = line.trim().strip_prefix("COLLAB_AUDIT_TOKEN ") {
                    let _ = sender.send(token.to_owned());
                    return;
                }
                line.clear();
            }
        });
        let text = receiver
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("owned fixture must publish its audit token");
        let token = MacAuditToken::parse(&text).expect("published token must parse");
        assert_eq!(
            audit_token_pid(&token).expect("the helper PID must match the token"),
            pid as libc::pid_t
        );

        // audit_token_t word 7 carries pidversion, the process-generation proof.
        let mut stale = token;
        stale.val[7] = stale.val[7].wrapping_add(1);
        assert_ne!(stale, token, "the stale-generation fixture must differ");
        let stale_target = RetainedProcessInstance::MacOS(stale);
        assert_eq!(
            signal_retained_process_instance(&stale_target, libc::SIGTERM),
            ProcessSignalOutcome::InstanceGone
        );
        assert_eq!(
            signal_retained_process_instance(&stale_target, libc::SIGKILL),
            ProcessSignalOutcome::InstanceGone
        );
        assert!(
            child.0.try_wait().unwrap().is_none(),
            "a stale audit-token generation must not signal the owned fixture"
        );

        let target = retained_process_instance(&text, pid).expect("retain the owned fixture");
        assert_eq!(
            signal_retained_process_instance(&target, libc::SIGTERM),
            ProcessSignalOutcome::Delivered
        );
        assert_eq!(
            child.0.wait().expect("reap the signalled helper").signal(),
            Some(libc::SIGTERM),
            "the valid token must deliver a normal termination signal"
        );
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
