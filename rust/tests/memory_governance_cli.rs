use serde_json::{json, Value};
use std::fs::{self, OpenOptions};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Clone, Copy)]
enum MemoryCli {
    ProjectMemory,
    AppsdkMemory,
}

struct Scratch {
    base: PathBuf,
}

impl Scratch {
    fn new(test_name: &str) -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let base = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("memory-governance-cli-tests")
            .join(format!("{}-{}-{}", test_name, std::process::id(), nanos));
        let scratch = Self { base };
        fs::create_dir_all(scratch.home().join("global")).unwrap();
        fs::create_dir_all(scratch.project().join("memory")).unwrap();
        scratch
    }

    fn project(&self) -> PathBuf {
        self.base.join("project")
    }

    fn home(&self) -> PathBuf {
        self.base.join("home")
    }

    fn project_named(&self, name: &str) -> PathBuf {
        let project = self.base.join(name);
        fs::create_dir_all(project.join("memory")).unwrap();
        project
    }

    fn write_json(&self, name: &str, value: &Value) -> String {
        let path = self.base.join(name);
        fs::write(&path, serde_json::to_string_pretty(value).unwrap() + "\n").unwrap();
        path.to_str().unwrap().to_owned()
    }

    fn raw_path(&self, global: bool, category: &str) -> PathBuf {
        if global {
            self.home().join("global").join(format!("{category}.jsonl"))
        } else {
            self.project()
                .join("memory")
                .join(format!("{category}.jsonl"))
        }
    }

    fn run(&self, project: &Path, cli: MemoryCli, args: &[&str]) -> Output {
        let home = fs::canonicalize(self.home()).unwrap();
        let binary = match cli {
            MemoryCli::ProjectMemory => env!("CARGO_BIN_EXE_project-memory"),
            MemoryCli::AppsdkMemory => env!("CARGO_BIN_EXE_appsdk"),
        };
        let mut command = Command::new(binary);
        if matches!(cli, MemoryCli::AppsdkMemory) {
            command.arg("memory");
        }
        command
            .args(args)
            .current_dir(project)
            .env("PROJECT_MEMORY_HOME", &home)
            .env("COLLAB_STATE_DIR", home.join("collab"))
            .output()
            .unwrap()
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.base);
    }
}

fn canonical_string(path: &Path) -> String {
    fs::canonicalize(path).unwrap().to_str().unwrap().to_owned()
}

fn assert_success(output: &Output) {
    assert!(
        output.status.success(),
        "project-memory failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn assert_failure(output: &Output, expected: &str) {
    assert!(
        !output.status.success(),
        "command unexpectedly succeeded\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let text = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !text.contains("\"error\":\"MEMORY_USAGE\""),
        "command was rejected by the CLI parser instead of the business gate:\n{text}"
    );
    assert!(
        text.contains(expected),
        "expected {expected:?} in failure\n{text}"
    );
}

fn output_text(output: &Output) -> String {
    format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn output_json(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout)
        .or_else(|_| serde_json::from_slice(&output.stderr))
        .unwrap_or_else(|error| {
            panic!(
                "expected JSON output ({error})\nstdout:\n{}\nstderr:\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            )
        })
}

fn json_contains_key_value(value: &Value, key: &str, expected: &Value) -> bool {
    if value.get(key) == Some(expected) {
        return true;
    }
    match value {
        Value::Array(values) => values
            .iter()
            .any(|value| json_contains_key_value(value, key, expected)),
        Value::Object(values) => values
            .values()
            .any(|value| json_contains_key_value(value, key, expected)),
        _ => false,
    }
}

fn read_raw(path: &Path) -> String {
    if path.is_file() {
        fs::read_to_string(path).unwrap()
    } else {
        String::new()
    }
}

fn append_raw(path: &Path, value: &Value) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    let mut text = read_raw(path);
    text.push_str(&serde_json::to_string(value).unwrap());
    text.push('\n');
    fs::write(path, text).unwrap();
}

fn raw_events(path: &Path) -> Vec<Value> {
    read_raw(path)
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

fn latest_for_id(path: &Path, id: &str) -> Value {
    raw_events(path)
        .into_iter()
        .filter(|event| event["id"] == id)
        .last()
        .unwrap_or_else(|| panic!("missing raw event for {id} in {}", path.display()))
}

fn assert_candidate(event: &Value, expected_level: i64) {
    assert_eq!(event["entry_kind"], "rule", "event: {event}");
    assert_eq!(event["memory_level"], expected_level, "event: {event}");
    assert_eq!(
        event["rule_control"]["approval"]["state"], "candidate",
        "candidate must carry candidate control, not an applied approval: {event}"
    );
}

fn assert_approved(event: &Value, expected_revision: i64) {
    assert_eq!(event["entry_kind"], "rule", "event: {event}");
    assert_eq!(
        event["rule_control"]["approval"]["state"], "approved",
        "event: {event}"
    );
    assert_eq!(
        event["rule_control"]["revision"], expected_revision,
        "event: {event}"
    );
}

fn rule_clause(clause_id: &str) -> Value {
    json!({
        "clause_id": clause_id,
        "axis": "behavior",
        "axis_id": format!("{clause_id}-behavior"),
        "scope": "Project Memory rule promotion test",
        "scenario": "a rule is promoted or replaced",
        "trigger": "a public Memory CLI submits a rule change",
        "rule": "A matching user approval is required for rule promotion.",
        "allowed_actions": ["accept a matching user approval"],
        "forbidden_actions": ["accept review or evidence as user approval"],
        "state_before": "rule_candidate",
        "state_after": "rule_approved",
        "effects": ["one scoped raw event is committed"],
        "owner": "MemoryCommitOwner",
        "evidence": ["rust/tests/memory_governance_cli.rs"],
        "governance": {
            "edge": "project-memory-rule-promotion/prepare->commit",
            "gate_kind": "attribute",
            "check": "memory.user_approval@1",
            "allow_case": "matching-user-approval",
            "deny_case": "missing-or-mismatched-approval",
            "observation": "public Memory CLI receipt and raw JSONL"
        }
    })
}

fn project_approval(id: &str, root: &str, revision: i64) -> Value {
    json!({
        "state": "approved",
        "role": "user",
        "scope": {"kind": "project", "root": root},
        "entry_id": id,
        "revision": revision,
        "source": "user-approval-fixture",
        "text": "The user explicitly approves this rule revision."
    })
}

fn global_approval(id: &str, home: &str, revision: i64) -> Value {
    json!({
        "state": "approved",
        "role": "user",
        "scope": {"kind": "global", "home": home},
        "entry_id": id,
        "revision": revision,
        "source": "user-approval-fixture",
        "text": "The user explicitly approves this global rule revision."
    })
}

fn rule_entry(id: &str, content: &str, approval: Option<Value>) -> Value {
    let mut control = json!({
        "revision": 1,
        "clause": rule_clause(id)
    });
    if let Some(approval) = approval {
        control["approval"] = approval;
    } else {
        control["approval"] = json!({"state": "candidate"});
    }
    json!({
        "id": id,
        "category": "knowledge",
        "title": format!("Rule {id}"),
        "content": content,
        "tags": ["memory-governance"],
        "source_refs": ["rust/tests/memory_governance_cli.rs"],
        "entry_kind": "rule",
        "rule_control": control,
        "memory_level": 3,
        "review_status": "unreviewed",
        "review_evidence": [],
        "layer": 3,
        "detail_path": format!("memory/L3/{id}.md"),
        "created_at": "2026-11-12T00:00:00Z",
        "updated_at": "2026-11-12T00:00:00Z"
    })
}

fn create_candidate(
    scratch: &Scratch,
    cli: MemoryCli,
    project: &Path,
    id: &str,
    global: bool,
) -> Value {
    let root = canonical_string(project);
    let entry = rule_entry(id, "A rule remains a candidate until approved.", None);
    let input = scratch.write_json(&format!("{id}-candidate.json"), &entry);
    let output = if global {
        scratch.run(
            project,
            cli,
            &["entry", &root, "--global", "--input", &input],
        )
    } else {
        scratch.run(project, cli, &["entry", &root, "--input", &input])
    };
    assert_success(&output);
    let latest = latest_for_id(&scratch.raw_path(global, "knowledge"), id);
    assert_candidate(&latest, 3);
    latest
}

fn approve_candidate(
    scratch: &Scratch,
    cli: MemoryCli,
    project: &Path,
    id: &str,
    approval: Value,
    global: bool,
) -> Output {
    let root = canonical_string(project);
    let approval_path = scratch.write_json(&format!("{id}-approval.json"), &approval);
    if global {
        scratch.run(
            project,
            cli,
            &[
                "promote",
                &root,
                "--global",
                "--id",
                id,
                "--level",
                "2",
                "--evidence",
                "review/rule-promotion",
                "--approval",
                &approval_path,
            ],
        )
    } else {
        scratch.run(
            project,
            cli,
            &[
                "promote",
                &root,
                "--id",
                id,
                "--level",
                "2",
                "--evidence",
                "review/rule-promotion",
                "--approval",
                &approval_path,
            ],
        )
    }
}

fn create_approved_rule(
    scratch: &Scratch,
    cli: MemoryCli,
    project: &Path,
    id: &str,
    global: bool,
) -> Value {
    create_candidate(scratch, cli, project, id, global);
    let root = canonical_string(project);
    let approval = if global {
        global_approval(id, &canonical_string(&scratch.home()), 1)
    } else {
        project_approval(id, &root, 1)
    };
    let output = approve_candidate(scratch, cli, project, id, approval, global);
    assert_success(&output);
    let latest = latest_for_id(&scratch.raw_path(global, "knowledge"), id);
    assert_approved(&latest, 1);
    latest
}

fn find_named_file(root: &Path, name: &str) -> Option<PathBuf> {
    for entry in fs::read_dir(root).ok()? {
        let entry = entry.ok()?;
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path).ok()?;
        if metadata.file_type().is_file() && path.file_name().is_some_and(|value| value == name) {
            return Some(path);
        }
        if metadata.file_type().is_dir() {
            if let Some(found) = find_named_file(&path, name) {
                return Some(found);
            }
        }
    }
    None
}

const DETAIL_MARKER_PREFIX: &str = "<!-- project-memory:v1 ";

fn detail_metadata(markdown: &str) -> Value {
    let start = markdown.find(DETAIL_MARKER_PREFIX).unwrap() + DETAIL_MARKER_PREFIX.len();
    let end = markdown[start..].find(" -->").unwrap() + start;
    serde_json::from_str(&markdown[start..end]).unwrap()
}

fn replace_detail_metadata(markdown: &str, metadata: &Value) -> String {
    let start = markdown.find(DETAIL_MARKER_PREFIX).unwrap() + DETAIL_MARKER_PREFIX.len();
    let end = markdown[start..].find(" -->").unwrap() + start;
    format!(
        "{}{}{}",
        &markdown[..start],
        serde_json::to_string(metadata).unwrap(),
        &markdown[end..]
    )
}

#[test]
fn ordinary_and_unclassified_records_remain_usable_without_approval() {
    let scratch = Scratch::new("ordinary-records");
    let project = scratch.project();
    let root = canonical_string(&project);
    let created = scratch.run(
        &project,
        MemoryCli::ProjectMemory,
        &[
            "entry",
            &root,
            "--id",
            "plain-fact",
            "--category",
            "knowledge",
            "--title",
            "Plain fact",
            "--text",
            "Ordinary facts do not require rule approval.",
            "--tag",
            "fact",
        ],
    );
    assert_success(&created);

    let legacy = json!({
        "id": "legacy-unclassified",
        "category": "knowledge",
        "title": "Legacy unclassified",
        "content": "Old records without entry_kind remain readable.",
        "tags": ["legacy"],
        "source_refs": ["fixture"],
        "memory_level": 3,
        "review_status": "unreviewed"
    });
    append_raw(&scratch.raw_path(false, "knowledge"), &legacy);

    for (id, expected) in [
        ("plain-fact", "Ordinary facts do not require rule approval."),
        (
            "legacy-unclassified",
            "Old records without entry_kind remain readable.",
        ),
    ] {
        let fetched = scratch.run(&project, MemoryCli::ProjectMemory, &["get", id, &root]);
        assert_success(&fetched);
        let fetched = output_json(&fetched);
        assert_eq!(fetched["matches"][0]["content"], expected, "{fetched}");
        assert_ne!(
            fetched["matches"][0]["rule_control"]["approval"]["state"], "approved",
            "{fetched}"
        );
    }
}

#[test]
fn review_records_rule_evidence_without_rule_promotion_and_keeps_fact_review() {
    let scratch = Scratch::new("review-paths");
    let project = scratch.project();
    let candidate = create_candidate(
        &scratch,
        MemoryCli::ProjectMemory,
        &project,
        "review-rule",
        false,
    );
    let run_dir = scratch.home().join("collab/runs/run-review-path");
    fs::create_dir_all(&run_dir).unwrap();
    let notes = [
        json!({
            "record_type": "knowledge",
            "memory": candidate,
            "memory_review": {
                "status": "reviewed",
                "level": 2,
                "evidence": ["review/rule-pass"]
            }
        }),
        json!({
            "record_type": "lesson",
            "memory": {
                "id": "review-fact",
                "category": "lesson",
                "title": "Reviewed fact",
                "content": "A legacy fact keeps the existing review-level behavior.",
                "tags": ["review"],
                "source_refs": ["run/review-path"]
            },
            "memory_review": {
                "status": "reviewed",
                "level": 2,
                "evidence": ["review/fact-pass"]
            }
        }),
    ];
    fs::write(
        run_dir.join("notes.jsonl"),
        notes
            .iter()
            .map(|note| serde_json::to_string(note).unwrap())
            .collect::<Vec<_>>()
            .join("\n")
            + "\n",
    )
    .unwrap();

    let reviewed = scratch.run(
        &project,
        MemoryCli::ProjectMemory,
        &["review", "--run", "run-review-path"],
    );
    assert_success(&reviewed);
    let receipt = output_json(&reviewed);
    assert!(json_contains_key_value(
        &receipt,
        "review_recorded",
        &json!(true)
    ));
    assert!(json_contains_key_value(
        &receipt,
        "promotion_applied",
        &json!(false)
    ));
    assert!(json_contains_key_value(
        &receipt,
        "reason",
        &json!("user_approval_required")
    ));

    let raw = scratch.raw_path(false, "knowledge");
    let rule = latest_for_id(&raw, "review-rule");
    assert_candidate(&rule, 3);
    assert_eq!(rule["review_status"], "reviewed", "{rule}");
    assert_eq!(rule["review_evidence"], json!(["review/rule-pass"]));

    let fact_raw = scratch.raw_path(false, "lesson");
    let fact = latest_for_id(&fact_raw, "review-fact");
    assert_eq!(fact["memory_level"], 2, "{fact}");
    assert_eq!(fact["review_status"], "reviewed", "{fact}");
}

#[test]
fn batch_io_failure_reports_committed_and_uncommitted_entries_and_retries() {
    let scratch = Scratch::new("batch-io-prefix");
    let project = scratch.project();
    let run_dir = scratch.home().join("collab/runs/run-batch-prefix");
    fs::create_dir_all(&run_dir).unwrap();
    let notes = [
        ("a-good", "knowledge"),
        ("b-blocked", "lesson"),
        ("c-pending", "knowledge"),
    ]
    .into_iter()
    .map(|(id, category)| {
        serde_json::to_string(&json!({
            "memory": {
                "id": id,
                "category": category,
                "title": id,
                "content": format!("Batch entry {id}"),
                "tags": ["batch"],
                "source_refs": ["fixture/batch-prefix"]
            }
        }))
        .unwrap()
    })
    .collect::<Vec<_>>()
    .join("\n")
        + "\n";
    fs::write(run_dir.join("notes.jsonl"), notes).unwrap();
    let blocked_raw = scratch.raw_path(false, "lesson");
    fs::create_dir(&blocked_raw).unwrap();

    let failed = scratch.run(
        &project,
        MemoryCli::ProjectMemory,
        &["review", "--run", "run-batch-prefix"],
    );
    assert_failure(&failed, "MEMORY_WRITE_FAILED");
    let receipt = output_json(&failed);
    assert_eq!(receipt["committed"], true, "{receipt}");
    let knowledge_raw = scratch.raw_path(false, "knowledge");
    assert_eq!(raw_events(&knowledge_raw).len(), 1);
    assert_eq!(latest_for_id(&knowledge_raw, "a-good")["id"], "a-good");
    assert_eq!(receipt["receipts"][0]["id"], "a-good", "{receipt}");
    assert_eq!(
        receipt["uncommitted_ids"],
        json!(["b-blocked", "c-pending"]),
        "{receipt}"
    );
    let staging_files = fs::read_dir(project.join("memory"))
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .filter(|name| name.to_string_lossy().starts_with("lesson.staging."))
        .collect::<Vec<_>>();
    assert!(
        staging_files.is_empty(),
        "uncommitted staging files remain after failure: {staging_files:?}"
    );

    fs::remove_dir(&blocked_raw).unwrap();
    let retried = scratch.run(
        &project,
        MemoryCli::ProjectMemory,
        &["review", "--run", "run-batch-prefix"],
    );
    assert_success(&retried);
    assert_eq!(raw_events(&knowledge_raw).len(), 2);
    assert_eq!(raw_events(&blocked_raw).len(), 1);
    assert_eq!(latest_for_id(&blocked_raw, "b-blocked")["id"], "b-blocked");
    assert_eq!(
        latest_for_id(&knowledge_raw, "c-pending")["id"],
        "c-pending"
    );
}

#[test]
fn new_rule_approval_is_not_applied_then_matching_approval_promotes() {
    let scratch = Scratch::new("initial-approval");
    let project = scratch.project();
    let root = canonical_string(&project);
    let id = "initial-approved-input";
    let approval = project_approval(id, &root, 1);
    let mut entry = rule_entry(
        id,
        "Initial approved input must first become a candidate.",
        Some(approval.clone()),
    );
    entry["memory_review"] = json!({
        "status": "reviewed",
        "level": 2,
        "evidence": ["review/initial-input"]
    });
    let input = scratch.write_json("initial-approved-input.json", &entry);
    let created = scratch.run(
        &project,
        MemoryCli::ProjectMemory,
        &["entry", &root, "--input", &input],
    );
    assert_success(&created);
    let receipt = output_json(&created);
    assert!(json_contains_key_value(
        &receipt,
        "approval_not_applied",
        &json!("new_entry_requires_promotion")
    ));
    let raw = scratch.raw_path(false, "knowledge");
    assert_candidate(&latest_for_id(&raw, id), 3);

    let promoted = approve_candidate(
        &scratch,
        MemoryCli::ProjectMemory,
        &project,
        id,
        approval,
        false,
    );
    assert_success(&promoted);
    let latest = latest_for_id(&raw, id);
    assert_approved(&latest, 1);
    assert_eq!(latest["memory_level"], 2, "{latest}");
}

#[test]
fn project_approval_scope_id_and_revision_mismatches_leave_raw_unchanged() {
    let scratch = Scratch::new("project-approval-mismatch");
    let project = scratch.project();
    let root = canonical_string(&project);
    let id = "project-match-rule";
    create_candidate(&scratch, MemoryCli::ProjectMemory, &project, id, false);
    let raw = scratch.raw_path(false, "knowledge");
    let before = read_raw(&raw);
    let other_root = scratch.project_named("other-project");
    let other_root = canonical_string(&other_root);

    let cases = [
        (
            "wrong-id",
            project_approval("other-rule", &root, 1),
            "MEMORY_",
        ),
        ("wrong-revision", project_approval(id, &root, 2), "MEMORY_"),
        (
            "wrong-scope",
            project_approval(id, &other_root, 1),
            "MEMORY_",
        ),
    ];
    for (label, approval, expected_error) in cases {
        let output = approve_candidate(
            &scratch,
            MemoryCli::ProjectMemory,
            &project,
            id,
            approval,
            false,
        );
        assert_failure(&output, expected_error);
        assert_eq!(
            read_raw(&raw),
            before,
            "mismatch case {label} changed project raw"
        );
    }
    assert_candidate(&latest_for_id(&raw, id), 3);
}

#[test]
fn global_scope_accepts_matching_approval_and_rejects_wrong_home() {
    let scratch = Scratch::new("global-approval");
    let project = scratch.project();
    let home = canonical_string(&scratch.home());
    let id = "global-approved-rule";
    create_candidate(&scratch, MemoryCli::AppsdkMemory, &project, id, true);
    let promoted = approve_candidate(
        &scratch,
        MemoryCli::AppsdkMemory,
        &project,
        id,
        global_approval(id, &home, 1),
        true,
    );
    assert_success(&promoted);
    let raw = scratch.raw_path(true, "knowledge");
    assert_approved(&latest_for_id(&raw, id), 1);

    let wrong_id = "global-wrong-home";
    create_candidate(&scratch, MemoryCli::AppsdkMemory, &project, wrong_id, true);
    let before = read_raw(&raw);
    let wrong_home = scratch.project_named("wrong-memory-home");
    let wrong_home = canonical_string(&wrong_home);
    let rejected = approve_candidate(
        &scratch,
        MemoryCli::AppsdkMemory,
        &project,
        wrong_id,
        global_approval(wrong_id, &wrong_home, 1),
        true,
    );
    assert_failure(&rejected, "MEMORY_");
    assert_eq!(read_raw(&raw), before);
    assert_candidate(&latest_for_id(&raw, wrong_id), 3);
}

#[test]
fn same_semantics_update_preserves_approved_control_and_revision() {
    let scratch = Scratch::new("same-semantics");
    let project = scratch.project();
    let root = canonical_string(&project);
    let id = "same-semantics-rule";
    create_approved_rule(&scratch, MemoryCli::ProjectMemory, &project, id, false);

    let changed = json!({
        "id": id,
        "category": "knowledge",
        "title": format!("Rule {id}"),
        "content": "A rule remains a candidate until approved.",
        "tags": ["memory-governance", "same-semantic-update"],
        "source_refs": ["rust/tests/memory_governance_cli.rs"],
        "memory_level": 2,
        "review_status": "reviewed",
        "review_evidence": ["review/rule-promotion"]
    });
    let input = scratch.write_json("same-semantics-update.json", &changed);
    let output = scratch.run(
        &project,
        MemoryCli::ProjectMemory,
        &["entry", &root, "--input", &input],
    );
    assert_success(&output);

    let latest = latest_for_id(&scratch.raw_path(false, "knowledge"), id);
    assert_approved(&latest, 1);
    assert_eq!(latest["memory_level"], 2, "{latest}");
    assert!(latest["tags"]
        .as_array()
        .unwrap()
        .iter()
        .any(|tag| tag == "same-semantic-update"));
}

#[test]
fn unapproved_semantic_change_is_rejected_without_raw_change() {
    let scratch = Scratch::new("unapproved-semantic-change");
    let project = scratch.project();
    let root = canonical_string(&project);
    let id = "unapproved-change-rule";
    create_approved_rule(&scratch, MemoryCli::ProjectMemory, &project, id, false);
    let raw = scratch.raw_path(false, "knowledge");
    let before = read_raw(&raw);

    let changed = json!({
        "id": id,
        "category": "knowledge",
        "title": "Changed rule semantics",
        "content": "This changed rule text has no matching new approval.",
        "tags": ["memory-governance"],
        "source_refs": ["rust/tests/memory_governance_cli.rs"]
    });
    let input = scratch.write_json("unapproved-change.json", &changed);
    let output = scratch.run(
        &project,
        MemoryCli::ProjectMemory,
        &["entry", &root, "--input", &input],
    );
    assert_failure(&output, "MEMORY_RULE_REAPPROVAL_REQUIRED");
    assert_eq!(read_raw(&raw), before);

    let fetched = scratch.run(&project, MemoryCli::ProjectMemory, &["get", id, &root]);
    assert_success(&fetched);
    let fetched = output_json(&fetched);
    assert_eq!(
        fetched["matches"][0]["content"],
        "A rule remains a candidate until approved."
    );
    assert_eq!(fetched["matches"][0]["memory_level"], 2);
}

#[test]
fn explicit_fact_to_rule_conversion_saves_candidate_then_requires_approval() {
    let scratch = Scratch::new("fact-to-rule");
    let project = scratch.project();
    let root = canonical_string(&project);
    let id = "converted-rule";
    let created = scratch.run(
        &project,
        MemoryCli::ProjectMemory,
        &[
            "entry",
            &root,
            "--id",
            id,
            "--category",
            "knowledge",
            "--title",
            "Converted fact",
            "--text",
            "This fact is explicitly converted into a rule candidate.",
        ],
    );
    assert_success(&created);

    let converted = rule_entry(
        id,
        "This fact is explicitly converted into a rule candidate.",
        None,
    );
    let input = scratch.write_json("converted-rule.json", &converted);
    let output = scratch.run(
        &project,
        MemoryCli::ProjectMemory,
        &["entry", &root, "--input", &input],
    );
    assert_success(&output);
    let raw = scratch.raw_path(false, "knowledge");
    let candidate = latest_for_id(&raw, id);
    assert_candidate(&candidate, 3);
    assert_eq!(candidate["review_status"], "unreviewed", "{candidate}");

    let promoted = approve_candidate(
        &scratch,
        MemoryCli::ProjectMemory,
        &project,
        id,
        project_approval(id, &root, 1),
        false,
    );
    assert_success(&promoted);
    assert_approved(&latest_for_id(&raw, id), 1);
}

#[test]
fn export_import_round_trip_preserves_control_and_rejects_old_approval_for_new_body() {
    let scratch = Scratch::new("control-round-trip");
    let project = scratch.project();
    let root = canonical_string(&project);
    let id = "round-trip-approved-rule";
    create_candidate(&scratch, MemoryCli::ProjectMemory, &project, id, false);
    let exported = scratch.run(&project, MemoryCli::ProjectMemory, &["export", &root]);
    assert_success(&exported);

    let fetched = scratch.run(&project, MemoryCli::ProjectMemory, &["get", id, &root]);
    assert_success(&fetched);
    let fetched = output_json(&fetched);
    let detail = project.join(fetched["matches"][0]["detail_path"].as_str().unwrap());
    let candidate_markdown = fs::read_to_string(&detail).unwrap();
    let mut metadata = detail_metadata(&candidate_markdown);
    metadata["entry_kind"] = json!("rule");
    metadata["rule_control"] = json!({
        "revision": 1,
        "clause": rule_clause(id),
        "approval": project_approval(id, &root, 1)
    });
    metadata["memory_review"] = json!({
        "status": "reviewed",
        "level": 2,
        "evidence": ["review/import-promotion"]
    });
    fs::write(
        &detail,
        replace_detail_metadata(&candidate_markdown, &metadata),
    )
    .unwrap();

    let imported = scratch.run(&project, MemoryCli::ProjectMemory, &["import", &root]);
    assert_success(&imported);
    let raw = scratch.raw_path(false, "knowledge");
    let approved = latest_for_id(&raw, id);
    assert_approved(&approved, 1);
    assert_eq!(approved["memory_level"], 2, "{approved}");

    let exported = scratch.run(&project, MemoryCli::ProjectMemory, &["export", &root]);
    assert_success(&exported);
    let fetched = scratch.run(&project, MemoryCli::ProjectMemory, &["get", id, &root]);
    assert_success(&fetched);
    let fetched = output_json(&fetched);
    let detail = project.join(fetched["matches"][0]["detail_path"].as_str().unwrap());
    let markdown = fs::read_to_string(&detail).unwrap();
    assert!(
        markdown.contains("\"rule_control\""),
        "export must preserve typed control: {markdown}"
    );

    let before = read_raw(&raw);
    let unchanged = scratch.run(&project, MemoryCli::ProjectMemory, &["import", &root]);
    assert_success(&unchanged);
    assert_eq!(read_raw(&raw), before, "unchanged import appended raw");

    let changed = markdown.replace(
        "A rule remains a candidate until approved.",
        "This body changed without a new approval.",
    );
    assert_ne!(changed, markdown);
    fs::write(&detail, changed).unwrap();
    let rejected = scratch.run(&project, MemoryCli::ProjectMemory, &["import", &root]);
    assert_failure(&rejected, "MEMORY_RULE_REAPPROVAL_REQUIRED");
    assert_eq!(
        read_raw(&raw),
        before,
        "failed import must not write a partial raw prefix"
    );
}

#[test]
fn mixed_rule_and_fact_import_projects_every_committed_entry() {
    let scratch = Scratch::new("mixed-import-projection");
    let project = scratch.project();
    let root = canonical_string(&project);
    let id = "aa-approved-rule";
    create_candidate(&scratch, MemoryCli::ProjectMemory, &project, id, false);
    let promoted = approve_candidate(
        &scratch,
        MemoryCli::ProjectMemory,
        &project,
        id,
        project_approval(id, &root, 1),
        false,
    );
    assert_success(&promoted);
    let fetched = scratch.run(&project, MemoryCli::ProjectMemory, &["get", id, &root]);
    assert_success(&fetched);
    let fetched = output_json(&fetched);
    let detail = project.join(fetched["matches"][0]["detail_path"].as_str().unwrap());
    let markdown = fs::read_to_string(&detail).unwrap();
    let mut metadata = detail_metadata(&markdown);
    metadata["rule_control"]["revision"] = json!(2);
    metadata["rule_control"]["approval"] = project_approval(id, &root, 2);
    let updated = markdown.replace(
        "A rule remains a candidate until approved.",
        "The user approved this mixed batch rule update.",
    );
    assert_ne!(updated, markdown);
    fs::write(&detail, replace_detail_metadata(&updated, &metadata)).unwrap();

    let fact = json!({
        "id": "zz-mixed-fact",
        "category": "knowledge",
        "title": "Mixed batch fact",
        "tags": ["mixed-batch"],
        "source_refs": ["fixture/mixed-import"],
        "memory_review": {
            "status": "reviewed",
            "level": 2,
            "evidence": ["review/mixed-import"]
        }
    });
    let fact_detail = project.join("memory/L2/zz-mixed-fact.md");
    fs::write(
        &fact_detail,
        format!(
            "<!-- project-memory:v1 {} -->\n\n# Mixed batch fact\n\nmixedbatchsearchtoken\n<!-- project-memory:end -->\n",
            serde_json::to_string(&fact).unwrap()
        ),
    )
    .unwrap();
    let imported = scratch.run(&project, MemoryCli::ProjectMemory, &["import", &root]);
    assert_success(&imported);
    let raw = scratch.raw_path(false, "knowledge");
    assert_approved(&latest_for_id(&raw, id), 2);
    assert_eq!(
        latest_for_id(&raw, "zz-mixed-fact")["content"],
        "mixedbatchsearchtoken"
    );
    let index = fs::read_to_string(project.join("memory/index.md")).unwrap();
    assert!(
        index.contains("zz-mixed-fact"),
        "the successful import omitted a committed fact from the index: {index}"
    );

    let queried = scratch.run(
        &project,
        MemoryCli::ProjectMemory,
        &["query", "mixedbatchsearchtoken", &root],
    );
    assert_success(&queried);
    let result = output_json(&queried);
    assert!(
        json_contains_key_value(&result, "id", &json!("zz-mixed-fact")),
        "the committed fact is missing from the projection: {result}"
    );
}

#[test]
fn automatic_l3_import_saves_approved_rule_as_candidate() {
    let scratch = Scratch::new("automatic-l3-rule");
    let project = scratch.project();
    let root = canonical_string(&project);
    let id = "auto-l3-rule";
    let mut control = json!({
        "revision": 1,
        "clause": rule_clause(id)
    });
    control["approval"] = project_approval(id, &root, 1);
    let metadata = json!({
        "id": id,
        "category": "knowledge",
        "title": "Automatic rule",
        "tags": ["auto-l3"],
        "source_refs": ["memory/L3/auto-l3-rule.md"],
        "entry_kind": "rule",
        "rule_control": control,
        "memory_level": 1,
        "review_status": "reviewed",
        "review_evidence": ["forged-auto-import"]
    });
    let detail = project.join("memory/L3/auto-l3-rule.md");
    fs::create_dir_all(detail.parent().unwrap()).unwrap();
    fs::write(
        &detail,
        format!(
            "<!-- project-memory:v1 {} -->\n\n# Automatic rule\n\nAutomatic import must not apply approval.\n<!-- project-memory:end -->\n",
            serde_json::to_string(&metadata).unwrap()
        ),
    )
    .unwrap();

    let fetched = scratch.run(&project, MemoryCli::ProjectMemory, &["get", id, &root]);
    assert_success(&fetched);
    let raw = scratch.raw_path(false, "knowledge");
    let candidate = latest_for_id(&raw, id);
    assert_candidate(&candidate, 3);
    assert_eq!(candidate["review_status"], "unreviewed", "{candidate}");
    assert!(
        candidate["review_evidence"]
            .as_array()
            .is_none_or(|values| values.is_empty()),
        "{candidate}"
    );
    let first_count = raw_events(&raw).len();

    let indexed = scratch.run(&project, MemoryCli::ProjectMemory, &["index", &root]);
    assert_success(&indexed);
    assert_eq!(raw_events(&raw).len(), first_count);
}

#[test]
fn both_public_cli_entrypoints_match_for_allow_and_reject_paths() {
    for (label, cli) in [
        ("standalone", MemoryCli::ProjectMemory),
        ("appsdk-memory", MemoryCli::AppsdkMemory),
    ] {
        let scratch = Scratch::new(&format!("public-cli-{label}"));
        let project = scratch.project();
        let root = canonical_string(&project);
        let allowed_id = "parity-allowed";
        create_candidate(&scratch, cli, &project, allowed_id, false);
        let allowed = approve_candidate(
            &scratch,
            cli,
            &project,
            allowed_id,
            project_approval(allowed_id, &root, 1),
            false,
        );
        assert_success(&allowed);
        assert_approved(
            &latest_for_id(&scratch.raw_path(false, "knowledge"), allowed_id),
            1,
        );

        let rejected_id = "parity-rejected";
        create_candidate(&scratch, cli, &project, rejected_id, false);
        let raw = scratch.raw_path(false, "knowledge");
        let before = read_raw(&raw);
        let rejected = approve_candidate(
            &scratch,
            cli,
            &project,
            rejected_id,
            project_approval(rejected_id, &root, 2),
            false,
        );
        assert_failure(&rejected, "MEMORY_");
        assert_eq!(read_raw(&raw), before);
    }
}

#[test]
fn committed_projection_failure_reports_recovery_and_rebuild_only_adds_no_raw() {
    let scratch = Scratch::new("projection-failure");
    let project = scratch.project();
    let id = "projection-failure";
    fs::create_dir_all(project.join("memory")).unwrap();
    fs::write(
        project.join("memory/L3"),
        "fixture blocks the L3 projection directory\n",
    )
    .unwrap();

    let output = scratch.run(
        &project,
        MemoryCli::ProjectMemory,
        &[
            "entry",
            "--id",
            id,
            "--category",
            "knowledge",
            "--title",
            "Projection failure",
            "--text",
            "The raw event is committed before detail projection.",
        ],
    );
    assert!(!output.status.success(), "projection failure must fail");
    let receipt = output_json(&output);
    assert_eq!(receipt["accepted"], true, "{receipt}");
    assert_eq!(receipt["committed"], true, "{receipt}");
    assert_eq!(receipt["projection_updated"], false, "{receipt}");
    assert!(
        json_contains_key_value(&receipt, "code", &json!("MEMORY_INDEX_WRITE_FAILED"))
            || output_text(&output).contains("MEMORY_INDEX_WRITE_FAILED"),
        "{receipt}"
    );
    let recovery = receipt["recovery"].as_array().unwrap();
    assert!(
        recovery
            .iter()
            .any(|part| part.as_str().is_some_and(|part| part == "--rebuild-only")),
        "{receipt}"
    );

    let raw = scratch.raw_path(false, "knowledge");
    let committed = read_raw(&raw);
    assert!(raw_events(&raw).iter().any(|event| event["id"] == id));
    fs::remove_file(project.join("memory/L3")).unwrap();
    fs::create_dir_all(project.join("memory/L3")).unwrap();
    let manual = project.join("memory/L3/manual-only.md");
    fs::write(
        &manual,
        "<!-- project-memory:v1 {\"id\":\"manual-only\",\"category\":\"knowledge\"} -->\n\n# Manual only\n\nRebuild-only must not import this file.\n<!-- project-memory:end -->\n",
    )
    .unwrap();

    let rebuilt = scratch.run(
        &project,
        MemoryCli::ProjectMemory,
        &["index", &canonical_string(&project), "--rebuild-only"],
    );
    assert_success(&rebuilt);
    assert_eq!(read_raw(&raw), committed);
    assert!(!raw_events(&raw)
        .iter()
        .any(|event| event["id"] == "manual-only"));
    assert!(project.join("memory/L3/projection-failure.md").is_file());
}

#[test]
fn held_scope_lock_returns_busy_without_writing_and_releases_cleanly() {
    let scratch = Scratch::new("scope-lock");
    let project = scratch.project();
    let created = scratch.run(
        &project,
        MemoryCli::ProjectMemory,
        &[
            "entry",
            "--id",
            "lock-seed",
            "--category",
            "knowledge",
            "--text",
            "Create the scope lock sidecar.",
        ],
    );
    assert_success(&created);
    let lock_path = find_named_file(&scratch.base, ".write.lock")
        .expect("Memory writer must create a scope lock sidecar");
    let lock = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .open(&lock_path)
        .unwrap();
    lock.try_lock().unwrap();

    let raw = scratch.raw_path(false, "knowledge");
    let before = read_raw(&raw);
    let blocked = scratch.run(
        &project,
        MemoryCli::ProjectMemory,
        &[
            "entry",
            "--id",
            "lock-blocked",
            "--category",
            "knowledge",
            "--text",
            "The held lock must reject this write.",
        ],
    );
    assert!(!blocked.status.success());
    let blocked_text = output_text(&blocked).to_ascii_uppercase();
    assert!(
        blocked_text.contains("LOCK") || blocked_text.contains("BUSY"),
        "{}",
        output_text(&blocked)
    );
    assert_eq!(read_raw(&raw), before);

    drop(lock);
    let retried = scratch.run(
        &project,
        MemoryCli::ProjectMemory,
        &[
            "entry",
            "--id",
            "lock-blocked",
            "--category",
            "knowledge",
            "--text",
            "The held lock must reject this write.",
        ],
    );
    assert_success(&retried);
    assert_eq!(
        raw_events(&raw)
            .iter()
            .filter(|event| event["id"] == "lock-blocked")
            .count(),
        1
    );
}

#[test]
fn concurrent_distinct_writers_do_not_lose_committed_events() {
    let scratch = Scratch::new("concurrent-writers");
    let project = scratch.project();
    let home = fs::canonicalize(scratch.home()).unwrap();
    let ids = (0..12)
        .map(|index| format!("concurrent-{index}"))
        .collect::<Vec<_>>();
    let mut children = Vec::new();
    for id in &ids {
        let mut command = Command::new(env!("CARGO_BIN_EXE_project-memory"));
        command
            .args([
                "entry",
                "--id",
                id,
                "--category",
                "knowledge",
                "--text",
                "Concurrent writers must not lose committed events.",
            ])
            .current_dir(&project)
            .env("PROJECT_MEMORY_HOME", &home)
            .env("COLLAB_STATE_DIR", home.join("collab"))
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        children.push((id, command.spawn().unwrap()));
    }

    let raw = scratch.raw_path(false, "knowledge");
    let mut successful = 0;
    for (id, child) in children {
        let output = child.wait_with_output().unwrap();
        if output.status.success() {
            successful += 1;
            assert_eq!(
                raw_events(&raw)
                    .iter()
                    .filter(|event| event["id"] == *id)
                    .count(),
                1,
                "successful writer {id} was lost or duplicated"
            );
        } else {
            let text = output_text(&output).to_ascii_uppercase();
            assert!(
                text.contains("LOCK") || text.contains("BUSY"),
                "writer {id} failed for a non-conflict reason: {}",
                output_text(&output)
            );
            assert!(!raw_events(&raw).iter().any(|event| event["id"] == *id));
        }
    }
    assert_eq!(raw_events(&raw).len(), successful);
}

#[test]
fn promotion_uses_real_sdk_commit_graph_without_fake_m5_registry_operator() {
    let ontology: Value = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../contracts/memory/rule-promotion.ontology.json"
    )))
    .unwrap();
    let graph: Value = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../docs/dagpipe/graphs/project-memory-rule-promotion.graph.json"
    )))
    .unwrap();

    let operations = ontology["execution"]["operations"].as_array().unwrap();
    assert_eq!(operations.len(), 2, "{ontology}");
    for operation in operations {
        assert_eq!(operation["graph_id"], "project-memory-rule-promotion");
        assert_eq!(operation["commit_node"], "commit");
        assert_eq!(operation["clauses"], json!(["M1", "M2", "M3", "M4"]));
    }
    assert!(ontology["clauses"]
        .as_array()
        .unwrap()
        .iter()
        .all(|clause| clause["governance"]["check"] != "memory.promotion_boundary@1"));

    let nodes = graph["nodes"].as_array().unwrap();
    let prepare = nodes.iter().find(|node| node["id"] == "prepare").unwrap();
    let commit = nodes.iter().find(|node| node["id"] == "commit").unwrap();
    assert_eq!(prepare["operator"], "memory.prepare_rule");
    assert_eq!(prepare["operator_version"], "1");
    assert_eq!(commit["operator"], "governance.commit");
    assert_eq!(commit["operator_version"], "1");
    assert_eq!(
        graph["edges"],
        json!([{"from": "prepare", "to": "commit", "arc_id": "prepared"}])
    );
    assert_eq!(graph["outputs"], json!(["committed"]));
}
