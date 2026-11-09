fn git_dir_for_root(root: &Path) -> std::io::Result<PathBuf> {
    let marker = root.join(".git");
    if marker.is_dir() {
        return Ok(marker);
    }
    if marker.is_file() {
        let text = std::fs::read_to_string(&marker)?;
        let value = text
            .trim()
            .strip_prefix("gitdir:")
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("invalid gitdir marker {}", marker.display()),
                )
            })?;
        let git_dir = PathBuf::from(value);
        let git_dir = if git_dir.is_absolute() {
            git_dir
        } else {
            marker.parent().unwrap_or(root).join(git_dir)
        };
        if git_dir.join("HEAD").is_file() {
            return Ok(git_dir);
        }
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::NotFound,
        format!("no git repository at {}", root.display()),
    ))
}

/// Keep the git child's cwd outside the project mount. A launchd-owned child
/// can block in getcwd when its initial cwd is protected; explicit git-dir and
/// work-tree preserve repository semantics without changing TCC permissions.
fn safe_git_command(root: &Path) -> std::io::Result<Command> {
    let git_dir = git_dir_for_root(root)?;
    let mut command = Command::new("git");
    command.current_dir(Path::new("/"));
    command.arg(format!("--git-dir={}", git_dir.display()));
    command.arg(format!("--work-tree={}", root.display()));
    Ok(command)
}

fn run_safe_git(root: &Path, args: &[&str]) -> std::io::Result<std::process::Output> {
    safe_git_command(root)?.args(args).output()
}

fn resolve_authoritative_main_head(root: &Path) -> Result<String, Resp> {
    let output = run_safe_git(root, &["rev-parse", "--verify", "refs/heads/main^{commit}"]);
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
            let dirty = run_safe_git(
                root,
                &["status", "--porcelain", "--untracked-files=all"],
            )
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
    let verified = run_safe_git(
        root,
        &["rev-parse", "--verify", &format!("{commit}^{{commit}}")],
    );
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
    let reachable = run_safe_git(
        root,
        &["merge-base", "--is-ancestor", &resolved, "refs/heads/main"],
    );
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
