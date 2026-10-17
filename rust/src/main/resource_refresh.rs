use super::*;

const RESOURCE_CLASSES: [&str; 4] = ["contracts", "docs", "rules", "skills"];
const REFRESH_CLASSES: [&str; 3] = ["docs", "rules", "skills"];

struct RefreshResource {
    source: String,
    class: String,
    path: String,
    content: &'static str,
}

pub(super) fn run_cli(args: &mut std::iter::Peekable<std::vec::IntoIter<String>>) {
    match args.next().as_deref() {
        Some("refresh") => {
            if args.peek().is_some_and(|value| is_help(value)) {
                print_cli_help(Some("resources"));
                return;
            }
            let root = project_root_or_cwd(args);
            if args.next().is_some() {
                fail("USAGE: appsdk resources refresh [project]");
            }
            refresh_project_resources(&root);
        }
        Some(value) if is_help(value) => print_cli_help(Some("resources")),
        _ => fail("USAGE: appsdk resources refresh [project]"),
    }
}

fn refresh_project_resources(root: &Path) {
    assert_project_root_safe(root);
    assert_mutation_worktree(root);

    let project = read_project(root);
    let core_version = validate_project_declaration(&project);
    let manifest_path = ".appsdk/contracts/sdk-bundle.manifest.json";
    let record_path = ".appsdk/sdk-resources.json";
    let manifest = read_owned_json(
        root,
        manifest_path,
        "sdk_bundle_manifest",
        "SDK_BUNDLE_MANIFEST_MISSING",
        "INVALID_SDK_BUNDLE_MANIFEST",
    );
    let record = read_owned_json(
        root,
        record_path,
        "sdk_resource_record",
        "MISSING_SDK_RESOURCES",
        "INVALID_SDK_RESOURCES",
    );

    let manifest_resources = validate_installed_manifest(&manifest, core_version);
    let contracts = validate_resource_record(&record, core_version, &manifest_resources);
    let refresh_resources = current_refresh_resources();

    for (_, class, path) in &manifest_resources {
        if class == "contracts" {
            preflight_target(root, path, "sdk_resource_refresh_contract", true);
        }
    }
    for resource in &refresh_resources {
        preflight_target(root, &resource.path, "sdk_resource_refresh_target", false);
    }
    let record_target = preflight_target(root, record_path, "sdk_resource_refresh_record", true);

    for resource in &refresh_resources {
        let target = root.join(&resource.path);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).unwrap_or_else(|error| {
                fail(format!(
                    "SDK_RESOURCE_WRITE_FAILED:{}:{}",
                    resource.path, error
                ))
            });
        }
        atomic_write_bytes(
            &target,
            resource.content.as_bytes(),
            "SDK_RESOURCE_WRITE_FAILED",
        );
        println!("refreshed {}", resource.path);
    }

    let mut resources = contracts;
    resources.extend(refresh_resources.iter().map(|resource| {
        serde_json::json!({
            "source": resource.source,
            "class": resource.class,
            "path": resource.path
        })
    }));
    let refreshed_record = serde_json::json!({
        "schema_version": 2,
        "sdk": "appsdk",
        "version": core_version,
        "resource_version": SDK_VERSION,
        "resources": resources
    });
    atomic_write_json(
        &record_target,
        &refreshed_record,
        "SDK_RESOURCE_RECORD_WRITE_FAILED",
    );
    println!(
        "resource refresh complete: {} resources, core {}, resource {}",
        refresh_resources.len(),
        core_version,
        SDK_VERSION
    );
}

fn validate_project_declaration(project: &Value) -> &str {
    if project.get("schema_version").and_then(Value::as_u64) != Some(1) {
        fail("UNSUPPORTED_PROJECT_SCHEMA");
    }
    if project.pointer("/sdk/name").and_then(Value::as_str) != Some("appsdk") {
        fail("INVALID_PROJECT_CONTRACT:/sdk/name");
    }
    let core_version = project
        .pointer("/sdk/version")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| fail("INVALID_PROJECT_CONTRACT:/sdk/version"));
    if project
        .pointer("/sdk/bundle_manifest")
        .and_then(Value::as_str)
        != Some(".appsdk/contracts/sdk-bundle.manifest.json")
    {
        fail("INVALID_SDK_CONTRACT:/sdk/bundle_manifest");
    }
    if project
        .pointer("/sdk/resource_record")
        .and_then(Value::as_str)
        != Some(".appsdk/sdk-resources.json")
    {
        fail("INVALID_SDK_CONTRACT:/sdk/resource_record");
    }
    core_version
}

fn read_owned_json(
    root: &Path,
    relative: &str,
    label: &str,
    missing_error: &str,
    invalid_error: &str,
) -> Value {
    let path = safe_owned_path(root, relative, label);
    let metadata = fs::symlink_metadata(&path).unwrap_or_else(|error| {
        if error.kind() == ErrorKind::NotFound {
            fail(missing_error);
        }
        fail(invalid_error);
    });
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        fail(invalid_error);
    }
    serde_json::from_slice(
        &fs::read(&path).unwrap_or_else(|_| fail(format!("{}:{}", invalid_error, relative))),
    )
    .unwrap_or_else(|_| fail(invalid_error))
}

fn validate_installed_manifest(
    manifest: &Value,
    core_version: &str,
) -> Vec<(String, String, String)> {
    if manifest.get("schema_version").and_then(Value::as_u64) != Some(1)
        || manifest.get("sdk").and_then(Value::as_str) != Some("appsdk")
        || manifest.get("version").and_then(Value::as_str) != Some(core_version)
        || manifest.get("runtime_entrypoint").and_then(Value::as_str) != Some("rust-binary")
    {
        fail("INVALID_SDK_BUNDLE_MANIFEST");
    }
    let project_roots = manifest
        .pointer("/install/project_roots")
        .and_then(Value::as_object)
        .unwrap_or_else(|| fail("INVALID_SDK_BUNDLE_MANIFEST"));
    for (class, root) in [
        ("contracts", ".appsdk/contracts"),
        ("docs", ".appsdk/docs"),
        ("rules", ".appsdk/rules"),
        ("skills", ".appsdk/skills"),
    ] {
        if project_roots.get(class).and_then(Value::as_str) != Some(root) {
            fail("INVALID_SDK_BUNDLE_MANIFEST");
        }
    }
    if manifest
        .pointer("/install/generated_record")
        .and_then(Value::as_str)
        != Some(".appsdk/sdk-resources.json")
    {
        fail("INVALID_SDK_BUNDLE_MANIFEST");
    }

    let resources = manifest
        .get("resources")
        .and_then(Value::as_object)
        .unwrap_or_else(|| fail("INVALID_SDK_BUNDLE_MANIFEST"));
    if resources
        .keys()
        .any(|class| !RESOURCE_CLASSES.contains(&class.as_str()))
    {
        fail("INVALID_SDK_BUNDLE_MANIFEST");
    }
    let mut entries = Vec::new();
    let mut keys = BTreeSet::new();
    let mut paths = BTreeSet::new();
    for class in RESOURCE_CLASSES {
        let sources = resources
            .get(class)
            .and_then(Value::as_array)
            .filter(|sources| !sources.is_empty())
            .unwrap_or_else(|| fail("INVALID_SDK_BUNDLE_MANIFEST"));
        for source in sources {
            let source = source
                .as_str()
                .filter(|source| !source.is_empty())
                .unwrap_or_else(|| fail("INVALID_SDK_BUNDLE_MANIFEST"));
            let key = resource_key(class, source);
            if !keys.insert(key) {
                fail("INVALID_SDK_BUNDLE_MANIFEST");
            }
            let path = sdk_resource_install_relative(source, class);
            validate_resource_path(&path);
            if !paths.insert(path.clone()) {
                fail("INVALID_SDK_BUNDLE_MANIFEST");
            }
            entries.push((source.to_string(), class.to_string(), path));
        }
    }
    entries
}

fn validate_resource_record(
    record: &Value,
    core_version: &str,
    manifest_resources: &[(String, String, String)],
) -> Vec<Value> {
    let schema_version = record
        .get("schema_version")
        .and_then(Value::as_u64)
        .unwrap_or_else(|| fail("INVALID_SDK_RESOURCES"));
    if !matches!(schema_version, 1 | 2)
        || record.get("sdk").and_then(Value::as_str) != Some("appsdk")
        || record.get("version").and_then(Value::as_str) != Some(core_version)
        || (schema_version == 2
            && record
                .get("resource_version")
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
                .is_none())
    {
        fail("INVALID_SDK_RESOURCES");
    }

    let entries = record
        .get("resources")
        .and_then(Value::as_array)
        .filter(|entries| !entries.is_empty())
        .unwrap_or_else(|| fail("INVALID_SDK_RESOURCES"));
    let manifest_keys = manifest_resources
        .iter()
        .map(|(source, class, _)| resource_key(class, source))
        .collect::<BTreeSet<_>>();
    let mut seen = BTreeSet::new();
    let mut paths = BTreeSet::new();
    let mut contracts = Vec::new();
    for entry in entries {
        let source = entry
            .get("source")
            .and_then(Value::as_str)
            .filter(|source| !source.is_empty())
            .unwrap_or_else(|| fail("INVALID_SDK_RESOURCES"));
        let class = entry
            .get("class")
            .and_then(Value::as_str)
            .filter(|class| RESOURCE_CLASSES.contains(class))
            .unwrap_or_else(|| fail("INVALID_SDK_RESOURCES"));
        let path = entry
            .get("path")
            .and_then(Value::as_str)
            .unwrap_or_else(|| fail("INVALID_SDK_RESOURCES"));
        validate_resource_path(path);
        if path != sdk_resource_install_relative(source, class) {
            fail("INVALID_SDK_RESOURCES");
        }
        let key = resource_key(class, source);
        if !seen.insert(key.clone()) || !paths.insert(path.to_string()) {
            fail("INVALID_SDK_RESOURCES");
        }
        if schema_version == 1 && !manifest_keys.contains(&key) {
            fail("INVALID_SDK_RESOURCES");
        }
        if class == "contracts" && !manifest_keys.contains(&key) {
            fail("INVALID_SDK_RESOURCES");
        }
        if class == "contracts" {
            contracts.push(entry.clone());
        }
    }
    if schema_version == 1 && seen != manifest_keys {
        fail("INVALID_SDK_RESOURCES");
    }

    for (source, class, _) in manifest_resources {
        if class != "contracts" {
            continue;
        }
        let key = resource_key(class, source);
        if !seen.contains(&key) {
            fail("INVALID_SDK_RESOURCES");
        }
    }
    contracts
}

fn current_refresh_resources() -> Vec<RefreshResource> {
    assert_bundle_manifest();
    let mut resources = Vec::new();
    let mut keys = BTreeSet::new();
    let mut paths = BTreeSet::new();
    for (source, class, content) in sdk_bundle_resource_entries() {
        if !REFRESH_CLASSES.contains(&class.as_str()) {
            continue;
        }
        let path = sdk_resource_install_relative(&source, &class);
        validate_resource_path(&path);
        if !keys.insert(resource_key(&class, &source)) || !paths.insert(path.clone()) {
            fail("INVALID_SDK_BUNDLE");
        }
        resources.push(RefreshResource {
            source,
            class,
            path,
            content,
        });
    }
    if resources.is_empty() {
        fail("INVALID_SDK_BUNDLE");
    }
    resources
}

fn preflight_target(root: &Path, relative: &str, label: &str, must_be_file: bool) -> PathBuf {
    validate_resource_path(relative);
    let target = root.join(relative);
    assert_no_symlink_components(root, &target, label);
    let mut current = root.to_path_buf();
    for component in Path::new(relative).components() {
        current.push(component.as_os_str());
        match fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                fail(format!("GOVERNANCE_PATH_SYMLINK:{}", label));
            }
            Ok(metadata) if current == target => {
                if !metadata.is_file() {
                    fail(format!("SDK_RESOURCE_TARGET_NOT_FILE:{}", relative));
                }
            }
            Ok(metadata) if !metadata.is_dir() => {
                fail(format!(
                    "SDK_RESOURCE_TARGET_PARENT_NOT_DIRECTORY:{}",
                    relative
                ));
            }
            Ok(_) => {}
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(_) => fail(format!("SDK_RESOURCE_TARGET_READ_FAILED:{}", relative)),
        }
    }
    if must_be_file && !target.is_file() {
        fail(format!("SDK_RESOURCE_TARGET_MISSING:{}", relative));
    }
    target
}

fn validate_resource_path(relative: &str) {
    let path = Path::new(relative);
    if path.is_absolute()
        || relative == ".appsdk"
        || !relative.starts_with(".appsdk/")
        || path.components().any(|component| {
            matches!(
                component,
                std::path::Component::ParentDir
                    | std::path::Component::CurDir
                    | std::path::Component::RootDir
                    | std::path::Component::Prefix(_)
            )
        })
    {
        fail("INVALID_SDK_RESOURCE_PATH");
    }
}

fn resource_key(class: &str, source: &str) -> String {
    format!("{}\0{}", class, source)
}
