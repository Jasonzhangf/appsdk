//! Public task-board commands and durable descriptive facts. TaskRec remains
//! the sole owner of task ownership, status, resources, and progress.
use clap::Subcommand;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, Subcommand)]
#[serde(tag = "action", rename_all = "kebab-case")]
pub enum BoardCommand {
    /// Read the project's public task board without consuming messages.
    Show,
    /// Publish a pending project task (live master only).
    Publish {
        id: String,
        #[arg(long)]
        title: String,
        #[arg(long)]
        description: String,
        #[arg(long)]
        delivery_condition: String,
        #[arg(long)]
        test_condition: String,
        #[arg(long, default_value = "p2")]
        priority: String,
    },
    /// Invite one live idle ordinary peer; ownership transfers only on accept.
    Invite {
        id: String,
        #[arg(long)]
        to: String,
        #[arg(long)]
        expected_revision: u64,
    },
    /// Accept or decline the invitation addressed to this peer.
    Respond {
        id: String,
        #[arg(long, conflicts_with = "decline", required_unless_present = "decline")]
        accept: bool,
        #[arg(long, conflicts_with = "accept", required_unless_present = "accept")]
        decline: bool,
        #[arg(long)]
        expected_revision: u64,
        #[arg(long)]
        reason: Option<String>,
    },
    /// Reject an unstarted legacy ordinary-peer scheduler assignment.
    #[command(hide = true)]
    DeclineAssigned {
        id: String,
        #[arg(long)]
        expected_revision: u64,
        #[arg(long)]
        reason: String,
    },
    /// Withdraw an invitation that has not been accepted (live master only).
    Withdraw {
        id: String,
        #[arg(long)]
        expected_revision: u64,
        #[arg(long)]
        reason: String,
    },
    /// Update the authenticated owner's progress using the observed revision.
    Update {
        id: String,
        #[arg(long)]
        expected_revision: u64,
        #[arg(long)]
        status: Option<String>,
        #[arg(long)]
        next: Option<String>,
    },
    /// Describe an existing owner-local task without changing its lifecycle.
    Describe {
        id: String,
        #[arg(long)]
        expected_revision: u64,
        #[arg(long)]
        title: String,
        #[arg(long)]
        description: String,
        #[arg(long)]
        delivery_condition: String,
        #[arg(long)]
        test_condition: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BoardInvitation {
    pub peer_id: String,
    pub binding_id: String,
    pub endpoint_generation: u64,
    pub message_id: String,
    pub created_ms: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BoardTaskDetails {
    pub title: String,
    pub description: String,
    pub delivery_condition: String,
    pub test_condition: String,
    pub revision: u64,
    pub public_visibility: bool,
    pub invitation: Option<BoardInvitation>,
    pub last_response: Option<String>,
}

impl BoardTaskDetails {
    pub fn legacy(id: &str, public_visibility: bool) -> Self {
        Self { title: id.into(), revision: 1, public_visibility, ..Self::default() }
    }
}

/// Deliberate allowlist: no tokens, runtime endpoints, role briefs, child
/// identity, internal resource records, or raw journal events.
#[derive(Debug, Serialize)]
pub struct BoardTaskView {
    pub id: String,
    pub title: String,
    pub description: String,
    pub delivery_condition: String,
    pub test_condition: String,
    pub revision: u64,
    pub owner: String,
    pub publisher: String,
    pub priority: String,
    pub status: String,
    pub next_step: Option<String>,
    pub invited_peer: Option<String>,
    pub last_response: Option<String>,
    pub updated_at: String,
    pub delivery_evidence: Option<String>,
    pub review_evidence: Option<String>,
    pub integration_commit: Option<String>,
    pub cleanup_status: String,
    pub blocking_task: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct BoardMemberView {
    pub id: String,
    pub role: String,
    pub status: String,
    pub task_ids: Vec<String>,
    pub invitation_ids: Vec<String>,
}
