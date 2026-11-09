use super::{append_log, remove_listener_socket, ProjectRuntimeManager};
use crate::federation::{server, FederationConfig};
use crate::proto::FederationResponse;
use std::os::unix::fs::FileTypeExt;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::{UnixListener, UnixStream};

/// This resource belongs to the daemon that already holds the host writer lock.
pub(super) struct Ingress {
    listener: UnixListener,
    path: PathBuf,
    metadata: std::fs::Metadata,
    configuration: FederationConfig,
}

impl Ingress {
    pub(super) fn open(path: PathBuf, configuration: FederationConfig) -> Result<Self, String> {
        configuration.validate()?;
        match std::fs::symlink_metadata(&path) {
            Ok(metadata) => {
                if !metadata.file_type().is_socket() {
                    return Err(
                        "FEDERATION_INGRESS_FAILED: ingress path is occupied by a non-socket"
                            .into(),
                    );
                }
                match std::os::unix::net::UnixStream::connect(&path) {
                    Ok(_) => {
                        return Err(
                            "FEDERATION_INGRESS_ACTIVE: an ingress listener is already running"
                                .into(),
                        )
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::ConnectionRefused => {
                        remove_listener_socket(&path, &metadata)
                            .map_err(|error| format!("FEDERATION_INGRESS_FAILED: {error}"))?;
                    }
                    Err(error) => {
                        return Err(format!(
                        "FEDERATION_INGRESS_FAILED: cannot classify the existing socket: {error}"
                    ))
                    }
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(format!("FEDERATION_INGRESS_FAILED: {error}")),
        }
        let listener = server::bind_ingress(&path)?;
        let metadata = std::fs::symlink_metadata(&path)
            .map_err(|error| format!("FEDERATION_INGRESS_FAILED: {error}"))?;
        Ok(Self {
            listener,
            path,
            metadata,
            configuration,
        })
    }

    pub(super) async fn accept(&self) -> std::io::Result<UnixStream> {
        self.listener.accept().await.map(|(stream, _)| stream)
    }

    pub(super) fn configuration(&self) -> FederationConfig {
        self.configuration.clone()
    }
}

impl Drop for Ingress {
    fn drop(&mut self) {
        if let Err(error) = remove_listener_socket(&self.path, &self.metadata) {
            eprintln!("FEDERATION_INGRESS_CLEANUP_FAILED: {error}");
        }
    }
}

pub(super) async fn connection(
    manager: Arc<ProjectRuntimeManager>,
    mut stream: UnixStream,
    configuration: FederationConfig,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
) {
    let deadline = Duration::from_secs(30);
    let operation = async {
        let response =
            match server::read_authenticated_request(&mut stream, &configuration, deadline).await {
                Ok((source, request)) => {
                    super::federation_runtime::ingress(manager.clone(), source, request).await
                }
                Err(detail) => FederationResponse::Error {
                    code: "FEDERATION_ADMISSION_FAILED".into(),
                    detail,
                },
            };
        if let Err(error) = server::write_response(&mut stream, &response, deadline).await {
            append_log(
                &manager.host.log_path(),
                &format!("FEDERATION_RESPONSE_FAILED: {error}"),
            );
        }
    };
    tokio::select! {
        _ = shutdown.changed() => {}
        _ = operation => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT: AtomicU64 = AtomicU64::new(0);

    #[tokio::test]
    async fn ingress_owns_its_socket_and_recovers_only_a_refused_socket() {
        let root = std::env::temp_dir().join(format!(
            "collab-federation-listener-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let socket = root.join("rpc.sock");
        let configuration = FederationConfig {
            machine: "studio".into(),
            peers: Default::default(),
        };
        let ingress = Ingress::open(socket.clone(), configuration.clone()).unwrap();
        assert_eq!(std::fs::metadata(&socket).unwrap().mode() & 0o777, 0o600);
        assert!(Ingress::open(socket.clone(), configuration.clone()).is_err());
        assert!(socket.exists());
        drop(ingress);
        assert!(!socket.exists());
        let stale = std::os::unix::net::UnixListener::bind(&socket).unwrap();
        drop(stale);
        let ingress = Ingress::open(socket.clone(), configuration.clone()).unwrap();
        drop(ingress);
        std::fs::write(&socket, "another owner's file").unwrap();
        assert!(Ingress::open(socket.clone(), configuration).is_err());
        assert_eq!(
            std::fs::read_to_string(&socket).unwrap(),
            "another owner's file"
        );
        std::fs::remove_file(socket).unwrap();
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
        std::fs::remove_dir(root).unwrap();
    }

    #[tokio::test]
    async fn ingress_cleanup_keeps_a_replaced_socket() {
        let root = std::env::temp_dir().join(format!(
            "collab-federation-replaced-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let socket = root.join("rpc.sock");
        let configuration = FederationConfig {
            machine: "studio".into(),
            peers: Default::default(),
        };
        let ingress = Ingress::open(socket.clone(), configuration).unwrap();
        std::fs::remove_file(&socket).unwrap();
        let replacement = server::bind_ingress(&socket).unwrap();
        let replacement_inode = std::fs::metadata(&socket).unwrap().ino();
        drop(ingress);
        assert_eq!(std::fs::metadata(&socket).unwrap().ino(), replacement_inode);
        drop(replacement);
        std::fs::remove_file(socket).unwrap();
        std::fs::remove_dir(root).unwrap();
    }
}
