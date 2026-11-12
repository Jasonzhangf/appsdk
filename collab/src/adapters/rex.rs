//! Rex terminal adapter primitives.
//!
//! This module is deliberately limited to one already-selected Rex endpoint.
//! It does not discover hosts, resolve agent identities, or own a status cache.
//! The daemon owns binding selection, event-task lifecycle, and state projection.

use crate::client::adapters::NotificationDeliveryClass;
use crate::proto::{RexCandidate, RexEndpoint, SelectedTransport, TransportKind};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::fmt;
use std::io;
use std::path::Path;
use std::path::PathBuf;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio::sync::mpsc;
use tokio::sync::watch;
use tokio::time::Instant;

pub const NOT_DELIVERED_PREFIX: &str = "REX_NOT_DELIVERED:";
pub const UNKNOWN_PREFIX: &str = "REX_UNKNOWN:";
pub const KNOWN_DELIVERED_PREFIX: &str = "REX_DELIVERED:";
pub const TERMINAL_CREATOR: &str = "com.superlogical.terminal";
pub const NATIVE_REJECTION_CODES: [&str; 7] = [
    "bad_request",
    "block_not_found",
    "conflict",
    "invalid_request",
    "method_not_found",
    "not_found",
    "session_not_found",
];

const DEFAULT_TIMEOUT_SECS: f64 = 10.0;
/// A Rex `terminal.write` returns when the bytes are queued, not when the
/// target program has read them. Give the terminal a bounded moment to
/// consume the body before the separate Enter key so a TUI editor submits the
/// complete text instead of dropping a key that arrives mid-paste.
const ENTER_SETTLE_SECS: f64 = 0.2;

#[cfg(any(target_os = "macos", test))]
fn installed_macos_cli(system_applications: &Path, user_home: Option<&Path>) -> PathBuf {
    let relative = "Rex Beta.app/Contents/Helpers/Rex Server.rex/Contents/MacOS/rex";
    let system_cli = system_applications.join(relative);
    if system_cli.is_file() {
        return system_cli;
    }
    if let Some(home) = user_home {
        let user_cli = home.join("Applications").join(relative);
        if user_cli.is_file() {
            return user_cli;
        }
    }
    system_cli
}

fn official_cli_path() -> String {
    #[cfg(test)]
    if let Some(path) = test_cli() {
        return path.to_string_lossy().into_owned();
    }
    #[cfg(target_os = "macos")]
    {
        let user_home = std::env::var_os("HOME").map(PathBuf::from);
        installed_macos_cli(Path::new("/Applications"), user_home.as_deref())
            .to_string_lossy()
            .into_owned()
    }
    #[cfg(target_os = "linux")]
    {
        "/usr/local/bin/rex".into()
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        String::new()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RexInputStage {
    BeforeWrite,
    WriteQueued { bytes_written: usize },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RexError {
    pub operation: &'static str,
    pub class: NotificationDeliveryClass,
    pub stage: RexInputStage,
    pub native_code: Option<String>,
    pub detail: String,
}

impl RexError {
    fn known_not_delivered(
        operation: &'static str,
        stage: RexInputStage,
        detail: impl Into<String>,
    ) -> Self {
        Self {
            operation,
            class: NotificationDeliveryClass::KnownNotDelivered,
            stage,
            native_code: None,
            detail: detail.into(),
        }
    }

    fn unknown(operation: &'static str, stage: RexInputStage, detail: impl Into<String>) -> Self {
        Self {
            operation,
            class: NotificationDeliveryClass::Unknown,
            stage,
            native_code: None,
            detail: detail.into(),
        }
    }

    fn native_rejection(
        operation: &'static str,
        stage: RexInputStage,
        code: String,
        detail: impl Into<String>,
    ) -> Self {
        Self {
            operation,
            class: NotificationDeliveryClass::KnownNotDelivered,
            stage,
            native_code: Some(code),
            detail: detail.into(),
        }
    }
}

impl fmt::Display for RexError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let prefix = match self.class {
            NotificationDeliveryClass::KnownNotDelivered => NOT_DELIVERED_PREFIX,
            NotificationDeliveryClass::Unknown => UNKNOWN_PREFIX,
            NotificationDeliveryClass::KnownDelivered => KNOWN_DELIVERED_PREFIX,
        };
        write!(
            f,
            "{prefix} operation={} stage={:?} native_code={} detail={}",
            self.operation,
            self.stage,
            self.native_code.as_deref().unwrap_or("none"),
            self.detail
        )
    }
}

impl std::error::Error for RexError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RexWriteReceipt {
    pub bytes_written: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RexProcessObservation {
    pub pid: Option<u64>,
    pub name: Option<String>,
    pub path: Option<String>,
    pub cwd: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RexProgramStatusRecord {
    pub id: Option<String>,
    pub state: Option<String>,
    pub kind: Option<String>,
    pub app: Option<String>,
    pub msg: Option<String>,
    pub updated_at: Option<String>,
    /// Foreground process at the time the program reported its status. Both
    /// pid and name are required; Rex reuses PIDs, so pid alone is stale.
    pub owner: Option<RexProgramStatusOwner>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RexProgramStatusOwner {
    pub pid: u64,
    pub name: String,
}

impl RexProgramStatusOwner {
    fn is_complete(&self) -> bool {
        self.pid > 0 && !self.name.trim().is_empty()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RexObservation {
    pub endpoint: RexEndpoint,
    pub process: Option<RexProcessObservation>,
    pub pwd: Option<String>,
    pub program_status: Vec<RexProgramStatusRecord>,
    pub sources: Vec<String>,
    pub source_errors: Vec<String>,
    pub gap_possible: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RexEvent {
    pub source: String,
    pub raw: Value,
    pub session_id: String,
    pub block_id: String,
    pub creator_name: String,
    pub name: String,
    pub payload: Value,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RexEventLine {
    Matched(RexEvent),
    Ignored,
    Malformed { detail: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RexEventBatch {
    pub events: Vec<RexEvent>,
    pub gap_possible: bool,
    pub detail: Option<String>,
}

#[derive(Debug)]
pub struct RexEventStream {
    pub cancel: watch::Sender<bool>,
    pub task: tokio::task::JoinHandle<RexEventBatch>,
}

fn validate_timeout_secs(value: f64) -> Result<Duration, RexError> {
    if !value.is_finite() || value <= 0.0 {
        return Err(RexError::known_not_delivered(
            "timeout",
            RexInputStage::BeforeWrite,
            "Rex timeout must be positive and finite",
        ));
    }
    Ok(Duration::from_secs_f64(value))
}

fn default_timeout() -> Result<Duration, RexError> {
    #[cfg(test)]
    if let Some(value) = test_timeout() {
        return validate_timeout_secs(value.as_secs_f64());
    }
    validate_timeout_secs(DEFAULT_TIMEOUT_SECS)
}

#[cfg(test)]
static TEST_CLI: std::sync::OnceLock<std::sync::RwLock<Option<PathBuf>>> =
    std::sync::OnceLock::new();
#[cfg(test)]
static TEST_TIMEOUT: std::sync::OnceLock<std::sync::RwLock<Option<Duration>>> =
    std::sync::OnceLock::new();
#[cfg(test)]
static TEST_ENTER_SETTLE: std::sync::OnceLock<std::sync::RwLock<Option<Duration>>> =
    std::sync::OnceLock::new();
#[cfg(test)]
pub(crate) static TEST_FIXTURE_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

#[cfg(test)]
fn test_cli() -> Option<PathBuf> {
    TEST_CLI
        .get_or_init(|| std::sync::RwLock::new(None))
        .read()
        .expect("Rex test CLI lock")
        .clone()
}

#[cfg(test)]
pub(crate) fn set_test_cli(value: Option<PathBuf>) {
    *TEST_CLI
        .get_or_init(|| std::sync::RwLock::new(None))
        .write()
        .expect("Rex test CLI lock") = value;
}

#[cfg(test)]
fn test_timeout() -> Option<Duration> {
    *TEST_TIMEOUT
        .get_or_init(|| std::sync::RwLock::new(None))
        .read()
        .expect("Rex test timeout lock")
}

#[cfg(test)]
pub(crate) fn set_test_timeout(value: Option<Duration>) {
    *TEST_TIMEOUT
        .get_or_init(|| std::sync::RwLock::new(None))
        .write()
        .expect("Rex test timeout lock") = value;
}

#[cfg(test)]
fn enter_settle() -> Duration {
    (*TEST_ENTER_SETTLE
        .get_or_init(|| std::sync::RwLock::new(None))
        .read()
        .expect("Rex test enter settle lock"))
    .unwrap_or_else(|| Duration::from_secs_f64(ENTER_SETTLE_SECS))
}

#[cfg(not(test))]
fn enter_settle() -> Duration {
    Duration::from_secs_f64(ENTER_SETTLE_SECS)
}

#[cfg(test)]
pub(crate) fn set_test_enter_settle(value: Option<Duration>) {
    *TEST_ENTER_SETTLE
        .get_or_init(|| std::sync::RwLock::new(None))
        .write()
        .expect("Rex test enter settle lock") = value;
}

struct OwnedReader {
    task: tokio::task::JoinHandle<io::Result<String>>,
}

impl Drop for OwnedReader {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// One CLI stage within its caller's absolute operation deadline. Dropping the
/// future also terminates its owned child and cancels both owned pipe readers.
async fn run_cli(
    operation: &'static str,
    cli_path: &Path,
    server: &str,
    deadline: Instant,
    command: &[String],
    stdin_body: Option<String>,
) -> Result<CompletedCall, RexError> {
    run_cli_with_capture(
        operation,
        cli_path,
        server,
        deadline,
        command,
        stdin_body,
        StdoutCapture::All,
    )
    .await
}

async fn run_cli_tail(
    operation: &'static str,
    cli_path: &Path,
    server: &str,
    deadline: Instant,
    command: &[String],
    stdin_body: Option<String>,
    lines: usize,
) -> Result<CompletedCall, RexError> {
    run_cli_with_capture(
        operation,
        cli_path,
        server,
        deadline,
        command,
        stdin_body,
        StdoutCapture::LastLines(lines),
    )
    .await
}

#[derive(Clone, Copy)]
enum StdoutCapture {
    All,
    LastLines(usize),
}

/// One CLI stage within its caller's absolute operation deadline. Dropping the
/// future also terminates its owned child and cancels both owned pipe readers.
async fn run_cli_with_capture(
    operation: &'static str,
    cli_path: &Path,
    server: &str,
    deadline: Instant,
    command: &[String],
    stdin_body: Option<String>,
    stdout_capture: StdoutCapture,
) -> Result<CompletedCall, RexError> {
    let remaining = deadline.saturating_duration_since(Instant::now());
    if remaining.is_zero() {
        return Err(RexError::known_not_delivered(
            operation,
            RexInputStage::BeforeWrite,
            "Rex operation deadline elapsed before starting the CLI stage",
        ));
    }
    let timeout_arg = format!("{}s", remaining.as_secs_f64());
    let mut cmd = Command::new(cli_path);
    cmd.args([
        "--server",
        server,
        "--autostart=false",
        "--timeout",
        timeout_arg.as_str(),
    ])
    .args(command)
    .env_clear()
    .envs(std::env::vars_os().filter(|(key, _)| !key.as_encoded_bytes().starts_with(b"REX_")))
    .stdin(std::process::Stdio::piped())
    .stdout(std::process::Stdio::piped())
    .stderr(std::process::Stdio::piped())
    .kill_on_drop(true);

    if Instant::now() >= deadline {
        return Err(RexError::known_not_delivered(
            operation,
            RexInputStage::BeforeWrite,
            "Rex operation deadline elapsed before spawning the CLI",
        ));
    }
    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(error) => {
            return Err(RexError::known_not_delivered(
                operation,
                RexInputStage::BeforeWrite,
                format!("official Rex CLI did not start: {error}"),
            ));
        }
    };

    let stdout = child.stdout.take().expect("child stdout captured");
    let stderr = child.stderr.take().expect("child stderr captured");
    let mut stdout_reader = OwnedReader {
        task: tokio::spawn(drain_stdout(stdout, stdout_capture)),
    };
    let mut stderr_reader = OwnedReader {
        task: tokio::spawn(drain_stdout(stderr, StdoutCapture::All)),
    };
    let mut phase = "stdin write";
    let completion = tokio::time::timeout_at(deadline, async {
        if Instant::now() >= deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "deadline elapsed before stdin write",
            ));
        }
        if let Some(body) = stdin_body {
            let mut stdin = child
                .stdin
                .take()
                .ok_or_else(|| io::Error::other("CLI stdin unavailable"))?;
            stdin.write_all(body.as_bytes()).await?;
        }
        drop(child.stdin.take());
        phase = "process exit";
        let status = child.wait().await?;
        phase = "stdout read";
        let stdout = (&mut stdout_reader.task)
            .await
            .map_err(io::Error::other)??;
        phase = "stderr read";
        let stderr = (&mut stderr_reader.task)
            .await
            .map_err(io::Error::other)??;
        Ok::<_, io::Error>(CompletedCall {
            status,
            stdout,
            stderr,
        })
    })
    .await;
    let mut detail = match completion {
        Ok(Ok(call)) => return Ok(call),
        Ok(Err(error)) => format!("official Rex CLI IO failed: {error}"),
        Err(_) => format!("official Rex CLI timed out during {phase}"),
    };
    if let Err(error) = kill_and_reap(&mut child, deadline).await {
        detail.push_str(&format!("; owned child cleanup failed: {error}"));
    }
    if let Err(error) = finish_reader(&mut stdout_reader, deadline).await {
        detail.push_str(&format!("; stdout cleanup: {error}"));
    }
    if let Err(error) = finish_reader(&mut stderr_reader, deadline).await {
        detail.push_str(&format!("; stderr cleanup: {error}"));
    }
    Err(RexError::unknown(
        operation,
        RexInputStage::BeforeWrite,
        detail,
    ))
}

async fn drain_stdout(
    mut reader: impl AsyncRead + Unpin + Send + 'static,
    capture: StdoutCapture,
) -> io::Result<String> {
    use tokio::io::AsyncReadExt;

    if let StdoutCapture::All = capture {
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).await?;
        return String::from_utf8(bytes)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error));
    }
    let StdoutCapture::LastLines(max_lines) = capture else {
        unreachable!("StdoutCapture::All handled above");
    };
    let mut chunk = [0u8; 8192];
    let mut carry: Vec<u8> = Vec::new();
    let mut lines: std::collections::VecDeque<Vec<u8>> =
        std::collections::VecDeque::with_capacity(max_lines.min(256));
    loop {
        let read = reader.read(&mut chunk).await?;
        if read == 0 {
            break;
        }
        carry.extend_from_slice(&chunk[..read]);
        while let Some(newline) = carry.iter().position(|byte| *byte == b'\n') {
            let mut line: Vec<u8> = carry.drain(..=newline).collect();
            line.pop();
            push_tail_line(&mut lines, line, max_lines);
        }
    }
    if !carry.is_empty() {
        push_tail_line(&mut lines, carry, max_lines);
    }
    let mut output: Vec<u8> = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        if index > 0 {
            output.push(b'\n');
        }
        output.extend_from_slice(line);
    }
    String::from_utf8(output).map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

fn push_tail_line(
    lines: &mut std::collections::VecDeque<Vec<u8>>,
    line: Vec<u8>,
    max_lines: usize,
) {
    if max_lines == 0 {
        return;
    }
    if lines.len() == max_lines {
        lines.pop_front();
    }
    lines.push_back(line);
}

async fn kill_and_reap(child: &mut tokio::process::Child, deadline: Instant) -> io::Result<()> {
    if child.try_wait()?.is_some() {
        return Ok(());
    }
    let killed = child.start_kill();
    if child.try_wait()?.is_some() {
        return Ok(());
    }
    killed?;
    if Instant::now() < deadline {
        match tokio::time::timeout_at(deadline, child.wait()).await {
            Ok(status) => return status.map(|_| ()),
            Err(_) => {}
        }
    }
    if child.try_wait()?.is_some() {
        return Ok(());
    }
    Err(io::Error::new(
        io::ErrorKind::TimedOut,
        "child_reap_unconfirmed",
    ))
}

async fn finish_reader(reader: &mut OwnedReader, deadline: Instant) -> io::Result<()> {
    if reader.task.is_finished() {
        return Ok(());
    }
    reader.task.abort();
    if Instant::now() < deadline {
        if tokio::time::timeout_at(deadline, &mut reader.task)
            .await
            .is_ok()
        {
            return Ok(());
        }
    }
    if reader.task.is_finished() {
        return Ok(());
    }
    Err(io::Error::new(
        io::ErrorKind::TimedOut,
        "reader_completion_unconfirmed",
    ))
}

fn validate_target_fields(server: &str, session: &str, block: &str) -> Result<(), RexError> {
    if server.trim().is_empty() || server.chars().any(char::is_control) {
        return Err(RexError::known_not_delivered(
            "target",
            RexInputStage::BeforeWrite,
            "Rex server must be explicit and nonempty",
        ));
    }
    for (prefix, id) in [("session:", session), ("block:", block)] {
        let suffix = id.strip_prefix(prefix).unwrap_or("");
        let segments: Vec<_> = suffix.split('-').collect();
        let canonical = segments
            .iter()
            .map(|segment| segment.len())
            .eq([8, 4, 4, 4, 12])
            && segments.iter().all(|segment| {
                segment
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            });
        if !canonical {
            return Err(RexError::known_not_delivered(
                "target",
                RexInputStage::BeforeWrite,
                format!("Rex target requires a full {prefix} ID with a canonical lowercase UUID"),
            ));
        }
    }
    Ok(())
}

fn nonempty_env(key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

/// True only when both block environment variables are present and non-empty.
///
/// A single variable is not a Rex block address. Callers use this to detect an
/// explicit candidate conflict before any CLI work.
pub fn env_block_present() -> bool {
    nonempty_env("REX_SESSION").is_some() && nonempty_env("REX_BLOCK").is_some()
}

/// True when either block variable is present.
///
/// A partial pair still means the caller is inside a Rex-aware environment.
/// It must not fall through to another transport, even though it cannot form a
/// complete Rex candidate.
pub fn env_block_hint() -> bool {
    nonempty_env("REX_SESSION").is_some() || nonempty_env("REX_BLOCK").is_some()
}

/// Read the official CLI's actual server endpoint from `whoami --json`.
///
/// This runs with the caller's environment intact so the CLI resolves the same
/// server the block belongs to; it never guesses a socket. A failed or
/// malformed reply stays a typed error.
fn whoami_endpoint() -> Result<String, RexError> {
    let operation = "discover_env";
    let call = std::process::Command::new(official_cli_path())
        .args(["whoami", "--json", "--autostart=false"])
        .output()
        .map_err(|error| {
            RexError::known_not_delivered(
                operation,
                RexInputStage::BeforeWrite,
                format!("official Rex CLI did not start: {error}"),
            )
        })?;
    let stdout = String::from_utf8_lossy(&call.stdout);
    let stderr = String::from_utf8_lossy(&call.stderr);
    if !call.status.success() {
        return Err(RexError::unknown(
            operation,
            RexInputStage::BeforeWrite,
            format!(
                "official Rex CLI whoami failed with returncode {}: stderr={}",
                call.status.code().unwrap_or(-1),
                stderr.trim()
            ),
        ));
    }
    let value: Value = serde_json::from_str(stdout.trim()).map_err(|error| {
        RexError::unknown(
            operation,
            RexInputStage::BeforeWrite,
            format!("official Rex CLI returned malformed whoami JSON: {error}"),
        )
    })?;
    value
        .get("endpoint")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| {
            RexError::unknown(
                operation,
                RexInputStage::BeforeWrite,
                "official Rex CLI whoami did not report a server endpoint",
            )
        })
}

/// Discover this invocation's Rex block placement from `REX_SESSION` and
/// `REX_BLOCK`.
///
/// Both variables are required. A partial pair is not a Rex block address and
/// yields `None` rather than a fabricated candidate. The server comes from the
/// official CLI's successful `whoami --json` endpoint, so the adapter never
/// invents a socket or falls back to another transport.
pub(crate) fn candidate_from_env() -> Result<Option<RexCandidate>, RexError> {
    let Some(session_id) = nonempty_env("REX_SESSION") else {
        return Ok(None);
    };
    let Some(block_id) = nonempty_env("REX_BLOCK") else {
        return Ok(None);
    };
    let server = whoami_endpoint()?;
    validate_target_fields(&server, &session_id, &block_id)?;
    let cwd = crate::scope::process_cwd().map_err(|error| {
        RexError::known_not_delivered(
            "discover_env",
            RexInputStage::BeforeWrite,
            format!("cannot resolve Rex candidate cwd: {error}"),
        )
    })?;
    let cwd = cwd.to_str().map(str::to_owned).ok_or_else(|| {
        RexError::known_not_delivered(
            "discover_env",
            RexInputStage::BeforeWrite,
            "Rex candidate cwd must be valid UTF-8",
        )
    })?;
    Ok(Some(RexCandidate {
        server,
        session_id,
        block_id,
        cwd,
    }))
}

#[derive(Debug, Clone)]
enum NativeError {
    Typed { code: String },
    Opaque { returncode: i32, detail: String },
}

fn json_payload_error(
    operation: &'static str,
    stage: RexInputStage,
    detail: impl Into<String>,
) -> RexError {
    RexError::unknown(operation, stage, detail)
}

fn native_failure(operation: &'static str, stage: RexInputStage, call: &CompletedCall) -> RexError {
    match call.native_error() {
        NativeError::Typed { code } if NATIVE_REJECTION_CODES.contains(&code.as_str()) => {
            RexError::native_rejection(operation, stage, code, call.stderr.trim().to_string())
        }
        NativeError::Typed { code } => RexError::unknown(
            operation,
            stage,
            format!("native reply carried unclassified code {code}"),
        ),
        NativeError::Opaque { returncode, detail } => RexError::unknown(
            operation,
            stage,
            format!("native call failed with returncode {returncode}: {detail}"),
        ),
    }
}

fn parse_json_call(operation: &'static str, call: &CompletedCall) -> Result<Value, RexError> {
    if !call.status.success() {
        return Err(native_failure(operation, RexInputStage::BeforeWrite, call));
    }
    serde_json::from_str(call.stdout.trim()).map_err(|error| {
        json_payload_error(
            operation,
            RexInputStage::BeforeWrite,
            format!("official Rex CLI returned malformed JSON: {error}"),
        )
    })
}

#[derive(Debug, Clone)]
struct CompletedCall {
    status: std::process::ExitStatus,
    stdout: String,
    stderr: String,
}

impl CompletedCall {
    fn returncode(&self) -> i32 {
        self.status.code().unwrap_or(-1)
    }

    fn native_error(&self) -> NativeError {
        if let Ok(value) = serde_json::from_str::<Value>(self.stdout.trim()) {
            let nested = value.get("error");
            if let Some(code) = value.get("code").and_then(Value::as_str) {
                return NativeError::Typed {
                    code: code.to_string(),
                };
            }
            if let Some(code) = nested
                .and_then(|nested| nested.get("code"))
                .and_then(Value::as_str)
            {
                return NativeError::Typed {
                    code: code.to_string(),
                };
            }
        }
        NativeError::Opaque {
            returncode: self.returncode(),
            detail: format!(
                "stdout={} stderr={}",
                self.stdout.trim(),
                self.stderr.trim()
            ),
        }
    }
}

fn api_call_argv(method: &str, session_id: Option<&str>) -> Vec<String> {
    let mut argv = vec!["api".to_string(), "call".to_string()];
    if let Some(session_id) = session_id {
        argv.push("-s".to_string());
        argv.push(session_id.to_string());
    }
    argv.push(method.to_string());
    argv.push("-".to_string());
    argv
}

fn terminal_write_argv(endpoint: &RexEndpoint) -> Vec<String> {
    api_call_argv(
        "com.superlogical.terminal.write",
        Some(endpoint.session_id.as_str()),
    )
}

fn send_key_argv(endpoint: &RexEndpoint) -> Vec<String> {
    vec![
        "send-key".to_string(),
        "-s".to_string(),
        endpoint.session_id.clone(),
        "-b".to_string(),
        endpoint.block_id.clone(),
        "enter".to_string(),
    ]
}

fn terminal_query_argv(endpoint: &RexEndpoint, method: &str) -> Vec<String> {
    api_call_argv(method, Some(endpoint.session_id.as_str()))
}

fn terminal_query_body(endpoint: &RexEndpoint) -> String {
    serde_json::to_string(&json!({
        "session_id": endpoint.session_id,
        "block_id": endpoint.block_id,
        "args": {},
    }))
    .expect("observe JSON")
}

fn capture_argv(endpoint: &RexEndpoint) -> Vec<String> {
    vec![
        "capture".to_string(),
        "-s".to_string(),
        endpoint.session_id.clone(),
        "-b".to_string(),
        endpoint.block_id.clone(),
        "--format=text".to_string(),
        "--unwrap".to_string(),
        "--trim".to_string(),
    ]
}

pub async fn verify_candidate(
    candidate: &RexCandidate,
    agent_session_id: &str,
) -> Result<SelectedTransport, RexError> {
    let deadline = Instant::now() + default_timeout()?;
    validate_target_fields(
        &candidate.server,
        &candidate.session_id,
        &candidate.block_id,
    )?;
    if agent_session_id.trim().is_empty() {
        return Err(RexError::known_not_delivered(
            "verify_candidate",
            RexInputStage::BeforeWrite,
            "agent sessionID is required",
        ));
    }
    let status_call = run_cli(
        "server.status",
        Path::new(&official_cli_path()),
        &candidate.server,
        deadline,
        &api_call_argv("server.status", None),
        Some("{}".to_string()),
    )
    .await?;
    let status = parse_json_call("server.status", &status_call)?;
    let host_id = status
        .get("host_id")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let instance_id = status
        .get("instance_id")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(str::to_owned)
        .ok_or_else(|| {
            RexError::known_not_delivered(
                "server.status",
                RexInputStage::BeforeWrite,
                "Rex service did not provide its instance_id",
            )
        })?;
    let endpoint = RexEndpoint {
        server: candidate.server.clone(),
        session_id: candidate.session_id.clone(),
        block_id: candidate.block_id.clone(),
        host_id,
        instance_id,
        cwd: candidate.cwd.clone(),
    };
    resolve_target_until(&endpoint, deadline).await?;
    Ok(SelectedTransport {
        kind: TransportKind::Rex,
        endpoint: None,
        namespace: None,
        // Rex block/session IDs are delivery addresses, never the agent identity.
        session_id: Some(agent_session_id.to_owned()),
        thread_id: None,
        tmux_endpoint: None,
        rex_endpoint: Some(endpoint),
        capabilities: vec![
            "send_message_to_block".to_owned(),
            "observe_current".to_owned(),
            "events".to_owned(),
        ],
        self_check: "official Rex server.status, session.list and session.list_blocks passed"
            .to_owned(),
    })
}

pub async fn resolve_target(endpoint: &RexEndpoint) -> Result<(), RexError> {
    resolve_target_until(endpoint, Instant::now() + default_timeout()?).await
}

async fn resolve_target_until(endpoint: &RexEndpoint, deadline: Instant) -> Result<(), RexError> {
    verify_service_until(endpoint, deadline).await?;
    let session_call = run_cli(
        "session.list",
        Path::new(&official_cli_path()),
        &endpoint.server,
        deadline,
        &api_call_argv("session.list", None),
        Some("{}".to_string()),
    )
    .await?;
    let sessions = parse_json_call("session.list", &session_call)?;
    let found_session = sessions
        .get("sessions")
        .and_then(Value::as_array)
        .map(|items| {
            items.iter().filter_map(Value::as_object).any(|item| {
                item.get("session_id").and_then(Value::as_str) == Some(&endpoint.session_id)
            })
        })
        .unwrap_or(false);
    if !found_session {
        return Err(RexError::known_not_delivered(
            "resolve_target",
            RexInputStage::BeforeWrite,
            "exact Rex session was not present",
        ));
    }

    let blocks_call = run_cli(
        "session.list_blocks",
        Path::new(&official_cli_path()),
        &endpoint.server,
        deadline,
        &api_call_argv("session.list_blocks", Some(&endpoint.session_id)),
        Some(serde_json::to_string(&json!({"session_id": endpoint.session_id})).expect("JSON")),
    )
    .await?;
    let blocks = parse_json_call("session.list_blocks", &blocks_call)?;
    let found_block = blocks
        .get("blocks")
        .and_then(Value::as_array)
        .map(|items| {
            items.iter().filter_map(Value::as_object).any(|item| {
                item.get("block_id").and_then(Value::as_str) == Some(&endpoint.block_id)
                    && item.get("creator_name").and_then(Value::as_str) == Some(TERMINAL_CREATOR)
            })
        })
        .unwrap_or(false);
    if !found_block {
        return Err(RexError::known_not_delivered(
            "resolve_target",
            RexInputStage::BeforeWrite,
            "exact terminal block was not present in the session",
        ));
    }
    Ok(())
}

async fn verify_service_until(endpoint: &RexEndpoint, deadline: Instant) -> Result<(), RexError> {
    validate_target_fields(&endpoint.server, &endpoint.session_id, &endpoint.block_id)?;
    if endpoint.instance_id.trim().is_empty() {
        return Err(RexError::known_not_delivered(
            "server.status",
            RexInputStage::BeforeWrite,
            "the bound service has no instance_id",
        ));
    }
    let checked = async {
        let call = run_cli(
            "server.status",
            Path::new(&official_cli_path()),
            &endpoint.server,
            deadline,
            &api_call_argv("server.status", None),
            Some("{}".into()),
        )
        .await?;
        parse_json_call("server.status", &call)
    }
    .await;
    let status = checked.map_err(|mut error: RexError| {
        error.class = NotificationDeliveryClass::KnownNotDelivered;
        error.stage = RexInputStage::BeforeWrite;
        error
    })?;
    let instance_matches =
        status.get("instance_id").and_then(Value::as_str) == Some(endpoint.instance_id.as_str());
    let host_matches = endpoint
        .host_id
        .as_deref()
        .is_none_or(|host| status.get("host_id").and_then(Value::as_str) == Some(host));
    if !instance_matches || !host_matches {
        return Err(RexError::known_not_delivered("server.status", RexInputStage::BeforeWrite,
            "REX_SERVICE_CHANGED: the bound host or instance changed; refresh the current Rex binding"));
    }
    Ok(())
}

pub async fn write_text(endpoint: &RexEndpoint, text: &str) -> Result<RexWriteReceipt, RexError> {
    write_text_until(endpoint, text, Instant::now() + default_timeout()?).await
}

async fn write_text_until(
    endpoint: &RexEndpoint,
    text: &str,
    deadline: Instant,
) -> Result<RexWriteReceipt, RexError> {
    verify_service_until(endpoint, deadline).await?;
    let request = json!({
        "session_id": endpoint.session_id,
        "block_id": endpoint.block_id,
        "args": {"data": text},
    });
    let call = run_cli(
        "write_text",
        Path::new(&official_cli_path()),
        &endpoint.server,
        deadline,
        &terminal_write_argv(endpoint),
        Some(serde_json::to_string(&request).expect("write JSON")),
    )
    .await?;
    if !call.status.success() {
        return Err(native_failure(
            "write_text",
            RexInputStage::BeforeWrite,
            &call,
        ));
    }
    let value: Value = serde_json::from_str(call.stdout.trim()).map_err(|error| {
        json_payload_error(
            "write_text",
            RexInputStage::BeforeWrite,
            format!("write reply was not JSON: {error}"),
        )
    })?;
    match value.get("bytes_written").and_then(Value::as_u64) {
        Some(bytes) if bytes as usize == text.as_bytes().len() => Ok(RexWriteReceipt {
            bytes_written: bytes as usize,
        }),
        Some(bytes) => Err(json_payload_error(
            "write_text",
            RexInputStage::WriteQueued {
                bytes_written: bytes as usize,
            },
            format!("write queued {bytes} bytes but expected {}", text.len()),
        )),
        _ => Err(json_payload_error(
            "write_text",
            RexInputStage::BeforeWrite,
            format!("write result did not confirm the full UTF-8 payload: {value}"),
        )),
    }
}

/// Send the real Enter key event after the body write. The write stage is
/// passed in so an Enter failure is classified at the body-write boundary.
async fn send_enter_until(endpoint: &RexEndpoint, deadline: Instant) -> Result<(), RexError> {
    let remaining = deadline.saturating_duration_since(Instant::now());
    let settle = enter_settle().min(remaining);
    if !settle.is_zero() {
        tokio::time::sleep(settle).await;
    }
    verify_service_until(endpoint, deadline).await?;
    let call = run_cli(
        "send_enter",
        Path::new(&official_cli_path()),
        &endpoint.server,
        deadline,
        &send_key_argv(endpoint),
        None,
    )
    .await?;
    if !call.status.success() {
        return Err(native_failure(
            "send_enter",
            RexInputStage::BeforeWrite,
            &call,
        ));
    }
    Ok(())
}

/// Read the latest requested number of text lines from one verified endpoint.
/// This is a read-only observation: it never writes input, never changes
/// activity, and never persists the captured text.
pub async fn snapshot(endpoint: &RexEndpoint, lines: usize) -> Result<Value, RexError> {
    if !(1..=200).contains(&lines) {
        return Err(RexError::known_not_delivered(
            "snapshot",
            RexInputStage::BeforeWrite,
            "snapshot lines must be 1..200",
        ));
    }
    let deadline = Instant::now() + default_timeout()?;
    snapshot_until(endpoint, lines, deadline).await
}

async fn snapshot_until(
    endpoint: &RexEndpoint,
    lines: usize,
    deadline: Instant,
) -> Result<Value, RexError> {
    verify_service_until(endpoint, deadline).await?;
    let call = run_cli_tail(
        "snapshot",
        Path::new(&official_cli_path()),
        &endpoint.server,
        deadline,
        &capture_argv(endpoint),
        None,
        lines,
    )
    .await?;
    if !call.status.success() {
        return Err(native_failure(
            "snapshot",
            RexInputStage::BeforeWrite,
            &call,
        ));
    }
    Ok(json!({
        "transport": "rex",
        "session_id": endpoint.session_id,
        "block_id": endpoint.block_id,
        "lines": lines,
        "output": call.stdout,
        "durable": false,
    }))
}

pub async fn observe_current(endpoint: &RexEndpoint) -> Result<RexObservation, RexError> {
    let deadline = Instant::now() + default_timeout()?;
    verify_service_until(endpoint, deadline).await?;
    let mut sources = Vec::new();
    let mut source_errors = Vec::new();
    let mut process = None;
    let mut pwd = None;
    let mut program_status = Vec::new();
    for method in [
        "com.superlogical.terminal.process",
        "com.superlogical.terminal.pwd",
        "com.superlogical.terminal.program_status",
    ] {
        let call = run_cli(
            "observe_current",
            Path::new(&official_cli_path()),
            &endpoint.server,
            deadline,
            &terminal_query_argv(endpoint, method),
            Some(terminal_query_body(endpoint)),
        )
        .await;
        match call {
            Ok(call) if call.status.success() => match parse_json_call("observe_current", &call) {
                Ok(value) => {
                    sources.push(method.to_owned());
                    match method {
                        "com.superlogical.terminal.process" => {
                            process = Some(extract_process(&value));
                        }
                        "com.superlogical.terminal.pwd" => {
                            pwd = value.get("pwd").and_then(Value::as_str).map(str::to_owned);
                        }
                        "com.superlogical.terminal.program_status" => {
                            program_status = extract_status_records(&value);
                        }
                        _ => {}
                    }
                }
                Err(error) => source_errors.push(error.detail),
            },
            Ok(call) => {
                let error = native_failure("observe_current", RexInputStage::BeforeWrite, &call);
                source_errors.push(format!("{method}: {error}"));
            }
            Err(error) => source_errors.push(format!("{method}: {error}")),
        }
    }
    let gap_possible = !source_errors.is_empty();
    Ok(RexObservation {
        endpoint: endpoint.clone(),
        process,
        pwd,
        program_status,
        sources,
        source_errors,
        gap_possible,
    })
}

fn extract_process(value: &Value) -> RexProcessObservation {
    let foreground = value.get("foreground").or_else(|| value.get("child"));
    RexProcessObservation {
        pid: foreground
            .and_then(|item| item.get("pid"))
            .and_then(Value::as_u64),
        name: foreground
            .and_then(|item| item.get("name"))
            .and_then(Value::as_str)
            .map(str::to_owned),
        path: foreground
            .and_then(|item| item.get("path"))
            .and_then(Value::as_str)
            .map(str::to_owned),
        cwd: value
            .get("foreground")
            .and_then(|item| item.get("cwd"))
            .or_else(|| value.get("child").and_then(|item| item.get("cwd")))
            .and_then(Value::as_str)
            .map(str::to_owned),
    }
}

fn extract_status_records(value: &Value) -> Vec<RexProgramStatusRecord> {
    let records = match value.get("records").and_then(Value::as_array) {
        Some(records) => records,
        None => return Vec::new(),
    };
    records
        .iter()
        .filter_map(Value::as_object)
        .map(|record| RexProgramStatusRecord {
            id: record.get("id").and_then(Value::as_str).map(str::to_owned),
            state: record
                .get("state")
                .and_then(Value::as_str)
                .map(str::to_owned),
            kind: record
                .get("kind")
                .and_then(Value::as_str)
                .map(str::to_owned),
            app: record.get("app").and_then(Value::as_str).map(str::to_owned),
            msg: record.get("msg").and_then(Value::as_str).map(str::to_owned),
            updated_at: record
                .get("updated_at")
                .and_then(Value::as_str)
                .map(str::to_owned),
            owner: record
                .get("owner")
                .and_then(Value::as_object)
                .and_then(|owner| {
                    let pid = owner.get("pid").and_then(Value::as_u64)?;
                    let name = owner.get("name").and_then(Value::as_str)?.to_owned();
                    let owner = RexProgramStatusOwner { pid, name };
                    owner.is_complete().then_some(owner)
                }),
        })
        .collect()
}

pub async fn notify(
    transport: &SelectedTransport,
    text: &str,
    mode: Option<&str>,
) -> Result<String, RexError> {
    let deadline = Instant::now() + default_timeout()?;
    let allowed = [
        None,
        Some("immediate"),
        Some("explicit-notification"),
        Some("daemon-live-closure"),
        Some("restart-replay-pending"),
    ];
    if !allowed.iter().any(|candidate| *candidate == mode) {
        return Err(RexError::known_not_delivered(
            "notify",
            RexInputStage::BeforeWrite,
            format!(
                "Rex mode {:?} is not a terminal submission mode",
                mode.unwrap_or("<none>")
            ),
        ));
    }
    let endpoint = transport.rex_endpoint.as_ref().ok_or_else(|| {
        RexError::known_not_delivered(
            "notify",
            RexInputStage::BeforeWrite,
            "selected transport carries no verified Rex endpoint",
        )
    })?;
    // The body write and the real Enter key are separate stages. A text write
    // ending in CR only queues characters; a real Codex TUI treats that CR as
    // input text and leaves the prompt unsubmitted. If Enter fails after the
    // body write, report the queued write boundary and never resend the body.
    let receipt = write_text_until(endpoint, text, deadline).await?;
    let text_stage = RexInputStage::WriteQueued {
        bytes_written: receipt.bytes_written,
    };
    if let Err(error) = send_enter_until(endpoint, deadline).await {
        return Err(RexError {
            operation: "notify",
            class: NotificationDeliveryClass::Unknown,
            stage: text_stage,
            native_code: error.native_code,
            detail: error.detail,
        });
    }
    let value = json!({
        "transport": "rex",
        "bytes_written": receipt.bytes_written,
        "text_queued": true,
        "enter_accepted": true,
        "consumed": false,
        "mode": mode.unwrap_or("immediate"),
    });
    Ok(value.to_string())
}

pub fn parse_event_line(endpoint: &RexEndpoint, line: &str) -> RexEventLine {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return RexEventLine::Ignored;
    }
    let value: Value = match serde_json::from_str(trimmed) {
        Ok(value) => value,
        Err(error) => {
            return RexEventLine::Malformed {
                detail: error.to_string(),
            };
        }
    };
    let Some(name) = event_name(endpoint, &value) else {
        return RexEventLine::Ignored;
    };
    let payload = value["payload"]["event"]["payload"]
        .get("payload")
        .cloned()
        .unwrap_or_else(|| json!({}));
    RexEventLine::Matched(RexEvent {
        source: format!("rex.events:{}", endpoint.instance_id),
        session_id: endpoint.session_id.clone(),
        block_id: endpoint.block_id.clone(),
        creator_name: TERMINAL_CREATOR.to_string(),
        name: name.to_string(),
        payload,
        raw: value,
    })
}

fn event_name<'a>(endpoint: &'a RexEndpoint, value: &'a Value) -> Option<&'a str> {
    if value.get("type").and_then(Value::as_str) != Some("session_event") {
        return None;
    }
    let wrapper = value.get("payload")?;
    if wrapper.get("session_id").and_then(Value::as_str) != Some(&endpoint.session_id) {
        return None;
    }
    if wrapper
        .get("event")
        .and_then(|e| e.get("type"))
        .and_then(Value::as_str)
        != Some("block_event")
    {
        return None;
    }
    let block = wrapper["event"].get("payload")?;
    if block.get("block_id").and_then(Value::as_str) != Some(&endpoint.block_id) {
        return None;
    }
    if block.get("creator_name").and_then(Value::as_str) != Some(TERMINAL_CREATOR) {
        return None;
    }
    block.get("name").and_then(Value::as_str)
}

pub fn parse_event_lines(
    endpoint: &RexEndpoint,
    lines: impl IntoIterator<Item = String>,
) -> RexEventBatch {
    let mut events = Vec::new();
    let mut gap_possible = false;
    let mut detail = None;
    for line in lines {
        match parse_event_line(endpoint, &line) {
            RexEventLine::Matched(event) => events.push(event),
            RexEventLine::Ignored => {}
            RexEventLine::Malformed { detail: message } => {
                gap_possible = true;
                detail = Some(message);
                break;
            }
        }
    }
    RexEventBatch {
        events,
        gap_possible,
        detail,
    }
}

pub fn spawn_events(endpoint: RexEndpoint, on_event: mpsc::Sender<RexEvent>) -> RexEventStream {
    let (cancel, mut cancel_signal) = watch::channel(false);
    let task = tokio::spawn(async move {
        let deadline = match default_timeout() {
            Ok(timeout) => Instant::now() + timeout,
            Err(error) => {
                return RexEventBatch {
                    events: Vec::new(),
                    gap_possible: true,
                    detail: Some(error.to_string()),
                }
            }
        };
        let verified = tokio::select! {
            _ = cancel_signal.changed() => return RexEventBatch { events: Vec::new(), gap_possible: true, detail: Some("cancelled before verifying the bound service".into()) },
            result = verify_service_until(&endpoint, deadline) => result,
        };
        if let Err(error) = verified {
            return RexEventBatch {
                events: Vec::new(),
                gap_possible: true,
                detail: Some(error.to_string()),
            };
        }
        if Instant::now() >= deadline {
            return RexEventBatch {
                events: Vec::new(),
                gap_possible: true,
                detail: Some("events setup deadline elapsed before spawning the CLI".into()),
            };
        }
        let cli_path = official_cli_path();
        let argv = [
            "--server",
            endpoint.server.as_str(),
            "--autostart=false",
            "--timeout",
            "30s",
            "events",
            "-s",
            endpoint.session_id.as_str(),
            "--json",
        ];
        let mut child = match Command::new(&cli_path)
            .args(argv)
            .env_clear()
            .envs(
                std::env::vars_os().filter(|(key, _)| !key.as_encoded_bytes().starts_with(b"REX_")),
            )
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn()
        {
            Ok(child) => child,
            Err(error) => {
                return RexEventBatch {
                    events: Vec::new(),
                    gap_possible: true,
                    detail: Some(format!("events CLI did not start: {error}")),
                };
            }
        };
        let stderr = child.stderr.take().expect("events stderr captured");
        let mut stderr_reader = OwnedReader {
            task: tokio::spawn(drain_stdout(stderr, StdoutCapture::All)),
        };
        let stdout = child.stdout.take().expect("events stdout captured");
        let mut lines = BufReader::new(stdout).lines();
        let mut batch = RexEventBatch {
            events: Vec::new(),
            gap_possible: false,
            detail: None,
        };
        loop {
            let cancelled = tokio::select! {
            changed = cancel_signal.changed() => changed.is_err() || *cancel_signal.borrow(),
                        line = lines.next_line() => match line {
                            Ok(Some(line)) => {
                                match parse_event_line(&endpoint, &line) {
                                    RexEventLine::Matched(event) => {
                                        let sent = tokio::select! {
                                            changed = cancel_signal.changed() => {
                                                if changed.is_err() || *cancel_signal.borrow() {
                                                    break;
                                                }
                                                continue;
                                            }
                                            sent = on_event.send(event) => sent,
                                        };
                                        if sent.is_err() {
                                            batch.gap_possible = true;
                                            batch.detail = Some("event receiver closed".into());
                                            break;
                                        }
                                    }
                                    RexEventLine::Ignored => {}
                                    RexEventLine::Malformed { detail: message } => {
                                        batch.gap_possible = true;
                                        batch.detail = Some(message);
                                        break;
                                    }
                                }
                                false
                            }
                            Ok(None) => {
                                batch.gap_possible = true;
                                batch.detail = Some("events stream disconnected".into());
                                break;
                            }
                            Err(error) => {
                                batch.gap_possible = true;
                                batch.detail = Some(format!("events stream read failed: {error}"));
                                break;
                            }
                        },
                    };
            if cancelled {
                break;
            }
        }
        let child_cleanup = kill_and_reap(&mut child, deadline).await;
        let reader_cleanup = finish_reader(&mut stderr_reader, deadline).await;
        for (stage, result) in [("child", child_cleanup), ("stderr", reader_cleanup)] {
            if let Err(error) = result {
                batch.gap_possible = true;
                let detail = batch.detail.get_or_insert_with(String::new);
                if !detail.is_empty() {
                    detail.push_str("; ");
                }
                detail.push_str(&format!("{stage} cleanup: {error}"));
            }
        }
        batch
    });
    RexEventStream { cancel, task }
}

#[cfg(test)]
#[path = "rex/tests.rs"]
mod tests;
