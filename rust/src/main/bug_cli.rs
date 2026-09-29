use super::*;

pub(super) fn bug_cli(args: &mut std::iter::Peekable<std::vec::IntoIter<String>>) {
    let mut root = PathBuf::from(".");
    if let Some(first) = args.peek() {
        if !matches!(
            first.as_str(),
            "intake"
                | "new"
                | "list"
                | "show"
                | "comment"
                | "close"
                | "webui"
                | "help"
                | "--help"
                | "-h"
        ) && !first.starts_with('-')
        {
            root = PathBuf::from(args.next().unwrap());
        }
    }
    handle_bug_command(&root, args.to_owned());
}

pub(super) fn setup_deps(check_only: bool) {
    if check_only {
        match locate_git_bug_binary() {
            Ok(p) => {
                let out = Command::new(&p).arg("version").output();
                let ver = out
                    .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
                    .unwrap_or_else(|_| "installed".into());
                println!("{{\"git_bug\":{{\"status\":\"installed\",\"path\":\"{}\",\"version\":\"{}\"}}}}", p.display(), ver);
            }
            Err(e) => {
                fail(format!("DEPENDENCY_CHECK_FAILED:{}", e));
            }
        }
        return;
    }

    let home = env::var("HOME").unwrap_or_else(|_| fail("HOME_NOT_SET"));
    let install_dir = PathBuf::from(&home).join(".local/bin");
    fs::create_dir_all(&install_dir).unwrap_or_else(|_| fail("INSTALL_DIR_CREATE_FAILED"));
    let git_bug_target = install_dir.join("git-bug");

    let os = match env::consts::OS {
        "macos" => "darwin",
        "linux" => "linux",
        other => fail(format!("UNSUPPORTED_OS:{}", other)),
    };
    let arch = match env::consts::ARCH {
        "x86_64" => "amd64",
        "aarch64" => "arm64",
        other => fail(format!("UNSUPPORTED_ARCH:{}", other)),
    };

    let version = "0.10.1";
    let binary_name = format!("git-bug_{}_{}", os, arch);
    let download_url = format!(
        "https://github.com/git-bug/git-bug/releases/download/v{}/{}",
        version, binary_name
    );

    println!("Downloading git-bug from {} ...", download_url);
    let curl_status = Command::new("curl")
        .args([
            "-fsSL",
            &download_url,
            "-o",
            git_bug_target.to_str().unwrap(),
        ])
        .status();

    let download_success = match curl_status {
        Ok(s) if s.success() => true,
        _ => {
            let wget_status = Command::new("wget")
                .args(["-qO", git_bug_target.to_str().unwrap(), &download_url])
                .status();
            wget_status.map(|s| s.success()).unwrap_or(false)
        }
    };

    if !download_success {
        fail(format!("DOWNLOAD_FAILED:{}", download_url));
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(metadata) = fs::metadata(&git_bug_target) {
            let mut perms = metadata.permissions();
            perms.set_mode(0o755);
            let _ = fs::set_permissions(&git_bug_target, perms);
        }
    }

    println!(
        "{{\"ok\":true,\"installed_to\":\"{}\",\"version\":\"{}\"}}",
        git_bug_target.display(),
        version
    );
}

pub(super) fn resolve_upstream_repo() -> Result<PathBuf, String> {
    let candidate = if let Some(value) = env::var_os("APPSDK_ROOT") {
        let value = value.to_string_lossy().trim().to_string();
        if value.is_empty() {
            return Err("APPSDK_UPSTREAM_REPO_NOT_FOUND: APPSDK_ROOT is empty".into());
        }
        PathBuf::from(value)
    } else {
        let home = env::var_os("HOME")
            .ok_or_else(|| "APPSDK_UPSTREAM_REPO_NOT_FOUND: set APPSDK_ROOT or HOME".to_string())?;
        PathBuf::from(home).join("Documents/github/appsdk")
    };

    if !candidate.exists() {
        return Err(format!(
            "APPSDK_UPSTREAM_REPO_NOT_FOUND: expected AppSDK repository at {} (set APPSDK_ROOT)",
            candidate.display()
        ));
    }

    let output = Command::new("git")
        .arg("-C")
        .arg(&candidate)
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .map_err(|error| {
            format!(
                "APPSDK_UPSTREAM_REPO_INVALID:{}:{}",
                candidate.display(),
                error
            )
        })?;
    if !output.status.success() {
        let detail = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(format!(
            "APPSDK_UPSTREAM_REPO_INVALID:{}{}",
            candidate.display(),
            if detail.is_empty() {
                String::new()
            } else {
                format!(":{}", detail)
            }
        ));
    }

    let root = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if root.is_empty() {
        return Err(format!(
            "APPSDK_UPSTREAM_REPO_INVALID:{}:git returned no repository root",
            candidate.display()
        ));
    }
    fs::canonicalize(Path::new(&root)).map_err(|error| {
        format!(
            "APPSDK_UPSTREAM_REPO_INVALID:{}:{}",
            candidate.display(),
            error
        )
    })
}

pub(super) fn select_bug_store(root: &Path, explicit_upstream: bool) -> Result<PathBuf, String> {
    if explicit_upstream {
        resolve_upstream_repo()
    } else {
        Ok(root.to_path_buf())
    }
}

pub(super) fn run_git_bug_read<F>(mut run: F, expect_output: bool) -> std::io::Result<Output>
where
    F: FnMut() -> std::io::Result<Output>,
{
    const MAX_ATTEMPTS: usize = 5;
    for attempt in 0..MAX_ATTEMPTS {
        let output = run()?;
        if (output.status.success() && (!expect_output || !output.stdout.is_empty()))
            || attempt + 1 == MAX_ATTEMPTS
        {
            return Ok(output);
        }
        let stderr = String::from_utf8_lossy(&output.stderr);
        // git-bug 0.10.1 can expose an empty lock owner as this parse error
        // while another read is releasing the repository lock.
        if !stderr.contains("already locked")
            && !stderr.contains("git-bug/lock")
            && !stderr.contains("strconv.Atoi: parsing \"\": invalid syntax")
            && !(expect_output && output.status.success())
        {
            return Ok(output);
        }
        thread::sleep(Duration::from_millis(25 * (attempt as u64 + 1)));
    }
    unreachable!("read retry loop always returns an output")
}

pub(super) fn bug_intake_string<'a>(input: &'a Value, field: &str, error: &str) -> &'a str {
    input
        .get(field)
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| fail(error))
}

pub(super) fn bug_intake_strings(input: &Value, field: &str, error: &str) -> Vec<String> {
    input
        .get(field)
        .and_then(Value::as_array)
        .unwrap_or_else(|| fail(error))
        .iter()
        .map(|value| {
            value
                .as_str()
                .filter(|entry| !entry.trim().is_empty())
                .unwrap_or_else(|| fail(error))
                .to_string()
        })
        .collect()
}

pub(super) fn bug_intake_query(git_bug: &Path, root: &Path, dedup_query: &str) -> (Value, String) {
    let query = format!(
        "git-bug bug {} -f json",
        serde_json::to_string(dedup_query)
            .unwrap_or_else(|_| fail("BUG_INTAKE_DEDUP_QUERY_INVALID"))
    );
    let listed = run_git_bug_read(
        || {
            Command::new(git_bug)
                .args(["bug", dedup_query, "-f", "json"])
                .current_dir(root)
                .output()
        },
        true,
    )
    .unwrap_or_else(|_| fail("GIT_BUG_EXECUTION_FAILED"));
    if !listed.status.success() {
        fail(format!(
            "GIT_BUG_LIST_FAILED:{}",
            String::from_utf8_lossy(&listed.stderr).trim()
        ));
    }
    let records: Value = serde_json::from_slice(&listed.stdout)
        .unwrap_or_else(|_| fail("GIT_BUG_LIST_INVALID_JSON"));
    if !records.is_array() {
        fail("GIT_BUG_LIST_INVALID_JSON");
    }
    (records, query)
}

pub(super) fn bug_intake_record_matches(record: &Value, classification: &str, title: &str) -> bool {
    if record.get("title").and_then(Value::as_str) != Some(title) {
        return false;
    }
    let labels = record
        .get("labels")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    let mut classifications = labels.iter().filter_map(|label| {
        label
            .as_str()
            .and_then(|label| label.strip_prefix("classification:"))
    });
    let Some(existing) = classifications.next() else {
        return true;
    };
    existing == classification && classifications.all(|value| value == existing)
}

pub(super) fn bug_intake_record_classification(record: &Value) -> Option<&str> {
    record
        .get("labels")
        .and_then(Value::as_array)
        .and_then(|labels| {
            labels.iter().find_map(|label| {
                label
                    .as_str()
                    .and_then(|label| label.strip_prefix("classification:"))
            })
        })
}

pub(super) fn bug_intake_record_needs_classification_migration(record: &Value) -> bool {
    record
        .get("labels")
        .and_then(Value::as_array)
        .map(|labels| {
            labels.iter().all(|label| {
                label
                    .as_str()
                    .is_none_or(|label| !label.starts_with("classification:"))
            })
        })
        .unwrap_or(true)
}

pub(super) fn bug_intake_migrate_classification(
    git_bug: &Path,
    root: &Path,
    issue_id: &str,
    classification: &str,
) {
    let label = format!("classification:{}", classification);
    let output = Command::new(git_bug)
        .args(["bug", "label", "new", issue_id, &label])
        .current_dir(root)
        .output()
        .unwrap_or_else(|_| fail("GIT_BUG_EXECUTION_FAILED"));
    if !output.status.success() {
        fail(format!(
            "GIT_BUG_LABEL_FAILED:{}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
}

pub(super) fn bug_intake_record_id(record: &Value) -> String {
    record
        .get("human_id")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .or_else(|| record.get("id").and_then(Value::as_str))
        .unwrap_or_else(|| fail("BUG_INTAKE_RECORD_ID_MISSING"))
        .to_string()
}

pub(super) fn bug_intake_record_body(input: &Value) -> String {
    let contract = serde_json::json!({
        "original_input": input["original_input"],
        "classification": input["classification"],
        "scope": input["scope"],
        "owner": input["owner"],
        "parent_id": input["parent_id"],
        "acceptance": input["acceptance"],
        "status": input["status"],
        "evidence_links": input["evidence_links"]
    });
    let mut body = format!(
        "### Intake Contract\n```json\n{}\n```\n\n### Original Input\n{}\n\nClassification: {}\n\nScope:\n",
        serde_json::to_string_pretty(&contract)
            .unwrap_or_else(|_| fail("BUG_INTAKE_INPUT_INVALID_JSON")),
        bug_intake_string(input, "original_input", "BUG_INTAKE_ORIGINAL_INPUT_MISSING"),
        bug_intake_string(input, "classification", "BUG_INTAKE_CLASSIFICATION_INVALID")
    );
    for item in bug_intake_strings(input, "scope", "BUG_INTAKE_SCOPE_INVALID") {
        body.push_str(&format!("- {}\n", item));
    }
    body.push_str(&format!(
        "\nOwner: {}\nParent: {}\n\nAcceptance:\n",
        bug_intake_string(input, "owner", "BUG_INTAKE_OWNER_MISSING"),
        input
            .get("parent_id")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .unwrap_or("none")
    ));
    for item in bug_intake_strings(input, "acceptance", "BUG_INTAKE_ACCEPTANCE_INVALID") {
        body.push_str(&format!("- {}\n", item));
    }
    body.push_str(&format!(
        "\nStatus: {}\n\nEvidence links:\n",
        bug_intake_string(input, "status", "BUG_INTAKE_STATUS_MISSING")
    ));
    for item in bug_intake_strings(input, "evidence_links", "BUG_INTAKE_EVIDENCE_LINKS_INVALID") {
        body.push_str(&format!("- {}\n", item));
    }
    body
}

pub(super) fn bug_intake_triage(
    issue_id: &str,
    query: &str,
    query_result: &Value,
    mode: &str,
    reopened_from_issue_id: Option<&str>,
    matched_issue_id: Option<&str>,
    matched_title: Option<&str>,
    matched_classification: Option<&str>,
) -> (Value, String) {
    let query_result = serde_json::json!({
        "records": query_result,
        "query_result_hash": bug_query_result_hash(query_result)
    });
    let binding_payload = serde_json::json!({
        "issue_id": issue_id,
        "query": query,
        "mode": mode,
        "reopened_from_issue_id": reopened_from_issue_id,
        "matched_issue_id": matched_issue_id,
        "matched_title": matched_title,
        "matched_classification": matched_classification,
        "query_result": query_result
    });
    let binding = sha256(&canonical(&binding_payload));
    let triage = serde_json::json!({
        "query_executed": true,
        "query": query,
        "mode": mode,
        "reopened_from_issue_id": reopened_from_issue_id,
        "matched_issue_id": matched_issue_id,
        "matched_title": matched_title,
        "matched_classification": matched_classification,
        "query_result": query_result
    });
    (triage, binding)
}

pub(super) fn bug_intake_validate_created(
    root: &Path,
    issue_id: &str,
    title: &str,
    classification: &str,
) -> Value {
    let record = query_bug_record(root, issue_id).unwrap_or_else(|error| fail(error));
    let human_id = record.get("human_id").and_then(Value::as_str);
    let canonical_id = record.get("id").and_then(Value::as_str);
    if human_id != Some(issue_id) {
        fail("BUG_INTAKE_CREATE_ID_MISMATCH");
    }
    if canonical_id.is_some_and(|value| value.is_empty() || !value.starts_with(issue_id)) {
        fail("BUG_INTAKE_CREATE_ID_MISMATCH");
    }
    if record.get("title").and_then(Value::as_str) != Some(title) {
        fail("BUG_INTAKE_CREATE_TITLE_MISMATCH");
    }
    let expected_label = format!("classification:{}", classification);
    let has_classification = record
        .get("labels")
        .and_then(Value::as_array)
        .is_some_and(|labels| {
            labels
                .iter()
                .any(|label| label.as_str() == Some(&expected_label))
        });
    if !has_classification {
        fail("BUG_INTAKE_CREATE_CLASSIFICATION_MISMATCH");
    }
    record
}

pub(super) fn bug_intake_create(
    git_bug: &Path,
    root: &Path,
    input: &Value,
    title: &str,
    classification: &str,
) -> (String, Value) {
    let body = bug_intake_record_body(input);
    let output = Command::new(git_bug)
        .args(["bug", "new", "-t", title, "-m", &body, "--non-interactive"])
        .current_dir(root)
        .output()
        .unwrap_or_else(|_| fail("GIT_BUG_EXECUTION_FAILED"));
    if !output.status.success() {
        fail(format!(
            "GIT_BUG_NEW_FAILED:{}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let bug_id = String::from_utf8_lossy(&output.stdout)
        .lines()
        .next()
        .and_then(|line| {
            line.split_whitespace()
                .find(|part| part.chars().all(|c| c.is_ascii_hexdigit()) && part.len() >= 7)
        })
        .unwrap_or_else(|| fail("GIT_BUG_NEW_ID_MISSING"))
        .to_string();
    let label = format!("classification:{}", classification);
    let output = Command::new(git_bug)
        .args(["bug", "label", "new", &bug_id, &label])
        .current_dir(root)
        .output()
        .unwrap_or_else(|_| fail("GIT_BUG_EXECUTION_FAILED"));
    if !output.status.success() {
        fail(format!(
            "GIT_BUG_LABEL_FAILED:{}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let record = bug_intake_validate_created(root, &bug_id, title, classification);
    (bug_id, record)
}

pub(super) fn bug_intake_reuse(
    git_bug: &Path,
    root: &Path,
    issue_id: &str,
    input: &Value,
    reopen: bool,
) -> (bool, bool) {
    let body = bug_intake_record_body(input);
    let record = query_bug_record(root, issue_id).unwrap_or_else(|error| fail(error));
    let already_recorded = record
        .get("comments")
        .and_then(Value::as_array)
        .is_some_and(|comments| {
            comments.iter().any(|comment| {
                comment
                    .get("message")
                    .and_then(Value::as_str)
                    .is_some_and(|message| message.trim_end() == body.trim_end())
            })
        });
    if !already_recorded {
        let output = Command::new(git_bug)
            .args([
                "bug",
                "comment",
                "new",
                issue_id,
                "-m",
                &body,
                "--non-interactive",
            ])
            .current_dir(root)
            .output()
            .unwrap_or_else(|_| fail("GIT_BUG_EXECUTION_FAILED"));
        if !output.status.success() {
            fail(format!(
                "GIT_BUG_COMMENT_FAILED:{}",
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
    }
    if reopen {
        let output = Command::new(git_bug)
            .args(["bug", "status", "open", issue_id])
            .current_dir(root)
            .output()
            .unwrap_or_else(|_| fail("GIT_BUG_EXECUTION_FAILED"));
        if !output.status.success() {
            fail(format!(
                "GIT_BUG_REOPEN_FAILED:{}",
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
    }
    (true, !already_recorded)
}

pub(super) fn bug_intake_input_bytes(root: &Path, input: &str) -> Vec<u8> {
    let input_path = if Path::new(input).is_absolute() {
        PathBuf::from(input)
    } else {
        root.join(input)
    };
    match fs::read(&input_path) {
        Ok(bytes) => bytes,
        Err(_) if input.trim_start().starts_with('{') || input.trim_start().starts_with('[') => {
            input.as_bytes().to_vec()
        }
        Err(_) => fail("BUG_INTAKE_INPUT_READ_FAILED"),
    }
}

pub(super) fn bug_intake(
    git_bug: &Path,
    root: &Path,
    input_arg: &str,
    ensure_identity: &dyn Fn(&Path),
) {
    let input: Value = serde_json::from_slice(&bug_intake_input_bytes(root, input_arg))
        .unwrap_or_else(|_| fail("BUG_INTAKE_INPUT_INVALID_JSON"));
    if input.get("execution_bound").and_then(Value::as_bool) != Some(true) {
        fail("DEVELOPMENT_INTAKE_READ_ONLY_CONVERSATION");
    }
    let classification = bug_intake_string(
        &input,
        "classification",
        "BUG_INTAKE_CLASSIFICATION_INVALID",
    );
    if !matches!(classification, "bug" | "feature") {
        fail("BUG_INTAKE_CLASSIFICATION_INVALID");
    }
    let title = bug_intake_string(&input, "title", "BUG_INTAKE_TITLE_MISSING");
    let dedup_query = bug_intake_string(&input, "dedup_query", "BUG_INTAKE_DEDUP_QUERY_MISSING");
    let _scope = bug_intake_strings(&input, "scope", "BUG_INTAKE_SCOPE_INVALID");
    let _acceptance = bug_intake_strings(&input, "acceptance", "BUG_INTAKE_ACCEPTANCE_INVALID");
    let _evidence_links = bug_intake_strings(
        &input,
        "evidence_links",
        "BUG_INTAKE_EVIDENCE_LINKS_INVALID",
    );
    let _owner = bug_intake_string(&input, "owner", "BUG_INTAKE_OWNER_MISSING");
    let _status = bug_intake_string(&input, "status", "BUG_INTAKE_STATUS_MISSING");

    ensure_identity(root);
    let (mut records, mut triage_query) = bug_intake_query(git_bug, root, dedup_query);
    let existing = records
        .as_array()
        .unwrap()
        .iter()
        .find(|record| bug_intake_record_matches(record, classification, title));
    let (issue_id, deduplicated, created, reopened, appended, triage_mode) = if let Some(record) =
        existing
    {
        let issue_id = bug_intake_record_id(record);
        if bug_intake_record_needs_classification_migration(record) {
            bug_intake_migrate_classification(git_bug, root, &issue_id, classification);
        }
        let reopen = record.get("status").and_then(Value::as_str) != Some("open");
        let (deduplicated, appended) = bug_intake_reuse(git_bug, root, &issue_id, &input, reopen);
        (
            issue_id,
            deduplicated,
            false,
            reopen,
            appended,
            if reopen {
                "reopened_same_record"
            } else {
                "reused"
            },
        )
    } else {
        let (issue_id, _record) = bug_intake_create(git_bug, root, &input, title, classification);
        records = serde_json::json!([]);
        (issue_id, false, true, false, false, "new_confirmed")
    };
    let (matched_issue_id, matched_title, matched_classification) = if created {
        (None, None, None)
    } else {
        let (current_records, current_query) = bug_intake_query(git_bug, root, dedup_query);
        records = current_records;
        triage_query = current_query;
        let matched = records
            .as_array()
            .unwrap()
            .iter()
            .find(|record| bug_record_matches_identity(record, &issue_id))
            .unwrap_or_else(|| fail("BUG_INTAKE_MATCHED_ISSUE_NOT_IN_RESULT"));
        if !bug_intake_record_matches(matched, classification, title) {
            fail("BUG_INTAKE_MATCHED_RECORD_INVALID");
        }
        let matched_title = matched
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or_else(|| fail("BUG_INTAKE_MATCHED_TITLE_MISSING"));
        let matched_classification = bug_intake_record_classification(matched)
            .unwrap_or_else(|| fail("BUG_INTAKE_MATCHED_CLASSIFICATION_MISSING"));
        (
            Some(issue_id.as_str()),
            Some(matched_title),
            Some(matched_classification),
        )
    };
    let (bug_triage, bug_triage_query_binding) = bug_intake_triage(
        &issue_id,
        &triage_query,
        &records,
        triage_mode,
        if reopened { Some(&issue_id) } else { None },
        matched_issue_id,
        matched_title,
        matched_classification,
    );

    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "ok": true,
            "issue_id": issue_id,
            "classification": classification,
            "deduplicated": deduplicated,
            "created": created,
            "reopened": reopened,
            "appended": appended,
            "governed_completion_requires_issue_id": true,
            "bug_triage": bug_triage,
            "bug_triage_query_binding": bug_triage_query_binding,
            "lifecycle_binding": {
                "worktree": "issue_id",
                "implementation": "issue_id",
                "test": "issue_id",
                "review": "issue_id",
                "merge": "issue_id",
                "closure": "issue_id"
            }
        }))
        .unwrap()
    );
}

pub(super) fn handle_bug_command<I>(root: &Path, mut args: I)
where
    I: Iterator<Item = String>,
{
    let sub = args.next().unwrap_or_else(|| {
        fail("USAGE: appsdk bug <intake|new|list|show|comment|close|webui> [options]")
    });

    let git_bug = locate_git_bug_binary().unwrap_or_else(|e| fail(e));

    let ensure_identity = |target_dir: &Path| {
        let user_list = Command::new(&git_bug)
            .args(["user", "-f", "json"])
            .current_dir(target_dir)
            .output();
        if let Ok(out) = user_list {
            let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if stdout.is_empty() || stdout == "[]" || stdout == "null" {
                let name_out = Command::new("git")
                    .args(["-C", target_dir.to_str().unwrap(), "config", "user.name"])
                    .output();
                let email_out = Command::new("git")
                    .args(["-C", target_dir.to_str().unwrap(), "config", "user.email"])
                    .output();
                let name = name_out
                    .ok()
                    .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
                    .unwrap_or_default();
                let email = email_out
                    .ok()
                    .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
                    .unwrap_or_default();
                let user_name = if name.is_empty() {
                    "AppSDK User".to_string()
                } else {
                    name
                };
                let user_email = if email.is_empty() {
                    "user@appsdk.local".to_string()
                } else {
                    email
                };

                let _ = Command::new(&git_bug)
                    .args([
                        "user",
                        "new",
                        "-n",
                        &user_name,
                        "-e",
                        &user_email,
                        "--non-interactive",
                    ])
                    .current_dir(target_dir)
                    .output();
            }
        }
    };

    match sub.as_str() {
        "intake" => {
            let mut input: Option<String> = None;
            while let Some(arg) = args.next() {
                match arg.as_str() {
                    "--input" => {
                        input = Some(args.next().unwrap_or_else(|| fail("MISSING_INPUT_ARG")));
                    }
                    _ => fail(format!("UNKNOWN_BUG_INTAKE_OPTION:{}", arg)),
                }
            }
            let input =
                input.unwrap_or_else(|| fail("USAGE: appsdk bug intake --input <json|json-file>"));
            bug_intake(&git_bug, root, &input, &ensure_identity);
        }
        "new" => {
            let mut title: Option<String> = None;
            let mut message: Option<String> = None;
            let mut labels: Vec<String> = Vec::new();
            let mut upstream = false;

            while let Some(arg) = args.next() {
                match arg.as_str() {
                    "-t" | "--title" => {
                        title = Some(args.next().unwrap_or_else(|| fail("MISSING_TITLE_ARG")));
                    }
                    "-m" | "--message" => {
                        message = Some(args.next().unwrap_or_else(|| fail("MISSING_MESSAGE_ARG")));
                    }
                    "-l" | "--label" => {
                        let l = args.next().unwrap_or_else(|| fail("MISSING_LABEL_ARG"));
                        for item in l.split(',') {
                            let trimmed = item.trim();
                            if !trimmed.is_empty() {
                                labels.push(trimmed.to_string());
                            }
                        }
                    }
                    "--upstream" => {
                        upstream = true;
                    }
                    _ => fail(format!("UNKNOWN_BUG_NEW_OPTION:{}", arg)),
                }
            }

            let title_str = title.unwrap_or_else(|| {
                fail(
                    "USAGE: appsdk bug new -t <title> -m <message> [--label <labels>] [--upstream]",
                )
            });
            let message_str = message.unwrap_or_else(|| "".to_string());

            let work_dir = if upstream {
                resolve_upstream_repo().unwrap_or_else(|error| fail(error))
            } else {
                root.to_path_buf()
            };

            ensure_identity(&work_dir);

            let mut cmd = Command::new(&git_bug);
            cmd.args([
                "bug",
                "new",
                "-t",
                &title_str,
                "-m",
                &message_str,
                "--non-interactive",
            ]);
            cmd.current_dir(&work_dir);

            let output = cmd
                .output()
                .unwrap_or_else(|_| fail("GIT_BUG_EXECUTION_FAILED"));
            if !output.status.success() {
                let err = String::from_utf8_lossy(&output.stderr);
                fail(format!("GIT_BUG_NEW_FAILED:{}", err.trim()));
            }
            let out_str = String::from_utf8_lossy(&output.stdout).trim().to_string();
            let bug_id = out_str
                .lines()
                .next()
                .and_then(|line| {
                    line.split_whitespace()
                        .find(|part| part.chars().all(|c| c.is_ascii_hexdigit()) && part.len() >= 7)
                })
                .map(|s| s.to_string())
                .unwrap_or_else(|| out_str.clone());

            for l in &labels {
                let _ = Command::new(&git_bug)
                    .args(["bug", "label", "new", &bug_id, l])
                    .current_dir(&work_dir)
                    .output();
            }

            println!(
                "{{\"ok\":true,\"id\":\"{}\",\"title\":\"{}\",\"labels\":{:?},\"upstream\":{}}}",
                bug_id, title_str, labels, upstream
            );
        }
        "list" | "ls" => {
            let mut status: Option<String> = None;
            let mut labels: Vec<String> = Vec::new();
            let mut sort_by: Option<String> = None;
            let mut direction: Option<String> = None;
            let mut author: Option<String> = None;
            let mut participant: Option<String> = None;
            let mut query: Option<String> = None;
            let mut format_json = false;
            let mut upstream = false;

            while let Some(arg) = args.next() {
                match arg.as_str() {
                    "-s" | "--status" => {
                        status = Some(args.next().unwrap_or_else(|| fail("MISSING_STATUS_ARG")));
                    }
                    "-l" | "--label" => {
                        let l = args.next().unwrap_or_else(|| fail("MISSING_LABEL_ARG"));
                        for item in l.split(',') {
                            let trimmed = item.trim();
                            if !trimmed.is_empty() {
                                labels.push(trimmed.to_string());
                            }
                        }
                    }
                    "-b" | "--by" | "--sort" => {
                        sort_by = Some(args.next().unwrap_or_else(|| fail("MISSING_SORT_ARG")));
                    }
                    "-d" | "--direction" => {
                        direction =
                            Some(args.next().unwrap_or_else(|| fail("MISSING_DIRECTION_ARG")));
                    }
                    "-a" | "--author" => {
                        author = Some(args.next().unwrap_or_else(|| fail("MISSING_AUTHOR_ARG")));
                    }
                    "-p" | "--participant" => {
                        participant = Some(
                            args.next()
                                .unwrap_or_else(|| fail("MISSING_PARTICIPANT_ARG")),
                        );
                    }
                    "-q" | "--query" => {
                        query = Some(args.next().unwrap_or_else(|| fail("MISSING_QUERY_ARG")));
                    }
                    "-f" => {
                        let f = args.next().unwrap_or_else(|| fail("MISSING_FORMAT_ARG"));
                        if f == "json" {
                            format_json = true;
                        }
                    }
                    "--json" => {
                        format_json = true;
                    }
                    "--upstream" => {
                        upstream = true;
                    }
                    _ => fail(format!("UNKNOWN_BUG_LIST_OPTION:{}", arg)),
                }
            }

            let work_dir = select_bug_store(root, upstream).unwrap_or_else(|error| fail(error));

            let build_cmd = |dir: &Path| {
                let mut cmd = Command::new(&git_bug);
                cmd.arg("bug");
                if let Some(ref q) = query {
                    cmd.arg(q);
                }
                if let Some(ref s) = status {
                    cmd.args(["--status", s]);
                }
                for l in &labels {
                    cmd.args(["--label", l]);
                }
                if let Some(ref b) = sort_by {
                    cmd.args(["--by", b]);
                }
                if let Some(ref d) = direction {
                    cmd.args(["--direction", d]);
                }
                if let Some(ref a) = author {
                    cmd.args(["--author", a]);
                }
                if let Some(ref p) = participant {
                    cmd.args(["--participant", p]);
                }
                if format_json {
                    cmd.args(["-f", "json"]);
                }
                cmd.current_dir(dir);
                cmd
            };

            let output = run_git_bug_read(|| build_cmd(&work_dir).output(), format_json)
                .unwrap_or_else(|_| fail("GIT_BUG_EXECUTION_FAILED"));

            if !output.status.success() {
                let err = String::from_utf8_lossy(&output.stderr);
                fail(format!("GIT_BUG_LIST_FAILED:{}", err.trim()));
            }
            let out_str = String::from_utf8_lossy(&output.stdout);
            print!("{}", out_str);
        }
        "show" => {
            let bug_id = args
                .next()
                .unwrap_or_else(|| fail("USAGE: appsdk bug show <id> [--json] [--upstream]"));
            let mut format_json = false;
            let mut upstream = false;
            while let Some(arg) = args.next() {
                match arg.as_str() {
                    "-f" => {
                        if args.next().as_deref() == Some("json") {
                            format_json = true;
                        }
                    }
                    "--json" => format_json = true,
                    "--upstream" => upstream = true,
                    _ => {}
                }
            }
            let work_dir = select_bug_store(root, upstream).unwrap_or_else(|error| fail(error));

            let run_show = |dir: &Path| {
                let mut cmd = Command::new(&git_bug);
                cmd.args(["bug", "show", &bug_id]);
                if format_json {
                    cmd.args(["-f", "json"]);
                }
                cmd.current_dir(dir);
                cmd.output()
            };

            let output = run_git_bug_read(|| run_show(&work_dir), format_json)
                .unwrap_or_else(|_| fail("GIT_BUG_EXECUTION_FAILED"));

            if !output.status.success() {
                let err = String::from_utf8_lossy(&output.stderr);
                fail(format!("GIT_BUG_SHOW_FAILED:{}", err.trim()));
            }
            if format_json {
                let mut record: Value = serde_json::from_slice(&output.stdout)
                    .unwrap_or_else(|_| fail("GIT_BUG_SHOW_INVALID_JSON"));
                if let Some(close_event) = git_bug_close_event(&work_dir, &record) {
                    record["close_event"] = close_event;
                }
                println!("{}", serde_json::to_string_pretty(&record).unwrap());
            } else {
                print!("{}", String::from_utf8_lossy(&output.stdout));
            }
        }
        "comment" => {
            let bug_id = args.next().unwrap_or_else(|| {
                fail("USAGE: appsdk bug comment <id> [-m] <message> [--upstream]")
            });
            let mut msg: Option<String> = None;
            let mut upstream = false;
            while let Some(arg) = args.next() {
                match arg.as_str() {
                    "-m" | "--message" => {
                        msg = Some(
                            args.next()
                                .unwrap_or_else(|| fail("MISSING_COMMENT_MESSAGE")),
                        );
                    }
                    "--upstream" => {
                        upstream = true;
                    }
                    other => {
                        if msg.is_none() {
                            msg = Some(other.to_string());
                        }
                    }
                }
            }
            let message = msg.unwrap_or_else(|| {
                fail("USAGE: appsdk bug comment <id> [-m] <message> [--upstream]")
            });
            let work_dir = select_bug_store(root, upstream).unwrap_or_else(|error| fail(error));

            let run_comment = |dir: &Path| {
                ensure_identity(dir);
                let mut cmd = Command::new(&git_bug);
                cmd.args(["bug", "comment", "new", &bug_id, "-m", &message]);
                cmd.current_dir(dir);
                cmd.output()
            };

            let output =
                run_comment(&work_dir).unwrap_or_else(|_| fail("GIT_BUG_EXECUTION_FAILED"));
            if !output.status.success() {
                let err = String::from_utf8_lossy(&output.stderr);
                fail(format!("GIT_BUG_COMMENT_FAILED:{}", err.trim()));
            }
            print!("{}", String::from_utf8_lossy(&output.stdout));
        }
        "close" => {
            let bug_id = args.next().unwrap_or_else(|| {
                fail(
                    "USAGE: appsdk bug close <id> [-m <solution>] [--receipt-id <id>] [--upstream]",
                )
            });
            let mut receipt_id: Option<String> = None;
            let mut solution: Option<String> = None;
            let mut upstream = false;

            while let Some(arg) = args.next() {
                match arg.as_str() {
                    "--receipt-id" => {
                        receipt_id = Some(
                            args.next()
                                .unwrap_or_else(|| fail("MISSING_RECEIPT_ID_ARG")),
                        );
                    }
                    "-m" | "--message" | "--solution" => {
                        solution =
                            Some(args.next().unwrap_or_else(|| fail("MISSING_SOLUTION_ARG")));
                    }
                    "--upstream" => {
                        upstream = true;
                    }
                    _ => fail(format!("UNKNOWN_BUG_CLOSE_OPTION:{}", arg)),
                }
            }
            let solution = solution.unwrap_or_else(|| {
                fail("USAGE: appsdk bug close <id> -m <solution> [--receipt-id <id>] [--upstream]")
            });
            let close_notes = {
                let mut notes = Vec::new();
                notes.push(format!("### Solution / Resolution\n{}", solution));
                if let Some(r_id) = receipt_id {
                    notes.push(format!("### Mainline Receipt\n{}", r_id));
                }
                notes
            };

            let target_dir = if upstream {
                resolve_upstream_repo().unwrap_or_else(|error| fail(error))
            } else {
                root.to_path_buf()
            };

            let run_close = |dir: &Path| -> Result<(), String> {
                ensure_identity(dir);
                if !close_notes.is_empty() {
                    let msg = close_notes.join("\n\n");
                    let mut cmd = Command::new(&git_bug);
                    cmd.args(["bug", "comment", "new", &bug_id, "-m", &msg]);
                    cmd.current_dir(dir);
                    let output = cmd
                        .output()
                        .map_err(|_| "GIT_BUG_EXECUTION_FAILED".to_string())?;
                    if !output.status.success() {
                        let err = String::from_utf8_lossy(&output.stderr);
                        return Err(format!("GIT_BUG_COMMENT_FAILED:{}", err.trim()));
                    }
                }

                let mut cmd = Command::new(&git_bug);
                cmd.args(["bug", "status", "close", &bug_id]);
                cmd.current_dir(dir);

                let output = cmd
                    .output()
                    .map_err(|_| "GIT_BUG_EXECUTION_FAILED".to_string())?;
                if !output.status.success() {
                    let err = String::from_utf8_lossy(&output.stderr);
                    return Err(format!("GIT_BUG_CLOSE_FAILED:{}", err.trim()));
                }
                Ok(())
            };

            match run_close(&target_dir) {
                Ok(_) => {
                    println!(
                        "{{\"ok\":true,\"bug_id\":\"{}\",\"status\":\"closed\"}}",
                        bug_id
                    );
                }
                Err(e) => {
                    fail(e);
                }
            }
        }
        "webui" => {
            let mut port: Option<String> = None;
            while let Some(arg) = args.next() {
                match arg.as_str() {
                    "-p" | "--port" => {
                        port = Some(args.next().unwrap_or_else(|| fail("MISSING_PORT_ARG")));
                    }
                    _ => fail(format!("UNKNOWN_WEBUI_OPTION:{}", arg)),
                }
            }
            let mut cmd = Command::new(&git_bug);
            cmd.arg("webui");
            if let Some(p) = port {
                cmd.args(["--port", &p]);
            }
            cmd.current_dir(root);
            println!("Launching git-bug webui for {} ...", root.display());
            let _ = cmd
                .status()
                .unwrap_or_else(|_| fail("GIT_BUG_WEBUI_FAILED"));
        }
        _ => fail(format!("UNKNOWN_BUG_SUBCOMMAND:{}", sub)),
    }
}
