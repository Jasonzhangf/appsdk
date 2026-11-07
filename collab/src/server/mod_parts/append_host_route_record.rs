fn append_host_route_record(path: &Path, record: &HostRouteRecord) -> Result<(), String> {
    let mut routes = load_host_route_records(path)
        .map_err(|error| format!("HOST_ROUTE_DURABILITY_FAILED: {error}"))?;
    if routes.iter().any(|existing| {
        existing.app_scope_id == record.app_scope_id
            && existing.project_scope == record.project_scope
    }) {
        return Err(format!(
            "HOST_ROUTE_DURABILITY_FAILED: route key ({}, {}) is already durable",
            record.app_scope_id, record.project_scope
        ));
    }
    routes.push(record.clone());
    crate::server::binding_store::publish_route_records(path, &routes)
        .map_err(|error| format!("HOST_ROUTE_DURABILITY_FAILED: {error}"))?
        .require_confirmed_route()
}
