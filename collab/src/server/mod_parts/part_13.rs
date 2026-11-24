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
    let main = resolve_main_commit(&repository)?;
    Ok(main.id.to_string())
}

fn resolve_main_commit(repo: &gix::Repository) -> Result<gix::Commit<'_>, Resp> {
    resolve_exact_local_ref(repo, "refs/heads/main").map_err(|detail| {
        Resp::err_data(
            "TASK_INTEGRATION_MAIN_UNRESOLVED",
            json!({"ref": "refs/heads/main", "detail": detail}),
        )
    })
}

fn resolve_exact_local_ref<'repo>(
    repo: &'repo gix::Repository,
    ref_name: &str,
) -> Result<gix::Commit<'repo>, String> {
    let mut reference = repo
        .find_reference(ref_name)
        .map_err(|error| format!("cannot resolve {ref_name}: {error}"))?;
    if reference.name().as_bstr() != ref_name {
        return Err(format!("{ref_name} did not resolve as an exact local ref"));
    }
    reference
        .peel_to_commit()
        .map_err(|error| format!("cannot read {ref_name} as a commit: {error}"))
}

fn resolve_integration_commit(repo: &gix::Repository, commit: &str) -> Result<gix::hash::ObjectId, Resp> {
    let error = |detail: String| Resp::err_data(
        "TASK_INTEGRATION_COMMIT_UNRESOLVED",
        json!({"provided": commit, "detail": detail, "expected": "a commit reachable from refs/heads/main"}),
    );
    let id = repo
        .rev_parse_single(format!("{commit}^{{commit}}").as_str())
        .map_err(|err| error(err.to_string()))?
        .detach();
    repo.find_commit(id).map_err(|err| error(err.to_string()))?;
    Ok(id)
}

fn integration_commit_reachable(
    main: &gix::Commit<'_>,
    receipt: gix::hash::ObjectId,
) -> Result<bool, Resp> {
    let error = |detail: String| Resp::err_data(
        "TASK_INTEGRATION_MAIN_UNRESOLVED",
        json!({"ref": "refs/heads/main", "detail": detail}),
    );
    for ancestor in main.ancestors().all().map_err(|err| error(err.to_string()))? {
        if ancestor.map_err(|err| error(err.to_string()))?.id == receipt {
            return Ok(true);
        }
    }
    Ok(false)
}

fn prove_cleanup_branch_landing(
    root: &Path,
    branch_ref: &str,
    integration: Option<&str>,
) -> Result<(), String> {
    let repo = open_git_repository(root).map_err(|err| err.error.unwrap_or_default())?;
    let main = resolve_main_commit(&repo).map_err(|err| err.error.unwrap_or_default())?;
    prove_declared_branch_landing(&main, branch_ref, || {
        let commit = integration
            .filter(|commit| !commit.trim().is_empty())
            .ok_or_else(|| format!("branch {branch_ref} is not merged into refs/heads/main"))?;
        let receipt = resolve_integration_commit(&repo, commit)
            .map_err(|err| err.error.unwrap_or_default())?;
        if !integration_commit_reachable(&main, receipt).map_err(|err| err.error.unwrap_or_default())? {
            return Err(format!("integration commit {commit} is not reachable from refs/heads/main"));
        }
        Ok(receipt)
    })
}

/// A declared branch is on main by ancestry, or its net changes are present
/// in the supplied main-reachable integration commit after a squash or rebase.
/// Read Git objects in process, including for proofs on protected mounts.
fn prove_declared_branch_landing(
    main: &gix::Commit<'_>,
    branch_ref: &str,
    reachable_receipt: impl FnOnce() -> Result<gix::hash::ObjectId, String>,
) -> Result<(), String> {
    let repo = main.repo;
    let branch = resolve_exact_local_ref(repo, branch_ref)?.id;
    let bases = repo
        .merge_bases_many(main.id, &[branch])
        .map_err(|error| format!("cannot verify branch {branch_ref}: {error}"))?;
    if bases.iter().any(|base| base.detach() == branch) {
        return Ok(());
    }
    let receipt = reachable_receipt()?;
    let bases = repo
        .merge_bases_many(branch, &[receipt])
        .map_err(|error| format!("cannot find a common ancestor for {branch_ref} and {receipt}: {error}"))?;
    let [base] = bases.as_slice() else {
        return Err(format!(
            "expected one common ancestor for {branch_ref} and {receipt}, found {}",
            bases.len()
        ));
    };

    let tree_at = |id| -> Result<gix::Tree<'_>, String> {
        let commit = repo.find_commit(id).map_err(|error| error.to_string())?;
        commit.tree().map_err(|error| error.to_string())
    };
    let base_tree = tree_at(base.detach())?;
    let branch_tree = tree_at(branch)?;
    let receipt_tree = tree_at(receipt)?;
    let mut changes = gix::diff::tree::Recorder::default();
    gix::diff::tree(
        gix::objs::TreeRefIter::from_bytes(&base_tree.data),
        gix::objs::TreeRefIter::from_bytes(&branch_tree.data),
        gix::diff::tree::State::default(),
        &repo.objects,
        &mut changes,
    )
    .map_err(|error| format!("cannot read net changed paths for {branch_ref}: {error}"))?;

    use gix::diff::tree::recorder::Change;
    let mut has_net_change = false;
    for change in changes.records {
        let path = match change {
            Change::Addition { entry_mode, path, .. } | Change::Deletion { entry_mode, path, .. }
                if !entry_mode.is_tree() => path,
            Change::Modification { previous_entry_mode, entry_mode, path, .. }
                if !previous_entry_mode.is_tree() || !entry_mode.is_tree() => path,
            _ => continue,
        };
        has_net_change = true;
        if git_tree_path_entry(&branch_tree, &path)? != git_tree_path_entry(&receipt_tree, &path)? {
            return Err(format!(
                "net changed path {} differs between {branch_ref} and integration commit {receipt}",
                gix::bstr::BStr::new(&path)
            ));
        }
    }
    if !has_net_change {
        return Err(format!("branch {branch_ref} has no net changed paths to compare"));
    }
    Ok(())
}

fn git_tree_path_entry(
    tree: &gix::Tree<'_>,
    path: &[u8],
) -> Result<Option<(gix::objs::tree::EntryMode, gix::hash::ObjectId)>, String> {
    let mut components = path.splitn(2, |byte| *byte == b'/');
    let name = gix::bstr::BStr::new(components.next().unwrap_or_default());
    for entry in tree.iter() {
        let entry = entry.map_err(|error| error.to_string())?;
        if entry.filename() != name {
            continue;
        }
        return match components.next() {
            None => Ok(Some((entry.mode(), entry.object_id()))),
            Some(rest) if entry.mode().is_tree() => {
                let subtree = tree.repo.find_tree(entry.object_id()).map_err(|err| err.to_string())?;
                git_tree_path_entry(&subtree, rest)
            }
            Some(_) => Ok(None),
        };
    }
    Ok(None)
}
/// Integration records accept the main tip, a real merge commit, or the
/// merged candidate SHA itself. The authoritative question is reachability
/// from `refs/heads/main`, not string equality with its current tip, because
/// the daemon root may be checked out on a different branch while the main
/// worktree advances elsewhere.
fn commit_is_integrated_in_main(root: &Path, commit: &str) -> Result<bool, Resp> {
    let repo = open_git_repository(root)?;
    let main = resolve_main_commit(&repo)?;
    let receipt = resolve_integration_commit(&repo, commit)?;
    integration_commit_reachable(&main, receipt)
}
