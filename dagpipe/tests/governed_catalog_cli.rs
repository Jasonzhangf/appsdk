use serde_json::Value;
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
    sync::{
        atomic::{AtomicUsize, Ordering},
        Mutex,
    },
};

static EXAMPLE_LOCK: Mutex<()> = Mutex::new(());
static TEMP_SEQUENCE: AtomicUsize = AtomicUsize::new(0);

struct TempFixture {
    root: PathBuf,
}

impl TempFixture {
    fn new(name: &str) -> Self {
        let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "governed-catalog-cli-{}-{sequence}-{name}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).expect("create isolated test fixture root");
        Self { root }
    }

    fn write_json(&self, name: &str, value: &Value) -> PathBuf {
        let path = self.root.join(name);
        fs::write(
            &path,
            serde_json::to_vec(value).expect("serialize test fixture"),
        )
        .expect("write isolated test fixture");
        path
    }
}

impl Drop for TempFixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn fixture_path(relative: &[&str]) -> PathBuf {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.push("tests");
    path.push("fixtures");
    path.push("ontology-governance");
    for segment in relative {
        path.push(segment);
    }
    path
}

fn read_json(path: &Path) -> Value {
    let source = fs::read_to_string(path).expect("read fixture JSON");
    serde_json::from_str(&source).expect("parse fixture JSON")
}

fn example_command(manifest: &Path, graph: &Path, script: &Path, initial: &Path) -> Command {
    let manifest_path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
    let mut command = Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()));
    command
        .arg("run")
        .arg("--quiet")
        .arg("--locked")
        .arg("--manifest-path")
        .arg(manifest_path)
        .arg("--example")
        .arg("governed_catalog")
        .arg("--")
        .arg("--manifest")
        .arg(manifest)
        .arg("--graph")
        .arg(graph)
        .arg("--script")
        .arg(script)
        .arg("--initial")
        .arg(initial)
        .env("CARGO_NET_OFFLINE", "true");
    command
}

fn run_example(manifest: &Path, graph: &Path, script: &Path, initial: &Path) -> Output {
    let _lock = EXAMPLE_LOCK.lock().expect("example test lock");
    example_command(manifest, graph, script, initial)
        .output()
        .expect("run governed catalog example")
}

fn json_lines(output: &Output) -> Vec<Value> {
    let text = String::from_utf8_lossy(&output.stdout);
    text.lines()
        .filter(|line| !line.trim().is_empty())
        .enumerate()
        .map(|(index, line)| {
            serde_json::from_str(line)
                .unwrap_or_else(|error| panic!("parse JSON output line {index} ({error}): {line}"))
        })
        .collect()
}

fn command_line<'a>(lines: &'a [Value], command: &str, index: usize) -> &'a Value {
    lines
        .get(index)
        .unwrap_or_else(|| panic!("missing `{command}` response at line {index}: {lines:?}"))
}

fn assert_apply_failure(output: &Output, lines: &[Value], expected_code: &str) {
    assert!(
        !output.status.success(),
        "rejected apply unexpectedly succeeded with exit 0"
    );
    let response = command_line(lines, "apply", 0);
    assert_eq!(response["command"], "apply");
    assert_eq!(response["ok"], false);
    let error = response["error"].as_str().expect("apply error is a string");
    assert!(
        error.contains(expected_code),
        "expected `{expected_code}` in first error cause: {error}"
    );
}

fn assert_unchanged_record(lines: &[Value], expected_title: &str, relation_id: &str) {
    let query = command_line(lines, "query", 1);
    assert_eq!(query["command"], "query");
    assert_eq!(query["ok"], true);
    assert_eq!(query["record"]["state"], "draft");
    assert_eq!(query["record"]["data"]["title"], expected_title);
    let relations = query["relations"].as_array().expect("query relations");
    if relation_id.is_empty() {
        assert!(
            relations.is_empty(),
            "rejected call left a relation: {relations:?}"
        );
    } else {
        assert_eq!(relations.len(), 1);
        assert_eq!(relations[0]["to"]["id"], relation_id);
    }
}

#[test]
fn legal_apply_commits_once_and_is_queryable() {
    let output = run_example(
        &fixture_path(&["catalog-guidance.json"]),
        &fixture_path(&["activate.graph.json"]),
        &fixture_path(&["legal-session.jsonl"]),
        &fixture_path(&["consumer", "initial-catalog.json"]),
    );

    assert!(
        output.status.success(),
        "legal apply failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let lines = json_lines(&output);
    let apply = command_line(&lines, "apply", 0);
    assert_eq!(apply["command"], "apply");
    assert_eq!(apply["ok"], true);
    assert_eq!(apply["result"]["record"]["state"], "active");
    assert_eq!(apply["result"]["record"]["data"]["title"], "Valid title");

    let query = command_line(&lines, "query", 1);
    assert_eq!(query["command"], "query");
    assert_eq!(query["ok"], true);
    assert_eq!(query["record"]["state"], "active");
    assert_eq!(query["record"]["data"]["title"], "Valid title");
    let relations = query["relations"].as_array().expect("query relations");
    assert_eq!(relations.len(), 1);
    assert_eq!(relations[0]["to"]["id"], "g1");
}

#[test]
fn invalid_object_is_rejected_without_side_effects() {
    let output = run_example(
        &fixture_path(&["catalog-guidance.json"]),
        &fixture_path(&["activate.graph.json"]),
        &fixture_path(&["consumer", "invalid-title.jsonl"]),
        &fixture_path(&["consumer", "initial-catalog.json"]),
    );

    let lines = json_lines(&output);
    assert_apply_failure(&output, &lines, "ONTOLOGY_OBJECT_REJECTED");
    assert_unchanged_record(&lines, "Original title", "");
}

#[test]
fn invalid_group_is_rejected_without_side_effects() {
    let output = run_example(
        &fixture_path(&["catalog-guidance.json"]),
        &fixture_path(&["activate.graph.json"]),
        &fixture_path(&["consumer", "invalid-group.jsonl"]),
        &fixture_path(&["consumer", "initial-catalog.json"]),
    );

    let lines = json_lines(&output);
    assert_apply_failure(&output, &lines, "ONTOLOGY_RELATION_REJECTED");
    assert_unchanged_record(&lines, "Original title", "");
}

#[test]
fn out_of_scope_group_is_rejected_without_side_effects() {
    let output = run_example(
        &fixture_path(&["catalog-guidance.json"]),
        &fixture_path(&["activate.graph.json"]),
        &fixture_path(&["consumer", "invalid-scope.jsonl"]),
        &fixture_path(&["consumer", "initial-cross-scope.json"]),
    );

    let lines = json_lines(&output);
    assert_apply_failure(&output, &lines, "ONTOLOGY_RELATION_REJECTED");
    assert!(lines[0]["error"]
        .as_str()
        .unwrap()
        .contains("missing endpoint"));
    assert_unchanged_record(&lines, "Original title", "");
}

#[test]
fn relation_replacement_is_rejected_without_side_effects() {
    let output = run_example(
        &fixture_path(&["catalog-guidance.json"]),
        &fixture_path(&["activate.graph.json"]),
        &fixture_path(&["consumer", "invalid-relation-replace.jsonl"]),
        &fixture_path(&["consumer", "initial-existing-relation.json"]),
    );

    let lines = json_lines(&output);
    assert_apply_failure(&output, &lines, "ONTOLOGY_RELATION_REJECTED");
    assert_unchanged_record(&lines, "Original title", "g2");
}

#[test]
fn cardinality_violation_is_rejected_without_side_effects() {
    let fixture = TempFixture::new("cardinality");
    let mut declaration = read_json(&fixture_path(&["catalog-guidance.json"]));
    declaration["ontology"]["execution"]["relations"][0]["forward"]["max"] = serde_json::json!(0);
    let manifest = fixture.write_json("zero-cardinality-guidance.json", &declaration);
    let output = run_example(
        &manifest,
        &fixture_path(&["activate.graph.json"]),
        &fixture_path(&["legal-session.jsonl"]),
        &fixture_path(&["consumer", "initial-catalog.json"]),
    );

    let lines = json_lines(&output);
    assert_apply_failure(&output, &lines, "ONTOLOGY_RELATION_REJECTED");
    assert_unchanged_record(&lines, "Original title", "");
}

#[test]
fn duplicate_activation_preserves_the_first_commit() {
    let output = run_example(
        &fixture_path(&["catalog-guidance.json"]),
        &fixture_path(&["activate.graph.json"]),
        &fixture_path(&["consumer", "duplicate-activation.jsonl"]),
        &fixture_path(&["consumer", "initial-catalog.json"]),
    );

    let lines = json_lines(&output);
    assert!(
        !output.status.success(),
        "duplicate activation unexpectedly succeeded with exit 0"
    );

    let first = command_line(&lines, "apply", 0);
    assert_eq!(first["ok"], true);
    assert_eq!(first["result"]["record"]["state"], "active");
    assert_eq!(first["result"]["record"]["data"]["title"], "First title");

    let second = command_line(&lines, "apply", 1);
    assert_eq!(second["ok"], false);
    let error = second["error"].as_str().expect("second apply error");
    assert!(
        error.contains("ONTOLOGY_TRANSITION_REJECTED"),
        "expected transition rejection, got {error}"
    );

    let query = command_line(&lines, "query", 2);
    assert_eq!(query["ok"], true);
    assert_eq!(query["record"]["state"], "active");
    assert_eq!(query["record"]["data"]["title"], "First title");
}

#[test]
fn multiple_records_may_share_one_group() {
    let output = run_example(
        &fixture_path(&["catalog-guidance.json"]),
        &fixture_path(&["activate.graph.json"]),
        &fixture_path(&["consumer", "legal-multi-record.jsonl"]),
        &fixture_path(&["consumer", "initial-multi.json"]),
    );

    assert!(
        output.status.success(),
        "multi-record apply failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let lines = json_lines(&output);
    for (index, record_id) in ["r1", "r2"].iter().enumerate() {
        let query = command_line(&lines, "query", index + 2);
        assert_eq!(query["ok"], true);
        assert_eq!(query["record"]["id"], *record_id);
        assert_eq!(query["record"]["state"], "active");
        let relations = query["relations"].as_array().expect("query relations");
        assert_eq!(relations.len(), 1);
        assert_eq!(relations[0]["to"]["id"], "g1");
    }
}

#[test]
fn unknown_gate_fails_before_business_execution() {
    let fixture = TempFixture::new("unknown-gate");
    let mut declaration = read_json(&fixture_path(&["catalog-guidance.json"]));
    declaration["ontology"]["clauses"][0]["governance"]["check"] = "ontology.object@99".into();
    let manifest = fixture.write_json("unknown-gate-guidance.json", &declaration);
    let output = run_example(
        &manifest,
        &fixture_path(&["activate.graph.json"]),
        &fixture_path(&["legal-session.jsonl"]),
        &fixture_path(&["consumer", "initial-catalog.json"]),
    );

    assert!(
        !output.status.success(),
        "unknown gate unexpectedly started business execution"
    );
    assert!(
        output.stdout.is_empty(),
        "business output produced despite unknown gate: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("ONTOLOGY_GATE_UNRESOLVED"),
        "expected gate resolution failure, got: {stderr}"
    );
}

#[test]
fn graph_bypass_fails_before_business_execution() {
    let fixture = TempFixture::new("graph-bypass");
    let mut graph = read_json(&fixture_path(&["activate.graph.json"]));
    graph["nodes"]
        .as_array_mut()
        .expect("graph nodes")
        .iter_mut()
        .find(|node| node["id"] == "commit")
        .expect("commit node")["operator"] = serde_json::json!("catalog.prepare");
    let graph_path = fixture.write_json("bypass.graph.json", &graph);
    let output = run_example(
        &fixture_path(&["catalog-guidance.json"]),
        &graph_path,
        &fixture_path(&["legal-session.jsonl"]),
        &fixture_path(&["consumer", "initial-catalog.json"]),
    );

    assert!(
        !output.status.success(),
        "graph bypass unexpectedly started business execution"
    );
    assert!(
        output.stdout.is_empty(),
        "business output produced despite graph bypass: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("ONTOLOGY_GRAPH_BYPASS"),
        "expected graph bypass failure, got: {stderr}"
    );
}
