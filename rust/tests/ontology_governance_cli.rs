use jsonschema::validator_for;
use serde_json::{json, Value};
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
            .join(format!(
                "appsdk-ontology-governance-{}-{}-{}",
                test_name,
                std::process::id(),
                nanos
            ));
        fs::create_dir_all(base.join("appsdk-home")).unwrap();
        Self { base }
    }

    fn project(&self) -> PathBuf {
        self.base.join("project")
    }

    fn prepare_project(&self, contract: &Value) -> PathBuf {
        let project = self.project();
        copy_dir(&fixture_root(), &project);
        fs::write(
            project.join(".appsdk/skills/ontology-governance/appsdk-guidance.json"),
            serde_json::to_vec_pretty(contract).unwrap(),
        )
        .unwrap();
        project
    }

    fn compile(&self, project: &Path) -> Output {
        Command::new(binary())
            .args(["guide", "compile"])
            .arg(project)
            .env("APPSDK_HOME", self.base.join("appsdk-home"))
            .env("GIT_CEILING_DIRECTORIES", &self.base)
            .output()
            .unwrap()
    }

    fn compiled(&self, project: &Path) -> Value {
        read_json(&project.join(".appsdk/guidance/compiled.json"))
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.base);
    }
}

fn fixture_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/ontology-governance")
}

fn shared_ontology() -> Value {
    let shared = read_json(
        &PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../dagpipe/tests/fixtures/ontology-governance/catalog-guidance.json"),
    );
    shared
        .get("ontology")
        .cloned()
        .unwrap_or_else(|| panic!("shared catalog declaration must contain ontology"))
}

fn execution_contract() -> Value {
    let mut envelope = read_json(&fixture_root().join("envelope.json"));
    envelope["ontology"] = shared_ontology();
    envelope
}

fn read_json(path: &Path) -> Value {
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

fn copy_dir(source: &Path, target: &Path) {
    fs::create_dir_all(target).unwrap();
    for entry in fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        let destination = target.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &destination);
        } else {
            fs::copy(entry.path(), destination).unwrap();
        }
    }
}

fn assert_success(output: &Output) {
    assert!(
        output.status.success(),
        "appsdk guide compile failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn assert_rejected(output: &Output, error_code: &str) {
    assert!(
        !output.status.success(),
        "appsdk guide compile unexpectedly succeeded\nstdout:\n{}",
        String::from_utf8_lossy(&output.stdout)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains(error_code),
        "expected {error_code}, got:\n{stderr}"
    );
}

fn without_execution(contract: &mut Value) {
    contract["ontology"]
        .as_object_mut()
        .unwrap()
        .remove("execution");
}

#[test]
fn guidance_execution_schema_and_compiler_use_the_shared_declaration() {
    let scratch = Scratch::new("execution-shared");
    let contract = execution_contract();
    let schema = read_json(
        &PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../contracts/guidance/guidance-manifest.schema.json"),
    );
    assert!(
        validator_for(&schema).unwrap().is_valid(&contract),
        "schema and compiler must accept the same shared execution declaration"
    );

    let project = scratch.prepare_project(&contract);
    let output = scratch.compile(&project);
    assert_success(&output);
    let compiled = scratch.compiled(&project);
    assert_eq!(compiled["enforcement"], "advisory");
    assert_eq!(
        compiled["ontology"]["execution"],
        contract["ontology"]["execution"]
    );
    assert_eq!(
        compiled["ontology"]["objects"],
        contract["ontology"]["objects"]
    );
    assert_eq!(
        compiled["ontology"]["relations"],
        contract["ontology"]["relations"]
    );
    assert_eq!(compiled["rules"], contract["rules"]);
    assert_eq!(compiled["workflows"], contract["workflows"]);
}

#[test]
fn guidance_execution_rejects_unresolved_references_before_writing() {
    let scratch = Scratch::new("execution-reference");
    let mut contract = execution_contract();
    contract["ontology"]["execution"]["operations"][0]["clauses"]
        .as_array_mut()
        .unwrap()
        .push(json!("missing-clause"));
    let project = scratch.prepare_project(&contract);

    let output = scratch.compile(&project);
    assert_rejected(&output, "ONTOLOGY_REFERENCE_UNRESOLVED");
    assert!(!project.join(".appsdk/guidance/compiled.json").exists());
}

#[test]
fn guidance_execution_rejects_invalid_state_bindings() {
    let scratch = Scratch::new("execution-state");
    let mut contract = execution_contract();
    contract["ontology"]["execution"]["objects"][0]["states"] = json!(["active"]);
    let project = scratch.prepare_project(&contract);

    let output = scratch.compile(&project);
    assert_rejected(&output, "ONTOLOGY_REFERENCE_UNRESOLVED");
    assert!(!project.join(".appsdk/guidance/compiled.json").exists());
}

#[test]
fn guidance_execution_rejects_duplicate_operation_dispatch() {
    let scratch = Scratch::new("execution-duplicate");
    let mut contract = execution_contract();
    let duplicate = contract["ontology"]["execution"]["operations"][0].clone();
    contract["ontology"]["execution"]["operations"]
        .as_array_mut()
        .unwrap()
        .push(duplicate);
    let project = scratch.prepare_project(&contract);

    let output = scratch.compile(&project);
    assert_rejected(&output, "ONTOLOGY_DECLARATION_INVALID");
    assert!(!project.join(".appsdk/guidance/compiled.json").exists());
}

#[test]
fn guidance_without_execution_preserves_advisory_compatibility() {
    let scratch = Scratch::new("legacy-advisory");
    let contract = read_json(&fixture_root().join("legacy-envelope.json"));
    assert!(contract.get("ontology").is_none());
    let project = scratch.prepare_project(&contract);

    let output = scratch.compile(&project);
    assert_success(&output);
    let compiled = scratch.compiled(&project);
    assert_eq!(compiled["enforcement"], "advisory");
    assert!(compiled.get("ontology").is_none());
    assert_eq!(compiled["rules"], contract["rules"]);
}

#[test]
fn guidance_without_execution_preserves_existing_ontology_clauses() {
    let scratch = Scratch::new("legacy-ontology");
    let mut contract = execution_contract();
    without_execution(&mut contract);
    let project = scratch.prepare_project(&contract);

    let output = scratch.compile(&project);
    assert_success(&output);
    let compiled = scratch.compiled(&project);
    assert!(compiled["ontology"].get("execution").is_none());
    assert_eq!(
        compiled["ontology"]["clauses"],
        contract["ontology"]["clauses"]
    );
}
