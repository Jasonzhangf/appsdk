use super::*;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(0);

struct Fixture(PathBuf);

struct OwnedHolder(libc::pid_t);

impl Drop for OwnedHolder {
    fn drop(&mut self) {
        unsafe {
            libc::kill(self.0, libc::SIGKILL);
        }
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while std::time::Instant::now() < deadline {
            if unsafe { libc::kill(self.0, 0) } == -1
                && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
            {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        eprintln!(
            "FEDERATION_TEST_HOLDER_CLEANUP_UNCONFIRMED: pid {} still visible",
            self.0
        );
    }
}

impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "collab-federation-auth-{}-{}",
            std::process::id(),
            NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
        Self(path)
    }

    fn secret(&self, name: &str, value: &str) -> PathBuf {
        let path = self.0.join(name);
        std::fs::write(&path, value).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        path
    }

    fn configuration(&self) -> FederationConfig {
        let inbound = self.secret("air-in", "air-to-studio-test-credential");
        let outbound = self.secret("air-out", "studio-to-air-test-credential");
        FederationConfig {
            machine: "studio".into(),
            peers: [(
                "air".into(),
                FederationPeer {
                    ssh_host: "air".into(),
                    ssh_config_file: None,
                    remote_socket: "/private/collab-federation.sock".into(),
                    allow_inbound: true,
                    allow_outbound: true,
                    inbound_credential_file: inbound,
                    outbound_credential_file: outbound,
                },
            )]
            .into(),
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}

#[test]
fn missing_federation_configuration_preserves_local_mode() {
    assert_eq!(config::from_toml("").unwrap(), None);
}

#[test]
fn configuration_rejects_ambiguous_names_and_ssh_option_injection() {
    let fixture = Fixture::new();
    let mut configuration = fixture.configuration();
    configuration.validate().unwrap();
    configuration.peers.get_mut("air").unwrap().ssh_host = "-oProxyCommand=unexpected".into();
    assert!(configuration.validate().is_err());
    configuration = fixture.configuration();
    let peer = configuration.peers["air"].clone();
    configuration.peers.insert("AIR".into(), peer);
    assert!(configuration.validate().is_err());
}

#[test]
fn project_override_cannot_create_machine_trust() {
    assert!(config::from_toml(
        "[[projects]]\nroot = '/project'\n[projects.federation]\nmachine = 'other'"
    )
    .is_err());
}

#[test]
fn directional_credential_fixes_the_authenticated_source_machine() {
    let fixture = Fixture::new();
    let configuration = fixture.configuration();
    assert_eq!(
        auth::authenticate(&configuration, "air-to-studio-test-credential").unwrap(),
        auth::AuthenticatedMachine {
            source: "air".into(),
            target: "studio".into()
        }
    );
    assert!(auth::authenticate(&configuration, "studio-to-air-test-credential").is_err());
    assert!(auth::authenticate(&configuration, "wrong").is_err());
}

#[test]
fn duplicate_ingress_credential_and_public_secret_file_are_rejected() {
    let fixture = Fixture::new();
    let mut configuration = fixture.configuration();
    configuration
        .peers
        .insert("coder1".into(), configuration.peers["air"].clone());
    assert!(
        auth::authenticate(&configuration, "air-to-studio-test-credential")
            .unwrap_err()
            .starts_with("FEDERATION_AUTH_AMBIGUOUS")
    );
    let secret = &configuration.peers["air"].inbound_credential_file;
    std::fs::set_permissions(secret, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert!(auth::read_credential(secret).is_err());
}

#[test]
fn an_unavailable_unrelated_credential_does_not_block_a_valid_source() {
    let fixture = Fixture::new();
    let mut configuration = fixture.configuration();
    let mut peer = configuration.peers["air"].clone();
    peer.inbound_credential_file = fixture.0.join("not-installed");
    configuration.peers.insert("coder1".into(), peer);
    assert_eq!(
        auth::authenticate(&configuration, "air-to-studio-test-credential")
            .unwrap()
            .source,
        "air"
    );
}

#[tokio::test]
async fn ingress_authentication_rejects_a_wrong_destination() {
    use tokio::io::AsyncWriteExt;
    let fixture = Fixture::new();
    let configuration = fixture.configuration();
    let (mut client, mut server_socket) = tokio::net::UnixStream::pair().unwrap();
    let envelope = crate::proto::FederationEnvelope {
        target: "other-machine".into(),
        credential: "air-to-studio-test-credential".into(),
        request: crate::proto::FederationReq::Hello,
    };
    client
        .write_all(&serde_json::to_vec(&envelope).unwrap())
        .await
        .unwrap();
    client.write_all(b"\n").await.unwrap();
    let error = server::read_authenticated_request(
        &mut server_socket,
        &configuration,
        std::time::Duration::from_secs(1),
    )
    .await
    .unwrap_err();
    assert!(error.starts_with("MACHINE_IDENTITY_MISMATCH"), "{error}");
}

#[tokio::test]
async fn ingress_derives_source_from_credentials_and_preserves_the_request() {
    use tokio::io::AsyncWriteExt;
    let fixture = Fixture::new();
    let configuration = fixture.configuration();
    let (mut client, mut server_socket) = tokio::net::UnixStream::pair().unwrap();
    let envelope = crate::proto::FederationEnvelope {
        target: "studio".into(),
        credential: "air-to-studio-test-credential".into(),
        request: crate::proto::FederationReq::Directory {
            project: Some("项目".into()),
        },
    };
    client
        .write_all(&serde_json::to_vec(&envelope).unwrap())
        .await
        .unwrap();
    client.write_all(b"\n").await.unwrap();
    let (source, request) = server::read_authenticated_request(
        &mut server_socket,
        &configuration,
        std::time::Duration::from_secs(1),
    )
    .await
    .unwrap();
    assert_eq!(source.source, "air");
    assert_eq!(request, envelope.request);
}

#[tokio::test]
async fn private_ingress_keeps_an_existing_listener_and_restricts_socket_permissions() {
    let fixture = Fixture::new();
    let socket = fixture.0.join("federation.sock");
    let listener = server::bind_ingress(&socket).unwrap();
    assert_eq!(
        std::fs::metadata(&socket).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert!(server::bind_ingress(&socket).is_err());
    let _client = tokio::net::UnixStream::connect(&socket).await.unwrap();
    let _connection = listener.accept().await.unwrap();
}

#[tokio::test]
async fn federation_ingress_rejects_local_control_operations() {
    use tokio::io::AsyncWriteExt;
    let fixture = Fixture::new();
    let configuration = fixture.configuration();
    let (mut client, mut server_socket) = tokio::net::UnixStream::pair().unwrap();
    client.write_all(b"{\"target\":\"studio\",\"credential\":\"air-to-studio-test-credential\",\"request\":{\"op\":\"Poll\"}}\n").await.unwrap();
    let error = server::read_authenticated_request(
        &mut server_socket,
        &configuration,
        std::time::Duration::from_secs(1),
    )
    .await
    .unwrap_err();
    assert!(error.starts_with("FEDERATION_PROTOCOL_INVALID"));
}

#[test]
fn ssh_command_uses_an_explicit_private_socket_and_no_remote_shell() {
    let fixture = Fixture::new();
    let configuration = fixture.configuration();
    let socket = fixture.0.join("connection.sock");
    let arguments = transport::ssh_arguments(&configuration.peers["air"], &socket);
    assert!(arguments.contains(&"-N".into()));
    assert!(arguments.contains(&"BatchMode=yes".into()));
    assert!(arguments.contains(&"ExitOnForwardFailure=yes".into()));
    assert!(arguments.contains(&"ControlPath=none".into()));
    assert!(arguments.contains(&"ForkAfterAuthentication=no".into()));
    let expected = format!("{}:/private/collab-federation.sock", socket.display());
    assert!(arguments.contains(&expected.into()));
    assert_eq!(arguments.last().unwrap(), "air");
    assert_eq!(arguments[arguments.len() - 2], "--");
    let mut peer = configuration.peers["air"].clone();
    peer.ssh_config_file = Some(fixture.0.join("isolated ssh config"));
    let arguments = transport::ssh_arguments(&peer, &socket);
    let option = arguments
        .iter()
        .position(|argument| argument == "-F")
        .unwrap();
    assert_eq!(
        arguments[option + 1],
        peer.ssh_config_file.unwrap().as_os_str()
    );
}

#[tokio::test]
async fn client_checks_the_machine_handshake_before_sending_an_operation() {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    let fixture = Fixture::new();
    let socket = fixture.0.join("peer.sock");
    let listener = server::bind_ingress(&socket).unwrap();
    let peer = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut stream = BufReader::new(stream);
        let mut frame = String::new();
        stream.read_line(&mut frame).await.unwrap();
        let envelope: crate::proto::FederationEnvelope = serde_json::from_str(&frame).unwrap();
        assert_eq!(envelope.request, crate::proto::FederationReq::Hello);
        let response = crate::proto::FederationResponse::Hello {
            machine: "wrong-machine".into(),
            protocol: crate::proto::FEDERATION_PROTOCOL.into(),
        };
        let mut response = serde_json::to_vec(&response).unwrap();
        response.push(b'\n');
        stream.get_mut().write_all(&response).await.unwrap();
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(100), listener.accept())
                .await
                .is_err()
        );
    });
    let mut stage = transport::FederationTransportStage::BeforeRequest;
    let error = transport::request_on_socket(
        &socket,
        "air",
        "credential",
        &crate::proto::FederationReq::Directory { project: None },
        &mut stage,
        tokio::time::Instant::now() + std::time::Duration::from_secs(5),
    )
    .await
    .unwrap_err();
    assert!(error.starts_with("MACHINE_IDENTITY_MISMATCH"), "{error}");
    assert_eq!(stage, transport::FederationTransportStage::BeforeRequest);
    peer.await.unwrap();
}

#[tokio::test]
async fn client_returns_the_response_after_a_verified_machine_handshake() {
    use crate::proto::{
        FederationEnvelope, FederationReq, FederationResponse, FEDERATION_PROTOCOL,
    };
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    let fixture = Fixture::new();
    let socket = fixture.0.join("peer.sock");
    let listener = server::bind_ingress(&socket).unwrap();
    let peer = tokio::spawn(async move {
        for request in [
            FederationReq::Hello,
            FederationReq::Directory {
                project: Some("appsdk".into()),
            },
        ] {
            let (stream, _) = listener.accept().await.unwrap();
            let mut stream = BufReader::new(stream);
            let mut frame = String::new();
            stream.read_line(&mut frame).await.unwrap();
            let envelope: FederationEnvelope = serde_json::from_str(&frame).unwrap();
            assert_eq!(envelope.target, "air");
            assert_eq!(envelope.credential, "test-only-credential");
            assert_eq!(envelope.request, request);
            let response = match request {
                FederationReq::Hello => FederationResponse::Hello {
                    machine: "air".into(),
                    protocol: FEDERATION_PROTOCOL.into(),
                },
                _ => FederationResponse::Directory {
                    targets: Vec::new(),
                },
            };
            let mut encoded = serde_json::to_vec(&response).unwrap();
            encoded.push(b'\n');
            stream.get_mut().write_all(&encoded).await.unwrap();
        }
    });
    let mut stage = transport::FederationTransportStage::BeforeRequest;
    let response = transport::request_on_socket(
        &socket,
        "air",
        "test-only-credential",
        &FederationReq::Directory {
            project: Some("appsdk".into()),
        },
        &mut stage,
        tokio::time::Instant::now() + std::time::Duration::from_secs(5),
    )
    .await
    .unwrap();
    assert_eq!(
        response,
        FederationResponse::Directory {
            targets: Vec::new()
        }
    );
    assert_eq!(stage, transport::FederationTransportStage::Request);
    peer.await.unwrap();
}

#[tokio::test]
async fn response_loss_after_submission_has_an_uncertain_request_stage() {
    use crate::proto::{FederationReq, FederationResponse, FEDERATION_PROTOCOL};
    use tokio::io::{AsyncBufReadExt, BufReader};
    let fixture = Fixture::new();
    let socket = fixture.0.join("peer.sock");
    let listener = server::bind_ingress(&socket).unwrap();
    let peer = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut frame = String::new();
        BufReader::new(&mut stream)
            .read_line(&mut frame)
            .await
            .unwrap();
        server::write_response(
            &mut stream,
            &FederationResponse::Hello {
                machine: "air".into(),
                protocol: FEDERATION_PROTOCOL.into(),
            },
            std::time::Duration::from_secs(1),
        )
        .await
        .unwrap();
        drop(stream);
        let (stream, _) = listener.accept().await.unwrap();
        let mut frame = String::new();
        BufReader::new(stream).read_line(&mut frame).await.unwrap();
        assert!(frame.contains("directory"));
    });
    let mut stage = transport::FederationTransportStage::BeforeRequest;
    let error = transport::request_on_socket(
        &socket,
        "air",
        "test-only-credential",
        &FederationReq::Directory { project: None },
        &mut stage,
        tokio::time::Instant::now() + std::time::Duration::from_secs(5),
    )
    .await
    .unwrap_err();
    assert!(error.contains("before a receipt"), "{error}");
    assert_eq!(stage, transport::FederationTransportStage::Request);
    peer.await.unwrap();
}

#[tokio::test]
async fn an_owned_ssh_forward_is_reaped_and_its_socket_removed() {
    let fixture = Fixture::new();
    let configuration = fixture.configuration();
    let executable = fixture.0.join("ssh-fixture.py");
    let record = fixture.0.join("child.json");
    let source = format!(
        r#"#!/usr/bin/python3
import json, os, socket, sys, time
forward = sys.argv[sys.argv.index('-L') + 1]
local = forward.split(':', 1)[0]
listener = socket.socket(socket.AF_UNIX)
pending = local + '.pending'
listener.bind(pending)
listener.listen(4)
os.rename(pending, local)
with open({record:?}, 'w') as output:
    json.dump({{'pid': os.getpid(), 'socket': local}}, output)
for request in range(2):
    connection, _ = listener.accept()
    frame = connection.makefile('rb').readline()
    envelope = json.loads(frame)
    if request == 0:
        assert envelope['request']['kind'] == 'hello'
        response = {{'kind': 'hello', 'value': {{'machine': 'air', 'protocol': 'collab-federation/v1'}}}}
    else:
        assert envelope['request']['kind'] == 'directory'
        response = {{'kind': 'directory', 'value': {{'targets': []}}}}
    connection.sendall(json.dumps(response).encode() + b'\n')
    connection.close()
time.sleep(60)
"#,
        record = record.to_str().unwrap()
    );
    std::fs::write(&executable, source).unwrap();
    std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
    let receipt = transport::call_with_program(
        &configuration,
        "air",
        &crate::proto::FederationReq::Directory { project: None },
        std::time::Duration::from_secs(5),
        &executable,
    )
    .await
    .unwrap();
    assert!(
        receipt.cleanup_error.is_none(),
        "{:?}",
        receipt.cleanup_error
    );
    assert!(matches!(
        receipt.response,
        crate::proto::FederationResponse::Directory { .. }
    ));
    let child: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&record).unwrap_or_else(|error| {
            panic!("fixture record missing: {error}; transport result: {receipt:?}")
        }))
        .unwrap();
    let pid = child["pid"].as_i64().unwrap() as libc::pid_t;
    assert_eq!(
        unsafe { libc::kill(pid, 0) },
        -1,
        "owned SSH child must be reaped"
    );
    assert!(!std::path::Path::new(child["socket"].as_str().unwrap()).exists());
}

#[tokio::test]
async fn transport_cleanup_does_not_outlive_the_request_deadline() {
    let fixture = Fixture::new();
    let configuration = fixture.configuration();
    let executable = fixture.0.join("ssh-delayed-stderr.py");
    let record = fixture.0.join("child.json");
    let source = format!(
        r#"#!/usr/bin/python3
import json, os, socket, subprocess, sys
if '-L' not in sys.argv:
    sys.exit(0)
forward = sys.argv[sys.argv.index('-L') + 1]
local = forward.split(':', 1)[0]
listener = socket.socket(socket.AF_UNIX)
listener.bind(local + '.pending')
listener.listen(4)
holder = subprocess.Popen(['/bin/sleep', '60']).pid
with open({record:?}, 'w') as output:
    json.dump({{'pid': os.getpid(), 'holder': holder, 'socket': local}}, output)
os.rename(local + '.pending', local)
for request in range(2):
    connection, _ = listener.accept()
    frame = connection.makefile('rb').readline()
    envelope = json.loads(frame)
    if request == 0:
        assert envelope['request']['kind'] == 'hello'
        response = {{'kind': 'hello', 'value': {{'machine': 'air', 'protocol': 'collab-federation/v1'}}}}
    else:
        assert envelope['request']['kind'] == 'directory'
        response = {{'kind': 'directory', 'value': {{'targets': []}}}}
    connection.sendall(json.dumps(response).encode() + b'\n')
    connection.close()
os._exit(0)
"#,
        record = record.to_str().unwrap()
    );
    std::fs::write(&executable, source).unwrap();
    std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
    let _ = std::process::Command::new(&executable)
        .arg("--warmup")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();

    let started = std::time::Instant::now();
    let receipt = transport::call_with_program(
        &configuration,
        "air",
        &crate::proto::FederationReq::Directory { project: None },
        std::time::Duration::from_millis(600),
        &executable,
    )
    .await;
    let elapsed = started.elapsed();

    let child: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&record).unwrap_or_else(|error| {
            panic!("fixture record missing: {error}; transport result: {receipt:?}")
        }))
        .unwrap();
    let holder: libc::pid_t = child["holder"].as_i64().unwrap() as libc::pid_t;
    let _holder = OwnedHolder(holder);

    assert!(
        elapsed < std::time::Duration::from_millis(1500),
        "cleanup exceeded the request budget: {elapsed:?}"
    );
    match receipt {
        Ok(receipt) => {
            assert!(matches!(
                receipt.response,
                crate::proto::FederationResponse::Directory { .. }
            ));
            assert!(
                receipt
                    .cleanup_error
                    .as_deref()
                    .is_some_and(|error| error.contains("stderr_reader_closure_unconfirmed")),
                "cleanup evidence must name the unconfirmed stderr reader: {:?}",
                receipt.cleanup_error
            );
        }
        Err(error) => panic!("authoritative response was discarded: {error}"),
    }
}
