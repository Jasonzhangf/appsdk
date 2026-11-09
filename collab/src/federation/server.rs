use super::auth::AuthenticatedMachine;
use super::FederationConfig;
use crate::proto::{FederationEnvelope, FederationReq, FederationResponse};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
use std::path::Path;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};

pub fn bind_ingress(socket: &Path) -> Result<UnixListener, String> {
    let parent = socket
        .parent()
        .ok_or_else(|| "FEDERATION_INGRESS_FAILED: socket parent is required".to_owned())?;
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(parent)
        .map_err(|error| format!("FEDERATION_INGRESS_FAILED: {error}"))?;
    let metadata = parent
        .metadata()
        .map_err(|error| format!("FEDERATION_INGRESS_FAILED: {error}"))?;
    if metadata.mode() & 0o077 != 0 || metadata.uid() != unsafe { libc::geteuid() } {
        return Err("FEDERATION_INGRESS_PERMISSIONS: socket directory must be private and owned by this user".into());
    }
    let listener = UnixListener::bind(socket)
        .map_err(|error| format!("FEDERATION_INGRESS_FAILED: {error}"))?;
    if let Err(error) = std::fs::set_permissions(socket, std::fs::Permissions::from_mode(0o600)) {
        drop(listener);
        let _ = std::fs::remove_file(socket);
        return Err(format!("FEDERATION_INGRESS_FAILED: {error}"));
    }
    Ok(listener)
}

pub async fn read_authenticated_request(
    stream: &mut UnixStream,
    configuration: &FederationConfig,
    deadline: Duration,
) -> Result<(AuthenticatedMachine, FederationReq), String> {
    if deadline.is_zero() {
        return Err("FEDERATION_DEADLINE_INVALID".into());
    }
    let mut line = String::new();
    let read = tokio::time::timeout(deadline, BufReader::new(stream).read_line(&mut line))
        .await
        .map_err(|_| "FEDERATION_PROTOCOL_TIMEOUT".to_owned())?
        .map_err(|error| format!("FEDERATION_PROTOCOL_IO: {error}"))?;
    if read == 0 {
        return Err("FEDERATION_PROTOCOL_EOF".into());
    }
    let envelope: FederationEnvelope = serde_json::from_str(&line)
        .map_err(|_| "FEDERATION_PROTOCOL_INVALID: expected a federation envelope".to_owned())?;
    let source = super::auth::authenticate(configuration, &envelope.credential)?;
    if !envelope.target.eq_ignore_ascii_case(&configuration.machine) {
        return Err("MACHINE_IDENTITY_MISMATCH: request targets another machine".into());
    }
    Ok((source, envelope.request))
}

pub async fn write_response(
    stream: &mut UnixStream,
    response: &FederationResponse,
    deadline: Duration,
) -> Result<(), String> {
    if deadline.is_zero() {
        return Err("FEDERATION_DEADLINE_INVALID".into());
    }
    let mut encoded = serde_json::to_vec(response)
        .map_err(|_| "FEDERATION_PROTOCOL_INVALID: response serialization failed".to_owned())?;
    encoded.push(b'\n');
    tokio::time::timeout(deadline, stream.write_all(&encoded))
        .await
        .map_err(|_| "FEDERATION_PROTOCOL_TIMEOUT".to_owned())?
        .map_err(|error| format!("FEDERATION_PROTOCOL_IO: {error}"))
}
