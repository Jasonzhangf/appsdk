#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentState {
    Unknown,
    Working,
    Waiting,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdentityPresence {
    Present,
    /// The durable identity and its App Server address are verified, but the
    /// native thread is not resident on the endpoint right now.  This is a
    /// distinct, honest state: the peer is addressable and will be loaded by
    /// the next immediate notification, so it is not "missing".
    Cold,
    Missing,
    Unknown,
}

pub fn append_log(path: &std::path::Path, text: &str) {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        use std::io::Write;
        let _ = writeln!(file, "{} {}", chrono::Utc::now().to_rfc3339(), text);
    }
}
