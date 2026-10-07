fn format_cli_error(error: &str) -> String {
    let decorated = if error.starts_with("ROUTE_RESOLVE_NOT_FOUND:")
        && !error.contains(crate::server::ROUTE_RESOLVE_NOT_FOUND_RECOVERY)
        && !error.contains(LEGACY_ROUTE_RESOLVE_NOT_FOUND_UPGRADE)
    {
        if error.contains(LEGACY_ROUTE_RESOLVE_NOT_FOUND_RECOVERY) {
            format!("{error}; {LEGACY_ROUTE_RESOLVE_NOT_FOUND_UPGRADE}")
        } else {
            format!(
                "{error}; {}",
                crate::server::ROUTE_RESOLVE_NOT_FOUND_RECOVERY
            )
        }
    } else {
        error.to_owned()
    };
    decorated
}
