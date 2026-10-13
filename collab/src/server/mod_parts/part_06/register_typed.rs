
/// Legacy Register owner: prepare then consume with no outer observation.
/// Every other Register path must route through the same two functions so the
/// identifier algorithm only lives in `part_02.rs`.
fn register_typed(
    server: &Server,
    worker_id: &str,
    token: &str,
    transport: &SelectedTransport,
    cwd: &str,
    project_scope: Option<ProjectScopeId>,
    app_scope: Option<AppServerId>,
    reuse_existing: bool,
) -> Resp {
    register_typed_observed(
        server,
        worker_id,
        token,
        transport,
        cwd,
        project_scope,
        app_scope,
        reuse_existing,
        None,
        None,
        None,
    )
}
