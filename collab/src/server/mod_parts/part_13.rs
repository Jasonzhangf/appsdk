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

fn open_git_repository(root: &Path) -> Result<gix::Repository, Resp> {
    gix::open(root).map_err(|error| {
        Resp::err_data(
            "TASK_INTEGRATION_MAIN_UNRESOLVED",
            json!({"root": root, "detail": error.to_string()}),
        )
    })
}

/// Debug-only fault-injection seam. CLI fixtures hold the in-process
/// integration resolution window by creating `held-integration-resolution`
/// under the project `.agent-collab` dir; the seam records entry and waits
/// until the fixture removes it. Compiled out of release builds.
#[cfg(debug_assertions)]
fn held_integration_resolution_test_seam(root: &Path) {
    let dir = root.join(".agent-collab");
    let hold = dir.join("held-integration-resolution");
    if !hold.is_file() {
        return;
    }
    let entered = dir.join("held-integration-resolution-entered");
    let _ = std::fs::write(&entered, "entered");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while hold.is_file() && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let _ = std::fs::remove_file(&entered);
}

fn resolve_authoritative_main_head(root: &Path) -> Result<String, Resp> {
    let repository = open_git_repository(root)?;
    let head = repository
        .rev_parse_single("refs/heads/main^{commit}")
        .map_err(|error| {
            Resp::err_data(
                "TASK_INTEGRATION_MAIN_UNRESOLVED",
                json!({
                    "root": root,
                    "ref": "refs/heads/main",
                    "detail": error.to_string(),
                }),
            )
        })?;
    Ok(head.detach().to_string())
}
/// Integration records accept the main tip, a real merge commit, or the
/// merged candidate SHA itself. The authoritative question is reachability
/// from `refs/heads/main`, not string equality with its current tip, because
/// the daemon root may be checked out on a different branch while the main
/// worktree advances elsewhere.
fn commit_is_integrated_in_main(root: &Path, commit: &str) -> Result<bool, Resp> {
    let repository = open_git_repository(root)?;
    let commit_spec = format!("{commit}^{{commit}}");
    let verified = repository
        .rev_parse_single(commit_spec.as_str())
        .map_err(|error| {
            Resp::err_data(
                "TASK_INTEGRATION_COMMIT_UNRESOLVED",
                json!({
                    "provided": commit,
                    "detail": error.to_string(),
                    "expected": "a commit reachable from refs/heads/main",
                }),
            )
        })?
        .detach();
    repository.find_commit(verified).map_err(|error| {
        Resp::err_data(
            "TASK_INTEGRATION_COMMIT_UNRESOLVED",
            json!({
                "provided": commit,
                "detail": error.to_string(),
                "expected": "a commit reachable from refs/heads/main",
            }),
        )
    })?;
    let main = repository
        .rev_parse_single("refs/heads/main^{commit}")
        .map_err(|error| {
            Resp::err_data(
                "TASK_INTEGRATION_MAIN_UNRESOLVED",
                json!({
                    "provided": commit,
                    "ref": "refs/heads/main",
                    "detail": error.to_string(),
                }),
            )
        })?
        .detach();
    let walk = repository
        .find_commit(main)
        .map_err(|error| {
            Resp::err_data(
                "TASK_INTEGRATION_MAIN_UNRESOLVED",
                json!({
                    "provided": commit,
                    "ref": "refs/heads/main",
                    "detail": error.to_string(),
                }),
            )
        })?
        .ancestors()
        .all()
        .map_err(|error| {
            Resp::err_data(
                "TASK_INTEGRATION_MAIN_UNRESOLVED",
                json!({
                    "provided": commit,
                    "ref": "refs/heads/main",
                    "detail": error.to_string(),
                }),
            )
        })?;
    for ancestor in walk {
        let ancestor = ancestor.map_err(|error| {
            Resp::err_data(
                "TASK_INTEGRATION_MAIN_UNRESOLVED",
                json!({
                    "provided": commit,
                    "ref": "refs/heads/main",
                    "detail": error.to_string(),
                }),
            )
        })?;
        if ancestor.id == verified {
            return Ok(true);
        }
    }
    Ok(false)
}
