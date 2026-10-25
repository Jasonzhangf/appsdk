//! Managed guidance block for a CEO workspace `<workspace>/AGENTS.md`.
//!
//! The caller authenticates the current typed global CEO grant before calling this
//! module. Writing this file grants no authority. The block only points an active
//! CEO session at the central Collab Skill and the existing user preference owner,
//! so no second profile or copied personal content is created here.

use std::error::Error;
use std::fmt;
use std::fs;
use std::fs::OpenOptions;
use std::io;
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

pub const MANAGED_BLOCK_START: &str = "<!-- collab:ceo-workspace-guidance:start -->";
pub const MANAGED_BLOCK_END: &str = "<!-- collab:ceo-workspace-guidance:end -->";

const TARGET_FILE_NAME: &str = "AGENTS.md";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GuidanceAction {
    Created,
    Changed,
    Noop,
}

impl GuidanceAction {
    pub fn as_str(self) -> &'static str {
        match self {
            GuidanceAction::Created => "created",
            GuidanceAction::Changed => "changed",
            GuidanceAction::Noop => "noop",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CeoWorkspaceGuidanceReceipt {
    pub path: PathBuf,
    pub action: GuidanceAction,
}

#[derive(Debug)]
pub enum CeoWorkspaceGuidanceError {
    WorkspaceMissing {
        path: PathBuf,
    },
    InvalidTarget {
        path: PathBuf,
        reason: &'static str,
    },
    MalformedManagedBlock {
        path: PathBuf,
        reason: &'static str,
    },
    NotUtf8 {
        path: PathBuf,
    },
    Io {
        path: PathBuf,
        action: &'static str,
        source: io::Error,
    },
}

impl fmt::Display for CeoWorkspaceGuidanceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CeoWorkspaceGuidanceError::WorkspaceMissing { path } => write!(
                formatter,
                "CEO workspace is not an existing directory: {}",
                path.display()
            ),
            CeoWorkspaceGuidanceError::InvalidTarget { path, reason } => write!(
                formatter,
                "refusing CEO workspace guidance target {}: {reason}",
                path.display()
            ),
            CeoWorkspaceGuidanceError::MalformedManagedBlock { path, reason } => write!(
                formatter,
                "malformed managed guidance block in {}: {reason}",
                path.display()
            ),
            CeoWorkspaceGuidanceError::NotUtf8 { path } => write!(
                formatter,
                "CEO workspace guidance target is not valid UTF-8: {}",
                path.display()
            ),
            CeoWorkspaceGuidanceError::Io {
                path,
                action,
                source,
            } => write!(formatter, "{action} {}: {source}", path.display()),
        }
    }
}

impl Error for CeoWorkspaceGuidanceError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            CeoWorkspaceGuidanceError::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}

struct Guidance {
    body: String,
    block: String,
}

/// Ensure `<workspace>/AGENTS.md` carries the managed CEO guidance block.
pub fn ensure_ceo_workspace_guidance(
    workspace: &Path,
    preferences_source: &Path,
    collab_skill_source: &Path,
) -> Result<CeoWorkspaceGuidanceReceipt, CeoWorkspaceGuidanceError> {
    ensure_workspace_directory(workspace)?;
    let target = workspace.join(TARGET_FILE_NAME);
    let guidance = guidance_content(preferences_source, collab_skill_source);

    let metadata = match fs::symlink_metadata(&target) {
        Ok(metadata) => metadata,
        Err(source) if source.kind() == io::ErrorKind::NotFound => {
            create_target(&target, &guidance.block)?;
            return Ok(receipt(target, GuidanceAction::Created));
        }
        Err(source) => {
            return Err(CeoWorkspaceGuidanceError::Io {
                path: target,
                action: "inspect",
                source,
            })
        }
    };

    if metadata.file_type().is_symlink() {
        return Err(CeoWorkspaceGuidanceError::InvalidTarget {
            path: target,
            reason: "symbolic links are not rewritten",
        });
    }
    if metadata.is_dir() {
        return Err(CeoWorkspaceGuidanceError::InvalidTarget {
            path: target,
            reason: "directories are not guidance files",
        });
    }
    if !metadata.is_file() {
        return Err(CeoWorkspaceGuidanceError::InvalidTarget {
            path: target,
            reason: "target is not a regular file",
        });
    }

    let bytes = fs::read(&target).map_err(|source| CeoWorkspaceGuidanceError::Io {
        path: target.clone(),
        action: "read",
        source,
    })?;
    let content = String::from_utf8(bytes).map_err(|_| CeoWorkspaceGuidanceError::NotUtf8 {
        path: target.clone(),
    })?;

    match merge_managed_block(&target, &content, &guidance)? {
        None => Ok(receipt(target, GuidanceAction::Noop)),
        Some(updated) => {
            replace_target(&target, &updated, metadata.permissions())?;
            Ok(receipt(target, GuidanceAction::Changed))
        }
    }
}

fn receipt(path: PathBuf, action: GuidanceAction) -> CeoWorkspaceGuidanceReceipt {
    CeoWorkspaceGuidanceReceipt { path, action }
}

fn ensure_workspace_directory(workspace: &Path) -> Result<(), CeoWorkspaceGuidanceError> {
    match fs::metadata(workspace) {
        Ok(metadata) if metadata.is_dir() => Ok(()),
        Ok(_) => Err(CeoWorkspaceGuidanceError::WorkspaceMissing {
            path: workspace.to_path_buf(),
        }),
        Err(source) if source.kind() == io::ErrorKind::NotFound => {
            Err(CeoWorkspaceGuidanceError::WorkspaceMissing {
                path: workspace.to_path_buf(),
            })
        }
        Err(source) => Err(CeoWorkspaceGuidanceError::Io {
            path: workspace.to_path_buf(),
            action: "inspect workspace",
            source,
        }),
    }
}

/// Create a target that was absent a moment ago. Exclusive creation refuses to
/// overwrite a file that appeared in between. A partial write removes only the
/// file this call just created.
fn create_target(target: &Path, content: &str) -> Result<(), CeoWorkspaceGuidanceError> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(target)
        .map_err(|source| CeoWorkspaceGuidanceError::Io {
            path: target.to_path_buf(),
            action: "create",
            source,
        })?;
    if let Err(source) = file.write_all(content.as_bytes()) {
        drop(file);
        let _ = fs::remove_file(target);
        return Err(CeoWorkspaceGuidanceError::Io {
            path: target.to_path_buf(),
            action: "write",
            source,
        });
    }
    Ok(())
}

/// Replace an existing target atomically. The complete content goes to a new
/// same-directory temporary file, then a rename swaps it in. The temporary is
/// created private at 0600 before any content is written, so an interrupted
/// write cannot leave a world-readable copy of private content, and the
/// original permissions are restored before the rename. Every error before the
/// rename leaves the original file untouched. Cleanup removes the temporary
/// only when this call created it; a preexisting file at the temporary path is
/// the caller's and is never removed.
fn replace_target(
    target: &Path,
    content: &str,
    original_permissions: fs::Permissions,
) -> Result<(), CeoWorkspaceGuidanceError> {
    let temporary = temporary_sibling(target);
    let mut created = false;
    let written = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temporary)
            .map_err(|source| CeoWorkspaceGuidanceError::Io {
                path: temporary.clone(),
                action: "create temporary",
                source,
            })?;
        created = true;
        file.write_all(content.as_bytes())
            .map_err(|source| CeoWorkspaceGuidanceError::Io {
                path: temporary.clone(),
                action: "write temporary",
                source,
            })?;
        file.set_permissions(original_permissions)
            .map_err(|source| CeoWorkspaceGuidanceError::Io {
                path: temporary.clone(),
                action: "preserve permissions",
                source,
            })?;
        drop(file);
        fs::rename(&temporary, target).map_err(|source| CeoWorkspaceGuidanceError::Io {
            path: target.to_path_buf(),
            action: "replace",
            source,
        })
    })();

    if written.is_err() && created {
        let _ = fs::remove_file(&temporary);
    }
    written
}

fn temporary_sibling(target: &Path) -> PathBuf {
    let mut name = target.file_name().unwrap_or_default().to_os_string();
    name.push(format!(".collab-tmp-{}", std::process::id()));
    target.with_file_name(name)
}

/// Returns the updated file content, or `None` when the canonical block is already
/// present. Existing text outside the markers is preserved byte for byte.
fn merge_managed_block(
    target: &Path,
    content: &str,
    guidance: &Guidance,
) -> Result<Option<String>, CeoWorkspaceGuidanceError> {
    let spans = line_spans(content);
    let mut starts = Vec::new();
    let mut ends = Vec::new();
    for (index, (begin, end)) in spans.iter().copied().enumerate() {
        let line = content[begin..end].trim_end_matches(['\r', '\n']).trim();
        if line == MANAGED_BLOCK_START {
            starts.push(index);
        } else if line == MANAGED_BLOCK_END {
            ends.push(index);
        }
    }

    let malformed = |reason| CeoWorkspaceGuidanceError::MalformedManagedBlock {
        path: target.to_path_buf(),
        reason,
    };

    if starts.len() > 1 || ends.len() > 1 {
        return Err(malformed("more than one managed guidance block marker"));
    }
    match (starts.first(), ends.first()) {
        (None, None) => Ok(Some(append_managed_block(content, &guidance.block))),
        (Some(_), None) => Err(malformed("start marker without end marker")),
        (None, Some(_)) => Err(malformed("end marker without start marker")),
        (Some(&start), Some(&end)) if end < start => {
            Err(malformed("end marker precedes start marker"))
        }
        (Some(&start), Some(&end)) => {
            let start_span = spans[start];
            let end_span = spans[end];
            if &content[start_span.1..end_span.0] == guidance.body {
                return Ok(None);
            }
            Ok(Some(format!(
                "{}{}{}",
                &content[..start_span.0],
                guidance.block,
                &content[end_span.1..]
            )))
        }
    }
}

fn append_managed_block(content: &str, block: &str) -> String {
    let mut updated = String::with_capacity(content.len() + block.len() + 2);
    updated.push_str(content);
    if !updated.is_empty() {
        if !updated.ends_with('\n') {
            updated.push('\n');
        }
        if !updated.ends_with("\n\n") {
            updated.push('\n');
        }
    }
    updated.push_str(block);
    updated
}

fn line_spans(content: &str) -> Vec<(usize, usize)> {
    let mut spans = Vec::new();
    let mut start = 0;
    for (index, byte) in content.bytes().enumerate() {
        if byte == b'\n' {
            spans.push((start, index + 1));
            start = index + 1;
        }
    }
    if start < content.len() {
        spans.push((start, content.len()));
    }
    spans
}

fn guidance_content(preferences_source: &Path, collab_skill_source: &Path) -> Guidance {
    let preferences = preferences_source.display();
    let collab_skill = collab_skill_source.display();
    let lines = vec![
        "## CEO coordination in this workspace".to_owned(),
        String::new(),
        "This block is managed by Collab. Text outside it belongs to the user and is never rewritten."
            .to_owned(),
        String::new(),
        "- Act as CEO only when the current typed global CEO grant names this actor and this workspace."
            .to_owned(),
        "  A workspace location or this text never grants authority, including ordinary chats opened"
            .to_owned(),
        "  in this directory or in its home subdirectories.".to_owned(),
        format!(
            "- Follow the central Collab Skill at `{collab_skill}` for cross-project reporting, patrol,"
        ),
        "  and escalation duties. Default nonurgent behavior is one digest per hour, with quiet hours"
            .to_owned(),
        "  from 00:00 to 08:00 Beijing time. The typed CEO config wins over these defaults, and typed"
            .to_owned(),
        "  urgent work is explicit and separate from the digest.".to_owned(),
        format!(
            "- Before using user style or preferences, read the existing preference source at `{preferences}`."
        ),
        "  That file is the single authority. Accumulate a stable, user-confirmed preference or style with"
            .to_owned(),
        "  its provenance, following that file's own sync contract.".to_owned(),
        "- Do not infer lasting preferences from one emotion or one task, do not copy project status into"
            .to_owned(),
        "  preferences, do not store secrets, do not overwrite user rules, and do not invent profile facts."
            .to_owned(),
        "  Do not create a second profile.".to_owned(),
    ];
    let body = format!("{}\n", lines.join("\n"));
    let block = format!("{MANAGED_BLOCK_START}\n{body}{MANAGED_BLOCK_END}\n");
    Guidance { body, block }
}
