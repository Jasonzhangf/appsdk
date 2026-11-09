//! QQBot platform boundary.
//!
//! The external SDK method, size limits and receipt fields are runtime-owned
//! facts. This module keeps the daemon contract typed and safe: only declared
//! attachments are read, every path stays under the configured root, and a
//! platform result is accepted, failed or uncertain. It never retries an
//! uncertain send and never rewrites `Message.state`.
use crate::config::QqBotDelivery;
use crate::server::channel_delivery::{
    ChannelDeliveryResult, ChannelDeliveryStatus, PlatformTimestamp,
};
use crate::server::channel_envelope::authorized_attachment_path;
use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

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
        platform_timestamp: PlatformTimestamp,
        platform_ref_idx: Option<String>,
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct QqBotRoute {
    pub scope: String,
    pub target_id: String,
}

/// Parse only the canonical `qqbot://<c2c|group>/<target>` route copied from
/// the original trusted envelope. No default target or alternate shape is
/// guessed for a malformed route.
pub(crate) fn parse_qqbot_reply_route(route: &str) -> Result<QqBotRoute, String> {
    let rest = route.strip_prefix("qqbot://").ok_or_else(|| {
        format!("QQBOT_ROUTE_INVALID: expected qqbot://scope/target, got {route}")
    })?;
    let mut segments = rest.split('/');
    let scope = segments.next().unwrap_or_default();
    let target_id = segments.next().unwrap_or_default();
    if !matches!(scope, "c2c" | "group")
        || target_id.is_empty()
        || target_id
            .chars()
            .any(|character| character.is_control() || character.is_whitespace())
        || target_id.contains(['?', '#'])
        || segments.next().is_some()
    {
        return Err(format!(
            "QQBOT_ROUTE_INVALID: expected qqbot://c2c|group/<target>, got {route}"
        ));
    }
    Ok(QqBotRoute {
        scope: scope.into(),
        target_id: target_id.into(),
    })
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
    parse_qqbot_reply_route(&request.reply_route)?;
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
    let (status, platform_message_id, error, platform_timestamp, platform_ref_idx) = match outcome {
        PlatformOutcome::Accepted {
            platform_message_id,
            platform_timestamp,
            platform_ref_idx,
        } => (
            ChannelDeliveryStatus::Accepted,
            Some(platform_message_id),
            None,
            Some(platform_timestamp),
            platform_ref_idx,
        ),
        PlatformOutcome::Failed { error } => {
            (ChannelDeliveryStatus::Failed, None, Some(error), None, None)
        }
        PlatformOutcome::Uncertain { detail } => (
            ChannelDeliveryStatus::Uncertain,
            None,
            Some(detail),
            None,
            None,
        ),
    };
    ChannelDeliveryResult {
        message_id: prepared.request.collab_message_id.clone(),
        channel_instance: prepared.request.channel_instance.clone(),
        status,
        platform_message_id,
        error,
        platform_timestamp,
        platform_ref_idx,
        updated_ms: 0,
    }
}

#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum HelperResult {
    Accepted {
        message_id: String,
        platform_message_id: String,
        timestamp: PlatformTimestamp,
        #[serde(default)]
        ref_idx: Option<String>,
    },
    Failed {
        message_id: String,
        #[serde(default)]
        code: Option<String>,
        error: String,
    },
    Uncertain {
        message_id: String,
        #[serde(default)]
        code: Option<String>,
        error: String,
    },
}

enum HelperFailure {
    NotStarted(String),
    Unknown(String),
}

fn helper_error(code: Option<String>, error: String) -> String {
    match code.filter(|code| !code.trim().is_empty()) {
        Some(code) if error.trim().is_empty() => code,
        Some(code) => format!("{code}: {error}"),
        None => error,
    }
}

fn map_helper_output(output: std::process::Output, expected_message_id: &str) -> PlatformOutcome {
    let parsed = serde_json::from_slice::<HelperResult>(&output.stdout);
    let result = match parsed {
        Ok(result) => result,
        Err(error) => {
            return PlatformOutcome::Uncertain {
                detail: format!(
                    "QQBOT_HELPER_RESPONSE_UNKNOWN: helper exited {} without one valid JSON result: {error}",
                    output.status
                ),
            }
        }
    };
    match result {
        HelperResult::Accepted {
            message_id,
            platform_message_id,
            timestamp,
            ref_idx,
        } if message_id == expected_message_id => PlatformOutcome::Accepted {
            platform_message_id,
            platform_timestamp: timestamp,
            platform_ref_idx: ref_idx,
        },
        HelperResult::Failed {
            message_id,
            code,
            error,
        } if message_id == expected_message_id => PlatformOutcome::Failed {
            error: helper_error(code, error),
        },
        HelperResult::Uncertain {
            message_id,
            code,
            error,
        } if message_id == expected_message_id => PlatformOutcome::Uncertain {
            detail: helper_error(code, error),
        },
        HelperResult::Accepted { message_id, .. }
        | HelperResult::Failed { message_id, .. }
        | HelperResult::Uncertain { message_id, .. } => PlatformOutcome::Uncertain {
            detail: format!(
                "QQBOT_HELPER_RESPONSE_MISMATCH: helper replied for {message_id}, expected {expected_message_id}"
            ),
        },
    }
}

async fn invoke_helper(
    config: &QqBotDelivery,
    request: &serde_json::Value,
) -> Result<std::process::Output, HelperFailure> {
    let input = serde_json::to_vec(request).map_err(|error| {
        HelperFailure::NotStarted(format!("QQBOT_HELPER_INPUT_INVALID: {error}"))
    })?;
    let mut child = Command::new(&config.node_program)
        .arg(&config.helper_path)
        .arg("--runtime-root")
        .arg(&config.runtime_root)
        .arg("--state-root")
        .arg(&config.state_root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|error| HelperFailure::NotStarted(format!("QQBOT_HELPER_NOT_STARTED: {error}")))?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(&input).await.map_err(|error| {
            HelperFailure::Unknown(format!("QQBOT_HELPER_STDIN_UNKNOWN: {error}"))
        })?;
    }
    child
        .wait_with_output()
        .await
        .map_err(|error| HelperFailure::Unknown(format!("QQBOT_HELPER_WAIT_UNKNOWN: {error}")))
}

fn helper_preflight(config: &QqBotDelivery) -> Result<(), String> {
    for (name, path) in [
        ("node_program", &config.node_program),
        ("helper_path", &config.helper_path),
    ] {
        if !path.is_file() {
            return Err(format!(
                "QQBOT_HELPER_NOT_READY: qqbot.delivery.{name} {} is not a file",
                path.display()
            ));
        }
    }
    for (name, path) in [
        ("runtime_root", &config.runtime_root),
        ("state_root", &config.state_root),
    ] {
        if !path.is_dir() {
            return Err(format!(
                "QQBOT_HELPER_NOT_READY: qqbot.delivery.{name} {} is not a directory",
                path.display()
            ));
        }
    }
    Ok(())
}

/// Invoke the declared helper once. No process, environment or target fallback
/// exists. A failure to start is known-not-delivered; every post-start anomaly
/// remains uncertain.
pub(crate) async fn send_prepared(
    config: &QqBotDelivery,
    prepared: &PreparedSend,
) -> PlatformOutcome {
    if let Err(error) = helper_preflight(config) {
        return PlatformOutcome::Failed { error };
    }
    let route = match parse_qqbot_reply_route(&prepared.request.reply_route) {
        Ok(route) => route,
        Err(error) => return PlatformOutcome::Failed { error },
    };
    let request = serde_json::json!({
        "message_id": prepared.request.collab_message_id,
        "scope": route.scope,
        "target_id": route.target_id,
        "content": prepared.request.content,
    });
    match tokio::time::timeout(
        Duration::from_millis(config.deadline_ms),
        invoke_helper(config, &request),
    )
    .await
    {
        Ok(Ok(output)) => map_helper_output(output, &prepared.request.collab_message_id),
        Ok(Err(HelperFailure::NotStarted(error))) => PlatformOutcome::Failed { error },
        Ok(Err(HelperFailure::Unknown(detail))) => PlatformOutcome::Uncertain { detail },
        Err(_) => PlatformOutcome::Uncertain {
            detail: format!(
                "QQBOT_HELPER_DEADLINE_UNKNOWN: helper did not complete within {}ms",
                config.deadline_ms
            ),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(content: &str, attachments: Vec<OutboundAttachment>) -> OutboundRequest {
        OutboundRequest {
            collab_message_id: "m-out-1".into(),
            channel_instance: "qqbot@host".into(),
            reply_route: "qqbot://c2c/user-1".into(),
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

    #[test]
    fn reply_route_parser_accepts_only_one_canonical_target_segment() {
        assert_eq!(
            parse_qqbot_reply_route("qqbot://c2c/user-1").unwrap(),
            QqBotRoute {
                scope: "c2c".into(),
                target_id: "user-1".into(),
            }
        );
        assert_eq!(
            parse_qqbot_reply_route("qqbot://group/group-1").unwrap(),
            QqBotRoute {
                scope: "group".into(),
                target_id: "group-1".into(),
            }
        );
        for invalid in [
            "qqbot://c2c",
            "qqbot://c2c/",
            "qqbot://guild/user-1",
            "qqbot://c2c/user-1/extra",
            "qqbot://c2c/user-1?x=1",
            "qqbot://c2c/user-1#fragment",
            "qqbot://c2c/user 1",
            "https://c2c/user-1",
        ] {
            assert!(parse_qqbot_reply_route(invalid).is_err(), "{invalid}");
        }
    }
}
