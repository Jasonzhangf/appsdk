use std::{
    env, fs,
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicUsize, Ordering},
};

static NEXT_ROOT: AtomicUsize = AtomicUsize::new(0);

struct TestRoot {
    path: PathBuf,
}

impl TestRoot {
    fn new() -> Self {
        let sequence = NEXT_ROOT.fetch_add(1, Ordering::Relaxed);
        let path = env::temp_dir().join(format!(
            "dagpipe install test-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir_all(&path).expect("create isolated installer test root");
        Self { path }
    }
}

impl Drop for TestRoot {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

struct InstallerFixture {
    root: TestRoot,
    source: PathBuf,
    home: PathBuf,
    local_appdata: PathBuf,
    cargo_root: PathBuf,
    sdk: PathBuf,
    skill: PathBuf,
    binary: PathBuf,
}

impl InstallerFixture {
    fn new() -> Self {
        let root = TestRoot::new();
        let home = root.path.join("home with spaces");
        let local_appdata = root.path.join("local app data");
        let cargo_root = root.path.join("cargo root");
        let sdk = if cfg!(windows) {
            local_appdata.join("dagpipe").join("sdk")
        } else {
            home.join(".local")
                .join("share")
                .join("dagpipe")
                .join("sdk")
        };
        let skill = home
            .join(".agents")
            .join("skills")
            .join("dagpipe-runtime")
            .join("SKILL.md");
        let binary = cargo_root.join("bin").join(if cfg!(windows) {
            "dagpipe.exe"
        } else {
            "dagpipe"
        });
        Self {
            root,
            source: PathBuf::from(env!("CARGO_MANIFEST_DIR")),
            home,
            local_appdata,
            cargo_root,
            sdk,
            skill,
            binary,
        }
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_dagpipe"));
        command
            .args(args)
            .env("HOME", &self.home)
            .env("USERPROFILE", &self.home)
            .env("LOCALAPPDATA", &self.local_appdata)
            .env("CARGO_INSTALL_ROOT", &self.cargo_root)
            .env_remove("CARGO_HOME");
        command
    }

    fn install(&self) -> std::process::Output {
        self.install_with_env(&[])
    }

    fn install_with_env(&self, envs: &[(&str, &str)]) -> std::process::Output {
        let mut command = self.command(&["install", "--source"]);
        for (key, value) in envs {
            command.env(key, value);
        }
        command
            .arg(&self.source)
            .output()
            .expect("run dagpipe installer")
    }

    fn sdk_path(&self) -> std::process::Output {
        self.command(&["sdk", "path"])
            .output()
            .expect("run dagpipe sdk path")
    }

    fn cargo_safe_sdk_path(&self) -> String {
        self.sdk.to_string_lossy().replace('\\', "/")
    }
}

fn assert_success(output: &std::process::Output, label: &str) {
    assert!(
        output.status.success(),
        "{label} failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn assert_failure_contains(output: &std::process::Output, needles: &[&str], label: &str) {
    assert!(
        !output.status.success(),
        "{label} unexpectedly succeeded\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    for needle in needles {
        assert!(
            stderr.contains(needle),
            "{label} missing `{needle}`\nstderr:\n{stderr}"
        );
    }
}

fn paths_with_prefix(parent: &Path, prefix: &str) -> Vec<PathBuf> {
    let mut paths = fs::read_dir(parent)
        .expect("read backup parent")
        .map(|entry| entry.expect("read backup entry").path())
        .filter(|path| {
            path.file_name()
                .map(|name| name.to_string_lossy().starts_with(prefix))
                .unwrap_or(false)
        })
        .collect::<Vec<_>>();
    paths.sort();
    paths
}

#[test]
fn installs_upgrades_and_resolves_a_real_cargo_consumer_with_spaces() {
    let fixture = InstallerFixture::new();

    assert_success(&fixture.install(), "initial install");
    assert!(fixture.binary.is_file());
    assert!(fixture.skill.is_file());
    assert!(fixture.sdk.join("Cargo.toml").is_file());
    assert!(fixture.sdk.join(".installed-files").is_file());
    assert!(fixture.sdk.join(".installed-tree").is_file());

    let path_output = fixture.sdk_path();
    assert_success(&path_output, "sdk path");
    assert_eq!(
        String::from_utf8(path_output.stdout)
            .expect("sdk path is UTF-8")
            .trim(),
        fixture.cargo_safe_sdk_path()
    );

    fs::remove_file(&fixture.skill).expect("remove installed skill for direct install test");
    let skill_output = fixture
        .command(&["skill", "install"])
        .output()
        .expect("run skill install");
    assert_success(&skill_output, "skill install");
    assert!(fixture.skill.is_file());

    let consumer = fixture.root.path.join("consumer with spaces");
    fs::create_dir_all(consumer.join("src")).expect("create consumer");
    fs::write(
        consumer.join("Cargo.toml"),
        format!(
            "[package]\nname = \"dagpipe-consumer\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\npipeline_runtime = {{ path = \"{}\" }}\n",
            fixture.cargo_safe_sdk_path().replace('"', "\\\"")
        ),
    )
    .expect("write consumer manifest");
    fs::write(
        consumer.join("src").join("main.rs"),
        "fn main() { let _ = pipeline_runtime::Registry::default(); }\n",
    )
    .expect("write consumer source");

    let target = fixture.root.path.join("consumer-target");
    let output = Command::new("cargo")
        .args([
            "check",
            "--manifest-path",
            consumer.join("Cargo.toml").to_str().expect("UTF-8 path"),
            "--offline",
        ])
        .env("CARGO_TARGET_DIR", target)
        .output()
        .expect("run cargo check");
    assert_success(&output, "real Cargo consumer");

    assert_success(&fixture.install(), "upgrade install");
    assert!(fixture.sdk.join("Cargo.toml").is_file());
}

#[test]
fn refuses_unowned_sdk_content_and_preserves_it() {
    let fixture = InstallerFixture::new();
    assert_success(&fixture.install(), "initial install");

    let unowned = fixture.sdk.join("unowned.txt");
    fs::write(&unowned, "keep this unowned file\n").expect("write unowned file");
    let output = fixture.install();
    assert!(!output.status.success(), "unowned SDK content was replaced");
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("differs"),
        "unexpected refusal: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        fs::read_to_string(&unowned).expect("read unowned file"),
        "keep this unowned file\n"
    );
}

#[test]
fn refuses_unowned_skill_before_mutating_install() {
    let fixture = InstallerFixture::new();
    fs::create_dir_all(fixture.skill.parent().expect("skill parent"))
        .expect("create skill directory");
    fs::write(&fixture.skill, "---\nname: other-skill\n---\n").expect("write unowned skill");

    let output = fixture.install();
    assert!(!output.status.success(), "unowned Skill was overwritten");
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("refusing to overwrite"),
        "unexpected refusal: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!fixture.binary.exists());
    assert!(!fixture.sdk.exists());
    assert_eq!(
        fs::read_to_string(&fixture.skill).expect("read unowned skill"),
        "---\nname: other-skill\n---\n"
    );
}

#[test]
fn replacement_failure_restores_previous_sdk_and_reports_paths() {
    let fixture = InstallerFixture::new();
    assert_success(&fixture.install(), "initial install");
    let manifest =
        fs::read_to_string(fixture.sdk.join("Cargo.toml")).expect("read installed SDK manifest");

    let output = fixture.install_with_env(&[("DAGPIPE_TEST_FAIL_REPLACEMENT", "sdk")]);
    assert_failure_contains(
        &output,
        &[
            "cannot replace DAGpipe SDK",
            "injected sdk replacement failure",
            fixture.sdk.to_str().expect("SDK path is UTF-8"),
            ".sdk-previous.",
            "restored",
        ],
        "SDK replacement failure",
    );
    assert!(fixture.sdk.join("Cargo.toml").is_file());
    assert_eq!(
        fs::read_to_string(fixture.sdk.join("Cargo.toml")).expect("read restored SDK manifest"),
        manifest
    );
    assert!(
        paths_with_prefix(fixture.sdk.parent().expect("SDK parent"), ".sdk-previous.").is_empty(),
        "restored SDK left a backup"
    );

    let version = Command::new(&fixture.binary)
        .arg("--version")
        .output()
        .expect("run restored CLI");
    assert_success(&version, "restored CLI after SDK replacement failure");
}

#[test]
fn recovery_failure_reports_paths_and_next_install_recovers_stale_sdk() {
    let fixture = InstallerFixture::new();
    assert_success(&fixture.install(), "initial install");

    let output = fixture.install_with_env(&[
        ("DAGPIPE_TEST_FAIL_REPLACEMENT", "sdk"),
        ("DAGPIPE_TEST_FAIL_RECOVERY", "sdk"),
    ]);
    let backups = paths_with_prefix(fixture.sdk.parent().expect("SDK parent"), ".sdk-previous.");
    assert_eq!(backups.len(), 1, "expected one interrupted SDK backup");
    let backup = backups[0].to_string_lossy().into_owned();
    assert_failure_contains(
        &output,
        &[
            "injected sdk replacement failure",
            "recovery failed",
            "injected sdk recovery failure",
            fixture.sdk.to_str().expect("SDK path is UTF-8"),
            backup.as_str(),
            "restore the backup manually",
        ],
        "SDK recovery failure",
    );
    assert!(!fixture.sdk.exists());

    assert_success(&fixture.install(), "recover stale SDK backup");
    assert!(fixture.sdk.join("Cargo.toml").is_file());
    assert!(
        !Path::new(&backup).exists(),
        "stale backup was not consumed by recovery"
    );
}

#[test]
fn stale_binary_backup_is_recovered_before_reinstall() {
    let fixture = InstallerFixture::new();
    assert_success(&fixture.install(), "initial install");
    let bin_dir = fixture.binary.parent().expect("binary parent");
    let backup = bin_dir.join(format!(".dagpipe-previous.stale-{}", std::process::id()));
    fs::rename(&fixture.binary, &backup).expect("simulate interrupted CLI replacement");

    assert_success(&fixture.install(), "reinstall with stale CLI backup");
    assert!(fixture.binary.is_file());
    assert!(!backup.exists());
    assert!(paths_with_prefix(bin_dir, ".dagpipe-previous.").is_empty());
}

#[test]
fn stale_backup_with_existing_target_is_not_overwritten() {
    let fixture = InstallerFixture::new();
    assert_success(&fixture.install(), "initial install");
    let backup = fixture
        .sdk
        .parent()
        .expect("SDK parent")
        .join(format!(".sdk-previous.stale-{}", std::process::id()));
    fs::create_dir_all(&backup).expect("create stale backup");
    fs::write(backup.join("user-data.txt"), "keep this user data\n")
        .expect("write user data in stale backup");

    let output = fixture.install();
    assert_failure_contains(
        &output,
        &[
            "stale DAGpipe SDK backup",
            fixture.sdk.to_str().expect("SDK path is UTF-8"),
            backup.to_str().expect("backup path is UTF-8"),
            "refusing",
        ],
        "ambiguous stale backup",
    );
    assert!(fixture.sdk.join("Cargo.toml").is_file());
    assert_eq!(
        fs::read_to_string(backup.join("user-data.txt")).expect("read stale backup user data"),
        "keep this user data\n"
    );
}

#[cfg(unix)]
#[test]
fn shell_entry_delegates_to_the_shared_owner() {
    use std::os::unix::fs::PermissionsExt;

    let fixture = InstallerFixture::new();
    let fake_bin = fixture.root.path.join("fake cargo bin");
    fs::create_dir_all(&fake_bin).expect("create fake cargo bin");
    let fake_cargo = fake_bin.join("cargo");
    fs::write(
        &fake_cargo,
        r#"#!/bin/sh
set -eu
root=''
while [ "$#" -gt 0 ]; do
    case "$1" in
        --root)
            root=$2
            shift 2
            ;;
        *)
            shift
            ;;
    esac
done
mkdir -p "$root/bin"
cp "$DAGPIPE_TEST_BINARY" "$root/bin/dagpipe"
chmod 0755 "$root/bin/dagpipe"
"#,
    )
    .expect("write fake cargo");
    fs::set_permissions(&fake_cargo, fs::Permissions::from_mode(0o755))
        .expect("make fake cargo executable");

    let install_sh = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("scripts")
        .join("install.sh");
    let path = format!(
        "{}:/usr/bin:/bin",
        fake_bin.to_str().expect("fake bin path is UTF-8")
    );
    let output = Command::new("/bin/sh")
        .arg(install_sh)
        .env("PATH", path)
        .env("DAGPIPE_TEST_BINARY", env!("CARGO_BIN_EXE_dagpipe"))
        .env("HOME", &fixture.home)
        .env("USERPROFILE", &fixture.home)
        .env("LOCALAPPDATA", &fixture.local_appdata)
        .env("CARGO_INSTALL_ROOT", &fixture.cargo_root)
        .env_remove("CARGO_HOME")
        .output()
        .expect("run install.sh");
    assert_success(&output, "shell installer entry");
    assert!(fixture.binary.is_file());
    assert!(fixture.sdk.join("Cargo.toml").is_file());
    assert!(fixture.skill.is_file());
}
