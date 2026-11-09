use crate::config;
use crate::federation::management;
use anyhow::Result;
use clap::Subcommand;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::time::Duration;

const CHECK_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Subcommand)]
pub enum MachineCommand {
    /// Show this host's federation declaration summary
    Status,
    /// List declared peer machines without exposing credentials
    List,
    /// Initialize or update this machine's federation name
    Init {
        #[arg(long)]
        name: String,
    },
    /// Add or replace one directed peer declaration
    PeerAdd {
        name: String,
        #[arg(long)]
        ssh_host: String,
        #[arg(long)]
        ssh_config_file: Option<PathBuf>,
        #[arg(long)]
        remote_socket: PathBuf,
        #[arg(long)]
        outbound_credential_file: PathBuf,
        #[arg(long)]
        inbound_credential_file: PathBuf,
        #[arg(long)]
        allow_inbound: bool,
        #[arg(long)]
        allow_outbound: bool,
    },
    /// Remove one peer declaration
    PeerRemove { name: String },
    /// Verify one running peer with an authenticated FederationClient Hello
    Check { name: String },
    /// Create one new 0600 directional credential without printing its secret
    CredentialCreate {
        #[arg(long)]
        path: PathBuf,
    },
}

pub fn execute(command: MachineCommand) -> Result<Value> {
    execute_at(&config::path()?, command)
}

fn execute_at(config_path: &Path, command: MachineCommand) -> Result<Value> {
    match command {
        MachineCommand::Status => management::status(config_path),
        MachineCommand::List => management::list(config_path),
        MachineCommand::Init { name } => management::initialize(config_path, &name),
        MachineCommand::PeerAdd {
            name,
            ssh_host,
            ssh_config_file,
            remote_socket,
            outbound_credential_file,
            inbound_credential_file,
            allow_inbound,
            allow_outbound,
        } => management::add_peer(
            config_path,
            &name,
            &ssh_host,
            &remote_socket,
            &outbound_credential_file,
            &inbound_credential_file,
            allow_inbound,
            allow_outbound,
            ssh_config_file.as_deref(),
        ),
        MachineCommand::PeerRemove { name } => management::remove_peer(config_path, &name),
        MachineCommand::Check { name } => {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()?;
            runtime.block_on(management::check_peer(config_path, &name, CHECK_TIMEOUT))
        }
        MachineCommand::CredentialCreate { path } => management::create_credential(&path),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);

    struct Fixture(PathBuf);

    impl Fixture {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "collab-machine-cli-{}-{}",
                std::process::id(),
                NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
            Self(path)
        }

        fn config(&self) -> PathBuf {
            self.0.join("config.toml")
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }

    #[test]
    fn status_and_list_are_file_free_until_a_mutation_is_explicit() {
        let fixture = Fixture::new();
        assert_eq!(
            execute_at(&fixture.config(), MachineCommand::Status).unwrap()["configured"],
            false
        );
        assert_eq!(
            execute_at(&fixture.config(), MachineCommand::List).unwrap()["peers"],
            json!([])
        );
        assert!(!fixture.config().exists());
        execute_at(
            &fixture.config(),
            MachineCommand::Init {
                name: "studio".into(),
            },
        )
        .unwrap();
        assert!(fixture.config().exists());
    }

    #[test]
    fn peer_management_outputs_configuration_without_credential_contents() {
        let fixture = Fixture::new();
        execute_at(
            &fixture.config(),
            MachineCommand::Init {
                name: "studio".into(),
            },
        )
        .unwrap();
        let outbound = fixture.0.join("air-out");
        let inbound = fixture.0.join("air-in");
        fs::write(&outbound, "outbound-secret").unwrap();
        fs::write(&inbound, "inbound-secret").unwrap();
        fs::set_permissions(&outbound, fs::Permissions::from_mode(0o600)).unwrap();
        fs::set_permissions(&inbound, fs::Permissions::from_mode(0o600)).unwrap();
        let added = execute_at(
            &fixture.config(),
            MachineCommand::PeerAdd {
                name: "air".into(),
                ssh_host: "air.example".into(),
                ssh_config_file: None,
                remote_socket: "/private/collab-federation.sock".into(),
                outbound_credential_file: outbound,
                inbound_credential_file: inbound,
                allow_inbound: true,
                allow_outbound: true,
            },
        )
        .unwrap();
        let listed = execute_at(&fixture.config(), MachineCommand::List).unwrap();
        assert_eq!(added["peer"]["allow_outbound"], true);
        assert_eq!(listed["peers"][0]["name"], "air");
        assert!(!added.to_string().contains("outbound-secret"));
        assert!(!listed.to_string().contains("inbound-secret"));
    }
}
