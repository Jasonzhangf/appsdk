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
///
/// The destination is owned by the full typed channel identity, so two
/// conversations with the same platform ID never share a file. A directory is
/// reused only when its recorded identity matches the same event; sanitized or
/// truncated ID collisions allocate a sibling instead of overwriting.
pub fn stage_inbound_attachment(
    authorized_root: &std::path::Path,
    envelope: &ChannelEnvelope,
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
    let namespace = root
        .join("inbound")
        .join(safe_path_segment(&envelope.source_channel))
        .join(safe_path_segment(&envelope.channel_instance))
        .join(safe_path_segment(&envelope.external_conversation));
    if let Err(error) = std::fs::create_dir_all(&namespace) {
        return failed(format!("ATTACHMENT_SAVE_FAILED: {error}"));
    }
    let identity = InboundAttachmentIdentity::from_envelope(envelope);
    let dir = match allocate_event_directory(&namespace, &identity) {
        Ok(dir) => dir,
        Err(error) => return failed(error),
    };
    let file_name = format!("{index}-{}", safe_path_segment(original_name));
    let path = dir.join(file_name);
    // `create_new` never truncates an already published file. A replay of the
    // same event therefore reuses the original bytes.
    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
    {
        Ok(mut file) => {
            use std::io::Write;
            let write = file.write_all(bytes).and_then(|()| file.sync_all());
            if let Err(error) = write {
                // Do not leave a half-written file that later looks published.
                std::fs::remove_file(&path).ok();
                return failed(format!("ATTACHMENT_SAVE_FAILED: {error}"));
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            // Same event identity and attachment slot: reuse the published file.
        }
        Err(error) => return failed(format!("ATTACHMENT_SAVE_FAILED: {error}")),
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

/// Typed identity recorded inside every event directory. It distinguishes a
/// true replay from another event whose sanitized or truncated ID collides.
#[derive(Debug, Serialize, Deserialize)]
struct InboundAttachmentIdentity {
    source_channel: String,
    channel_instance: String,
    external_user: String,
    external_conversation: String,
    platform_message_id: String,
}

impl InboundAttachmentIdentity {
    fn from_envelope(envelope: &ChannelEnvelope) -> Self {
        Self {
            source_channel: envelope.source_channel.clone(),
            channel_instance: envelope.channel_instance.clone(),
            external_user: envelope.external_user.clone(),
            external_conversation: envelope.external_conversation.clone(),
            platform_message_id: envelope.platform_message_id.clone(),
        }
    }
}

const EVENT_IDENTITY_FILE: &str = "event-identity.json";

fn allocate_event_directory(
    namespace: &std::path::Path,
    identity: &InboundAttachmentIdentity,
) -> Result<std::path::PathBuf, String> {
    let payload =
        serde_json::to_vec(identity).map_err(|error| format!("ATTACHMENT_SAVE_FAILED: {error}"))?;
    let base = safe_path_segment(&identity.platform_message_id);
    for attempt in 0..1000usize {
        let name = if attempt == 0 {
            base.clone()
        } else {
            format!("{base}-{}", attempt + 1)
        };
        let candidate = namespace.join(name);
        match std::fs::create_dir(&candidate) {
            Ok(()) => {
                let record = candidate.join(EVENT_IDENTITY_FILE);
                if let Err(error) = std::fs::write(&record, &payload) {
                    std::fs::remove_dir_all(&candidate).ok();
                    return Err(format!("ATTACHMENT_SAVE_FAILED: {error}"));
                }
                return Ok(candidate);
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                let record = candidate.join(EVENT_IDENTITY_FILE);
                match std::fs::read(&record) {
                    Ok(existing) if existing == payload => return Ok(candidate),
                    // Same sanitized segment but another event (or an unknown
                    // legacy directory): keep looking for a free sibling.
                    _ => continue,
                }
            }
            Err(error) => return Err(format!("ATTACHMENT_SAVE_FAILED: {error}")),
        }
    }
    Err("ATTACHMENT_SAVE_FAILED: attachment namespace is exhausted".into())
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
            &ChannelEnvelope {
                source_channel: "qqbot".into(),
                channel_instance: "qqbot@host".into(),
                external_user: "u1".into(),
                external_conversation: "u1:session".into(),
                platform_message_id: "../../platform/../../id".into(),
                reply_route: "qqbot://u1/session".into(),
                recipient_address: "agent".into(),
                attachments: Vec::new(),
            },
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
        // The path is fully under the namespace; the hostile platform id never
        // escapes into the directory tree.
        assert!(descriptor
            .local_path
            .contains("/inbound/qqbot/qqbot_host/u1_session/"));
        assert!(!descriptor.local_path.contains(".."));
        assert!(!descriptor.local_path.contains("/../../"));
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
        let staged = stage_inbound_attachment(
            &missing,
            &ChannelEnvelope {
                source_channel: "qqbot".into(),
                channel_instance: "qqbot@host".into(),
                external_user: "u1".into(),
                external_conversation: "u1:session".into(),
                platform_message_id: "platform-1".into(),
                reply_route: "qqbot://u1/session".into(),
                recipient_address: "agent".into(),
                attachments: Vec::new(),
            },
            0,
            "photo.jpg",
            "image/jpeg",
            b"x",
        );
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

    #[test]
    fn same_platform_event_id_in_two_conversations_preserves_both_files() {
        let root = temp_root("conversation-isolation");
        let envelope_a = ChannelEnvelope {
            source_channel: "qqbot".into(),
            channel_instance: "qqbot@host".into(),
            external_user: "u1".into(),
            external_conversation: "u1:session".into(),
            platform_message_id: "same-event".into(),
            reply_route: "qqbot://u1/session".into(),
            recipient_address: "agent".into(),
            attachments: Vec::new(),
        };
        let mut envelope_b = envelope_a.clone();
        envelope_b.external_conversation = "u2:session".into();
        envelope_b.reply_route = "qqbot://u2/session".into();
        let first =
            stage_inbound_attachment(&root, &envelope_a, 0, "photo.jpg", "image/jpeg", b"first");
        let second =
            stage_inbound_attachment(&root, &envelope_b, 0, "photo.jpg", "image/jpeg", b"second");
        let StagedInbound::Published(first) = first else {
            panic!("expected a published attachment: {first:?}");
        };
        let StagedInbound::Published(second) = second else {
            panic!("expected a published attachment: {second:?}");
        };
        assert_ne!(
            first.local_path, second.local_path,
            "different conversations must not share one attachment destination"
        );
        assert_eq!(std::fs::read(&first.local_path).unwrap(), b"first");
        assert_eq!(std::fs::read(&second.local_path).unwrap(), b"second");
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn sanitized_and_truncated_event_ids_do_not_overwrite_each_other() {
        let root = temp_root("sanitize-collision");
        let mut published = Vec::new();
        for event_id in ["a/b", "a?b"] {
            let staged = stage_inbound_attachment(
                &root,
                &ChannelEnvelope {
                    source_channel: "qqbot".into(),
                    channel_instance: "qqbot@host".into(),
                    external_user: "u1".into(),
                    external_conversation: "u1:session".into(),
                    platform_message_id: event_id.into(),
                    reply_route: "qqbot://u1/session".into(),
                    recipient_address: "agent".into(),
                    attachments: Vec::new(),
                },
                0,
                "f.bin",
                "application/octet-stream",
                event_id.as_bytes(),
            );
            let StagedInbound::Published(descriptor) = staged else {
                panic!("expected a published attachment: {staged:?}");
            };
            published.push(descriptor);
        }
        let long_prefix = "x".repeat(96);
        for suffix in ["a", "b"] {
            let event_id = format!("{long_prefix}-{suffix}");
            let staged = stage_inbound_attachment(
                &root,
                &ChannelEnvelope {
                    source_channel: "qqbot".into(),
                    channel_instance: "qqbot@host".into(),
                    external_user: "u1".into(),
                    external_conversation: "u1:session".into(),
                    platform_message_id: event_id.clone(),
                    reply_route: "qqbot://u1/session".into(),
                    recipient_address: "agent".into(),
                    attachments: Vec::new(),
                },
                0,
                "f.bin",
                "application/octet-stream",
                suffix.as_bytes(),
            );
            let StagedInbound::Published(descriptor) = staged else {
                panic!("expected a published attachment: {staged:?}");
            };
            published.push(descriptor);
        }
        let unique: std::collections::BTreeSet<_> = published
            .iter()
            .map(|item| item.local_path.clone())
            .collect();
        assert_eq!(
            unique.len(),
            published.len(),
            "sanitized or truncated event ids must not share one destination: {published:?}"
        );
        assert_eq!(std::fs::read(&published[0].local_path).unwrap(), b"a/b");
        assert_eq!(std::fs::read(&published[1].local_path).unwrap(), b"a?b");
        assert_eq!(std::fs::read(&published[2].local_path).unwrap(), b"a");
        assert_eq!(std::fs::read(&published[3].local_path).unwrap(), b"b");
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn repeated_staging_of_the_same_event_reuses_the_published_bytes() {
        let root = temp_root("repeat");
        let envelope = ChannelEnvelope {
            source_channel: "qqbot".into(),
            channel_instance: "qqbot@host".into(),
            external_user: "u1".into(),
            external_conversation: "u1:session".into(),
            platform_message_id: "event-1".into(),
            reply_route: "qqbot://u1/session".into(),
            recipient_address: "agent".into(),
            attachments: Vec::new(),
        };
        let first = stage_inbound_attachment(
            &root,
            &envelope,
            0,
            "f.bin",
            "application/octet-stream",
            b"original",
        );
        let StagedInbound::Published(first) = first else {
            panic!("expected a published attachment: {first:?}");
        };
        let second = stage_inbound_attachment(
            &root,
            &envelope,
            0,
            "f.bin",
            "application/octet-stream",
            b"replacement",
        );
        let StagedInbound::Published(second) = second else {
            panic!("expected a published attachment: {second:?}");
        };
        assert_eq!(first.local_path, second.local_path);
        assert_eq!(
            std::fs::read(&first.local_path).unwrap(),
            b"original",
            "a repeated save must never truncate an already published attachment"
        );
        std::fs::remove_dir_all(root).ok();
    }
}
