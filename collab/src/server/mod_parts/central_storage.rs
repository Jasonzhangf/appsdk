fn select_central_runtime_storage(
    host_paths: &HostPaths,
    route_file: &Path,
    key: &RouteKey,
    publication: Option<&std::sync::Mutex<()>>,
) -> Result<PathBuf, String> {
    let central = storage_owner_path(&host_paths.app_storage_root(&key.1, &key.0))?;
    validate_runtime_storage_root(
        Path::new(&key.1),
        &central,
        host_paths.state_root(),
        "HOST_ROUTE_DURABILITY_FAILED",
    )?;
    let _publication = publication.map(|gate| gate.lock().unwrap());
    let records = load_host_route_records(route_file)?;
    if let Some(record) = records
        .iter()
        .find(|record| record.app_scope_id == key.0 && record.project_scope == key.1)
    {
        let (record_key, _, storage) = validate_host_route_record(record, host_paths)
            .map_err(|error| format!("HOST_ROUTE_DURABILITY_FAILED: {error}"))?;
        if record_key != *key {
            return Err("HOST_ROUTE_DURABILITY_FAILED: route key changed during selection".into());
        }
        ensure_current_namespace(&storage, true)?;
        return Ok(storage);
    }
    // Resolve every declared owner before namespace creation. A rejected
    // registration must not create state in another route's storage, or leave
    // an unreachable namespace after an ownership read fails.
    for record in &records {
        if storage_roots_equal(Path::new(&record.storage_root), &central)? {
            return Err(format!(
                "HOST_ROUTE_DURABILITY_FAILED: runtime storage root {} is already owned by route ({}, {})",
                central.display(), record.app_scope_id, record.project_scope
            ));
        }
    }
    let mapped = crate::server::binding_store::BindingControlStore::new(host_paths.state_root())
        .load()?
        .slots()
        .any(|slot| {
            slot.identity_key.project_scope.as_str() == key.1
                && slot.identity_key.app_scope_id.as_str() == key.0
        });
    ensure_current_namespace(&central, mapped)?;
    Ok(central)
}

fn ensure_current_namespace(storage: &Path, declared: bool) -> Result<(), String> {
    let directory = storage.join(".agent-collab/server");
    let path = directory.join("state.json");
    if path.is_file() {
        return Ok(());
    }
    let has_data = match std::fs::read_dir(&directory) {
        Ok(entries) => {
            let mut present = false;
            for entry in entries {
                let entry = entry.map_err(|error| error.to_string())?;
                if entry.file_name() != "daemon.lock" {
                    present = true;
                    break;
                }
            }
            present
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
        Err(error) => return Err(format!("CURRENT_STATE_READ_FAILED: {error}")),
    };
    if declared || has_data {
        return Err(format!(
            "CURRENT_STATE_MIGRATION_REQUIRED: {}",
            storage.display()
        ));
    }
    match crate::server::current_state::save(&path, &State::default())
        .map_err(|error| error.to_string())?
    {
        true => Ok(()),
        false => Err(format!(
            "CURRENT_STATE_DURABILITY_UNKNOWN: published=true; {}",
            path.display()
        )),
    }
}
