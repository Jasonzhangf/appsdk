//! Typed channel envelope for channel-inbound (e.g. QQBot) messages.
//!
//! The body is user data and never carries control facts. Route, sender and
//! attachment metadata live here so reply routing, platform event idempotency
//! and attachment publication never need to be reconstructed from text.
use serde::{Deserialize, Serialize};

/// One inbound attachment after it has been safely saved to local storage.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttachmentDescriptor {
    /// Absolute local path published only after the file is fully saved.
    pub local_path: String,
    /// Original platform file name; display only, never a path segment.
    pub display_name: String,
    pub media_type: String,
}

/// Control facts for one channel-inbound message. Persisted together with the
/// Collab `Sent` message in the same durable batch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChannelEnvelope {
    pub source_channel: String,
    pub channel_instance: String,
    pub external_user: String,
    pub external_conversation: String,
    pub platform_message_id: String,
    /// Reply route back to the original external conversation. Immutable even
    /// if the conversation default later changes.
    pub reply_route: String,
    /// Resolved recipient address (explicit or conversation default).
    pub recipient_address: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<AttachmentDescriptor>,
}

/// Minimal immutable outbound route copied from one inbound envelope. It is
/// bound to the Collab reply message so an adapter never has to reconstruct a
/// QQ route from message text or a later conversation default.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChannelReplyRoute {
    pub source_channel: String,
    pub channel_instance: String,
    pub external_conversation: String,
    pub reply_route: String,
    pub in_reply_to: String,
}

impl ChannelReplyRoute {
    pub fn from_envelope(envelope: &ChannelEnvelope, in_reply_to: impl Into<String>) -> Self {
        Self {
            source_channel: envelope.source_channel.clone(),
            channel_instance: envelope.channel_instance.clone(),
            external_conversation: envelope.external_conversation.clone(),
            reply_route: envelope.reply_route.clone(),
            in_reply_to: in_reply_to.into(),
        }
    }
}

/// Fixed readable projection for one channel-inbound message. This text is a
/// convenience for agents; the typed envelope remains the control truth.
pub fn readable_projection(envelope: &ChannelEnvelope, body: &str) -> String {
    let source = if envelope.source_channel.eq_ignore_ascii_case("qqbot") {
        "QQBot"
    } else {
        envelope.source_channel.as_str()
    };
    let mut lines = vec![format!("来源：{source}"), format!("消息：{body}")];
    lines.extend(
        envelope
            .attachments
            .iter()
            .map(|attachment| format!("附件路径：{}", attachment.local_path)),
    );
    lines.join("\n")
}

/// Stable platform event idempotency key. A duplicate platform event maps to
/// exactly one Collab inbound message.
pub fn platform_event_key(
    channel_instance: &str,
    external_conversation: &str,
    platform_message_id: &str,
) -> String {
    format!(
        "{}:{}:{}",
        channel_instance, external_conversation, platform_message_id
    )
}

/// Validate and resolve an outbound attachment path. The canonicalized path
/// (after following symlinks) must be a regular readable file and must remain
/// inside the single authorized root. The daemon must not accept arbitrary home,
/// credential or repository paths as the root.
pub fn authorized_attachment_path(
    authorized_root: &std::path::Path,
    requested: &std::path::Path,
) -> Result<std::path::PathBuf, String> {
    let root = std::fs::canonicalize(authorized_root)
        .map_err(|error| format!("ATTACHMENT_ROOT_UNAVAILABLE: {error}"))?;
    let path = std::fs::canonicalize(requested)
        .map_err(|error| format!("ATTACHMENT_PATH_UNAVAILABLE: {error}"))?;
    if !path.starts_with(&root) {
        return Err(format!(
            "ATTACHMENT_PATH_OUT_OF_ROOT: {} is outside {}",
            path.display(),
            root.display()
        ));
    }
    let metadata =
        std::fs::metadata(&path).map_err(|error| format!("ATTACHMENT_PATH_UNREADABLE: {error}"))?;
    if !metadata.is_file() {
        return Err(format!(
            "ATTACHMENT_PATH_NOT_FILE: {} is not a regular file",
            path.display()
        ));
    }
    std::fs::File::open(&path).map_err(|error| format!("ATTACHMENT_PATH_UNREADABLE: {error}"))?;
    Ok(path)
}

/// Convert arbitrary platform text into one safe path segment. The original
/// name is kept only for display; it never becomes a directory or file name.
fn safe_path_segment(raw: &str) -> String {
    let mut segment: String = raw
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .collect();
    if segment.is_empty() || segment == "." || segment == ".." {
        segment = "item".into();
    }
    // Cap the length so a hostile platform id cannot create an unwritable name.
    segment.truncate(96);
    segment
}

/// One inbound attachment that passed or failed the save step. Only a
/// `Published` descriptor carries a local path, so a failed save never
/// publishes a path that does not exist.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StagedInbound {
    Published(AttachmentDescriptor),
    Failed { display_name: String, error: String },
}

/// Save one inbound attachment completely before returning its published path.
/// The caller must only put `Published` descriptors into the envelope.
pub fn stage_inbound_attachment(
    authorized_root: &std::path::Path,
    platform_message_id: &str,
    index: usize,
    original_name: &str,
    media_type: &str,
    bytes: &[u8],
) -> StagedInbound {
    let failed = |error: String| StagedInbound::Failed {
        display_name: original_name.to_owned(),
        error,
    };
    let root = match std::fs::canonicalize(authorized_root) {
        Ok(root) => root,
        Err(error) => return failed(format!("ATTACHMENT_ROOT_UNAVAILABLE: {error}")),
    };
    let dir = root
        .join("inbound")
        .join(safe_path_segment(platform_message_id));
    if let Err(error) = std::fs::create_dir_all(&dir) {
        return failed(format!("ATTACHMENT_SAVE_FAILED: {error}"));
    }
    let file_name = format!("{index}-{}", safe_path_segment(original_name));
    let path = dir.join(file_name);
    // Write through an explicit handle so a partial write is detected before
    // the path is published.
    let write = || -> std::io::Result<()> {
        use std::io::Write;
        let mut file = std::fs::File::create(&path)?;
        file.write_all(bytes)?;
        file.sync_all()
    };
    if let Err(error) = write() {
        // Do not leave a half-written file that later looks published.
        std::fs::remove_file(&path).ok();
        return failed(format!("ATTACHMENT_SAVE_FAILED: {error}"));
    }
    let canonical = match std::fs::canonicalize(&path) {
        Ok(path) => path,
        Err(error) => return failed(format!("ATTACHMENT_SAVE_FAILED: {error}")),
    };
    StagedInbound::Published(AttachmentDescriptor {
        local_path: canonical.to_string_lossy().into_owned(),
        display_name: original_name.to_owned(),
        media_type: media_type.to_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(tag: &str) -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!(
            "channel-envelope-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn inbound_attachment_is_published_only_after_a_complete_save() {
        let root = temp_root("publish");
        let staged = stage_inbound_attachment(
            &root,
            "../../platform/../../id",
            0,
            "../报告 file.pdf",
            "application/pdf",
            b"payload",
        );
        let StagedInbound::Published(descriptor) = staged else {
            panic!("expected a published attachment: {staged:?}");
        };
        let canonical_root = std::fs::canonicalize(&root).unwrap();
        let path = std::path::Path::new(&descriptor.local_path);
        assert!(path.is_absolute());
        assert!(path.starts_with(&canonical_root));
        assert!(!descriptor.local_path.contains(".."));
        // Original name is display-only; the safe segment is what reached disk.
        assert_eq!(descriptor.display_name, "../报告 file.pdf");
        assert_eq!(std::fs::read(path).unwrap(), b"payload");
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn inbound_save_failure_reports_the_name_without_publishing_a_path() {
        let missing = std::env::temp_dir().join(format!(
            "channel-envelope-missing-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let staged =
            stage_inbound_attachment(&missing, "platform-1", 0, "photo.jpg", "image/jpeg", b"x");
        let StagedInbound::Failed {
            display_name,
            error,
        } = staged
        else {
            panic!("missing root must not publish a path: {staged:?}");
        };
        assert_eq!(display_name, "photo.jpg");
        assert!(error.contains("ATTACHMENT_ROOT_UNAVAILABLE"), "{error}");
    }
}
