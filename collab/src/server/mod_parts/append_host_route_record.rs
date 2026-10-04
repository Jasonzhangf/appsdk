fn append_host_route_record(path: &Path, record: &HostRouteRecord) -> Result<(), String> {
    let parent = path.parent().ok_or_else(|| {
        "HOST_ROUTE_DURABILITY_FAILED: route journal has no parent directory".to_string()
    })?;
    std::fs::create_dir_all(parent).map_err(|error| {
        format!("HOST_ROUTE_DURABILITY_FAILED: create route journal directory: {error}")
    })?;
    let existing = match std::fs::read(path) {
        Ok(content) => content,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(error) => {
            return Err(format!(
                "HOST_ROUTE_DURABILITY_FAILED: read route journal: {error}"
            ))
        }
    };
    let existing_records = load_host_route_records(path).map_err(|error| {
        format!("HOST_ROUTE_DURABILITY_FAILED: validate route journal: {error}")
    })?;
    if existing_records.iter().any(|existing| {
        existing.app_scope_id == record.app_scope_id
            && existing.project_scope == record.project_scope
    }) {
        return Err(format!(
            "HOST_ROUTE_DURABILITY_FAILED: route key ({}, {}) is already durable",
            record.app_scope_id, record.project_scope
        ));
    }
    if !existing.is_empty() && !existing.ends_with(b"\n") {
        return Err("HOST_ROUTE_DURABILITY_FAILED: route journal must end with a newline".into());
    }
    use std::io::Write;
    let mut line = serde_json::to_vec(record)
        .map_err(|error| format!("HOST_ROUTE_DURABILITY_FAILED: serialize route: {error}"))?;
    line.push(b'\n');
    let mut body = existing;
    body.extend_from_slice(&line);

    // Replace the complete JSONL file after syncing a private temporary file.
    // A crash or short write therefore leaves either the previous valid route
    // set or the complete new route set, never a partial JSON object.
    static TEMP_SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let sequence = TEMP_SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let tmp = path.with_file_name(format!(
        "{}.tmp-{}-{sequence}",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("routes.jsonl"),
        std::process::id()
    ));
    let mut tmp_file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&tmp)
        .map_err(|error| {
            format!("HOST_ROUTE_DURABILITY_FAILED: create route journal temp: {error}")
        })?;
    if let Err(error) = tmp_file.write_all(&body) {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!(
            "HOST_ROUTE_DURABILITY_FAILED: write route journal temp: {error}"
        ));
    }
    if let Err(error) = tmp_file.sync_data() {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!(
            "HOST_ROUTE_DURABILITY_FAILED: flush route journal temp: {error}"
        ));
    }
    drop(tmp_file);
    if let Err(error) = std::fs::rename(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!(
            "HOST_ROUTE_DURABILITY_FAILED: publish route journal: {error}"
        ));
    }
    if let Err(error) = sync_parent_dir(path) {
        return Err(format!(
            "HOST_ROUTE_DURABILITY_FAILED: sync route journal directory: {error}"
        ));
    }
    load_host_route_records(path)
        .map_err(|error| format!("HOST_ROUTE_DURABILITY_FAILED: verify route journal: {error}"))?;
    Ok(())
}
