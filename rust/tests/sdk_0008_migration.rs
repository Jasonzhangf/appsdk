use serde_json::Value;
use std::fs;
use std::path::Path;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

#[test]
fn pin_lock_preserves_0007_source_maps_when_upgrading_to_0008() {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = std::env::temp_dir().join(format!(
        "appsdk-0007-to-0008-{}-{nonce}",
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
    contract["sdk"]["version"] = Value::String("0.1.0007".into());
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
        let old = repo
            .join("contracts/migrations/0.1.0007/governance-maps")
            .join(name);
        fs::copy(old, project.join(".appsdk/maps").join(name)).unwrap();
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
        &fs::read(project.join(".appsdk/migrations/0.1.0007-to-0.1.0008/record.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(record["source_version"], "0.1.0007");
    assert_eq!(record["target_version"], "0.1.0008");
    for name in [
        "resource-map.json",
        "function-map.json",
        "mainline-call-map.json",
        "verification-map.json",
    ] {
        assert_eq!(
            fs::read(
                project
                    .join(".appsdk/migrations/0.1.0007-to-0.1.0008/maps")
                    .join(name)
            )
            .unwrap(),
            fs::read(
                repo.join("contracts/migrations/0.1.0007/governance-maps")
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
