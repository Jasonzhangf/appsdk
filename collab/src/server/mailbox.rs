//! Notification formatting and batch selection over current messages.

use super::state::{Message, State, MAX_WAKE_ATTEMPTS};

const MAX_NOTIFICATION_SUBJECT_CHARS: usize = 48;
const MAX_NOTIFICATION_CHARS: usize = 1024;

pub const MAX_NOTIFICATION_TTL_SECONDS: u64 = 7 * 24 * 60 * 60;
pub const MAX_ACTIVE_SUBSCRIPTIONS_PER_WORKER: usize = 3;
// Keepalive uses this as its idle-episode cadence. Automatic notification
// retry eligibility is event-policy-driven below and must not use this value.
pub(super) const AUTOMATIC_BATCH_WINDOW_MS: i64 = 120_000;
pub const NOTIFICATION_EVENTS: [&str; 4] = [
    "direct-message",
    "resource-released",
    "deadline",
    "master-idle",
];
pub const DEFAULT_DIRECT_MESSAGE_TTL_SECONDS: u64 = MAX_NOTIFICATION_TTL_SECONDS;

pub fn default_direct_message_id(worker_id: &str) -> String {
    format!("sub-default-direct-message-{worker_id}")
}

fn abbreviated_subject(subject: &str) -> Option<String> {
    let normalized = subject.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.is_empty() {
        return None;
    }
    if normalized.chars().count() <= MAX_NOTIFICATION_SUBJECT_CHARS {
        return Some(normalized);
    }
    let mut abbreviated = normalized
        .chars()
        .take(MAX_NOTIFICATION_SUBJECT_CHARS - 1)
        .collect::<String>();
    abbreviated.push('…');
    Some(abbreviated)
}

/// Derive a stable notification title when a content-first sender omits the
/// legacy `--subject` field. The first non-empty line stays recognizable while
/// the complete body remains untouched in the durable message.
pub fn notification_subject_from_body(body: &str) -> Option<String> {
    body.lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .and_then(abbreviated_subject)
}

pub fn visible_body(body: &str) -> String {
    let mut visible = String::with_capacity(body.len());
    for ch in body.chars() {
        match ch {
            '\n' => visible.push_str("\\n"),
            '\r' => visible.push_str("\\r"),
            '\t' => visible.push_str("\\t"),
            ch if ch.is_control() => visible.push_str(&format!("\\u{{{:x}}}", ch as u32)),
            ch => visible.push(ch),
        }
    }
    visible
}

pub fn notification_class(subject: &str, from: &str, mtype: &str) -> (&'static str, &'static str) {
    if subject.starts_with("worker-unresponsive") {
        return (
            "P1",
            "inspect durable tasks and mailbox for this worker; resolve ownership with evidence",
        );
    }
    if subject.starts_with("worker-idle") {
        return ("P1", "dispatch work to this idle capacity");
    }
    if subject.starts_with("master-idle") {
        return ("P1", "run the scheduling pass: inspect graph/load/liveness, dispatch authorized work, and resolve blockers");
    }
    if subject.starts_with("merge-pending:") {
        return ("P0", "the accepted task is awaiting your merge on refs/heads/main; record collab task integrated before close");
    }
    if subject.starts_with("subagent-status") {
        return ("P1", "re-dispatch, close, or leave the child idle");
    }
    if subject.starts_with("task-keepalive") {
        return ("P1", "continue your own task or record a real blocker");
    }
    if from == "collab-server"
        && mtype == "notification"
        && (subject.starts_with("goal:") || subject.starts_with("deadline:"))
    {
        return ("P0", "run the long-horizon briefing and schedule");
    }
    if subject.starts_with("release") || subject.contains("released") {
        return (
            "P1",
            "the resource is free; resume the task that waited on it",
        );
    }
    if subject.contains("recorded") || subject.contains("receipt") || subject.contains("delivered")
    {
        return ("P2", "note it and go straight back to your current task");
    }
    ("P1", "do the in-scope action the message asks for")
}

pub fn semantic_notification_category(subject: &str, mtype: &str) -> &'static str {
    if subject.starts_with("master-idle") || subject.starts_with("worker-idle") {
        return "idle";
    }
    if subject.starts_with("task-keepalive")
        || subject.starts_with("subagent-status")
        || subject.starts_with("progress")
        || subject.starts_with("delivery")
    {
        return "progress";
    }
    if subject.starts_with("resource")
        || subject.starts_with("deadline")
        || subject.starts_with("worker-unresponsive")
    {
        return "system";
    }
    if mtype == "notify" {
        return "direct";
    }
    "system"
}

pub fn notification_text(message: &Message) -> Option<String> {
    let subject_raw = message.subject.as_deref()?;
    Some(compose_notification_with_mtype(
        &message.id,
        subject_raw,
        &visible_body(&message.body),
        &message.from,
        &message.mtype,
    ))
}

pub fn is_explicit_delivery_mode(state: &State, message: &Message) -> bool {
    state.delivery_modes.get(&message.id).is_some_and(|mode| {
        matches!(
            mode.as_str(),
            "explicit-notification" | "immediate" | "queued"
        )
    })
}

pub fn is_explicit_notification(state: &State, message: &Message) -> bool {
    is_explicit_delivery_mode(state, message)
}

pub(super) fn automatic_retry_eligible(message: &Message, retry_delay_ms: i64, now: i64) -> bool {
    message.last_wake_attempt_ms == 0
        || now.saturating_sub(message.last_wake_attempt_ms) >= retry_delay_ms
}

fn preview_for_text(text: &str) -> Option<String> {
    let mut preview_width = 80;
    let head = text.strip_prefix("COLLAB_NOTIFY ")?;
    let (_, head) = head.split_once(' ')?;
    let (_, tail) = head.strip_prefix('[')?.split_once("] ")?;
    let tail = tail
        .split_once(" | P1 ACTION:")
        .or_else(|| tail.split_once(" | P0 ACTION:"))
        .or_else(|| tail.split_once(" | P2 ACTION:"))
        .map(|(body, _)| body.to_string())
        .unwrap_or_else(|| tail.to_string());
    if tail.chars().count() <= preview_width {
        return Some(tail);
    }
    preview_width -= 1;
    let mut preview = tail.chars().take(preview_width).collect::<String>();
    preview.push('…');
    Some(preview)
}

pub fn batch_notification_text(
    batch: &[(i64, String, String, String, String)],
    remaining: usize,
) -> String {
    let entries = batch
        .iter()
        .map(|(_, id, _, _, text)| {
            let title = text
                .split_once('[')
                .and_then(|(_, rest)| rest.split_once(']'))
                .map(|(subject, _)| subject)
                .unwrap_or("notification");
            let preview = preview_for_text(text).unwrap_or_else(|| "…".to_string());
            format!("{id}:{title}={preview}")
        })
        .collect::<Vec<_>>()
        .join(";");
    let older = (remaining > 0)
        .then(|| format!(" older_messages={remaining}"))
        .unwrap_or_default();
    // Message does not carry a typed task association. Do not infer one from
    // preview text; the durable inbox remains the source for task details.
    // Identity, relevance and overflow only: the composed notification owns
    // the single read action, and the Collab Skill owns the execution protocol.
    // Each entry already carries its own id, title and a faithful preview, so
    // the per-batch `message_ids` re-listing and `last` title repeat were
    // ablated as redundant content; ids/titles/previews remain recoverable
    // from `entries`, and the durable inbox remains the source for details.
    format!("Batch wake: entries={entries}.{older}")
}

fn compose_notification_with_mtype(
    id: &str,
    subject_raw: &str,
    body: &str,
    from: &str,
    mtype: &str,
) -> String {
    let subject = abbreviated_subject(subject_raw).unwrap_or_else(|| "notice".to_string());
    let (priority, action) = notification_class(subject_raw, from, mtype);

    let head = format!("COLLAB_NOTIFY {} [{}] ", id, subject);
    let tail = format!(
        " | {} ACTION: {}. Read it once with `collab recv`; that consumes this durable message.",
        priority, action
    );

    // The action and the read step must survive a long body, so the body
    // absorbs the truncation instead of the instructions being cut off.
    let fixed = head.chars().count() + tail.chars().count();
    let budget = MAX_NOTIFICATION_CHARS.saturating_sub(fixed);
    let body = if body.chars().count() > budget {
        const ELLIPSIS: &str = "… [collab inbox]";
        let keep = budget.saturating_sub(ELLIPSIS.chars().count());
        body.chars().take(keep).collect::<String>() + ELLIPSIS
    } else {
        body.to_string()
    };

    format!("{head}{body}{tail}")
}

pub fn compose_notification(id: &str, subject_raw: &str, body: &str) -> String {
    compose_notification_with_mtype(id, subject_raw, body, "", "notify")
}

pub fn truncate_notification(text: String) -> String {
    if text.chars().count() <= MAX_NOTIFICATION_CHARS {
        return text;
    }
    const SUFFIX: &str = "… [truncated; run collab inbox]";
    let keep = MAX_NOTIFICATION_CHARS.saturating_sub(SUFFIX.chars().count());
    let mut truncated = text.chars().take(keep).collect::<String>();
    truncated.push_str(SUFFIX);
    truncated
}

pub fn select_batch(
    candidates: Vec<(i64, String, String, String, String)>,
    delay: i64,
    window_start_ms: i64,
) -> (Vec<(i64, String, String, String, String)>, usize) {
    const MAX_BATCH_DELIVERY: usize = 3;
    let window_end = window_start_ms.saturating_add(delay);
    let mut batch: Vec<_> = candidates
        .into_iter()
        .filter(|candidate| candidate.0 <= window_end)
        .collect();
    let total_pending = batch.len();
    let remaining = total_pending.saturating_sub(MAX_BATCH_DELIVERY);
    if remaining > 0 {
        batch.drain(..remaining);
    }
    (batch, remaining)
}

pub fn max_wake_attempts() -> u32 {
    MAX_WAKE_ATTEMPTS
}

#[cfg(test)]
#[path = "mailbox_tests.rs"]
mod tests;
