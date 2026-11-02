fn format_cli_error(error: &str) -> String {
    if error.starts_with("ROUTE_RESOLVE_NOT_FOUND:")
        && !error.contains(crate::server::ROUTE_RESOLVE_NOT_FOUND_RECOVERY)
    {
        format!(
            "{error}; {}",
            crate::server::ROUTE_RESOLVE_NOT_FOUND_RECOVERY
        )
    } else {
        error.to_owned()
    }
}
