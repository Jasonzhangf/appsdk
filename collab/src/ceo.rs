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
    pub phase: String,
    pub pane_id: Option<String>,
    pub error: Option<String>,
    pub handshake_message_id: Option<String>,
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
    },
    Progress {
        project: Option<String>,
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
}
