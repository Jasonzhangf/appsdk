use pipeline_runtime::governance::{
    bind_operation, CommitOwner, CompiledOperation, GovernanceSnapshot, Invocation,
    OperationFailure, OperationRequest, Subject,
};
use pipeline_runtime::{
    parse_graph_json, CompileError, Identity, Operator, OperatorContext, Registry, ValueType,
};
use serde_json::{json, Value};
use std::{
    collections::BTreeSet,
    fmt, fs,
    io::{self, BufRead},
    path::{Path, PathBuf},
    process,
    sync::{Arc, Mutex, MutexGuard},
};

#[derive(Debug)]
struct AppError(String);

impl fmt::Display for AppError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for AppError {}

type AppResult<T> = Result<T, AppError>;

struct CatalogPrepare;

impl Operator for CatalogPrepare {
    fn name(&self) -> &'static str {
        "catalog.prepare"
    }

    fn version(&self) -> &'static str {
        "1"
    }

    fn input_type(&self) -> ValueType {
        ValueType::Object
    }

    fn output_type(&self) -> ValueType {
        ValueType::Object
    }

    fn execute(&self, input: Value, _: &OperatorContext) -> Result<Value, String> {
        Ok(json!({
            "title": input.get("title").and_then(Value::as_str).unwrap_or_default(),
            "group": input.get("group").and_then(Value::as_str).unwrap_or_default(),
        }))
    }
}

struct CatalogReceipt;

impl Operator for CatalogReceipt {
    fn name(&self) -> &'static str {
        "catalog.receipt"
    }

    fn version(&self) -> &'static str {
        "1"
    }

    fn input_type(&self) -> ValueType {
        ValueType::Object
    }

    fn output_type(&self) -> ValueType {
        ValueType::Object
    }

    fn execute(&self, input: Value, _: &OperatorContext) -> Result<Value, String> {
        Ok(input)
    }
}

struct RecordContract;

impl Operator for RecordContract {
    fn name(&self) -> &'static str {
        "catalog.record_contract"
    }

    fn version(&self) -> &'static str {
        "1"
    }

    fn input_type(&self) -> ValueType {
        ValueType::Object
    }

    fn output_type(&self) -> ValueType {
        ValueType::Boolean
    }

    fn execute(&self, input: Value, _: &OperatorContext) -> Result<Value, String> {
        let title = input["data"]["title"].as_str().unwrap_or_default();
        Ok(json!(!title.trim().is_empty()))
    }
}

struct GroupContract;

impl Operator for GroupContract {
    fn name(&self) -> &'static str {
        "catalog.group_contract"
    }

    fn version(&self) -> &'static str {
        "1"
    }

    fn input_type(&self) -> ValueType {
        ValueType::Object
    }

    fn output_type(&self) -> ValueType {
        ValueType::Boolean
    }

    fn execute(&self, input: Value, _: &OperatorContext) -> Result<Value, String> {
        let name = input["data"]["name"].as_str().unwrap_or_default();
        Ok(json!(!name.trim().is_empty()))
    }
}

fn register_catalog_operators(
    registry: &mut pipeline_runtime::Registry,
) -> Result<(), pipeline_runtime::CompileError> {
    registry.register(CatalogPrepare)?;
    registry.register(CatalogReceipt)?;
    registry.register(RecordContract)?;
    registry.register(GroupContract)?;
    Ok(())
}

struct CatalogState {
    objects: Vec<Value>,
    relations: Vec<Value>,
}

impl CatalogState {
    fn from_json(value: Value) -> AppResult<Self> {
        let objects = value
            .get("objects")
            .and_then(Value::as_array)
            .ok_or_else(|| AppError("initial catalog objects must be an array".into()))?
            .clone();
        let relations = value
            .get("relations")
            .and_then(Value::as_array)
            .ok_or_else(|| AppError("initial catalog relations must be an array".into()))?
            .clone();
        Ok(Self { objects, relations })
    }

    fn default_catalog() -> Self {
        Self {
            objects: vec![
                json!({
                    "type": "record",
                    "scope": "demo",
                    "id": "r1",
                    "state": "draft",
                    "data": {"title": "Original title"}
                }),
                json!({
                    "type": "group",
                    "scope": "demo",
                    "id": "g1",
                    "state": "open",
                    "data": {"name": "Primary group"}
                }),
            ],
            relations: Vec::new(),
        }
    }

    fn query(&self, record_id: &str) -> Value {
        let record = self
            .objects
            .iter()
            .find(|object| object_is(object, "record", record_id))
            .cloned()
            .unwrap_or(Value::Null);
        let subject = Subject {
            object_type: "record".into(),
            scope: record["scope"].as_str().unwrap_or("demo").into(),
            id: record_id.into(),
        };
        let relations = self
            .relations
            .iter()
            .filter(|relation| relation_is_from(relation, &subject))
            .cloned()
            .collect::<Vec<_>>();
        json!({
            "record": record,
            "relations": relations,
        })
    }
}

fn object_is(object: &Value, object_type: &str, id: &str) -> bool {
    object["type"] == object_type && object["id"] == id
}

fn object_matches_subject(object: &Value, subject: &Subject) -> bool {
    object["type"] == subject.object_type
        && object["scope"] == subject.scope
        && object["id"] == subject.id
}

fn relation_is_from(relation: &Value, subject: &Subject) -> bool {
    relation["type"] == "record-group"
        && relation["from"]["type"] == subject.object_type
        && relation["from"]["scope"] == subject.scope
        && relation["from"]["id"] == subject.id
}

struct CatalogOwner {
    state: Mutex<CatalogState>,
}

impl CatalogOwner {
    fn lock(&self) -> MutexGuard<'_, CatalogState> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

impl CommitOwner for CatalogOwner {
    fn apply(
        &self,
        invocation: &Invocation,
        prepared: Value,
        validate: &mut dyn FnMut(
            &GovernanceSnapshot,
            &GovernanceSnapshot,
        )
            -> Result<(), pipeline_runtime::governance::GovernanceError>,
    ) -> Result<Value, pipeline_runtime::governance::GovernanceError> {
        let subject = invocation.subject();
        let mut state = self.lock();
        let current = GovernanceSnapshot {
            objects: state.objects.clone(),
            relations: state.relations.clone(),
        };

        let title = prepared["title"].as_str().unwrap_or_default();
        let target_group = prepared["group"].as_str().unwrap_or_default();
        let mut candidate_objects = current.objects.clone();
        for object in &mut candidate_objects {
            if object_matches_subject(object, subject) {
                object["state"] = json!("active");
                object["data"]["title"] = json!(title);
            }
        }

        let mut candidate_relations = current.relations.clone();
        candidate_relations.retain(|relation| !relation_is_from(relation, subject));
        candidate_relations.push(json!({
            "type": "record-group",
            "from": {
                "type": subject.object_type,
                "scope": subject.scope,
                "id": subject.id,
            },
            "to": {
                "type": "group",
                "scope": subject.scope,
                "id": target_group,
            },
        }));
        let candidate = GovernanceSnapshot {
            objects: candidate_objects.clone(),
            relations: candidate_relations.clone(),
        };

        validate(&current, &candidate)?;

        state.objects = candidate_objects;
        state.relations = candidate_relations;
        let record = state
            .objects
            .iter()
            .find(|object| object_matches_subject(object, subject))
            .cloned()
            .unwrap_or(Value::Null);
        let relations = state
            .relations
            .iter()
            .filter(|relation| relation_is_from(relation, subject))
            .cloned()
            .collect::<Vec<_>>();
        Ok(json!({
            "record": record,
            "relations": relations,
        }))
    }
}

struct CliPaths {
    manifest: PathBuf,
    graph: PathBuf,
    script: PathBuf,
    initial: Option<PathBuf>,
}

fn parse_args() -> AppResult<CliPaths> {
    let mut arguments = std::env::args().skip(1);
    let mut manifest = None;
    let mut graph = None;
    let mut script = None;
    let mut initial = None;
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--manifest" => manifest = Some(next_path(&mut arguments, "--manifest")?),
            "--graph" => graph = Some(next_path(&mut arguments, "--graph")?),
            "--script" => script = Some(next_path(&mut arguments, "--script")?),
            "--initial" => initial = Some(next_path(&mut arguments, "--initial")?),
            other => return Err(AppError(format!("unknown argument `{other}`"))),
        }
    }
    Ok(CliPaths {
        manifest: manifest.ok_or_else(|| AppError("--manifest is required".into()))?,
        graph: graph.ok_or_else(|| AppError("--graph is required".into()))?,
        script: script.ok_or_else(|| AppError("--script is required".into()))?,
        initial,
    })
}

fn next_path(arguments: &mut impl Iterator<Item = String>, option: &str) -> AppResult<PathBuf> {
    arguments
        .next()
        .map(PathBuf::from)
        .ok_or_else(|| AppError(format!("{option} requires a path")))
}

fn read_json(path: &Path) -> AppResult<Value> {
    let source = fs::read_to_string(path)
        .map_err(|error| AppError(format!("read {}: {error}", path.display())))?;
    serde_json::from_str(&source).map_err(|error| AppError(format!("invalid JSON: {error}")))
}

fn print_json(value: &Value) -> AppResult<()> {
    let text = serde_json::to_string(value).map_err(|error| AppError(error.to_string()))?;
    println!("{text}");
    Ok(())
}

fn failure_message(failure: OperationFailure) -> String {
    match failure {
        OperationFailure::Admission(error) => error.to_string(),
        OperationFailure::Execution(error) => error.to_string(),
    }
}

struct CatalogSession {
    owner: Arc<CatalogOwner>,
    operation: CompiledOperation,
    graph_id: String,
    graph_version: String,
}

fn bind_catalog(
    manifest_path: &Path,
    graph_path: &Path,
    initial_path: Option<&Path>,
) -> AppResult<CatalogSession> {
    let manifest = read_json(manifest_path)?;
    let ontology = manifest
        .get("ontology")
        .ok_or_else(|| AppError("manifest has no ontology object".into()))?;
    let graph_source = fs::read_to_string(graph_path)
        .map_err(|error| AppError(format!("read {}: {error}", graph_path.display())))?;
    let graph = parse_graph_json(&graph_source)
        .map_err(|error| AppError(format!("invalid graph JSON: {error}")))?;
    let graph_id = graph.id.clone();
    let graph_version = graph.version.clone();

    let initial = match initial_path {
        Some(path) => CatalogState::from_json(read_json(path)?)?,
        None => CatalogState::default_catalog(),
    };
    let owner = Arc::new(CatalogOwner {
        state: Mutex::new(initial),
    });
    let mut registry = Registry::default();
    register_catalog_operators(&mut registry)
        .map_err(|error: CompileError| AppError(format!("catalog registration failed: {error}")))?;
    let capabilities = BTreeSet::from(["ontology.commit".into()]);
    let operation = bind_operation(
        ontology,
        "activate",
        graph,
        registry,
        owner.clone(),
        &capabilities,
    )
    .map_err(|error| AppError(format!("ontology bind failed: {error}")))?;

    Ok(CatalogSession {
        owner,
        operation,
        graph_id,
        graph_version,
    })
}

fn run() -> AppResult<i32> {
    let paths = parse_args()?;
    let session = bind_catalog(&paths.manifest, &paths.graph, paths.initial.as_deref())?;
    let owner = session.owner;
    let operation = session.operation;
    let graph_id = session.graph_id;
    let graph_version = session.graph_version;

    let script = fs::File::open(&paths.script)
        .map_err(|error| AppError(format!("read {}: {error}", paths.script.display())))?;
    let lines = io::BufReader::new(script).lines();
    let mut command_failed = false;
    let mut execution = 0_usize;
    for (index, line) in lines.enumerate() {
        let line = line.map_err(|error| AppError(format!("read script line {index}: {error}")))?;
        if line.trim().is_empty() {
            continue;
        }
        execution += 1;
        let command: Value = serde_json::from_str(&line)
            .map_err(|error| AppError(format!("invalid script line {index}: {error}")))?;
        match command.get("command").and_then(Value::as_str) {
            Some("apply") => {
                let record = command
                    .get("record")
                    .and_then(Value::as_str)
                    .ok_or_else(|| AppError(format!("script line {index} has no record")))?;
                let result = operation.call(
                    OperationRequest {
                        subject: Subject {
                            object_type: "record".into(),
                            scope: "demo".into(),
                            id: record.into(),
                        },
                        event: "activate".into(),
                        data: json!({
                            "title": command.get("title").cloned().unwrap_or_default(),
                            "group": command.get("group").cloned().unwrap_or_default(),
                        }),
                    },
                    Identity {
                        project_id: "isolated-catalog".into(),
                        graph_id: graph_id.clone(),
                        graph_version: graph_version.clone(),
                        execution_id: format!("execution-{execution}"),
                        attempt_id: format!("attempt-{execution}"),
                    },
                    &Default::default(),
                );
                match result {
                    Ok(execution_result) => {
                        let output = execution_result
                            .outputs
                            .get("receipt")
                            .ok_or_else(|| AppError("graph did not return receipt".into()))?;
                        print_json(&json!({
                            "command": "apply",
                            "ok": true,
                            "result": output.payload,
                        }))?;
                    }
                    Err(failure) => {
                        command_failed = true;
                        print_json(&json!({
                            "command": "apply",
                            "ok": false,
                            "error": failure_message(failure),
                        }))?;
                    }
                }
            }
            Some("query") => {
                let record = command
                    .get("record")
                    .and_then(Value::as_str)
                    .ok_or_else(|| AppError(format!("script line {index} has no record")))?;
                let state = owner.lock();
                let result = state.query(record);
                let ok = !result["record"].is_null();
                print_json(&json!({
                    "command": "query",
                    "ok": ok,
                    "error": if ok { Value::Null } else { json!("record not found") },
                    "record": result["record"],
                    "relations": result["relations"],
                }))?;
                if !ok {
                    command_failed = true;
                }
            }
            _ => {
                return Err(AppError(format!(
                    "script line {index} has an unknown command"
                )))
            }
        }
    }
    Ok(i32::from(command_failed))
}

fn main() {
    match run() {
        Ok(exit_code) => process::exit(exit_code),
        Err(error) => {
            eprintln!("{error}");
            process::exit(2);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Barrier;
    use std::thread;

    fn fixture_path(relative: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/ontology-governance")
            .join(relative)
    }

    #[test]
    fn concurrent_activation_serializes_validation_and_commit() {
        let session = bind_catalog(
            &fixture_path("catalog-guidance.json"),
            &fixture_path("activate.graph.json"),
            Some(&fixture_path("consumer/initial-catalog.json")),
        )
        .expect("bind catalog operation");
        let operation = Arc::new(session.operation);
        let barrier = Arc::new(Barrier::new(3));
        let mut handles = Vec::new();

        for (execution, title) in [
            ("concurrent-1", "First concurrent title"),
            ("concurrent-2", "Second concurrent title"),
        ] {
            let operation = operation.clone();
            let barrier = barrier.clone();
            let graph_id = session.graph_id.clone();
            let graph_version = session.graph_version.clone();
            handles.push(thread::spawn(move || {
                barrier.wait();
                operation
                    .call(
                        OperationRequest {
                            subject: Subject {
                                object_type: "record".into(),
                                scope: "demo".into(),
                                id: "r1".into(),
                            },
                            event: "activate".into(),
                            data: json!({"title": title, "group": "g1"}),
                        },
                        Identity {
                            project_id: "isolated-catalog".into(),
                            graph_id,
                            graph_version,
                            execution_id: execution.into(),
                            attempt_id: format!("{execution}-attempt"),
                        },
                        &Default::default(),
                    )
                    .map(|_| title)
                    .map_err(|failure| failure.to_string())
            }));
        }
        barrier.wait();

        let results = handles
            .into_iter()
            .map(|handle| handle.join().expect("concurrent worker"))
            .collect::<Vec<_>>();
        let successes = results
            .iter()
            .filter_map(|result| result.as_ref().ok())
            .collect::<Vec<_>>();
        let failures = results
            .iter()
            .filter_map(|result| result.as_ref().err())
            .collect::<Vec<_>>();

        assert_eq!(successes.len(), 1, "results: {results:?}");
        assert_eq!(failures.len(), 1, "results: {results:?}");
        assert!(
            failures[0].contains("ONTOLOGY_TRANSITION_REJECTED"),
            "unexpected concurrent failure: {}",
            failures[0]
        );

        let state = session.owner.lock();
        let queried = state.query("r1");
        assert_eq!(queried["record"]["state"], "active");
        assert_eq!(queried["record"]["data"]["title"], *successes[0]);
        assert_eq!(queried["relations"][0]["to"]["id"], "g1");
    }
}
