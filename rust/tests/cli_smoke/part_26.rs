const HISTORICAL_LEGACY_BUNDLE: &str =
    "sha256:4813363da77e9eec678bb7e4bbc8664beec920479aa61677c5991d4e45f82018";
const HISTORICAL_LOCK_BUNDLE: &str =
    "sha256:868fe60e645e4215c87a511c04f940589d7d1096e1835e8d444da44357a85bd4";
const HISTORICAL_TARGET_DIGESTS: [&str; 4] = [
    "sha256:373f7121a351f87c7126c7b163784190a0c5393edc9ddccb5c776a9c1224246b",
    "sha256:c0fbf20f6e697e3ea71d424f9d550eb22821eefeee2d7568447f2689a215c0d6",
    "sha256:d8964f67c3d7e51e5131a0a5fffc011462198231328f7e5644de5740095ca8f6",
    "sha256:f873ccad5a2590a1cec2a516a1ef9ebda36edab4023f18e4d69e211fe3ce7668",
];

fn repo_relative_text(relative: &str) -> String {
    fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join(relative)).unwrap()
}

fn write_legacy_migration_record(
    root: &Path,
    step: &str,
    bundle_digest: &str,
    target_digests: Option<[&str; 4]>,
) -> String {
    let manifest: Value = serde_json::from_str(&repo_relative_text(&format!(
        "contracts/migrations/sdk-{step}.json"
    )))
    .unwrap();
    let migration_root = root.join(".appsdk/migrations").join(step);
    fs::create_dir_all(migration_root.join("maps")).unwrap();
    let mut maps = Vec::new();
    for (index, name) in [
        "resource-map.json",
        "function-map.json",
        "mainline-call-map.json",
        "verification-map.json",
    ]
    .iter()
    .enumerate()
    {
        let declared = manifest["maps"]
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| entry["name"] == *name)
            .unwrap();
        let source = repo_relative_text(declared["source"].as_str().unwrap());
        fs::write(migration_root.join("maps").join(name), &source).unwrap();
        let target_digest = target_digests
            .map(|digests| digests[index].to_string())
            .unwrap_or_else(|| declared["target_digest"].as_str().unwrap().to_string());
        maps.push(serde_json::json!({
            "name": name,
            "source_digest": digest(&source),
            "target_digest": target_digest,
            "canonical_source_digest": Value::Null,
            "canonical_target_digest": Value::Null,
            "snapshot_path": format!(".appsdk/migrations/{step}/maps/{name}")
        }));
    }
    let record = serde_json::json!({
        "schema_version": 1,
        "migration_id": manifest["migration_id"],
        "source_version": manifest["source_version"],
        "target_version": manifest["target_version"],
        "bundle_digest": bundle_digest,
        "maps": maps,
        "frozen_reviews": [],
        "legacy_reconciled_reviews": [],
        "created_at": "2026-01-01T00:00:00Z"
    });
    let record_text = serde_json::to_string_pretty(&record).unwrap() + "\n";
    fs::write(migration_root.join("record.json"), &record_text).unwrap();
    record_text
}

fn set_sdk_lock_bundle(root: &Path, bundle_digest: &str) {
    let lock_path = root.join(".appsdk/sdk.lock");
    let mut lock: Value = serde_json::from_str(&fs::read_to_string(&lock_path).unwrap()).unwrap();
    lock["bundle_digest"] = Value::String(bundle_digest.to_string());
    lock.as_object_mut()
        .unwrap()
        .remove("previous_bundle_digest");
    lock.as_object_mut()
        .unwrap()
        .remove("previous_bundle_digests");
    fs::write(
        &lock_path,
        serde_json::to_string_pretty(&lock).unwrap() + "\n",
    )
    .unwrap();
}

fn snapshot_paths(root: &Path, relative_paths: &[&str]) -> Vec<(PathBuf, Vec<u8>)> {
    let mut snapshot = Vec::new();
    for relative in relative_paths {
        let path = root.join(relative);
        if path.is_file() {
            snapshot.push((path.clone(), fs::read(&path).unwrap()));
        } else if path.is_dir() {
            let mut pending = vec![path];
            while let Some(directory) = pending.pop() {
                let mut entries: Vec<PathBuf> = fs::read_dir(&directory)
                    .unwrap()
                    .map(|entry| entry.unwrap().path())
                    .collect();
                entries.sort();
                for entry in entries {
                    if entry.is_dir() {
                        pending.push(entry);
                    } else {
                        snapshot.push((entry.clone(), fs::read(&entry).unwrap()));
                    }
                }
            }
        }
    }
    snapshot.sort_by(|left, right| left.0.cmp(&right.0));
    snapshot
}

fn snapshot_appsdk_tree(root: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    snapshot_paths(root, &[".appsdk"])
}

fn assert_snapshot_unchanged(snapshot: &[(PathBuf, Vec<u8>)]) {
    for (path, expected) in snapshot {
        assert_eq!(
            &fs::read(path).unwrap(),
            expected,
            "file changed: {}",
            path.display()
        );
    }
}

fn assert_tree_unchanged(root: &Path, snapshot: &[(PathBuf, Vec<u8>)]) {
    assert_snapshot_unchanged(snapshot);
    assert_eq!(
        snapshot_appsdk_tree(root).len(),
        snapshot.len(),
        "unexpected governance file written"
    );
}

fn proven_historical_fixture(name: &str) -> (PathBuf, PathBuf) {
    let root = temp_root(name);
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());
    let record_path = root.join(".appsdk/migrations/0.1.5-to-0.1.6/record.json");
    let _ = write_legacy_migration_record(
        &root,
        "0.1.5-to-0.1.6",
        HISTORICAL_LEGACY_BUNDLE,
        Some(HISTORICAL_TARGET_DIGESTS),
    );
    set_sdk_lock_bundle(&root, HISTORICAL_LOCK_BUNDLE);
    (root, record_path)
}

fn declared_map_digest(step: &str, name: &str, field: &str) -> String {
    let manifest: Value = serde_json::from_str(&repo_relative_text(&format!(
        "contracts/migrations/sdk-{step}.json"
    )))
    .unwrap();
    manifest["maps"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["name"] == name)
        .unwrap()[field]
        .as_str()
        .unwrap()
        .to_string()
}

fn assert_paths_unchanged(root: &Path, relative_paths: &[&str], snapshot: &[(PathBuf, Vec<u8>)]) {
    assert_snapshot_unchanged(snapshot);
    assert_eq!(
        snapshot_paths(root, relative_paths).len(),
        snapshot.len(),
        "unexpected governance file written"
    );
}

fn corrupt_historical_target(record_path: &Path) {
    let mut record: Value =
        serde_json::from_str(&fs::read_to_string(record_path).unwrap()).unwrap();
    record["maps"][0]["target_digest"] = Value::String(format!("sha256:{}", "f".repeat(64)));
    fs::write(
        record_path,
        serde_json::to_string_pretty(&record).unwrap() + "\n",
    )
    .unwrap();
}

#[test]
fn pin_lock_accepts_proven_historical_bundle_without_lock_anchor() {
    let (root, record_path) = proven_historical_fixture("pin-lock-proven-historical-bundle");
    let root_text = root.to_str().unwrap();
    let original_record = fs::read_to_string(&record_path).unwrap();
    let preserved = snapshot_paths(
        &root,
        &[".appsdk/migrations", ".appsdk/maps", ".appsdk/records"],
    );

    let first = run(&[
        "pin-lock",
        root_text,
        "--binary",
        binary().to_str().unwrap(),
    ]);
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    assert_eq!(fs::read_to_string(&record_path).unwrap(), original_record);
    assert_snapshot_unchanged(&preserved);

    let lock_path = root.join(".appsdk/sdk.lock");
    let lock: Value = serde_json::from_str(&fs::read_to_string(&lock_path).unwrap()).unwrap();
    let witnesses = lock["previous_bundle_digests"].as_array().unwrap();
    assert!(witnesses
        .iter()
        .any(|digest| digest.as_str() == Some(HISTORICAL_LEGACY_BUNDLE)));
    assert_eq!(lock["previous_bundle_digest"], HISTORICAL_LEGACY_BUNDLE);
    assert!(run(&["verify", root_text]).status.success());

    let first_lock = fs::read_to_string(&lock_path).unwrap();
    let second = run(&[
        "pin-lock",
        root_text,
        "--binary",
        binary().to_str().unwrap(),
    ]);
    assert!(
        second.status.success(),
        "{}",
        String::from_utf8_lossy(&second.stderr)
    );
    assert_eq!(fs::read_to_string(&lock_path).unwrap(), first_lock);
    assert_eq!(fs::read_to_string(&record_path).unwrap(), original_record);
    assert_snapshot_unchanged(&preserved);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn pin_lock_rejects_unknown_historical_bundle_while_other_record_anchored() {
    let root = temp_root("pin-lock-unknown-historical-bundle-anchored");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());
    let anchored_bundle = format!("sha256:{}", "1".repeat(64));
    let unknown_bundle = format!("sha256:{}", "2".repeat(64));
    let _ = write_legacy_migration_record(&root, "0.1.5-to-0.1.6", &anchored_bundle, None);
    set_sdk_lock_bundle(&root, &anchored_bundle);
    let _ = write_legacy_migration_record(&root, "0.1.6-to-0.1.0007", &unknown_bundle, None);
    let before = snapshot_appsdk_tree(&root);

    let rejected = run(&[
        "pin-lock",
        root_text,
        "--binary",
        binary().to_str().unwrap(),
    ]);
    assert!(!rejected.status.success());
    assert!(
        String::from_utf8_lossy(&rejected.stderr).contains("SDK_MIGRATION_BUNDLE_WITNESS_REQUIRED")
    );
    assert_tree_unchanged(&root, &before);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn pin_lock_rejects_unsupported_historical_bundle_without_anchor() {
    let root = temp_root("pin-lock-unsupported-historical-bundle");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());
    let unsupported_bundle = format!("sha256:{}", "2".repeat(64));
    let _ = write_legacy_migration_record(&root, "0.1.5-to-0.1.6", &unsupported_bundle, None);
    set_sdk_lock_bundle(&root, HISTORICAL_LOCK_BUNDLE);
    let before = snapshot_appsdk_tree(&root);

    let rejected = run(&[
        "pin-lock",
        root_text,
        "--binary",
        binary().to_str().unwrap(),
    ]);
    assert!(!rejected.status.success());
    assert!(
        String::from_utf8_lossy(&rejected.stderr).contains("SDK_MIGRATION_BUNDLE_WITNESS_REQUIRED")
    );
    assert_tree_unchanged(&root, &before);
    fs::remove_dir_all(root).unwrap();
}

fn assert_rejected_historical_target(name: &str, mutate: fn(&mut Value)) {
    let (root, record_path) = proven_historical_fixture(name);
    let mut record: Value =
        serde_json::from_str(&fs::read_to_string(&record_path).unwrap()).unwrap();
    mutate(&mut record);
    fs::write(
        &record_path,
        serde_json::to_string_pretty(&record).unwrap() + "\n",
    )
    .unwrap();
    let before = snapshot_appsdk_tree(&root);

    let rejected = run(&[
        "pin-lock",
        root.to_str().unwrap(),
        "--binary",
        binary().to_str().unwrap(),
    ]);
    assert!(!rejected.status.success());
    assert!(
        String::from_utf8_lossy(&rejected.stderr)
            .contains("SDK_MIGRATION_TARGET_MAP_MISMATCH:resource-map.json"),
        "stderr={}",
        String::from_utf8_lossy(&rejected.stderr)
    );
    assert_tree_unchanged(&root, &before);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn pin_lock_rejects_unknown_or_cross_source_historical_target_tuple() {
    assert_rejected_historical_target("pin-lock-unknown-historical-target", |record| {
        record["maps"][0]["target_digest"] =
            Value::String(format!("sha256:{}", "f".repeat(64)));
    });
    assert_rejected_historical_target("pin-lock-cross-source-historical-target", |record| {
        let other_target = record["maps"][1]["target_digest"].clone();
        record["maps"][0]["target_digest"] = other_target;
    });
}

#[test]
fn pin_lock_rejects_altered_or_symlinked_historical_snapshot() {
    let (root, _) = proven_historical_fixture("pin-lock-altered-historical-snapshot");
    let snapshot_path = root.join(".appsdk/migrations/0.1.5-to-0.1.6/maps/resource-map.json");
    fs::write(&snapshot_path, "{\"tampered\":true}\n").unwrap();
    let before = snapshot_appsdk_tree(&root);
    let rejected = run(&[
        "pin-lock",
        root.to_str().unwrap(),
        "--binary",
        binary().to_str().unwrap(),
    ]);
    assert!(!rejected.status.success());
    assert!(
        String::from_utf8_lossy(&rejected.stderr)
            .contains("SDK_MIGRATION_SNAPSHOT_MISMATCH:resource-map.json")
    );
    assert_tree_unchanged(&root, &before);
    fs::remove_dir_all(root).unwrap();

    let (root, _) = proven_historical_fixture("pin-lock-symlinked-historical-snapshot");
    let snapshot_path = root.join(".appsdk/migrations/0.1.5-to-0.1.6/maps/resource-map.json");
    let original = snapshot_path.with_extension("original");
    fs::rename(&snapshot_path, &original).unwrap();
    symlink(&original, &snapshot_path).unwrap();
    let before = snapshot_appsdk_tree(&root);
    let rejected = run(&[
        "pin-lock",
        root.to_str().unwrap(),
        "--binary",
        binary().to_str().unwrap(),
    ]);
    assert!(!rejected.status.success());
    assert!(
        String::from_utf8_lossy(&rejected.stderr)
            .contains("ARTIFACT_PATH_SYMLINK:sdk_migration_snapshot")
    );
    assert_tree_unchanged(&root, &before);
    fs::remove_dir_all(root).unwrap();
}

fn assert_rejected_historical_record(name: &str, mutate: fn(&mut Value)) {
    let (root, record_path) = proven_historical_fixture(name);
    let mut record: Value =
        serde_json::from_str(&fs::read_to_string(&record_path).unwrap()).unwrap();
    mutate(&mut record);
    fs::write(
        &record_path,
        serde_json::to_string_pretty(&record).unwrap() + "\n",
    )
    .unwrap();
    let before = snapshot_appsdk_tree(&root);

    let rejected = run(&[
        "pin-lock",
        root.to_str().unwrap(),
        "--binary",
        binary().to_str().unwrap(),
    ]);
    assert!(!rejected.status.success());
    assert!(
        String::from_utf8_lossy(&rejected.stderr).contains("INVALID_SDK_MIGRATION_RECORD"),
        "stderr={}",
        String::from_utf8_lossy(&rejected.stderr)
    );
    assert_tree_unchanged(&root, &before);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn pin_lock_rejects_invalid_historical_record_metadata_and_digest() {
    assert_rejected_historical_record("pin-lock-invalid-historical-metadata", |record| {
        record["created_at"] = Value::String("not-a-timestamp".into());
    });
    assert_rejected_historical_record("pin-lock-missing-historical-map-digest", |record| {
        record["maps"][0].as_object_mut().unwrap().remove("source_digest");
    });
    assert_rejected_historical_record("pin-lock-malformed-historical-bundle-digest", |record| {
        record["bundle_digest"] = Value::String("sha256:not-a-digest".into());
    });
}

#[test]
fn pin_lock_rejects_live_map_mismatch_after_historical_recovery() {
    let (root, _) = proven_historical_fixture("pin-lock-live-mismatch-after-historical");
    let root_text = root.to_str().unwrap();
    let pinned = run(&[
        "pin-lock",
        root_text,
        "--binary",
        binary().to_str().unwrap(),
    ]);
    assert!(
        pinned.status.success(),
        "{}",
        String::from_utf8_lossy(&pinned.stderr)
    );
    write_matching_old_authoring_bundle_manifest(&root);
    let live_map = root.join(".appsdk/maps/resource-map.json");
    fs::write(&live_map, "{\"tampered\":true}\n").unwrap();
    let before = snapshot_paths(&root, &["contracts", ".appsdk"]);

    let rejected = run(&[
        "pin-lock",
        root_text,
        "--binary",
        binary().to_str().unwrap(),
    ]);
    assert!(!rejected.status.success());
    assert!(
        String::from_utf8_lossy(&rejected.stderr)
            .contains("SDK_MIGRATION_LIVE_MAP_UNRECONCILED:resource-map.json")
    );
    assert_paths_unchanged(&root, &["contracts", ".appsdk"], &before);
    let root_manifest = fs::read_to_string(root.join("contracts/sdk-bundle.manifest.json")).unwrap();
    assert!(
        root_manifest.contains("0.1.0014"),
        "root authoring manifest was rewritten: {root_manifest}"
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn pin_lock_rejects_binary_mismatch_before_historical_recovery_writes() {
    let (root, _) = proven_historical_fixture("pin-lock-historical-binary-mismatch");
    let wrong_binary = root.join("wrong-appsdk");
    fs::write(&wrong_binary, "not the running AppSDK bundle\n").unwrap();
    let before = snapshot_appsdk_tree(&root);

    let rejected = run(&[
        "pin-lock",
        root.to_str().unwrap(),
        "--binary",
        wrong_binary.to_str().unwrap(),
    ]);
    assert!(!rejected.status.success());
    assert!(String::from_utf8_lossy(&rejected.stderr).contains("SDK_PIN_BINARY_BUNDLE_MISMATCH"));
    assert_tree_unchanged(&root, &before);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn pin_lock_rejects_mixed_historical_current_target_tuple() {
    let (root, record_path) = proven_historical_fixture("pin-lock-mixed-historical-current-target");
    // Keep the other three proven historical targets intact and replace only
    // resource-map's target with the current manifest target. The unanchored
    // historical record must be validated as one complete tuple.
    let mut record: Value =
        serde_json::from_str(&fs::read_to_string(&record_path).unwrap()).unwrap();
    record["maps"][0]["target_digest"] = Value::String(declared_map_digest(
        "0.1.5-to-0.1.6",
        "resource-map.json",
        "target_digest",
    ));
    fs::write(
        &record_path,
        serde_json::to_string_pretty(&record).unwrap() + "\n",
    )
    .unwrap();
    let before = snapshot_appsdk_tree(&root);

    let rejected = run(&[
        "pin-lock",
        root.to_str().unwrap(),
        "--binary",
        binary().to_str().unwrap(),
    ]);
    assert!(!rejected.status.success());
    assert!(
        String::from_utf8_lossy(&rejected.stderr)
            .contains("SDK_MIGRATION_TARGET_MAP_MISMATCH:resource-map.json"),
        "stderr={}",
        String::from_utf8_lossy(&rejected.stderr)
    );
    assert_tree_unchanged(&root, &before);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn pin_lock_rejects_explicit_custom_binding_on_unanchored_historical_record() {
    let (root, record_path) = proven_historical_fixture("pin-lock-custom-binding-unanchored");
    // An explicit custom canonical binding is not the proven legacy tuple, so it
    // must not authorize an unanchored historical record.
    let mut record: Value =
        serde_json::from_str(&fs::read_to_string(&record_path).unwrap()).unwrap();
    record["maps"][0]["canonical_source_digest"] = Value::String(declared_map_digest(
        "0.1.5-to-0.1.6",
        "resource-map.json",
        "source_digest",
    ));
    record["maps"][0]["canonical_target_digest"] = Value::String(declared_map_digest(
        "0.1.5-to-0.1.6",
        "resource-map.json",
        "target_digest",
    ));
    // Make the custom binding self-consistent (actual target equals the custom
    // canonical target) so it is exactly the shape the old allowances accepted.
    record["maps"][0]["target_digest"] = Value::String(declared_map_digest(
        "0.1.5-to-0.1.6",
        "resource-map.json",
        "target_digest",
    ));
    fs::write(
        &record_path,
        serde_json::to_string_pretty(&record).unwrap() + "\n",
    )
    .unwrap();
    let before = snapshot_appsdk_tree(&root);

    let rejected = run(&[
        "pin-lock",
        root.to_str().unwrap(),
        "--binary",
        binary().to_str().unwrap(),
    ]);
    assert!(!rejected.status.success());
    assert!(
        String::from_utf8_lossy(&rejected.stderr)
            .contains("SDK_MIGRATION_TARGET_MAP_MISMATCH:resource-map.json"),
        "stderr={}",
        String::from_utf8_lossy(&rejected.stderr)
    );
    assert_tree_unchanged(&root, &before);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn malformed_historical_record_cannot_seed_init_witness_or_unsupported_step() {
    let root = temp_root("malformed-historical-init-witness");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());
    let record_path = root.join(".appsdk/migrations/0.1.5-to-0.1.6/record.json");
    let _ = write_legacy_migration_record(
        &root,
        "0.1.5-to-0.1.6",
        HISTORICAL_LEGACY_BUNDLE,
        Some(HISTORICAL_TARGET_DIGESTS),
    );
    set_sdk_lock_bundle(&root, HISTORICAL_LOCK_BUNDLE);
    corrupt_historical_target(&record_path);

    let lock_path = root.join(".appsdk/sdk.lock");
    let lock_before = fs::read(&lock_path).unwrap();

    // The ordinary init lock writer must reject the malformed record before it
    // persists the record's bundle as a durable witness.
    let initialized = run_ordinary_init_without_collab(&root);
    assert!(
        !initialized.status.success(),
        "init unexpectedly succeeded: {}",
        String::from_utf8_lossy(&initialized.stdout)
    );
    assert!(
        String::from_utf8_lossy(&initialized.stderr)
            .contains("SDK_MIGRATION_TARGET_MAP_MISMATCH:resource-map.json"),
        "stderr={}",
        String::from_utf8_lossy(&initialized.stderr)
    );
    assert_eq!(
        fs::read(&lock_path).unwrap(),
        lock_before,
        "init persisted a witness for a malformed historical record"
    );
    let lock: Value = serde_json::from_slice(&lock_before).unwrap();
    assert!(lock.get("previous_bundle_digest").is_none());
    assert!(lock.get("previous_bundle_digests").is_none());

    // Removing the malformed record leaves no seeded witness, so an unrelated
    // unsupported step carrying the same historical bundle is still rejected.
    fs::remove_dir_all(root.join(".appsdk/migrations/0.1.5-to-0.1.6")).unwrap();
    let _ = write_legacy_migration_record(
        &root,
        "0.1.6-to-0.1.0007",
        HISTORICAL_LEGACY_BUNDLE,
        None,
    );
    let rejected = run(&[
        "pin-lock",
        root_text,
        "--binary",
        binary().to_str().unwrap(),
    ]);
    assert!(!rejected.status.success());
    assert!(
        String::from_utf8_lossy(&rejected.stderr).contains("SDK_MIGRATION_BUNDLE_WITNESS_REQUIRED"),
        "stderr={}",
        String::from_utf8_lossy(&rejected.stderr)
    );
    fs::remove_dir_all(&root).unwrap();
    let _ = fs::remove_dir_all(test_global_registry_root_for_project(&root));
}

fn write_matching_old_authoring_bundle_manifest(root: &Path) {
    let mut old: Value =
        serde_json::from_str(include_str!("../../../contracts/sdk-bundle.manifest.json")).unwrap();
    old["version"] = Value::String("0.1.0014".into());
    let text = serde_json::to_string_pretty(&old).unwrap() + "\n";
    fs::create_dir_all(root.join("contracts")).unwrap();
    fs::write(root.join("contracts/sdk-bundle.manifest.json"), &text).unwrap();
    fs::write(root.join(".appsdk/contracts/sdk-bundle.manifest.json"), &text).unwrap();
}

#[test]
fn pin_lock_rejects_corrupt_history_before_rewriting_root_authoring_manifest() {
    let (root, record_path) = proven_historical_fixture("pin-lock-corrupt-history-root-authoring");
    // A matching old root authoring manifest and installed manifest would make
    // `reconcile_authoring_bundle_manifest` rewrite the root file. A corrupt
    // historical record must be rejected before that write.
    write_matching_old_authoring_bundle_manifest(&root);
    corrupt_historical_target(&record_path);
    let before = snapshot_paths(&root, &["contracts", ".appsdk"]);

    let rejected = run(&[
        "pin-lock",
        root.to_str().unwrap(),
        "--binary",
        binary().to_str().unwrap(),
    ]);
    assert!(!rejected.status.success());
    assert!(
        String::from_utf8_lossy(&rejected.stderr)
            .contains("SDK_MIGRATION_TARGET_MAP_MISMATCH:resource-map.json"),
        "stderr={}",
        String::from_utf8_lossy(&rejected.stderr)
    );
    assert_paths_unchanged(&root, &["contracts", ".appsdk"], &before);
    let root_manifest = fs::read_to_string(root.join("contracts/sdk-bundle.manifest.json")).unwrap();
    assert!(
        root_manifest.contains("0.1.0014"),
        "root authoring manifest was rewritten: {root_manifest}"
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn pin_lock_rejects_missing_lock_with_history_before_rewriting_root_authoring_manifest() {
    let (root, _) = proven_historical_fixture("pin-lock-missing-lock-root-authoring");
    write_matching_old_authoring_bundle_manifest(&root);
    fs::remove_file(root.join(".appsdk/sdk.lock")).unwrap();
    let before = snapshot_paths(&root, &["contracts", ".appsdk"]);

    let rejected = run(&[
        "pin-lock",
        root.to_str().unwrap(),
        "--binary",
        binary().to_str().unwrap(),
    ]);
    assert!(!rejected.status.success());
    assert!(
        String::from_utf8_lossy(&rejected.stderr)
            .contains("SDK_MIGRATION_BUNDLE_WITNESS_REQUIRED"),
        "stderr={}",
        String::from_utf8_lossy(&rejected.stderr)
    );
    assert_paths_unchanged(&root, &["contracts", ".appsdk"], &before);
    let root_manifest = fs::read_to_string(root.join("contracts/sdk-bundle.manifest.json")).unwrap();
    assert!(
        root_manifest.contains("0.1.0014"),
        "root authoring manifest was rewritten: {root_manifest}"
    );
    fs::remove_dir_all(root).unwrap();
}

const CURRENT_MIGRATION_STEP: &str = "0.1.0014-to-0.1.0015";
const CURRENT_MIGRATION_MAPS: [&str; 4] = [
    "resource-map.json",
    "function-map.json",
    "mainline-call-map.json",
    "verification-map.json",
];

fn current_step_declared(name: &str, field: &str) -> String {
    let manifest: Value = serde_json::from_str(&repo_relative_text(
        "contracts/migrations/sdk-0.1.0014-to-0.1.0015.json",
    ))
    .unwrap();
    manifest["maps"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["name"] == name)
        .unwrap()[field]
        .as_str()
        .unwrap()
        .to_string()
}

fn current_bundle_digest(root: &Path) -> String {
    let lock: Value =
        serde_json::from_str(&fs::read_to_string(root.join(".appsdk/sdk.lock")).unwrap()).unwrap();
    lock["bundle_digest"].as_str().unwrap().to_string()
}

/// Write a `0.1.0014-to-0.1.0015` migration record.
///
/// When `custom` is set the record keeps explicit custom canonical bindings whose
/// actual source and target are the preserved `0.1.0009` governance maps, so a
/// canonical `0.1.0014` source replay would overwrite a custom live map; this
/// mirrors the historical fixture's custom current-step record. Otherwise it is
/// the ordinary canonical record for the declared transition.
fn write_current_step_record(root: &Path, custom: bool) {
    let migration_root = root.join(".appsdk/migrations").join(CURRENT_MIGRATION_STEP);
    fs::create_dir_all(migration_root.join("maps")).unwrap();
    let mut maps = Vec::new();
    for name in CURRENT_MIGRATION_MAPS {
        let declared_source = current_step_declared(name, "source_digest");
        let declared_target = current_step_declared(name, "target_digest");
        let source_text = repo_relative_text(&format!(
            "contracts/migrations/0.1.0014/governance-maps/{name}"
        ));
        let (snapshot_text, source_digest, target_digest, canonical_source, canonical_target) =
            if custom {
                let custom_text = repo_relative_text(&format!(
                    "contracts/migrations/0.1.0009/governance-maps/{name}"
                ));
                let custom_digest = digest(&custom_text);
                (
                    custom_text,
                    custom_digest.clone(),
                    custom_digest,
                    Value::String(declared_source),
                    Value::String(declared_target),
                )
            } else {
                (
                    source_text,
                    declared_source,
                    declared_target,
                    Value::Null,
                    Value::Null,
                )
            };
        fs::write(migration_root.join("maps").join(name), &snapshot_text).unwrap();
        maps.push(serde_json::json!({
            "name": name,
            "source_digest": source_digest,
            "target_digest": target_digest,
            "canonical_source_digest": canonical_source,
            "canonical_target_digest": canonical_target,
            "snapshot_path": format!(".appsdk/migrations/{CURRENT_MIGRATION_STEP}/maps/{name}")
        }));
    }
    let record = serde_json::json!({
        "schema_version": 1,
        "migration_id": format!("appsdk-{CURRENT_MIGRATION_STEP}"),
        "source_version": "0.1.0014",
        "target_version": "0.1.0015",
        "bundle_digest": current_bundle_digest(root),
        "maps": maps,
        "frozen_reviews": [],
        "legacy_reconciled_reviews": [],
        "created_at": "2026-01-01T00:00:00Z"
    });
    fs::write(
        migration_root.join("record.json"),
        serde_json::to_string_pretty(&record).unwrap() + "\n",
    )
    .unwrap();
}

fn reset_live_maps_to_0014_source(root: &Path) {
    for name in CURRENT_MIGRATION_MAPS {
        let source = repo_relative_text(&format!(
            "contracts/migrations/0.1.0014/governance-maps/{name}"
        ));
        fs::write(root.join(".appsdk/maps").join(name), source).unwrap();
    }
}

#[test]
fn pin_lock_rejects_custom_current_record_source_replay_before_writes() {
    let root = temp_root("pin-lock-custom-current-source-replay");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());
    write_current_step_record(&root, true);
    reset_live_maps_to_0014_source(&root);
    write_matching_old_authoring_bundle_manifest(&root);
    let before = snapshot_paths(&root, &["contracts", ".appsdk"]);

    let rejected = run(&[
        "pin-lock",
        root_text,
        "--binary",
        binary().to_str().unwrap(),
    ]);
    assert!(!rejected.status.success());
    assert!(
        String::from_utf8_lossy(&rejected.stderr)
            .contains("SDK_MIGRATION_TARGET_MAP_MISMATCH:resource-map.json"),
        "stderr={}",
        String::from_utf8_lossy(&rejected.stderr)
    );
    assert_paths_unchanged(&root, &["contracts", ".appsdk"], &before);
    let root_manifest = fs::read_to_string(root.join("contracts/sdk-bundle.manifest.json")).unwrap();
    assert!(
        root_manifest.contains("0.1.0014"),
        "root authoring manifest was rewritten: {root_manifest}"
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn pin_lock_replays_canonical_current_record_from_0014_source() {
    let root = temp_root("pin-lock-canonical-current-source-replay");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());
    write_current_step_record(&root, false);
    reset_live_maps_to_0014_source(&root);
    let record_path = root
        .join(".appsdk/migrations")
        .join(CURRENT_MIGRATION_STEP)
        .join("record.json");
    let original_record = fs::read_to_string(&record_path).unwrap();

    let pinned = run(&[
        "pin-lock",
        root_text,
        "--binary",
        binary().to_str().unwrap(),
    ]);
    assert!(
        pinned.status.success(),
        "{}",
        String::from_utf8_lossy(&pinned.stderr)
    );
    for name in CURRENT_MIGRATION_MAPS {
        assert_eq!(
            file_digest(&root.join(".appsdk/maps").join(name)),
            current_step_declared(name, "target_digest"),
            "live map {name} was not migrated to the canonical target"
        );
    }
    assert_eq!(fs::read_to_string(&record_path).unwrap(), original_record);
    fs::remove_dir_all(root).unwrap();
}
