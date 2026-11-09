include!("rex_public_fixture.rs");

use std::io::{BufRead, BufReader};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixListener;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::thread::JoinHandle;

const SSH_ALIAS: &str = "collab-federation-loopback";
const HOST_KEY_PATH: &str = "/etc/ssh/ssh_host_ed25519_key.pub";

/// Keep one test-owned descendant's stderr open after the SSH child exits.
/// The original private SSH configuration is restored before another call.
struct OwnedSshStderrHolder {
    configuration: PathBuf,
    original: String,
    pid_file: PathBuf,
}

impl OwnedSshStderrHolder {
    fn start(fixture: &FederationPublicFixture) -> Self {
        let original = fs::read_to_string(&fixture.ssh_config).unwrap();
        let script = fixture.root.join("hold-ssh-stderr.py");
        let pid_file = fixture.root.join("held-stderr.pid");
        fs::write(
            &script,
            "import os, pathlib, sys, time\npid = os.fork()\nif pid == 0:\n    time.sleep(60)\n    os._exit(0)\npathlib.Path(sys.argv[1]).write_text(str(pid))\n",
        )
        .unwrap();
        fs::write(
            &fixture.ssh_config,
            format!(
                "{original}  PermitLocalCommand yes\n  LocalCommand /usr/bin/python3 '{}' '{}'\n",
                script.display(),
                pid_file.display(),
            ),
        )
        .unwrap();
        Self {
            configuration: fixture.ssh_config.clone(),
            original,
            pid_file,
        }
    }
}

impl Drop for OwnedSshStderrHolder {
    fn drop(&mut self) {
        if let Err(error) = fs::write(&self.configuration, &self.original) {
            eprintln!("restore private SSH fixture configuration failed: {error}");
        }
        let pid = fs::read_to_string(&self.pid_file)
            .ok()
            .and_then(|pid| pid.trim().parse::<u32>().ok());
        if let Some(pid) = pid {
            let result = Command::new("/bin/kill")
                .args(["-TERM", &pid.to_string()])
                .output();
            if let Err(error) = result {
                eprintln!("stop owned stderr-holder PID {pid} failed: {error}");
            }
        }
    }
}

/// A test-owned Unix socket proxy. It forwards to the real target daemon and
/// then drops Put replies or delays operation replies. Product code is unchanged.
pub struct FederationFaultProxy {
    socket: PathBuf,
    stop: Arc<AtomicBool>,
    join: Option<JoinHandle<()>>,
}

#[derive(Clone, Copy)]
enum FederationFaultMode {
    DropPut,
    DelayOperations(Duration),
}

impl FederationFaultProxy {
    fn start(socket: PathBuf, target: PathBuf) -> Self {
        Self::start_with_mode(socket, target, FederationFaultMode::DropPut)
    }

    fn start_with_mode(socket: PathBuf, target: PathBuf, mode: FederationFaultMode) -> Self {
        let listener = UnixListener::bind(&socket).expect("bind test-owned federation proxy");
        listener
            .set_nonblocking(true)
            .expect("set test-owned federation proxy nonblocking");
        fs::set_permissions(&socket, fs::Permissions::from_mode(0o600))
            .expect("restrict test-owned federation proxy socket");
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = stop.clone();
        let join = std::thread::spawn(move || {
            while !thread_stop.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        if let Err(error) = proxy_federation_connection(stream, &target, mode) {
                            eprintln!("test federation proxy connection failed: {error}");
                        }
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(10));
                    }
                    Err(error) => {
                        eprintln!("test federation proxy accept failed: {error}");
                        break;
                    }
                }
            }
        });
        Self {
            socket,
            stop,
            join: Some(join),
        }
    }
}

fn proxy_federation_connection(
    stream: UnixStream,
    target: &Path,
    mode: FederationFaultMode,
) -> std::io::Result<()> {
    stream.set_nonblocking(false)?;
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    stream.set_write_timeout(Some(Duration::from_secs(5)))?;
    let request_line = {
        let mut reader = BufReader::new(&stream);
        let mut line = String::new();
        reader.read_line(&mut line)?;
        line
    };
    if request_line.is_empty() {
        return Ok(());
    }
    let kind = serde_json::from_str::<Value>(&request_line)
        .ok()
        .and_then(|value| value["request"]["kind"].as_str().map(str::to_owned));
    let mut upstream = UnixStream::connect(target)?;
    upstream.set_nonblocking(false)?;
    upstream.set_read_timeout(Some(Duration::from_secs(5)))?;
    upstream.set_write_timeout(Some(Duration::from_secs(5)))?;
    upstream.write_all(request_line.as_bytes())?;
    let response_line = {
        let mut reader = BufReader::new(&upstream);
        let mut line = String::new();
        reader.read_line(&mut line)?;
        line
    };
    if let FederationFaultMode::DelayOperations(delay) = mode {
        if matches!(kind.as_deref(), Some("lookup" | "put")) {
            std::thread::sleep(delay);
        }
    }
    let drop_response =
        matches!(mode, FederationFaultMode::DropPut) && kind.as_deref() == Some("put");
    if !drop_response && !response_line.is_empty() {
        (&stream).write_all(response_line.as_bytes())?;
    }
    Ok(())
}

impl Drop for FederationFaultProxy {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
        let _ = fs::remove_file(&self.socket);
    }
}

/// Two independent Collab daemons, each with its own HOME, state root, project
/// root and agent session. The only shared transport is a task-private
/// loopback SSH Unix forward. Production SSH configuration is never written.
pub struct FederationPublicFixture {
    pub alpha: RexLiveFixture,
    pub beta: RexLiveFixture,
    pub root: PathBuf,
    pub ssh_config: PathBuf,
    pub known_hosts: PathBuf,
    pub alpha_to_beta_credential: PathBuf,
    pub beta_to_alpha_credential: PathBuf,
    proxy: Option<FederationFaultProxy>,
    configured: bool,
}

impl FederationPublicFixture {
    pub fn new(label: &str, with_fault_proxy: bool) -> Self {
        assert_candidate_binary();
        let root = unique_root(&format!("federation-public-{label}"));
        fs::create_dir_all(&root).expect("create federation fixture root");
        let alpha = RexLiveFixture::new(&format!("federation-alpha-{label}"));
        let beta = RexLiveFixture::new(&format!("federation-beta-{label}"));
        let ssh_config = root.join("ssh_config");
        let known_hosts = root.join("known_hosts");
        write_private_ssh_config(&ssh_config, &known_hosts);
        let proxy = with_fault_proxy.then(|| {
            FederationFaultProxy::start(
                root.join("fault-proxy.sock"),
                beta.host_state.join("federation").join("rpc.sock"),
            )
        });
        Self {
            alpha,
            beta,
            root: root.clone(),
            ssh_config,
            known_hosts,
            alpha_to_beta_credential: root.join("credential-alpha-to-beta"),
            beta_to_alpha_credential: root.join("credential-beta-to-alpha"),
            proxy,
            configured: false,
        }
    }

    pub fn configure(&mut self) {
        assert!(!self.configured, "federation fixture is already configured");
        self.probe_ssh();
        let alpha_init = self
            .alpha
            .run_ok(&["machine", "init", "--name", "alpha"], None);
        let beta_init = self
            .beta
            .run_ok(&["machine", "init", "--name", "beta"], None);
        self.alpha.evidence("machine_init", alpha_init.clone());
        self.beta.evidence("machine_init", beta_init.clone());

        let alpha_credential = self.alpha.run_ok(
            &[
                "machine",
                "credential-create",
                "--path",
                self.alpha_to_beta_credential
                    .to_str()
                    .expect("credential path is UTF-8"),
            ],
            None,
        );
        let beta_credential = self.beta.run_ok(
            &[
                "machine",
                "credential-create",
                "--path",
                self.beta_to_alpha_credential
                    .to_str()
                    .expect("credential path is UTF-8"),
            ],
            None,
        );
        self.alpha.evidence("credential_created", alpha_credential);
        self.beta.evidence("credential_created", beta_credential);

        let alpha_socket = self.alpha.host_state.join("federation").join("rpc.sock");
        let beta_socket = self.beta.host_state.join("federation").join("rpc.sock");
        let alpha_remote_socket = self
            .proxy
            .as_ref()
            .map(|proxy| proxy.socket.as_path())
            .unwrap_or(beta_socket.as_path());
        let alpha_peer = self.alpha.run_ok(
            &[
                "machine",
                "peer-add",
                "beta",
                "--ssh-host",
                SSH_ALIAS,
                "--ssh-config-file",
                self.ssh_config.to_str().expect("SSH config path is UTF-8"),
                "--remote-socket",
                alpha_remote_socket
                    .to_str()
                    .expect("remote socket path is UTF-8"),
                "--outbound-credential-file",
                self.alpha_to_beta_credential
                    .to_str()
                    .expect("credential path is UTF-8"),
                "--inbound-credential-file",
                self.beta_to_alpha_credential
                    .to_str()
                    .expect("credential path is UTF-8"),
                "--allow-inbound",
                "--allow-outbound",
            ],
            None,
        );
        let beta_peer = self.beta.run_ok(
            &[
                "machine",
                "peer-add",
                "alpha",
                "--ssh-host",
                SSH_ALIAS,
                "--ssh-config-file",
                self.ssh_config.to_str().expect("SSH config path is UTF-8"),
                "--remote-socket",
                alpha_socket.to_str().expect("remote socket path is UTF-8"),
                "--outbound-credential-file",
                self.beta_to_alpha_credential
                    .to_str()
                    .expect("credential path is UTF-8"),
                "--inbound-credential-file",
                self.alpha_to_beta_credential
                    .to_str()
                    .expect("credential path is UTF-8"),
                "--allow-inbound",
                "--allow-outbound",
            ],
            None,
        );
        self.alpha.evidence("machine_peer_added", alpha_peer);
        self.beta.evidence("machine_peer_added", beta_peer);

        let alpha_status = self.alpha.run_ok(&["machine", "status"], None);
        let beta_status = self.beta.run_ok(&["machine", "status"], None);
        assert_eq!(alpha_status["machine"], "alpha", "{alpha_status}");
        assert_eq!(alpha_status["peers"], json!(["beta"]), "{alpha_status}");
        assert_eq!(beta_status["machine"], "beta", "{beta_status}");
        assert_eq!(beta_status["peers"], json!(["alpha"]), "{beta_status}");
        self.alpha.evidence("machine_status", alpha_status);
        self.beta.evidence("machine_status", beta_status);
        self.configured = true;
    }

    pub fn start(&mut self) {
        assert!(
            self.configured,
            "configure the federation fixture before starting daemons"
        );
        self.alpha.start_daemon();
        self.beta.start_daemon();
    }

    pub fn finish(&mut self) -> Vec<String> {
        let mut errors = self.alpha.finish();
        errors.extend(self.beta.finish());
        errors
    }

    pub fn daemon_logs(&self) -> String {
        format!(
            "alpha daemon log:\n{}\nbeta daemon log:\n{}",
            fs::read_to_string(self.alpha.root.join("test-daemon.log")).unwrap_or_default(),
            fs::read_to_string(self.beta.root.join("test-daemon.log")).unwrap_or_default()
        )
    }

    fn probe_ssh(&self) -> Value {
        let output = Command::new("/usr/bin/ssh")
            .args([
                "-F",
                self.ssh_config.to_str().expect("SSH config path is UTF-8"),
                SSH_ALIAS,
                "printf",
                "FEDERATION_SSH_OK",
            ])
            .env_clear()
            .env("LC_ALL", "en_US.UTF-8")
            .output()
            .expect("run task-private SSH probe");
        assert!(
            output.status.success(),
            "task-private SSH probe failed: stdout={} stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let evidence = json!({
            "stdout": String::from_utf8_lossy(&output.stdout),
            "stderr": String::from_utf8_lossy(&output.stderr),
        });
        self.alpha.evidence("ssh_probe", evidence.clone());
        evidence
    }
}

fn assert_candidate_binary() {
    let binary = std::env::var_os("COLLAB_TEST_BINARY")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            panic!(
                "COLLAB_TEST_BINARY must explicitly name the current federation candidate binary"
            )
        });
    assert!(
        binary.is_file(),
        "COLLAB_TEST_BINARY {} is not a file",
        binary.display()
    );
}

fn write_private_ssh_config(config: &Path, known_hosts: &Path) {
    let identity = std::env::var_os("COLLAB_TEST_SSH_IDENTITY")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".ssh").join("id_rsa"))
        })
        .expect("COLLAB_TEST_SSH_IDENTITY or HOME must provide an SSH identity");
    assert!(
        identity.is_file(),
        "task-private SSH identity {} is missing; set COLLAB_TEST_SSH_IDENTITY",
        identity.display()
    );
    let host = std::env::var("COLLAB_TEST_SSH_HOST").unwrap_or_else(|_| "127.0.0.1".into());
    let port = std::env::var("COLLAB_TEST_SSH_PORT").unwrap_or_else(|_| "22".into());
    let user = std::env::var("USER")
        .or_else(|_| std::env::var("LOGNAME"))
        .expect("USER or LOGNAME must name the loopback SSH account");
    let host_key =
        fs::read_to_string(HOST_KEY_PATH).expect("read the local SSH ed25519 host public key");
    let mut fields = host_key.split_whitespace();
    let key_type = fields.next().expect("local SSH host key has a key type");
    let key = fields.next().expect("local SSH host key has key material");
    let known_host_name = if port == "22" {
        host.clone()
    } else {
        format!("[{host}]:{port}")
    };
    fs::write(known_hosts, format!("{known_host_name} {key_type} {key}\n"))
        .expect("write task-private SSH known_hosts");
    fs::write(
        config,
        format!(
            "Host {SSH_ALIAS}\n  HostName {host}\n  Port {port}\n  User {user}\n  IdentityFile \"{}\"\n  IdentitiesOnly yes\n  StrictHostKeyChecking yes\n  UserKnownHostsFile \"{}\"\n  BatchMode yes\n",
            identity.display(),
            known_hosts.display()
        ),
    )
    .expect("write task-private SSH config");
    fs::set_permissions(config, fs::Permissions::from_mode(0o600))
        .expect("restrict task-private SSH config");
    fs::set_permissions(known_hosts, fs::Permissions::from_mode(0o600))
        .expect("restrict task-private SSH known_hosts");
}
