use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

fn binary() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_appsdk"))
}

fn temp_root(label: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "appsdk-registry-home-{label}-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ))
}

fn command(project: &Path) -> Command {
    let mut command = Command::new(binary());
    command
        .arg("new")
        .arg(project)
        .env_remove("APPSDK_HOME")
        .env_remove("HOME")
        .env_remove("USERPROFILE")
        .env_remove("TMUX_PANE");
    command
}

fn output_text(output: &Output) -> String {
    format!(
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn receipt(output: &Output) -> Value {
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .find_map(|line| {
            line.strip_prefix("appsdk-registration ")
                .map(|value| serde_json::from_str::<Value>(value).unwrap())
        })
        .expect("new must emit a registration receipt")
}

fn canonical(path: &Path) -> String {
    fs::canonicalize(path)
        .unwrap()
        .to_str()
        .unwrap()
        .to_string()
}

fn assert_registered(registry: &Path, project: &Path, output: &Output) -> Value {
    let receipt = receipt(output);
    assert_eq!(receipt["registry_root"], canonical(registry));
    assert_eq!(receipt["project_root"], canonical(project));
    assert_eq!(receipt["sdk_version"], "0.1.0015");
    assert_eq!(receipt["idempotent"], false);

    let registry_file = registry.join("projects.jsonl");
    let lines = fs::read_to_string(&registry_file).unwrap();
    assert_eq!(lines.lines().count(), 1);
    let event: Value = serde_json::from_str(lines.lines().next().unwrap()).unwrap();
    assert_eq!(event["event"], "project.registered");
    assert_eq!(event["project_root"], receipt["project_root"]);
    assert_eq!(event["project_id"], receipt["project_id"]);
    assert_eq!(event["sdk_version"], "0.1.0015");
    assert!(project.join(".appsdk/project.json").is_file());
    receipt
}

#[test]
fn registry_home_preserves_override_and_errors() {
    let workspace = temp_root("override");
    let project = workspace.join("project");
    let registry = workspace.join("registry");
    let home = workspace.join("home");
    let empty_override_project = workspace.join("empty-override-project");
    let unavailable_project = workspace.join("unavailable-project");
    fs::create_dir_all(&workspace).unwrap();

    let created = command(&project)
        .env("APPSDK_HOME", &registry)
        .env("HOME", &home)
        .env("USERPROFILE", workspace.join("other-profile"))
        .output()
        .unwrap();
    assert!(created.status.success(), "{}", output_text(&created));
    assert_registered(&registry, &project, &created);
    assert!(!home.join(".appsdk").exists());

    let relative_project = workspace.join("relative-project");
    let mut relative = command(&relative_project);
    relative
        .env("APPSDK_HOME", "relative-registry")
        .env("HOME", &home)
        .env("USERPROFILE", workspace.join("other-profile"));
    let output = relative.output().unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains(
        "GLOBAL_PROJECT_REGISTRATION_FAILED:GLOBAL_APPSDK_HOME_INVALID: path must be absolute"
    ));
    assert!(!relative_project.join(".appsdk").exists());
    assert!(!home.join(".appsdk").exists());

    let invalid_home_project = workspace.join("invalid-home-project");
    let mut invalid_home = command(&invalid_home_project);
    invalid_home
        .env_remove("APPSDK_HOME")
        .env("HOME", "relative-home")
        .env("USERPROFILE", workspace.join("other-profile"));
    let output = invalid_home.output().unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains(
        "GLOBAL_PROJECT_REGISTRATION_FAILED:GLOBAL_APPSDK_HOME_INVALID: path must be absolute"
    ));
    assert!(!invalid_home_project.join(".appsdk").exists());
    assert!(!workspace.join("other-profile/.appsdk").exists());

    let mut empty_override = command(&empty_override_project);
    empty_override
        .env("APPSDK_HOME", "")
        .env("HOME", &home)
        .env("USERPROFILE", workspace.join("other-profile"));
    let output = empty_override.output().unwrap();
    assert!(output.status.success(), "{}", output_text(&output));
    assert_registered(&home.join(".appsdk"), &empty_override_project, &output);
    assert!(!workspace.join("other-profile/.appsdk").exists());

    let mut unavailable = command(&unavailable_project);
    unavailable
        .env_remove("APPSDK_HOME")
        .env_remove("HOME")
        .env_remove("USERPROFILE");
    let output = unavailable.output().unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr)
        .contains("GLOBAL_PROJECT_REGISTRATION_FAILED:GLOBAL_APPSDK_HOME_UNAVAILABLE"));
    assert!(!unavailable_project.join(".appsdk").exists());

    fs::remove_dir_all(workspace).unwrap();
}

#[test]
fn registry_home_preserves_home_default() {
    let workspace = temp_root("home-default");
    let project = workspace.join("project");
    let home = workspace.join("home");
    let userprofile = workspace.join("profile");
    fs::create_dir_all(&workspace).unwrap();

    let created = command(&project)
        .env_remove("APPSDK_HOME")
        .env("HOME", &home)
        .env("USERPROFILE", &userprofile)
        .output()
        .unwrap();
    assert!(created.status.success(), "{}", output_text(&created));
    assert_registered(&home.join(".appsdk"), &project, &created);
    assert!(!userprofile.join(".appsdk").exists());

    let verify = Command::new(binary())
        .arg("verify")
        .arg(&project)
        .env_remove("APPSDK_HOME")
        .env("HOME", &home)
        .env("USERPROFILE", &userprofile)
        .env_remove("TMUX_PANE")
        .output()
        .unwrap();
    assert!(verify.status.success(), "{}", output_text(&verify));
    let result: Value = serde_json::from_slice(&verify.stdout).unwrap();
    assert_eq!(result["command_ok"], true);
    assert_eq!(result["development_ready"], true);

    fs::remove_dir_all(workspace).unwrap();
}

#[cfg(windows)]
#[test]
fn registry_home_uses_userprofile_without_home() {
    let workspace = temp_root("userprofile-only");
    let project = workspace.join("project");
    let userprofile = workspace.join("profile");
    fs::create_dir_all(&workspace).unwrap();

    let created = command(&project)
        .env_remove("APPSDK_HOME")
        .env_remove("HOME")
        .env("USERPROFILE", &userprofile)
        .output()
        .unwrap();
    assert!(created.status.success(), "{}", output_text(&created));
    assert_registered(&userprofile.join(".appsdk"), &project, &created);

    let verify = Command::new(binary())
        .arg("verify")
        .arg(&project)
        .env_remove("APPSDK_HOME")
        .env_remove("HOME")
        .env("USERPROFILE", &userprofile)
        .env_remove("TMUX_PANE")
        .output()
        .unwrap();
    assert!(verify.status.success(), "{}", output_text(&verify));
    let result: Value = serde_json::from_slice(&verify.stdout).unwrap();
    assert_eq!(result["command_ok"], true);
    assert_eq!(result["development_ready"], true);

    let relative_project = workspace.join("relative-project");
    let mut relative = command(&relative_project);
    relative
        .env_remove("APPSDK_HOME")
        .env_remove("HOME")
        .env("USERPROFILE", "relative-profile");
    let output = relative.output().unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains(
        "GLOBAL_PROJECT_REGISTRATION_FAILED:GLOBAL_APPSDK_HOME_INVALID: path must be absolute"
    ));
    assert!(!relative_project.join(".appsdk").exists());

    let empty_project = workspace.join("empty-project");
    let mut empty = command(&empty_project);
    empty
        .env_remove("APPSDK_HOME")
        .env_remove("HOME")
        .env("USERPROFILE", "");
    let output = empty.output().unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr)
        .contains("GLOBAL_PROJECT_REGISTRATION_FAILED:GLOBAL_APPSDK_HOME_UNAVAILABLE"));
    assert!(!empty_project.join(".appsdk").exists());

    fs::remove_dir_all(workspace).unwrap();
}
