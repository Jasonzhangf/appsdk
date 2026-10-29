//! Typed public requests for global CEO coordination. Authorization remains
//! with the host runtime manager; these fields confer no actor authority.
use crate::proto::ReplyPolicy;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CeoGrant {
    pub workspace: String,
    pub app_scope_id: String,
    pub name: String,
    pub granted_by: String,
    pub approval: String,
    pub granted_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CeoBootstrapRecord {
    pub operation_id: String,
    pub project: String,
    pub approval: String,
    pub profile: String,
    pub tmux_socket: String,
    pub tmux_session: String,
    pub requested_by: String,
    pub requested_workspace: String,
    pub requested_app_scope: String,
    pub phase: String,
    pub pane_id: Option<String>,
    pub error: Option<String>,
    pub handshake_message_id: Option<String>,
    #[serde(default)]
    pub cancellation_approval: Option<String>,
    pub updated_ms: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "kebab-case")]
pub enum CeoCommand {
    Status,
    Promote {
        approval: String,
        workspace: Option<String>,
    },
    Clear {
        approval: String,
    },
    Send {
        project: String,
        to: Option<String>,
        subject: String,
        body: String,
        in_reply_to: Option<String>,
        reply_policy: Option<ReplyPolicy>,
    },
    AppendTask {
        project: String,
        id: String,
        title: String,
        description: String,
        delivery_condition: String,
        test_condition: String,
        priority: String,
        reply_policy: Option<ReplyPolicy>,
    },
    Reply {
        subject: String,
        body: String,
        in_reply_to: Option<String>,
        /// Explicit typed urgency for this progress. Never inferred from text.
        #[serde(default)]
        urgent: bool,
    },
    /// Durable direct delivery from the current project master to the CEO
    /// mailbox. Unlike an uncorrelated `Reply`, this never coalesces into
    /// unsolicited progress.
    DirectMessage {
        subject: String,
        body: String,
    },
    Progress {
        project: Option<String>,
    },
    /// Acknowledge one prepared report batch after actual visible user output.
    ReportAck {
        batch_id: String,
    },
    BootstrapPrepare {
        project: String,
        approval: String,
        profile: String,
        tmux_socket: String,
        tmux_session: String,
        operation_id: String,
    },
    BootstrapRecord {
        operation_id: String,
        pane_id: Option<String>,
        outcome: String,
        error: Option<String>,
    },
    BootstrapReady {
        operation_id: String,
        subject: String,
        body: String,
    },
    BootstrapCancel {
        operation_id: String,
        approval: String,
    },
}

#[cfg(test)]
mod tests {
    use super::CeoCommand;
    use serde_json::json;

    #[test]
    fn reply_urgent_is_typed_wire_flag_defaulting_to_false() {
        let legacy: CeoCommand =
            serde_json::from_value(json!({"action":"reply","subject":"s","body":"b"})).unwrap();
        match legacy {
            CeoCommand::Reply {
                urgent,
                in_reply_to,
                ..
            } => {
                assert!(!urgent, "reply without --urgent must default to false");
                assert!(in_reply_to.is_none());
            }
            other => panic!("expected reply, got {other:?}"),
        }

        let urgent: CeoCommand = serde_json::from_value(json!({
            "action":"reply","subject":"s","body":"b","urgent":true
        }))
        .unwrap();
        let wire = serde_json::to_value(&urgent).unwrap();
        assert_eq!(
            wire,
            json!({
                "action":"reply","subject":"s","body":"b","in_reply_to":null,"urgent":true
            })
        );
    }

    #[test]
    fn report_ack_round_trips_on_the_typed_action() {
        let command: CeoCommand = serde_json::from_value(json!({
            "action":"report-ack","batch_id":"batch-1"
        }))
        .unwrap();
        assert_eq!(
            serde_json::to_value(&command).unwrap(),
            json!({"action":"report-ack","batch_id":"batch-1"})
        );
    }
}
