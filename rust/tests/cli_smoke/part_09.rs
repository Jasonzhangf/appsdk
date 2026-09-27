#[test]
fn pin_lock_migrates_stale_project_record_contracts() {
    let root = temp_root("record-contract-migration");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());
    let project_file = root.join(".appsdk/project.json");
    let worktree = root.join("contracts/records/worktree-record.schema.json");
    let promotion = root.join("contracts/records/promotion-record.schema.json");
    let live_closure = root.join("contracts/records/collab-live-closure-record.schema.json");
    fs::remove_file(&live_closure).unwrap();
    let mut current_worktree: Value =
        serde_json::from_str(&fs::read_to_string(&worktree).unwrap()).unwrap();
    current_worktree["properties"]
        .as_object_mut()
        .unwrap()
        .remove("bug_triage");
    fs::write(
        &worktree,
        serde_json::to_vec_pretty(&current_worktree).unwrap(),
    )
    .unwrap();
    let mut current_promotion: Value =
        serde_json::from_str(&fs::read_to_string(&promotion).unwrap()).unwrap();
    current_promotion["properties"]
        .as_object_mut()
        .unwrap()
        .remove("bug_closure_verified");
    fs::write(
        &promotion,
        serde_json::to_vec_pretty(&current_promotion).unwrap(),
    )
    .unwrap();
    let rejected = run(&["verify", root_text]);
    assert!(
        !rejected.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&rejected.stdout),
        String::from_utf8_lossy(&rejected.stderr)
    );
    assert!(String::from_utf8_lossy(&rejected.stderr).contains("DECLARED_RECORD_CONTRACT_MISSING"));
    assert!(run(&[
        "pin-lock",
        root_text,
        "--binary",
        binary().to_str().unwrap()
    ])
    .status
    .success());
    assert_eq!(
        serde_json::from_str::<Value>(&fs::read_to_string(&worktree).unwrap()).unwrap(),
        serde_json::from_str::<Value>(include_str!(
            "../../../contracts/records/worktree-record.schema.json"
        ))
        .unwrap()
    );
    assert_eq!(
        serde_json::from_str::<Value>(&fs::read_to_string(&promotion).unwrap()).unwrap(),
        serde_json::from_str::<Value>(include_str!(
            "../../../contracts/records/promotion-record.schema.json"
        ))
        .unwrap()
    );
    assert_eq!(
        serde_json::from_str::<Value>(&fs::read_to_string(&live_closure).unwrap()).unwrap(),
        serde_json::from_str::<Value>(include_str!(
            "../../../contracts/records/collab-live-closure-record.schema.json"
        ))
        .unwrap()
    );
    let migrated_project: Value =
        serde_json::from_str(&fs::read_to_string(&project_file).unwrap()).unwrap();
    assert_eq!(
        migrated_project["governance"]["record_contracts"],
        serde_json::json!([
            "contracts/records/worktree-record.schema.json",
            "contracts/records/reproduction-record.schema.json",
            "contracts/records/evidence-record.schema.json",
            "contracts/records/fix-candidate-record.schema.json",
            "contracts/records/goal-clarification-record.schema.json",
            "contracts/records/review-record.schema.json",
            "contracts/records/effectiveness-record.schema.json",
            "contracts/records/pre-review-validation-record.schema.json",
            "contracts/records/collaboration-record.schema.json",
            "contracts/records/collaboration-index.schema.json",
            "contracts/records/merge-queue-record.schema.json",
            "contracts/records/merge-queue-state.schema.json",
            "contracts/records/integration-record.schema.json",
            "contracts/records/mainline-receipt-record.schema.json",
            "contracts/records/collab-live-closure-record.schema.json",
            "contracts/records/merge-record.schema.json",
            "contracts/records/promotion-record.schema.json",
            "contracts/records/regression-report.schema.json",
            "contracts/records/freeze-record.schema.json",
            "contracts/records/record-graph.contract.json"
        ])
    );
    assert!(run(&["verify", root_text]).status.success());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn pin_lock_restores_missing_record_contract_directory() {
    let root = temp_root("record-contract-directory-migration");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());
    let records = root.join("contracts/records");
    fs::remove_dir_all(&records).unwrap();

    let result = run(&[
        "pin-lock",
        root_text,
        "--binary",
        binary().to_str().unwrap(),
    ]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(records.is_dir());
    assert!(run(&["verify", root_text]).status.success());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn pin_lock_reconciles_matching_authoring_bundle_mirror() {
    let root = temp_root("pin-lock-authoring-bundle-mirror");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());
    let project_file = root.join(".appsdk/project.json");
    let mut project: Value =
        serde_json::from_str(&fs::read_to_string(&project_file).unwrap()).unwrap();
    project["sdk"]["version"] = Value::String("0.1.5".into());
    fs::write(
        &project_file,
        serde_json::to_string_pretty(&project).unwrap() + "\n",
    )
    .unwrap();
    install_legacy_governance_maps(&root);

    let previous_manifest = serde_json::json!({
        "schema_version": 1,
        "sdk": "appsdk",
        "version": "0.1.5",
        "runtime_entrypoint": "rust-binary"
    });
    fs::create_dir_all(root.join("contracts")).unwrap();
    let previous_bytes = serde_json::to_vec_pretty(&previous_manifest).unwrap();
    fs::write(
        root.join("contracts/sdk-bundle.manifest.json"),
        &previous_bytes,
    )
    .unwrap();
    fs::write(
        root.join(".appsdk/contracts/sdk-bundle.manifest.json"),
        &previous_bytes,
    )
    .unwrap();

    let migrated = run(&[
        "pin-lock",
        root_text,
        "--binary",
        binary().to_str().unwrap(),
    ]);
    assert!(
        migrated.status.success(),
        "{}",
        String::from_utf8_lossy(&migrated.stderr)
    );
    assert_eq!(
        fs::read_to_string(root.join("contracts/sdk-bundle.manifest.json")).unwrap(),
        include_str!("../../../contracts/sdk-bundle.manifest.json")
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn pin_lock_rejects_drifted_authoring_bundle_mirror_before_migration() {
    let root = temp_root("pin-lock-authoring-bundle-drift");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());
    let project_file = root.join(".appsdk/project.json");
    let mut project: Value =
        serde_json::from_str(&fs::read_to_string(&project_file).unwrap()).unwrap();
    project["sdk"]["version"] = Value::String("0.1.5".into());
    fs::write(
        &project_file,
        serde_json::to_string_pretty(&project).unwrap() + "\n",
    )
    .unwrap();
    install_legacy_governance_maps(&root);

    fs::create_dir_all(root.join("contracts")).unwrap();
    fs::write(
        root.join("contracts/sdk-bundle.manifest.json"),
        br#"{"schema_version":1,"sdk":"appsdk","version":"0.1.5","runtime_entrypoint":"rust-binary"}
"#,
    )
    .unwrap();
    fs::write(
        root.join(".appsdk/contracts/sdk-bundle.manifest.json"),
        br#"{"schema_version":1,"sdk":"appsdk","version":"0.1.5","runtime_entrypoint":"other"}
"#,
    )
    .unwrap();
    let original_project = fs::read_to_string(&project_file).unwrap();
    let rejected = run(&[
        "pin-lock",
        root_text,
        "--binary",
        binary().to_str().unwrap(),
    ]);
    assert!(!rejected.status.success());
    assert!(String::from_utf8_lossy(&rejected.stderr).contains("SDK_AUTHORING_BUNDLE_MIRROR_DRIFT"));
    assert_eq!(fs::read_to_string(&project_file).unwrap(), original_project);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn migrated_project_verifies_without_local_sdk_witness_or_binary_digest_match() {
    let root = temp_root("global-sdk-no-local-witness");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());
    fs::write(
        root.join(".appsdk/goal.json"),
        r#"{"goal_id":"goal-1","raw_request":"change","understood_objective":"change","acceptance_criteria":["pass"],"non_goals":[],"assumptions":[],"ambiguities":[],"questions":[],"status":"confirmed","confirmed_by":"test","confirmed_at":"2026-01-01T00:00:00Z","created_at":"2026-01-01T00:00:00Z"}
"#,
    )
    .unwrap();
    pin_test_lock(root_text);
    let source_promote = run_in(&root, &["promote", "--to", "source_implemented"]);
    assert!(
        source_promote.status.success(),
        "{}",
        String::from_utf8_lossy(&source_promote.stderr)
    );
    assert!(run_in(&root, &["promote", "--to", "contract_bound"])
        .status
        .success());
    let compile = run_in(&root, &["compile"]);
    assert!(
        compile.status.success(),
        "{}",
        String::from_utf8_lossy(&compile.stderr)
    );
    assert!(run(&["promote", root_text, "--to", "compiled"])
        .status
        .success());
    fs::remove_file(root.join(".appsdk/sdk.bin")).unwrap();

    let verified = run(&["verify", root_text]);
    assert!(
        verified.status.success(),
        "{}",
        String::from_utf8_lossy(&verified.stderr)
    );

    let lock_path = root.join(".appsdk/sdk.lock");
    let mut lock: Value = serde_json::from_str(&fs::read_to_string(&lock_path).unwrap()).unwrap();
    lock["digest"] = Value::String(format!("sha256:{}", "0".repeat(64)));
    lock["compiler_digest"] = lock["digest"].clone();
    fs::write(
        &lock_path,
        serde_json::to_string_pretty(&lock).unwrap() + "\n",
    )
    .unwrap();
    let verified_after_digest_change = run(&["verify", root_text]);
    assert!(
        verified_after_digest_change.status.success(),
        "{}",
        String::from_utf8_lossy(&verified_after_digest_change.stderr)
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn pinned_sdk_witness_is_executable_and_resolvable_in_a_fresh_worktree() {
    let main = temp_root("sdk-witness-main");
    let worktree = temp_root("sdk-witness-linked");
    let registry = temp_root("sdk-witness-registry");
    let main_text = main.to_str().unwrap();
    let worktree_text = worktree.to_str().unwrap();
    let registry_text = registry.to_str().unwrap();

    assert!(run(&["new", main_text]).status.success());
    init_git(&main);
    // pin-lock is the only writer of the ignored witness and is the historical
    // source of the `binary_ref: project-sdk` lock a consumer gate inspects.
    let pinned = run(&[
        "pin-lock",
        main_text,
        "--binary",
        binary().to_str().unwrap(),
    ]);
    assert!(
        pinned.status.success(),
        "{}",
        String::from_utf8_lossy(&pinned.stderr)
    );
    let witness = main.join(".appsdk/sdk.bin");
    let mode = fs::metadata(&witness).unwrap().permissions().mode();
    assert!(
        mode & 0o111 != 0,
        "pin-lock witness must stay executable for consumer gates, mode={mode:o}"
    );
    assert!(fs::read_to_string(main.join(".gitignore"))
        .unwrap()
        .contains(".appsdk/sdk.bin"));
    // The pinned lock is tracked project truth; only the ignored witness is
    // per-checkout, so commit the lock before branching a worktree from it.
    assert!(Command::new("git")
        .args(["-C", main_text, "add", ".appsdk/sdk.lock", ".gitignore"])
        .status()
        .unwrap()
        .success());
    assert!(Command::new("git")
        .args(["-C", main_text, "commit", "-m", "pin"])
        .status()
        .unwrap()
        .success());

    let added = Command::new("git")
        .args([
            "-C",
            main_text,
            "worktree",
            "add",
            "--detach",
            worktree_text,
            "HEAD",
        ])
        .status()
        .unwrap();
    assert!(added.success());
    // A fresh worktree inherits the tracked lock but never the ignored witness.
    assert!(!worktree.join(".appsdk/sdk.bin").exists());

    let resolved = Command::new(binary())
        .args(["sdk-witness", worktree_text])
        .env("APPSDK_HOME", registry_text)
        .env_remove("TMUX_PANE")
        .output()
        .unwrap();
    assert!(
        resolved.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&resolved.stdout),
        String::from_utf8_lossy(&resolved.stderr)
    );
    let restored = worktree.join(".appsdk/sdk.bin");
    assert_eq!(fs::read(&restored).unwrap(), fs::read(&witness).unwrap());
    let restored_mode = fs::metadata(&restored).unwrap().permissions().mode();
    assert!(
        restored_mode & 0o111 != 0,
        "resolved witness must be executable, mode={restored_mode:o}"
    );
    let lock: Value =
        serde_json::from_str(&fs::read_to_string(worktree.join(".appsdk/sdk.lock")).unwrap())
            .unwrap();
    assert_eq!(lock["binary_ref"], Value::String("project-sdk".into()));
    assert_eq!(
        file_digest(&restored),
        lock["digest"].as_str().unwrap(),
        "witness must stay byte-identical to the locked digest"
    );

    let _ = Command::new("git")
        .args([
            "-C",
            main_text,
            "worktree",
            "remove",
            "--force",
            worktree_text,
        ])
        .status();
    fs::remove_dir_all(main).unwrap();
    let _ = fs::remove_dir_all(registry);
}

#[test]
fn pinned_sdk_witness_resolution_fails_closed_on_a_mismatched_binary() {
    let main = temp_root("sdk-witness-mismatch-main");
    let worktree = temp_root("sdk-witness-mismatch-linked");
    let registry = temp_root("sdk-witness-mismatch-registry");
    let main_text = main.to_str().unwrap();
    let worktree_text = worktree.to_str().unwrap();
    let registry_text = registry.to_str().unwrap();

    assert!(run(&["new", main_text]).status.success());
    init_git(&main);
    assert!(run(&[
        "pin-lock",
        main_text,
        "--binary",
        binary().to_str().unwrap(),
    ])
    .status
    .success());
    assert!(Command::new("git")
        .args(["-C", main_text, "add", ".appsdk/sdk.lock"])
        .status()
        .unwrap()
        .success());
    assert!(Command::new("git")
        .args(["-C", main_text, "commit", "-m", "pin"])
        .status()
        .unwrap()
        .success());
    let added = Command::new("git")
        .args([
            "-C",
            main_text,
            "worktree",
            "add",
            "--detach",
            worktree_text,
            "HEAD",
        ])
        .status()
        .unwrap();
    assert!(added.success());

    let wrong = worktree.join("wrong-appsdk");
    fs::write(&wrong, "not the pinned AppSDK binary\n").unwrap();
    let rejected = Command::new(binary())
        .args([
            "sdk-witness",
            worktree_text,
            "--binary",
            wrong.to_str().unwrap(),
        ])
        .env("APPSDK_HOME", registry_text)
        .env_remove("TMUX_PANE")
        .output()
        .unwrap();
    assert!(!rejected.status.success());
    assert!(
        String::from_utf8_lossy(&rejected.stderr).contains("SDK_WITNESS_BINARY_MISMATCH"),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&rejected.stdout),
        String::from_utf8_lossy(&rejected.stderr)
    );
    assert!(
        !worktree.join(".appsdk/sdk.bin").exists(),
        "a mismatched binary must not materialize an unverifiable witness"
    );

    fs::remove_file(&wrong).unwrap();
    let _ = Command::new("git")
        .args([
            "-C",
            main_text,
            "worktree",
            "remove",
            "--force",
            worktree_text,
        ])
        .status();
    fs::remove_dir_all(main).unwrap();
    let _ = fs::remove_dir_all(registry);
}

#[test]
fn verify_admission_requires_generated_artifact_requirement() {
    let root = temp_root("verify-admission");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());
    fs::write(
        root.join(".appsdk/goal.json"),
        r#"{"goal_id":"goal-1","raw_request":"change","understood_objective":"change","acceptance_criteria":["pass"],"non_goals":[],"assumptions":[],"ambiguities":[],"questions":[],"status":"confirmed","confirmed_by":"test","confirmed_at":"2026-01-01T00:00:00Z","created_at":"2026-01-01T00:00:00Z"}
"#,
    )
    .unwrap();
    pin_test_lock(root_text);
    let source_promote = run(&["promote", root_text, "--to", "source_implemented"]);
    assert!(
        source_promote.status.success(),
        "{}",
        String::from_utf8_lossy(&source_promote.stderr)
    );
    assert!(run(&["promote", root_text, "--to", "contract_bound"])
        .status
        .success());
    let compile = run(&["compile", root_text]);
    assert!(compile.status.success());
    assert!(run(&["promote", root_text, "--to", "compiled"])
        .status
        .success());
    fs::remove_file(root.join("generated/project.compiled.json")).unwrap();

    let full = run(&["verify", root_text]);
    assert!(!full.status.success());
    assert!(String::from_utf8_lossy(&full.stderr).contains("COMPILED_STAGE_REQUIRES_ARTIFACT"));

    let admission = run(&["verify", "--admission", root_text]);
    assert!(!admission.status.success());
    assert!(
        String::from_utf8_lossy(&admission.stderr).contains("COMPILED_STAGE_REQUIRES_ARTIFACT"),
        "{}",
        String::from_utf8_lossy(&admission.stderr)
    );
    fs::remove_dir_all(root).unwrap();
}

fn canonical(value: &Value) -> String {
    match value {
        Value::Null => "null".into(),
        Value::Bool(value) => value.to_string(),
        Value::Number(value) => value.to_string(),
        Value::String(value) => serde_json::to_string(value).unwrap(),
        Value::Array(values) => format!(
            "[{}]",
            values.iter().map(canonical).collect::<Vec<_>>().join(",")
        ),
        Value::Object(values) => {
            let mut keys = values.keys().collect::<Vec<_>>();
            keys.sort();
            format!(
                "{{{}}}",
                keys.iter()
                    .map(|key| format!(
                        "{}:{}",
                        serde_json::to_string(key).unwrap(),
                        canonical(&values[*key])
                    ))
                    .collect::<Vec<_>>()
                    .join(",")
            )
        }
    }
}

fn digest(value: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(value.as_bytes());
    format!("sha256:{:x}", hasher.finalize())
}

fn set_sdk_resource_digest(root: &Path, source: &str, projection: &Path) {
    let record_path = root.join(".appsdk/sdk-resources.json");
    let mut record: Value = serde_json::from_slice(&fs::read(&record_path).unwrap()).unwrap();
    let entry = record["resources"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|entry| entry["source"] == source)
        .unwrap();
    entry["digest"] = Value::String(file_digest(projection));
    fs::write(&record_path, serde_json::to_vec_pretty(&record).unwrap()).unwrap();
}

fn restore_sdk_contract(
    source_path: &Path,
    projection_path: &Path,
    sdk_resources_path: &Path,
    source_before: &[u8],
    sdk_resources_before: &[u8],
) {
    fs::write(source_path, source_before).unwrap();
    fs::write(projection_path, source_before).unwrap();
    fs::write(sdk_resources_path, sdk_resources_before).unwrap();
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

fn file_digest(path: &Path) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(fs::read(path).unwrap());
    format!("sha256:{:x}", hasher.finalize())
}

fn stable_review_id(
    promotion_id: &str,
    fix_candidate_id: &str,
    reviewer: &Value,
    verdict: &str,
    evidence_ids: &[&str],
) -> String {
    let identity = serde_json::json!({
        "promotion_id": promotion_id,
        "fix_candidate_id": fix_candidate_id,
        "reviewer": reviewer,
        "verdict": verdict,
        "evidence_ids": evidence_ids
    });
    format!(
        "review-{}",
        digest(&canonical(&identity))
            .strip_prefix("sha256:")
            .unwrap()
    )
}

fn install_authoritative_bug_fixture(root: &Path, issue_id: &str) -> PathBuf {
    let fake_bin = root.with_extension("fake-git-bug");
    fs::create_dir_all(&fake_bin).unwrap();
    let fake_git_bug = fake_bin.join("git-bug");
    fs::write(
        &fake_git_bug,
        format!(
            "#!/bin/sh\ncase \"$1 $2\" in\n  \"bug show\")\n    printf '%s\\n' '{{\"id\":\"bug-object\",\"human_id\":\"{}\",\"status\":\"closed\",\"comments\":[{{\"id\":\"comment-1\",\"message\":\"### Solution / Resolution\\nfixed\"}}]}}'\n    exit 0\n    ;;\n  *) exit 64 ;;\nesac\n",
            issue_id
        ),
    )
    .unwrap();
    fs::set_permissions(&fake_git_bug, fs::Permissions::from_mode(0o755)).unwrap();
    let ops = root.join("bug-ops.json");
    fs::write(
        &ops,
        r#"{"ops":[{"type":4,"status":2,"timestamp":"2026-01-01T00:00:00Z"}]}"#,
    )
    .unwrap();
    let blob = git_test_value(root, &["hash-object", "-w", ops.to_str().unwrap()]);
    let tree_input = format!("100644 blob {}\tops\n", blob);
    let mut mktree = Command::new("git")
        .args(["-C", root.to_str().unwrap(), "mktree"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    use std::io::Write;
    mktree
        .stdin
        .as_mut()
        .unwrap()
        .write_all(tree_input.as_bytes())
        .unwrap();
    let tree = mktree.wait_with_output().unwrap();
    assert!(tree.status.success());
    let tree = String::from_utf8(tree.stdout).unwrap().trim().to_string();
    let commit = Command::new("git")
        .args([
            "-C",
            root.to_str().unwrap(),
            "commit-tree",
            &tree,
            "-m",
            "close bug",
        ])
        .output()
        .unwrap();
    assert!(commit.status.success());
    let commit = String::from_utf8(commit.stdout).unwrap().trim().to_string();
    assert!(Command::new("git")
        .args([
            "-C",
            root.to_str().unwrap(),
            "update-ref",
            "refs/bugs/bug-object",
            &commit
        ])
        .status()
        .unwrap()
        .success());
    fs::remove_file(ops).unwrap();
    fake_git_bug
}

fn write_records(
    root: &PathBuf,
    module_id: &str,
    artifact_hash: &str,
    include_freeze: bool,
    issue_id: &str,
) {
    install_authoritative_bug_fixture(root, issue_id);
    let records = root.join(".appsdk/records");
    fs::create_dir_all(&records).unwrap();
    let evidence_dir = records.join("evidence").join(module_id);
    fs::create_dir_all(&evidence_dir).unwrap();
    let commit = git_test_value(root, &["rev-parse", "HEAD"]);
    let tree = git_test_value(root, &["rev-parse", "HEAD^{tree}"]);
    let review_id = stable_review_id(
        "promotion-1",
        "candidate-1",
        &serde_json::json!({"adapter":"test","identity":"test"}),
        "pass",
        &["candidate-evidence-1", "positive-1", "negative-1"],
    );
    let map_root = root.join(".appsdk/maps");
    let evidence = |id: &str, phase: &str, kind: &str, created_at: &str| {
        serde_json::json!({
            "evidence_id": id,
            "issue_id": issue_id,
            "experiment_id": "experiment-1",
            "phase": phase,
            "kind": kind,
            "source_commit": commit,
            "artifact_hash": artifact_hash,
            "scope": {"module_id": module_id},
            "producer": {"adapter":"test","identity":"test"},
            "result":"pass",
            "created_at": created_at,
            "expires_at":"2099-01-01T00:00:00Z",
            "input_hashes":["input-1"],
            "scope_hash":"scope-1"
        })
    };
    for (id, phase, kind, created_at) in [
        (
            "baseline-1",
            "baseline_reproduction",
            "sample_replay",
            "2026-01-01T00:01:00Z",
        ),
        (
            "candidate-evidence-1",
            "fix_candidate",
            "build",
            "2026-01-01T00:03:00Z",
        ),
        (
            "positive-1",
            "positive_intervention",
            "positive_test",
            "2026-01-01T00:03:00Z",
        ),
        (
            "negative-1",
            "negative_intervention",
            "negative_test",
            "2026-01-01T00:03:00Z",
        ),
        (
            "effective-1",
            "post_architecture_effectiveness",
            "sample_replay",
            "2026-01-01T00:05:00Z",
        ),
        (
            "post-positive-1",
            "positive_intervention",
            "positive_test",
            "2026-01-01T00:05:00Z",
        ),
        (
            "post-negative-1",
            "negative_intervention",
            "negative_test",
            "2026-01-01T00:05:00Z",
        ),
    ] {
        fs::write(
            evidence_dir.join(format!("{id}.json")),
            serde_json::to_string_pretty(&evidence(id, phase, kind, created_at)).unwrap() + "\n",
        )
        .unwrap();
    }
    fs::write(
        evidence_dir.join("whitebox-1.json"),
        serde_json::to_string_pretty(&serde_json::json!({
            "evidence_id":"whitebox-1","issue_id":issue_id,"experiment_id":"experiment-1",
            "phase":"development_whitebox","kind":"gate","source_commit":commit,
            "artifact_hash":artifact_hash,"execution_surface":"development_whitebox",
            "scope":{"module_id":module_id},"producer":{"adapter":"test","identity":"whitebox-runner"},
            "result":"pass","created_at":"2026-01-01T00:03:10Z","expires_at":"2099-01-01T00:00:00Z",
            "input_hashes":["input-1"],"scope_hash":"scope-1"
        }))
        .unwrap()
            + "\n",
    )
    .unwrap();
    fs::write(
        evidence_dir.join("install-1.json"),
        serde_json::to_string_pretty(&serde_json::json!({
            "evidence_id":"install-1","issue_id":issue_id,"experiment_id":"experiment-1",
            "phase":"deployment_install","kind":"install","source_commit":commit,
            "artifact_hash":artifact_hash,"execution_surface":"deployed_blackbox",
            "environment_id":"test-deployment","entrypoint":"test://installed-app",
            "scope":{"module_id":module_id,"entrypoint":"test://installed-app"},
            "producer":{"adapter":"test","identity":"test-deployment-adapter"},"result":"pass",
            "created_at":"2026-01-01T00:03:15Z","expires_at":"2099-01-01T00:00:00Z",
            "input_hashes":["input-1"],"scope_hash":"scope-1"
        }))
        .unwrap()
            + "\n",
    )
    .unwrap();
    fs::write(
        evidence_dir.join("restart-1.json"),
        serde_json::to_string_pretty(&serde_json::json!({
            "evidence_id":"restart-1","issue_id":issue_id,"experiment_id":"experiment-1",
            "phase":"deployment_restart","kind":"restart","source_commit":commit,
            "artifact_hash":artifact_hash,"execution_surface":"deployed_blackbox",
            "environment_id":"test-deployment","entrypoint":"test://installed-app",
            "scope":{"module_id":module_id,"entrypoint":"test://installed-app"},
            "producer":{"adapter":"test","identity":"test-deployment-adapter"},"result":"pass",
            "created_at":"2026-01-01T00:03:20Z","expires_at":"2099-01-01T00:00:00Z",
            "input_hashes":["input-1"],"scope_hash":"scope-1"
        }))
        .unwrap()
            + "\n",
    )
    .unwrap();
    fs::write(
        evidence_dir.join("blackbox-1.json"),
        serde_json::to_string_pretty(&serde_json::json!({
            "evidence_id":"blackbox-1","issue_id":issue_id,"experiment_id":"experiment-1",
            "phase":"deployed_blackbox","kind":"runtime","source_commit":commit,
            "artifact_hash":artifact_hash,"execution_surface":"deployed_blackbox",
            "environment_id":"test-deployment","entrypoint":"test://installed-app",
            "scope":{"module_id":module_id,"entrypoint":"test://installed-app"},
            "producer":{"adapter":"test","identity":"test-deployment-adapter"},"result":"pass",
            "created_at":"2026-01-01T00:03:30Z","expires_at":"2099-01-01T00:00:00Z",
            "input_hashes":["input-1"],"scope_hash":"scope-1"
        }))
        .unwrap()
            + "\n",
    )
    .unwrap();
    fs::write(
        records.join(format!("evidence-record-{module_id}.json")),
        serde_json::to_string_pretty(&evidence(
            "candidate-evidence-1",
            "fix_candidate",
            "build",
            "2026-01-01T00:03:00Z",
        ))
        .unwrap()
            + "\n",
    )
    .unwrap();
    let mut worktree_record = serde_json::json!({
        "worktree_id":"worktree-1","issue_id":issue_id,"module_id":module_id,
        "base_ref":"HEAD","base_commit":commit,"branch":"test-fix","head_commit":commit,
        "initial_clean":true,"final_clean":true,"isolation_mode":"isolated_worktree",
        "scope_hash":"scope-1","created_at":"2026-01-01T00:00:00Z",
        "bug_triage":{"query_executed":true,"query":format!("appsdk bug list -q {}", issue_id),"mode":"new_confirmed","reopened_from_issue_id":null}
    });
    worktree_record["bug_triage_query_binding"] =
        Value::String(digest(&canonical(&serde_json::json!({
            "issue_id": issue_id,
            "query": format!("appsdk bug list -q {}", issue_id),
            "mode": "new_confirmed",
            "reopened_from_issue_id": null
        }))));
    fs::write(
        records.join(format!("worktree-record-{module_id}.json")),
        serde_json::to_string_pretty(&worktree_record).unwrap() + "\n",
    )
    .unwrap();
    fs::write(
        records.join(format!("reproduction-record-{module_id}.json")),
        serde_json::to_string_pretty(&serde_json::json!({
            "reproduction_id":"reproduction-1","issue_id":issue_id,"module_id":module_id,
            "worktree_id":"worktree-1","base_commit":commit,"input_hashes":["input-1"],
            "baseline_evidence_id":"baseline-1","first_divergence":"test-owner",
            "result":"reproduced","created_at":"2026-01-01T00:01:00Z"
        }))
        .unwrap()
            + "\n",
    )
    .unwrap();
    fs::write(
        records.join(format!("fix-candidate-record-{module_id}.json")),
        serde_json::to_string_pretty(&serde_json::json!({
            "fix_candidate_id":"candidate-1","issue_id":issue_id,"module_id":module_id,
            "worktree_id":"worktree-1","base_commit":commit,"head_commit":commit,
            "tree_hash":tree,"diff_hash":"sha256:test-diff","design_id":"design-1",
            "owner":"app-core","scope_hash":"scope-1","changed_paths":[],
            "verification_evidence_ids":["candidate-evidence-1","positive-1","negative-1"],
            "created_at":"2026-01-01T00:03:00Z"
        }))
        .unwrap()
            + "\n",
    )
    .unwrap();
    fs::write(
        records.join(format!("pre-review-validation-record-{module_id}.json")),
        serde_json::to_string_pretty(&serde_json::json!({
            "validation_id":"pre-review-validation-1","issue_id":issue_id,"module_id":module_id,
            "fix_candidate_id":"candidate-1","candidate_commit":commit,"candidate_tree_hash":tree,
            "artifact_hash":artifact_hash,"whitebox_producer":{"adapter":"test","identity":"whitebox-runner"},
            "whitebox_evidence_ids":["whitebox-1"],
            "blackbox_evidence_ids":["blackbox-1"],"deployment":{"environment_id":"test-deployment",
            "install_receipt_id":"install-1","restart_receipt_id":"restart-1",
            "entrypoint":"test://installed-app","producer":{"adapter":"test","identity":"test-deployment-adapter"},
            "observed_at":"2026-01-01T00:03:30Z"},"source_unchanged":true,
            "result":"pass","created_at":"2026-01-01T00:03:45Z"
        }))
        .unwrap()
            + "\n",
    )
    .unwrap();
    fs::write(
        records.join(format!("review-record-{module_id}.json")),
        serde_json::to_string_pretty(&serde_json::json!({
            "review_id":review_id,"issue_id":issue_id,"promotion_id":"promotion-1",
            "review_kind":"architecture","fix_candidate_id":"candidate-1",
            "pre_review_validation_id":"pre-review-validation-1",
            "reviewer":{"adapter":"test","identity":"test"},"verdict":"pass",
            "evidence_ids":["candidate-evidence-1","positive-1","negative-1"],"reviewed_commit":commit,
            "reviewed_tree_hash":tree,"reviewed_diff_hash":"sha256:test-diff",
            "reviewed_artifact_hash":artifact_hash,"reviewed_scope_hash":"scope-1",
            "resource_map_hash":file_digest(&map_root.join("resource-map.json")),
            "function_map_hash":file_digest(&map_root.join("function-map.json")),
            "mainline_call_map_hash":file_digest(&map_root.join("mainline-call-map.json")),
            "verification_map_hash":file_digest(&map_root.join("verification-map.json")),
            "created_at":"2026-01-01T00:04:00Z"
        }))
        .unwrap()
            + "\n",
    )
    .unwrap();
    fs::write(
        records.join(format!("effectiveness-record-{module_id}.json")),
        serde_json::to_string_pretty(&serde_json::json!({
            "effectiveness_id":"effectiveness-1","issue_id":issue_id,"module_id":module_id,
            "fix_candidate_id":"candidate-1","architecture_review_id":review_id,
            "reviewed_commit":commit,"reviewed_tree_hash":tree,
            "reproduction_input_hashes":["input-1"],"baseline_evidence_id":"baseline-1",
            "fixed_replay_evidence_id":"effective-1","positive_evidence_ids":["post-positive-1"],
            "negative_evidence_ids":["post-negative-1"],"blackbox_evidence_ids":["effective-1"],
            "source_unchanged_since_review":true,"result":"pass","created_at":"2026-01-01T00:05:00Z"
        }))
        .unwrap()
            + "\n",
    )
    .unwrap();
    fs::write(
        records.join(format!("merge-record-{module_id}.json")),
        serde_json::to_string_pretty(&serde_json::json!({
            "merge_id":"merge-1","issue_id":issue_id,"module_id":module_id,
            "fix_candidate_id":"candidate-1","effectiveness_id":"effectiveness-1",
            "mainline_ref":"HEAD","candidate_commit":commit,"merge_commit":commit,
            "candidate_tree_hash":tree,"merged_tree_hash":tree,"change_identity":"exact",
            "result":"pass","created_at":"2026-01-01T00:06:00Z"
        }))
        .unwrap()
            + "\n",
    )
    .unwrap();
    let promotion_value = serde_json::json!({
        "promotion_id":"promotion-1","issue_id":issue_id,"experiment_id":"experiment-1",
        "module_id":module_id,"base_commit":commit,"source_commit":commit,
        "candidate_commit":commit,"merged_commit":commit,
        "worktree_record_id":"worktree-1","reproduction_record_id":"reproduction-1",
        "fix_candidate_id":"candidate-1","architecture_review_id":review_id,
        "effectiveness_record_id":"effectiveness-1","merge_record_id":"merge-1",
        "previous_active_version":null,"new_active_version":"active-v1",
        "artifact_hash":artifact_hash,"scope_hash":"scope-1","public_api_hash":"api-1",
        "review_id":review_id,"evidence_ids":["candidate-evidence-1","effective-1"],
        "required_gate_results":[{"gate_id":"fix_lifecycle_graph","result":"pass","producer":"test"}],
        "change_set_id":"change-1","compatibility_level":"compatible","root_cause":"test root cause",
        "design_id":"design-1","change_reason_comment":"test reason",
        "playground_cleanup_record_id":"cleanup-1","bug_closure_verified":true,"created_at":"2026-01-01T00:07:00Z"
    });
    let promotion = serde_json::to_string_pretty(&promotion_value).unwrap() + "\n";
    fs::write(
        records.join(format!("promotion-record-{module_id}.json")),
        &promotion,
    )
    .unwrap();
    fs::write(
        records.join(format!("playground-cleanup-cleanup-1.json")),
        r#"{"cleanup_id":"cleanup-1","disposition":"archive_then_remove","removed_paths":["playground/experiments/app-core"],"created_at":"2026-01-01T00:00:00Z"}"#,
    )
    .unwrap();
    if include_freeze {
        let promotion_hash = digest(&canonical(&promotion_value));
        fs::write(
            records.join(format!("freeze-record-{module_id}.json")),
            format!(
                r#"{{"freeze_id":"freeze-1","issue_id":"{}","module_id":"{}","promotion_id":"promotion-1","promotion_record_hash":"{}","artifact_record_id":"candidate-evidence-1","source_commit_or_tag":"{}","active_version":"active-v1","previous_active_version":null,"library_hash":"{}","public_api_hash":"api-1","review_id":"{}","previous_active_immutable":false,"git_clean":true,"clean_scope":{{"base_commit":"{}","changed_paths":[],"ignored_paths":[],"generated_policy":"tracked_hash"}},"owners":{{"vcs":"test","compiler":"test","api_extractor":"test","review":"test","artifact_registry":"test"}},"created_at":"2026-01-01T00:08:00Z"}}"#,
                issue_id, module_id, promotion_hash, commit, artifact_hash, review_id, commit
            ),
        )
        .unwrap();
    }
}

fn enable_parallel_development(root: &Path) {
    let project_file = root.join(".appsdk/project.json");
    let mut project: Value =
        serde_json::from_str(&fs::read_to_string(&project_file).unwrap()).unwrap();
    project["development_scenarios"] = serde_json::json!({
        "manifest": ".appsdk/contracts/development-scenarios.manifest.json",
        "enabled": ["multi_worker_collaboration", "multi_worktree_merge_queue"]
    });
    fs::write(
        &project_file,
        serde_json::to_string_pretty(&project).unwrap() + "\n",
    )
    .unwrap();
}

fn install_fixture_collab_cli(root: &Path, artifact_hash: &str, source_commit: &str) {
    let bin = root.join("fixture-bin");
    fs::create_dir_all(&bin).unwrap();
    let collab = bin.join("collab");
    let script = r#"#!/bin/sh
source_commit="__SOURCE_COMMIT__"
artifact_hash="__ARTIFACT_HASH__"
case "$1 $2" in
  "context ")
    if [ "$TMUX" != "/tmp/collab-live-closure.sock,12345,0" ] || [ "$TMUX_PANE" != "%1" ] || [ "$CODEX_SESSION_ID" != "session-fixture-thread" ] || [ "$CODEX_THREAD_ID" != "fixture-thread" ]; then
      printf '%s\n' 'fixture collab stub: missing or incorrect tmux endpoint environment' >&2
      exit 65
    fi
    printf '%s\n' '{"registered":true,"project_root":"fixture-project","liveness":{"live":true,"presence":"present","transport_kind":"tmux"},"identity":{"worker_id":"fixture-worker","kind":"peer","transport":{"kind":"tmux","tmux_endpoint":{"socket_path":"/tmp/collab-live-closure.sock","server_pid":12345,"tmux_session_id":"$1","pane_id":"%1","pane_pid":23456,"codex_session_id":"session-fixture-thread","codex_thread_id":"fixture-thread"}}}}'
    ;;
  "route resolve")
    if [ "$#" -ne 6 ] || [ "$3" != "--tmux-session-id" ] || [ "$4" != '$1' ] || [ "$5" != "--pane-id" ] || [ "$6" != "%1" ] || [ "$TMUX" != "/tmp/collab-live-closure.sock,12345,0" ] || [ "$TMUX_PANE" != "%1" ]; then
      printf '%s\n' 'fixture collab stub: route resolve requires exact tmux session and pane arguments' >&2
      exit 65
    fi
    printf '%s\n' '{"app_scope_id":"fixture-app","project_scope":"fixture-project","canonical_root":"fixture-project","storage_root":"fixture-storage","agent_id":"fixture-worker","binding_id":"fixture-binding","endpoint_generation":1,"native_thread_id":"fixture-thread","tmux_endpoint":{"socket_path":"/tmp/collab-live-closure.sock","server_pid":12345,"tmux_session_id":"$1","pane_id":"%1","pane_pid":23456,"codex_session_id":"session-fixture-thread","codex_thread_id":"fixture-thread"}}'
    ;;
  "msg collab-"*)
    case "$2" in
      "collab-peer-to-peer") sender="peer"; receiver="peer"; path="peer_to_peer" ;;
      "collab-peer-to-master") sender="peer"; receiver="master"; path="peer_to_master" ;;
      "collab-master-to-peer") sender="master"; receiver="peer"; path="master_to_peer" ;;
      "collab-master-to-master") sender="master"; receiver="master"; path="master_to_master" ;;
      "collab-daemon-to-peer") sender="daemon"; receiver="peer"; path="daemon_to_peer" ;;
      "collab-daemon-to-master") sender="daemon"; receiver="master"; path="daemon_to_master" ;;
      "collab-restart-replay") sender="daemon"; receiver="peer"; path="restart_replay" ;;
      *)
        printf '%s\n' 'fixture collab stub: unknown message' >&2
        exit 66
        ;;
    esac
    printf '{"id":"%s","from":"%s","to":"%s","subject":"appsdk-collab-live:fixture-not-live:%s:%s:%s:fixture-environment:1","state":"read","answered":false}\n' "$2" "$sender" "$receiver" "$path" "$source_commit" "$artifact_hash"
    ;;
  *)
    printf '%s\n' 'fixture collab stub: unsupported command' >&2
    exit 64
    ;;
esac
"#
    .replace("__SOURCE_COMMIT__", source_commit)
    .replace("__ARTIFACT_HASH__", artifact_hash);
    fs::write(&collab, script).unwrap();
    fs::set_permissions(&collab, fs::Permissions::from_mode(0o755)).unwrap();
}

fn write_collab_live_closure_fixture(
    root: &Path,
    module_id: &str,
    artifact_hash: &str,
    source_commit: &str,
) -> Value {
    let records = root.join(".appsdk/records");
    let evidence_dir = records.join("evidence").join(module_id);
    let mut evidence_ids = serde_json::Map::new();
    let mut path_receipts = serde_json::Map::new();
    for path in [
        "peer_to_peer",
        "peer_to_master",
        "master_to_peer",
        "master_to_master",
        "daemon_to_peer",
        "daemon_to_master",
        "restart_replay",
    ] {
        let evidence_id = format!("collab-{}", path.replace('_', "-"));
        let phase = if path == "restart_replay" {
            "deployment_restart"
        } else {
            "deployed_blackbox"
        };
        let kind = if path == "restart_replay" {
            "restart"
        } else {
            "sample_replay"
        };
        let (sender, receiver) = match path {
            "peer_to_peer" => ("peer", "peer"),
            "peer_to_master" => ("peer", "master"),
            "master_to_peer" => ("master", "peer"),
            "master_to_master" => ("master", "master"),
            "daemon_to_peer" => ("daemon", "peer"),
            "daemon_to_master" => ("daemon", "master"),
            "restart_replay" => ("daemon", "peer"),
            _ => unreachable!(),
        };
        let challenge = format!(
            "appsdk-collab-live:fixture-not-live:{path}:{source_commit}:{artifact_hash}:fixture-environment:1"
        );
        fs::write(
            evidence_dir.join(format!("{evidence_id}.json")),
            serde_json::to_string_pretty(&serde_json::json!({
                "evidence_id": evidence_id,
                "issue_id": "issue-1",
                "experiment_id": "experiment-1",
                "phase": phase,
                "kind": kind,
                "source_commit": source_commit,
                "artifact_hash": artifact_hash,
                "execution_surface": "deployed_blackbox",
                "environment_id": "fixture-environment",
                "entrypoint": "fixture://collab-live-closure",
                "scope": {"module_id": module_id},
                "producer": {"adapter": "fixture", "identity": "fixture-not-live"},
                "result": "pass",
                "created_at": "2026-01-01T00:06:40Z",
                "expires_at": "2099-01-01T00:00:00Z",
                "input_hashes": ["input-1"],
                "scope_hash": "scope-1"
            }))
            .unwrap()
                + "\n",
        )
        .unwrap();
        evidence_ids.insert(path.into(), Value::String(evidence_id.clone()));
        path_receipts.insert(
            path.into(),
            serde_json::json!({
                "evidence_id": evidence_id,
                "message_id": format!("collab-{}", path.replace('_', "-")),
                "sender": sender,
                "receiver": receiver,
                "source_commit": source_commit,
                "artifact_hash": artifact_hash,
                "environment_id": "fixture-environment",
                "entrypoint": "fixture://collab-live-closure",
                "endpoint_generation": 1,
                "challenge": challenge,
                "observed_at": "2026-01-01T00:06:40Z"
            }),
        );
    }
    let closure = serde_json::json!({
        "closure_id": "fixture-not-live",
        "issue_id": "issue-1",
        "module_id": module_id,
        "fix_candidate_id": "candidate-1",
        "artifact_hash": artifact_hash,
        "scope_hash": "scope-1",
        "source_commit": source_commit,
        "environment_id": "fixture-environment",
        "entrypoint": "fixture://collab-live-closure",
        "collab_identity": {
            "worker_id": "fixture-worker",
            "binding_id": "fixture-binding",
            "thread_id": "fixture-thread",
            "app_scope_id": "fixture-app",
            "project_scope_id": "fixture-project"
        },
        "route_receipt": {
            "daemon_live": true,
            "endpoint_generation": 1,
            "route_scope": {
                "project_scope": "fixture-project",
                "app_scope_id": "fixture-app"
            },
            "storage_root": "fixture-storage",
            "tmux_endpoint": {
                "socket_path": "/tmp/collab-live-closure.sock",
                "server_pid": 12345,
                "tmux_session_id": "$1",
                "pane_id": "%1",
                "pane_pid": 23456,
                "codex_session_id": "session-fixture-thread",
                "codex_thread_id": "fixture-thread"
            },
            "resolved_at": "2026-01-01T00:06:40Z",
            "source": "collab_cli"
        },
        "evidence_ids": Value::Object(evidence_ids),
        "path_receipts": Value::Object(path_receipts),
        "created_at": "2026-01-01T00:06:40Z"
    });
    fs::write(
        records.join("collab-live-closure-fixture-not-live.json"),
        serde_json::to_string_pretty(&closure).unwrap() + "\n",
    )
    .unwrap();
    closure
}

fn prepare_retire_fixture(name: &str, candidate_issue: &str, validation_issue: &str) -> PathBuf {
    let root = temp_root(name);
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());
    init_git(&root);
    let candidate_commit = git_test_value(&root, &["rev-parse", "HEAD"]);
    let candidate_tree = git_test_value(&root, &["rev-parse", "HEAD^{tree}"]);
    let records = root.join(".appsdk/records");
    fs::write(
        records.join("fix-candidate-record-app-core.json"),
        serde_json::to_string_pretty(&serde_json::json!({
            "fix_candidate_id":"candidate-1",
            "issue_id":candidate_issue,
            "module_id":"app-core",
            "worktree_id":"worktree-1",
            "base_commit":candidate_commit,
            "head_commit":candidate_commit,
            "tree_hash":candidate_tree,
            "diff_hash":"sha256:diff",
            "design_id":"design-1",
            "owner":"test",
            "scope_hash":"sha256:scope",
            "changed_paths":[],
            "verification_evidence_ids":["evidence-1","evidence-2","evidence-3"],
            "created_at":"2026-01-01T00:00:00Z"
        }))
        .unwrap()
            + "\n",
    )
    .unwrap();
    fs::write(
        records.join("pre-review-validation-record-app-core.json"),
        serde_json::to_string_pretty(&serde_json::json!({
            "validation_id":"validation-1",
            "issue_id":validation_issue,
            "module_id":"app-core",
            "fix_candidate_id":"candidate-1",
            "candidate_commit":candidate_commit,
            "candidate_tree_hash":candidate_tree,
            "artifact_hash":"sha256:artifact",
            "whitebox_producer":{"adapter":"test","identity":"whitebox"},
            "whitebox_evidence_ids":["whitebox-1"],
            "blackbox_evidence_ids":["blackbox-1"],
            "deployment":{},
            "source_unchanged":true,
            "result":"pass",
            "created_at":"2026-01-01T00:01:00Z"
        }))
        .unwrap()
            + "\n",
    )
    .unwrap();
    assert!(Command::new("git")
        .args([
            "-C",
            root_text,
            "add",
            ".appsdk/records/fix-candidate-record-app-core.json",
            ".appsdk/records/pre-review-validation-record-app-core.json",
        ])
        .status()
        .unwrap()
        .success());
    assert!(Command::new("git")
        .args(["-C", root_text, "commit", "-m", "candidate records"])
        .status()
        .unwrap()
        .success());
    root
}

fn retire_fixture_stable_id(root: &Path, issue_id: &str) -> String {
    let candidate: Value = serde_json::from_str(
        &fs::read_to_string(root.join(".appsdk/records/fix-candidate-record-app-core.json"))
            .unwrap(),
    )
    .unwrap();
    let commit = candidate["head_commit"].as_str().unwrap();
    let tree = candidate["tree_hash"].as_str().unwrap();
    let identity = serde_json::json!({
        "module_id":"app-core",
        "issue_id":issue_id,
        "fix_candidate_id":candidate["fix_candidate_id"],
        "candidate_commit":commit,
        "candidate_tree":tree
    });
    format!(
        "candidate-{}",
        digest(&canonical(&identity))
            .strip_prefix("sha256:")
            .unwrap()
    )
}

#[test]
fn retire_lifecycle_records_archives_pair_and_is_idempotent() {
    let root = prepare_retire_fixture("retire-success", "stale-issue", "stale-issue");
    let root_text = root.to_str().unwrap();
    let records = root.join(".appsdk/records");
    let candidate_bytes = fs::read(records.join("fix-candidate-record-app-core.json")).unwrap();
    let first = run(&[
        "retire-lifecycle-records",
        root_text,
        "--module",
        "app-core",
        "--issue",
        "current-issue",
    ]);
    assert!(
        first.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&first.stdout),
        String::from_utf8_lossy(&first.stderr)
    );
    let first_json: Value = serde_json::from_slice(&first.stdout).unwrap();
    assert_eq!(first_json["retired"], true);
    assert_eq!(first_json["reused"], false);
    let archive = records
        .join("rejected/app-core")
        .join(first_json["stable_id"].as_str().unwrap());
    assert_eq!(
        fs::read(archive.join("fix-candidate-record.json")).unwrap(),
        candidate_bytes
    );
    assert!(!records.join("fix-candidate-record-app-core.json").exists());
    assert!(!records
        .join("pre-review-validation-record-app-core.json")
        .exists());

    let second = run(&[
        "retire-lifecycle-records",
        root_text,
        "--module",
        "app-core",
        "--issue",
        "current-issue",
    ]);
    assert!(
        second.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&second.stdout),
        String::from_utf8_lossy(&second.stderr)
    );
    let second_json: Value = serde_json::from_slice(&second.stdout).unwrap();
    assert_eq!(second_json["stable_id"], first_json["stable_id"]);
    assert_eq!(second_json["retired"], false);
    assert_eq!(second_json["reused"], true);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn retire_lifecycle_records_accepts_producer_dirty_candidate_and_validation() {
    let root = prepare_retire_fixture(
        "retire-producer-dirty-records",
        "stale-issue",
        "stale-issue",
    );
    let root_text = root.to_str().unwrap();
    let records = root.join(".appsdk/records");
    let candidate_path = records.join("fix-candidate-record-app-core.json");
    let validation_path = records.join("pre-review-validation-record-app-core.json");

    let mut candidate_bytes = fs::read(&candidate_path).unwrap();
    candidate_bytes.extend_from_slice(b"\n");
    fs::write(&candidate_path, &candidate_bytes).unwrap();
    let mut validation_bytes = fs::read(&validation_path).unwrap();
    validation_bytes.extend_from_slice(b"\n");
    fs::write(&validation_path, &validation_bytes).unwrap();

    let result = run(&[
        "retire-lifecycle-records",
        root_text,
        "--module",
        "app-core",
        "--issue",
        "current-issue",
    ]);
    assert!(
        result.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    let value: Value = serde_json::from_slice(&result.stdout).unwrap();
    let archive = records
        .join("rejected/app-core")
        .join(value["stable_id"].as_str().unwrap());
    assert_eq!(
        fs::read(archive.join("fix-candidate-record.json")).unwrap(),
        candidate_bytes
    );
    assert_eq!(
        fs::read(archive.join("pre-review-validation-record.json")).unwrap(),
        validation_bytes
    );
    assert!(!candidate_path.exists());
    assert!(!validation_path.exists());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn retire_lifecycle_records_rejects_binding_mismatch_and_current_issue() {
    let mismatch = prepare_retire_fixture("retire-mismatch", "stale-issue", "other-issue");
    let mismatch_result = run(&[
        "retire-lifecycle-records",
        mismatch.to_str().unwrap(),
        "--module",
        "app-core",
        "--issue",
        "current-issue",
    ]);
    assert!(!mismatch_result.status.success());
    assert!(
        String::from_utf8_lossy(&mismatch_result.stderr).contains("RETIRE_RECORD_BINDING_MISMATCH")
    );
    assert!(mismatch
        .join(".appsdk/records/fix-candidate-record-app-core.json")
        .is_file());
    fs::remove_dir_all(mismatch).unwrap();

    let current = prepare_retire_fixture("retire-current-issue", "current-issue", "current-issue");
    let current_result = run(&[
        "retire-lifecycle-records",
        current.to_str().unwrap(),
        "--module",
        "app-core",
        "--issue",
        "current-issue",
    ]);
    assert!(!current_result.status.success());
    assert!(
        String::from_utf8_lossy(&current_result.stderr).contains("RETIRE_CURRENT_ISSUE_PROTECTED")
    );
    assert!(current
        .join(".appsdk/records/pre-review-validation-record-app-core.json")
        .is_file());
    fs::remove_dir_all(current).unwrap();
}

#[test]
fn retire_lifecycle_records_rejects_partial_archive_and_dirty_worktree() {
    let partial = prepare_retire_fixture("retire-partial", "stale-issue", "stale-issue");
    let stable_id = retire_fixture_stable_id(&partial, "stale-issue");
    let archive = partial
        .join(".appsdk/records/rejected/app-core")
        .join(stable_id);
    fs::create_dir_all(&archive).unwrap();
    let candidate: Value = serde_json::from_str(
        &fs::read_to_string(partial.join(".appsdk/records/fix-candidate-record-app-core.json"))
            .unwrap(),
    )
    .unwrap();
    fs::write(
        archive.join("manifest.json"),
        serde_json::to_string_pretty(&serde_json::json!({
            "schema_version":1,
            "stable_id":archive.file_name().unwrap().to_string_lossy(),
            "module_id":"app-core",
            "issue_id":"stale-issue",
            "fix_candidate_id":candidate["fix_candidate_id"],
            "candidate_commit":candidate["head_commit"],
            "candidate_tree":candidate["tree_hash"],
            "records":[]
        }))
        .unwrap()
            + "\n",
    )
    .unwrap();
    let partial_result = run(&[
        "retire-lifecycle-records",
        partial.to_str().unwrap(),
        "--module",
        "app-core",
        "--issue",
        "current-issue",
    ]);
    assert!(!partial_result.status.success());
    assert!(String::from_utf8_lossy(&partial_result.stderr).contains("RETIRE_ARCHIVE_PARTIAL"));
    assert!(partial
        .join(".appsdk/records/fix-candidate-record-app-core.json")
        .is_file());
    fs::remove_dir_all(partial).unwrap();

    let dirty = prepare_retire_fixture("retire-dirty", "stale-issue", "stale-issue");
    for path in [
        dirty.join(".appsdk/records/fix-candidate-record-app-core.json"),
        dirty.join(".appsdk/records/pre-review-validation-record-app-core.json"),
    ] {
        let mut bytes = fs::read(&path).unwrap();
        bytes.extend_from_slice(b"\n");
        fs::write(&path, bytes).unwrap();
    }
    fs::write(dirty.join("unrelated-dirty.txt"), "must block\n").unwrap();
    let dirty_result = run(&[
        "retire-lifecycle-records",
        dirty.to_str().unwrap(),
        "--module",
        "app-core",
        "--issue",
        "current-issue",
    ]);
    assert!(!dirty_result.status.success());
    assert!(String::from_utf8_lossy(&dirty_result.stderr).contains("RETIRE_WORKTREE_DIRTY"));
    assert!(dirty
        .join(".appsdk/records/pre-review-validation-record-app-core.json")
        .is_file());
    fs::remove_dir_all(dirty).unwrap();
}
