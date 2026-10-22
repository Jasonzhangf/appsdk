use serde::{Deserialize, Serialize};

use super::state::State;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NameOccupancy {
    pub name: String,
    pub worker_id: String,
    pub session_id: String,
    pub occupancy_revision: u64,
    pub updated_ms: i64,
}

impl NameOccupancy {
    pub fn reference(&self, project_scope: &str) -> NameOccupancyRef {
        NameOccupancyRef {
            project_scope: project_scope.to_owned(),
            name: self.name.clone(),
            worker_id: self.worker_id.clone(),
            session_id: self.session_id.clone(),
            occupancy_revision: self.occupancy_revision,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NameOccupancyRef {
    pub project_scope: String,
    pub name: String,
    pub worker_id: String,
    pub session_id: String,
    pub occupancy_revision: u64,
}

impl NameOccupancyRef {
    pub fn matches(&self, project_scope: &str, occupancy: &NameOccupancy) -> bool {
        self.project_scope == project_scope
            && self.name == occupancy.name
            && self.worker_id == occupancy.worker_id
            && self.session_id == occupancy.session_id
            && self.occupancy_revision == occupancy.occupancy_revision
    }
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
pub struct NameBindingReference {
    pub app_scope_id: String,
    pub binding_id: String,
    pub endpoint_generation: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NameRecoveryRequest {
    pub invocation: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requested_name: Option<String>,
    pub caller_worker_id: String,
    pub caller_session_id: String,
    pub caller_binding: NameBindingReference,
    #[serde(default)]
    pub replace_confirmed: bool,
    pub requested_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NameRecoveryPending {
    pub operation_id: String,
    pub project_scope: String,
    pub target_name: String,
    pub occupancy_ref: NameOccupancyRef,
    pub request: NameRecoveryRequest,
    pub progress: NameRecoveryProgress,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decision: Option<NameRecoveryDecision>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<NameRecoveryResult>,
    pub created_ms: i64,
    pub updated_ms: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NameRecoveryProgress {
    Pending,
    AdjudicationRequested,
    Applying,
    Applied,
    StaleDecision,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NameRecoveryDecision {
    pub verdict: NameRecoveryVerdict,
    pub evidence: String,
    pub decided_by: String,
    pub decided_ms: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NameRecoveryVerdict {
    Online,
    Offline,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NameRecoveryResult {
    pub outcome: NameRecoveryOutcome,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    pub recorded_ms: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NameRecoveryOutcome {
    Applied,
    StaleDecision,
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

    pub fn name_occupancy_ref(&self, project_scope: &str, name: &str) -> Option<NameOccupancyRef> {
        self.name_occupancy(project_scope, name)
            .map(|occupancy| occupancy.reference(project_scope))
    }

    pub fn task_assignment(&self, task_id: &str) -> Option<&TaskAssignment> {
        self.task_assignments.get(task_id)
    }

    pub fn name_recovery(&self, operation_id: &str) -> Option<&NameRecoveryPending> {
        self.name_recoveries.get(operation_id)
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

    pub(crate) fn apply_name_recovery(
        &mut self,
        recovery: &NameRecoveryPending,
    ) -> Result<(), String> {
        validate_name_recovery(recovery)?;
        self.retain_name_occupancy_revision(
            &recovery.occupancy_ref.project_scope,
            &recovery.occupancy_ref.name,
            recovery.occupancy_ref.occupancy_revision,
        );
        self.name_recoveries
            .insert(recovery.operation_id.clone(), recovery.clone());
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

fn validate_name_recovery(recovery: &NameRecoveryPending) -> Result<(), String> {
    if recovery.operation_id.trim().is_empty()
        || recovery.project_scope.trim().is_empty()
        || recovery.target_name.trim().is_empty()
        || recovery.request.invocation.trim().is_empty()
        || recovery.request.caller_worker_id.trim().is_empty()
        || recovery.request.caller_session_id.trim().is_empty()
        || recovery
            .request
            .caller_binding
            .app_scope_id
            .trim()
            .is_empty()
        || recovery.request.caller_binding.binding_id.trim().is_empty()
        || recovery.request.caller_binding.endpoint_generation == 0
        || recovery.occupancy_ref.project_scope != recovery.project_scope
        || recovery.occupancy_ref.name != recovery.target_name
    {
        return Err(format!(
            "name recovery {} has incomplete request or occupancy reference",
            recovery.operation_id
        ));
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
