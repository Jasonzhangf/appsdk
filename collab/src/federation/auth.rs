use super::FederationConfig;
use std::io::Read;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthenticatedMachine {
    pub source: String,
    pub target: String,
}

pub fn read_credential(path: &Path) -> Result<String, String> {
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
        .map_err(|error| format!("FEDERATION_CREDENTIAL_READ_FAILED: {error}"))?;
    let metadata = file
        .metadata()
        .map_err(|error| format!("FEDERATION_CREDENTIAL_READ_FAILED: {error}"))?;
    if !metadata.is_file()
        || metadata.mode() & 0o077 != 0
        || metadata.uid() != unsafe { libc::geteuid() }
    {
        return Err("FEDERATION_CREDENTIAL_PERMISSIONS: credential must be a private regular file owned by this user".into());
    }
    let mut credential = String::new();
    file.read_to_string(&mut credential)
        .map_err(|error| format!("FEDERATION_CREDENTIAL_READ_FAILED: {error}"))?;
    let credential = credential.trim_end_matches(['\r', '\n']);
    if credential.is_empty() || credential.chars().any(char::is_control) {
        return Err(
            "FEDERATION_CREDENTIAL_INVALID: credential is empty or contains control characters"
                .into(),
        );
    }
    Ok(credential.to_owned())
}

pub fn authenticate(
    configuration: &FederationConfig,
    credential: &str,
) -> Result<AuthenticatedMachine, String> {
    configuration.validate()?;
    if credential.is_empty() {
        return Err("FEDERATION_AUTH_FAILED".into());
    }
    let mut source = None;
    let mut unavailable = Vec::new();
    for (name, peer) in &configuration.peers {
        if !peer.allow_inbound {
            continue;
        }
        let expected = match read_credential(&peer.inbound_credential_file) {
            Ok(expected) => expected,
            Err(error) => {
                unavailable.push(format!("{name}: {error}"));
                continue;
            }
        };
        if credential == expected {
            if source.is_some() {
                return Err(
                    "FEDERATION_AUTH_AMBIGUOUS: credential is assigned to multiple source machines"
                        .into(),
                );
            }
            source = Some(name.clone());
        }
    }
    if let Some(source) = source {
        return Ok(AuthenticatedMachine {
            source,
            target: configuration.machine.clone(),
        });
    }
    if !unavailable.is_empty() {
        return Err(format!(
            "FEDERATION_AUTH_UNAVAILABLE: {}",
            unavailable.join("; ")
        ));
    }
    Err("FEDERATION_AUTH_FAILED".into())
}
