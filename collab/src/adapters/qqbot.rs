//! QQBot platform boundary.
//!
//! The external SDK method, size limits and receipt fields are runtime-owned
//! facts. This module keeps the daemon contract typed and safe: only declared
//! attachments are read, every path stays under the configured root, and a
//! platform result is accepted, failed or uncertain. It never retries an
//! uncertain send and never rewrites `Message.state`.
use crate::server::channel_delivery::{ChannelDeliveryResult, ChannelDeliveryStatus};
use crate::server::channel_envelope::authorized_attachment_path;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutboundAttachment {
    pub local_path: PathBuf,
    pub display_name: String,
    pub media_type: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutboundRequest {
    pub collab_message_id: String,
    pub channel_instance: String,
    pub reply_route: String,
    /// Project-qualified sender label derived from the authenticated identity,
    /// never from message content.
    pub sender_label: String,
    pub content: String,
    /// Only this typed list can trigger a file read. Paths mentioned in
    /// `content` are plain text and remain untouched.
    pub attachments: Vec<OutboundAttachment>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthorizedAttachment {
    pub resolved_path: PathBuf,
    pub display_name: String,
    pub media_type: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedSend {
    pub request: OutboundRequest,
    pub attachments: Vec<AuthorizedAttachment>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlatformOutcome {
    Accepted {
        platform_message_id: String,
    },
    Failed {
        error: String,
    },
    /// The request may have reached the platform but no unambiguous response
    /// was received. The caller must not resend it blindly.
    Uncertain {
        detail: String,
    },
}

pub trait QqBotPlatform {
    fn send(&self, prepared: &PreparedSend) -> PlatformOutcome;
}

/// Validate one outbound request before any platform call. A missing root is an
/// explicit unavailable result, not a broader filesystem search.
pub fn prepare_outbound(
    authorized_root: Option<&Path>,
    request: OutboundRequest,
) -> Result<PreparedSend, String> {
    if request.collab_message_id.trim().is_empty() {
        return Err(
            "QQBOT_OUTBOUND_MESSAGE_REQUIRED: Collab outbound message id is required".into(),
        );
    }
    if request.channel_instance.trim().is_empty() {
        return Err("QQBOT_OUTBOUND_CHANNEL_REQUIRED: channel instance is required".into());
    }
    if request.reply_route.trim().is_empty() {
        return Err("QQBOT_OUTBOUND_ROUTE_REQUIRED: original reply route is required".into());
    }
    if request.sender_label.trim().is_empty() {
        return Err(
            "QQBOT_OUTBOUND_SENDER_REQUIRED: authenticated sender label is required".into(),
        );
    }
    if request.content.trim().is_empty() && request.attachments.is_empty() {
        return Err("QQBOT_OUTBOUND_EMPTY: message has no content or attachment".into());
    }
    // A root is required only when typed attachments exist. Text-only sends do
    // not touch the filesystem and must not fail for a missing root.
    let Some(root) = authorized_root else {
        if request.attachments.is_empty() {
            return Ok(PreparedSend {
                request,
                attachments: Vec::new(),
            });
        }
        return Err(
            "QQBOT_ATTACHMENT_ROOT_UNAVAILABLE: no authorized attachment root is configured".into(),
        );
    };
    let attachments = request
        .attachments
        .iter()
        .map(|attachment| {
            let resolved_path = authorized_attachment_path(root, &attachment.local_path)?;
            Ok(AuthorizedAttachment {
                resolved_path,
                display_name: attachment.display_name.clone(),
                media_type: attachment.media_type.clone(),
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    Ok(PreparedSend {
        request,
        attachments,
    })
}

/// Convert one platform outcome to the durable result shape. No retry is
/// possible here; an uncertain outcome stays uncertain until a separate query
/// proves otherwise.
pub fn delivery_result(prepared: &PreparedSend, outcome: PlatformOutcome) -> ChannelDeliveryResult {
    let (status, platform_message_id, error) = match outcome {
        PlatformOutcome::Accepted {
            platform_message_id,
        } => (
            ChannelDeliveryStatus::Accepted,
            Some(platform_message_id),
            None,
        ),
        PlatformOutcome::Failed { error } => (ChannelDeliveryStatus::Failed, None, Some(error)),
        PlatformOutcome::Uncertain { detail } => {
            (ChannelDeliveryStatus::Uncertain, None, Some(detail))
        }
    };
    ChannelDeliveryResult {
        message_id: prepared.request.collab_message_id.clone(),
        channel_instance: prepared.request.channel_instance.clone(),
        status,
        platform_message_id,
        error,
        updated_ms: 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(content: &str, attachments: Vec<OutboundAttachment>) -> OutboundRequest {
        OutboundRequest {
            collab_message_id: "m-out-1".into(),
            channel_instance: "qqbot@host".into(),
            reply_route: "qqbot://u1/session".into(),
            sender_label: "appsdk.Agent".into(),
            content: content.into(),
            attachments,
        }
    }

    #[test]
    fn text_only_send_needs_no_attachment_root() {
        let prepared = prepare_outbound(None, request("plain hello", Vec::new())).unwrap();
        assert!(prepared.attachments.is_empty());
    }

    #[test]
    fn candidate_paths_in_text_never_trigger_a_file_read() {
        // A path-looking body is user data. Without a typed attachment the
        // daemon must not read or reject it even though no root is configured.
        let prepared =
            prepare_outbound(None, request("please open /etc/hosts for me", Vec::new())).unwrap();
        assert!(prepared.attachments.is_empty());
    }

    #[test]
    fn typed_attachment_is_resolved_inside_the_authorized_root() {
        let base = std::env::temp_dir().join(format!(
            "qqbot-att-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let root = base.join("root");
        std::fs::create_dir_all(&root).unwrap();
        let file = root.join("report.pdf");
        std::fs::write(&file, b"payload").unwrap();

        let prepared = prepare_outbound(
            Some(&root),
            request(
                "see attached",
                vec![OutboundAttachment {
                    local_path: file.clone(),
                    display_name: "report.pdf".into(),
                    media_type: "application/pdf".into(),
                }],
            ),
        )
        .unwrap();
        assert_eq!(prepared.attachments.len(), 1);
        assert_eq!(
            prepared.attachments[0].resolved_path,
            file.canonicalize().unwrap()
        );

        // The same typed attachment without a root is an explicit failure, not
        // a broader filesystem search.
        let err = prepare_outbound(
            None,
            request(
                "see attached",
                vec![OutboundAttachment {
                    local_path: file,
                    display_name: "report.pdf".into(),
                    media_type: "application/pdf".into(),
                }],
            ),
        )
        .unwrap_err();
        assert!(err.contains("QQBOT_ATTACHMENT_ROOT_UNAVAILABLE"), "{err}");
        std::fs::remove_dir_all(&base).ok();
    }
}
