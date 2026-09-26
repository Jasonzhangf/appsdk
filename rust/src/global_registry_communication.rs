use super::*;
use std::collections::BTreeMap;

/// Stable address used by the host-wide communication discovery index.
///
/// The project mailbox remains the source of truth for the complete scope and
/// agent records.  This host-wide stream only maps an address to the project
/// mailbox that owns it, so a sender can open that mailbox and revalidate the
/// live record before routing a message.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CommunicationAddress {
    #[serde(rename = "scopeId", alias = "scope_id")]
    pub scope_id: String,
    #[serde(rename = "sessionId", alias = "session_id")]
    pub session_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CommunicationTarget {
    #[serde(rename = "projectRoot")]
    pub project_root: PathBuf,
    pub address: CommunicationAddress,
    #[serde(rename = "reboundFrom", skip_serializing_if = "Option::is_none")]
    pub rebound_from: Option<CommunicationAddress>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CommunicationReceipt {
    pub registry_root: PathBuf,
    pub registry_path: PathBuf,
    pub idempotent: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct CommunicationEvent {
    schema_version: u64,
    event: String,
    #[serde(rename = "scopeId")]
    scope_id: String,
    #[serde(default, rename = "sessionId")]
    session_id: Option<String>,
    #[serde(rename = "projectRoot")]
    project_root: String,
    #[serde(default, rename = "reboundTo")]
    rebound_to: Option<String>,
    #[serde(rename = "registeredAt")]
    registered_at: String,
    source: String,
}

#[derive(Debug, Default)]
struct CommunicationProjection {
    scopes: BTreeMap<String, String>,
    agents: BTreeMap<String, String>,
    tombstones: BTreeMap<String, (String, String)>,
}

fn communication_address_key(scope_id: &str, session_id: &str) -> String {
    format!(
        "{}#{}|{}#{}",
        scope_id.len(),
        scope_id,
        session_id.len(),
        session_id
    )
}

fn validate_communication_part(value: &str, label: &str) -> Result<(), String> {
    if value.trim().is_empty() {
        return Err(format!(
            "GLOBAL_COMMUNICATION_IDENTITY_INVALID:{label}:empty"
        ));
    }
    if value.chars().count() > 512 {
        return Err(format!(
            "GLOBAL_COMMUNICATION_IDENTITY_INVALID:{label}:too_long"
        ));
    }
    Ok(())
}

fn validate_communication_project_root_for_write(root: &Path) -> Result<PathBuf, String> {
    if !root.is_absolute() || !is_lexically_canonical_absolute(root) {
        return Err(format!(
            "GLOBAL_COMMUNICATION_PROJECT_INVALID:project root must be canonical: {}",
            root.display()
        ));
    }
    if !root.is_dir() {
        return Err(format!(
            "GLOBAL_COMMUNICATION_PROJECT_INVALID:project root must be an existing directory: {}",
            root.display()
        ));
    }
    ensure_no_symlink(root, "communication_project_root")?;
    let canonical = fs::canonicalize(root)
        .map_err(|error| format!("GLOBAL_COMMUNICATION_PROJECT_CANONICALIZE_FAILED:{error}"))?;
    if canonical != root {
        return Err(format!(
            "GLOBAL_COMMUNICATION_PROJECT_INVALID:project root is not canonical: {}",
            root.display()
        ));
    }
    Ok(canonical)
}

fn validate_communication_project_root_record(value: &str, line: usize) -> Result<(), String> {
    let root = Path::new(value);
    if !root.is_absolute() || !is_lexically_canonical_absolute(root) {
        return Err(format!(
            "GLOBAL_COMMUNICATION_REGISTRY_INVALID_EVENT:{line}:project root is not canonical"
        ));
    }
    ensure_no_symlink(root, "communication_project_root")
        .map_err(|error| format!("GLOBAL_COMMUNICATION_REGISTRY_INVALID_EVENT:{line}:{error}"))?;
    match fs::canonicalize(root) {
        Ok(canonical) if canonical == root => Ok(()),
        Ok(_) => Err(format!(
            "GLOBAL_COMMUNICATION_REGISTRY_INVALID_EVENT:{line}:project root is not canonical"
        )),
        Err(error) if error.kind() == ErrorKind::NotFound => {
            let canonical = canonicalize_missing_root(root).map_err(|error| {
                format!("GLOBAL_COMMUNICATION_REGISTRY_INVALID_EVENT:{line}:{error}")
            })?;
            if canonical == root {
                Ok(())
            } else {
                Err(format!(
                    "GLOBAL_COMMUNICATION_REGISTRY_INVALID_EVENT:{line}:project root is not canonical"
                ))
            }
        }
        Err(error) => Err(format!(
            "GLOBAL_COMMUNICATION_REGISTRY_INVALID_EVENT:{line}:project root canonicalization failed:{error}"
        )),
    }
}

fn apply_communication_event(
    projection: &mut CommunicationProjection,
    event: &CommunicationEvent,
    line: usize,
) -> Result<(), String> {
    if event.schema_version != REGISTRY_SCHEMA_VERSION
        || event.source != COMMUNICATION_SOURCE
        || DateTime::parse_from_rfc3339(&event.registered_at).is_err()
    {
        return Err(format!(
            "GLOBAL_COMMUNICATION_REGISTRY_INVALID_EVENT:{line}:unsupported communication event"
        ));
    }
    validate_communication_part(&event.scope_id, "scope_id")
        .map_err(|error| format!("GLOBAL_COMMUNICATION_REGISTRY_INVALID_EVENT:{line}:{error}"))?;
    validate_communication_project_root_record(&event.project_root, line)?;
    if let Some(session_id) = event.session_id.as_deref() {
        validate_communication_part(session_id, "session_id").map_err(|error| {
            format!("GLOBAL_COMMUNICATION_REGISTRY_INVALID_EVENT:{line}:{error}")
        })?;
    }
    if let Some(rebound_to) = event.rebound_to.as_deref() {
        validate_communication_part(rebound_to, "rebound_to").map_err(|error| {
            format!("GLOBAL_COMMUNICATION_REGISTRY_INVALID_EVENT:{line}:{error}")
        })?;
    }

    match event.event.as_str() {
        "communication.scope.registered" => {
            if event.session_id.is_some() || event.rebound_to.is_some() {
                return Err(format!(
                    "GLOBAL_COMMUNICATION_REGISTRY_INVALID_EVENT:{line}:scope registration has agent fields"
                ));
            }
            if projection
                .scopes
                .insert(event.scope_id.clone(), event.project_root.clone())
                .is_some()
            {
                return Err(format!(
                    "GLOBAL_COMMUNICATION_REGISTRY_INVALID_EVENT:{line}:duplicate scope registration"
                ));
            }
        }
        "communication.scope.unregistered" => {
            if event.session_id.is_some() || event.rebound_to.is_some() {
                return Err(format!(
                    "GLOBAL_COMMUNICATION_REGISTRY_INVALID_EVENT:{line}:scope removal has agent fields"
                ));
            }
            if projection.scopes.get(&event.scope_id) != Some(&event.project_root) {
                return Err(format!(
                    "GLOBAL_COMMUNICATION_REGISTRY_INVALID_EVENT:{line}:scope removal does not match current owner"
                ));
            }
            projection.scopes.remove(&event.scope_id);
            projection.agents.retain(|key, _| {
                !key.starts_with(&format!("{}#{}|", event.scope_id.len(), event.scope_id))
            });
            projection.tombstones.retain(|key, _| {
                !key.starts_with(&format!("{}#{}|", event.scope_id.len(), event.scope_id))
            });
        }
        "communication.agent.registered" => {
            let session_id = event.session_id.as_deref().ok_or_else(|| {
                format!(
                    "GLOBAL_COMMUNICATION_REGISTRY_INVALID_EVENT:{line}:agent sessionId missing"
                )
            })?;
            if event.rebound_to.is_some() {
                return Err(format!(
                    "GLOBAL_COMMUNICATION_REGISTRY_INVALID_EVENT:{line}:agent registration has reboundTo"
                ));
            }
            if projection.scopes.get(&event.scope_id) != Some(&event.project_root) {
                return Err(format!(
                    "GLOBAL_COMMUNICATION_REGISTRY_INVALID_EVENT:{line}:agent scope owner mismatch"
                ));
            }
            let key = communication_address_key(&event.scope_id, session_id);
            if projection.agents.contains_key(&key) || projection.tombstones.contains_key(&key) {
                return Err(format!(
                    "GLOBAL_COMMUNICATION_REGISTRY_INVALID_EVENT:{line}:duplicate agent registration"
                ));
            }
            projection.agents.insert(key, event.project_root.clone());
        }
        "communication.agent.rebound" => {
            let from_session = event.session_id.as_deref().ok_or_else(|| {
                format!(
                    "GLOBAL_COMMUNICATION_REGISTRY_INVALID_EVENT:{line}:rebound sessionId missing"
                )
            })?;
            let to_session = event.rebound_to.as_deref().ok_or_else(|| {
                format!("GLOBAL_COMMUNICATION_REGISTRY_INVALID_EVENT:{line}:reboundTo missing")
            })?;
            if from_session == to_session {
                return Err(format!(
                    "GLOBAL_COMMUNICATION_REGISTRY_INVALID_EVENT:{line}:rebind must change address"
                ));
            }
            if projection.scopes.get(&event.scope_id) != Some(&event.project_root) {
                return Err(format!(
                    "GLOBAL_COMMUNICATION_REGISTRY_INVALID_EVENT:{line}:rebind scope owner mismatch"
                ));
            }
            let from_key = communication_address_key(&event.scope_id, from_session);
            let to_key = communication_address_key(&event.scope_id, to_session);
            let owner = projection.agents.remove(&from_key).ok_or_else(|| {
                format!(
                    "GLOBAL_COMMUNICATION_REGISTRY_INVALID_EVENT:{line}:rebind source is not active"
                )
            })?;
            if owner != event.project_root
                || projection.agents.contains_key(&to_key)
                || projection.tombstones.contains_key(&to_key)
            {
                return Err(format!(
                    "GLOBAL_COMMUNICATION_REGISTRY_INVALID_EVENT:{line}:rebind target is occupied or owner mismatched"
                ));
            }
            projection.agents.insert(to_key, owner);
            projection
                .tombstones
                .insert(from_key, (event.scope_id.clone(), to_session.to_string()));
        }
        other => {
            return Err(format!(
                "GLOBAL_COMMUNICATION_REGISTRY_UNKNOWN_EVENT:{line}:{other}"
            ));
        }
    }
    Ok(())
}

fn read_communication_projection(
    path: &Path,
    missing_index_is_empty: bool,
) -> Result<CommunicationProjection, String> {
    let mut file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == ErrorKind::NotFound && missing_index_is_empty => {
            return Ok(CommunicationProjection::default())
        }
        Err(error) if error.kind() == ErrorKind::NotFound => {
            return Err(format!(
                "GLOBAL_COMMUNICATION_REGISTRY_UNAVAILABLE:{}",
                path.display()
            ))
        }
        Err(error) => return Err(format!("GLOBAL_COMMUNICATION_REGISTRY_READ_FAILED:{error}")),
    };
    let mut text = String::new();
    file.read_to_string(&mut text)
        .map_err(|error| format!("GLOBAL_COMMUNICATION_REGISTRY_READ_FAILED:{error}"))?;
    if !text.is_empty() && !text.ends_with('\n') {
        return Err("GLOBAL_COMMUNICATION_REGISTRY_INVALID_LINE:missing final newline".into());
    }
    let mut projection = CommunicationProjection::default();
    for (index, line) in text.lines().enumerate() {
        let line_number = index + 1;
        if line.trim().is_empty() {
            return Err(format!(
                "GLOBAL_COMMUNICATION_REGISTRY_INVALID_LINE:{line_number}:blank line"
            ));
        }
        let event: CommunicationEvent = serde_json::from_str(line).map_err(|error| {
            format!("GLOBAL_COMMUNICATION_REGISTRY_INVALID_LINE:{line_number}:{error}")
        })?;
        apply_communication_event(&mut projection, &event, line_number)?;
    }
    Ok(projection)
}

fn communication_registry_paths(
    registry_root: &Path,
) -> Result<(PathBuf, PathBuf, PathBuf), String> {
    let registry_root = ensure_registry_root(registry_root)?;
    ensure_no_symlink(&registry_root, "registry_root")?;
    let path = registry_root.join(COMMUNICATION_FILE);
    let lock_path = registry_root.join(COMMUNICATION_LOCK);
    ensure_no_symlink(&path, "communication_registry_file")?;
    ensure_no_symlink(&lock_path, "communication_registry_lock")?;
    Ok((registry_root, path, lock_path))
}

fn communication_lock(lock_path: &Path) -> Result<File, String> {
    lock_registry(lock_path).map_err(|error| {
        if let Some(detail) = error.strip_prefix("GLOBAL_REGISTRY_BUSY:") {
            format!("GLOBAL_COMMUNICATION_REGISTRY_BUSY:{detail}")
        } else {
            format!("GLOBAL_COMMUNICATION_REGISTRY_LOCK_FAILED:{error}")
        }
    })
}

fn append_communication_event(path: &Path, event: &CommunicationEvent) -> Result<(), String> {
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|error| format!("GLOBAL_COMMUNICATION_REGISTRY_OPEN_FAILED:{error}"))?;
    let mut line = serde_json::to_vec(event)
        .map_err(|error| format!("GLOBAL_COMMUNICATION_REGISTRY_SERIALIZE_FAILED:{error}"))?;
    line.push(b'\n');
    file.write_all(&line)
        .and_then(|_| file.sync_all())
        .map_err(|error| format!("GLOBAL_COMMUNICATION_REGISTRY_WRITE_FAILED:{error}"))
}

fn communication_event(
    event: &str,
    scope_id: &str,
    session_id: Option<&str>,
    project_root: &Path,
    rebound_to: Option<&str>,
) -> Result<CommunicationEvent, String> {
    validate_communication_part(scope_id, "scope_id")?;
    if let Some(session_id) = session_id {
        validate_communication_part(session_id, "session_id")?;
    }
    if let Some(rebound_to) = rebound_to {
        validate_communication_part(rebound_to, "rebound_to")?;
    }
    let canonical_root = validate_communication_project_root_for_write(project_root)?;
    Ok(CommunicationEvent {
        schema_version: REGISTRY_SCHEMA_VERSION,
        event: event.to_string(),
        scope_id: scope_id.to_string(),
        session_id: session_id.map(str::to_string),
        project_root: canonical_root.to_string_lossy().to_string(),
        rebound_to: rebound_to.map(str::to_string),
        registered_at: Utc::now().to_rfc3339(),
        source: COMMUNICATION_SOURCE.to_string(),
    })
}

pub fn register_communication_scope(
    scope_id: &str,
    project_root: &Path,
) -> Result<CommunicationReceipt, String> {
    let root = registry_root()?;
    register_communication_scope_at(scope_id, project_root, &root)
}

pub fn register_communication_scope_at(
    scope_id: &str,
    project_root: &Path,
    registry_root: &Path,
) -> Result<CommunicationReceipt, String> {
    let event = communication_event(
        "communication.scope.registered",
        scope_id,
        None,
        project_root,
        None,
    )?;
    let (registry_root, path, lock_path) = communication_registry_paths(registry_root)?;
    let _lock = communication_lock(&lock_path)?;
    let projection = read_communication_projection(&path, true)?;
    if let Some(existing) = projection.scopes.get(scope_id) {
        if existing == &event.project_root {
            return Ok(CommunicationReceipt {
                registry_root,
                registry_path: path,
                idempotent: true,
            });
        }
        return Err(format!("GLOBAL_COMMUNICATION_SCOPE_CONFLICT:{scope_id}"));
    }
    append_communication_event(&path, &event)?;
    Ok(CommunicationReceipt {
        registry_root,
        registry_path: path,
        idempotent: false,
    })
}

pub fn register_communication_agent(
    scope_id: &str,
    session_id: &str,
    project_root: &Path,
) -> Result<CommunicationReceipt, String> {
    let root = registry_root()?;
    register_communication_agent_at(scope_id, session_id, project_root, &root)
}

pub fn register_communication_agent_at(
    scope_id: &str,
    session_id: &str,
    project_root: &Path,
    registry_root: &Path,
) -> Result<CommunicationReceipt, String> {
    let event = communication_event(
        "communication.agent.registered",
        scope_id,
        Some(session_id),
        project_root,
        None,
    )?;
    let (registry_root, path, lock_path) = communication_registry_paths(registry_root)?;
    let _lock = communication_lock(&lock_path)?;
    let projection = read_communication_projection(&path, true)?;
    if projection.scopes.get(scope_id) != Some(&event.project_root) {
        return Err(format!("GLOBAL_COMMUNICATION_SCOPE_NOT_FOUND:{scope_id}"));
    }
    let key = communication_address_key(scope_id, session_id);
    if let Some(existing) = projection.agents.get(&key) {
        if existing == &event.project_root {
            return Ok(CommunicationReceipt {
                registry_root,
                registry_path: path,
                idempotent: true,
            });
        }
        return Err(format!(
            "GLOBAL_COMMUNICATION_AGENT_CONFLICT:{scope_id}/{session_id}"
        ));
    }
    if projection.tombstones.contains_key(&key) {
        return Err(format!(
            "GLOBAL_COMMUNICATION_AGENT_REBOUND:{scope_id}/{session_id}"
        ));
    }
    append_communication_event(&path, &event)?;
    Ok(CommunicationReceipt {
        registry_root,
        registry_path: path,
        idempotent: false,
    })
}

pub fn rebind_communication_agent(
    scope_id: &str,
    from_session_id: &str,
    to_session_id: &str,
    project_root: &Path,
) -> Result<CommunicationReceipt, String> {
    let root = registry_root()?;
    rebind_communication_agent_at(
        scope_id,
        from_session_id,
        to_session_id,
        project_root,
        &root,
    )
}

pub fn rebind_communication_agent_at(
    scope_id: &str,
    from_session_id: &str,
    to_session_id: &str,
    project_root: &Path,
    registry_root: &Path,
) -> Result<CommunicationReceipt, String> {
    if from_session_id == to_session_id {
        return Err("GLOBAL_COMMUNICATION_REBIND_INVALID:same address".into());
    }
    let event = communication_event(
        "communication.agent.rebound",
        scope_id,
        Some(from_session_id),
        project_root,
        Some(to_session_id),
    )?;
    let (registry_root, path, lock_path) = communication_registry_paths(registry_root)?;
    let _lock = communication_lock(&lock_path)?;
    let projection = read_communication_projection(&path, true)?;
    if projection.scopes.get(scope_id) != Some(&event.project_root) {
        return Err(format!("GLOBAL_COMMUNICATION_SCOPE_NOT_FOUND:{scope_id}"));
    }
    let from_key = communication_address_key(scope_id, from_session_id);
    let to_key = communication_address_key(scope_id, to_session_id);
    if let Some((rebound_scope, rebound_session)) = projection.tombstones.get(&from_key) {
        if rebound_scope == scope_id && rebound_session == to_session_id {
            return Ok(CommunicationReceipt {
                registry_root,
                registry_path: path,
                idempotent: true,
            });
        }
        return Err(format!(
            "GLOBAL_COMMUNICATION_AGENT_REBOUND:{scope_id}/{from_session_id}"
        ));
    }
    if !projection.agents.contains_key(&from_key) {
        return Err(format!(
            "GLOBAL_COMMUNICATION_AGENT_NOT_FOUND:{scope_id}/{from_session_id}"
        ));
    }
    if projection.agents.contains_key(&to_key) || projection.tombstones.contains_key(&to_key) {
        return Err(format!(
            "GLOBAL_COMMUNICATION_AGENT_CONFLICT:{scope_id}/{to_session_id}"
        ));
    }
    append_communication_event(&path, &event)?;
    Ok(CommunicationReceipt {
        registry_root,
        registry_path: path,
        idempotent: false,
    })
}

pub fn communication_target(
    address: &CommunicationAddress,
) -> Result<Option<CommunicationTarget>, String> {
    let root = registry_root()?;
    communication_target_at(address, &root)
}

pub fn communication_target_at(
    address: &CommunicationAddress,
    registry_root: &Path,
) -> Result<Option<CommunicationTarget>, String> {
    validate_communication_part(&address.scope_id, "scope_id")?;
    validate_communication_part(&address.session_id, "session_id")?;
    let (registry_root, path, lock_path) = communication_registry_paths(registry_root)?;
    let _lock = communication_lock(&lock_path)?;
    let projection = read_communication_projection(&path, false)?;
    let original = address.clone();
    let mut current = address.clone();
    let mut visited = BTreeMap::new();
    loop {
        let key = communication_address_key(&current.scope_id, &current.session_id);
        if visited.insert(key.clone(), true).is_some() {
            return Err("GLOBAL_COMMUNICATION_REGISTRY_INVALID_EVENT:rebind cycle".into());
        }
        if let Some(project_root) = projection.agents.get(&key) {
            let rebound_from = (current != original).then_some(original);
            return Ok(Some(CommunicationTarget {
                project_root: PathBuf::from(project_root),
                address: current,
                rebound_from,
            }));
        }
        let Some((scope_id, session_id)) = projection.tombstones.get(&key) else {
            let _ = registry_root;
            return Ok(None);
        };
        current = CommunicationAddress {
            scope_id: scope_id.clone(),
            session_id: session_id.clone(),
        };
    }
}
