const ZONE_TRANSITION_CANONICAL_PATH: &str =
    "contracts/transitions/zone-transition.manifest.json";
const ZONE_TRANSITION_LEGACY_PATH: &str =
    "contracts/transitions/zone-transition-manifest.json";
const ZONE_TRANSITION_CURRENT: &str =
    include_str!("../../../contracts/transitions/zone-transition.manifest.json");
const ZONE_TRANSITION_SCHEMA: &str =
    include_str!("../../../contracts/transitions/zone-transition-manifest.json");
const ZONE_TRANSITION_PREDECESSORS: [(&str, &str); 3] = [
    (
        "0.1.3",
        include_str!("../fixtures/zone-transition-0.1.3.json"),
    ),
    (
        "0.1.4",
        include_str!("../fixtures/zone-transition-0.1.4.json"),
    ),
    (
        "0.1.5-0.1.6",
        include_str!("../fixtures/zone-transition-0.1.5-0.1.6.json"),
    ),
];

fn write_zone_transition_contract(root: &Path, relative: &str, content: &[u8]) {
    fs::write(root.join(relative), content).unwrap();
}

fn set_zone_transition_declaration(root: &Path, declared: &str) {
    let project_path = root.join(".appsdk/project.json");
    let mut project: Value =
        serde_json::from_str(&fs::read_to_string(&project_path).unwrap()).unwrap();
    project["governance"]["zone_transition_contract"] = Value::String(declared.into());
    fs::write(
        &project_path,
        serde_json::to_string_pretty(&project).unwrap() + "\n",
    )
    .unwrap();
}

fn set_project_sdk_version(root: &Path, version: &str) {
    let project_path = root.join(".appsdk/project.json");
    let mut project: Value =
        serde_json::from_str(&fs::read_to_string(&project_path).unwrap()).unwrap();
    project["sdk"]["version"] = Value::String(version.into());
    fs::write(
        &project_path,
        serde_json::to_string_pretty(&project).unwrap() + "\n",
    )
    .unwrap();
}

fn zone_transition_declaration(root: &Path) -> String {
    let project: Value = serde_json::from_str(
        &fs::read_to_string(root.join(".appsdk/project.json")).unwrap(),
    )
    .unwrap();
    project["governance"]["zone_transition_contract"]
        .as_str()
        .unwrap()
        .to_string()
}

fn assert_zone_transition_contract(root: &Path, relative: &str, expected: &str) {
    assert_eq!(
        fs::read_to_string(root.join(relative)).unwrap(),
        expected,
        "{relative}"
    );
}

#[cfg(target_os = "macos")]
fn assert_no_zone_transition_staging(root: &Path) {
    let mut residues = Vec::new();
    for entry in fs::read_dir(root.join("contracts/transitions")).unwrap() {
        let name = entry.unwrap().file_name().to_string_lossy().into_owned();
        if name.contains(".staging.") {
            residues.push(name);
        }
    }
    assert!(residues.is_empty(), "transition staging residue: {residues:?}");
}

#[cfg(target_os = "macos")]
struct ImmutableFileGuard {
    path: PathBuf,
    active: bool,
}

#[cfg(target_os = "macos")]
impl ImmutableFileGuard {
    fn new(path: &Path) -> Self {
        let output = Command::new("chflags")
            .args(["uchg"])
            .arg(path)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "chflags uchg {}: {}",
            path.display(),
            String::from_utf8_lossy(&output.stderr)
        );
        Self {
            path: path.to_path_buf(),
            active: true,
        }
    }

    fn restore(&mut self) {
        if !self.active {
            return;
        }
        let output = Command::new("chflags")
            .args(["nouchg"])
            .arg(&self.path)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "chflags nouchg {}: {}",
            self.path.display(),
            String::from_utf8_lossy(&output.stderr)
        );
        self.active = false;
    }
}

#[cfg(target_os = "macos")]
impl Drop for ImmutableFileGuard {
    fn drop(&mut self) {
        if self.active {
            let _ = Command::new("chflags")
                .args(["nouchg"])
                .arg(&self.path)
                .status();
        }
    }
}

#[test]
fn pin_lock_refreshes_trusted_legacy_zone_transition_contracts() {
    for (version, legacy) in ZONE_TRANSITION_PREDECESSORS {
        let root = temp_root(&format!("zone-transition-legacy-{version}"));
        let root_text = root.to_str().unwrap();
        assert!(run(&["new", root_text]).status.success());
        write_zone_transition_contract(
            &root,
            ZONE_TRANSITION_CANONICAL_PATH,
            legacy.as_bytes(),
        );
        write_zone_transition_contract(&root, ZONE_TRANSITION_LEGACY_PATH, legacy.as_bytes());
        set_zone_transition_declaration(&root, ZONE_TRANSITION_LEGACY_PATH);

        let pinned = run(&[
            "pin-lock",
            root_text,
            "--binary",
            binary().to_str().unwrap(),
        ]);
        assert!(
            pinned.status.success(),
            "{version}: {}",
            String::from_utf8_lossy(&pinned.stderr)
        );
        assert_zone_transition_contract(
            &root,
            ZONE_TRANSITION_CANONICAL_PATH,
            ZONE_TRANSITION_CURRENT,
        );
        assert_zone_transition_contract(
            &root,
            ZONE_TRANSITION_LEGACY_PATH,
            ZONE_TRANSITION_CURRENT,
        );
        assert_eq!(
            zone_transition_declaration(&root),
            ZONE_TRANSITION_LEGACY_PATH
        );
        let verified = run(&["verify", root_text]);
        assert!(
            verified.status.success(),
            "{version}: {}",
            String::from_utf8_lossy(&verified.stderr)
        );
        fs::remove_dir_all(root).unwrap();
    }

    let root = temp_root("zone-transition-canonical-declaration");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());
    write_zone_transition_contract(
        &root,
        ZONE_TRANSITION_CANONICAL_PATH,
        ZONE_TRANSITION_PREDECESSORS[2].1.as_bytes(),
    );
    write_zone_transition_contract(
        &root,
        ZONE_TRANSITION_LEGACY_PATH,
        ZONE_TRANSITION_CURRENT.as_bytes(),
    );
    set_zone_transition_declaration(&root, ZONE_TRANSITION_CANONICAL_PATH);
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
    assert_zone_transition_contract(
        &root,
        ZONE_TRANSITION_CANONICAL_PATH,
        ZONE_TRANSITION_CURRENT,
    );
    assert_zone_transition_contract(
        &root,
        ZONE_TRANSITION_LEGACY_PATH,
        ZONE_TRANSITION_CURRENT,
    );
    assert_eq!(
        zone_transition_declaration(&root),
        ZONE_TRANSITION_CANONICAL_PATH
    );
    assert!(run(&["verify", root_text]).status.success());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn pin_lock_rejects_unknown_zone_transition_content() {
    let root = temp_root("zone-transition-unknown");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());
    set_project_sdk_version(&root, "0.1.5");
    install_legacy_governance_maps(&root);
    let legacy = ZONE_TRANSITION_PREDECESSORS[2].1;
    write_zone_transition_contract(
        &root,
        ZONE_TRANSITION_CANONICAL_PATH,
        legacy.as_bytes(),
    );
    write_zone_transition_contract(
        &root,
        ZONE_TRANSITION_LEGACY_PATH,
        b"user-edited transition runtime\n",
    );
    set_zone_transition_declaration(&root, ZONE_TRANSITION_LEGACY_PATH);
    let project_before = fs::read(root.join(".appsdk/project.json")).unwrap();
    let lock_before = fs::read(root.join(".appsdk/sdk.lock")).unwrap();

    let rejected = run(&[
        "pin-lock",
        root_text,
        "--binary",
        binary().to_str().unwrap(),
    ]);
    assert!(!rejected.status.success());
    assert!(
        String::from_utf8_lossy(&rejected.stderr)
            .contains("SDK_TRANSITION_CONTRACT_UNKNOWN_CONTENT:"),
        "{}",
        String::from_utf8_lossy(&rejected.stderr)
    );
    assert_eq!(
        fs::read(root.join(ZONE_TRANSITION_CANONICAL_PATH)).unwrap(),
        legacy.as_bytes()
    );
    assert_eq!(
        fs::read(root.join(ZONE_TRANSITION_LEGACY_PATH)).unwrap(),
        b"user-edited transition runtime\n"
    );
    assert_eq!(
        fs::read(root.join(".appsdk/project.json")).unwrap(),
        project_before
    );
    assert_eq!(fs::read(root.join(".appsdk/sdk.lock")).unwrap(), lock_before);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn pin_lock_rejects_schema_shaped_zone_transition_alias() {
    let root = temp_root("zone-transition-schema-declared");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());
    write_zone_transition_contract(
        &root,
        ZONE_TRANSITION_LEGACY_PATH,
        ZONE_TRANSITION_SCHEMA.as_bytes(),
    );
    set_zone_transition_declaration(&root, ZONE_TRANSITION_LEGACY_PATH);

    let rejected = run(&[
        "pin-lock",
        root_text,
        "--binary",
        binary().to_str().unwrap(),
    ]);
    assert!(!rejected.status.success());
    assert!(
        String::from_utf8_lossy(&rejected.stderr)
            .contains("SDK_TRANSITION_CONTRACT_UNKNOWN_CONTENT:contracts/transitions/zone-transition-manifest.json"),
        "{}",
        String::from_utf8_lossy(&rejected.stderr)
    );
    assert_zone_transition_contract(&root, ZONE_TRANSITION_LEGACY_PATH, ZONE_TRANSITION_SCHEMA);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn pin_lock_preserves_non_declared_schema_alias() {
    let root = temp_root("zone-transition-schema-sibling");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());
    write_zone_transition_contract(
        &root,
        ZONE_TRANSITION_CANONICAL_PATH,
        ZONE_TRANSITION_PREDECESSORS[2].1.as_bytes(),
    );
    write_zone_transition_contract(
        &root,
        ZONE_TRANSITION_LEGACY_PATH,
        ZONE_TRANSITION_SCHEMA.as_bytes(),
    );
    set_zone_transition_declaration(&root, ZONE_TRANSITION_CANONICAL_PATH);

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
    assert_zone_transition_contract(
        &root,
        ZONE_TRANSITION_CANONICAL_PATH,
        ZONE_TRANSITION_CURRENT,
    );
    assert_zone_transition_contract(&root, ZONE_TRANSITION_LEGACY_PATH, ZONE_TRANSITION_SCHEMA);
    assert!(run(&["verify", root_text]).status.success());

    fs::remove_file(root.join(ZONE_TRANSITION_LEGACY_PATH)).unwrap();
    let repinned = run(&[
        "pin-lock",
        root_text,
        "--binary",
        binary().to_str().unwrap(),
    ]);
    assert!(
        repinned.status.success(),
        "{}",
        String::from_utf8_lossy(&repinned.stderr)
    );
    assert!(!root.join(ZONE_TRANSITION_LEGACY_PATH).exists());
    assert!(run(&["verify", root_text]).status.success());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn pin_lock_refresh_is_idempotent_and_preserves_migration_history() {
    let root = temp_root("zone-transition-idempotent");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());
    set_project_sdk_version(&root, "0.1.5");
    install_legacy_governance_maps(&root);
    let legacy = ZONE_TRANSITION_PREDECESSORS[2].1;
    write_zone_transition_contract(
        &root,
        ZONE_TRANSITION_CANONICAL_PATH,
        legacy.as_bytes(),
    );
    write_zone_transition_contract(&root, ZONE_TRANSITION_LEGACY_PATH, legacy.as_bytes());
    set_zone_transition_declaration(&root, ZONE_TRANSITION_LEGACY_PATH);

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
    assert!(run(&["verify", root_text]).status.success());
    let canonical_after = fs::read(root.join(ZONE_TRANSITION_CANONICAL_PATH)).unwrap();
    let legacy_after = fs::read(root.join(ZONE_TRANSITION_LEGACY_PATH)).unwrap();
    let history_root = root.join(".appsdk/migrations/0.1.5-to-0.1.6");
    let history_record = fs::read(history_root.join("record.json")).unwrap();
    let history_maps = [
        fs::read(history_root.join("maps/resource-map.json")).unwrap(),
        fs::read(history_root.join("maps/function-map.json")).unwrap(),
        fs::read(history_root.join("maps/mainline-call-map.json")).unwrap(),
        fs::read(history_root.join("maps/verification-map.json")).unwrap(),
    ];

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
    assert_eq!(
        fs::read(root.join(ZONE_TRANSITION_CANONICAL_PATH)).unwrap(),
        canonical_after
    );
    assert_eq!(
        fs::read(root.join(ZONE_TRANSITION_LEGACY_PATH)).unwrap(),
        legacy_after
    );
    assert_eq!(
        fs::read(history_root.join("record.json")).unwrap(),
        history_record
    );
    assert_eq!(
        [
            fs::read(history_root.join("maps/resource-map.json")).unwrap(),
            fs::read(history_root.join("maps/function-map.json")).unwrap(),
            fs::read(history_root.join("maps/mainline-call-map.json")).unwrap(),
            fs::read(history_root.join("maps/verification-map.json")).unwrap(),
        ],
        history_maps
    );
    assert!(run(&["verify", root_text]).status.success());
    fs::remove_dir_all(root).unwrap();
}

#[cfg(target_os = "macos")]
#[test]
fn pin_lock_cleans_failed_transition_staging_and_resumes_partial_pair() {
    let root = temp_root("zone-transition-write-failure");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());
    let legacy = ZONE_TRANSITION_PREDECESSORS[2].1;
    write_zone_transition_contract(
        &root,
        ZONE_TRANSITION_CANONICAL_PATH,
        legacy.as_bytes(),
    );
    write_zone_transition_contract(&root, ZONE_TRANSITION_LEGACY_PATH, legacy.as_bytes());
    set_zone_transition_declaration(&root, ZONE_TRANSITION_LEGACY_PATH);
    let project_before = fs::read(root.join(".appsdk/project.json")).unwrap();
    let lock_before = fs::read(root.join(".appsdk/sdk.lock")).unwrap();

    let mut guard = ImmutableFileGuard::new(&root.join(ZONE_TRANSITION_LEGACY_PATH));
    let failed = run(&[
        "pin-lock",
        root_text,
        "--binary",
        binary().to_str().unwrap(),
    ]);
    guard.restore();
    assert!(!failed.status.success());
    assert!(
        String::from_utf8_lossy(&failed.stderr)
            .contains("SDK_TRANSITION_CONTRACT_WRITE_FAILED"),
        "{}",
        String::from_utf8_lossy(&failed.stderr)
    );
    assert_no_zone_transition_staging(&root);
    assert_eq!(
        fs::read(root.join(".appsdk/project.json")).unwrap(),
        project_before
    );
    assert_eq!(fs::read(root.join(".appsdk/sdk.lock")).unwrap(), lock_before);
    assert_zone_transition_contract(
        &root,
        ZONE_TRANSITION_CANONICAL_PATH,
        ZONE_TRANSITION_CURRENT,
    );
    assert_zone_transition_contract(&root, ZONE_TRANSITION_LEGACY_PATH, legacy);

    let resumed = run(&[
        "pin-lock",
        root_text,
        "--binary",
        binary().to_str().unwrap(),
    ]);
    assert!(
        resumed.status.success(),
        "{}",
        String::from_utf8_lossy(&resumed.stderr)
    );
    assert_zone_transition_contract(
        &root,
        ZONE_TRANSITION_CANONICAL_PATH,
        ZONE_TRANSITION_CURRENT,
    );
    assert_zone_transition_contract(
        &root,
        ZONE_TRANSITION_LEGACY_PATH,
        ZONE_TRANSITION_CURRENT,
    );
    assert!(run(&["verify", root_text]).status.success());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn pin_lock_creates_missing_supported_zone_transition_paths() {
    let root = temp_root("zone-transition-missing-canonical");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());
    let legacy = ZONE_TRANSITION_PREDECESSORS[2].1;
    write_zone_transition_contract(&root, ZONE_TRANSITION_LEGACY_PATH, legacy.as_bytes());
    fs::remove_file(root.join(ZONE_TRANSITION_CANONICAL_PATH)).unwrap();
    set_zone_transition_declaration(&root, ZONE_TRANSITION_LEGACY_PATH);
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
    assert_zone_transition_contract(
        &root,
        ZONE_TRANSITION_CANONICAL_PATH,
        ZONE_TRANSITION_CURRENT,
    );
    assert_zone_transition_contract(
        &root,
        ZONE_TRANSITION_LEGACY_PATH,
        ZONE_TRANSITION_CURRENT,
    );
    assert!(run(&["verify", root_text]).status.success());
    fs::remove_dir_all(root).unwrap();

    let root = temp_root("zone-transition-missing-declared");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());
    write_zone_transition_contract(
        &root,
        ZONE_TRANSITION_CANONICAL_PATH,
        legacy.as_bytes(),
    );
    fs::remove_file(root.join(ZONE_TRANSITION_LEGACY_PATH)).unwrap();
    set_zone_transition_declaration(&root, ZONE_TRANSITION_CANONICAL_PATH);
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
    assert_zone_transition_contract(
        &root,
        ZONE_TRANSITION_CANONICAL_PATH,
        ZONE_TRANSITION_CURRENT,
    );
    assert!(!root.join(ZONE_TRANSITION_LEGACY_PATH).exists());
    assert!(run(&["verify", root_text]).status.success());
    fs::remove_dir_all(root).unwrap();
}
