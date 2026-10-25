//! Independent consumer contract for the CEO workspace AGENTS.md guidance helper.
//!
//! This test imports the exact source file with `#[path]` so the filesystem effect
//! and its receipt can be verified before the binary wiring exists. Every case uses
//! its own temporary root; no global or user file is read or written.

#[path = "../src/ceo_workspace_guidance.rs"]
mod ceo_workspace_guidance;

use ceo_workspace_guidance::{
    ensure_ceo_workspace_guidance, CeoWorkspaceGuidanceError, GuidanceAction, MANAGED_BLOCK_END,
    MANAGED_BLOCK_START,
};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static SEQUENCE: AtomicU64 = AtomicU64::new(0);

struct TempRoot {
    path: PathBuf,
}

impl TempRoot {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "ceo-guidance-{}-{}",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).expect("create temp root");
        TempRoot { path }
    }

    fn root(&self) -> &Path {
        &self.path
    }

    fn workspace(&self) -> PathBuf {
        let path = self.path.join("workspace");
        fs::create_dir_all(&path).expect("create workspace");
        path
    }

    fn preferences(&self) -> PathBuf {
        self.path.join("preferences").join("USER.md")
    }

    fn collab_skill(&self) -> PathBuf {
        self.path.join("skills").join("collab").join("SKILL.md")
    }
}

impl Drop for TempRoot {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn marker_count(content: &str, marker: &str) -> usize {
    content.matches(marker).count()
}

fn ensure_invalid_target(err: CeoWorkspaceGuidanceError, label: &str) {
    match err {
        CeoWorkspaceGuidanceError::InvalidTarget { .. } => {}
        other => panic!("{label}: expected InvalidTarget, got {other}"),
    }
}

#[test]
fn creates_agents_file_with_referenced_guidance() {
    let temp = TempRoot::new();
    let workspace = temp.workspace();
    let preferences = temp.preferences();
    let skill = temp.collab_skill();

    let receipt = ensure_ceo_workspace_guidance(&workspace, &preferences, &skill)
        .expect("creates guidance for empty workspace");

    let agents = workspace.join("AGENTS.md");
    assert_eq!(receipt.path, agents);
    assert_eq!(receipt.action, GuidanceAction::Created);
    assert_eq!(receipt.action.as_str(), "created");

    let content = fs::read_to_string(&agents).expect("agents file is readable");
    assert_eq!(marker_count(&content, MANAGED_BLOCK_START), 1, "{content}");
    assert_eq!(marker_count(&content, MANAGED_BLOCK_END), 1, "{content}");
    assert!(
        content.contains(&preferences.display().to_string()),
        "{content}"
    );
    assert!(content.contains(&skill.display().to_string()), "{content}");
}

#[test]
fn guidance_limits_ceo_authority_to_the_typed_grant() {
    let temp = TempRoot::new();
    let workspace = temp.workspace();

    ensure_ceo_workspace_guidance(&workspace, &temp.preferences(), &temp.collab_skill())
        .expect("creates guidance");

    let content = fs::read_to_string(workspace.join("AGENTS.md")).expect("read agents file");
    let lowered = content.to_lowercase();
    assert!(lowered.contains("typed global ceo grant"), "{content}");
    assert!(lowered.contains("never grants authority"), "{content}");
    assert!(lowered.contains("home subdirectories"), "{content}");
    assert!(lowered.contains("00:00 to 08:00"), "{content}");
    assert!(lowered.contains("per hour"), "{content}");
    assert!(lowered.contains("single authority"), "{content}");
    assert!(
        lowered.contains("do not create a second profile"),
        "{content}"
    );
    assert!(
        !content.contains("/Users/"),
        "guidance must not embed personal paths: {content}"
    );
}

#[test]
fn existing_user_text_is_preserved_and_second_call_is_noop() {
    let temp = TempRoot::new();
    let workspace = temp.workspace();
    let agents = workspace.join("AGENTS.md");
    let user_text = "# House rules\n\n- keep commits small\n- ask before deleting data\n";
    fs::write(&agents, user_text).expect("write user agents file");
    let mut mode = fs::metadata(&agents).expect("read user mode").permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut mode, 0o640);
    fs::set_permissions(&agents, mode).expect("set non-default user mode");

    let receipt =
        ensure_ceo_workspace_guidance(&workspace, &temp.preferences(), &temp.collab_skill())
            .expect("appends guidance to existing file");
    assert_eq!(receipt.action, GuidanceAction::Changed);

    let updated = fs::read_to_string(&agents).expect("read updated file");
    assert!(updated.starts_with(user_text), "{updated}");
    assert_eq!(marker_count(&updated, MANAGED_BLOCK_START), 1, "{updated}");
    assert!(updated.trim_end().ends_with(MANAGED_BLOCK_END), "{updated}");
    let updated_mode = std::os::unix::fs::PermissionsExt::mode(
        &fs::metadata(&agents).expect("read mode").permissions(),
    );
    assert_eq!(
        updated_mode & 0o777,
        0o640,
        "rewrite must preserve the original file mode"
    );

    let second =
        ensure_ceo_workspace_guidance(&workspace, &temp.preferences(), &temp.collab_skill())
            .expect("second call succeeds");
    assert_eq!(second.action, GuidanceAction::Noop);
    let after = fs::read_to_string(&agents).expect("read file after second call");
    assert_eq!(after, updated);
    assert_eq!(marker_count(&after, MANAGED_BLOCK_START), 1, "{after}");
}

#[test]
fn existing_managed_block_is_updated_in_place() {
    let temp = TempRoot::new();
    let workspace = temp.workspace();
    let agents = workspace.join("AGENTS.md");
    let stale = format!(
        "# House rules\n\n- keep commits small\n\n{start}\n## stale guidance\nold body\n{end}\n\n## Tail notes\n- after block\n",
        start = MANAGED_BLOCK_START,
        end = MANAGED_BLOCK_END
    );
    fs::write(&agents, &stale).expect("write file with stale managed block");

    let receipt =
        ensure_ceo_workspace_guidance(&workspace, &temp.preferences(), &temp.collab_skill())
            .expect("updates stale managed block");
    assert_eq!(receipt.action, GuidanceAction::Changed);

    let updated = fs::read_to_string(&agents).expect("read updated file");
    assert!(
        updated.starts_with("# House rules\n\n- keep commits small\n\n"),
        "{updated}"
    );
    assert!(
        updated.contains("## Tail notes\n- after block"),
        "{updated}"
    );
    assert!(!updated.contains("old body"), "{updated}");
    assert!(!updated.contains("## stale guidance"), "{updated}");
    assert_eq!(marker_count(&updated, MANAGED_BLOCK_START), 1, "{updated}");

    let second =
        ensure_ceo_workspace_guidance(&workspace, &temp.preferences(), &temp.collab_skill())
            .expect("repeated call succeeds");
    assert_eq!(second.action, GuidanceAction::Noop);
    assert_eq!(
        fs::read_to_string(&agents).expect("read file again"),
        updated
    );
}

#[test]
fn directory_target_is_refused_without_mutation() {
    let temp = TempRoot::new();
    let workspace = temp.workspace();
    let agents = workspace.join("AGENTS.md");
    fs::create_dir(&agents).expect("create directory target");

    let err = ensure_ceo_workspace_guidance(&workspace, &temp.preferences(), &temp.collab_skill())
        .expect_err("directory target is refused");
    ensure_invalid_target(err, "directory target");

    assert!(agents.is_dir(), "target must stay a directory");
    assert_eq!(
        fs::read_dir(&agents)
            .expect("read directory target")
            .count(),
        0,
        "no file may be created inside the directory target"
    );
}

#[test]
fn symlink_target_is_refused_without_touching_link_or_destination() {
    let temp = TempRoot::new();
    let workspace = temp.workspace();
    let shared = temp.root().join("shared-agents.md");
    fs::write(&shared, "# shared global rules\n").expect("write shared file");
    let agents = workspace.join("AGENTS.md");
    std::os::unix::fs::symlink(&shared, &agents).expect("create symlink target");

    let err = ensure_ceo_workspace_guidance(&workspace, &temp.preferences(), &temp.collab_skill())
        .expect_err("symlink target is refused");
    ensure_invalid_target(err, "symlink target");

    let meta = fs::symlink_metadata(&agents).expect("read symlink metadata");
    assert!(meta.file_type().is_symlink(), "target must stay a symlink");
    assert_eq!(
        fs::read_to_string(&shared).expect("read shared file"),
        "# shared global rules\n",
        "symlink destination must not be modified"
    );
}

#[test]
fn malformed_managed_block_is_refused_without_mutation() {
    let temp = TempRoot::new();
    let workspace = temp.workspace();
    let agents = workspace.join("AGENTS.md");
    let broken = format!(
        "# House rules\n\n{start}\nhalf a block\n",
        start = MANAGED_BLOCK_START
    );
    fs::write(&agents, &broken).expect("write file with start marker only");

    let err = ensure_ceo_workspace_guidance(&workspace, &temp.preferences(), &temp.collab_skill())
        .expect_err("unterminated managed block is refused");
    match err {
        CeoWorkspaceGuidanceError::MalformedManagedBlock { .. } => {}
        other => panic!("expected MalformedManagedBlock, got {other}"),
    }
    assert_eq!(
        fs::read_to_string(&agents).expect("read untouched file"),
        broken
    );
}

#[test]
fn io_failure_during_update_keeps_existing_file_untouched() {
    let temp = TempRoot::new();
    let workspace = temp.workspace();
    let agents = workspace.join("AGENTS.md");
    let original = format!("# House rules\n{}\n", "x".repeat(512));
    fs::write(&agents, &original).expect("write original agents file");
    let original_permissions = fs::metadata(&agents)
        .expect("read original mode")
        .permissions();

    let child = std::process::Command::new(std::env::current_exe().expect("test binary path"))
        .arg("--ignored")
        .arg("--exact")
        .arg("fsize_limited_update_child")
        .env("CEO_GUIDANCE_FSIZE_WORKSPACE", &workspace)
        .env("CEO_GUIDANCE_FSIZE_PREFERENCES", temp.preferences())
        .env("CEO_GUIDANCE_FSIZE_SKILL", temp.collab_skill())
        .env("CEO_GUIDANCE_FSIZE_LIMIT", "64")
        .spawn()
        .expect("spawn fsize-limited child");
    let child_pid = child.id();
    let child = child.wait_with_output().expect("wait fsize-limited child");

    assert!(
        !child.status.success(),
        "child process must hit the file size limit"
    );
    assert_eq!(
        fs::read(&agents).expect("read agents file after child"),
        original.as_bytes(),
        "failed update must leave the original file bytes untouched"
    );
    assert_eq!(
        fs::metadata(&agents)
            .expect("read mode after child")
            .permissions(),
        original_permissions,
        "failed update must leave the original file mode untouched"
    );
    let leftover = workspace.join(format!("AGENTS.md.collab-tmp-{}", child_pid));
    if leftover.exists() {
        let leftover_mode = std::os::unix::fs::PermissionsExt::mode(
            &fs::metadata(&leftover)
                .expect("read leftover temporary metadata")
                .permissions(),
        );
        assert_eq!(
            leftover_mode & 0o777,
            0o600,
            "leftover temporary must stay private to the owner"
        );
    }
}

#[test]
fn preexisting_temporary_sibling_is_never_removed() {
    let temp = TempRoot::new();
    let workspace = temp.workspace();
    let agents = workspace.join("AGENTS.md");
    let original = "# House rules\n";
    fs::write(&agents, original).expect("write original agents file");

    let sentinel = workspace.join(format!("AGENTS.md.collab-tmp-{}", std::process::id()));
    let sentinel_content = "sentinel owned by the caller\n";
    fs::write(&sentinel, sentinel_content).expect("write preexisting temporary sibling");

    let err = ensure_ceo_workspace_guidance(&workspace, &temp.preferences(), &temp.collab_skill())
        .expect_err("preexisting temporary sibling must fail exclusive creation");
    match err {
        CeoWorkspaceGuidanceError::Io { action, source, .. } => {
            assert_eq!(action, "create temporary");
            assert_eq!(source.kind(), io::ErrorKind::AlreadyExists);
        }
        other => panic!("expected Io, got {other}"),
    }

    assert_eq!(
        fs::read_to_string(&agents).expect("read original target"),
        original,
        "original target must stay untouched"
    );
    assert_eq!(
        fs::read_to_string(&sentinel).expect("read preexisting temporary"),
        sentinel_content,
        "a file this call could not create must never be removed"
    );
}

#[test]
#[ignore]
fn fsize_limited_update_child() {
    let workspace =
        PathBuf::from(std::env::var_os("CEO_GUIDANCE_FSIZE_WORKSPACE").expect("workspace env"));
    let preferences =
        PathBuf::from(std::env::var_os("CEO_GUIDANCE_FSIZE_PREFERENCES").expect("preferences env"));
    let skill =
        PathBuf::from(std::env::var_os("CEO_GUIDANCE_FSIZE_SKILL").expect("collab skill env"));
    let limit: u64 = std::env::var("CEO_GUIDANCE_FSIZE_LIMIT")
        .expect("limit env")
        .parse()
        .expect("numeric limit");
    let rlimit = libc::rlimit {
        rlim_cur: limit,
        rlim_max: limit,
    };
    let rc = unsafe { libc::setrlimit(libc::RLIMIT_FSIZE, &rlimit) };
    assert_eq!(rc, 0, "setrlimit failed");

    let _ = ensure_ceo_workspace_guidance(&workspace, &preferences, &skill);
}
