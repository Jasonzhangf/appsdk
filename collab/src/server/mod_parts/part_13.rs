fn resolve_authoritative_main_head(root: &Path) -> Result<String, Resp> {
    let output = Command::new("git")
        .current_dir(root)
        .args(["rev-parse", "--verify", "refs/heads/main^{commit}"])
        .output();
    match output {
        Ok(output) if output.status.success() => {
            let head = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if head.is_empty() {
                Err(Resp::err_data(
                    "TASK_INTEGRATION_MAIN_UNRESOLVED",
                    json!({"root": root, "ref": "refs/heads/main"}),
                ))
            } else {
                Ok(head)
            }
        }
        Ok(output) => {
            let dirty = Command::new("git")
                .current_dir(root)
                .args(["status", "--porcelain", "--untracked-files=all"])
                .output()
                .map(|status| status.status.success() && !status.stdout.is_empty())
                .unwrap_or(false);
            Err(Resp::err_data(
                "TASK_INTEGRATION_MAIN_UNRESOLVED",
                json!({
                    "root": root,
                    "dirty": dirty,
                    "ref": "refs/heads/main",
                    "detail": String::from_utf8_lossy(&output.stderr).trim(),
                }),
            ))
        }
        Err(error) => Err(Resp::err_data(
            "TASK_INTEGRATION_MAIN_UNRESOLVED",
            json!({"root": root, "ref": "refs/heads/main", "detail": error.to_string()}),
        )),
    }
}

/// Integration records accept the main tip, a real merge commit, or the
/// merged candidate SHA itself. The authoritative question is reachability
/// from `refs/heads/main`, not string equality with its current tip, because
/// the daemon root may be checked out on a different branch while the main
/// worktree advances elsewhere.
fn commit_is_integrated_in_main(root: &Path, commit: &str) -> Result<bool, Resp> {
    let verified = Command::new("git")
        .current_dir(root)
        .args(["rev-parse", "--verify", &format!("{commit}^{{commit}}")])
        .output();
    let verified = match verified {
        Ok(output) if output.status.success() => output,
        Ok(output) => {
            return Err(Resp::err_data(
                "TASK_INTEGRATION_COMMIT_UNRESOLVED",
                json!({
                    "provided": commit,
                    "detail": String::from_utf8_lossy(&output.stderr).trim(),
                    "expected": "a commit reachable from refs/heads/main",
                }),
            ))
        }
        Err(error) => {
            return Err(Resp::err_data(
                "TASK_INTEGRATION_COMMIT_UNRESOLVED",
                json!({
                    "provided": commit,
                    "detail": error.to_string(),
                    "expected": "a commit reachable from refs/heads/main",
                }),
            ))
        }
    };
    let resolved = String::from_utf8_lossy(&verified.stdout).trim().to_string();
    let reachable = Command::new("git")
        .current_dir(root)
        .args(["merge-base", "--is-ancestor", &resolved, "refs/heads/main"])
        .output();
    match reachable {
        Ok(output) if output.status.success() => Ok(true),
        Ok(output) if output.status.code() == Some(1) => Ok(false),
        Ok(output) => Err(Resp::err_data(
            "TASK_INTEGRATION_MAIN_UNRESOLVED",
            json!({
                "provided": commit,
                "ref": "refs/heads/main",
                "detail": String::from_utf8_lossy(&output.stderr).trim(),
            }),
        )),
        Err(error) => Err(Resp::err_data(
            "TASK_INTEGRATION_MAIN_UNRESOLVED",
            json!({
                "provided": commit,
                "ref": "refs/heads/main",
                "detail": error.to_string(),
            }),
        )),
    }
}

