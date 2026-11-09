use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationConfig {
    pub machine: String,
    #[serde(default)]
    pub peers: BTreeMap<String, FederationPeer>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationPeer {
    pub ssh_host: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ssh_config_file: Option<PathBuf>,
    pub remote_socket: PathBuf,
    #[serde(default)]
    pub allow_outbound: bool,
    #[serde(default)]
    pub allow_inbound: bool,
    pub outbound_credential_file: PathBuf,
    pub inbound_credential_file: PathBuf,
}

impl FederationConfig {
    pub fn validate(&self) -> Result<(), String> {
        crate::address::validate_machine_name(&self.machine).map_err(|error| error.to_string())?;
        let mut names = std::collections::BTreeSet::new();
        for (name, peer) in &self.peers {
            crate::address::validate_machine_name(name).map_err(|error| error.to_string())?;
            if name.eq_ignore_ascii_case(&self.machine) || !names.insert(name.to_ascii_lowercase())
            {
                return Err(format!(
                    "FEDERATION_CONFIG_INVALID: ambiguous machine {name}"
                ));
            }
            if peer.ssh_host.is_empty()
                || peer.ssh_host.starts_with('-')
                || peer
                    .ssh_host
                    .chars()
                    .any(|character| character.is_whitespace() || character.is_control())
            {
                return Err(format!(
                    "FEDERATION_CONFIG_INVALID: invalid SSH host for {name}"
                ));
            }
            if peer
                .ssh_config_file
                .as_ref()
                .is_some_and(|path| !path.is_absolute())
            {
                return Err(format!(
                    "FEDERATION_CONFIG_INVALID: SSH config path for {name} must be absolute"
                ));
            }
            let socket = peer
                .remote_socket
                .to_str()
                .ok_or_else(|| "FEDERATION_CONFIG_INVALID: socket must be UTF-8".to_owned())?;
            if !peer.remote_socket.is_absolute()
                || socket.contains(':')
                || socket.chars().any(char::is_control)
            {
                return Err(format!(
                    "FEDERATION_CONFIG_INVALID: invalid private socket for {name}"
                ));
            }
            if !peer.outbound_credential_file.is_absolute()
                || !peer.inbound_credential_file.is_absolute()
            {
                return Err(format!(
                    "FEDERATION_CONFIG_INVALID: credential paths for {name} must be absolute"
                ));
            }
        }
        Ok(())
    }

    pub fn peer(&self, name: &str) -> Result<(&str, &FederationPeer), String> {
        self.peers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(key, peer)| (key.as_str(), peer))
            .ok_or_else(|| format!("MACHINE_UNKNOWN: machine {name} is not declared"))
    }
}

pub fn ensure_host_only(projects: Option<&toml::Value>) -> Result<(), String> {
    if let Some(projects) = projects {
        let entries = projects
            .as_array()
            .ok_or_else(|| "projects must be an array of tables".to_owned())?;
        if entries
            .iter()
            .any(|project| project.get("federation").is_some())
        {
            return Err("federation is global-only; project overrides are not supported".into());
        }
    }
    Ok(())
}

pub fn from_toml(text: &str) -> Result<Option<FederationConfig>, String> {
    let document: toml::Value =
        toml::from_str(text).map_err(|_| "FEDERATION_CONFIG_INVALID: malformed TOML".to_owned())?;
    ensure_host_only(document.get("projects"))?;
    let Some(section) = document.get("federation") else {
        return Ok(None);
    };
    let configuration: FederationConfig = section
        .clone()
        .try_into()
        .map_err(|error| format!("FEDERATION_CONFIG_INVALID: {error}"))?;
    configuration.validate()?;
    Ok(Some(configuration))
}

pub fn read_from_path(path: &Path) -> Result<Option<FederationConfig>, String> {
    match std::fs::read_to_string(path) {
        Ok(text) => from_toml(&text),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!("FEDERATION_CONFIG_READ_FAILED: {error}")),
    }
}
