use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

#[test]
fn pin_lock_accepts_current_0015_without_creating_migration_record() {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = std::env::temp_dir().join(format!(
        "appsdk-current-0015-{}-{nonce}",
        std::process::id()
    ));
    let project = root.join("project");
    let registry = root.join("registry");
    let binary = env!("CARGO_BIN_EXE_appsdk");
    let created = Command::new(binary)
        .args(["new", project.to_str().unwrap()])
        .env("APPSDK_HOME", &registry)
        .output()
        .unwrap();
    assert!(
        created.status.success(),
        "{}",
        String::from_utf8_lossy(&created.stderr)
    );

    let pinned = Command::new(binary)
        .args(["pin-lock", project.to_str().unwrap(), "--binary", binary])
        .env("APPSDK_HOME", &registry)
        .output()
        .unwrap();
    assert!(
        pinned.status.success(),
        "{}",
        String::from_utf8_lossy(&pinned.stderr)
    );
    let lock: Value =
        serde_json::from_slice(&fs::read(project.join(".appsdk/sdk.lock")).unwrap()).unwrap();
    assert_eq!(lock["version"], "0.1.0015");
    assert!(!project
        .join(".appsdk/migrations/0.1.0011-to-0.1.0012/record.json")
        .exists());
    assert!(!project
        .join(".appsdk/migrations/0.1.0012-to-0.1.0013/record.json")
        .exists());
    assert!(!project
        .join(".appsdk/migrations/0.1.0013-to-0.1.0014/record.json")
        .exists());
    assert!(!project
        .join(".appsdk/migrations/0.1.0014-to-0.1.0015/record.json")
        .exists());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn pin_lock_preserves_0008_maps_when_upgrading_to_0009() {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = std::env::temp_dir().join(format!(
        "appsdk-0008-to-0009-{}-{nonce}",
        std::process::id()
    ));
    let project = root.join("project");
    let registry = root.join("registry");
    let binary = env!("CARGO_BIN_EXE_appsdk");
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let created = Command::new(binary)
        .args(["new", project.to_str().unwrap()])
        .env("APPSDK_HOME", &registry)
        .output()
        .unwrap();
    assert!(
        created.status.success(),
        "{}",
        String::from_utf8_lossy(&created.stderr)
    );

    let project_path = project.join(".appsdk/project.json");
    let mut contract: Value = serde_json::from_slice(&fs::read(&project_path).unwrap()).unwrap();
    contract["sdk"]["version"] = Value::String("0.1.0008".into());
    fs::write(
        &project_path,
        serde_json::to_string_pretty(&contract).unwrap() + "\n",
    )
    .unwrap();
    for name in [
        "resource-map.json",
        "function-map.json",
        "mainline-call-map.json",
        "verification-map.json",
    ] {
        fs::copy(
            repo.join("contracts/migrations/0.1.0008/governance-maps")
                .join(name),
            project.join(".appsdk/maps").join(name),
        )
        .unwrap();
    }

    let pinned = Command::new(binary)
        .args(["pin-lock", project.to_str().unwrap(), "--binary", binary])
        .env("APPSDK_HOME", &registry)
        .output()
        .unwrap();
    assert!(
        pinned.status.success(),
        "{}",
        String::from_utf8_lossy(&pinned.stderr)
    );
    let record: Value = serde_json::from_slice(
        &fs::read(project.join(".appsdk/migrations/0.1.0008-to-0.1.0009/record.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(record["source_version"], "0.1.0008");
    assert_eq!(record["target_version"], "0.1.0009");
    for name in [
        "resource-map.json",
        "function-map.json",
        "mainline-call-map.json",
        "verification-map.json",
    ] {
        assert_eq!(
            fs::read(
                project
                    .join(".appsdk/migrations/0.1.0008-to-0.1.0009/maps")
                    .join(name)
            )
            .unwrap(),
            fs::read(
                repo.join("contracts/migrations/0.1.0008/governance-maps")
                    .join(name)
            )
            .unwrap()
        );
        assert_eq!(
            fs::read(project.join(".appsdk/maps").join(name)).unwrap(),
            fs::read(repo.join("contracts/maps").join(name)).unwrap()
        );
    }
    fs::remove_dir_all(root).unwrap();
}

const GOVERNANCE_MAP_NAMES: [&str; 4] = [
    "resource-map.json",
    "function-map.json",
    "mainline-call-map.json",
    "verification-map.json",
];

fn repo_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap()
}

fn create_current_consumer(binary: &str, root: &Path) -> (PathBuf, PathBuf) {
    let project = root.join("project");
    let registry = root.join("registry");
    let created = Command::new(binary)
        .args(["new", project.to_str().unwrap()])
        .env("APPSDK_HOME", &registry)
        .output()
        .unwrap();
    assert!(
        created.status.success(),
        "{}",
        String::from_utf8_lossy(&created.stderr)
    );
    (project, registry)
}

// Build a consumer whose project pin and live maps describe a historical
// source version, so pin-lock exercises the real migration chain from it.
fn create_historical_consumer(binary: &str, root: &Path, version: &str) -> (PathBuf, PathBuf) {
    let (project, registry) = create_current_consumer(binary, root);
    let project_path = project.join(".appsdk/project.json");
    let mut contract: Value = serde_json::from_slice(&fs::read(&project_path).unwrap()).unwrap();
    contract["sdk"]["version"] = Value::String(version.into());
    fs::write(
        &project_path,
        serde_json::to_string_pretty(&contract).unwrap() + "\n",
    )
    .unwrap();
    for name in GOVERNANCE_MAP_NAMES {
        fs::copy(
            repo_root()
                .join("contracts/migrations")
                .join(version)
                .join("governance-maps")
                .join(name),
            project.join(".appsdk/maps").join(name),
        )
        .unwrap();
    }
    (project, registry)
}

// Build a consumer that is pinned at `0.1.0014` with the historical 0014 maps
// so the canonical `0.1.0014 -> 0.1.0015` migration is exercised.
fn create_0014_consumer(binary: &str, root: &Path) -> (PathBuf, PathBuf) {
    create_historical_consumer(binary, root, "0.1.0014")
}

fn pin_lock(binary: &str, registry: &Path, project: &Path) -> std::process::Output {
    Command::new(binary)
        .args(["pin-lock", project.to_str().unwrap(), "--binary", binary])
        .env("APPSDK_HOME", registry)
        .output()
        .unwrap()
}

fn run_init(binary: &str, registry: &Path, project: &Path) -> std::process::Output {
    Command::new(binary)
        .args(["init", project.to_str().unwrap()])
        .env("APPSDK_HOME", registry)
        .output()
        .unwrap()
}

fn historical_0014_lock(
    digest: &str,
    compiler_digest: &str,
    bundle_digest: &str,
    bundle_manifest_digest: &str,
) -> String {
    serde_json::json!({
        "sdk": "appsdk",
        "version": "0.1.0014",
        "digest": digest,
        "compiler_digest": compiler_digest,
        "bundle_digest": bundle_digest,
        "bundle_manifest_digest": bundle_manifest_digest,
        "contract_schema": 1
    })
    .to_string()
        + "\n"
}

#[test]
fn pin_lock_migrates_0014_to_0015_canonically_and_is_idempotent() {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = std::env::temp_dir().join(format!(
        "appsdk-0014-to-0015-{}-{nonce}",
        std::process::id()
    ));
    fs::create_dir_all(&root).unwrap();
    let binary = env!("CARGO_BIN_EXE_appsdk");
    let (project, registry) = create_0014_consumer(binary, &root);

    let pinned = pin_lock(binary, &registry, &project);
    assert!(
        pinned.status.success(),
        "{}",
        String::from_utf8_lossy(&pinned.stderr)
    );
    let lock: Value =
        serde_json::from_slice(&fs::read(project.join(".appsdk/sdk.lock")).unwrap()).unwrap();
    assert_eq!(lock["version"], "0.1.0015");

    let record_path = project.join(".appsdk/migrations/0.1.0014-to-0.1.0015/record.json");
    assert!(record_path.is_file());
    let record: Value = serde_json::from_slice(&fs::read(&record_path).unwrap()).unwrap();
    assert_eq!(record["source_version"], "0.1.0014");
    assert_eq!(record["target_version"], "0.1.0015");
    // A consumer already pinned at 0.1.0014 must not materialize the skipped
    // historical step; only the new final step owns the live binding.
    assert!(!project
        .join(".appsdk/migrations/0.1.0013-to-0.1.0014/record.json")
        .exists());
    for name in GOVERNANCE_MAP_NAMES {
        assert_eq!(
            fs::read(
                project
                    .join(".appsdk/migrations/0.1.0014-to-0.1.0015/maps")
                    .join(name)
            )
            .unwrap(),
            fs::read(
                repo_root()
                    .join("contracts/migrations/0.1.0014/governance-maps")
                    .join(name)
            )
            .unwrap()
        );
        assert_eq!(
            fs::read(project.join(".appsdk/maps").join(name)).unwrap(),
            fs::read(repo_root().join("contracts/maps").join(name)).unwrap()
        );
    }

    let record_bytes = fs::read(&record_path).unwrap();
    let repinned = pin_lock(binary, &registry, &project);
    assert!(
        repinned.status.success(),
        "{}",
        String::from_utf8_lossy(&repinned.stderr)
    );
    assert_eq!(fs::read(&record_path).unwrap(), record_bytes);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn pin_lock_rejects_tampered_current_0015_map_after_record() {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = std::env::temp_dir().join(format!(
        "appsdk-0015-live-map-tamper-{}-{nonce}",
        std::process::id()
    ));
    fs::create_dir_all(&root).unwrap();
    let binary = env!("CARGO_BIN_EXE_appsdk");
    let (project, registry) = create_0014_consumer(binary, &root);
    assert!(pin_lock(binary, &registry, &project).status.success());

    let live = project.join(".appsdk/maps/resource-map.json");
    let mut changed = fs::read(&live).unwrap();
    changed.push(b'\n');
    fs::write(&live, &changed).unwrap();
    let record_path = project.join(".appsdk/migrations/0.1.0014-to-0.1.0015/record.json");
    let record_before = fs::read(&record_path).unwrap();

    let rejected = pin_lock(binary, &registry, &project);
    assert!(!rejected.status.success());
    assert!(
        String::from_utf8_lossy(&rejected.stderr).contains("SDK_MIGRATION_LIVE_MAP_UNRECONCILED"),
        "{}",
        String::from_utf8_lossy(&rejected.stderr)
    );
    assert_eq!(fs::read(&live).unwrap(), changed);
    assert_eq!(fs::read(&record_path).unwrap(), record_before);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn pin_lock_rejects_tampered_0014_snapshot_evidence() {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = std::env::temp_dir().join(format!(
        "appsdk-0014-snapshot-tamper-{}-{nonce}",
        std::process::id()
    ));
    fs::create_dir_all(&root).unwrap();
    let binary = env!("CARGO_BIN_EXE_appsdk");
    let (project, registry) = create_0014_consumer(binary, &root);
    assert!(pin_lock(binary, &registry, &project).status.success());

    let snapshot = project.join(".appsdk/migrations/0.1.0014-to-0.1.0015/maps/resource-map.json");
    fs::write(&snapshot, "{\"tampered\":true}\n").unwrap();

    let rejected = pin_lock(binary, &registry, &project);
    assert!(!rejected.status.success());
    assert!(
        String::from_utf8_lossy(&rejected.stderr)
            .contains("SDK_MIGRATION_SNAPSHOT_MISMATCH:resource-map.json"),
        "{}",
        String::from_utf8_lossy(&rejected.stderr)
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn init_rejects_bound_historical_0014_lock_without_pin_lock() {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = std::env::temp_dir().join(format!(
        "appsdk-init-bound-0014-lock-{}-{nonce}",
        std::process::id()
    ));
    fs::create_dir_all(&root).unwrap();
    let binary = env!("CARGO_BIN_EXE_appsdk");
    let (project, registry) = create_current_consumer(binary, &root);
    let lock = historical_0014_lock(
        &format!("sha256:{}", "a".repeat(64)),
        &format!("sha256:{}", "b".repeat(64)),
        &format!("sha256:{}", "c".repeat(64)),
        &format!("sha256:{}", "d".repeat(64)),
    );
    let lock_path = project.join(".appsdk/sdk.lock");
    fs::write(&lock_path, &lock).unwrap();

    let rejected = run_init(binary, &registry, &project);
    assert!(!rejected.status.success());
    assert!(
        String::from_utf8_lossy(&rejected.stderr).contains("INVALID_SDK_LOCK"),
        "{}",
        String::from_utf8_lossy(&rejected.stderr)
    );
    assert_eq!(fs::read_to_string(&lock_path).unwrap(), lock);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn init_rejects_mixed_historical_0014_placeholder_lock() {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = std::env::temp_dir().join(format!(
        "appsdk-init-mixed-0014-lock-{}-{nonce}",
        std::process::id()
    ));
    fs::create_dir_all(&root).unwrap();
    let binary = env!("CARGO_BIN_EXE_appsdk");
    let (project, registry) = create_current_consumer(binary, &root);
    let lock = historical_0014_lock(
        &format!("sha256:{}", "a".repeat(64)),
        "sha256:replace-with-compiler-digest",
        "sha256:replace-with-sdk-bundle-digest",
        "sha256:replace-with-bundle-manifest-digest",
    );
    let lock_path = project.join(".appsdk/sdk.lock");
    fs::write(&lock_path, &lock).unwrap();

    let rejected = run_init(binary, &registry, &project);
    assert!(!rejected.status.success());
    assert!(
        String::from_utf8_lossy(&rejected.stderr).contains("INVALID_SDK_LOCK"),
        "{}",
        String::from_utf8_lossy(&rejected.stderr)
    );
    assert_eq!(fs::read_to_string(&lock_path).unwrap(), lock);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn pin_lock_rejects_tampered_historical_0013_to_0014_canonical_target_digest() {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = std::env::temp_dir().join(format!(
        "appsdk-0013-canonical-target-tamper-{}-{nonce}",
        std::process::id()
    ));
    fs::create_dir_all(&root).unwrap();
    let binary = env!("CARGO_BIN_EXE_appsdk");
    let (project, registry) = create_historical_consumer(binary, &root, "0.1.0013");
    let pinned = pin_lock(binary, &registry, &project);
    assert!(
        pinned.status.success(),
        "{}",
        String::from_utf8_lossy(&pinned.stderr)
    );

    let record_path = project.join(".appsdk/migrations/0.1.0013-to-0.1.0014/record.json");
    let snapshot_path =
        project.join(".appsdk/migrations/0.1.0013-to-0.1.0014/maps/resource-map.json");
    let live_path = project.join(".appsdk/maps/resource-map.json");
    let mut record: Value = serde_json::from_slice(&fs::read(&record_path).unwrap()).unwrap();
    record["maps"][0]["target_digest"] = Value::String(format!("sha256:{}", "e".repeat(64)));
    let tampered_record = serde_json::to_string_pretty(&record).unwrap() + "\n";
    fs::write(&record_path, &tampered_record).unwrap();
    let snapshot_before = fs::read(&snapshot_path).unwrap();
    let live_before = fs::read(&live_path).unwrap();

    let rejected = pin_lock(binary, &registry, &project);
    assert!(!rejected.status.success());
    assert!(
        String::from_utf8_lossy(&rejected.stderr)
            .contains("SDK_MIGRATION_TARGET_MAP_MISMATCH:resource-map.json"),
        "{}",
        String::from_utf8_lossy(&rejected.stderr)
    );
    assert_eq!(fs::read_to_string(&record_path).unwrap(), tampered_record);
    assert_eq!(fs::read(&snapshot_path).unwrap(), snapshot_before);
    assert_eq!(fs::read(&live_path).unwrap(), live_before);
    fs::remove_dir_all(root).unwrap();
}
