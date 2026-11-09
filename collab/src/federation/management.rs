use super::config::read_from_path;
use super::{FederationConfig, FederationPeer};
use anyhow::{bail, Context, Result};
use rand::RngCore;
use serde_json::{json, Value};
use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::os::fd::AsRawFd;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use toml_edit::{value, DocumentMut, Item, Table};

static NEXT_TEMPORARY: AtomicU64 = AtomicU64::new(0);

pub fn status(config_path: &Path) -> Result<Value> {
    let configuration = read_configuration(config_path)?;
    Ok(match configuration {
        Some(configuration) => json!({
            "configured": true,
            "machine": configuration.machine,
            "peers": configuration.peers.keys().collect::<Vec<_>>(),
        }),
        None => json!({"configured": false}),
    })
}

pub fn list(config_path: &Path) -> Result<Value> {
    let configuration = read_configuration(config_path)?;
    Ok(match configuration {
        Some(configuration) => json!({
            "configured": true,
            "machine": configuration.machine,
            "peers": configuration
                .peers
                .iter()
                .map(|(name, peer)| peer_json(name, peer))
                .collect::<Vec<_>>(),
        }),
        None => json!({"configured": false, "peers": []}),
    })
}

pub fn initialize(config_path: &Path, machine: &str) -> Result<Value> {
    let configuration = update_configuration(config_path, |document| {
        let federation = federation_table_mut(document)?;
        federation.insert("machine", value(machine.to_owned()));
        Ok(())
    })?;
    Ok(json!({
        "configured": true,
        "machine": configuration.machine
    }))
}

pub fn add_peer(
    config_path: &Path,
    name: &str,
    ssh_host: &str,
    remote_socket: &Path,
    outbound_credential_file: &Path,
    inbound_credential_file: &Path,
    allow_inbound: bool,
    allow_outbound: bool,
    ssh_config_file: Option<&Path>,
) -> Result<Value> {
    let remote_socket = toml_path(remote_socket, "remote socket")?;
    let outbound_credential_file = toml_path(outbound_credential_file, "outbound credential file")?;
    let inbound_credential_file = toml_path(inbound_credential_file, "inbound credential file")?;
    let ssh_config_file = ssh_config_file
        .map(|path| toml_path(path, "SSH config file"))
        .transpose()?;
    let configuration = update_configuration(config_path, |document| {
        let federation = federation_table_mut(document)?;
        let peers = table_entry_mut(federation, "peers", "federation.peers")?;
        let existing = peers
            .iter()
            .find_map(|(key, _)| key.eq_ignore_ascii_case(name).then(|| key.to_owned()));
        if let Some(existing) = existing {
            peers.remove(&existing);
        }
        let mut peer = Table::new();
        peer.insert("ssh_host", value(ssh_host.to_owned()));
        if let Some(path) = ssh_config_file.as_ref() {
            peer.insert("ssh_config_file", value(path.clone()));
        }
        peer.insert("remote_socket", value(remote_socket.clone()));
        peer.insert("allow_outbound", value(allow_outbound));
        peer.insert("allow_inbound", value(allow_inbound));
        peer.insert(
            "outbound_credential_file",
            value(outbound_credential_file.clone()),
        );
        peer.insert(
            "inbound_credential_file",
            value(inbound_credential_file.clone()),
        );
        peers.insert(name, Item::Table(peer));
        Ok(())
    })?;
    let (canonical_name, peer) = configuration.peer(name).map_err(anyhow::Error::msg)?;
    Ok(json!({
        "configured": true,
        "machine": configuration.machine,
        "peer": peer_json(canonical_name, peer),
    }))
}

pub fn remove_peer(config_path: &Path, name: &str) -> Result<Value> {
    let configuration = update_configuration(config_path, |document| {
        let federation = federation_table_mut(document)?;
        let peers = federation
            .get_mut("peers")
            .context("FEDERATION_PEERS_INVALID: federation.peers must be a table")?
            .as_table_mut()
            .context("FEDERATION_PEERS_INVALID: federation.peers must be a table")?;
        let existing = peers
            .iter()
            .find_map(|(key, _)| key.eq_ignore_ascii_case(name).then(|| key.to_owned()))
            .ok_or_else(|| anyhow::anyhow!("MACHINE_UNKNOWN: machine {name} is not declared"))?;
        peers.remove(&existing);
        if peers.is_empty() {
            federation.remove("peers");
        }
        Ok(())
    })?;
    Ok(json!({
        "configured": true,
        "machine": configuration.machine,
        "removed": name
    }))
}

pub async fn check_peer(
    config_path: &Path,
    name: &str,
    timeout: std::time::Duration,
) -> Result<Value> {
    let configuration = read_configuration(config_path)?
        .context("MACHINE_NOT_CONFIGURED: federation configuration is unavailable")?;
    let (declared_name, _) = configuration.peer(name).map_err(anyhow::Error::msg)?;
    let response = super::FederationClient::call(
        &configuration,
        declared_name,
        &crate::proto::FederationReq::Hello,
        timeout,
    )
    .await?;
    match response.response {
        crate::proto::FederationResponse::Hello { protocol, .. } => {
            let mut result = json!({
            "reachable": true,
            "machine": declared_name,
            "protocol": protocol,
            });
            if let Some(error) = response.cleanup_error {
                result["transport_error"] = json!(error);
            }
            Ok(result)
        }
        _ => bail!("MACHINE_IDENTITY_MISMATCH: peer did not return a federation Hello response"),
    }
}

pub fn create_credential(path: &Path) -> Result<Value> {
    if !path.is_absolute() {
        bail!("FEDERATION_CREDENTIAL_PATH_INVALID: credential path must be absolute");
    }
    toml_path(path, "credential path")?;
    let parent = path
        .parent()
        .context("FEDERATION_CREDENTIAL_PATH_INVALID: credential path has no parent")?;
    fs::create_dir_all(parent)
        .with_context(|| format!("create credential directory {}", parent.display()))?;
    let mut file = match OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
    {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            bail!(
                "FEDERATION_CREDENTIAL_EXISTS: refusing to overwrite {}",
                path.display()
            )
        }
        Err(error) => {
            return Err(error)
                .with_context(|| format!("create credential file {}", path.display()));
        }
    };
    if let Err(error) = fs::set_permissions(path, fs::Permissions::from_mode(0o600)) {
        drop(file);
        let _ = fs::remove_file(path);
        return Err(error).with_context(|| format!("restrict credential file {}", path.display()));
    }
    let mut random = [0_u8; 32];
    rand::thread_rng().fill_bytes(&mut random);
    let secret = random
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    if let Err(error) = file
        .write_all(secret.as_bytes())
        .and_then(|()| file.sync_all())
    {
        drop(file);
        let _ = fs::remove_file(path);
        return Err(error).with_context(|| format!("write credential file {}", path.display()));
    }
    Ok(json!({"path": path.to_string_lossy()}))
}

fn read_configuration(path: &Path) -> Result<Option<FederationConfig>> {
    read_from_path(path).map_err(anyhow::Error::msg)
}

fn update_configuration<F>(path: &Path, update: F) -> Result<FederationConfig>
where
    F: FnOnce(&mut DocumentMut) -> Result<()>,
{
    let parent = path
        .parent()
        .context("FEDERATION_CONFIG_PATH_INVALID: config path has no parent")?;
    fs::create_dir_all(parent)
        .with_context(|| format!("create config directory {}", parent.display()))?;
    let _lock = ConfigLock::acquire(path)?;
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(error).with_context(|| format!("read {}", path.display())),
    };
    let mut document = text
        .parse::<DocumentMut>()
        .with_context(|| format!("invalid TOML in {}", path.display()))?;
    update(&mut document)?;
    let updated = document.to_string();
    let parsed = crate::config::parse(&updated, Path::new("/"))
        .context("FEDERATION_CONFIG_INVALID: updated config failed validation")?;
    let configuration = parsed
        .federation
        .context("FEDERATION_CONFIG_INVALID: federation section is missing")?;
    atomic_replace(path, updated.as_bytes())?;
    Ok(configuration)
}

fn federation_table_mut(document: &mut DocumentMut) -> Result<&mut Table> {
    if document.get("federation").is_none() {
        document.insert("federation", Item::Table(Table::new()));
    }
    document
        .get_mut("federation")
        .expect("federation table was inserted")
        .as_table_mut()
        .context("FEDERATION_CONFIG_INVALID: federation must be a table")
}

fn table_entry_mut<'a>(table: &'a mut Table, key: &str, label: &str) -> Result<&'a mut Table> {
    if table.get(key).is_none() {
        table.insert(key, Item::Table(Table::new()));
    }
    table
        .get_mut(key)
        .expect("table entry was inserted")
        .as_table_mut()
        .with_context(|| format!("FEDERATION_CONFIG_INVALID: {label} must be a table"))
}

fn peer_json(name: &str, peer: &FederationPeer) -> Value {
    json!({
        "name": name,
        "ssh_host": peer.ssh_host,
        "ssh_config_file": peer.ssh_config_file,
        "remote_socket": peer.remote_socket.to_string_lossy(),
        "allow_inbound": peer.allow_inbound,
        "allow_outbound": peer.allow_outbound,
    })
}

fn toml_path(path: &Path, label: &str) -> Result<String> {
    path.to_str()
        .map(str::to_owned)
        .with_context(|| format!("FEDERATION_CONFIG_INVALID: {label} must be UTF-8"))
}

struct ConfigLock {
    file: File,
}

impl ConfigLock {
    fn acquire(config_path: &Path) -> Result<Self> {
        let path = lock_path(config_path);
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .mode(0o600)
            .open(&path)
            .with_context(|| format!("open config lock {}", path.display()))?;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))
            .with_context(|| format!("restrict config lock {}", path.display()))?;
        loop {
            let result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) };
            if result == 0 {
                return Ok(Self { file });
            }
            let error = std::io::Error::last_os_error();
            if error.kind() != std::io::ErrorKind::Interrupted {
                return Err(error).with_context(|| format!("lock config with {}", path.display()));
            }
        }
    }
}

impl Drop for ConfigLock {
    fn drop(&mut self) {
        let _ = unsafe { libc::flock(self.file.as_raw_fd(), libc::LOCK_UN) };
    }
}

fn lock_path(config_path: &Path) -> PathBuf {
    let mut name = config_path
        .file_name()
        .unwrap_or_else(|| std::ffi::OsStr::new("config.toml"))
        .to_os_string();
    name.push(".lock");
    config_path.with_file_name(name)
}

fn atomic_replace(path: &Path, contents: &[u8]) -> Result<()> {
    let parent = path
        .parent()
        .context("FEDERATION_CONFIG_PATH_INVALID: config path has no parent")?;
    let temporary = temporary_path(path);
    let result = (|| -> Result<()> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temporary)
            .with_context(|| format!("create temporary config {}", temporary.display()))?;
        file.write_all(contents)
            .with_context(|| format!("write temporary config {}", temporary.display()))?;
        file.sync_all()
            .with_context(|| format!("sync temporary config {}", temporary.display()))?;
        drop(file);
        fs::rename(&temporary, path).with_context(|| format!("replace {}", path.display()))?;
        let directory = File::open(parent)
            .with_context(|| format!("open config directory {}", parent.display()))?;
        directory
            .sync_all()
            .with_context(|| format!("sync config directory {}", parent.display()))?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn temporary_path(path: &Path) -> PathBuf {
    let mut name = OsString::from(".");
    name.push(
        path.file_name()
            .unwrap_or_else(|| std::ffi::OsStr::new("config.toml")),
    );
    name.push(format!(
        ".{}.{}.tmp",
        std::process::id(),
        NEXT_TEMPORARY.fetch_add(1, Ordering::Relaxed)
    ));
    path.with_file_name(name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::{Arc, Barrier};
    use std::thread;

    static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);

    struct Fixture(PathBuf);

    impl Fixture {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "collab-machine-management-{}-{}",
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

        fn secret(&self, name: &str) -> PathBuf {
            self.0.join(name)
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }

    fn add(fixture: &Fixture, name: &str) {
        add_peer(
            &fixture.config(),
            name,
            &format!("{name}.example"),
            Path::new("/private/collab-federation.sock"),
            &fixture.secret(&format!("{name}-out")),
            &fixture.secret(&format!("{name}-in")),
            true,
            true,
            None,
        )
        .unwrap();
    }

    #[test]
    fn read_only_commands_do_not_create_the_config_file() {
        let fixture = Fixture::new();
        assert_eq!(status(&fixture.config()).unwrap()["configured"], false);
        assert_eq!(list(&fixture.config()).unwrap()["configured"], false);
        assert!(!fixture.config().exists());
    }

    #[test]
    fn invalid_machine_and_ssh_option_injection_never_replace_the_config() {
        let fixture = Fixture::new();
        let original = "# keep\n[federation]\nmachine = 'studio'\n";
        fs::write(fixture.config(), original).unwrap();
        assert!(initialize(&fixture.config(), "bad.machine").is_err());
        assert!(add_peer(
            &fixture.config(),
            "air",
            "-oProxyCommand=unexpected",
            Path::new("/private/collab-federation.sock"),
            &fixture.secret("air-out"),
            &fixture.secret("air-in"),
            true,
            true,
            None,
        )
        .is_err());
        assert_eq!(fs::read_to_string(fixture.config()).unwrap(), original);
    }

    #[test]
    fn credential_creation_is_private_random_and_never_overwrites() {
        let fixture = Fixture::new();
        let path = fixture.secret("directional-credential");
        let result = create_credential(&path).unwrap();
        assert_eq!(result, json!({"path": path.to_string_lossy()}));
        let first = fs::read_to_string(&path).unwrap();
        assert_eq!(first.len(), 64);
        assert!(first.bytes().all(|byte| byte.is_ascii_hexdigit()));
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let error = create_credential(&path).unwrap_err().to_string();
        assert!(error.starts_with("FEDERATION_CREDENTIAL_EXISTS"), "{error}");
        assert_eq!(fs::read_to_string(&path).unwrap(), first);
        assert!(!result.to_string().contains(&first));
    }

    #[test]
    fn mutations_atomically_preserve_comments_and_unrelated_configuration() {
        let fixture = Fixture::new();
        let config = fixture.config();
        fs::write(
            &config,
            r#"# host config
[notifications]
mode = "immediate" # keep this
submit_enter = true
"#,
        )
        .unwrap();
        initialize(&config, "studio").unwrap();
        add(&fixture, "air");
        let updated = fs::read_to_string(&config).unwrap();
        assert!(updated.contains("# host config"));
        assert!(updated.contains("# keep this"));
        assert!(updated.contains("[federation]"));
        let parsed = super::super::config::from_toml(&updated).unwrap().unwrap();
        assert_eq!(parsed.machine, "studio");
        assert!(parsed.peers.contains_key("air"));
        assert!(!fixture
            .0
            .read_dir()
            .unwrap()
            .filter_map(Result::ok)
            .any(|entry| entry.file_name().to_string_lossy().ends_with(".tmp")));
    }

    #[test]
    fn concurrent_management_calls_do_not_lose_each_others_additions() {
        let fixture = Fixture::new();
        initialize(&fixture.config(), "studio").unwrap();
        let barrier = Arc::new(Barrier::new(2));
        let mut handles = Vec::new();
        for name in ["air", "coder1"] {
            let config = fixture.config();
            let barrier = Arc::clone(&barrier);
            let outbound = fixture.secret(&format!("{name}-out"));
            let inbound = fixture.secret(&format!("{name}-in"));
            handles.push(thread::spawn(move || {
                barrier.wait();
                add_peer(
                    &config,
                    name,
                    &format!("{name}.example"),
                    Path::new("/private/collab-federation.sock"),
                    &outbound,
                    &inbound,
                    true,
                    true,
                    None,
                )
            }));
        }
        for handle in handles {
            handle.join().unwrap().unwrap();
        }
        let configuration =
            super::super::config::from_toml(&fs::read_to_string(fixture.config()).unwrap())
                .unwrap()
                .unwrap();
        assert_eq!(
            configuration.peers.keys().cloned().collect::<Vec<_>>(),
            vec!["air".to_owned(), "coder1".to_owned()]
        );
    }
}
