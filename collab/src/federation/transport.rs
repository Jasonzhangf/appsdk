use super::{FederationConfig, FederationPeer};
use crate::proto::{FederationEnvelope, FederationReq, FederationResponse, FEDERATION_PROTOCOL};
use std::ffi::OsString;
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::process::{Child, Command};
use tokio::time::{timeout_at, Instant};

static NEXT_FORWARD: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FederationTransportStage {
    BeforeRequest,
    Request,
}

#[derive(Debug, Clone)]
pub struct FederationTransportError {
    pub stage: FederationTransportStage,
    pub detail: String,
}

impl std::fmt::Display for FederationTransportError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "MACHINE_UNREACHABLE: {:?}: {}",
            self.stage, self.detail
        )
    }
}

impl std::error::Error for FederationTransportError {}

#[derive(Debug)]
pub struct FederationCallReceipt {
    pub response: FederationResponse,
    pub cleanup_error: Option<String>,
}

pub struct FederationClient;

impl FederationClient {
    pub async fn call(
        configuration: &FederationConfig,
        peer_name: &str,
        request: &FederationReq,
        timeout: Duration,
    ) -> Result<FederationCallReceipt, FederationTransportError> {
        if timeout.is_zero() {
            return Err(FederationTransportError {
                stage: FederationTransportStage::BeforeRequest,
                detail: "FEDERATION_DEADLINE_INVALID".into(),
            });
        }
        call_until(configuration, peer_name, request, Instant::now() + timeout).await
    }
}

pub(crate) async fn call_until(
    configuration: &FederationConfig,
    peer_name: &str,
    request: &FederationReq,
    deadline: Instant,
) -> Result<FederationCallReceipt, FederationTransportError> {
    call_with_program_until(
        configuration,
        peer_name,
        request,
        deadline,
        Path::new("/usr/bin/ssh"),
    )
    .await
}

async fn call_with_program_until(
    configuration: &FederationConfig,
    peer_name: &str,
    request: &FederationReq,
    deadline: Instant,
    program: &Path,
) -> Result<FederationCallReceipt, FederationTransportError> {
    let before = |detail| FederationTransportError {
        stage: FederationTransportStage::BeforeRequest,
        detail,
    };
    if Instant::now() >= deadline {
        return Err(before(
            "federation request deadline elapsed before forward start".into(),
        ));
    }
    configuration.validate().map_err(before)?;
    let (target, peer) = configuration.peer(peer_name).map_err(before)?;
    if !peer.allow_outbound {
        return Err(before(format!(
            "MACHINE_NOT_ALLOWED: outbound communication to {target} is disabled"
        )));
    }
    let credential =
        super::auth::read_credential(&peer.outbound_credential_file).map_err(before)?;
    if Instant::now() >= deadline {
        return Err(before(
            "federation request deadline elapsed before SSH forward start".into(),
        ));
    }
    let mut forward = OwnedForward::start(peer, program).map_err(before)?;
    let mut stage = FederationTransportStage::BeforeRequest;
    let completed = timeout_at(deadline, async {
        forward.wait_ready(deadline).await?;
        request_on_socket(
            &forward.socket,
            target,
            &credential,
            request,
            &mut stage,
            deadline,
        )
        .await
    })
    .await;
    let outcome = match completed {
        Ok(Ok(response)) => Ok(response),
        Ok(Err(detail)) => Err(FederationTransportError { stage, detail }),
        Err(_) => Err(FederationTransportError {
            stage,
            detail: "federation request deadline elapsed".into(),
        }),
    };
    let cleanup = forward.finish(deadline).await;
    match outcome {
        Ok(response) => Ok(FederationCallReceipt {
            response,
            cleanup_error: cleanup.error,
        }),
        Err(mut error) => {
            if !cleanup.stderr.trim().is_empty() {
                error
                    .detail
                    .push_str(&format!("; SSH: {}", cleanup.stderr.trim()));
            }
            if let Some(cleanup_error) = cleanup.error {
                error
                    .detail
                    .push_str(&format!("; cleanup: {cleanup_error}"));
            }
            Err(error)
        }
    }
}

pub(super) fn ssh_arguments(peer: &FederationPeer, local_socket: &Path) -> Vec<OsString> {
    let mut arguments = vec![
        "-T".into(),
        "-N".into(),
        "-o".into(),
        "BatchMode=yes".into(),
        "-o".into(),
        "ExitOnForwardFailure=yes".into(),
        "-o".into(),
        "ControlPath=none".into(),
        "-o".into(),
        "ForkAfterAuthentication=no".into(),
        "-o".into(),
        "StreamLocalBindUnlink=no".into(),
        "-o".into(),
        "StreamLocalBindMask=0177".into(),
        "-L".into(),
        format!(
            "{}:{}",
            local_socket.display(),
            peer.remote_socket.display()
        )
        .into(),
    ];
    if let Some(path) = &peer.ssh_config_file {
        arguments.extend([OsString::from("-F"), path.as_os_str().to_owned()]);
    }
    arguments.extend([OsString::from("--"), OsString::from(&peer.ssh_host)]);
    arguments
}

pub(super) async fn request_on_socket(
    socket: &Path,
    target: &str,
    credential: &str,
    request: &FederationReq,
    stage: &mut FederationTransportStage,
    deadline: Instant,
) -> Result<FederationResponse, String> {
    let hello = exchange(socket, target, credential, &FederationReq::Hello, deadline).await?;
    match &hello {
        FederationResponse::Hello { machine, protocol }
            if machine.eq_ignore_ascii_case(target) && protocol == FEDERATION_PROTOCOL => {}
        FederationResponse::Error { code, detail } => {
            return Err(format!("FEDERATION_HANDSHAKE_REJECTED: {code}: {detail}"));
        }
        _ => return Err("MACHINE_IDENTITY_MISMATCH: peer handshake does not match the declared machine and protocol".into()),
    }
    if matches!(request, FederationReq::Hello) {
        return Ok(hello);
    }
    if Instant::now() >= deadline {
        return Err("federation request deadline elapsed".into());
    }
    *stage = FederationTransportStage::Request;
    exchange(socket, target, credential, request, deadline).await
}

async fn exchange(
    socket: &Path,
    target: &str,
    credential: &str,
    request: &FederationReq,
    deadline: Instant,
) -> Result<FederationResponse, String> {
    if Instant::now() >= deadline {
        return Err("federation request deadline elapsed".into());
    }
    let exchange = async {
        let mut stream = UnixStream::connect(socket)
            .await
            .map_err(|error| format!("federation connect failed: {error}"))?;
        let envelope = FederationEnvelope {
            target: target.to_owned(),
            credential: credential.to_owned(),
            request: request.clone(),
        };
        let mut encoded = serde_json::to_vec(&envelope)
            .map_err(|_| "federation request serialization failed".to_owned())?;
        encoded.push(b'\n');
        stream
            .write_all(&encoded)
            .await
            .map_err(|error| format!("federation request write failed: {error}"))?;
        let mut response = String::new();
        let length = BufReader::new(stream)
            .read_line(&mut response)
            .await
            .map_err(|error| format!("federation response read failed: {error}"))?;
        if length == 0 {
            return Err("federation response ended before a receipt".into());
        }
        serde_json::from_str(&response)
            .map_err(|_| "FEDERATION_PROTOCOL_INVALID: malformed response".into())
    };
    match timeout_at(deadline, exchange).await {
        Ok(result) => result,
        Err(_) => Err("federation request deadline elapsed".into()),
    }
}

pub(super) async fn call_with_program(
    configuration: &FederationConfig,
    peer_name: &str,
    request: &FederationReq,
    timeout: Duration,
    program: &Path,
) -> Result<FederationCallReceipt, FederationTransportError> {
    if timeout.is_zero() {
        return Err(FederationTransportError {
            stage: FederationTransportStage::BeforeRequest,
            detail: "FEDERATION_DEADLINE_INVALID".into(),
        });
    }
    call_with_program_until(
        configuration,
        peer_name,
        request,
        Instant::now() + timeout,
        program,
    )
    .await
}

struct OwnedForward {
    child: Child,
    directory: PathBuf,
    socket: PathBuf,
    stderr: Option<tokio::task::JoinHandle<Result<String, std::io::Error>>>,
}

impl OwnedForward {
    fn start(peer: &FederationPeer, program: &Path) -> Result<Self, String> {
        let directory = std::env::temp_dir().join(format!(
            "collab-federation-{}-{}",
            std::process::id(),
            NEXT_FORWARD.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&directory)
            .map_err(|error| format!("private forward directory failed: {error}"))?;
        let socket = directory.join("rpc.sock");
        let child = Command::new(program)
            .args(ssh_arguments(peer, &socket))
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn();
        let mut child = match child {
            Ok(child) => child,
            Err(error) => {
                let _ = remove_owned_directory(&directory, &socket);
                return Err(format!("SSH forward did not start: {error}"));
            }
        };
        let mut stderr = child.stderr.take().expect("SSH stderr captured");
        let stderr = tokio::spawn(async move {
            let mut text = String::new();
            stderr.read_to_string(&mut text).await?;
            Ok(text)
        });
        Ok(Self {
            child,
            directory,
            socket,
            stderr: Some(stderr),
        })
    }

    async fn wait_ready(&mut self, deadline: Instant) -> Result<(), String> {
        loop {
            if let Some(status) = self
                .child
                .try_wait()
                .map_err(|error| format!("SSH forward status failed: {error}"))?
            {
                return Err(format!("SSH forward exited with {status}"));
            }
            if self.socket.exists() {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(
                    "federation request deadline elapsed before SSH forward became ready".into(),
                );
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    async fn finish(&mut self, deadline: Instant) -> ForwardCleanup {
        let mut errors = Vec::new();
        let mut reaped = match self.child.try_wait() {
            Ok(Some(_)) => true,
            Ok(None) => {
                if let Err(error) = self.child.start_kill() {
                    errors.push(format!("child kill failed: {error}"));
                }
                false
            }
            Err(error) => {
                errors.push(format!("child status failed: {error}"));
                if let Err(kill_error) = self.child.start_kill() {
                    errors.push(format!("child kill failed: {kill_error}"));
                }
                false
            }
        };
        if !reaped && Instant::now() < deadline {
            match timeout_at(deadline, self.child.wait()).await {
                Ok(Ok(_)) => reaped = true,
                Ok(Err(error)) => errors.push(format!("owned SSH process wait failed: {error}")),
                Err(_) => {}
            }
        }
        if !reaped {
            match self.child.try_wait() {
                Ok(Some(_)) => {}
                Ok(None) => errors.push("child_reap_unconfirmed".into()),
                Err(error) => errors.push(format!("child_reap_unconfirmed: {error}")),
            }
        }
        let stderr = match self.stderr.as_mut() {
            Some(task) => {
                let mut text = String::new();
                if Instant::now() < deadline {
                    match timeout_at(deadline, &mut *task).await {
                        Ok(Ok(Ok(reader))) => text = reader,
                        Ok(Ok(Err(error))) => {
                            errors.push(format!("SSH stderr read failed: {error}"))
                        }
                        Ok(Err(error)) => errors.push(format!("SSH stderr task failed: {error}")),
                        Err(_) => {
                            task.abort();
                            if Instant::now() >= deadline
                                || timeout_at(deadline, &mut *task).await.is_err()
                            {
                                errors.push("stderr_reader_closure_unconfirmed".into());
                            }
                        }
                    }
                } else {
                    task.abort();
                    errors.push("stderr_reader_closure_unconfirmed".into());
                }
                text
            }
            None => String::new(),
        };
        self.stderr.take();
        if let Err(error) = remove_owned_directory(&self.directory, &self.socket) {
            errors.push(error);
        }
        ForwardCleanup {
            stderr,
            error: (!errors.is_empty()).then(|| errors.join("; ")),
        }
    }
}

struct ForwardCleanup {
    stderr: String,
    error: Option<String>,
}

impl Drop for OwnedForward {
    fn drop(&mut self) {
        let _ = self.child.start_kill();
        if let Some(stderr) = &self.stderr {
            stderr.abort();
        }
        let _ = remove_owned_directory(&self.directory, &self.socket);
    }
}

fn remove_owned_directory(directory: &Path, socket: &Path) -> Result<(), String> {
    let mut errors = Vec::new();
    for path in [socket, directory] {
        let result = if path == directory {
            std::fs::remove_dir(path)
        } else {
            std::fs::remove_file(path)
        };
        if let Err(error) = result {
            if error.kind() != std::io::ErrorKind::NotFound {
                errors.push(format!("owned forward path cleanup failed: {error}"));
            }
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("; "))
    }
}

#[cfg(test)]
mod cancellation_tests {
    use super::*;
    use std::sync::Arc;
    use tokio::sync::{oneshot, Notify};

    struct ReaderCompletion {
        finished: Arc<Notify>,
    }

    impl Drop for ReaderCompletion {
        fn drop(&mut self) {
            self.finished.notify_one();
        }
    }

    #[tokio::test]
    async fn cancellation_during_stderr_cleanup_keeps_reader_owned() {
        let directory = std::env::temp_dir().join(format!(
            "collab-forward-cancel-{}-{}",
            std::process::id(),
            NEXT_FORWARD.fetch_add(1, Ordering::Relaxed),
        ));
        std::fs::create_dir(&directory).unwrap();
        let child = Command::new("/bin/sleep")
            .arg("30")
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let pid = child.id().unwrap() as libc::pid_t;
        let finished = Arc::new(Notify::new());
        let completion = finished.clone();
        let (started, reader_started) = oneshot::channel();
        let stderr = tokio::spawn(async move {
            let _completion = ReaderCompletion {
                finished: completion,
            };
            started.send(()).unwrap();
            std::future::pending::<Result<String, std::io::Error>>().await
        });
        let cleanup_reader = stderr.abort_handle();
        reader_started.await.unwrap();
        let mut forward = OwnedForward {
            child,
            socket: directory.join("rpc.sock"),
            directory: directory.clone(),
            stderr: Some(stderr),
        };
        let operation = tokio::spawn(async move {
            forward
                .finish(Instant::now() + Duration::from_secs(5))
                .await;
        });
        tokio::time::timeout(Duration::from_secs(2), async {
            while unsafe { libc::kill(pid, 0) } == 0 {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("owned child reaped before waiting for stderr");
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(!operation.is_finished());
        operation.abort();
        let _ = operation.await;
        let closed = tokio::time::timeout(Duration::from_millis(300), finished.notified()).await;
        cleanup_reader.abort();
        assert!(
            closed.is_ok(),
            "cancellation detached the pending stderr reader"
        );
        assert!(!directory.exists());
    }
}
