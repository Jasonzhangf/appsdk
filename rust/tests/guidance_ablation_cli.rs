use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

fn binary() -> PathBuf {
    env::var_os("APPSDK_TEST_BINARY")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_BIN_EXE_appsdk")))
}

/// Unique scratch area under the worktree `target/`; removed on drop.
struct Scratch {
    base: PathBuf,
}

impl Scratch {
    fn new() -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let base = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join(format!(
                "appsdk-guidance-ablation-{}-{}",
                std::process::id(),
                nanos
            ));
        fs::create_dir_all(base.join("appsdk-home")).unwrap();
        fs::create_dir_all(base.join("collab-state")).unwrap();
        Self { base }
    }

    fn root(&self) -> PathBuf {
        self.base.join("project")
    }

    fn cmd(&self) -> Command {
        let mut command = Command::new(binary());
        command
            .env("APPSDK_HOME", self.base.join("appsdk-home"))
            .env("COLLAB_STATE_DIR", self.base.join("collab-state"));
        command
    }

    fn temp_root(&self, name: &str) -> PathBuf {
        self.base.join(name)
    }

    fn run(&self, name: &str, args: &[&str]) -> Output {
        let mut command = self.cmd();
        command.current_dir(self.temp_root(name)).args(args);
        command.output().unwrap()
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.base);
    }
}

fn init_git(root: &Path) {
    for args in [
        &["init"][..],
        &["config", "maintenance.auto", "false"],
        &["config", "user.email", "test@appsdk.local"],
        &["config", "user.name", "AppSDK Test"],
        &["add", "."],
        &["commit", "-m", "baseline"],
        &["branch", "-M", "codex/test"],
    ] {
        assert!(Command::new("git")
            .arg("-C")
            .arg(root)
            .args(args)
            .status()
            .unwrap()
            .success());
    }
}

fn git_test_value(root: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .unwrap();
    assert!(output.status.success());
    String::from_utf8(output.stdout).unwrap().trim().to_string()
}

#[test]
fn guidance_active_plan_rejects_revoked_scope_paths() {
    let scratch = Scratch::new();
    let root = scratch.temp_root("guidance-revoked-scope");
    let root_text = root.to_str().unwrap();
    fs::create_dir_all(&root).unwrap();
    assert!(scratch
        .run("guidance-revoked-scope", &["new", "."])
        .status
        .success());
    assert!(scratch
        .run("guidance-revoked-scope", &["guide", "compile", "."])
        .status
        .success());
    init_git(&root);
    fs::write(
        root.join("plan.json"),
        serde_json::to_string_pretty(&serde_json::json!({
            "schema_version": 1,
            "mode": "develop",
            "goal_id": "goal-change-me",
            "task_id": "task-revoked-scope",
            "module_id": "app-core",
            "objective": "reject work after scope ownership is revoked",
            "scope_paths": ["playground/experiments/**"],
            "steps": [
                {"step_id":"step-1","node_id":"requirements","action":"analyze","owner":"app-core","expected_evidence":["requirements"]},
                {"step_id":"step-2","node_id":"map_check","action":"bind maps","owner":"app-core","expected_evidence":["function-map-binding","verification-map-binding"]}
            ]
        }))
        .unwrap(),
    )
    .unwrap();
    let planned = scratch.run(
        "guidance-revoked-scope",
        &[
            "guide",
            "plan",
            root_text,
            "--task",
            "task-revoked-scope",
            "--input",
            "plan.json",
        ],
    );
    assert!(
        planned.status.success(),
        "{}",
        String::from_utf8_lossy(&planned.stderr)
    );
    let accepted = serde_json::json!({
        "schema_version":1,"event_id":"accepted-1","step_id":"step-1","result":"pass",
        "observations":["requirements recorded"],"evidence":["requirements-1"]
    });
    fs::write(root.join("accepted.json"), accepted.to_string()).unwrap();
    let first = scratch.run(
        "guidance-revoked-scope",
        &[
            "guide",
            "update",
            root_text,
            "--task",
            "task-revoked-scope",
            "--input",
            "accepted.json",
        ],
    );
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );

    let project_path = root.join(".appsdk/project.json");
    let mut project: serde_json::Value =
        serde_json::from_slice(&fs::read(&project_path).unwrap()).unwrap();
    let module = project["modules"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|module| module["module_id"] == "app-core")
        .unwrap();
    module["owned_paths"] = serde_json::json!(["tests/core/**"]);
    module["contract_paths"] = serde_json::json!([]);
    fs::write(
        &project_path,
        serde_json::to_string_pretty(&project).unwrap(),
    )
    .unwrap();

    let replay = scratch.run(
        "guidance-revoked-scope",
        &[
            "guide",
            "update",
            root_text,
            "--task",
            "task-revoked-scope",
            "--input",
            "accepted.json",
        ],
    );
    assert!(
        replay.status.success(),
        "{}",
        String::from_utf8_lossy(&replay.stderr)
    );
    let replay_json: serde_json::Value = serde_json::from_slice(&replay.stdout).unwrap();
    assert_eq!(replay_json["idempotent"], true);
    let events_path = root.join(".appsdk-control/guidance/task-revoked-scope/events.jsonl");
    let events_before = fs::read_to_string(&events_path).unwrap();
    let next = scratch.run(
        "guidance-revoked-scope",
        &["guide", "next", root_text, "--task", "task-revoked-scope"],
    );
    assert!(
        next.status.success(),
        "{}",
        String::from_utf8_lossy(&next.stderr)
    );
    let next_json: serde_json::Value = serde_json::from_slice(&next.stdout).unwrap();
    fs::write(
        root.join("fresh.json"),
        serde_json::json!({
            "schema_version":1,"event_id":"fresh-2","step_id":"step-2","result":"pass",
            "observations":["maps recorded"],"evidence":["map-1"]
        })
        .to_string(),
    )
    .unwrap();
    let fresh = scratch.run(
        "guidance-revoked-scope",
        &[
            "guide",
            "update",
            root_text,
            "--task",
            "task-revoked-scope",
            "--input",
            "fresh.json",
        ],
    );
    assert_eq!(
        next_json["reason_code"],
        "GUIDANCE_CONTEXT_DRIFT:scope",
        "revoked scope remained admitted; fresh update success={}, stdout={}",
        fresh.status.success(),
        String::from_utf8_lossy(&fresh.stdout)
    );
    assert_eq!(next_json["readiness"], "blocked");
    assert_eq!(next_json["next"]["revision_reason"], "scope_changed");
    assert!(!fresh.status.success());
    assert!(String::from_utf8_lossy(&fresh.stderr).contains("GUIDANCE_CONTEXT_DRIFT:scope"));
    assert_eq!(fs::read_to_string(&events_path).unwrap(), events_before);
}

#[test]
fn guidance_scope_state_detects_content_change_without_git_status_shape_change() {
    let scratch = Scratch::new();
    let root = scratch.temp_root("guidance-scope-content-drift");
    let root_text = root.to_str().unwrap();
    fs::create_dir_all(&root).unwrap();
    assert!(scratch
        .run("guidance-scope-content-drift", &["new", root_text])
        .status
        .success());
    assert!(scratch
        .run(
            "guidance-scope-content-drift",
            &["guide", "compile", root_text]
        )
        .status
        .success());
    let scope_dir = root.join("playground/experiments");
    fs::create_dir_all(&scope_dir).unwrap();
    fs::write(scope_dir.join("input.txt"), "v1\n").unwrap();
    init_git(&root);

    let plan_file = root.join("plan.json");
    let proposal = serde_json::json!({
        "schema_version": 1,
        "mode": "develop",
        "goal_id": "goal-change-me",
        "task_id": "task-content-drift",
        "module_id": "app-core",
        "objective": "continue the plan after ordinary source changes",
        "scope_paths": ["playground/experiments/input.txt"],
        "steps": [{
            "step_id": "step-1",
            "node_id": "requirements",
            "action": "analyze requirements",
            "owner": "app-core",
            "expected_evidence": ["requirements"]
        }]
    });
    fs::write(
        &plan_file,
        serde_json::to_string_pretty(&proposal).unwrap() + "\n",
    )
    .unwrap();
    assert!(scratch
        .run(
            "guidance-scope-content-drift",
            &[
                "guide",
                "plan",
                root_text,
                "--task",
                "task-content-drift",
                "--input",
                "plan.json",
            ],
        )
        .status
        .success());

    fs::write(scope_dir.join("input.txt"), "v2\n").unwrap();
    let status = scratch.run(
        "guidance-scope-content-drift",
        &["guide", "next", root_text, "--task", "task-content-drift"],
    );
    assert!(
        status.status.success(),
        "{}",
        String::from_utf8_lossy(&status.stderr)
    );
    let payload: serde_json::Value = serde_json::from_slice(&status.stdout).unwrap();
    assert_eq!(payload["reason_code"], "NEXT_STEP_READY");

    git_test_value(&root, &["add", "playground/experiments/input.txt"]);
    git_test_value(&root, &["commit", "-qm", "update scoped input"]);
    let status = scratch.run(
        "guidance-scope-content-drift",
        &["guide", "next", root_text, "--task", "task-content-drift"],
    );
    assert!(
        status.status.success(),
        "{}",
        String::from_utf8_lossy(&status.stderr)
    );
    let payload: serde_json::Value = serde_json::from_slice(&status.stdout).unwrap();
    assert_eq!(payload["reason_code"], "NEXT_STEP_READY");

    fs::write(
        root.join("result.json"),
        r#"{"schema_version":1,"event_id":"source-change-1","step_id":"step-1","result":"pass","observations":["scoped input changed and committed; next step remains ready"],"evidence":["playground/experiments/input.txt"]}
"#,
    )
    .unwrap();
    let updated = scratch.run(
        "guidance-scope-content-drift",
        &[
            "guide",
            "update",
            root_text,
            "--task",
            "task-content-drift",
            "--input",
            "result.json",
        ],
    );
    assert!(
        updated.status.success(),
        "{}",
        String::from_utf8_lossy(&updated.stderr)
    );
    let payload: serde_json::Value = serde_json::from_slice(&updated.stdout).unwrap();
    assert_eq!(payload["accepted"], true);
    assert_eq!(payload["status"]["readiness"], "complete");
}

/// Red test: a compiled Guidance artifact without `manifest_hash` must still be
/// consumable by the public `guide status` command.
#[test]
fn guide_status_does_not_require_compiled_manifest_hash() {
    let scratch = Scratch::new();
    let root = scratch.root();
    let root_arg = root.to_str().unwrap();

    let new = scratch.cmd().args(["new", root_arg]).output().unwrap();
    assert!(
        new.status.success(),
        "new failed: {}{}",
        String::from_utf8_lossy(&new.stdout),
        String::from_utf8_lossy(&new.stderr)
    );
    let compile = scratch
        .cmd()
        .args(["guide", "compile", root_arg])
        .output()
        .unwrap();
    assert!(
        compile.status.success(),
        "guide compile failed: {}{}",
        String::from_utf8_lossy(&compile.stdout),
        String::from_utf8_lossy(&compile.stderr)
    );

    let compiled_path = root.join(".appsdk/guidance/compiled.json");
    let compiled: serde_json::Value =
        serde_json::from_slice(&fs::read(&compiled_path).unwrap()).unwrap();
    assert!(compiled.get("manifest_hash").is_none());
    assert!(compiled.get("project_contract_digest").is_none());
    for source in compiled["sources"].as_array().unwrap() {
        assert!(source.get("digest").is_none());
        assert!(source.get("contract_digest").is_none());
    }

    let status = scratch
        .cmd()
        .args(["guide", "status", root_arg])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&status.stdout).to_string();
    let stderr = String::from_utf8_lossy(&status.stderr).to_string();

    assert!(
        status.status.success(),
        "guide status must succeed when a valid compiled artifact lacks manifest_hash\nexit={:?}\nstdout={}\nstderr={}",
        status.status.code(),
        stdout,
        stderr
    );
    serde_json::from_str::<serde_json::Value>(&stdout).unwrap_or_else(|e| {
        panic!("status stdout is not JSON: {e}\nstdout={stdout}\nstderr={stderr}")
    });
}
