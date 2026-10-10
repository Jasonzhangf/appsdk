use pipeline_runtime::{graph_topology, parse_graph_json, Graph};
use std::{
    env,
    ffi::OsString,
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    path::{Path, PathBuf},
    process,
};

const PACKAGED_SKILL: &str = include_str!("../../.agents/skills/dagpipe-runtime/SKILL.md");
const INSTALLED_FILES: &str = ".installed-files";
const INSTALLED_TREE: &str = ".installed-tree";

fn main() {
    if let Err(error) = run(env::args().skip(1).collect()) {
        eprintln!("dagpipe: {error}");
        std::process::exit(2);
    }
}

fn run(args: Vec<String>) -> Result<(), String> {
    let args: Vec<_> = args.iter().map(String::as_str).collect();
    match args.as_slice() {
        [command] if *command == "--version" || *command == "-V" => {
            println!("dagpipe {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        [command] if *command == "--help" || *command == "-h" => {
            print_help();
            Ok(())
        }
        ["modules", "list"] => {
            println!("graph.inspect\tinspect DAG topology and operator bindings");
            println!("graph.validate\tvalidate external DAG with one input and output (SESE)");
            println!("skill.install\tinstall the packaged project-usage skill");
            Ok(())
        }
        ["graph", "validate", path] => {
            let graph = load_graph(path)?;
            let topology = graph_topology(&graph).map_err(|error| error.to_string())?;
            println!(
                "valid DAG: {}@{} ({} nodes, {} edges, {} waves)",
                graph.id,
                graph.version,
                graph.nodes.len(),
                graph.edges.len(),
                topology.waves.len()
            );
            println!("operator bindings are syntactically present; project compile() remains the authoritative registry/schema/effect gate");
            Ok(())
        }
        ["graph", "inspect", path] => {
            let graph = load_graph(path)?;
            let topology = graph_topology(&graph).map_err(|error| error.to_string())?;
            println!("Graph {}@{}", graph.id, graph.version);
            println!("Operator bindings:");
            for node in &graph.nodes {
                println!(
                    "  {} -> {}@{}",
                    node.id, node.operator, node.operator_version
                );
            }
            println!("DAG execution waves:");
            for (index, wave) in topology.waves.iter().enumerate() {
                println!("  {}: {}", index + 1, wave.join(", "));
            }
            println!("Edges:");
            for edge in &graph.edges {
                println!("  {} --{}--> {}", edge.from, edge.arc_id, edge.to);
            }
            println!("Note: inspect checks DAG shape only; compile() resolves registered Operators and contracts.");
            Ok(())
        }
        ["install", "--source", source] => install_from_source(Path::new(source)),
        ["skill", "install"] => install_skill(PACKAGED_SKILL.as_bytes()),
        ["sdk", "path"] => print_sdk_path(),
        _ => {
            print_help();
            Err("invalid command".into())
        }
    }
}

fn load_graph(path: &str) -> Result<Graph, String> {
    let contents =
        fs::read_to_string(path).map_err(|error| format!("cannot read graph `{path}`: {error}"))?;
    parse_graph_json(&contents).map_err(|error| error.to_string())
}

fn install_from_source(source: &Path) -> Result<(), String> {
    let source = fs::canonicalize(source)
        .map_err(|error| format!("cannot resolve source `{}`: {error}", source.display()))?;
    validate_source(&source)?;

    let sdk_target = sdk_target()?;
    let skill_target = skill_target()?;
    let skill_source = source
        .join(".agents")
        .join("skills")
        .join("dagpipe-runtime")
        .join("SKILL.md");
    let binary_target = binary_target()?;
    recover_stale_backup(
        &binary_target,
        ".dagpipe-previous.",
        BackupKind::File,
        "DAGpipe CLI",
    )?;
    recover_stale_backup(
        &sdk_target,
        ".sdk-previous.",
        BackupKind::Directory,
        "DAGpipe SDK",
    )?;
    let stage = stage_sdk(&source, &sdk_target)?;

    validate_existing_sdk(&sdk_target, &stage.path)?;
    validate_skill_target(&skill_source, &skill_target)?;
    install_binary(&binary_target)?;
    install_skill_from_source(&skill_source, &skill_target)?;
    replace_sdk(&stage.path, &sdk_target)?;
    println!("Installed DAGpipe SDK at {}", sdk_target.display());
    Ok(())
}

fn validate_source(source: &Path) -> Result<(), String> {
    let manifest = source.join("Cargo.toml");
    let src = source.join("src");
    let skill = source
        .join(".agents")
        .join("skills")
        .join("dagpipe-runtime")
        .join("SKILL.md");
    require_regular_file(&manifest, "source Cargo.toml")?;
    require_directory(&src, "source src directory")?;
    require_regular_file(&skill, "source DAGpipe Skill")?;

    let contents = fs::read_to_string(&manifest).map_err(|error| {
        format!(
            "cannot read source manifest `{}`: {error}",
            manifest.display()
        )
    })?;
    if package_field(&contents, "name") != Some("pipeline_runtime") {
        return Err(format!(
            "source manifest is not pipeline_runtime at {}",
            manifest.display()
        ));
    }
    match package_field(&contents, "repository") {
        Some("https://github.com/Jasonzhangf/DAGpipe")
        | Some("https://github.com/Jasonzhangf/appsdk") => Ok(()),
        _ => Err(format!(
            "source manifest is not the owned DAGpipe repository at {}",
            manifest.display()
        )),
    }
}

fn stage_sdk(source: &Path, sdk_target: &Path) -> Result<TempDir, String> {
    let sdk_parent = sdk_target
        .parent()
        .ok_or_else(|| format!("invalid SDK installation path {}", sdk_target.display()))?;
    fs::create_dir_all(sdk_parent).map_err(|error| {
        format!(
            "cannot create SDK parent directory `{}`: {error}",
            sdk_parent.display()
        )
    })?;
    let stage = TempDir::new(create_temp_dir(sdk_parent, ".sdk-stage.")?);

    let skill_target = stage
        .path
        .join(".agents")
        .join("skills")
        .join("dagpipe-runtime");
    fs::create_dir_all(&skill_target).map_err(|error| {
        format!(
            "cannot create staged Skill directory `{}`: {error}",
            skill_target.display()
        )
    })?;
    copy_regular_file(&source.join("Cargo.toml"), &stage.path.join("Cargo.toml"))?;
    copy_tree(&source.join("src"), &stage.path.join("src"))?;
    copy_regular_file(
        &source
            .join(".agents")
            .join("skills")
            .join("dagpipe-runtime")
            .join("SKILL.md"),
        &skill_target.join("SKILL.md"),
    )?;

    let files = file_manifest(&stage.path)?;
    let tree = tree_manifest(&stage.path)?;
    write_new_file(&stage.path.join(INSTALLED_FILES), files.as_bytes())?;
    write_new_file(&stage.path.join(INSTALLED_TREE), tree.as_bytes())?;
    Ok(stage)
}

fn validate_existing_sdk(target: &Path, stage: &Path) -> Result<(), String> {
    match fs::symlink_metadata(target) {
        Ok(metadata) => {
            if is_link_or_reparse(&metadata) {
                return Err(format!(
                    "SDK path is a symlink or reparse point at {}; refusing to follow it",
                    target.display()
                ));
            }
            if !metadata.is_dir() {
                return Err(format!(
                    "existing SDK path is not a DAGpipe SDK directory at {}; refusing to overwrite it",
                    target.display()
                ));
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(format!(
                "cannot inspect existing SDK path `{}`: {error}",
                target.display()
            ));
        }
    }

    let manifest = target.join("Cargo.toml");
    let src = target.join("src");
    require_regular_file(&manifest, "existing SDK Cargo.toml")?;
    require_directory(&src, "existing SDK src directory")?;
    let contents = fs::read_to_string(&manifest).map_err(|error| {
        format!(
            "cannot read existing SDK manifest `{}`: {error}",
            manifest.display()
        )
    })?;
    if package_field(&contents, "name") != Some("pipeline_runtime") {
        return Err(format!(
            "existing SDK manifest is not pipeline_runtime at {}; refusing to overwrite it",
            manifest.display()
        ));
    }
    match package_field(&contents, "repository") {
        Some("https://github.com/Jasonzhangf/DAGpipe")
        | Some("https://github.com/Jasonzhangf/appsdk") => {}
        _ => {
            return Err(format!(
                "existing SDK manifest is not pipeline_runtime at {}; refusing to overwrite it",
                manifest.display()
            ));
        }
    }

    collect_tree_entries(target)?;
    let installed_files = target.join(INSTALLED_FILES);
    let installed_tree = target.join(INSTALLED_TREE);
    let actual_files = file_manifest(target)?;
    let actual_tree = tree_manifest(target)?;
    if path_exists(&installed_files)? {
        require_regular_file(&installed_files, "existing SDK install manifest")?;
        let expected_files = read_manifest(&installed_files)?;
        if manifest_lines(&actual_files) != manifest_lines(&expected_files) {
            return Err(format!(
                "existing SDK differs from the last installed file manifest; refusing to replace it at {}",
                target.display()
            ));
        }
        let expected_tree = if path_exists(&installed_tree)? {
            require_regular_file(&installed_tree, "existing SDK tree manifest")?;
            read_manifest(&installed_tree)?
        } else {
            tree_manifest(stage)?
        };
        if manifest_lines(&actual_tree) != manifest_lines(&expected_tree) {
            return Err(format!(
                "existing SDK tree differs from the last installed tree; refusing to replace it at {}",
                target.display()
            ));
        }
    } else {
        if manifest_lines(&actual_files) != manifest_lines(&file_manifest(stage)?)
            || manifest_lines(&actual_tree) != manifest_lines(&tree_manifest(stage)?)
            || !trees_equal(&src, &stage.join("src"))?
        {
            return Err(format!(
                "legacy SDK contains extra or modified source; refusing to replace it at {}",
                target.display()
            ));
        }
    }
    Ok(())
}

fn validate_skill_target(source: &Path, target: &Path) -> Result<(), String> {
    let source = fs::read(source)
        .map_err(|error| format!("cannot read source Skill `{}`: {error}", source.display()))?;
    validate_skill_contents(&source, target)
}

fn validate_skill_contents(source: &[u8], target: &Path) -> Result<(), String> {
    if let Ok(metadata) = fs::symlink_metadata(target) {
        if is_link_or_reparse(&metadata) {
            return Err(format!(
                "skill path is a symlink or reparse point at {}; refusing to follow it",
                target.display()
            ));
        }
        if !metadata.is_file() {
            return Err(format!(
                "skill path is not a file at {}; refusing to overwrite it",
                target.display()
            ));
        }
        let existing = fs::read(target).map_err(|error| {
            format!("cannot read existing skill `{}`: {error}", target.display())
        })?;
        if existing != source && !existing.starts_with(b"---\nname: dagpipe-runtime\n") {
            return Err(format!(
                "skill already exists with different contents at {}; refusing to overwrite it",
                target.display()
            ));
        }
    }
    Ok(())
}

fn install_skill(contents: &[u8]) -> Result<(), String> {
    let target = skill_target()?;
    install_skill_bytes(contents, &target)
}

fn install_skill_from_source(source: &Path, target: &Path) -> Result<(), String> {
    let contents = fs::read(source)
        .map_err(|error| format!("cannot read source Skill `{}`: {error}", source.display()))?;
    install_skill_bytes(&contents, target)
}

fn install_skill_bytes(contents: &[u8], target: &Path) -> Result<(), String> {
    validate_skill_contents(contents, target)?;
    if path_exists(target)? {
        let existing = fs::read(target).map_err(|error| {
            format!("cannot read existing skill `{}`: {error}", target.display())
        })?;
        if existing == contents {
            println!("DAGpipe skill already installed at {}", target.display());
            return Ok(());
        }
        fs::write(target, contents).map_err(|error| {
            format!(
                "cannot update DAGpipe skill `{}`: {error}",
                target.display()
            )
        })?;
        println!("Updated DAGpipe skill at {}", target.display());
        return Ok(());
    }

    let parent = target
        .parent()
        .ok_or_else(|| "invalid skill installation path".to_owned())?;
    fs::create_dir_all(parent).map_err(|error| {
        format!(
            "cannot create skill directory `{}`: {error}",
            parent.display()
        )
    })?;
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(target)
        .map_err(|error| format!("cannot create skill `{}`: {error}", target.display()))?;
    if let Err(error) = file.write_all(contents) {
        drop(file);
        let _ = fs::remove_file(target);
        return Err(format!(
            "cannot write skill `{}`: {error}",
            target.display()
        ));
    }
    println!("Installed DAGpipe skill at {}", target.display());
    Ok(())
}

fn binary_target() -> Result<PathBuf, String> {
    let install_root = cargo_install_root()?;
    Ok(install_root.join("bin").join(if cfg!(windows) {
        "dagpipe.exe"
    } else {
        "dagpipe"
    }))
}

fn install_binary(target: &Path) -> Result<(), String> {
    let source = env::current_exe()
        .map_err(|error| format!("cannot resolve the running dagpipe binary: {error}"))?;
    if paths_equivalent(&source, target) {
        println!("DAGpipe CLI already installed at {}", target.display());
        return Ok(());
    }
    if let Ok(metadata) = fs::symlink_metadata(target) {
        if is_link_or_reparse(&metadata) {
            return Err(format!(
                "dagpipe binary path is a symlink or reparse point at {}; refusing to follow it",
                target.display()
            ));
        }
        if !metadata.is_file() {
            return Err(format!(
                "dagpipe binary path is not a file at {}; refusing to overwrite it",
                target.display()
            ));
        }
    }
    let bin_dir = target
        .parent()
        .ok_or_else(|| format!("invalid dagpipe binary path {}", target.display()))?;
    fs::create_dir_all(bin_dir).map_err(|error| {
        format!(
            "cannot create Cargo binary directory `{}`: {error}",
            bin_dir.display()
        )
    })?;
    let stage = unique_nonexistent_path(bin_dir, ".dagpipe-stage.")?;
    let stage_file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&stage)
        .map_err(|error| format!("cannot stage dagpipe binary `{}`: {error}", stage.display()))?;
    drop(stage_file);
    if let Err(error) = fs::copy(&source, &stage) {
        let _ = fs::remove_file(&stage);
        return Err(format!(
            "cannot copy dagpipe binary from `{}` to `{}`: {error}",
            source.display(),
            stage.display()
        ));
    }
    #[cfg(unix)]
    {
        let permissions = match fs::metadata(&source) {
            Ok(metadata) => metadata.permissions(),
            Err(error) => {
                let _ = fs::remove_file(&stage);
                return Err(format!(
                    "cannot inspect dagpipe binary permissions: {error}"
                ));
            }
        };
        if let Err(error) = fs::set_permissions(&stage, permissions) {
            let _ = fs::remove_file(&stage);
            return Err(format!("cannot set dagpipe binary permissions: {error}"));
        }
    }

    if let Err(error) = replace_path(
        &stage,
        target,
        ".dagpipe-previous.",
        BackupKind::File,
        "DAGpipe CLI",
    ) {
        let _ = fs::remove_file(&stage);
        return Err(error);
    }
    println!("Installed DAGpipe CLI at {}", target.display());
    Ok(())
}

fn replace_sdk(stage: &Path, target: &Path) -> Result<(), String> {
    replace_path(
        stage,
        target,
        ".sdk-previous.",
        BackupKind::Directory,
        "DAGpipe SDK",
    )
}

#[derive(Clone, Copy)]
enum BackupKind {
    File,
    Directory,
}

impl BackupKind {
    fn matches(self, metadata: &fs::Metadata) -> bool {
        match self {
            Self::File => metadata.is_file(),
            Self::Directory => metadata.is_dir(),
        }
    }

    fn description(self) -> &'static str {
        match self {
            Self::File => "regular file",
            Self::Directory => "directory",
        }
    }

    fn failure_key(self) -> &'static str {
        match self {
            Self::File => "binary",
            Self::Directory => "sdk",
        }
    }

    fn remove(self, path: &Path) -> io::Result<()> {
        match self {
            Self::File => fs::remove_file(path),
            Self::Directory => fs::remove_dir_all(path),
        }
    }
}

fn recover_stale_backup(
    target: &Path,
    prefix: &str,
    kind: BackupKind,
    label: &str,
) -> Result<(), String> {
    let parent = target
        .parent()
        .ok_or_else(|| format!("invalid {label} installation path {}", target.display()))?;
    let mut backups = match fs::read_dir(parent) {
        Ok(entries) => entries
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| {
                format!(
                    "cannot read backup directory `{}`: {error}",
                    parent.display()
                )
            })?
            .into_iter()
            .map(|entry| entry.path())
            .filter(|path| {
                path.file_name()
                    .map(|name| name.to_string_lossy().starts_with(prefix))
                    .unwrap_or(false)
            })
            .collect::<Vec<_>>(),
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(format!(
                "cannot inspect backup directory `{}`: {error}",
                parent.display()
            ));
        }
    };
    backups.sort();
    if backups.is_empty() {
        return Ok(());
    }
    let rendered = backups
        .iter()
        .map(|path| format!("`{}`", path.display()))
        .collect::<Vec<_>>()
        .join(", ");
    if path_exists(target)? {
        return Err(format!(
            "stale {label} backup(s) found at {rendered} while target `{}` exists; refusing to overwrite either. Inspect both paths, keep the desired copy at the target, and remove or rename the backup(s) before retrying",
            target.display()
        ));
    }
    if backups.len() != 1 {
        return Err(format!(
            "multiple stale {label} backups found at {rendered} while target `{}` is missing; refusing automatic recovery. Inspect the backups, move the correct one to the target, and remove or rename the others before retrying",
            target.display()
        ));
    }
    let backup = &backups[0];
    let metadata = fs::symlink_metadata(backup).map_err(|error| {
        format!(
            "cannot inspect stale {label} backup `{}`: {error}",
            backup.display()
        )
    })?;
    if is_link_or_reparse(&metadata) || !kind.matches(&metadata) {
        return Err(format!(
            "stale {label} backup `{}` is not a {}; refusing to restore it to `{}`. Inspect both paths and recover manually",
            backup.display(),
            kind.description(),
            target.display()
        ));
    }
    fs::rename(backup, target).map_err(|error| {
        format!(
            "cannot recover stale {label} backup `{}` to target `{}`: {error}. Inspect both paths and move the backup manually before retrying",
            backup.display(),
            target.display()
        )
    })?;
    println!(
        "Recovered {label} from stale backup {} to {}",
        backup.display(),
        target.display()
    );
    Ok(())
}

fn replace_path(
    stage: &Path,
    target: &Path,
    prefix: &str,
    kind: BackupKind,
    label: &str,
) -> Result<(), String> {
    let parent = target
        .parent()
        .ok_or_else(|| format!("invalid {label} installation path {}", target.display()))?;
    let backup = if path_exists(target)? {
        let backup = unique_nonexistent_path(parent, prefix)?;
        fs::rename(target, &backup).map_err(|error| {
            format!(
                "cannot preserve existing {label} `{}` before replacement: {error}",
                target.display()
            )
        })?;
        Some(backup)
    } else {
        None
    };

    let failure_key = kind.failure_key();
    let replacement = injected_failure("DAGPIPE_TEST_FAIL_REPLACEMENT", failure_key)
        .map_or_else(|| fs::rename(stage, target), Err);
    if let Err(error) = replacement {
        if let Some(backup) = &backup {
            let recovery = injected_failure("DAGPIPE_TEST_FAIL_RECOVERY", failure_key)
                .map_or_else(|| fs::rename(backup, target), Err);
            return match recovery {
                Ok(()) => Err(format!(
                    "cannot replace {label} at `{}`: {error}; restored the previous {label} from backup `{}`",
                    target.display(),
                    backup.display()
                )),
                Err(recovery_error) => Err(format!(
                    "cannot replace {label} at `{}`: {error}; recovery failed while restoring backup `{}` to `{}`: {recovery_error}. Inspect both paths and restore the backup manually before retrying",
                    target.display(),
                    backup.display(),
                    target.display()
                )),
            };
        }
        return Err(format!(
            "cannot replace {label} at `{}`: {error}",
            target.display()
        ));
    }
    if let Some(backup) = backup {
        kind.remove(&backup).map_err(|error| {
            format!(
                "installed {label} at `{}` but cannot remove backup `{}`: {error}. Inspect both paths and remove or rename the backup before retrying",
                target.display(),
                backup.display()
            )
        })?;
    }
    Ok(())
}

fn injected_failure(variable: &str, key: &str) -> Option<io::Error> {
    #[cfg(debug_assertions)]
    {
        if env::var(variable).ok().as_deref() == Some(key) {
            let action = if variable.ends_with("REPLACEMENT") {
                "replacement"
            } else {
                "recovery"
            };
            return Some(io::Error::other(format!("injected {key} {action} failure")));
        }
    }
    #[cfg(not(debug_assertions))]
    {
        let _ = (variable, key);
    }
    None
}

fn print_sdk_path() -> Result<(), String> {
    println!("{}", render_cargo_path(&sdk_target()?, cfg!(windows)));
    Ok(())
}

fn user_home() -> Result<PathBuf, String> {
    #[cfg(windows)]
    {
        if let Some(path) = env_path("USERPROFILE") {
            return Ok(path);
        }
        if let Some(path) = env_path("HOME") {
            return Ok(path);
        }
        if let (Some(mut drive), Some(home)) = (nonempty_env("HOMEDRIVE"), nonempty_env("HOMEPATH"))
        {
            drive.push(home);
            return Ok(PathBuf::from(drive));
        }
    }
    #[cfg(not(windows))]
    {
        if let Some(path) = env_path("HOME") {
            return Ok(path);
        }
        if let Some(path) = env_path("USERPROFILE") {
            return Ok(path);
        }
    }
    Err("cannot determine the user home directory".to_owned())
}

fn sdk_target() -> Result<PathBuf, String> {
    let home = user_home()?;
    let local_appdata = if cfg!(windows) {
        env_path("LOCALAPPDATA")
    } else {
        None
    };
    Ok(sdk_target_for(
        &home,
        local_appdata.as_deref(),
        cfg!(windows),
    ))
}

fn sdk_target_for(home: &Path, local_appdata: Option<&Path>, windows: bool) -> PathBuf {
    if windows {
        local_appdata
            .map(Path::to_path_buf)
            .unwrap_or_else(|| home.join("AppData").join("Local"))
            .join("dagpipe")
            .join("sdk")
    } else {
        home.join(".local")
            .join("share")
            .join("dagpipe")
            .join("sdk")
    }
}

fn skill_target() -> Result<PathBuf, String> {
    Ok(skill_target_for(&user_home()?))
}

fn skill_target_for(home: &Path) -> PathBuf {
    home.join(".agents")
        .join("skills")
        .join("dagpipe-runtime")
        .join("SKILL.md")
}

fn cargo_install_root() -> Result<PathBuf, String> {
    if let Some(path) = env_path("CARGO_INSTALL_ROOT") {
        return Ok(path);
    }
    if let Some(path) = env_path("CARGO_HOME") {
        return Ok(path);
    }
    Ok(user_home()?.join(".cargo"))
}

fn render_cargo_path(path: &Path, windows: bool) -> String {
    let rendered = path.display().to_string();
    if windows {
        rendered.replace('\\', "/")
    } else {
        rendered
    }
}

fn nonempty_env(name: &str) -> Option<OsString> {
    env::var_os(name).filter(|value| !value.is_empty())
}

fn env_path(name: &str) -> Option<PathBuf> {
    nonempty_env(name).map(PathBuf::from)
}

fn package_field<'a>(manifest: &'a str, field: &str) -> Option<&'a str> {
    let mut in_package = false;
    for line in manifest.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_package = line == "[package]";
            continue;
        }
        if !in_package {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        if key.trim() != field {
            continue;
        }
        let value = value.split('#').next()?.trim();
        return value.strip_prefix('"')?.strip_suffix('"');
    }
    None
}

fn require_regular_file(path: &Path, label: &str) -> Result<(), String> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("{label} is unavailable at `{}`: {error}", path.display()))?;
    if is_link_or_reparse(&metadata) || !metadata.is_file() {
        return Err(format!(
            "{label} is not a regular file at `{}`",
            path.display()
        ));
    }
    Ok(())
}

fn require_directory(path: &Path, label: &str) -> Result<(), String> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("{label} is unavailable at `{}`: {error}", path.display()))?;
    if is_link_or_reparse(&metadata) || !metadata.is_dir() {
        return Err(format!(
            "{label} is not a directory at `{}`",
            path.display()
        ));
    }
    Ok(())
}

fn is_link_or_reparse(metadata: &fs::Metadata) -> bool {
    if metadata.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0400;
        return metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0;
    }
    #[cfg(not(windows))]
    {
        false
    }
}

fn path_exists(path: &Path) -> Result<bool, String> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(format!("cannot inspect `{}`: {error}", path.display())),
    }
}

fn copy_regular_file(source: &Path, target: &Path) -> Result<(), String> {
    require_regular_file(source, "source file")?;
    fs::copy(source, target).map_err(|error| {
        format!(
            "cannot copy `{}` to `{}`: {error}",
            source.display(),
            target.display()
        )
    })?;
    Ok(())
}

fn copy_tree(source: &Path, target: &Path) -> Result<(), String> {
    let metadata = fs::symlink_metadata(source)
        .map_err(|error| format!("cannot inspect `{}`: {error}", source.display()))?;
    if is_link_or_reparse(&metadata) {
        return Err(format!(
            "source tree contains a symlink or reparse point at `{}`",
            source.display()
        ));
    }
    if metadata.is_file() {
        return copy_regular_file(source, target);
    }
    if !metadata.is_dir() {
        return Err(format!(
            "source tree contains a special entry at `{}`",
            source.display()
        ));
    }
    fs::create_dir_all(target)
        .map_err(|error| format!("cannot create directory `{}`: {error}", target.display()))?;
    let mut entries = fs::read_dir(source)
        .map_err(|error| format!("cannot read directory `{}`: {error}", source.display()))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("cannot read directory `{}`: {error}", source.display()))?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        copy_tree(&entry.path(), &target.join(entry.file_name()))?;
    }
    Ok(())
}

fn create_temp_dir(parent: &Path, prefix: &str) -> Result<PathBuf, String> {
    for attempt in 0..1000 {
        let path = parent.join(format!("{prefix}{}-{attempt}", process::id()));
        match fs::create_dir(&path) {
            Ok(()) => return Ok(path),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(format!(
                    "cannot create temporary directory `{}`: {error}",
                    path.display()
                ));
            }
        }
    }
    Err(format!(
        "cannot allocate a temporary directory in `{}`",
        parent.display()
    ))
}

fn unique_nonexistent_path(parent: &Path, prefix: &str) -> Result<PathBuf, String> {
    for attempt in 0..1000 {
        let path = parent.join(format!("{prefix}{}-{attempt}", process::id()));
        if !path_exists(&path)? {
            return Ok(path);
        }
    }
    Err(format!(
        "cannot allocate a temporary path in `{}`",
        parent.display()
    ))
}

fn write_new_file(path: &Path, contents: &[u8]) -> Result<(), String> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|error| format!("cannot create `{}`: {error}", path.display()))?;
    file.write_all(contents)
        .map_err(|error| format!("cannot write `{}`: {error}", path.display()))
}

fn collect_tree_entries(root: &Path) -> Result<Vec<PathBuf>, String> {
    let mut entries = vec![PathBuf::from(".")];
    collect_tree_entries_inner(root, Path::new("."), &mut entries)?;
    entries.sort_by_key(|path| logical_path(path));
    Ok(entries)
}

fn collect_tree_entries_inner(
    root: &Path,
    relative: &Path,
    entries: &mut Vec<PathBuf>,
) -> Result<(), String> {
    let absolute = if relative == Path::new(".") {
        root.to_path_buf()
    } else {
        root.join(relative)
    };
    let mut children = fs::read_dir(&absolute)
        .map_err(|error| format!("cannot read directory `{}`: {error}", absolute.display()))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("cannot read directory `{}`: {error}", absolute.display()))?;
    children.sort_by_key(|entry| entry.file_name());
    for child in children {
        let child_relative = if relative == Path::new(".") {
            PathBuf::from(child.file_name())
        } else {
            relative.join(child.file_name())
        };
        let metadata = fs::symlink_metadata(child.path()).map_err(|error| {
            format!(
                "cannot inspect installed entry `{}`: {error}",
                child.path().display()
            )
        })?;
        if is_link_or_reparse(&metadata) {
            return Err(format!(
                "tree contains a symlink or reparse point at `{}`",
                child.path().display()
            ));
        }
        if metadata.is_dir() {
            entries.push(child_relative.clone());
            collect_tree_entries_inner(root, &child_relative, entries)?;
        } else if metadata.is_file() {
            entries.push(child_relative);
        } else {
            return Err(format!(
                "tree contains a special entry at `{}`",
                child.path().display()
            ));
        }
    }
    Ok(())
}

fn tree_manifest(root: &Path) -> Result<String, String> {
    let mut lines = String::new();
    for path in collect_tree_entries(root)? {
        if is_manifest_path(&path) {
            continue;
        }
        lines.push_str(&logical_path(&path));
        lines.push('\n');
    }
    Ok(lines)
}

fn file_manifest(root: &Path) -> Result<String, String> {
    let mut lines = Vec::new();
    for path in collect_tree_entries(root)? {
        if path == Path::new(".") || is_manifest_path(&path) {
            continue;
        }
        let absolute = root.join(&path);
        if !absolute.is_file() {
            continue;
        }
        lines.push(format!(
            "{}  {}",
            sha256_file(&absolute)?,
            logical_path(&path)
        ));
    }
    lines.sort();
    let mut manifest = String::new();
    for line in lines {
        manifest.push_str(&line);
        manifest.push('\n');
    }
    Ok(manifest)
}

fn is_manifest_path(path: &Path) -> bool {
    path == Path::new(INSTALLED_FILES) || path == Path::new(INSTALLED_TREE)
}

fn logical_path(path: &Path) -> String {
    if path == Path::new(".") {
        ".".to_owned()
    } else {
        format!("./{}", path.to_string_lossy().replace('\\', "/"))
    }
}

const SHA256_INITIAL_STATE: [u32; 8] = [
    0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19,
];

const SHA256_ROUND_CONSTANTS: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

struct Sha256 {
    state: [u32; 8],
    buffer: [u8; 64],
    buffer_len: usize,
    length: u64,
}

impl Sha256 {
    fn new() -> Self {
        Self {
            state: SHA256_INITIAL_STATE,
            buffer: [0; 64],
            buffer_len: 0,
            length: 0,
        }
    }

    fn update(&mut self, mut data: &[u8]) {
        self.length = self.length.wrapping_add(data.len() as u64);
        if self.buffer_len > 0 {
            let count = (64 - self.buffer_len).min(data.len());
            self.buffer[self.buffer_len..self.buffer_len + count].copy_from_slice(&data[..count]);
            self.buffer_len += count;
            data = &data[count..];
            if self.buffer_len == 64 {
                let block = self.buffer;
                self.compress(&block);
                self.buffer_len = 0;
            } else {
                return;
            }
        }
        while data.len() >= 64 {
            let block: [u8; 64] = data[..64].try_into().expect("64-byte block");
            self.compress(&block);
            data = &data[64..];
        }
        self.buffer[..data.len()].copy_from_slice(data);
        self.buffer_len = data.len();
    }

    fn finalize(mut self) -> [u8; 32] {
        let bit_length = self.length.wrapping_mul(8);
        self.buffer[self.buffer_len] = 0x80;
        self.buffer_len += 1;
        if self.buffer_len > 56 {
            self.buffer[self.buffer_len..].fill(0);
            let block = self.buffer;
            self.compress(&block);
            self.buffer = [0; 64];
            self.buffer_len = 0;
        }
        self.buffer[self.buffer_len..56].fill(0);
        self.buffer[56..64].copy_from_slice(&bit_length.to_be_bytes());
        let block = self.buffer;
        self.compress(&block);

        let mut digest = [0_u8; 32];
        for (index, word) in self.state.iter().enumerate() {
            digest[index * 4..index * 4 + 4].copy_from_slice(&word.to_be_bytes());
        }
        digest
    }

    fn compress(&mut self, block: &[u8; 64]) {
        let mut words = [0_u32; 64];
        for (index, chunk) in block.chunks_exact(4).enumerate() {
            words[index] = u32::from_be_bytes(chunk.try_into().expect("4-byte word"));
        }
        for index in 16..64 {
            let s0 = words[index - 15].rotate_right(7)
                ^ words[index - 15].rotate_right(18)
                ^ (words[index - 15] >> 3);
            let s1 = words[index - 2].rotate_right(17)
                ^ words[index - 2].rotate_right(19)
                ^ (words[index - 2] >> 10);
            words[index] = words[index - 16]
                .wrapping_add(s0)
                .wrapping_add(words[index - 7])
                .wrapping_add(s1);
        }

        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = self.state;
        for index in 0..64 {
            let sum1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let choose = (e & f) ^ (!e & g);
            let temp1 = h
                .wrapping_add(sum1)
                .wrapping_add(choose)
                .wrapping_add(SHA256_ROUND_CONSTANTS[index])
                .wrapping_add(words[index]);
            let sum0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let majority = (a & b) ^ (a & c) ^ (b & c);
            let temp2 = sum0.wrapping_add(majority);
            h = g;
            g = f;
            f = e;
            e = d.wrapping_add(temp1);
            d = c;
            c = b;
            b = a;
            a = temp1.wrapping_add(temp2);
        }

        self.state[0] = self.state[0].wrapping_add(a);
        self.state[1] = self.state[1].wrapping_add(b);
        self.state[2] = self.state[2].wrapping_add(c);
        self.state[3] = self.state[3].wrapping_add(d);
        self.state[4] = self.state[4].wrapping_add(e);
        self.state[5] = self.state[5].wrapping_add(f);
        self.state[6] = self.state[6].wrapping_add(g);
        self.state[7] = self.state[7].wrapping_add(h);
    }
}

fn sha256_file(path: &Path) -> Result<String, String> {
    let file =
        File::open(path).map_err(|error| format!("cannot open `{}`: {error}", path.display()))?;
    let digest = sha256_reader(file)
        .map_err(|error| format!("cannot read `{}`: {error}", path.display()))?;
    Ok(hex(&digest))
}

fn sha256_reader<R: Read>(mut reader: R) -> io::Result<[u8; 32]> {
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let count = reader.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    Ok(hasher.finalize())
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(DIGITS[(byte >> 4) as usize] as char);
        output.push(DIGITS[(byte & 0x0f) as usize] as char);
    }
    output
}

fn read_manifest(path: &Path) -> Result<String, String> {
    fs::read_to_string(path)
        .map_err(|error| format!("cannot read install manifest `{}`: {error}", path.display()))
}

fn manifest_lines(contents: &str) -> Vec<&str> {
    contents
        .lines()
        .map(|line| line.strip_suffix('\r').unwrap_or(line))
        .collect()
}

fn trees_equal(left: &Path, right: &Path) -> Result<bool, String> {
    Ok(
        manifest_lines(&tree_manifest(left)?) == manifest_lines(&tree_manifest(right)?)
            && manifest_lines(&file_manifest(left)?) == manifest_lines(&file_manifest(right)?),
    )
}

fn paths_equivalent(left: &Path, right: &Path) -> bool {
    if left == right {
        return true;
    }
    match (left.canonicalize(), right.canonicalize()) {
        (Ok(left), Ok(right)) => left == right,
        _ => false,
    }
}

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(path: PathBuf) -> Self {
        Self { path }
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        if self.path.exists() {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}

fn print_help() {
    println!(
        "DAGpipe framework governance CLI\n\n\
Usage:\n\
  dagpipe modules list\n\
  dagpipe graph validate <graph.json>\n\
  dagpipe graph inspect <graph.json>\n\
  dagpipe install --source <checkout>\n\
  dagpipe sdk path\n\
  dagpipe skill install\n\
  dagpipe --version\n\n\
The CLI validates one external object flow per SESE Graph and inspects\n\
operator bindings. Validate each project source as a separate Graph.\n\
Project module internals need not themselves be DAGs.\n\
Projects compile and execute their registered Operators through the Rust SDK."
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_native_and_cargo_paths() {
        assert_eq!(
            render_cargo_path(Path::new("/home/jane/.local/share/dagpipe/sdk"), false),
            "/home/jane/.local/share/dagpipe/sdk"
        );
        assert_eq!(
            render_cargo_path(
                Path::new(r"C:\Users\Jane Doe\AppData\Local\dagpipe\sdk"),
                true
            ),
            "C:/Users/Jane Doe/AppData/Local/dagpipe/sdk"
        );
    }

    #[test]
    fn selects_platform_sdk_roots() {
        let home = Path::new("/home/jane");
        assert_eq!(
            sdk_target_for(home, None, false),
            PathBuf::from("/home/jane/.local/share/dagpipe/sdk")
        );
        assert_eq!(
            sdk_target_for(home, Some(Path::new("C:/Users/Jane/AppData/Local")), true),
            PathBuf::from("C:/Users/Jane/AppData/Local/dagpipe/sdk")
        );
        assert_eq!(
            sdk_target_for(home, None, true),
            PathBuf::from(r"/home/jane/AppData/Local/dagpipe/sdk")
        );
    }

    #[test]
    fn reads_package_identity_fields() {
        let manifest = "[package]\nname = \"pipeline_runtime\"\nrepository = \"https://github.com/Jasonzhangf/appsdk\"\n\n[dependencies]\nname = \"ignored\"\n";
        assert_eq!(package_field(manifest, "name"), Some("pipeline_runtime"));
        assert_eq!(
            package_field(manifest, "repository"),
            Some("https://github.com/Jasonzhangf/appsdk")
        );
    }

    #[test]
    fn computes_sha256_for_manifest_hashes() {
        let mut hasher = Sha256::new();
        hasher.update(b"abc");
        assert_eq!(
            hex(&hasher.finalize()),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );

        let mut hasher = Sha256::new();
        hasher.update(b"");
        assert_eq!(
            hex(&hasher.finalize()),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );

        let mut hasher = Sha256::new();
        hasher.update(&[b'a'; 64]);
        assert_eq!(
            hex(&hasher.finalize()),
            "ffe054fe7ae0cb6dc65c3af9b61d5209f439851db43d0ba5997337df154668eb"
        );

        let mut hasher = Sha256::new();
        hasher.update(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq");
        assert_eq!(
            hex(&hasher.finalize()),
            "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"
        );
    }

    fn digest_chunks(chunks: &[&[u8]]) -> String {
        let mut hasher = Sha256::new();
        for chunk in chunks {
            hasher.update(chunk);
        }
        hex(&hasher.finalize())
    }

    #[test]
    fn computes_sha256_across_update_boundaries() {
        assert_eq!(
            digest_chunks(&[b"a".as_slice(), b"bc".as_slice()]),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );

        let mut one_byte_chunks = Sha256::new();
        for _ in 0..64 {
            one_byte_chunks.update(b"a");
        }
        assert_eq!(
            hex(&one_byte_chunks.finalize()),
            "ffe054fe7ae0cb6dc65c3af9b61d5209f439851db43d0ba5997337df154668eb"
        );

        let bytes = [b'a'; 130];
        let mut multiple_blocks = Sha256::new();
        for chunk in bytes.chunks(63) {
            multiple_blocks.update(chunk);
        }
        assert_eq!(
            hex(&multiple_blocks.finalize()),
            "1e3c4f4750c8c29bbfa9ced317788176b156d342e57f7777f62fd7221a44312f"
        );
    }

    struct OneByteReader<R> {
        inner: R,
    }

    impl<R: std::io::Read> std::io::Read for OneByteReader<R> {
        fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
            if buffer.is_empty() {
                return Ok(0);
            }
            self.inner.read(&mut buffer[..1])
        }
    }

    #[test]
    fn hashes_real_files_through_repeated_short_reads() {
        static NEXT_SHA_FILE: std::sync::atomic::AtomicUsize =
            std::sync::atomic::AtomicUsize::new(0);

        let path = env::temp_dir().join(format!(
            "dagpipe-sha256-{}-{}.tmp",
            process::id(),
            NEXT_SHA_FILE.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        let contents = b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq";
        fs::write(&path, contents).expect("write SHA fixture");
        let file = File::open(&path).expect("open SHA fixture");
        let digest = sha256_reader(OneByteReader { inner: file }).expect("hash short reads");
        let expected = "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1";
        assert_eq!(hex(&digest), expected);
        assert_eq!(sha256_file(&path).expect("hash fixture file"), expected);
        let _ = fs::remove_file(path);
    }
}
