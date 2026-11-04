fn central_runtime_storage_root(
    host_paths: &HostPaths,
    project_scope: &str,
    app_scope: &str,
) -> PathBuf {
    host_paths.app_storage_root(project_scope, app_scope)
}

/// The storage-version boundary is the central host namespace. Retired
/// project-local journals are never read, copied, or used to choose a reducer.
fn select_central_runtime_storage(
    host_paths: &HostPaths,
    route_journal: &Path,
    key: &RouteKey,
    publication: Option<&std::sync::Mutex<()>>,
) -> Result<PathBuf, String> {
    let central = central_runtime_storage_root(host_paths, &key.1, &key.0);
    let _publication = publication.map(|gate| gate.lock().unwrap());
    if let Some(mut record) = load_host_route_records(route_journal)?
        .into_iter()
        .find(|record| record.app_scope_id == key.0 && record.project_scope == key.1)
    {
        if Path::new(&record.storage_root) != central {
            if Path::new(&record.storage_root).starts_with(host_paths.state_root().join("projects")) {
                // Current central aliases keep their declared owner. The
                // existing route validator checks sharing and scope; storage
                // version isolation must not reset active central records.
                return Ok(PathBuf::from(record.storage_root));
            }
            record.storage_root = central.to_string_lossy().into_owned();
            publish_central_route_record(route_journal, record)?;
        }
    }
    Ok(central)
}

fn publish_central_route_record(path: &Path, record: HostRouteRecord) -> Result<(), String> {
    let store = crate::server::binding_store::BindingControlStore::at(path);
    let (mut routes, control) = store
        .load_shared()
        .map_err(|error| format!("HOST_ROUTE_DURABILITY_FAILED: {error}"))?;
    let mut replaced = false;
    for existing in routes.iter_mut() {
        if existing.app_scope_id == record.app_scope_id
            && existing.project_scope == record.project_scope
        {
            replaced = true;
            *existing = record.clone();
        }
    }
    if !replaced {
        routes.push(record);
    }
    store
        .publish_shared(&routes, &control)
        .map_err(|error| format!("HOST_ROUTE_DURABILITY_FAILED: {error}"))?
        .require_confirmed_route()
}
