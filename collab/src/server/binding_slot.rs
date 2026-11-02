use crate::identity::AppServerId;
use crate::proto::SelectedTransport;
use crate::scope::ProjectScopeId;
use crate::server::global_state::RuntimeBinding;
use crate::server::name_state::NamePaneAnchor;
use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum PendingEffectKind {
    Register,
    Name,
}

/// Logical slot key. It deliberately does NOT include session-derived
/// agent_id or any transport/runtime fact; those live in the binding value.
/// The stable name owns work; app_scope is the binding partition inside the
/// same logical slot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct IdentityBindingKey {
    pub(crate) project_scope: ProjectScopeId,
    pub(crate) app_scope_id: AppServerId,
    pub(crate) name: String,
}

impl IdentityBindingKey {
    pub(crate) fn new(
        project_scope: ProjectScopeId,
        app_scope_id: AppServerId,
        name: impl Into<String>,
    ) -> Self {
        let name = name.into();
        assert!(!name.trim().is_empty(), "binding name must not be empty");
        Self {
            project_scope,
            app_scope_id,
            name,
        }
    }
}

/// A committed binding value. It intentionally carries no token, role grant,
/// subscription authority or other recoverable capability.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct BindingValue {
    pub(crate) binding: RuntimeBinding,
    pub(crate) transport: SelectedTransport,
}

impl BindingValue {
    pub(crate) fn new(binding: RuntimeBinding, transport: SelectedTransport) -> Self {
        Self { binding, transport }
    }

    fn validate(&self) -> Result<(), SlotError> {
        self.binding
            .validate()
            .map_err(|error| SlotError::Invalid(error.to_string()))
    }

    /// Confirm the value belongs to the given slot partition. The agent_id
    /// (session-derived carrier) is deliberately not part of this check.
    fn scope_matches(&self, key: &IdentityBindingKey) -> bool {
        self.binding.project_scope == key.project_scope
            && self.binding.app_scope_id == key.app_scope_id
    }

    /// Compare only durable routing facts. Capability ordering, self-check
    /// text, readiness and the control generation are not binding changes.
    fn same_fact(&self, other: &Self) -> bool {
        self.binding.project_scope == other.binding.project_scope
            && self.binding.app_scope_id == other.binding.app_scope_id
            && self.binding.agent_id == other.binding.agent_id
            && self.binding.runtime_id == other.binding.runtime_id
            && self.binding.binding_id == other.binding.binding_id
            && self.binding.session_id == other.binding.session_id
            && self.binding.native_thread_id == other.binding.native_thread_id
            && self.binding.tmux_endpoint == other.binding.tmux_endpoint
            && self.transport.kind == other.transport.kind
            && self.transport.endpoint == other.transport.endpoint
            && self.transport.namespace == other.transport.namespace
            && self.transport.session_id == other.transport.session_id
            && self.transport.thread_id == other.transport.thread_id
            && self.transport.tmux_endpoint == other.transport.tmux_endpoint
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum IdentityStatus {
    Active,
    Unregistered,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct IdentityBindingSlot {
    pub(crate) identity_key: IdentityBindingKey,
    pub(crate) identity_status: IdentityStatus,
    pub(crate) generation: u64,
    pub(crate) current: Option<BindingValue>,
    pub(crate) previous: Option<BindingValue>,
}

/// Bounded handoff intent persisted with the control set. It records only the
/// exact cross-file work that must be consumed after the control slot is
/// published: the operation, scope, stable name, and the before/after carrier
/// references. No task, message, subscription or binding payload is copied.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PendingEffect {
    pub(crate) kind: PendingEffectKind,
    pub(crate) effect_id: String,
    pub(crate) operation_id: String,
    pub(crate) project_scope: ProjectScopeId,
    pub(crate) app_scope_id: AppServerId,
    pub(crate) name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) previous_worker_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) previous_session_id: Option<String>,
    pub(crate) target_worker_id: String,
    pub(crate) target_session_id: String,
    /// Stable name revision the business handoff will commit. Reusing the same
    /// revision makes a retry idempotent for mailbox-handoff dedup.
    pub(crate) occupancy_revision: u64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) source_names: Vec<String>,
    /// Whether recovery must apply the source names as a business rename.
    /// Role recovery retires its old control name but leaves the stable-name
    /// mailbox and task assignment for that name in place.
    #[serde(default)]
    pub(crate) moves_existing_names: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) pane_anchor: Option<NamePaneAnchor>,
    pub(crate) created_ms: i64,
}

impl PendingEffect {
    pub(crate) fn validate(&self) -> Result<(), SlotError> {
        if self.effect_id.trim().is_empty()
            || self.operation_id.trim().is_empty()
            || self.name.trim().is_empty()
            || self.target_worker_id.trim().is_empty()
            || self.target_session_id.trim().is_empty()
            || self.occupancy_revision == 0
        {
            return Err(SlotError::Invalid(
                "pending effect requires effect, operation, name, target carrier and revision"
                    .into(),
            ));
        }
        if self
            .previous_worker_id
            .as_deref()
            .is_some_and(|worker| worker == self.target_worker_id)
            && self.previous_session_id.as_deref() == Some(self.target_session_id.as_str())
            && self.source_names.is_empty()
        {
            return Err(SlotError::Invariant(
                "pending effect must describe a carrier or name change".into(),
            ));
        }
        Ok(())
    }

    pub(crate) fn matches_scope(
        &self,
        project_scope: &ProjectScopeId,
        app_scope_id: &AppServerId,
    ) -> bool {
        &self.project_scope == project_scope && &self.app_scope_id == app_scope_id
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CommitOutcome {
    Committed { generation: u64 },
    Unchanged { generation: u64 },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SlotError {
    Invalid(String),
    KeyMismatch {
        expected: IdentityBindingKey,
        observed: IdentityBindingKey,
    },
    GenerationConflict {
        expected: u64,
        observed: u64,
    },
    Invariant(String),
}

impl fmt::Display for SlotError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid(reason) => write!(formatter, "invalid binding slot: {reason}"),
            Self::KeyMismatch { expected, observed } => write!(
                formatter,
                "binding key mismatch: expected {}, observed {}",
                identity_key_label(expected),
                identity_key_label(observed)
            ),
            Self::GenerationConflict { expected, observed } => write!(
                formatter,
                "binding generation conflict: expected {expected}, observed {observed}"
            ),
            Self::Invariant(reason) => write!(formatter, "binding slot invariant failed: {reason}"),
        }
    }
}

impl std::error::Error for SlotError {}

fn identity_key_label(key: &IdentityBindingKey) -> String {
    format!(
        "({}, {}, name={})",
        key.project_scope.as_str(),
        key.app_scope_id.as_str(),
        key.name
    )
}

impl IdentityBindingSlot {
    pub(crate) fn new(identity_key: IdentityBindingKey) -> Self {
        Self {
            identity_key,
            identity_status: IdentityStatus::Unregistered,
            generation: 0,
            current: None,
            previous: None,
        }
    }

    pub(crate) fn identity_status(&self) -> IdentityStatus {
        self.identity_status
    }

    pub(crate) fn generation(&self) -> u64 {
        self.generation
    }

    pub(crate) fn active_binding(&self) -> Option<&BindingValue> {
        if self.identity_status == IdentityStatus::Active {
            self.current.as_ref()
        } else {
            None
        }
    }

    pub(crate) fn reference_previous(&self) -> Option<&BindingValue> {
        self.previous.as_ref()
    }

    /// Commit a new binding fact for this logical slot. Repeating the current
    /// fact is a no-op. A different fact becomes current and the old current
    /// becomes the only previous value; any earlier previous value is dropped.
    pub(crate) fn commit(
        &mut self,
        key: &IdentityBindingKey,
        value: BindingValue,
        expected_generation: Option<u64>,
    ) -> Result<CommitOutcome, SlotError> {
        self.check_expected(expected_generation)?;
        self.validate_shape()?;
        value.validate()?;
        if !value.scope_matches(key) {
            return Err(SlotError::KeyMismatch {
                expected: key.clone(),
                observed: IdentityBindingKey::new(
                    value.binding.project_scope.clone(),
                    value.binding.app_scope_id.clone(),
                    key.name.clone(),
                ),
            });
        }

        if let Some(current) = self.current.as_ref() {
            if current.same_fact(&value) {
                return Ok(CommitOutcome::Unchanged {
                    generation: self.generation,
                });
            }
        }

        if value.binding.endpoint_generation == 0 {
            return Err(SlotError::Invalid(
                "binding endpoint generation must be non-zero".into(),
            ));
        }
        let next = self.next_generation()?;
        if let Some(current) = self.current.take() {
            self.previous = Some(current);
        }
        self.current = Some(value);
        self.identity_status = IdentityStatus::Active;
        self.generation = next;
        Ok(CommitOutcome::Committed { generation: next })
    }

    /// Close the current binding while keeping it as the reference previous
    /// value. Repeating close is a no-op and never erases that reference.
    pub(crate) fn unregister(
        &mut self,
        expected_generation: Option<u64>,
    ) -> Result<CommitOutcome, SlotError> {
        self.check_expected(expected_generation)?;
        self.validate_shape()?;
        if self.identity_status == IdentityStatus::Unregistered {
            return Ok(CommitOutcome::Unchanged {
                generation: self.generation,
            });
        }
        let current = self
            .current
            .take()
            .ok_or_else(|| SlotError::Invariant("active slot has no current binding".into()))?;
        let next = self.next_generation()?;
        self.previous = Some(current);
        self.identity_status = IdentityStatus::Unregistered;
        self.generation = next;
        Ok(CommitOutcome::Committed { generation: next })
    }

    pub(crate) fn validate(&self) -> Result<(), SlotError> {
        self.validate_shape()?;
        if let Some(current) = self.current.as_ref() {
            current.validate()?;
        }
        if let Some(previous) = self.previous.as_ref() {
            previous.validate()?;
        }
        if let (Some(current), Some(previous)) = (self.current.as_ref(), self.previous.as_ref()) {
            if current.same_fact(previous) {
                return Err(SlotError::Invariant(
                    "current and previous bindings must be different facts".into(),
                ));
            }
        }
        Ok(())
    }

    fn check_expected(&self, expected_generation: Option<u64>) -> Result<(), SlotError> {
        if let Some(expected) = expected_generation {
            if expected != self.generation {
                return Err(SlotError::GenerationConflict {
                    expected: self.generation,
                    observed: expected,
                });
            }
        }
        Ok(())
    }

    fn next_generation(&self) -> Result<u64, SlotError> {
        self.generation
            .checked_add(1)
            .ok_or_else(|| SlotError::Invariant("binding generation overflow".into()))
    }

    fn validate_shape(&self) -> Result<(), SlotError> {
        match (self.identity_status, self.current.is_some()) {
            (IdentityStatus::Active, false) => Err(SlotError::Invariant(
                "active slot requires a current binding".into(),
            )),
            (IdentityStatus::Unregistered, true) => Err(SlotError::Invariant(
                "unregistered slot cannot expose a current binding".into(),
            )),
            _ => Ok(()),
        }
    }
}

/// The fixed current/previous set for every identity. It is the candidate
/// shape of the host control resource; persistence and startup projection
/// consume this set rather than deriving bindings from project journals.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub(crate) struct BindingControlSet {
    slots: Vec<IdentityBindingSlot>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    effects: Vec<PendingEffect>,
}

impl BindingControlSet {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn from_slots(slots: Vec<IdentityBindingSlot>) -> Self {
        Self {
            slots,
            effects: Vec::new(),
        }
    }

    pub(crate) fn from_parts(slots: Vec<IdentityBindingSlot>, effects: Vec<PendingEffect>) -> Self {
        Self { slots, effects }
    }

    pub(crate) fn slot(&self, key: &IdentityBindingKey) -> Option<&IdentityBindingSlot> {
        self.slots.iter().find(|slot| &slot.identity_key == key)
    }

    pub(crate) fn slots(&self) -> impl Iterator<Item = &IdentityBindingSlot> {
        self.slots.iter()
    }

    pub(crate) fn pending_effects(&self) -> impl Iterator<Item = &PendingEffect> {
        self.effects.iter()
    }

    pub(crate) fn pending_effect_for_scope<'a>(
        &'a self,
        project_scope: &'a ProjectScopeId,
        app_scope_id: &'a AppServerId,
    ) -> impl Iterator<Item = &'a PendingEffect> {
        self.effects
            .iter()
            .filter(move |effect| effect.matches_scope(project_scope, app_scope_id))
    }

    pub(crate) fn pending_effect(&self, effect_id: &str) -> Option<&PendingEffect> {
        self.effects
            .iter()
            .find(|effect| effect.effect_id == effect_id)
    }

    pub(crate) fn set_effect(&mut self, effect: PendingEffect) -> Result<bool, SlotError> {
        effect.validate()?;
        if let Some(existing) = self
            .effects
            .iter_mut()
            .find(|existing| existing.effect_id == effect.effect_id)
        {
            let changed = *existing != effect;
            *existing = effect;
            Ok(changed)
        } else {
            self.effects.push(effect);
            Ok(true)
        }
    }

    pub(crate) fn clear_effect(&mut self, effect_id: &str) -> bool {
        let before = self.effects.len();
        self.effects.retain(|effect| effect.effect_id != effect_id);
        self.effects.len() != before
    }

    /// Active current bindings are the only values that may enter routing,
    /// authentication, subscription or delivery projections.
    pub(crate) fn active_values(&self) -> impl Iterator<Item = &BindingValue> {
        self.slots
            .iter()
            .filter_map(IdentityBindingSlot::active_binding)
    }

    pub(crate) fn commit(
        &mut self,
        key: &IdentityBindingKey,
        value: BindingValue,
        expected_generation: Option<u64>,
    ) -> Result<CommitOutcome, SlotError> {
        let index = self.slots.iter().position(|slot| &slot.identity_key == key);
        let index = match index {
            Some(index) => index,
            None => {
                self.slots.push(IdentityBindingSlot::new(key.clone()));
                self.slots.len() - 1
            }
        };
        self.slots[index].commit(key, value, expected_generation)
    }

    pub(crate) fn unregister(
        &mut self,
        key: &IdentityBindingKey,
        expected_generation: Option<u64>,
    ) -> Result<CommitOutcome, SlotError> {
        let slot = self
            .slots
            .iter_mut()
            .find(|slot| &slot.identity_key == key)
            .ok_or_else(|| SlotError::Invariant("identity is not in the control set".into()))?;
        slot.unregister(expected_generation)
    }

    /// Move one carrier onto the target stable name inside a single control
    /// candidate. The target slot is committed first (so its displaced holder
    /// is preserved as `previous`), then every other name the same carrier
    /// previously held is retired. Source names must belong to the same
    /// project/app partition as the target; a cross-partition move is invalid.
    pub(crate) fn move_name(
        &mut self,
        target: &IdentityBindingKey,
        value: BindingValue,
        source_names: &[String],
        expected_generation: Option<u64>,
    ) -> Result<MoveNameOutcome, SlotError> {
        let target_outcome = self.commit(target, value, expected_generation)?;
        let mut sources_retired = 0;
        for name in source_names {
            if name == &target.name {
                continue;
            }
            let key = IdentityBindingKey::new(
                target.project_scope.clone(),
                target.app_scope_id.clone(),
                name.clone(),
            );
            if self.slot(&key).is_none() {
                continue;
            }
            match self.unregister(&key, None)? {
                CommitOutcome::Committed { .. } => sources_retired += 1,
                CommitOutcome::Unchanged { .. } => {}
            }
        }
        Ok(MoveNameOutcome {
            target: target_outcome,
            sources_retired,
        })
    }

    /// Remove every slot and pending effect in the given project scope. Used by
    /// project reset to retire identities and their unfinished handoff intents
    /// in the same control commit as their route lines. A leftover effect whose
    /// route is gone would otherwise block startup recovery instead of staying
    /// retired with its scope.
    pub(crate) fn remove_scope(&mut self, project_scope: &str) -> usize {
        let before = self.slots.len();
        self.slots
            .retain(|slot| slot.identity_key.project_scope.as_str() != project_scope);
        self.effects
            .retain(|effect| effect.project_scope.as_str() != project_scope);
        before - self.slots.len()
    }

    pub(crate) fn validate(&self) -> Result<(), SlotError> {
        for (index, slot) in self.slots.iter().enumerate() {
            slot.validate()?;
            if self.slots[..index]
                .iter()
                .any(|earlier| earlier.identity_key == slot.identity_key)
            {
                return Err(SlotError::Invariant(
                    "control set contains a duplicate identity key".into(),
                ));
            }
        }
        for effect in &self.effects {
            effect.validate()?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct MoveNameOutcome {
    pub(crate) target: CommitOutcome,
    pub(crate) sources_retired: usize,
}
