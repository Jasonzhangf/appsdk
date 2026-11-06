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
    pub assignment_revision: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reflow: Option<TaskSchedulingReflow>,
    pub updated_ms: i64,
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
    Unknown,
}

impl State {
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
