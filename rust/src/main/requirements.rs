use super::*;
use std::collections::BTreeMap;

/// Session-authorized user requirement lock.
///
/// Single owner for reading, validating and committing `.appsdk/requirements.json`.
/// The session authorization text is the normative basis; this module only checks
/// the current contract shape, versions, atomic commit and consumption binding.
pub(super) fn requirements_cli(args: &mut std::iter::Peekable<std::vec::IntoIter<String>>) {
    if args.peek().is_some_and(|value| is_help(value)) {
        print_requirements_usage();
        return;
    }
    let command = args.next();
    let root = project_root_or_cwd(args);
    match command.as_deref() {
        Some("show") => {
            if args.next().is_some() {
                fail("USAGE: appsdk requirements <show|history|apply|verify> [project]");
            }
            requirements_show(&root);
        }
        Some("history") => {
            if args.next().is_some() {
                fail("USAGE: appsdk requirements <show|history|apply|verify> [project]");
            }
            requirements_history(&root);
        }
        Some("verify") => {
            if args.next().is_some() {
                fail("USAGE: appsdk requirements <show|history|apply|verify> [project]");
            }
            requirements_verify(&root);
        }
        Some("apply") => {
            if args.next().as_deref() != Some("--input") {
                fail("USAGE: appsdk requirements apply [project] --input <json>");
            }
            let input = args.next().unwrap_or_else(|| {
                fail("USAGE: appsdk requirements apply [project] --input <json>")
            });
            if args.next().is_some() {
                fail("USAGE: appsdk requirements apply [project] --input <json>");
            }
            requirements_apply(&root, &input);
        }
        Some(other) => {
            print_requirements_usage();
            fail(format!("UNKNOWN_REQUIREMENTS_COMMAND:{other}"));
        }
        None => {
            print_requirements_usage();
            fail("USAGE: appsdk requirements <show|history|apply|verify> [project]");
        }
    }
}

/// Returns the validated ledger when `.appsdk/requirements.json` exists.
///
/// `None` means the project has not established a requirement lock. An existing
/// but structurally invalid ledger fails as `REQUIREMENTS_INVALID`.
pub(super) fn read_requirements_if_present(root: &Path) -> Option<Value> {
    ledger_state(root).map(|(ledger, _)| ledger)
}

/// Checks the goal's integer `requirements_version` binding against the ledger.
pub(super) fn assert_requirements_current(root: &Path, goal: &Value) {
    let bound = goal.get("requirements_version").and_then(Value::as_u64);
    match ledger_state(root) {
        Some((ledger, _)) => {
            if bound != ledger.get("version").and_then(Value::as_u64) {
                fail("REQUIREMENTS_VERSION_STALE");
            }
        }
        None => {
            if bound.unwrap_or(0) != 0 {
                fail("REQUIREMENTS_MISSING");
            }
        }
    }
}

/// Review material: every active requirement plus the complete history and source.
pub(super) fn requirement_review_material(root: &Path) -> Value {
    match ledger_state(root) {
        Some((ledger, state)) => serde_json::json!({
            "status": "locked",
            "source": ".appsdk/requirements.json",
            "project_id": ledger.get("project_id").cloned().unwrap_or(Value::Null),
            "version": ledger.get("version").and_then(Value::as_u64).unwrap_or(0),
            "requirements": state.current(),
            "history": ledger.get("changes").cloned().unwrap_or_else(|| Value::Array(Vec::new())),
        }),
        None => serde_json::json!({
            "status": "not_established",
            "version": 0,
            "requirements": [],
            "history": [],
        }),
    }
}

fn print_requirements_usage() {
    println!("Usage: appsdk requirements <show|history|apply|verify> [project]");
    println!("       appsdk requirements apply [project] --input <json>");
}

fn requirements_show(root: &Path) {
    let output = match ledger_state(root) {
        Some((ledger, state)) => serde_json::json!({
            "status": "locked",
            "version": ledger.get("version").and_then(Value::as_u64).unwrap_or(0),
            "requirements": state.current(),
        }),
        None => serde_json::json!({
            "status": "not_established",
            "version": 0,
            "requirements": [],
        }),
    };
    print_requirements_value(&output);
}

fn requirements_history(root: &Path) {
    let output = match ledger_state(root) {
        Some((ledger, _)) => ledger,
        None => serde_json::json!({
            "status": "not_established",
            "version": 0,
            "changes": [],
        }),
    };
    print_requirements_value(&output);
}

fn requirements_verify(root: &Path) {
    let output = match ledger_state(root) {
        Some((ledger, _)) => serde_json::json!({
            "status": "locked",
            "version": ledger.get("version").and_then(Value::as_u64).unwrap_or(0),
        }),
        None => serde_json::json!({
            "status": "not_established",
            "version": 0,
        }),
    };
    print_requirements_value(&output);
}

fn print_requirements_value(value: &Value) {
    println!(
        "{}",
        serde_json::to_string_pretty(value).unwrap_or_else(|_| fail("REQUIREMENTS_OUTPUT_FAILED"))
    );
}

fn requirements_apply(root: &Path, input_path: &str) {
    assert_project_root_safe(root);
    let input_file = producer_input_path(root, input_path);
    let request: Value = serde_json::from_str(
        &fs::read_to_string(input_file).unwrap_or_else(|_| fail("PRODUCER_INPUT_READ_FAILED")),
    )
    .unwrap_or_else(|_| fail("INVALID_PRODUCER_INPUT"));
    if !request.is_object() {
        fail("INVALID_PRODUCER_INPUT");
    }
    let project_id = read_project(root)
        .get("project_id")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| fail("INVALID_PROJECT_ID"))
        .to_string();
    if request.get("project_id").and_then(Value::as_str) != Some(project_id.as_str()) {
        fail("REQUIREMENTS_PROJECT_MISMATCH");
    }
    assert_request(&request);
    let request_id = request
        .get("request_id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let requirement_id = request
        .get("requirement_id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let operation = request
        .get("operation")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let base_version = request
        .get("base_version")
        .and_then(Value::as_u64)
        .unwrap_or_else(|| fail("REQUIREMENTS_INVALID"));

    if operation == "cancel" {
        // A cancel never writes: the caller withdraws the request before commit.
        if request.get("text").is_some() {
            fail("REQUIREMENTS_INVALID");
        }
        print_requirements_value(&serde_json::json!({
            "ok": true,
            "operation": "cancel",
            "cancelled": true,
            "request_id": request_id,
            "requirement_id": requirement_id,
        }));
        return;
    }
    let _lock = reset_transaction_acquire_lock(root).unwrap_or_else(|error| fail(error));
    let (mut ledger, state) = match ledger_state(root) {
        Some((ledger, state)) => (ledger, state),
        None => (fresh_ledger(&project_id), LedgerState::default()),
    };
    if let Some(previous) = state.request(&request_id) {
        if previous.get("request") == Some(&request) {
            print_requirements_value(&receipt_for(previous, true));
            return;
        }
        fail("REQUIREMENTS_REQUEST_CONFLICT");
    }
    let previous_entry = state.latest(&requirement_id).cloned();
    let requirement_version = match operation.as_str() {
        "create" => {
            if previous_entry.is_some() {
                fail("REQUIREMENTS_ALREADY_EXISTS");
            }
            if base_version != 0 {
                fail("REQUIREMENTS_BASE_VERSION_CONFLICT");
            }
            1
        }
        "replace" => {
            let entry = previous_entry
                .as_ref()
                .unwrap_or_else(|| fail("REQUIREMENTS_NOT_FOUND"));
            if entry.get("version").and_then(Value::as_u64) != Some(base_version) {
                fail("REQUIREMENTS_BASE_VERSION_CONFLICT");
            }
            base_version + 1
        }
        "revoke" => {
            let entry = previous_entry
                .as_ref()
                .filter(|entry| entry.get("status").and_then(Value::as_str) == Some("active"))
                .unwrap_or_else(|| fail("REQUIREMENTS_NOT_FOUND"));
            if entry.get("version").and_then(Value::as_u64) != Some(base_version) {
                fail("REQUIREMENTS_BASE_VERSION_CONFLICT");
            }
            base_version + 1
        }
        _ => fail("REQUIREMENTS_OPERATION_INVALID"),
    };
    let status = if operation == "revoke" {
        "revoked"
    } else {
        "active"
    };
    let result = request_result(&request, requirement_version, previous_entry.as_ref());
    let submitted_version = ledger.get("version").and_then(Value::as_u64).unwrap_or(0) + 1;
    let change = serde_json::json!({
        "version": submitted_version,
        "request": request,
        "result": result,
    });
    ledger_write(root, &mut ledger, change);
    print_requirements_value(&serde_json::json!({
        "ok": true,
        "project_id": project_id,
        "request_id": request_id,
        "requirement_id": requirement_id,
        "operation": operation,
        "base_version": base_version,
        "requirement_version": requirement_version,
        "version": submitted_version,
        "status": status,
        "reused": false,
    }));
}

fn request_result(request: &Value, version: u64, previous: Option<&Value>) -> Value {
    if request["operation"] == "revoke" {
        serde_json::json!({
            "requirement_id": request["requirement_id"],
            "version": version,
            "status": "revoked",
            "previous_text": previous.and_then(|entry| entry.get("text")),
        })
    } else {
        serde_json::json!({
            "requirement_id": request["requirement_id"],
            "version": version,
            "status": "active",
            "text": request["text"],
            "authorization": request["authorization"],
        })
    }
}

fn fresh_ledger(project_id: &str) -> Value {
    serde_json::json!({
        "schema_version": 1,
        "project_id": project_id,
        "version": 0,
        "changes": [],
    })
}

fn ledger_write(root: &Path, ledger: &mut Value, change: Value) {
    let changes = ledger
        .get_mut("changes")
        .and_then(Value::as_array_mut)
        .unwrap_or_else(|| fail("REQUIREMENTS_INVALID"));
    changes.push(change);
    let version = changes.len() as u64;
    ledger
        .as_object_mut()
        .unwrap_or_else(|| fail("REQUIREMENTS_INVALID"))
        .insert("version".into(), serde_json::json!(version));
    let target = ledger_file(root);
    assert_no_symlink_components(root, &target, "requirements");
    atomic_write_json(&target, ledger, "REQUIREMENTS_WRITE_FAILED");
}

fn receipt_for(change: &Value, reused: bool) -> Value {
    let request = change.get("request").cloned().unwrap_or(Value::Null);
    let result = change.get("result").cloned().unwrap_or(Value::Null);
    serde_json::json!({
        "ok": true,
        "project_id": request.get("project_id").cloned().unwrap_or(Value::Null),
        "request_id": request.get("request_id").cloned().unwrap_or(Value::Null),
        "requirement_id": request.get("requirement_id").cloned().unwrap_or(Value::Null),
        "operation": request.get("operation").cloned().unwrap_or(Value::Null),
        "base_version": request.get("base_version").cloned().unwrap_or(Value::Null),
        "requirement_version": result.get("version").cloned().unwrap_or(Value::Null),
        "version": change.get("version").cloned().unwrap_or(Value::Null),
        "status": result.get("status").cloned().unwrap_or(Value::Null),
        "reused": reused,
    })
}

fn ledger_file(root: &Path) -> PathBuf {
    root.join(".appsdk").join("requirements.json")
}

fn ledger_state(root: &Path) -> Option<(Value, LedgerState)> {
    let file = ledger_file(root);
    let metadata = match fs::symlink_metadata(&file) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return None,
        Err(_) => fail("REQUIREMENTS_INVALID"),
    };
    if metadata.file_type().is_symlink() {
        fail("REQUIREMENTS_INVALID");
    }
    assert_no_symlink_components(root, &file, "requirements");
    let text = fs::read_to_string(&file).unwrap_or_else(|_| fail("REQUIREMENTS_INVALID"));
    let ledger: Value =
        serde_json::from_str(&text).unwrap_or_else(|_| fail("REQUIREMENTS_INVALID"));
    let state = validate_ledger(&ledger);
    if ledger.get("project_id") != read_project(root).get("project_id") {
        fail("REQUIREMENTS_PROJECT_MISMATCH");
    }
    Some((ledger, state))
}

#[derive(Default)]
struct LedgerState {
    entries: Vec<Value>,
    requests: BTreeMap<String, usize>,
}

impl LedgerState {
    /// Latest recorded entry for a requirement, regardless of status.
    fn latest(&self, requirement_id: &str) -> Option<&Value> {
        self.entries
            .iter()
            .rev()
            .find(|change| {
                change
                    .pointer("/result/requirement_id")
                    .and_then(Value::as_str)
                    == Some(requirement_id)
            })
            .and_then(|change| change.get("result"))
    }

    fn request(&self, request_id: &str) -> Option<&Value> {
        self.requests
            .get(request_id)
            .map(|index| &self.entries[*index])
    }

    fn current(&self) -> Vec<Value> {
        let mut latest: BTreeMap<&str, &Value> = BTreeMap::new();
        for change in &self.entries {
            if let Some(result) = change.get("result") {
                if let Some(requirement_id) = result.get("requirement_id").and_then(Value::as_str) {
                    latest.insert(requirement_id, result);
                }
            }
        }
        latest
            .values()
            .filter(|result| result.get("status").and_then(Value::as_str) == Some("active"))
            .map(|result| (*result).clone())
            .collect()
    }
}

/// Validates the ledger chain and derives current entries from history.
fn validate_ledger(ledger: &Value) -> LedgerState {
    if !ledger.is_object() {
        fail("REQUIREMENTS_INVALID");
    }
    let project_id = ledger
        .get("project_id")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| fail("REQUIREMENTS_INVALID"))
        .to_string();
    let changes = ledger
        .get("changes")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_else(|| fail("REQUIREMENTS_INVALID"));
    if ledger.get("schema_version").and_then(Value::as_u64) != Some(1)
        || ledger.get("version").and_then(Value::as_u64) != Some(changes.len() as u64)
    {
        fail("REQUIREMENTS_INVALID");
    }
    let mut state = LedgerState::default();
    for (index, change) in changes.iter().enumerate() {
        if change.get("version").and_then(Value::as_u64) != Some(index as u64 + 1) {
            fail("REQUIREMENTS_INVALID");
        }
        let request = change
            .get("request")
            .cloned()
            .unwrap_or_else(|| fail("REQUIREMENTS_INVALID"));
        assert_request(&request);
        if request.get("project_id").and_then(Value::as_str) != Some(project_id.as_str()) {
            fail("REQUIREMENTS_INVALID");
        }
        let request_id = request
            .get("request_id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        if state.requests.contains_key(&request_id) {
            fail("REQUIREMENTS_INVALID");
        }
        let requirement_id = request
            .get("requirement_id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let expected_base = match request.get("operation").and_then(Value::as_str) {
            Some("create") => {
                if state.latest(&requirement_id).is_some() {
                    fail("REQUIREMENTS_INVALID");
                }
                0
            }
            Some("replace") => state
                .latest(&requirement_id)
                .and_then(|entry| entry.get("version").and_then(Value::as_u64))
                .unwrap_or_else(|| fail("REQUIREMENTS_INVALID")),
            Some("revoke") => state
                .latest(&requirement_id)
                .filter(|entry| entry.get("status").and_then(Value::as_str) == Some("active"))
                .and_then(|entry| entry.get("version").and_then(Value::as_u64))
                .unwrap_or_else(|| fail("REQUIREMENTS_INVALID")),
            _ => fail("REQUIREMENTS_INVALID"),
        };
        if request.get("base_version").and_then(Value::as_u64) != Some(expected_base) {
            fail("REQUIREMENTS_INVALID");
        }
        let expected = request_result(&request, expected_base + 1, state.latest(&requirement_id));
        if change.get("result") != Some(&expected) {
            fail("REQUIREMENTS_INVALID");
        }
        state.requests.insert(request_id, state.entries.len());
        state.entries.push(change.clone());
    }
    state
}

fn assert_request_fields(request: &Value) {
    for field in ["project_id", "request_id", "requirement_id"] {
        if request
            .get(field)
            .and_then(Value::as_str)
            .map(|value| value.is_empty())
            .unwrap_or(true)
        {
            fail("REQUIREMENTS_INVALID");
        }
    }
    if request
        .get("base_version")
        .and_then(Value::as_u64)
        .is_none()
    {
        fail("REQUIREMENTS_INVALID");
    }
    match request.get("operation").and_then(Value::as_str) {
        Some("create" | "replace" | "revoke" | "cancel") => {}
        _ => fail("REQUIREMENTS_INVALID"),
    }
}

/// Shape of a committable request: create/replace carry the authorized text,
/// revoke carries only the authorization that authorizes the撤销.
fn assert_request(request: &Value) {
    assert_request_fields(request);
    match request.get("operation").and_then(Value::as_str) {
        Some("create" | "replace") => {
            if !request
                .get("text")
                .and_then(Value::as_str)
                .map(|value| !value.is_empty())
                .unwrap_or(false)
                || !request
                    .get("authorization")
                    .map(authorization_valid)
                    .unwrap_or(false)
            {
                fail("REQUIREMENTS_INVALID");
            }
        }
        Some("revoke" | "cancel") => {
            if request.get("text").is_some()
                || !request
                    .get("authorization")
                    .map(authorization_valid)
                    .unwrap_or(false)
            {
                fail("REQUIREMENTS_INVALID");
            }
        }
        _ => fail("REQUIREMENTS_INVALID"),
    }
}

/// The user's real session authorization. AppSDK checks only presence and role;
/// whether the text covers the requested change is verified by agent/reviewer.
fn authorization_valid(authorization: &Value) -> bool {
    authorization
        .as_object()
        .map(|object| {
            object.len() == 3
                && object.get("role").and_then(Value::as_str) == Some("user")
                && object
                    .get("source")
                    .and_then(Value::as_str)
                    .map(|value| !value.is_empty())
                    .unwrap_or(false)
                && object
                    .get("original_text")
                    .and_then(Value::as_str)
                    .map(|value| !value.is_empty())
                    .unwrap_or(false)
        })
        .unwrap_or(false)
}
