#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PeerLifecycleTarget {
    pub worker_id: String,
    pub project_scope: ProjectScopeId,
    pub app_scope_id: AppServerId,
    pub binding_id: BindingId,
    pub endpoint_generation: u64,
    pub transport: PeerLifecycleTransport,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PeerLifecycleTransport {
    pub kind: TransportKind,
    pub endpoint: Option<String>,
    pub namespace: Option<String>,
    pub session_id: Option<String>,
    pub thread_id: Option<String>,
    pub tmux_endpoint: Option<TmuxEndpoint>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PeerLifecycleAction {
    Create,
    Read,
    Update,
    Close,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PeerLifecycleOutcome {
    Ok,
    Refused,
    Missing,
    Unknown,
    Closed,
    CleanupOpen,
    Complete,
    Partial,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PeerLifecycleSource {
    CommittedProjection,
    ExactHostObservation,
    LifecycleOperation,
    LegacyCloseReceipt,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PeerLifecyclePhase {
    IntentPersisted,
    HostDispatched,
    HostDispatchClaimed,
    ReadbackPending,
    Complete,
    Refused,
    Partial,
    Unknown,
    Cancelled,
    #[serde(rename = "cleanup-open")]
    CleanupOpen,
}

impl PeerLifecyclePhase {
    /// True while a mutating lifecycle operation still excludes new
    /// responsibility for its exact target. Terminal phases release it.
    pub fn is_in_flight(&self) -> bool {
        matches!(
            self,
            Self::IntentPersisted
                | Self::HostDispatchClaimed
                | Self::HostDispatched
                | Self::ReadbackPending
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PeerLifecycleStage {
    NotAttempted,
    Pending,
    Verified,
    Missing,
    NotApplicable,
    Refused,
    Failed,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PeerSettingsState {
    NotAttempted,
    Acknowledged,
    Refused,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum PeerEffectiveCwd {
    Unproven,
    Verified {
        cwd: String,
        thread_id: Option<String>,
        turn_id: Option<String>,
    },
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PeerLifecycleSettings {
    pub state: PeerSettingsState,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PeerLifecycleUpdate {
    pub previous_cwd: String,
    pub intended_cwd: String,
    pub settings: PeerLifecycleSettings,
    pub effective_cwd: PeerEffectiveCwd,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub challenge: Option<PeerLifecycleChallenge>,
}

/// One operation-owned execution challenge.
///
/// The marker file name and content hash are persisted before the challenge
/// turn is dispatched; the returned turn id is persisted before any
/// completion can be accepted. The marker content itself never appears in the
/// prompt, so only a peer that actually read the file can produce the exact
/// result envelope.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PeerLifecycleChallenge {
    pub marker_file: String,
    pub marker_sha256: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<String>,
    pub dispatched_ms: i64,
    pub state: PeerLifecycleStage,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cleanup: Option<PeerLifecycleStage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cleanup_error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PeerLifecycleStageReadback {
    pub state: PeerLifecycleStage,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub challenge: Option<PeerLifecycleChallenge>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PeerLifecycleClose {
    pub close_outcome: String,
    pub runtime_archive: PeerLifecycleStageReadback,
    pub worker_retirement: PeerLifecycleStageReadback,
    pub binding_retirement: PeerLifecycleStageReadback,
    pub route_retirement: PeerLifecycleStageReadback,
    pub lease_retirement: PeerLifecycleStageReadback,
    pub subscription_retirement: PeerLifecycleStageReadback,
}

/// Durable receipt for a Create operation. It captures the frozen request
/// intent and, once the native host answers, the exact returned thread id and
/// the observed lifecycle stages. Nothing here is inferred from cwd or a
/// notification; a lost response leaves `thread_id` absent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PeerLifecycleCreate {
    pub peer_id: String,
    pub cwd: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<String>,
    #[serde(default)]
    pub stages: Vec<String>,
    pub readiness: PeerLifecycleStageReadback,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PeerLifecycleResult {
    pub action: PeerLifecycleAction,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub operation_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target: Option<PeerLifecycleTarget>,
    pub outcome: PeerLifecycleOutcome,
    pub source: PeerLifecycleSource,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub phase: Option<PeerLifecyclePhase>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub projection: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub update: Option<PeerLifecycleUpdate>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub close: Option<PeerLifecycleClose>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub create: Option<PeerLifecycleCreate>,
    pub requires: ContextOperationRequires,
}
