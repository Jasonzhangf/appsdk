const AGENTTEAMS_BASELINE_ARCHIVE: &[u8] = include_bytes!(
    "../../../docs/evidence/a7cc2a4-pin-impl-20261003/fixture-agentteams-de099b79.tar.gz"
);

fn extract_agentteams_baseline(name: &str) -> PathBuf {
    let root = temp_root(name);
    fs::create_dir_all(&root).unwrap();
    let archive = root.with_extension("tar.gz");
    fs::write(&archive, AGENTTEAMS_BASELINE_ARCHIVE).unwrap();
    let output = Command::new("tar")
        .args([
            "-xzf",
            archive.to_str().unwrap(),
            "-C",
            root.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    fs::remove_file(archive).unwrap();
    assert!(root.join(".appsdk/maps/module-registry.json").is_file());
    root
}

fn historical_agentteams_inputs(root: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    [
        ".appsdk/migrations/0.1.5-to-0.1.6/record.json",
        ".appsdk/migrations/0.1.5-to-0.1.6/maps/resource-map.json",
        ".appsdk/migrations/0.1.5-to-0.1.6/maps/function-map.json",
        ".appsdk/migrations/0.1.5-to-0.1.6/maps/mainline-call-map.json",
        ".appsdk/migrations/0.1.5-to-0.1.6/maps/verification-map.json",
        ".appsdk/maps/resource-map.json",
        ".appsdk/maps/function-map.json",
        ".appsdk/maps/mainline-call-map.json",
        ".appsdk/maps/verification-map.json",
        ".appsdk/maps/module-registry.json",
    ]
    .iter()
    .map(|relative| {
        let path = root.join(relative);
        let bytes = fs::read(&path).unwrap();
        (path, bytes)
    })
    .collect()
}

fn assert_files_unchanged(snapshot: &[(PathBuf, Vec<u8>)]) {
    for (path, expected) in snapshot {
        assert_eq!(
            &fs::read(path).unwrap(),
            expected,
            "file changed: {}",
            path.display()
        );
    }
}

fn pin_lock_agentteams(root: &Path) -> std::process::Output {
    run(&[
        "pin-lock",
        root.to_str().unwrap(),
        "--binary",
        binary().to_str().unwrap(),
    ])
}

fn pinned_agentteams_fixture(name: &str) -> PathBuf {
    let root = extract_agentteams_baseline(name);
    let pinned = pin_lock_agentteams(&root);
    assert!(
        pinned.status.success(),
        "{}",
        String::from_utf8_lossy(&pinned.stderr)
    );
    root
}

fn assert_agentteams_pin_rejected(root: &Path, expected: &str) {
    let historical = historical_agentteams_inputs(root);
    let rejected = pin_lock_agentteams(root);
    assert!(
        !rejected.status.success(),
        "unexpected pin-lock success: {}",
        String::from_utf8_lossy(&rejected.stdout)
    );
    assert!(
        String::from_utf8_lossy(&rejected.stderr).contains(expected),
        "expected {expected}, stderr={}",
        String::from_utf8_lossy(&rejected.stderr)
    );
    assert_files_unchanged(&historical);
}

#[test]
fn pin_lock_accepts_complete_historical_agentteams_baseline() {
    let root = extract_agentteams_baseline("pin-lock-complete-agentteams-baseline");
    let historical = historical_agentteams_inputs(&root);

    let pinned = pin_lock_agentteams(&root);
    assert!(
        pinned.status.success(),
        "{}",
        String::from_utf8_lossy(&pinned.stderr)
    );
    assert_files_unchanged(&historical);

    assert!(root.join(".appsdk/migrations/0.1.0011-to-0.1.0012/record.json").is_file());
    let current_record: Value = serde_json::from_str(
        &fs::read_to_string(root.join(".appsdk/migrations/0.1.0012-to-0.1.0013/record.json"))
            .unwrap(),
    )
    .unwrap();
    let manifest: Value = serde_json::from_str(include_str!(
        "../../../contracts/migrations/sdk-0.1.0012-to-0.1.0013.json"
    ))
    .unwrap();
    for name in [
        "resource-map.json",
        "function-map.json",
        "mainline-call-map.json",
        "verification-map.json",
    ] {
        let declared = manifest["maps"]
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| entry["name"] == name)
            .unwrap();
        let recorded = current_record["maps"]
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| entry["name"] == name)
            .unwrap();
        let live_digest = file_digest(&root.join(".appsdk/maps").join(name));
        assert_eq!(recorded["source_digest"], live_digest);
        assert_eq!(recorded["target_digest"], live_digest);
        assert_eq!(
            recorded["canonical_source_digest"],
            declared["source_digest"]
        );
        assert_eq!(
            recorded["canonical_target_digest"],
            declared["target_digest"]
        );
    }

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn pin_lock_rejects_unlisted_historical_agentteams_target_tuple() {
    let root = extract_agentteams_baseline("pin-lock-agentteams-unlisted-target");
    let record_path = root.join(".appsdk/migrations/0.1.5-to-0.1.6/record.json");
    let mut record: Value = serde_json::from_slice(&fs::read(&record_path).unwrap()).unwrap();
    record["maps"][0]["canonical_target_digest"] =
        Value::String(format!("sha256:{}", "f".repeat(64)));
    fs::write(
        &record_path,
        serde_json::to_string_pretty(&record).unwrap() + "\n",
    )
    .unwrap();

    assert_agentteams_pin_rejected(&root, "INVALID_SDK_MIGRATION_RECORD");
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn pin_lock_rejects_absent_historical_agentteams_bundle_witness() {
    let root = extract_agentteams_baseline("pin-lock-agentteams-absent-witness");
    let lock_path = root.join(".appsdk/sdk.lock");
    let mut lock: Value = serde_json::from_slice(&fs::read(&lock_path).unwrap()).unwrap();
    lock["bundle_digest"] = Value::String(format!("sha256:{}", "a".repeat(64)));
    lock.as_object_mut()
        .unwrap()
        .remove("previous_bundle_digest");
    lock.as_object_mut()
        .unwrap()
        .remove("previous_bundle_digests");
    let tampered_lock = serde_json::to_string_pretty(&lock).unwrap() + "\n";
    fs::write(&lock_path, &tampered_lock).unwrap();

    assert_agentteams_pin_rejected(&root, "INVALID_SDK_MIGRATION_RECORD");
    assert_eq!(fs::read_to_string(&lock_path).unwrap(), tampered_lock);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn pin_lock_rejects_malformed_historical_agentteams_metadata() {
    let root = extract_agentteams_baseline("pin-lock-agentteams-malformed-metadata");
    let record_path = root.join(".appsdk/migrations/0.1.5-to-0.1.6/record.json");
    let mut record: Value = serde_json::from_slice(&fs::read(&record_path).unwrap()).unwrap();
    record["created_at"] = Value::String("not-a-timestamp".into());
    fs::write(
        &record_path,
        serde_json::to_string_pretty(&record).unwrap() + "\n",
    )
    .unwrap();

    assert_agentteams_pin_rejected(&root, "INVALID_SDK_MIGRATION_RECORD");
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn pin_lock_rejects_altered_historical_agentteams_snapshot() {
    let root = extract_agentteams_baseline("pin-lock-agentteams-altered-snapshot");
    let snapshot = root.join(".appsdk/migrations/0.1.5-to-0.1.6/maps/resource-map.json");
    fs::write(&snapshot, "{\"tampered\":true}\n").unwrap();

    assert_agentteams_pin_rejected(&root, "SDK_MIGRATION_SNAPSHOT_MISMATCH:resource-map.json");
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn pin_lock_rejects_changed_live_agentteams_map_after_current_record() {
    let root = pinned_agentteams_fixture("pin-lock-agentteams-changed-live-map");
    let live = root.join(".appsdk/maps/resource-map.json");
    let mut changed = fs::read(&live).unwrap();
    changed.push(b'\n');
    fs::write(&live, &changed).unwrap();
    let record_path = root.join(".appsdk/migrations/0.1.0012-to-0.1.0013/record.json");
    let record_before = fs::read(&record_path).unwrap();

    assert_agentteams_pin_rejected(&root, "SDK_MIGRATION_TARGET_MAP_MISMATCH:resource-map.json");
    assert_eq!(fs::read(&live).unwrap(), changed);
    assert_eq!(fs::read(&record_path).unwrap(), record_before);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn pin_lock_rejects_current_agentteams_target_live_mismatch() {
    let root = pinned_agentteams_fixture("pin-lock-agentteams-current-target-mismatch");
    let record_path = root.join(".appsdk/migrations/0.1.0012-to-0.1.0013/record.json");
    let mut record: Value = serde_json::from_slice(&fs::read(&record_path).unwrap()).unwrap();
    record["maps"][0]["target_digest"] = Value::String(format!("sha256:{}", "e".repeat(64)));
    let tampered_record = serde_json::to_string_pretty(&record).unwrap() + "\n";
    fs::write(&record_path, &tampered_record).unwrap();
    let live = root.join(".appsdk/maps/resource-map.json");
    let live_before = fs::read(&live).unwrap();

    assert_agentteams_pin_rejected(&root, "SDK_MIGRATION_TARGET_MAP_MISMATCH:resource-map.json");
    assert_eq!(fs::read_to_string(&record_path).unwrap(), tampered_record);
    assert_eq!(fs::read(&live).unwrap(), live_before);
    fs::remove_dir_all(root).unwrap();
}
