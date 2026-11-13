use crate::identity::AppServerId;
use crate::proto::SelectedTransport;
use crate::scope::ProjectScopeId;
use crate::server::global_state::RuntimeBinding;
use crate::server::name_state::NamePaneAnchor;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::cmp::Ordering;
use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum PendingEffectKind {
    Register,
    RegisterAcrossScopes {
        sources: Vec<CrossScopeSourceRef>,
    },
    Name,
    /// Explicit takeover where one or more source currents live in other app
    /// scopes. Every exact source current is released in the same control
    /// candidate as the target name publication.
    NameAcrossScopes {
        sources: Vec<CrossScopeSourceRef>,
    },
    Retire,
    /// Explicit human `retire_name`. Distinct from the legacy `Retire`
    /// self-unregister path so recovery releases the exact target binding
    /// without reading the target token or calling `unregister_self`.
    HumanRetire {
        binding_id: crate::identity::BindingId,
        generation: u64,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum RoleAction {
    SetMaster,
    ClearMaster,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RoleEffect {
    pub(crate) action: RoleAction,
    pub(crate) authorization: RoleAuthorization,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum RoleAuthorization {
    User {
        authorization: String,
        granted_by: String,
    },
    Delegate {
        actor: String,
        target: String,
        grant_id: String,
        grant_generation: u64,
    },
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
    pub(crate) tab_id: LogicalTab,
    pub(crate) host: BindingHost,
    pub(crate) invocation_cwd: String,
    pub(crate) physical_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) layout_binding_id: Option<String>,
    /// Daemon-verified owner process of the current Rex execution. The
    /// project, app, agent session and endpoint come from this same value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) rex_execution: Option<VerifiedRexExecutionAssociation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) rex_execution_fence: Option<RexExecutionFence>,
}

/// Current admission boundary in the local process-start clock domain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RexExecutionFence {
    UnixWall {
        seconds: u64,
        microseconds: u32,
    },
    LinuxBoot {
        boot_id: String,
        ticks_per_second: u64,
        ticks: u64,
    },
}

impl RexExecutionFence {
    pub(crate) fn from_start_id(start: &str) -> Option<Self> {
        if let Some(rest) = start.strip_prefix("linux:") {
            let mut fields = rest.split(':');
            let boot_id = fields.next()?.to_owned();
            let ticks_per_second = fields.next()?.parse().ok()?;
            let ticks = fields.next()?.parse().ok()?;
            if fields.next().is_some() {
                return None;
            }
            return Self::linux_boot(boot_id, ticks_per_second, ticks);
        }
        let (seconds, microseconds) = start.split_once(':')?;
        let seconds = seconds.parse().ok()?;
        let microseconds = microseconds.parse().ok()?;
        Self::unix_wall(seconds, microseconds)
    }

    pub(crate) fn unix_wall(seconds: u64, microseconds: u32) -> Option<Self> {
        (microseconds < 1_000_000).then_some(Self::UnixWall {
            seconds,
            microseconds,
        })
    }

    pub(crate) fn linux_boot(boot_id: String, ticks_per_second: u64, ticks: u64) -> Option<Self> {
        if boot_id.trim() != boot_id
            || boot_id.is_empty()
            || boot_id.chars().any(|character| {
                character.is_whitespace() || character.is_control() || character == ':'
            })
            || ticks_per_second == 0
        {
            return None;
        }
        Some(Self::LinuxBoot {
            boot_id,
            ticks_per_second,
            ticks,
        })
    }

    fn validate(&self) -> bool {
        match self {
            Self::UnixWall { microseconds, .. } => *microseconds < 1_000_000,
            Self::LinuxBoot {
                boot_id,
                ticks_per_second,
                ..
            } => Self::linux_boot(boot_id.clone(), *ticks_per_second, 0).is_some(),
        }
    }

    pub(crate) fn compare(&self, other: &Self) -> Option<Ordering> {
        match (self, other) {
            (
                Self::UnixWall {
                    seconds: left_seconds,
                    microseconds: left_microseconds,
                },
                Self::UnixWall {
                    seconds: right_seconds,
                    microseconds: right_microseconds,
                },
            ) => Some(
                (*left_seconds, *left_microseconds).cmp(&(*right_seconds, *right_microseconds)),
            ),
            (
                Self::LinuxBoot {
                    boot_id: left_boot,
                    ticks_per_second: left_rate,
                    ticks: left_ticks,
                },
                Self::LinuxBoot {
                    boot_id: right_boot,
                    ticks_per_second: right_rate,
                    ticks: right_ticks,
                },
            ) if left_boot == right_boot && left_rate == right_rate => {
                Some(left_ticks.cmp(right_ticks))
            }
            _ => None,
        }
    }

    fn merge_comparable(destination: Option<Self>, incoming: Option<&Self>) -> Option<Self> {
        match (destination, incoming) {
            (None, Some(incoming)) => Some(incoming.clone()),
            (Some(destination), Some(incoming)) => match destination.compare(incoming) {
                Some(Ordering::Less) => Some(incoming.clone()),
                Some(Ordering::Equal | Ordering::Greater) | None => Some(destination),
            },
            (destination, None) => destination,
        }
    }
}

impl PartialOrd for RexExecutionFence {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        self.compare(other)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct UnixWallFenceJson {
    seconds: u64,
    microseconds: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum LinuxFenceClock {
    LinuxBoottime,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct LinuxBootFenceJson {
    clock: LinuxFenceClock,
    boot_id: String,
    ticks_per_second: u64,
    ticks: u64,
}

impl Serialize for RexExecutionFence {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            Self::UnixWall {
                seconds,
                microseconds,
            } => UnixWallFenceJson {
                seconds: *seconds,
                microseconds: *microseconds,
            }
            .serialize(serializer),
            Self::LinuxBoot {
                boot_id,
                ticks_per_second,
                ticks,
            } => LinuxBootFenceJson {
                clock: LinuxFenceClock::LinuxBoottime,
                boot_id: boot_id.clone(),
                ticks_per_second: *ticks_per_second,
                ticks: *ticks,
            }
            .serialize(serializer),
        }
    }
}

impl<'de> Deserialize<'de> for RexExecutionFence {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = serde_json::Value::deserialize(deserializer)?;
        if value.get("clock").is_some() {
            let linux: LinuxBootFenceJson =
                serde_json::from_value(value).map_err(serde::de::Error::custom)?;
            Self::linux_boot(linux.boot_id, linux.ticks_per_second, linux.ticks)
                .ok_or_else(|| serde::de::Error::custom("invalid Linux Rex execution fence"))
        } else {
            let unix: UnixWallFenceJson =
                serde_json::from_value(value).map_err(serde::de::Error::custom)?;
            Self::unix_wall(unix.seconds, unix.microseconds)
                .ok_or_else(|| serde::de::Error::custom("invalid Unix Rex execution fence"))
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct VerifiedRexExecutionAssociation {
    pub(crate) owner_pid: u32,
    pub(crate) owner_name: String,
    pub(crate) process_start_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct LogicalTab {
    pub(crate) project_scope: ProjectScopeId,
    pub(crate) app_scope_id: AppServerId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct BindingHost {
    pub(crate) agent_kind: Option<String>,
    pub(crate) host_type: String,
    pub(crate) profile: Option<String>,
}

impl BindingValue {
    pub(crate) fn new(binding: RuntimeBinding, transport: SelectedTransport) -> Self {
        let tab_id = LogicalTab {
            project_scope: binding.project_scope.clone(),
            app_scope_id: binding.app_scope_id.clone(),
        };
        let host_type = match transport.kind {
            crate::proto::TransportKind::Tmux => "tui",
            crate::proto::TransportKind::Dsh => "dsh",
            crate::proto::TransportKind::DesktopBridge => "codex_desktop",
            crate::proto::TransportKind::AppServer => "native",
            crate::proto::TransportKind::Rex => "rex",
        }
        .to_owned();
        Self {
            invocation_cwd: binding.project_scope.as_str().to_owned(),
            binding,
            transport,
            tab_id,
            host: BindingHost {
                agent_kind: None,
                host_type,
                profile: None,
            },
            physical_name: None,
            layout_binding_id: None,
            rex_execution: None,
            rex_execution_fence: None,
        }
    }

    pub(crate) fn preserve_rex_fence(&mut self, current: &Self) {
        self.rex_execution_fence = RexExecutionFence::merge_comparable(
            self.rex_execution_fence.take(),
            current.rex_execution_fence.as_ref(),
        );
        if self.rex_execution.is_none()
            && self.transport.kind == crate::proto::TransportKind::Rex
            && self.binding == current.binding
            && self.transport.rex_endpoint == current.transport.rex_endpoint
        {
            self.rex_execution = current.rex_execution.clone();
        }
    }

    /// Only explicit human replacement calls this producer. Current refresh
    /// and effect retries preserve the boundary without advancing it.
    pub(crate) fn replace_rex_holder(&mut self, target: &Self) -> Result<(), String> {
        self.preserve_rex_fence(target);
        if self.binding.agent_id == target.binding.agent_id
            && self.binding.session_id == target.binding.session_id
        {
            return Ok(());
        }
        let displaced = target
            .rex_execution
            .as_ref()
            .and_then(|execution| RexExecutionFence::from_start_id(&execution.process_start_id));
        let publication = crate::host_runtime::current_rex_execution_boundary()?;
        let boundary = displaced
            .filter(|boundary| boundary.compare(&publication).is_some())
            .unwrap_or(publication);
        self.rex_execution_fence = match self.rex_execution_fence.take() {
            None => Some(boundary),
            Some(existing) => match existing.compare(&boundary) {
                Some(Ordering::Less) => Some(boundary),
                Some(Ordering::Equal | Ordering::Greater) => Some(existing),
                None => Some(boundary),
            },
        };
        Ok(())
    }

    fn validate(&self) -> Result<(), SlotError> {
        if self
            .rex_execution_fence
            .as_ref()
            .is_some_and(|fence| !fence.validate())
        {
            return Err(SlotError::Invariant(
                "Rex execution fence contains invalid clock facts".into(),
            ));
        }
        if self.tab_id.project_scope != self.binding.project_scope
            || self.tab_id.app_scope_id != self.binding.app_scope_id
        {
            return Err(SlotError::Invariant(
                "logical tab differs from binding scope".into(),
            ));
        }
        if let Some(execution) = self.rex_execution.as_ref() {
            if self.transport.kind != crate::proto::TransportKind::Rex
                || self.transport.rex_endpoint.is_none()
            {
                return Err(SlotError::Invariant(
                    "Rex execution association requires the Rex transport".into(),
                ));
            }
            if execution.owner_pid == 0
                || execution.owner_name.trim().is_empty()
                || execution.owner_name.chars().any(char::is_control)
                || execution.process_start_id.trim().is_empty()
                || execution.process_start_id.chars().any(char::is_control)
            {
                return Err(SlotError::Invariant(
                    "Rex execution association fields are incomplete".into(),
                ));
            }
        }
        self.binding
            .validate()
            .map_err(|error| SlotError::Invalid(error.to_string()))
    }

    fn validate_for_publication(&self) -> Result<(), SlotError> {
        self.validate()?;
        if self.transport.kind == crate::proto::TransportKind::Rex
            && self.transport.rex_endpoint.as_ref().is_none_or(|endpoint| {
                endpoint.server.trim().is_empty()
                    || endpoint.session_id.trim().is_empty()
                    || endpoint.block_id.trim().is_empty()
            })
        {
            return Err(SlotError::Invalid(
                "Rex binding requires a complete server, session and block".into(),
            ));
        }
        Ok(())
    }

    fn rex_block_key(&self) -> Option<(&str, &str)> {
        (self.transport.kind == crate::proto::TransportKind::Rex)
            .then(|| self.transport.rex_endpoint.as_ref())
            .flatten()
            .map(|endpoint| (endpoint.server.as_str(), endpoint.block_id.as_str()))
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
            && self.transport.rex_endpoint == other.transport.rex_endpoint
            && self.tab_id == other.tab_id
            && self.host == other.host
            && self.invocation_cwd == other.invocation_cwd
            && self.physical_name == other.physical_name
            && self.layout_binding_id == other.layout_binding_id
            && self.rex_execution == other.rex_execution
            && self.rex_execution_fence == other.rex_execution_fence
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
    pub(crate) display_slot: Option<u32>,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) role: Option<RoleEffect>,
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

/// Exact source current selected for a cross-scope explicit takeover.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CrossScopeSourceRef {
    pub(crate) key: IdentityBindingKey,
    pub(crate) binding_id: crate::identity::BindingId,
    pub(crate) generation: u64,
}

impl PendingEffect {
    pub(crate) fn is_register(&self) -> bool {
        matches!(
            self.kind,
            PendingEffectKind::Register | PendingEffectKind::RegisterAcrossScopes { .. }
        )
    }

    pub(crate) fn is_name_like(&self) -> bool {
        matches!(
            self.kind,
            PendingEffectKind::Name | PendingEffectKind::NameAcrossScopes { .. }
        )
    }

    pub(crate) fn cross_scope_sources(&self) -> &[CrossScopeSourceRef] {
        match &self.kind {
            PendingEffectKind::NameAcrossScopes { sources }
            | PendingEffectKind::RegisterAcrossScopes { sources } => sources,
            _ => &[],
        }
    }

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
        let cross_scope_sources = self.cross_scope_sources();
        if !cross_scope_sources.is_empty() {
            let mut keys = std::collections::BTreeSet::new();
            for source in cross_scope_sources {
                if source.key.project_scope != self.project_scope
                    || source.key.app_scope_id == self.app_scope_id
                    || source.generation == 0
                    || source.binding_id.as_str().trim().is_empty()
                    || !keys.insert((
                        source.key.app_scope_id.as_str().to_owned(),
                        source.key.name.clone(),
                    ))
                {
                    return Err(SlotError::Invalid(
                        "cross-scope sources require exact, distinct other-app currents in the same project"
                            .into(),
                    ));
                }
            }
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
            display_slot: super::name_resolution::display_slot(&identity_key.name)
                .expect("validated identity name"),
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
        value.validate_for_publication()?;
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

        let next = self.next_generation()?;
        if let Some(current) = self.current.take() {
            self.previous = Some(current);
        }
        // Returning to a retired name can restore exactly the reference fact.
        // That reference has no distinct handoff information and cannot veto
        // the new current publication.
        if self
            .previous
            .as_ref()
            .is_some_and(|previous| previous.same_fact(&value))
        {
            self.previous = None;
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
        let expected_display_slot = super::name_resolution::display_slot(&self.identity_key.name)
            .map_err(SlotError::Invalid)?;
        if expected_display_slot.is_some() && expected_display_slot != self.display_slot {
            return Err(SlotError::Invariant(
                "display slot differs from the registered name".into(),
            ));
        }
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
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) pending_resets: Vec<String>,
}

/// Exact match of a caller's current Rex target against a saved binding. The
/// Rex `server`/`session`/`block` triple is the placement identity; `host_id`
/// and `instance_id` are transport routing details that the caller does not
/// claim, and the agent session is a mutable carrier that may change inside the
/// same block.
fn rex_target_matches(value: &BindingValue, candidate: &crate::proto::RexCandidate) -> bool {
    value.transport.kind == crate::proto::TransportKind::Rex
        && value
            .transport
            .rex_endpoint
            .as_ref()
            .is_some_and(|endpoint| {
                endpoint.server == candidate.server
                    && endpoint.session_id == candidate.session_id
                    && endpoint.block_id == candidate.block_id
            })
}

impl BindingControlSet {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn holds_current_binding(&self, binding: &RuntimeBinding) -> bool {
        self.active_values()
            .any(|current| &current.binding == binding)
    }

    /// The only recovery selector. Only active current rows participate.
    pub(crate) fn lookup(
        &self,
        project: &ProjectScopeId,
        app: &AppServerId,
        facts: &crate::proto::IdentityFacts,
        cwd: Option<&str>,
    ) -> Result<Option<&IdentityBindingSlot>, String> {
        self.lookup_scoped(project, app, facts, cwd)
    }

    /// Select the caller's current actor after the daemon has validated an
    /// explicit human name replacement. A Rex block remains a placement
    /// candidate. When it points at another name, the session holder is the
    /// actor and the requested target stays a separate handoff decision.
    pub(crate) fn lookup_explicit_rex_rebind(
        &self,
        project: &ProjectScopeId,
        app: &AppServerId,
        facts: &crate::proto::IdentityFacts,
        cwd: Option<&str>,
    ) -> Result<Option<&IdentityBindingSlot>, String> {
        self.lookup_scoped(project, app, facts, cwd)
    }

    fn lookup_scoped(
        &self,
        project: &ProjectScopeId,
        app: &AppServerId,
        facts: &crate::proto::IdentityFacts,
        cwd: Option<&str>,
    ) -> Result<Option<&IdentityBindingSlot>, String> {
        let dsh = facts.dsh_session_id.as_deref().or_else(|| {
            facts
                .dsh
                .as_ref()
                .map(|candidate| candidate.session_id.as_str())
        });
        let scoped_currents = self
            .slots
            .iter()
            .filter(|slot| {
                &slot.identity_key.project_scope == project
                    && &slot.identity_key.app_scope_id == app
            })
            .filter_map(|slot| slot.active_binding().map(|value| (slot, value)))
            .filter(|(_, value)| {
                facts.agent_kind.as_ref().is_none_or(|kind| {
                    value
                        .host
                        .agent_kind
                        .as_ref()
                        .is_none_or(|saved| saved == kind)
                })
            })
            .collect::<Vec<_>>();
        let candidates = scoped_currents
            .iter()
            .copied()
            .filter(|(_, value)| {
                (dsh.is_none() || value.transport.kind == crate::proto::TransportKind::Dsh)
                    && (facts.rex.is_none()
                        || value.transport.kind == crate::proto::TransportKind::Rex)
                    && (facts.tmux.is_none()
                        || value.transport.kind == crate::proto::TransportKind::Tmux)
                    && facts.agent_kind.as_ref().is_none_or(|kind| {
                        value
                            .host
                            .agent_kind
                            .as_ref()
                            .is_none_or(|saved| saved == kind)
                    })
            })
            .collect::<Vec<_>>();
        fn unique(
            matches: Vec<&IdentityBindingSlot>,
        ) -> Result<Option<&IdentityBindingSlot>, String> {
            match matches.as_slice() {
                [] => Ok(None),
                [slot] => Ok(Some(*slot)),
                _ => Err(
                    "IDENTITY_MAP_AMBIGUOUS: multiple active mappings match the current caller"
                        .into(),
                ),
            }
        }
        let session_matches = match facts.session_id.as_deref() {
            Some(session) => scoped_currents
                .iter()
                .filter(|(_, value)| {
                    value
                        .binding
                        .session_id
                        .as_ref()
                        .is_some_and(|saved| saved.as_str() == session)
                })
                .map(|(slot, _)| *slot)
                .collect::<Vec<_>>(),
            None => Vec::new(),
        };
        // A Rex block is the placement anchor. A unique agent session may
        // move to a new block or cross adaptors. If that block still names a
        // prior owner, publishing the selected session later replaces only the
        // block's current owner; the prior identity and its history remain.
        if let Some(rex) = facts.rex.as_ref() {
            let targets = candidates
                .iter()
                .filter(|(_, value)| rex_target_matches(value, rex))
                .map(|(slot, _)| *slot)
                .collect::<Vec<_>>();
            return match session_matches.as_slice() {
                [] => unique(targets),
                [holder] => Ok(Some(*holder)),
                _ => match unique(targets)? {
                    Some(target)
                        if session_matches
                            .iter()
                            .any(|slot| slot.identity_key == target.identity_key) =>
                    {
                        Ok(Some(target))
                    }
                    _ => Err(
                        "IDENTITY_MAP_AMBIGUOUS: multiple active mappings match the current caller"
                            .into(),
                    ),
                },
            };
        }
        if facts.tmux.is_some() {
            if let Some(candidate) = facts.tmux.as_ref() {
                let found = unique(
                    candidates
                        .iter()
                        .filter(|(_, value)| {
                            value.transport.kind == crate::proto::TransportKind::Tmux
                                && value.binding.tmux_endpoint.as_ref().is_some_and(|saved| {
                                    saved.socket_path == candidate.endpoint.socket_path
                                        && saved.tmux_session_id
                                            == candidate.endpoint.tmux_session_id
                                })
                        })
                        .map(|(slot, _)| *slot)
                        .collect(),
                )?;
                if found.is_some() {
                    return Ok(found);
                }
            }
            if let Some(name) = facts.tmux_session_name.as_ref() {
                let found = unique(
                    candidates
                        .iter()
                        .filter(|(_, value)| value.physical_name.as_ref() == Some(name))
                        .map(|(slot, _)| *slot)
                        .collect(),
                )?;
                if found.is_some() {
                    return Ok(found);
                }
            }
        }
        if let Some(session) = dsh.or(facts.session_id.as_deref()) {
            let found = unique(
                candidates
                    .iter()
                    .filter(|(_, value)| {
                        value
                            .binding
                            .session_id
                            .as_ref()
                            .is_some_and(|saved| saved.as_str() == session)
                    })
                    .map(|(slot, _)| *slot)
                    .collect(),
            )?;
            if found.is_some() {
                return Ok(found);
            }
        }
        if let (Some(candidate), Some(cwd)) = (facts.tmux.as_ref(), cwd) {
            return unique(
                candidates
                    .iter()
                    .filter(|(_, value)| {
                        value.invocation_cwd == cwd
                            && value.binding.tmux_endpoint.as_ref().is_some_and(|saved| {
                                crate::client::adapters::tmux::same_owned_pane(
                                    saved,
                                    &candidate.endpoint,
                                )
                            })
                    })
                    .map(|(slot, _)| *slot)
                    .collect(),
            );
        }
        Ok(None)
    }

    pub(crate) fn key_for_worker(
        &self,
        project: &ProjectScopeId,
        app: &AppServerId,
        worker: &str,
    ) -> Result<Option<&IdentityBindingKey>, String> {
        let mut matches = self.slots.iter().filter(|slot| {
            &slot.identity_key.project_scope == project
                && &slot.identity_key.app_scope_id == app
                && slot
                    .active_binding()
                    .is_some_and(|value| value.binding.agent_id.as_str() == worker)
        });
        let first = matches.next();
        if matches.next().is_some() {
            return Err("IDENTITY_MAP_AMBIGUOUS: worker carries multiple active names".into());
        }
        Ok(first.map(|slot| &slot.identity_key))
    }

    pub(crate) fn current_for_worker(
        &self,
        project: &ProjectScopeId,
        app: &AppServerId,
        worker: &str,
    ) -> Result<Option<&IdentityBindingSlot>, String> {
        Ok(self
            .key_for_worker(project, app, worker)?
            .and_then(|key| self.slot(key)))
    }

    pub(crate) fn current_for_name(
        &self,
        project: &ProjectScopeId,
        app: &AppServerId,
        name: &str,
    ) -> Result<Option<&IdentityBindingSlot>, String> {
        let mut matches = self.slots.iter().filter(|slot| {
            &slot.identity_key.project_scope == project
                && &slot.identity_key.app_scope_id == app
                && slot.identity_key.name.eq_ignore_ascii_case(name)
                && slot.active_binding().is_some()
        });
        let first = matches.next();
        if matches.next().is_some() {
            return Err(format!(
                "IDENTITY_MAP_AMBIGUOUS: {name} carries multiple active names"
            ));
        }
        Ok(first)
    }

    pub(crate) fn project_currents_for_name<'a>(
        &'a self,
        project: &ProjectScopeId,
        name: &str,
    ) -> Vec<&'a IdentityBindingSlot> {
        self.slots
            .iter()
            .filter(|slot| {
                &slot.identity_key.project_scope == project
                    && slot.identity_key.name.eq_ignore_ascii_case(name)
                    && slot.active_binding().is_some()
            })
            .collect()
    }

    /// Select one current name across all app scopes in the same project. A
    /// unique match returns the full existing slot. Multiple active currents
    /// remain visible as typed ambiguity rather than being guessed.
    pub(crate) fn project_current_for_name(
        &self,
        project: &ProjectScopeId,
        name: &str,
    ) -> Result<Option<&IdentityBindingSlot>, String> {
        let matches = self.project_currents_for_name(project, name);
        let Some(first) = matches.first().copied() else {
            return Ok(None);
        };
        if let Some(conflict) = matches.get(1) {
            return Err(format!(
                "IDENTITY_MAP_AMBIGUOUS: project {} name {} has active current in app {} and app {}",
                project.as_str(),
                name,
                first.identity_key.app_scope_id.as_str(),
                conflict.identity_key.app_scope_id.as_str()
            ));
        }
        Ok(Some(first))
    }

    /// Locate the unique current caller in a project without widening the
    /// exact-app `lookup` contract. Each app scope is still matched by the
    /// existing typed lookup; more than one matching app stays ambiguous.
    pub(crate) fn project_current_for_facts(
        &self,
        project: &ProjectScopeId,
        facts: &crate::proto::IdentityFacts,
        cwd: Option<&str>,
    ) -> Result<Option<&IdentityBindingSlot>, String> {
        self.project_current_for_facts_excluding_app(project, None, facts, cwd)
    }

    /// Locate the caller's source current outside the selected target app.
    /// Exact-app lookup remains unchanged; this selector only compares the
    /// same typed host facts against the other app partitions in the project.
    pub(crate) fn project_current_for_facts_excluding_app(
        &self,
        project: &ProjectScopeId,
        excluded_app: Option<&AppServerId>,
        facts: &crate::proto::IdentityFacts,
        cwd: Option<&str>,
    ) -> Result<Option<&IdentityBindingSlot>, String> {
        let apps: std::collections::BTreeSet<&str> = self
            .slots
            .iter()
            .filter(|slot| {
                &slot.identity_key.project_scope == project
                    && excluded_app
                        .is_none_or(|excluded| &slot.identity_key.app_scope_id != excluded)
            })
            .map(|slot| slot.identity_key.app_scope_id.as_str())
            .collect();
        let mut matches = Vec::new();
        for app in apps {
            let app = AppServerId::new(app.to_owned()).map_err(|error| error.to_string())?;
            if let Some(slot) = self.lookup(project, &app, facts, cwd)? {
                matches.push(slot);
            }
        }
        match matches.as_slice() {
            [] => Ok(None),
            [slot] => Ok(Some(*slot)),
            _ => {
                let apps: Vec<_> = matches
                    .iter()
                    .map(|slot| slot.identity_key.app_scope_id.as_str())
                    .collect();
                Err(format!(
                    "IDENTITY_MAP_AMBIGUOUS: current caller matches active names in apps {}",
                    apps.join(", ")
                ))
            }
        }
    }

    pub(crate) fn cross_scope_source_ref(
        slot: &IdentityBindingSlot,
    ) -> Option<CrossScopeSourceRef> {
        let current = slot.active_binding()?;
        Some(CrossScopeSourceRef {
            key: slot.identity_key.clone(),
            binding_id: current.binding.binding_id.clone(),
            generation: slot.generation(),
        })
    }

    /// One active current is valid; more than one is a publication error. The
    /// error names the exact project/name and conflicting apps so a historical
    /// ambiguity remains diagnosable without making the whole map unloadable.
    pub(crate) fn validate_project_name_uniqueness(
        &self,
        project: &ProjectScopeId,
        name: &str,
    ) -> Result<(), String> {
        let matches: Vec<_> = self
            .slots
            .iter()
            .filter(|slot| {
                &slot.identity_key.project_scope == project
                    && slot.identity_key.name.eq_ignore_ascii_case(name)
                    && slot.active_binding().is_some()
            })
            .collect();
        if matches.len() > 1 {
            let apps: Vec<_> = matches
                .iter()
                .map(|slot| slot.identity_key.app_scope_id.as_str())
                .collect();
            return Err(format!(
                "IDENTITY_PROJECT_NAME_DUPLICATE: project {} name {} has active current in apps {}",
                project.as_str(),
                name,
                apps.join(", ")
            ));
        }
        Ok(())
    }

    /// Validate the exact source generation an explicit cross-scope takeover
    /// may release. `true` means release is still needed; `false` recognizes an
    /// already released retry without making the publication non-idempotent.
    pub(crate) fn validate_cross_scope_source(
        &self,
        source: &CrossScopeSourceRef,
    ) -> Result<bool, String> {
        let slot = self.slot(&source.key).ok_or_else(|| {
            format!(
                "CROSS_SCOPE_SOURCE_MISSING: {}",
                identity_key_label(&source.key)
            )
        })?;
        if let Some(current) = slot.active_binding() {
            if slot.generation() != source.generation
                || current.binding.binding_id != source.binding_id
            {
                return Err(format!(
                    "CROSS_SCOPE_SOURCE_CHANGED: {}",
                    identity_key_label(&source.key)
                ));
            }
            return Ok(true);
        }
        let previous = slot.reference_previous().ok_or_else(|| {
            format!(
                "CROSS_SCOPE_SOURCE_BINDING_MISSING: {}",
                identity_key_label(&source.key)
            )
        })?;
        if slot.generation() != source.generation.saturating_add(1)
            || previous.binding.binding_id != source.binding_id
        {
            return Err(format!(
                "CROSS_SCOPE_SOURCE_CHANGED: {}",
                identity_key_label(&source.key)
            ));
        }
        Ok(false)
    }

    pub(crate) fn validate_cross_scope_sources(
        &self,
        sources: &[CrossScopeSourceRef],
    ) -> Result<(), String> {
        for source in sources {
            self.validate_cross_scope_source(source)?;
        }
        Ok(())
    }

    /// Release the validated cross-scope source in the same in-memory candidate
    /// as its target publication. An already released retry is a no-op.
    pub(crate) fn release_cross_scope_source(
        &mut self,
        source: &CrossScopeSourceRef,
    ) -> Result<CommitOutcome, SlotError> {
        if !self
            .validate_cross_scope_source(source)
            .map_err(SlotError::Invariant)?
        {
            return Ok(CommitOutcome::Unchanged {
                generation: source.generation.saturating_add(1),
            });
        }
        self.unregister(&source.key, Some(source.generation))
    }

    pub(crate) fn release_cross_scope_sources(
        &mut self,
        sources: &[CrossScopeSourceRef],
    ) -> Result<usize, SlotError> {
        let mut released = 0;
        for source in sources {
            if matches!(
                self.release_cross_scope_source(source)?,
                CommitOutcome::Committed { .. }
            ) {
                released += 1;
            }
        }
        Ok(released)
    }

    pub(crate) fn ordinary_name(
        &self,
        project: &ProjectScopeId,
        app: &AppServerId,
        worker: &str,
    ) -> Result<String, String> {
        if let Some(name) = self.slots.iter().find_map(|slot| {
            let name = slot.identity_key.name.as_str();
            let ordinary = super::name_resolution::ORDINARY_NAME_POOL.contains(&name)
                || super::name_resolution::peer_registration_number(name).is_some();
            (&slot.identity_key.project_scope == project
                && &slot.identity_key.app_scope_id == app
                && ordinary
                && slot
                    .active_binding()
                    .is_some_and(|value| value.binding.agent_id.as_str() == worker))
            .then_some(name)
        }) {
            return Ok(name.into());
        }
        if let Some(name) = super::name_resolution::ORDINARY_NAME_POOL
            .into_iter()
            .find(|name| {
                !self.slots.iter().any(|slot| {
                    &slot.identity_key.project_scope == project
                        && slot.identity_key.name == *name
                        && slot.active_binding().is_some()
                })
            })
        {
            return Ok(name.into());
        }
        let mut number = 1u64;
        loop {
            let name = format!("peer-{number}");
            if !self.slots.iter().any(|slot| {
                &slot.identity_key.project_scope == project && slot.identity_key.name == name
            }) {
                return Ok(name);
            }
            number = number.checked_add(1).ok_or_else(|| {
                "PEER_NAME_EXHAUSTED: peer-<number> space is exhausted".to_string()
            })?;
        }
    }

    pub(crate) fn from_slots(slots: Vec<IdentityBindingSlot>) -> Self {
        Self {
            slots,
            effects: Vec::new(),
            pending_resets: Vec::new(),
        }
    }

    pub(crate) fn from_parts(slots: Vec<IdentityBindingSlot>, effects: Vec<PendingEffect>) -> Self {
        Self {
            slots,
            effects,
            pending_resets: Vec::new(),
        }
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
        value.validate_for_publication()?;
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
        let mut candidate = self.clone();
        let outcome = candidate.commit_candidate(key, value, expected_generation)?;
        *self = candidate;
        Ok(outcome)
    }

    fn commit_candidate(
        &mut self,
        key: &IdentityBindingKey,
        value: BindingValue,
        expected_generation: Option<u64>,
    ) -> Result<CommitOutcome, SlotError> {
        struct SupersededEffectOwner {
            key: IdentityBindingKey,
            target_worker_id: String,
            target_session_id: Option<String>,
        }

        fn superseded_effect_owner(slot: &IdentityBindingSlot) -> Option<SupersededEffectOwner> {
            let current = slot.active_binding()?;
            Some(SupersededEffectOwner {
                key: slot.identity_key.clone(),
                target_worker_id: current.binding.agent_id.as_str().to_owned(),
                target_session_id: current
                    .binding
                    .session_id
                    .as_ref()
                    .map(|session| session.as_str().to_owned()),
            })
        }

        let rex_target = value.rex_block_key();
        let mut replaced = Vec::new();
        let mut superseded_effects = Vec::new();
        if let Some(existing_index) = self.slots.iter().position(|slot| &slot.identity_key == key) {
            if let Some(existing) = self.slots[existing_index].active_binding() {
                if existing.rex_block_key() != rex_target {
                    superseded_effects
                        .push(superseded_effect_owner(&self.slots[existing_index]).unwrap());
                }
            }
        }
        if let Some(target) = rex_target {
            for (index, slot) in self.slots.iter().enumerate() {
                if slot.identity_key == *key {
                    continue;
                }
                if slot.active_binding().and_then(BindingValue::rex_block_key) == Some(target) {
                    if let Some(owner) = superseded_effect_owner(slot) {
                        superseded_effects.push(owner);
                    }
                    replaced.push((index, slot.generation));
                }
            }
        }

        let index = self.slots.iter().position(|slot| &slot.identity_key == key);
        let index = match index {
            Some(index) => index,
            None => {
                self.slots.push(IdentityBindingSlot::new(key.clone()));
                self.slots.len() - 1
            }
        };
        let outcome = self.slots[index].commit(key, value, expected_generation)?;
        for (index, generation) in replaced {
            self.slots[index].unregister(Some(generation))?;
        }
        // A block replacement invalidates only the effects that targeted the
        // replaced current. An unrelated name handoff for the same stable name
        // can target a different worker/session and must survive.
        if !superseded_effects.is_empty() {
            self.effects.retain(|effect| {
                if !matches!(
                    effect.kind,
                    PendingEffectKind::Register | PendingEffectKind::Name
                ) {
                    return true;
                }
                !superseded_effects.iter().any(|owner| {
                    owner.key
                        == IdentityBindingKey::new(
                            effect.project_scope.clone(),
                            effect.app_scope_id.clone(),
                            &effect.name,
                        )
                        && owner.target_worker_id == effect.target_worker_id
                        && owner.target_session_id.as_deref() == Some(&effect.target_session_id)
                })
            });
        }
        Ok(outcome)
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

    /// Remove every slot and pending effect in the given project scope except
    /// the active DSH gateway binding. Project reset must keep the current DSH
    /// session, communication API endpoint, and committed binding so the live
    /// gateway can recover the receive loop after reset and daemon restart.
    /// Non-DSH slots and unfinished handoff intents for the scope are retired.
    pub(crate) fn remove_scope_except_dsh(&mut self, project_scope: &str) -> usize {
        let before = self.slots.len();
        self.slots.retain(|slot| {
            let in_scope = slot.identity_key.project_scope.as_str() == project_scope;
            if !in_scope {
                return true;
            }
            let is_dsh = slot
                .active_binding()
                .is_some_and(|value| value.transport.kind == crate::proto::TransportKind::Dsh);
            // Keep the active DSH slot as the retained host-owned gateway
            // binding. A retired or non-DSH slot in this scope is disposable.
            is_dsh
        });
        // Unfinished handoff intents are disposable project state. Reset must
        // not revive a name move that never completed, not even for the
        // retained DSH slot.
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
