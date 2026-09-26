use super::*;

pub(super) fn reset_governance_internal(
    root: &Path,
    discard_legacy: bool,
    mode: ResetMode,
) -> Result<(), String> {
    if !discard_legacy {
        return Err("RESET_REQUIRES_DISCARD_LEGACY_CONFIRMATION".into());
    }
    assert_project_root_safe(root);

    let branch = Command::new("git")
        .args([
            "-C",
            root.to_str().unwrap_or(""),
            "branch",
            "--show-current",
        ])
        .output()
        .map_err(|_| "RESET_GIT_WORKTREE_REQUIRED".to_string())?;
    if !branch.status.success() {
        return Err("RESET_GIT_WORKTREE_REQUIRED".into());
    }
    let branch = String::from_utf8_lossy(&branch.stdout).trim().to_string();
    if branch.is_empty() || branch == "main" || branch == "master" {
        return Err("RESET_REQUIRES_NON_MAIN_WORKTREE".into());
    }

    let _lock = reset_transaction_acquire_lock(root)?;
    match reset_transaction_recover(root) {
        Ok(Some(true)) => {
            println!("{}", mode.applied_message());
            print_reset_result(mode);
            return Ok(());
        }
        Ok(Some(false)) => return Err("GOVERNANCE_RESET_RECOVERED_RETRY".into()),
        Ok(None) => {}
        Err(error) => return Err(error),
    }
    reset_requires_clean_worktree(root, mode)?;
    let generated_roots = reset_generated_roots(root)?;
    reset_transaction_run(root, &branch, &generated_roots, mode)?;
    println!("{}", mode.applied_message());
    print_reset_result(mode);
    Ok(())
}

pub(super) fn locate_git_bug_binary() -> Result<PathBuf, String> {
    if let Ok(path) = env::var("GIT_BUG_BIN") {
        let p = PathBuf::from(path);
        if p.exists() {
            return Ok(p);
        }
    }
    if let Ok(output) = Command::new("which").arg("git-bug").output() {
        if output.status.success() {
            let s = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !s.is_empty() {
                return Ok(PathBuf::from(s));
            }
        }
    }
    if let Ok(home) = env::var("HOME") {
        let p = PathBuf::from(home).join(".local/bin/git-bug");
        if p.exists() {
            return Ok(p);
        }
    }
    Err("GIT_BUG_NOT_FOUND: please run `appsdk setup-deps` to install git-bug".into())
}

pub(super) fn bug_record_matches_identity(record: &Value, issue_id: &str) -> bool {
    record.get("human_id").and_then(Value::as_str) == Some(issue_id)
        || record.get("id").and_then(Value::as_str) == Some(issue_id)
}

pub(super) fn query_bug_record(root: &Path, issue_id: &str) -> Result<Value, String> {
    let git_bug = locate_git_bug_binary()?;
    let run = |dir: &Path| {
        Command::new(&git_bug)
            .args(["bug", "show", issue_id, "-f", "json"])
            .current_dir(dir)
            .output()
    };
    let output = run_git_bug_read(|| run(root), true)
        .map_err(|error| format!("BUG_TRIAGE_QUERY_EXECUTION_FAILED:{error}"))?;
    if !output.status.success() {
        let detail = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(if detail.is_empty() {
            format!(
                "BUG_TRIAGE_QUERY_FAILED:{}:exit={}",
                issue_id,
                output.status.code().unwrap_or(-1)
            )
        } else {
            format!("BUG_TRIAGE_QUERY_FAILED:{}:{}", issue_id, detail)
        });
    }
    let record: Value = serde_json::from_slice(&output.stdout)
        .map_err(|error| format!("BUG_TRIAGE_QUERY_INVALID_JSON:{}:{error}", issue_id))?;
    if !bug_record_matches_identity(&record, issue_id) {
        return Err(format!("BUG_TRIAGE_QUERY_IDENTITY_MISMATCH:{issue_id}"));
    }
    Ok(record)
}

pub(super) fn git_bug_close_event(root: &Path, record: &Value) -> Option<Value> {
    if record.get("status").and_then(Value::as_str) != Some("closed") {
        return None;
    }
    let bug_id = record.get("id").and_then(Value::as_str)?;
    let ref_name = format!("refs/bugs/{bug_id}^{{commit}}");
    let tip = String::from_utf8(
        Command::new("git")
            .args(["rev-parse", "--verify", &ref_name])
            .current_dir(root)
            .output()
            .ok()?
            .stdout,
    )
    .ok()?
    .trim()
    .to_string();
    if tip.is_empty() {
        return None;
    }
    let commit = String::from_utf8(
        Command::new("git")
            .args(["cat-file", "-p", &tip])
            .current_dir(root)
            .output()
            .ok()?
            .stdout,
    )
    .ok()?;
    let tree = commit.lines().find_map(|line| line.strip_prefix("tree "))?;
    let ops_blob = String::from_utf8(
        Command::new("git")
            .args(["ls-tree", "-r", tree])
            .current_dir(root)
            .output()
            .ok()?
            .stdout,
    )
    .ok()?
    .lines()
    .find_map(|line| {
        let mut parts = line.split_whitespace();
        let blob = parts.nth(2)?;
        (parts.next() == Some("ops")).then(|| blob.to_string())
    })?;
    let ops: Value = serde_json::from_slice(
        &Command::new("git")
            .args(["cat-file", "-p", &ops_blob])
            .current_dir(root)
            .output()
            .ok()?
            .stdout,
    )
    .ok()?;
    let close_op = ops.get("ops").and_then(Value::as_array).and_then(|items| {
        items.iter().find(|item| {
            item.get("type").and_then(Value::as_u64) == Some(4)
                && item.get("status").and_then(Value::as_u64) == Some(2)
        })
    })?;
    let comment_id = record
        .get("comments")
        .and_then(Value::as_array)
        .and_then(|comments| comments.last())
        .and_then(|comment| comment.get("id"))
        .and_then(Value::as_str)?;
    Some(serde_json::json!({
        "event_id": tip,
        "action": "close",
        "comment_id": comment_id,
        "producer": {
            "adapter": "appsdk",
            "identity": "appsdk::bug-close"
        },
        "metadata": {
            "bug_id": bug_id,
            "status": "closed",
            "timestamp": close_op.get("timestamp")
        }
    }))
}

pub(super) fn bug_triage_binding(issue_id: &str, triage: &Value) -> String {
    let mut binding = serde_json::json!({
        "issue_id": issue_id,
        "query": triage["query"],
        "mode": triage["mode"],
        "reopened_from_issue_id": triage["reopened_from_issue_id"]
    });
    if let Some(matched_issue_id) = triage.get("matched_issue_id") {
        binding["matched_issue_id"] = matched_issue_id.clone();
    }
    if let Some(matched_title) = triage.get("matched_title") {
        binding["matched_title"] = matched_title.clone();
    }
    if let Some(matched_classification) = triage.get("matched_classification") {
        binding["matched_classification"] = matched_classification.clone();
    }
    if let Some(query_result) = triage.get("query_result") {
        binding["query_result"] = query_result.clone();
    }
    sha256(&canonical(&binding))
}

pub(super) fn bug_query_result_hash(records: &Value) -> String {
    sha256(&canonical(records))
}

pub(super) fn bug_triage_query_result(triage: &Value) -> Option<&Value> {
    triage.get("query_result")
}

pub(super) fn assert_bug_triage_query_result(
    triage: &Value,
    issue_id: &str,
    query: &str,
    matched_issue_id: Option<&str>,
    matched_title: Option<&str>,
    matched_classification: Option<&str>,
    real_query_root: Option<&Path>,
) -> Result<(), String> {
    let Some(query_result) = bug_triage_query_result(triage) else {
        return Ok(());
    };
    let query_result = query_result
        .as_object()
        .ok_or_else(|| "BUG_TRIAGE_QUERY_RESULT_INVALID".to_string())?;
    let records = query_result
        .get("records")
        .filter(|value| value.is_array())
        .ok_or_else(|| "BUG_TRIAGE_QUERY_RESULT_INVALID".to_string())?;
    let expected_hash = query_result
        .get("query_result_hash")
        .and_then(Value::as_str)
        .ok_or_else(|| "BUG_TRIAGE_QUERY_RESULT_HASH_INVALID".to_string())?;
    if expected_hash != bug_query_result_hash(records) {
        return Err("BUG_TRIAGE_QUERY_RESULT_HASH_MISMATCH".into());
    }

    let Some(matched_issue_id) = matched_issue_id else {
        return Ok(());
    };
    let matched_title =
        matched_title.ok_or_else(|| "BUG_TRIAGE_MATCHED_TITLE_MISSING".to_string())?;
    let matched_classification = matched_classification
        .ok_or_else(|| "BUG_TRIAGE_MATCHED_CLASSIFICATION_MISSING".to_string())?;
    let matched = records
        .as_array()
        .unwrap()
        .iter()
        .find(|record| bug_record_matches_identity(record, matched_issue_id))
        .ok_or_else(|| "BUG_TRIAGE_MATCHED_ISSUE_NOT_IN_RESULT".to_string())?;
    if bug_intake_record_id(matched) != issue_id {
        return Err("BUG_TRIAGE_MATCHED_ISSUE_MISMATCH".into());
    }
    if matched.get("title").and_then(Value::as_str) != Some(matched_title) {
        return Err("BUG_TRIAGE_MATCHED_TITLE_MISMATCH".into());
    }
    if !bug_intake_record_matches(matched, matched_classification, matched_title) {
        return Err("BUG_TRIAGE_MATCHED_CLASSIFICATION_MISMATCH".into());
    }
    let Some(root) = real_query_root else {
        return Err("BUG_TRIAGE_QUERY_RESULT_UNVERIFIED".into());
    };
    let git_bug = locate_git_bug_binary()?;
    let query = query
        .strip_prefix("git-bug bug ")
        .and_then(|value| value.strip_suffix(" -f json"))
        .and_then(|value| serde_json::from_str::<String>(value).ok())
        .ok_or_else(|| "BUG_TRIAGE_QUERY_INVALID".to_string())?;
    let (actual_records, _) = bug_intake_query(&git_bug, root, &query);
    if bug_query_result_hash(&actual_records) != expected_hash {
        return Err("BUG_TRIAGE_QUERY_RESULT_DRIFT".into());
    }
    Ok(())
}

pub(super) fn assert_bug_tracker_triage_evidence(
    worktree: &Value,
    issue_id: &str,
    real_query_root: Option<&Path>,
    require_binding: bool,
) {
    let triage = worktree.get("bug_triage");
    let legacy_issue = issue_id.starts_with("legacy-");
    let exempt_issue = issue_id.is_empty() || issue_id == "none" || legacy_issue;
    if triage.is_none() {
        if !exempt_issue {
            fail("BUG_TRIAGE_MISSING");
        }
        return;
    }
    if issue_id.is_empty() || issue_id == "none" {
        fail("BUG_TRIAGE_UNEXPECTED_FOR_EXEMPT_ISSUE");
    }

    let triage = triage.unwrap();
    if !triage.is_object() {
        fail("INVALID_BUG_TRIAGE");
    }
    let mode = triage.get("mode").and_then(Value::as_str).unwrap_or("");
    if !matches!(
        mode,
        "new_confirmed" | "reused" | "reopened" | "reopened_same_record" | "historical_legacy"
    ) {
        fail("BUG_TRIAGE_MODE_INVALID");
    }
    if triage.get("query_executed") != Some(&Value::Bool(true)) {
        fail("BUG_TRIAGE_QUERY_MISSING");
    }
    let query = triage.get("query").and_then(Value::as_str).unwrap_or("");
    if query.is_empty() {
        fail("BUG_TRIAGE_QUERY_UNBOUND");
    }
    let matched_issue_id = if let Some(value) = triage.get("matched_issue_id") {
        match value {
            Value::Null => None,
            Value::String(value) if !value.is_empty() => Some(value.as_str()),
            _ => fail("BUG_TRIAGE_MATCHED_ISSUE_INVALID"),
        }
    } else {
        if !query.split_whitespace().any(|token| token == issue_id) {
            fail("BUG_TRIAGE_QUERY_UNBOUND");
        }
        None
    };
    let matched_title = triage
        .get("matched_title")
        .map(|value| match value {
            Value::Null => None,
            Value::String(value) if !value.is_empty() => Some(value.as_str()),
            _ => fail("BUG_TRIAGE_MATCHED_TITLE_INVALID"),
        })
        .unwrap_or(None);
    let matched_classification = triage
        .get("matched_classification")
        .map(|value| match value {
            Value::Null => None,
            Value::String(value) if !value.is_empty() => Some(value.as_str()),
            _ => fail("BUG_TRIAGE_MATCHED_CLASSIFICATION_INVALID"),
        })
        .unwrap_or(None);
    let reopened_from = triage
        .get("reopened_from_issue_id")
        .unwrap_or_else(|| fail("BUG_TRIAGE_REOPENED_SOURCE_MISSING"));
    let reopened_from_id = match reopened_from {
        Value::Null => None,
        Value::String(value) => Some(value.as_str()),
        _ => fail("BUG_TRIAGE_REOPENED_SOURCE_INVALID"),
    };
    if mode == "historical_legacy" {
        if !legacy_issue {
            fail("BUG_TRIAGE_LEGACY_ID_MISMATCH");
        }
        if reopened_from_id.is_some() {
            fail("BUG_TRIAGE_REOPENED_SOURCE_UNEXPECTED");
        }
        if matched_issue_id.is_some() {
            fail("BUG_TRIAGE_MATCHED_ISSUE_UNEXPECTED");
        }
        return;
    }
    if legacy_issue {
        fail("BUG_TRIAGE_LEGACY_MODE_INVALID");
    }
    if mode == "new_confirmed" {
        if matched_issue_id.is_some() {
            fail("BUG_TRIAGE_MATCHED_ISSUE_UNEXPECTED");
        }
        if reopened_from_id.is_some() {
            fail("BUG_TRIAGE_REOPENED_SOURCE_UNEXPECTED");
        }
    } else if mode == "reused" {
        if matched_issue_id != Some(issue_id) {
            fail("BUG_TRIAGE_MATCHED_ISSUE_MISMATCH");
        }
        if reopened_from_id.is_some() {
            fail("BUG_TRIAGE_REOPENED_SOURCE_UNEXPECTED");
        }
    } else if mode == "reopened_same_record" {
        if matched_issue_id != Some(issue_id) {
            fail("BUG_TRIAGE_MATCHED_ISSUE_MISMATCH");
        }
        if reopened_from_id != Some(issue_id) {
            fail("BUG_TRIAGE_REOPENED_SOURCE_MISMATCH");
        }
    } else if mode == "reopened" {
        let Some(reopened_from_id) = reopened_from_id.filter(|value| !value.is_empty()) else {
            fail("BUG_TRIAGE_REOPENED_SOURCE_MISSING");
        };
        if reopened_from_id == issue_id
            || !query
                .split_whitespace()
                .any(|token| token == reopened_from_id)
        {
            fail("BUG_TRIAGE_REOPENED_QUERY_UNBOUND");
        }
    } else if reopened_from_id.is_some() {
        fail("BUG_TRIAGE_REOPENED_SOURCE_UNEXPECTED");
    }
    if matched_issue_id.is_some() && (matched_title.is_none() || matched_classification.is_none()) {
        fail("BUG_TRIAGE_MATCHED_CLASSIFICATION_MISSING");
    }
    if matched_issue_id.is_none() && (matched_title.is_some() || matched_classification.is_some()) {
        fail("BUG_TRIAGE_MATCHED_CLASSIFICATION_UNEXPECTED");
    }

    assert_bug_triage_query_result(
        triage,
        issue_id,
        query,
        matched_issue_id,
        matched_title,
        matched_classification,
        real_query_root,
    )
    .unwrap_or_else(|error| fail(error));

    let expected_binding = bug_triage_binding(issue_id, triage);
    if require_binding
        && worktree
            .get("bug_triage_query_binding")
            .and_then(Value::as_str)
            != Some(expected_binding.as_str())
    {
        fail("BUG_TRIAGE_QUERY_BINDING_MISMATCH");
    }

    if let Some(root) = real_query_root {
        query_bug_record(root, issue_id).unwrap_or_else(|error| fail(error));
        if let Some(reopened_from_id) = reopened_from_id {
            query_bug_record(root, reopened_from_id).unwrap_or_else(|error| fail(error));
        }
    }
}

pub(super) fn assert_bug_tracker_solution_evidence(root: &Path, issue_id: &str, promotion: &Value) {
    if issue_id.is_empty() || issue_id == "none" || issue_id.starts_with("legacy-") {
        return;
    }

    if promotion.get("bug_closure_verified") != Some(&Value::Bool(true)) {
        fail("BUG_TRACKER_CLOSURE_NOT_VERIFIED");
    }
    let record = query_bug_record(root, issue_id)
        .unwrap_or_else(|error| fail(format!("BUG_TRACKER_CLOSURE_QUERY_FAILED:{}", error)));
    if record.get("status").and_then(Value::as_str) != Some("closed") {
        fail(format!("BUG_TRACKER_ISSUE_NOT_CLOSED:{}", issue_id));
    }
    if git_bug_close_event(root, &record).is_none() {
        fail(format!("BUG_TRACKER_CLOSE_EVENT_MISSING:{}", issue_id));
    }
    let has_solution = record
        .get("comments")
        .and_then(Value::as_array)
        .map(|comments| {
            comments.iter().any(|comment| {
                comment
                    .get("message")
                    .and_then(Value::as_str)
                    .is_some_and(|message| {
                        message.contains("### Solution / Resolution")
                            || message.contains("Solution:")
                    })
            })
        })
        .unwrap_or(false);
    if !has_solution {
        fail(format!(
            "BUG_TRACKER_SOLUTION_EVIDENCE_MISSING:{}",
            issue_id
        ));
    }
}
