//! Process-local record of which durable observation owners have already
//! produced a live sample since this daemon process started.
//!
//! Durable idle episodes (the typed activity record, the board patrol gate and
//! the standing project-patrol subscription) survive a daemon restart. If the
//! first sample after a restart extended a pre-restart episode, a reminder
//! could fire from idle time this process never observed. The first sample of
//! each owner after start therefore begins a fresh episode, and later samples
//! in the same process keep it.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

fn observed() -> &'static Mutex<HashSet<(PathBuf, String)>> {
    static OBSERVED: OnceLock<Mutex<HashSet<(PathBuf, String)>>> = OnceLock::new();
    OBSERVED.get_or_init(|| Mutex::new(HashSet::new()))
}

/// Returns `true` for the first sample this process takes for `owner` under
/// `root`, and `false` for every later sample.
pub(crate) fn first_observation_since_start(root: &Path, owner: &str) -> bool {
    observed()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .insert((root.to_owned(), owner.to_owned()))
}
