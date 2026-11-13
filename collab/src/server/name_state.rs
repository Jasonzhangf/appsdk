use serde::{Deserialize, Serialize};

use super::state::State;

/// Current pane and exact invocation directory for a name's execution carrier.
/// The socket path disambiguates the same pane id across tmux servers without
/// becoming an identity anchor. `cwd` is the precise call directory, never the
/// canonical project root.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NamePaneAnchor {
    pub pane: crate::proto::TmuxPaneAnchor,
    pub cwd: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NameOccupancy {
    pub name: String,
    pub worker_id: String,
    pub session_id: String,
    pub occupancy_revision: u64,
    pub updated_ms: i64,
    /// Current pane and exact invocation cwd, replaced on each rebind. Legacy
    /// records decode this as `None`; history is never scanned to backfill it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pane_anchor: Option<NamePaneAnchor>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskAssignment {
    pub task_id: String,
    pub name: String,
    pub session_id: String,
    /// Assignment commit generation. Reflow delivery progress keeps this value.
    pub assignment_revision: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reflow: Option<TaskSchedulingReflow>,
    pub updated_ms: i64,
}

/// Fixed automatic identity-return wait. One production constant; no
/// configurable fast-forward and no startup test hook.
pub(crate) const IDENTITY_RETURN_WAIT_MS: i64 = 900_000;

/// One current reassignment schedule for a stable name. This is the single
/// durable scheduling relation. It is independent of `NameOccupancy`, so
/// releasing or replacing the occupancy never cancels the wait. It never
/// grants identity and never carries credentials; the relation reference only
/// identifies stale events for the same generation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum NameReassignmentSchedule {
    /// A sufficiently confirmed loss of the current holder waits a fixed
    /// interval for that holder or a same-name successor to return online.
    /// The deadline is set once. A repeated loss, a restart and a capacity
    /// change keep it.
    WaitingReturn {
        lost_relation_ref: String,
        started_ms: i64,
        deadline_ms: i64,
    },
    /// An explicit human retirement already released the name, so the name is
    /// immediately eligible for reassignment. It must not take the automatic
    /// return wait.
    EligibleAfterRetirement {
        retired_relation_ref: String,
        retired_ms: i64,
    },
}

impl NameReassignmentSchedule {
    pub(crate) fn round_ms(&self) -> i64 {
        match self {
            Self::WaitingReturn { started_ms, .. } => *started_ms,
            Self::EligibleAfterRetirement { retired_ms, .. } => *retired_ms,
        }
    }

    /// Whether reassignment capacity may be admitted for this name at `now`.
    pub(crate) fn is_due(&self, now: i64) -> bool {
        match self {
            Self::WaitingReturn { deadline_ms, .. } => now >= *deadline_ms,
            Self::EligibleAfterRetirement { .. } => true,
        }
    }

    /// Whether this is still the schedule round recorded by an automatic
    /// invitation. A cleared schedule means the holder returned; a different
    /// relation means the invitation belongs to a superseded round.
    pub(crate) fn is_current_for_source(&self, name: &str) -> bool {
        let reference = reassignment_relation_ref(name);
        match self {
            Self::WaitingReturn {
                lost_relation_ref, ..
            } => reference == *lost_relation_ref,
            Self::EligibleAfterRetirement {
                retired_relation_ref,
                ..
            } => reference == *retired_relation_ref,
        }
    }
}

pub(crate) fn reassignment_relation_ref(name: &str) -> String {
    name.to_owned()
}

/// Start or keep the fixed return wait for one name. The first confirmed loss
/// sets the deadline. A repeated loss or restart keeps it. An existing
/// explicit-retirement eligibility is never downgraded to a wait.
pub(crate) fn start_return_wait(
    existing: Option<&NameReassignmentSchedule>,
    lost_relation_ref: &str,
    confirmed_ms: i64,
) -> NameReassignmentSchedule {
    match existing {
        Some(schedule @ NameReassignmentSchedule::WaitingReturn { .. })
        | Some(schedule @ NameReassignmentSchedule::EligibleAfterRetirement { .. }) => {
            schedule.clone()
        }
        None => NameReassignmentSchedule::WaitingReturn {
            lost_relation_ref: lost_relation_ref.to_owned(),
            started_ms: confirmed_ms,
            deadline_ms: confirmed_ms.saturating_add(IDENTITY_RETURN_WAIT_MS),
        },
    }
}

/// Replace any automatic return wait with immediate explicit-retirement
/// eligibility. The caller must hold a typed explicit human-retirement cause;
/// an ordinary close or self-unregister is not this cause.
pub(crate) fn eligible_after_retirement(
    retired_relation_ref: &str,
    retired_ms: i64,
) -> NameReassignmentSchedule {
    NameReassignmentSchedule::EligibleAfterRetirement {
        retired_relation_ref: retired_relation_ref.to_owned(),
        retired_ms,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskSchedulingReflow {
    pub episode: u64,
    pub reason: String,
    pub delivery: SchedulingReflowDelivery,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message_id: Option<String>,
    pub updated_ms: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SchedulingReflowDelivery {
    Pending,
    Delivered,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentPresenceRecord {
    pub project_scope: String,
    pub worker_id: String,
    pub session_id: String,
    pub presence: AgentPresence,
    pub reason: String,
    pub observed_ms: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentPresence {
    Online,
    Offline,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentActivityRecord {
    pub project_scope: String,
    pub worker_id: String,
    pub session_id: String,
    pub activity: AgentActivity,
    #[serde(default)]
    pub background: bool,
    pub reason: String,
    pub observed_ms: i64,
    /// Trusted continuous online-idle start for the current binding/session/
    /// generation. `None` means the current observation is not part of a
    /// trusted idle interval. Missing facts never backfill an interval from
    /// `observed_ms`.
    #[serde(default)]
    pub idle_since_ms: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentActivity {
    Busy,
    Idle,
    WaitingUser,
    Error,
    Unknown,
}

impl State {
    /// Current reassignment schedule for one project/stable name, if any. This
    /// is scheduling state only; it never proves a current identity.
    pub(crate) fn name_reassignment(
        &self,
        project_scope: &str,
        name: &str,
    ) -> Option<&NameReassignmentSchedule> {
        self.name_reassignments
            .get(project_scope)
            .and_then(|names| names.get(name))
    }

    pub(crate) fn name_occupancy_revision(&self, project_scope: &str, name: &str) -> u64 {
        self.name_occupancy_revisions
            .get(project_scope)
            .and_then(|names| names.get(name))
            .copied()
            .unwrap_or(0)
    }

    pub(crate) fn retain_name_occupancy_revision(
        &mut self,
        project_scope: &str,
        name: &str,
        revision: u64,
    ) {
        let retained = self
            .name_occupancy_revisions
            .entry(project_scope.to_owned())
            .or_default()
            .entry(name.to_owned())
            .or_default();
        *retained = (*retained).max(revision);
    }

    pub fn name_occupancy(&self, project_scope: &str, name: &str) -> Option<&NameOccupancy> {
        self.name_occupancies
            .get(project_scope)
            .and_then(|names| names.get(name))
    }

    pub fn task_assignment(&self, task_id: &str) -> Option<&TaskAssignment> {
        self.task_assignments.get(task_id)
    }

    pub fn agent_presence(
        &self,
        project_scope: &str,
        worker_id: &str,
    ) -> Option<&AgentPresenceRecord> {
        self.agent_presence
            .get(project_scope)
            .and_then(|workers| workers.get(worker_id))
    }

    pub fn agent_activity(
        &self,
        project_scope: &str,
        worker_id: &str,
    ) -> Option<&AgentActivityRecord> {
        self.agent_activity
            .get(project_scope)
            .and_then(|workers| workers.get(worker_id))
    }

    pub(crate) fn apply_name_occupancy(
        &mut self,
        project_scope: &str,
        occupancy: &NameOccupancy,
    ) -> Result<(), String> {
        validate_occupancy(project_scope, occupancy)?;
        self.commit_name_occupancy(project_scope, occupancy.clone());
        Ok(())
    }

    pub(crate) fn apply_name_resolution(
        &mut self,
        project_scope: &str,
        occupancy: &NameOccupancy,
        assignments: &[TaskAssignment],
    ) -> Result<(), String> {
        validate_occupancy(project_scope, occupancy)?;
        for assignment in assignments {
            validate_assignment(assignment)?;
            if assignment.name != occupancy.name || assignment.session_id != occupancy.session_id {
                return Err(format!(
                    "name resolution assignment {} does not match occupancy {} / {}",
                    assignment.task_id, occupancy.name, occupancy.session_id
                ));
            }
        }

        self.commit_name_occupancy(project_scope, occupancy.clone());
        for assignment in assignments {
            self.task_assignments
                .insert(assignment.task_id.clone(), assignment.clone());
        }
        Ok(())
    }

    pub(crate) fn apply_task_assignment(
        &mut self,
        assignment: &TaskAssignment,
    ) -> Result<(), String> {
        validate_assignment(assignment)?;
        self.task_assignments
            .insert(assignment.task_id.clone(), assignment.clone());
        Ok(())
    }

    pub(crate) fn apply_agent_presence(
        &mut self,
        presence: &AgentPresenceRecord,
    ) -> Result<(), String> {
        validate_presence(presence)?;
        self.agent_presence
            .entry(presence.project_scope.clone())
            .or_default()
            .insert(presence.worker_id.clone(), presence.clone());
        Ok(())
    }

    pub(crate) fn apply_agent_activity(
        &mut self,
        activity: &AgentActivityRecord,
    ) -> Result<(), String> {
        validate_activity(activity)?;
        self.agent_activity
            .entry(activity.project_scope.clone())
            .or_default()
            .insert(activity.worker_id.clone(), activity.clone());
        Ok(())
    }

    fn commit_name_occupancy(&mut self, project_scope: &str, occupancy: NameOccupancy) {
        // Releasing an old name must not release that name's epoch.
        self.retain_name_occupancy_revision(
            project_scope,
            &occupancy.name,
            occupancy.occupancy_revision,
        );
        let occupancies = self
            .name_occupancies
            .entry(project_scope.to_owned())
            .or_default();
        // One agent session has one current name carrier. A later commit for
        // the same session releases the previous worker's names too, while
        // task assignments and message history stay in their own records.
        occupancies.retain(|name, current| {
            name == &occupancy.name
                || (current.worker_id != occupancy.worker_id
                    && current.session_id != occupancy.session_id)
        });
        occupancies.insert(occupancy.name.clone(), occupancy);
    }
}

fn validate_occupancy(project_scope: &str, occupancy: &NameOccupancy) -> Result<(), String> {
    if project_scope.trim().is_empty()
        || occupancy.name.trim().is_empty()
        || occupancy.worker_id.trim().is_empty()
        || occupancy.session_id.trim().is_empty()
        || occupancy.occupancy_revision == 0
    {
        return Err("name occupancy has incomplete project/name/session identity".into());
    }
    Ok(())
}

fn validate_assignment(assignment: &TaskAssignment) -> Result<(), String> {
    if assignment.task_id.trim().is_empty()
        || assignment.name.trim().is_empty()
        || assignment.session_id.trim().is_empty()
        || assignment.assignment_revision == 0
    {
        return Err(format!(
            "task assignment {} has incomplete name/session identity",
            assignment.task_id
        ));
    }
    if let Some(reflow) = &assignment.reflow {
        if reflow.episode == 0 || reflow.reason.trim().is_empty() {
            return Err(format!(
                "task assignment {} has incomplete scheduling reflow",
                assignment.task_id
            ));
        }
    }
    Ok(())
}

fn validate_presence(presence: &AgentPresenceRecord) -> Result<(), String> {
    if presence.project_scope.trim().is_empty()
        || presence.worker_id.trim().is_empty()
        || presence.session_id.trim().is_empty()
        || presence.reason.trim().is_empty()
    {
        return Err("agent presence has incomplete project/worker/session observation".into());
    }
    Ok(())
}

fn validate_activity(activity: &AgentActivityRecord) -> Result<(), String> {
    if activity.project_scope.trim().is_empty()
        || activity.worker_id.trim().is_empty()
        || activity.session_id.trim().is_empty()
        || activity.reason.trim().is_empty()
    {
        return Err("agent activity has incomplete project/worker/session observation".into());
    }
    Ok(())
}
