fn validate_route_owner_table(
    host: &Arc<Server>,
    routes: &std::collections::BTreeMap<RouteKey, RuntimeRoute>,
) -> Result<(), String> {
    let mut owners = std::collections::BTreeMap::<PathBuf, (String, bool)>::new();
    owners.insert(
        storage_owner_path(&host.root)
            .map_err(|error| format!("HOST_ROUTE_REPLAY_FAILED: {error}"))?,
        ("resident host".into(), true),
    );
    owners.insert(
        storage_owner_path(&host.storage_root)
            .map_err(|error| format!("HOST_ROUTE_REPLAY_FAILED: {error}"))?,
        ("resident host".into(), true),
    );

    for (key, route) in routes {
        let route_owner = format!("route ({}, {})", key.0, key.1);
        let is_resident_runtime = route
            .runtime
            .as_ref()
            .is_some_and(|runtime| Arc::ptr_eq(runtime, host));
        let route_storage_root = storage_owner_path(&route.storage_root)
            .map_err(|error| format!("HOST_ROUTE_REPLAY_FAILED: {error}"))?;
        if let Some((owner, is_host)) = owners.get(&route_storage_root) {
            if is_resident_runtime && *is_host {
                continue;
            }
            return Err(format!(
                "HOST_ROUTE_REPLAY_FAILED: runtime storage root {} is already owned by {}",
                route.storage_root.display(),
                owner
            ));
        }
        owners.insert(route_storage_root, (route_owner, is_resident_runtime));
    }
    Ok(())
}
