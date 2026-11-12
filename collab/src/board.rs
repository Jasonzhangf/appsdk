//! Public task-board commands and durable descriptive facts. TaskRec remains
//! the sole owner of task ownership, status, resources, and progress.
use clap::Subcommand;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct NameWorkRoles {
    pub name: String,
    #[serde(default)]
    pub work_roles: Vec<String>,
}

/// User-editable workflow text for one target. The target is either a standard
/// identity (`master`/`peer`) or a `role:<specialty>` projection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkflowTemplateSpec {
    pub role_task: String,
    pub responsibilities: Vec<String>,
    pub next_action: String,
    pub completion_action: String,
}

impl WorkflowTemplateSpec {
    /// Trim the editable text and reject empty required fields. There is
    /// deliberately no length or item cap: the request contract only requires
    /// nonempty content.
    pub fn normalized(self) -> Result<Self, &'static str> {
        let role_task = self.role_task.trim().to_owned();
        if role_task.is_empty() {
            return Err("role_task must not be empty");
        }
        let next_action = self.next_action.trim().to_owned();
        if next_action.is_empty() {
            return Err("next_action must not be empty");
        }
        let completion_action = self.completion_action.trim().to_owned();
        if completion_action.is_empty() {
            return Err("completion_action must not be empty");
        }
        let mut responsibilities = Vec::with_capacity(self.responsibilities.len());
        for responsibility in self.responsibilities {
            let responsibility = responsibility.trim().to_owned();
            if responsibility.is_empty() {
                return Err("responsibilities must contain only nonempty values");
            }
            responsibilities.push(responsibility);
        }
        if responsibilities.is_empty() {
            return Err("responsibilities must not be empty");
        }
        Ok(Self {
            role_task,
            responsibilities,
            next_action,
            completion_action,
        })
    }
}

/// One durable project-scoped workflow override. `revision` counts writes for
/// this exact target while the override exists; a reset removes the record and
/// restores the base source at revision 0.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkflowTemplateOverride {
    pub base_version: u32,
    pub revision: u64,
    pub spec: WorkflowTemplateSpec,
    pub updated_by: String,
    pub updated_ms: i64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkPolicy {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub work_kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preferred_roles: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub executor_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reviewer_name: Option<String>,
    /// Fields explicitly cleared at this scope. A cleared field blocks
    /// inheritance from a broader scope instead of meaning "unspecified".
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cleared_fields: Vec<String>,
}

pub(crate) fn apply_policy_field<T: Clone>(value: &Option<T>, clear: bool, target: &mut Option<T>) {
    if clear {
        *target = None;
    } else if let Some(value) = value {
        *target = Some(value.clone());
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LongHorizonTaskSpec {
    pub content: String,
    pub interval_ms: i64,
    pub repeat_count: u32,
    pub ttl_seconds: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, clap::Args)]
pub struct LongHorizonArgs {
    #[arg(long = "long-horizon-content")]
    pub long_horizon_content: Option<String>,
    #[arg(long = "long-horizon-every-ms")]
    pub long_horizon_every_ms: Option<i64>,
    #[arg(long = "long-horizon-repeat")]
    pub long_horizon_repeat: Option<u32>,
    #[arg(long = "long-horizon-ttl-seconds")]
    pub long_horizon_ttl_seconds: Option<u64>,
}

impl LongHorizonArgs {
    pub fn merge(
        self,
        previous: Option<&LongHorizonTaskSpec>,
    ) -> Result<Option<LongHorizonTaskSpec>, String> {
        if previous.is_none()
            && self.long_horizon_content.is_none()
            && self.long_horizon_every_ms.is_none()
            && self.long_horizon_repeat.is_none()
            && self.long_horizon_ttl_seconds.is_none()
        {
            return Ok(None);
        }
        let content = self
            .long_horizon_content
            .or_else(|| previous.map(|spec| spec.content.clone()))
            .filter(|text| !text.trim().is_empty())
            .ok_or("BOARD_LONG_HORIZON_CONTENT_REQUIRED")?;
        let interval_ms = self
            .long_horizon_every_ms
            .or_else(|| previous.map(|spec| spec.interval_ms))
            .filter(|interval| *interval > 0)
            .ok_or(
                "BOARD_LONG_HORIZON_CADENCE_REQUIRED: provide a positive --long-horizon-every-ms",
            )?;
        let repeat_count = self
            .long_horizon_repeat
            .or_else(|| previous.map(|spec| spec.repeat_count))
            .unwrap_or(100);
        if !(1..=100).contains(&repeat_count) {
            return Err("long-horizon-repeat must be between 1 and 100".into());
        }
        let ttl_seconds = self
            .long_horizon_ttl_seconds
            .or_else(|| previous.map(|spec| spec.ttl_seconds))
            .unwrap_or(crate::server::mailbox::MAX_NOTIFICATION_TTL_SECONDS);
        if !(1..=crate::server::mailbox::MAX_NOTIFICATION_TTL_SECONDS).contains(&ttl_seconds) {
            return Err(format!(
                "long-horizon-ttl-seconds must be between 1 and {}",
                crate::server::mailbox::MAX_NOTIFICATION_TTL_SECONDS
            ));
        }
        Ok(Some(LongHorizonTaskSpec {
            content,
            interval_ms,
            repeat_count,
            ttl_seconds,
        }))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Subcommand)]
#[serde(tag = "action", rename_all = "kebab-case")]
pub enum BoardCommand {
    /// Read open public work plus the caller's open own or managed-child private queue.
    Show,
    /// Publish pending work; managed-child publications remain private.
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
        #[arg(long)]
        work_kind: Option<String>,
        #[arg(long)]
        parent_task: Option<String>,
        #[arg(long)]
        prefer_role: Option<String>,
        #[arg(long)]
        executor: Option<String>,
        #[arg(long)]
        reviewer: Option<String>,
        /// Task ids that must complete before this ticket's execute phase may start.
        #[arg(long = "depends-on", num_args(1..))]
        #[serde(default)]
        depends_on: Vec<String>,
        #[command(flatten)]
        #[serde(flatten)]
        long_horizon: LongHorizonArgs,
    },
    /// Atomically claim one pending task at the observed board revision.
    Claim {
        id: String,
        #[arg(long)]
        expected_revision: u64,
    },
    /// Start a claimed ticket execution phase at the current attempt/generation.
    Start {
        id: String,
        #[arg(long)]
        expected_revision: u64,
        #[arg(long)]
        attempt: u64,
    },
    /// Invite a current name or registered peer; ownership transfers only on accept.
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
        #[command(flatten)]
        #[serde(flatten)]
        long_horizon: LongHorizonArgs,
    },
    /// Save project, goal, or task-scoped work assignment policy.
    Policy {
        #[arg(long)]
        scope: String,
        #[arg(long)]
        id: Option<String>,
        #[arg(long)]
        work_kind: Option<String>,
        #[arg(long)]
        prefer_role: Option<String>,
        #[arg(long)]
        executor: Option<String>,
        #[arg(long)]
        reviewer: Option<String>,
        #[arg(long)]
        clear_prefer_role: bool,
        #[arg(long)]
        clear_executor: bool,
        #[arg(long)]
        clear_reviewer: bool,
        #[arg(long)]
        expected_revision: Option<u64>,
    },
    /// Resolve task policy and invite the selected executor.
    Dispatch {
        id: String,
        #[arg(long)]
        expected_revision: u64,
        #[command(flatten)]
        #[serde(flatten)]
        long_horizon: LongHorizonArgs,
    },
    /// Master-only repair: recreate the current Execute phase of an admitted
    /// task that predates the ticket-phase machine, preserving its owner. With
    /// `--admit-legacy`, explicitly admit one public, described, unadmitted
    /// `working` or `rework` task that a stable owner already accepted and has
    /// no phase records of its own.
    BackfillExecute {
        id: String,
        #[arg(long)]
        expected_revision: u64,
        /// Explicitly admit an owned legacy task and record one claimed
        /// Execute phase in the same batch. Without this flag the command only
        /// repairs an already admitted ticket.
        #[arg(long)]
        #[serde(default)]
        admit_legacy: bool,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BoardInvitationSource {
    pub project_scope: String,
    pub name: String,
    #[serde(default)]
    /// Source assignment generation captured before this invitation.
    pub assignment_revision: u64,
    #[serde(default)]
    pub schedule_round_ms: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BoardInvitation {
    pub peer_id: String,
    pub binding_id: String,
    pub message_id: String,
    pub created_ms: i64,
    #[serde(default)]
    pub original_status: Option<String>,
    #[serde(default)]
    pub original_owner: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub automatic_source: Option<BoardInvitationSource>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BoardTaskDetails {
    pub title: String,
    pub description: String,
    pub delivery_condition: String,
    pub test_condition: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub long_horizon: Option<LongHorizonTaskSpec>,
    pub revision: u64,
    pub public_visibility: bool,
    /// True only after ticket admission atomically created the ticket phases.
    /// Legacy/public board records are not admitted tickets until this flag is set.
    #[serde(default)]
    pub admitted: bool,
    #[serde(default)]
    pub parent_task_id: Option<String>,
    #[serde(default)]
    pub work_policy: WorkPolicy,
    #[serde(default)]
    pub child_work_policies: std::collections::BTreeMap<String, WorkPolicy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dispatch_selection: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dispatch_reason: Option<String>,
    pub invitation: Option<BoardInvitation>,
    pub last_response: Option<String>,
}

impl BoardTaskDetails {
    pub fn legacy(id: &str, public_visibility: bool) -> Self {
        Self {
            title: id.into(),
            revision: 1,
            public_visibility,
            ..Self::default()
        }
    }
}

/// Deliberate allowlist: no tokens, runtime endpoints, role briefs, child
/// identity or internal resource records.
#[derive(Debug, Serialize)]
pub struct BoardTaskView {
    pub id: String,
    pub title: String,
    pub description: String,
    pub delivery_condition: String,
    pub test_condition: String,
    pub long_horizon: Option<LongHorizonTaskSpec>,
    pub revision: u64,
    pub owner: String,
    pub owner_display_name: String,
    pub owner_name: Option<String>,
    pub owner_session_id: Option<String>,
    pub reassignment_reason: Option<String>,
    pub needs_reassignment: bool,
    pub publisher: String,
    pub publisher_display_name: String,
    pub priority: String,
    pub status: String,
    pub next_step: Option<String>,
    pub work_kind: Option<String>,
    pub parent_task_id: Option<String>,
    pub child_work_policies: std::collections::BTreeMap<String, WorkPolicy>,
    pub preferred_roles: Vec<String>,
    pub executor_name: Option<String>,
    pub reviewer_name: Option<String>,
    pub dispatch_selection: Option<String>,
    pub dispatch_reason: Option<String>,
    pub review_subject_revision: u64,
    pub reviewed_subject_revision: Option<u64>,
    pub actual_reviewer_name: Option<String>,
    pub completion_delivery_complete: bool,
    pub completion_missing: Vec<String>,
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
    pub display_name: String,
    pub role: String,
    pub work_roles: Vec<String>,
    pub status: String,
    pub task_ids: Vec<String>,
    pub invitation_ids: Vec<String>,
}
