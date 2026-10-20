fn central_runtime_storage_root(
    host_paths: &HostPaths,
    project_scope: &str,
    app_scope: &str,
) -> PathBuf {
    host_paths.app_storage_root(project_scope, app_scope)
}

fn route_record_for_key<'a>(
    records: &'a [HostRouteRecord],
    key: &RouteKey,
) -> Option<&'a HostRouteRecord> {
    records
        .iter()
        .find(|record| record.app_scope_id == key.0 && record.project_scope == key.1)
}

fn legacy_runtime_storage_source(
    host_paths: &HostPaths,
    route_journal: &Path,
    key: &RouteKey,
    fallback_storage_root: &Path,
) -> Result<Option<PathBuf>, String> {
    let central = central_runtime_storage_root(host_paths, &key.1, &key.0);
    let central = storage_owner_path(&central)
        .map_err(|error| format!("CENTRAL_STORAGE_MIGRATION_FAILED: {error}"))?;
    let records = load_host_route_records(route_journal)?;
    if let Some(record) = route_record_for_key(&records, key) {
        let storage = storage_owner_path(Path::new(&record.storage_root))
            .map_err(|error| format!("CENTRAL_STORAGE_MIGRATION_FAILED: {error}"))?;
        if storage != central {
            return Ok(Some(storage));
        }
        if central.join(".agent-collab/server/journal.jsonl").exists() {
            return Ok(None);
        }
    }
    let fallback = storage_owner_path(fallback_storage_root)
        .map_err(|error| format!("CENTRAL_STORAGE_MIGRATION_FAILED: {error}"))?;
    if fallback == central {
        if central.join(".agent-collab/server/journal.jsonl").exists() {
            return Ok(None);
        }
        let mut legacy = storage_owner_path(Path::new(&key.1))
            .map_err(|error| format!("CENTRAL_STORAGE_MIGRATION_FAILED: {error}"))?;
        if records.iter().any(|record| {
            (record.app_scope_id.as_str(), record.project_scope.as_str())
                != (key.0.as_str(), key.1.as_str())
                && storage_owner_path(Path::new(&record.storage_root))
                    .is_ok_and(|owner| owner == legacy)
        }) {
            legacy = storage_owner_path(&app_scope_storage_path(Path::new(&key.1), &key.0))
                .map_err(|error| format!("CENTRAL_STORAGE_MIGRATION_FAILED: {error}"))?;
        }
        return Ok(
            (legacy != central && legacy_runtime_artifacts_exist(&legacy)).then_some(legacy),
        );
    }
    Ok(Some(fallback))
}

fn legacy_runtime_artifacts_exist(root: &Path) -> bool {
    [
        ".agent-collab/server/journal.jsonl",
        ".agent-collab/server/events.jsonl",
        ".agent-collab/mailbox",
        ".agent-collab/messages",
        ".agent-collab/mailboxes",
    ]
    .iter()
    .any(|relative| root.join(relative).exists())
}

fn migrate_legacy_runtime_storage(
    host_paths: &HostPaths,
    route_journal: &Path,
    project_root: &Path,
    key: &RouteKey,
    source_root: &Path,
    destination_root: &Path,
) -> Result<bool, String> {
    migrate_legacy_runtime_storage_with_publication(
        host_paths,
        route_journal,
        project_root,
        key,
        source_root,
        destination_root,
        None,
    )
}

fn migrate_legacy_runtime_storage_with_publication(
    host_paths: &HostPaths,
    route_journal: &Path,
    project_root: &Path,
    key: &RouteKey,
    source_root: &Path,
    destination_root: &Path,
    publication: Option<&std::sync::Mutex<()>>,
) -> Result<bool, String> {
    let source_root = storage_owner_path(source_root)
        .map_err(|error| format!("CENTRAL_STORAGE_MIGRATION_FAILED: {error}"))?;
    let destination_root = storage_owner_path(destination_root)
        .map_err(|error| format!("CENTRAL_STORAGE_MIGRATION_FAILED: {error}"))?;
    let expected = central_runtime_storage_root(host_paths, &key.1, &key.0);
    let expected = storage_owner_path(&expected)
        .map_err(|error| format!("CENTRAL_STORAGE_MIGRATION_FAILED: {error}"))?;
    if destination_root != expected {
        return Err(format!(
            "CENTRAL_STORAGE_MIGRATION_FAILED: destination {} is not the canonical root {}",
            destination_root.display(),
            expected.display()
        ));
    }
    if source_root == destination_root {
        return Ok(false);
    }

    let source_journal = source_root.join(".agent-collab/server/journal.jsonl");
    replay_from_journal_read_only(project_root, &source_journal).map_err(|error| {
        format!(
            "CENTRAL_STORAGE_MIGRATION_FAILED: replay legacy journal {}: {error}",
            source_journal.display()
        )
    })?;

    let parent = destination_root.parent().ok_or_else(|| {
        "CENTRAL_STORAGE_MIGRATION_FAILED: destination has no parent directory".to_string()
    })?;
    std::fs::create_dir_all(parent).map_err(|error| {
        format!(
            "CENTRAL_STORAGE_MIGRATION_FAILED: create destination parent {}: {error}",
            parent.display()
        )
    })?;
    static MIGRATION_SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let sequence = MIGRATION_SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let staging = parent.join(format!(
        ".storage-migration-{}-{sequence}",
        std::process::id()
    ));
    if staging.exists() {
        project_reset::remove_project_tree(&staging, &[])?;
    }
    let migration = (|| {
        copy_legacy_runtime_tree(&source_root, &staging)?;
        let staging_journal = staging.join(".agent-collab/server/journal.jsonl");
        replay_from_journal_read_only(project_root, &staging_journal).map_err(|error| {
            format!(
                "CENTRAL_STORAGE_MIGRATION_FAILED: validate staged journal {}: {error}",
                staging_journal.display()
            )
        })?;
        if destination_root.exists() {
            let destination_journal = destination_root.join(".agent-collab/server/journal.jsonl");
            if destination_journal.is_file()
                && replay_from_journal_read_only(project_root, &destination_journal).is_ok()
                && std::fs::read(&destination_journal).map_err(|error| {
                    format!("CENTRAL_STORAGE_MIGRATION_FAILED: read destination journal: {error}")
                })? != std::fs::read(&source_journal).map_err(|error| {
                    format!("CENTRAL_STORAGE_MIGRATION_FAILED: read source journal: {error}")
                })?
            {
                return Err("CENTRAL_STORAGE_CONFLICT: an unactivated destination contains divergent journal history; both stores were preserved".into());
            }
            // Only an identical copy or an incomplete, unactivated transfer
            // can be replaced from the locked authoritative source.
            project_reset::remove_project_tree(&destination_root, &[])?;
        }
        std::fs::rename(&staging, &destination_root).map_err(|error| {
            format!(
                "CENTRAL_STORAGE_MIGRATION_FAILED: publish destination {}: {error}",
                destination_root.display()
            )
        })?;
        sync_parent_dir(&destination_root).map_err(|error| {
            format!("CENTRAL_STORAGE_MIGRATION_FAILED: sync destination parent: {error}")
        })?;
        publish_central_route_record(
            route_journal,
            HostRouteRecord {
                version: 1,
                op: "register".into(),
                app_scope_id: key.0.clone(),
                project_scope: key.1.clone(),
                canonical_root: std::fs::canonicalize(project_root)
                    .map_err(|error| {
                        format!(
                            "CENTRAL_STORAGE_MIGRATION_FAILED: canonicalize project root: {error}"
                        )
                    })?
                    .to_string_lossy()
                    .into_owned(),
                storage_root: destination_root.to_string_lossy().into_owned(),
                registered_ms: route_record_for_key(&load_host_route_records(route_journal)?, key)
                    .map_or_else(now_ms, |record| record.registered_ms),
            },
            publication,
        )?;
        Ok(true)
    })();
    if migration.is_err() && staging.exists() {
        let _ = project_reset::remove_project_tree(&staging, &[]);
    }
    migration
}

fn copy_legacy_runtime_tree(source_root: &Path, destination_root: &Path) -> Result<(), String> {
    let source = source_root.join(".agent-collab");
    if !source.exists() {
        return Ok(());
    }
    copy_runtime_directory(&source, &destination_root.join(".agent-collab"), &source)
}

fn copy_runtime_directory(
    source: &Path,
    destination: &Path,
    source_base: &Path,
) -> Result<(), String> {
    let metadata = std::fs::symlink_metadata(source).map_err(|error| {
        format!(
            "CENTRAL_STORAGE_MIGRATION_FAILED: inspect legacy runtime {}: {error}",
            source.display()
        )
    })?;
    if metadata.file_type().is_symlink() {
        return Err(format!(
            "CENTRAL_STORAGE_MIGRATION_FAILED: legacy runtime contains symlink {}",
            source.display()
        ));
    }
    if metadata.is_dir() {
        std::fs::create_dir_all(destination).map_err(|error| {
            format!(
                "CENTRAL_STORAGE_MIGRATION_FAILED: create staged directory {}: {error}",
                destination.display()
            )
        })?;
        for child in std::fs::read_dir(source).map_err(|error| {
            format!(
                "CENTRAL_STORAGE_MIGRATION_FAILED: list legacy runtime {}: {error}",
                source.display()
            )
        })? {
            let child = child.map_err(|error| {
                format!("CENTRAL_STORAGE_MIGRATION_FAILED: read legacy entry: {error}")
            })?;
            let child_source = child.path();
            let relative = child_source.strip_prefix(source_base).map_err(|error| {
                format!("CENTRAL_STORAGE_MIGRATION_FAILED: relative path: {error}")
            })?;
            if skip_legacy_runtime_path(relative) {
                continue;
            }
            copy_runtime_directory(
                &child_source,
                &destination.join(child.file_name()),
                source_base,
            )?;
        }
        return sync_parent_dir(destination).map_err(|error| {
            format!("CENTRAL_STORAGE_MIGRATION_FAILED: sync staged directory: {error}")
        });
    }
    if !metadata.is_file() {
        return Err(format!(
            "CENTRAL_STORAGE_MIGRATION_FAILED: unsupported legacy runtime file {}",
            source.display()
        ));
    }
    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent).map_err(|error| {
            format!(
                "CENTRAL_STORAGE_MIGRATION_FAILED: create staged file parent {}: {error}",
                parent.display()
            )
        })?;
    }
    std::fs::copy(source, destination).map_err(|error| {
        format!(
            "CENTRAL_STORAGE_MIGRATION_FAILED: copy legacy runtime {}: {error}",
            source.display()
        )
    })?;
    std::fs::File::open(destination)
        .and_then(|file| file.sync_all())
        .map_err(|error| format!("CENTRAL_STORAGE_MIGRATION_FAILED: sync copied file: {error}"))?;
    sync_parent_dir(destination).map_err(|error| {
        format!("CENTRAL_STORAGE_MIGRATION_FAILED: sync copied file parent: {error}")
    })
}

fn skip_legacy_runtime_path(relative: &Path) -> bool {
    matches!(
        relative.to_str(),
        Some(
            "server/journal.jsonl.tmp"
                | "server/daemon.lock"
                | "server/server.sock"
                | "server/server.pid"
                | "server/routes.jsonl"
                | "server/runtimes"
        )
    ) || relative.starts_with("server/runtimes")
}

fn publish_central_route_record(
    path: &Path,
    record: HostRouteRecord,
    publication: Option<&std::sync::Mutex<()>>,
) -> Result<(), String> {
    use std::io::Write;

    let _publication = publication.map(|gate| gate.lock().unwrap());
    let records = load_host_route_records(path)?;
    #[cfg(test)]
    central_publication_test_hook(&record.app_scope_id);
    let mut replaced = false;
    let mut output = Vec::new();
    for existing in records {
        let candidate = if existing.app_scope_id == record.app_scope_id
            && existing.project_scope == record.project_scope
        {
            replaced = true;
            &record
        } else {
            &existing
        };
        serde_json::to_writer(&mut output, candidate).map_err(|error| {
            format!("CENTRAL_STORAGE_MIGRATION_FAILED: serialize route: {error}")
        })?;
        output.push(b'\n');
    }
    if !replaced {
        serde_json::to_writer(&mut output, &record).map_err(|error| {
            format!("CENTRAL_STORAGE_MIGRATION_FAILED: serialize route: {error}")
        })?;
        output.push(b'\n');
    }

    let parent = path.parent().ok_or_else(|| {
        "CENTRAL_STORAGE_MIGRATION_FAILED: route journal has no parent".to_string()
    })?;
    std::fs::create_dir_all(parent).map_err(|error| {
        format!("CENTRAL_STORAGE_MIGRATION_FAILED: create route parent: {error}")
    })?;
    static ROUTE_SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let sequence = ROUTE_SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let temporary = path.with_file_name(format!(
        "routes.jsonl.migration-{}-{sequence}",
        std::process::id()
    ));
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
        .map_err(|error| {
            format!(
                "CENTRAL_STORAGE_MIGRATION_FAILED: create route temp {}: {error}",
                temporary.display()
            )
        })?;
    file.write_all(&output)
        .and_then(|_| file.sync_all())
        .map_err(|error| format!("CENTRAL_STORAGE_MIGRATION_FAILED: write route temp: {error}"))?;
    drop(file);
    std::fs::rename(&temporary, path)
        .map_err(|error| format!("CENTRAL_STORAGE_MIGRATION_FAILED: publish route: {error}"))?;
    sync_parent_dir(path)
        .map_err(|error| format!("CENTRAL_STORAGE_MIGRATION_FAILED: sync route parent: {error}"))?;
    load_host_route_records(path)
        .map(|_| ())
        .map_err(|error| format!("CENTRAL_STORAGE_MIGRATION_FAILED: verify route: {error}"))
}

#[cfg(test)]
static PUBLICATION_HOOK: std::sync::Mutex<
    Option<(
        String,
        Arc<(std::sync::atomic::AtomicBool, std::sync::atomic::AtomicBool)>,
    )>,
> = std::sync::Mutex::new(None);

#[cfg(test)]
fn central_publication_test_hook(app_scope_id: &str) {
    let hook = PUBLICATION_HOOK.lock().unwrap().clone();
    let Some((expected, progress)) = hook else {
        return;
    };
    if expected != app_scope_id {
        return;
    }
    progress.0.store(true, std::sync::atomic::Ordering::SeqCst);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    while !progress.1.load(std::sync::atomic::Ordering::SeqCst) {
        assert!(
            std::time::Instant::now() < deadline,
            "registration publication did not start"
        );
        std::thread::yield_now();
    }
    std::thread::sleep(std::time::Duration::from_millis(100));
}

#[cfg(test)]
mod central_storage_tests {
    use super::*;

    #[test]
    fn migration_preserves_a_divergent_valid_destination() {
        let root = std::env::temp_dir().join(format!(
            "collab-storage-conflict-{}-{}",
            std::process::id(),
            now_ms()
        ));
        let project = root.join("project");
        std::fs::create_dir_all(project.join(".agent-collab/server")).unwrap();
        let project = std::fs::canonicalize(project).unwrap();
        let paths = HostPaths::for_state_root(root.join("host")).unwrap();
        let key = (
            crate::identity::CLI_APP_SERVER_ID.to_string(),
            project.to_string_lossy().into_owned(),
        );
        let destination = paths.app_storage_root(&key.1, &key.0);
        let source_journal = project.join(".agent-collab/server/journal.jsonl");
        let destination_journal = destination.join(".agent-collab/server/journal.jsonl");
        std::fs::create_dir_all(destination_journal.parent().unwrap()).unwrap();
        std::fs::write(&source_journal, b"").unwrap();
        let existing = serde_json::to_vec(&Event::ReducerSnapshot {
            sequence: 0,
            revision: 0,
            events: Vec::new(),
        })
        .unwrap();
        std::fs::write(&destination_journal, &existing).unwrap();
        let error = migrate_legacy_runtime_storage(
            &paths,
            &paths.state_root().join("routes.jsonl"),
            &project,
            &key,
            &project,
            &destination,
        )
        .unwrap_err();
        assert!(error.starts_with("CENTRAL_STORAGE_CONFLICT:"), "{error}");
        assert_eq!(std::fs::read(&source_journal).unwrap(), b"");
        assert_eq!(std::fs::read(&destination_journal).unwrap(), existing);
        assert!(!paths.state_root().join("routes.jsonl").exists());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn migration_publication_retains_an_overlapping_registration() {
        let root = std::env::temp_dir().join(format!(
            "collab-storage-publication-{}-{}",
            std::process::id(),
            now_ms()
        ));
        let project = root.join("project");
        std::fs::create_dir_all(project.join(".agent-collab/server")).unwrap();
        let project = std::fs::canonicalize(project).unwrap();
        let paths = HostPaths::for_state_root(root.join("host")).unwrap();
        let route_journal = paths.state_root().join("routes.jsonl");
        let publication = Arc::new(std::sync::Mutex::new(()));
        let migration_app = "publication-migration-app";
        let registration_app = "publication-registration-app";
        let migrating_key = (
            migration_app.to_owned(),
            project.to_string_lossy().into_owned(),
        );
        append_host_route_record(
            &route_journal,
            &HostRouteRecord {
                version: 1,
                op: "register".into(),
                app_scope_id: "publication-existing-app".into(),
                project_scope: migrating_key.1.clone(),
                canonical_root: migrating_key.1.clone(),
                storage_root: project
                    .join("existing-storage")
                    .to_string_lossy()
                    .into_owned(),
                registered_ms: now_ms(),
            },
        )
        .unwrap();
        std::fs::write(project.join(".agent-collab/server/journal.jsonl"), b"").unwrap();

        let progress = Arc::new((
            std::sync::atomic::AtomicBool::new(false),
            std::sync::atomic::AtomicBool::new(false),
        ));
        *PUBLICATION_HOOK.lock().unwrap() = Some((migration_app.to_owned(), progress.clone()));
        let registration_path = route_journal.clone();
        let registration_gate = publication.clone();
        let registration_record = HostRouteRecord {
            version: 1,
            op: "register".into(),
            app_scope_id: registration_app.into(),
            project_scope: migrating_key.1.clone(),
            canonical_root: migrating_key.1.clone(),
            storage_root: project
                .join("registration-storage")
                .to_string_lossy()
                .into_owned(),
            registered_ms: now_ms(),
        };
        let registration = std::thread::spawn(move || {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
            while !progress.0.load(std::sync::atomic::Ordering::SeqCst) {
                assert!(
                    std::time::Instant::now() < deadline,
                    "migration did not load the route snapshot"
                );
                std::thread::yield_now();
            }
            progress.1.store(true, std::sync::atomic::Ordering::SeqCst);
            let _publication = registration_gate.lock().unwrap();
            append_host_route_record(&registration_path, &registration_record)
        });
        let destination = paths.app_storage_root(&migrating_key.1, migration_app);
        let result = migrate_legacy_runtime_storage_with_publication(
            &paths,
            &route_journal,
            &project,
            &migrating_key,
            &project,
            &destination,
            Some(&publication),
        );
        *PUBLICATION_HOOK.lock().unwrap() = None;
        result.expect("migration route publication");
        registration
            .join()
            .expect("registration thread")
            .expect("registration route publication");

        let records = load_host_route_records(&route_journal).unwrap();
        assert!(records
            .iter()
            .any(|record| record.app_scope_id == "publication-existing-app"));
        assert!(records
            .iter()
            .any(|record| record.app_scope_id == registration_app));
        assert!(records
            .iter()
            .any(|record| record.app_scope_id == migration_app));
        std::fs::remove_dir_all(root).unwrap();
    }
}
