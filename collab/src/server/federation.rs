//! Federation message core (rex-federation-message-20261108).
//!
//! This module owns the typed, network-free message core shared by the origin
//! adapter and the target mailbox adapter:
//!
//! - the exact origin-reference dedup index and its tombstones,
//! - the origin outbound recovery record,
//! - the accepted/reused/expired/conflict decision for one `Put`,
//! - participant validation for typed replies, and
//! - receipt projection from the owner's current facts.
//!
//! It never performs network IO and never holds a mailbox lock. The daemon
//! adapters in `part_08`/`part_10` read a current-state snapshot, call the
//! pure decisions here, and commit a single current-state batch. No second
//! mailbox, no identity/name machine keys, and no durable body copy are added.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::proto::{
    FederatedChannelRef, FederationActor, FederationDestination, FederationEndpoint,
    FederationMessageReceipt, FederationNotificationStatus, FederationPrincipal, FederationPut,
    FederationResponse, LocalNameAddress, MachineName, MessageRef, OriginMessageRef,
    QualifiedNameAddress, ReplyPolicy,
};

pub use crate::federation::auth::AuthenticatedMachine;

/// Typed failure with a stable public code. The daemon maps this into the
/// private federation response; it is never fabricated from prose.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FederationError {
    pub code: &'static str,
    pub detail: String,
}

impl FederationError {
    pub fn new(code: &'static str, detail: impl Into<String>) -> Self {
        Self {
            code,
            detail: detail.into(),
        }
    }
}

impl std::fmt::Display for FederationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}: {}", self.code, self.detail)
    }
}

/// One origin outbound operation. Keyed by the compact `OriginMessageRef`
/// key. It is a recovery record, not a second consumable mailbox: the body is
/// dropped once the target's acceptance is authoritative.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederatedOutboundRecord {
    pub origin: OriginMessageRef,
    pub sender: FederationActor,
    pub target: FederationEndpoint,
    pub mtype: String,
    pub subject: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    pub delivery: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub in_reply_to: Option<MessageRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub original_origin: Option<OriginMessageRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply_policy: Option<ReplyPolicy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_session: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel: Option<FederatedChannelRef>,
    /// pending -> accepted | rejected | unknown.
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accepted: Option<MessageRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub created_ms: i64,
    pub updated_ms: i64,
}

impl FederatedOutboundRecord {
    pub const STATUS_PENDING: &'static str = "pending";
    pub const STATUS_ACCEPTED: &'static str = "accepted";
    pub const STATUS_REJECTED: &'static str = "rejected";
    pub const STATUS_UNKNOWN: &'static str = "unknown";

    /// The exact `Put` to (re)send. `None` once the body was released after a
    /// confirmed acceptance; a released body is never resent.
    pub fn to_put(&self) -> Option<FederationPut> {
        let body = self.body.clone()?;
        Some(FederationPut {
            origin: self.origin.clone(),
            sender: self.sender.clone(),
            target: self.target.destination(),
            mtype: self.mtype.clone(),
            subject: self.subject.clone(),
            body,
            delivery: self.delivery.clone(),
            target_session: self.target_session.clone(),
            in_reply_to: self.in_reply_to.clone(),
            original_origin: self.original_origin.clone(),
            reply_policy: self.reply_policy,
            channel: self.channel.clone(),
        })
    }

    /// Whether a candidate request is byte-for-byte the same accepted intent.
    pub fn matches_request(&self, other: &FederatedOutboundRecord) -> bool {
        self.matches_request_fields(other) && self.body == other.body
    }

    pub(crate) fn matches_request_fields(&self, other: &FederatedOutboundRecord) -> bool {
        self.sender == other.sender
            && self.target == other.target
            && self.mtype == other.mtype
            && self.subject == other.subject
            && self.delivery == other.delivery
            && self.in_reply_to == other.in_reply_to
            && self.original_origin == other.original_origin
            && self.reply_policy == other.reply_policy
            && self.target_session == other.target_session
            && self.channel == other.channel
    }

    /// Result updates may release the body after acceptance, but they must keep
    /// every immutable request field and may never reintroduce a body.
    fn matches_result_request(&self, other: &FederatedOutboundRecord) -> bool {
        self.matches_request_fields(other)
            && (self.body == other.body || (self.body.is_some() && other.body.is_none()))
    }
}

/// Typed target-side metadata for one accepted mailbox message. Keyed by the
/// local `Message.id`; it is the authoritative federation projection for that
/// message, not a second message copy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederatedMessageMeta {
    pub origin: OriginMessageRef,
    pub sender: FederationActor,
    pub target: FederationDestination,
    pub recipient: FederationActor,
    pub reference: MessageRef,
    pub mtype: String,
    pub subject: String,
    pub delivery: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub in_reply_to: Option<MessageRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub original_origin: Option<OriginMessageRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply_policy: Option<ReplyPolicy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel: Option<FederatedChannelRef>,
    pub accepted_ms: i64,
}

impl FederatedMessageMeta {
    /// Compare the immutable request content against a retried `Put`. Sender,
    /// target, mode, body, target session, correlation and reply policy must
    /// all match. `existing_target_session` is read from the same target-state
    /// snapshot as `existing_body` so an accepted intent is never split.
    pub fn matches_put(
        &self,
        request: &FederationPut,
        existing_body: Option<&str>,
        existing_target_session: Option<&str>,
    ) -> bool {
        self.sender == request.sender
            && self.target == request.target
            && self.mtype == request.mtype
            && self.subject == request.subject
            && existing_body == Some(request.body.as_str())
            && self.delivery == request.delivery
            && existing_target_session == request.target_session.as_deref()
            && self.in_reply_to == request.in_reply_to
            && self.original_origin == request.original_origin
            && self.reply_policy == request.reply_policy
            && self.channel == request.channel
    }
}

/// Tombstone kept after a message body retention. It prevents a delayed retry
/// from resurrecting an already-consumed message and never keeps the body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederatedExpiredOrigin {
    pub origin: OriginMessageRef,
    pub reference: MessageRef,
    pub expired_ms: i64,
}

/// The single typed federation index owned by the daemon current state. It is
/// stored as one `State` field so the reducer, retention cleanup and the
/// network-free core share one definition.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct FederationStore {
    /// Compact origin key -> outbound recovery record.
    pub outbound: HashMap<String, FederatedOutboundRecord>,
    /// Local message id -> accepted federated metadata.
    pub messages: HashMap<String, FederatedMessageMeta>,
    /// Compact origin key -> local message id. The atomic accepted index.
    pub origin_index: HashMap<String, String>,
    /// Compact `MessageRef` key -> local message id, for reply/receipt lookup.
    pub reference_index: HashMap<String, String>,
    /// Compact origin key -> tombstone after body retention.
    pub expired_origins: HashMap<String, FederatedExpiredOrigin>,
}

/// Stable compact key for one origin reference. The full typed reference is
/// the dedup identity; the key is only its exact, reversible encoding, not a
/// hash and never reconstructed from logs or message text.
pub fn origin_key(reference: &OriginMessageRef) -> String {
    serde_json::to_string(reference).expect("origin reference is serializable")
}

/// Stable compact key for one message reference.
pub fn message_ref_key(reference: &MessageRef) -> String {
    serde_json::to_string(reference).expect("message reference is serializable")
}

/// What one incoming `Put` should do, decided from the current owner facts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AcceptDisposition {
    /// A brand new message must be committed.
    New,
    /// A byte-identical retry: return the original reference, do not notify.
    Reuse(MessageRef),
    /// The body retention already closed this origin: report expiry.
    Expired(MessageRef),
}

impl FederationStore {
    pub fn outbound_record(&self, origin: &OriginMessageRef) -> Option<&FederatedOutboundRecord> {
        self.outbound.get(&origin_key(origin))
    }

    pub fn message_for_origin(&self, origin: &OriginMessageRef) -> Option<&FederatedMessageMeta> {
        let id = self.origin_index.get(&origin_key(origin))?;
        self.messages.get(id)
    }

    pub fn message_id_for_origin(&self, origin: &OriginMessageRef) -> Option<&str> {
        self.origin_index
            .get(&origin_key(origin))
            .map(String::as_str)
    }

    pub fn message_for_reference(&self, reference: &MessageRef) -> Option<&FederatedMessageMeta> {
        let id = self.reference_index.get(&message_ref_key(reference))?;
        self.messages.get(id)
    }

    pub fn message_id_for_reference(&self, reference: &MessageRef) -> Option<&str> {
        self.reference_index
            .get(&message_ref_key(reference))
            .map(String::as_str)
    }

    /// Insert an outbound recovery record once. Repeating the same intent is a
    /// no-op; a different intent for the same origin is a conflict.
    pub fn record_outbound(
        &mut self,
        record: FederatedOutboundRecord,
    ) -> Result<(), FederationError> {
        let key = origin_key(&record.origin);
        if let Some(existing) = self.outbound.get(&key) {
            if !existing.matches_request(&record) {
                return Err(FederationError::new(
                    "FEDERATION_OPERATION_CONFLICT",
                    format!("outbound operation {key} already recorded a different request"),
                ));
            }
            return Ok(());
        }
        self.outbound.insert(key, record);
        Ok(())
    }

    /// Advance one outbound record to its authoritative result. The immutable
    /// request fields must stay identical; an accepted reference never changes.
    pub fn update_outbound(
        &mut self,
        record: FederatedOutboundRecord,
    ) -> Result<(), FederationError> {
        let key = origin_key(&record.origin);
        let Some(existing) = self.outbound.get(&key) else {
            return Err(FederationError::new(
                "FEDERATION_OUTBOUND_UNKNOWN",
                format!("outbound operation {key} is not recorded"),
            ));
        };
        if !existing.matches_result_request(&record) {
            return Err(FederationError::new(
                "FEDERATION_OPERATION_CONFLICT",
                format!("outbound operation {key} changed its immutable intent"),
            ));
        }
        if let Some(accepted) = &existing.accepted {
            if Some(accepted) != record.accepted.as_ref() {
                return Err(FederationError::new(
                    "FEDERATION_OPERATION_CONFLICT",
                    format!("outbound operation {key} changed its accepted reference"),
                ));
            }
        }
        self.outbound.insert(key, record);
        Ok(())
    }

    /// Atomically install one accepted message and its two indexes. Repeating
    /// the same acceptance is idempotent; any other reuse of the origin key is
    /// a conflict so a late retry can never fork the mailbox.
    pub fn accept(
        &mut self,
        message_id: &str,
        meta: FederatedMessageMeta,
    ) -> Result<(), FederationError> {
        let key = origin_key(&meta.origin);
        if let Some(existing_id) = self.origin_index.get(&key) {
            if existing_id == message_id {
                return Ok(());
            }
            return Err(FederationError::new(
                "FEDERATION_OPERATION_CONFLICT",
                format!("origin {key} is already accepted as {existing_id}"),
            ));
        }
        if self.expired_origins.contains_key(&key) {
            return Err(FederationError::new(
                "MESSAGE_EXPIRED",
                format!("origin {key} retention already closed"),
            ));
        }
        if self.messages.contains_key(message_id) {
            return Err(FederationError::new(
                "FEDERATION_OPERATION_CONFLICT",
                format!("message {message_id} already carries federation metadata"),
            ));
        }
        self.reference_index
            .insert(message_ref_key(&meta.reference), message_id.to_owned());
        self.origin_index.insert(key, message_id.to_owned());
        self.messages.insert(message_id.to_owned(), meta);
        Ok(())
    }

    /// Move one message to an origin tombstone after its body retention. The
    /// body is owned by the mailbox and is dropped by `State::drop_message`.
    pub fn expire(&mut self, message_id: &str, expired_ms: i64) {
        let Some(meta) = self.messages.remove(message_id) else {
            return;
        };
        let key = origin_key(&meta.origin);
        self.origin_index.remove(&key);
        self.reference_index
            .remove(&message_ref_key(&meta.reference));
        self.expired_origins.insert(
            key,
            FederatedExpiredOrigin {
                origin: meta.origin,
                reference: meta.reference,
                expired_ms,
            },
        );
    }
}

/// One local target resolved by the target daemon's own directory owner. The
/// worker id is the current mailbox carrier; the principal is the canonical
/// address handoff-safe identity. Neither is a remote identity cache.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedRecipient {
    pub worker_id: String,
    pub principal: FederationPrincipal,
    pub address: LocalNameAddress,
}

/// Verify the authenticated source fact and the request's claimed machine.
pub fn verify_source(
    connection: &AuthenticatedMachine,
    local_machine: &MachineName,
    request: &FederationPut,
) -> Result<(), FederationError> {
    if &connection.target != local_machine {
        return Err(FederationError::new(
            "MACHINE_IDENTITY_MISMATCH",
            format!(
                "connection targets {} but this daemon is {local_machine}",
                connection.target
            ),
        ));
    }
    if request.origin.machine != connection.source {
        return Err(FederationError::new(
            "FEDERATION_SOURCE_MISMATCH",
            format!(
                "origin machine {} does not match authenticated source {}",
                request.origin.machine, connection.source
            ),
        ));
    }
    if request.sender.machine() != connection.source {
        return Err(FederationError::new(
            "FEDERATION_SOURCE_MISMATCH",
            format!(
                "sender machine {} does not match authenticated source {}",
                request.sender.machine(),
                connection.source
            ),
        ));
    }
    if request.sender.project_scope() != &request.origin.project_scope
        || request.sender.app_scope_id() != &request.origin.app_scope_id
    {
        return Err(FederationError::new(
            "FEDERATION_SCOPE_MISMATCH",
            "sender project/app scope does not match the origin reference",
        ));
    }
    let channel_matches = match (&request.sender, &request.target, &request.channel) {
        (FederationActor::Peer(_), FederationDestination::Named(_), None) => true,
        (
            FederationActor::ExternalChannel(source),
            FederationDestination::Named(_),
            Some(channel),
        ) if source == channel => true,
        (FederationActor::Peer(_), FederationDestination::ChannelOwner(target), Some(channel)) => {
            target == channel
        }
        _ => false,
    };
    if !channel_matches {
        return Err(FederationError::new(
            "FEDERATION_CHANNEL_MISMATCH",
            "typed actor, target and channel must describe the same channel owner",
        ));
    }
    if let FederationDestination::ChannelOwner(channel) = &request.target {
        if &channel.owner_machine != local_machine {
            return Err(FederationError::new(
                "MACHINE_IDENTITY_MISMATCH",
                "channel destination is owned by another machine",
            ));
        }
        let original = request.original_origin.as_ref().ok_or_else(|| {
            FederationError::new(
                "FEDERATION_REPLY_ORIGIN_REQUIRED",
                "channel replies require the original channel origin",
            )
        })?;
        if channel.owner_machine != original.machine
            || channel.project_scope != original.project_scope
            || channel.app_scope_id != original.app_scope_id
        {
            return Err(FederationError::new(
                "FEDERATION_SCOPE_MISMATCH",
                "channel destination scope does not match the original channel origin",
            ));
        }
    }
    if let FederationActor::ExternalChannel(channel) = &request.sender {
        if channel.project_scope != request.origin.project_scope
            || channel.app_scope_id != request.origin.app_scope_id
            || channel.owner_machine != request.origin.machine
        {
            return Err(FederationError::new(
                "FEDERATION_CHANNEL_MISMATCH",
                "external channel source does not match the origin reference",
            ));
        }
    }
    Ok(())
}

/// Decide the outcome of one `Put`. `existing_body` is the body the mailbox
/// currently owns for the already-accepted message, if any. The full typed
/// origin reference is the dedup key; the decision never consults logs or
/// message prose.
pub fn plan_accept(
    store: &FederationStore,
    connection: &AuthenticatedMachine,
    local_machine: &MachineName,
    request: &FederationPut,
    existing_body: Option<&str>,
    existing_target_session: Option<&str>,
) -> Result<AcceptDisposition, FederationError> {
    verify_source(connection, local_machine, request)?;
    let key = origin_key(&request.origin);
    if let Some(meta) = store.message_for_origin(&request.origin) {
        if meta.matches_put(request, existing_body, existing_target_session) {
            return Ok(AcceptDisposition::Reuse(meta.reference.clone()));
        }
        return Err(FederationError::new(
            "FEDERATION_OPERATION_CONFLICT",
            format!("origin {key} is already accepted with different content"),
        ));
    }
    if let Some(expired) = store.expired_origins.get(&key) {
        return Ok(AcceptDisposition::Expired(expired.reference.clone()));
    }
    Ok(AcceptDisposition::New)
}

/// Compare one accepted intent without accepting, notifying, or committing.
/// `requester` and `compare.sender` must match the original sender actor. The
/// caller reads body and target-session from the target mailbox once and passes
/// them in; this helper never performs IO and never mutates the store.
pub fn compare_accepted_intent(
    store: &FederationStore,
    origin: &OriginMessageRef,
    requester: &FederationActor,
    compare: &FederationPut,
    existing_body: Option<&str>,
    existing_target_session: Option<&str>,
) -> Result<MessageRef, FederationError> {
    let meta = store
        .message_for_origin(origin)
        .ok_or_else(|| FederationError::new("NOT_SEEN", "no accepted message for this origin"))?;
    if &meta.sender != requester {
        return Err(FederationError::new(
            "FEDERATION_PARTICIPANT_MISMATCH",
            "lookup requester is not the original sender actor",
        ));
    }
    if compare.origin != *origin {
        return Err(FederationError::new(
            "FEDERATION_OPERATION_CONFLICT",
            "lookup compare origin does not match the queried origin",
        ));
    }
    if &compare.sender != requester {
        return Err(FederationError::new(
            "FEDERATION_PARTICIPANT_MISMATCH",
            "lookup compare sender is not the lookup requester",
        ));
    }
    if meta.matches_put(compare, existing_body, existing_target_session) {
        Ok(meta.reference.clone())
    } else {
        Err(FederationError::new(
            "FEDERATION_OPERATION_CONFLICT",
            format!(
                "origin {} changed its immutable accepted intent",
                origin_key(origin)
            ),
        ))
    }
}

/// Participant check for a typed reply. The requester must itself be one of
/// the two original participants of the referenced message; a third party can
/// never forge a linked reply.
pub fn reply_participant_ok(meta: &FederatedMessageMeta, requester: &FederationActor) -> bool {
    requester == &meta.sender || requester == &meta.recipient
}

/// Outcome of one origin `Put`, whether accepted, rejected, or network-unknown.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FederationSendOutcome {
    Accepted {
        origin: OriginMessageRef,
        reference: MessageRef,
    },
    Rejected {
        origin: OriginMessageRef,
        code: String,
        detail: String,
    },
    Unknown {
        origin: OriginMessageRef,
        detail: String,
    },
}

impl FederationSendOutcome {
    pub fn origin(&self) -> &OriginMessageRef {
        match self {
            Self::Accepted { origin, .. }
            | Self::Rejected { origin, .. }
            | Self::Unknown { origin, .. } => origin,
        }
    }
}

pub(crate) fn same_endpoint(left: &FederationEndpoint, right: &FederationEndpoint) -> bool {
    match (left, right) {
        (FederationEndpoint::Named(left), FederationEndpoint::Named(right)) => {
            left.machine.eq_ignore_ascii_case(&right.machine)
                && left
                    .local
                    .project
                    .eq_ignore_ascii_case(&right.local.project)
                && left.local.name.eq_ignore_ascii_case(&right.local.name)
        }
        _ => left == right,
    }
}

fn receipt_matches_record(
    record: &FederatedOutboundRecord,
    receipt: &FederationMessageReceipt,
) -> bool {
    receipt.origin == record.origin
        && receipt.sender == record.sender
        && same_endpoint(&receipt.recipient, &record.target)
        && receipt.reference.machine == record.target.machine()
}

/// Map one response onto a typed outcome against the recorded intent. No new
/// operation id is generated on any path.
pub fn outcome_from_response(
    record: &FederatedOutboundRecord,
    response: &FederationResponse,
) -> FederationSendOutcome {
    let origin = record.origin.clone();
    match response {
        FederationResponse::Message { receipt } => {
            if receipt_matches_record(record, receipt) {
                FederationSendOutcome::Accepted {
                    origin,
                    reference: receipt.reference.clone(),
                }
            } else {
                FederationSendOutcome::Rejected {
                    origin,
                    code: "FEDERATION_RESPONSE_MISMATCH".to_owned(),
                    detail: "target receipt does not match the recorded origin".to_owned(),
                }
            }
        }
        FederationResponse::Expired { .. } => FederationSendOutcome::Rejected {
            origin,
            code: "MESSAGE_EXPIRED".to_owned(),
            detail: "target retention already closed this origin".to_owned(),
        },
        FederationResponse::Error { code, detail } => FederationSendOutcome::Rejected {
            origin,
            code: code.clone(),
            detail: detail.clone(),
        },
        FederationResponse::NotSeen => FederationSendOutcome::Unknown {
            origin,
            detail: "target did not see the origin reference".to_owned(),
        },
        FederationResponse::Hello { .. } | FederationResponse::Directory { .. } => {
            FederationSendOutcome::Rejected {
                origin,
                code: "FEDERATION_UNEXPECTED_RESPONSE".to_owned(),
                detail: "unexpected response for a Put".to_owned(),
            }
        }
    }
}

/// Map the target mailbox's current notification owners onto the receipt fact.
pub fn notification_facts(
    consumed: bool,
    accepted: bool,
    known_not_delivered: bool,
    attempted: bool,
) -> ConsumptionFacts {
    let notification = if accepted {
        FederationNotificationStatus::Accepted
    } else if known_not_delivered {
        FederationNotificationStatus::KnownNotDelivered
    } else if attempted {
        FederationNotificationStatus::Unknown
    } else {
        FederationNotificationStatus::NotAttempted
    };
    ConsumptionFacts {
        consumed,
        notification,
    }
}

/// Record facts the mailbox currently owns for one consumed message. Injected
/// so receipt projection reads the authoritative owners, not a second state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConsumptionFacts {
    pub consumed: bool,
    pub notification: FederationNotificationStatus,
}

/// Project one receipt from the accepted metadata, the consuming owner's
/// facts, and any accepted reply op that names this reference. Only a
/// participant of the referenced message may read the receipt.
pub fn project_receipt(
    store: &FederationStore,
    reference: &MessageRef,
    requester: &FederationActor,
    facts: &ConsumptionFacts,
) -> Result<FederationMessageReceipt, FederationError> {
    let meta = store.message_for_reference(reference).ok_or_else(|| {
        FederationError::new("NOT_SEEN", "no accepted message for this reference")
    })?;
    if !reply_participant_ok(meta, requester) {
        return Err(FederationError::new(
            "FEDERATION_PARTICIPANT_MISMATCH",
            "requester is not a participant of this message",
        ));
    }
    let reply_reference = store
        .outbound
        .values()
        .filter(|record| record.in_reply_to.as_ref() == Some(reference))
        .filter_map(|record| {
            record
                .accepted
                .clone()
                .map(|accepted| (record.created_ms, accepted))
        })
        .min_by_key(|(created_ms, accepted)| (*created_ms, accepted.msg_id.as_str().to_owned()))
        .map(|(_, accepted)| accepted);
    Ok(FederationMessageReceipt {
        origin: meta.origin.clone(),
        reference: meta.reference.clone(),
        sender: meta.sender.clone(),
        recipient: match &meta.recipient {
            FederationActor::Peer(principal) => {
                FederationEndpoint::Named(principal.address.clone())
            }
            FederationActor::ExternalChannel(channel) => {
                FederationEndpoint::ChannelOwner(channel.clone())
            }
        },
        notification: facts.notification,
        consumed: facts.consumed,
        channel_delivery: None,
        channel_delivery_error: None,
        reply_reference,
    })
}

/// Canonical display for one qualified address. Machine names contain no dot.
/// Project and name segments encode `%` and `.` once, matching the address
/// owner's canonical form.
pub fn display_sender(actor: &FederationActor) -> String {
    match actor {
        FederationActor::Peer(principal) => display_qualified(&principal.address),
        FederationActor::ExternalChannel(channel) => channel.reply_address(),
    }
}

pub(crate) fn display_qualified(address: &QualifiedNameAddress) -> String {
    format!(
        "{}.{}.{}",
        address.machine,
        encode_segment(&address.local.project),
        encode_segment(&address.local.name)
    )
}

fn encode_segment(segment: &str) -> String {
    let mut out = String::with_capacity(segment.len());
    for ch in segment.chars() {
        match ch {
            '%' => out.push_str("%25"),
            '.' => out.push_str("%2E"),
            _ => out.push(ch),
        }
    }
    out
}

/// Input to the origin prepare owner. The actor principal is already
/// authenticated by the daemon; this core never re-derives identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AddressSendRequest {
    pub sender: FederationActor,
    pub target: FederationEndpoint,
    pub mtype: String,
    pub subject: String,
    pub body: String,
    pub delivery: String,
    pub operation_id: Option<crate::identity::OperationId>,
    pub in_reply_to: Option<MessageRef>,
    pub original_origin: Option<OriginMessageRef>,
    pub reply_policy: Option<ReplyPolicy>,
    pub target_session: Option<String>,
    pub channel: Option<FederatedChannelRef>,
    pub now_ms: i64,
}

/// Build the origin recovery record. A local-machine target is an explicit
/// non-federation result so the daemon never routes it over the network.
pub fn prepare_outbound(
    request: AddressSendRequest,
    local_machine: &MachineName,
) -> Result<FederatedOutboundRecord, FederationError> {
    if request.target.machine() == local_machine {
        return Err(FederationError::new(
            "FEDERATION_LOCAL_TARGET",
            "target machine is this daemon; use the local send owner",
        ));
    }
    if request.sender.machine() != local_machine {
        return Err(FederationError::new(
            "FEDERATION_SOURCE_MISMATCH",
            "sender actor is not the local machine",
        ));
    }
    match (&request.sender, &request.target, &request.channel) {
        (FederationActor::Peer(_), FederationEndpoint::Named(_), None) => {}
        (FederationActor::ExternalChannel(source), FederationEndpoint::Named(_), Some(channel))
            if source == channel => {}
        (FederationActor::Peer(_), FederationEndpoint::ChannelOwner(target), Some(channel))
            if target == channel => {}
        _ => {
            return Err(FederationError::new(
                "FEDERATION_CHANNEL_MISMATCH",
                "typed actor, target and channel must describe the same channel owner",
            ));
        }
    }
    if let FederationActor::Peer(principal) = &request.sender {
        if principal.stable_name.trim().is_empty() {
            return Err(FederationError::new(
                "FEDERATION_SOURCE_MISMATCH",
                "sender principal has no stable name",
            ));
        }
    }
    if let FederationEndpoint::Named(address) = &request.target {
        if address.local.project.trim().is_empty() || address.local.name.trim().is_empty() {
            return Err(FederationError::new(
                "ADDRESS_INVALID",
                "target project and name are required",
            ));
        }
    }
    if request.subject.trim().is_empty() {
        return Err(FederationError::new(
            "MESSAGE_SUBJECT_REQUIRED",
            "federation send requires a subject",
        ));
    }
    let operation_id = match request.operation_id {
        Some(operation_id) => operation_id,
        None => next_operation_id(request.now_ms),
    };
    Ok(FederatedOutboundRecord {
        origin: OriginMessageRef {
            machine: request.sender.machine().to_owned(),
            project_scope: request.sender.project_scope().clone(),
            app_scope_id: request.sender.app_scope_id().clone(),
            operation_id,
        },
        sender: request.sender,
        target: request.target,
        mtype: request.mtype,
        subject: request.subject,
        body: Some(request.body),
        delivery: request.delivery,
        in_reply_to: request.in_reply_to,
        original_origin: request.original_origin,
        reply_policy: request.reply_policy,
        target_session: request.target_session,
        channel: request.channel,
        status: FederatedOutboundRecord::STATUS_PENDING.to_owned(),
        accepted: None,
        error: None,
        created_ms: request.now_ms,
        updated_ms: request.now_ms,
    })
}

/// Mark an outbound record with its typed outcome. Accepted records release
/// their body; unknown records keep the body for same-reference recovery.
pub fn record_with_outcome(
    record: &FederatedOutboundRecord,
    outcome: &FederationSendOutcome,
    now_ms: i64,
) -> FederatedOutboundRecord {
    let mut updated = record.clone();
    updated.updated_ms = now_ms;
    match outcome {
        FederationSendOutcome::Accepted { reference, .. } => {
            updated.status = FederatedOutboundRecord::STATUS_ACCEPTED.to_owned();
            updated.accepted = Some(reference.clone());
            updated.body = None;
            updated.error = None;
        }
        FederationSendOutcome::Rejected { code, detail, .. } => {
            updated.status = FederatedOutboundRecord::STATUS_REJECTED.to_owned();
            updated.error = Some(format!("{code}: {detail}"));
        }
        FederationSendOutcome::Unknown { detail, .. } => {
            updated.status = FederatedOutboundRecord::STATUS_UNKNOWN.to_owned();
            updated.error = Some(detail.clone());
        }
    }
    updated
}

fn next_operation_id(now_ms: i64) -> crate::identity::OperationId {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(1);
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    let value = format!("fed-op-{now_ms}-{seq}");
    crate::identity::OperationId::new(value).expect("generated operation id is valid")
}

#[cfg(test)]
mod federation_message_tests {
    use super::*;
    use crate::identity::{AppServerId, MessageId, OperationId};
    use crate::scope::ProjectScopeId;
    use serde_json::json;

    fn scope() -> ProjectScopeId {
        ProjectScopeId::new("/projects/alpha").unwrap()
    }

    fn app() -> AppServerId {
        AppServerId::new("appserver-alpha").unwrap()
    }

    fn principal(machine: &str, project: &str, name: &str) -> FederationActor {
        FederationActor::Peer(FederationPrincipal {
            address: QualifiedNameAddress {
                machine: machine.to_owned(),
                local: LocalNameAddress {
                    project: project.to_owned(),
                    name: name.to_owned(),
                },
            },
            project_scope: scope(),
            app_scope_id: app(),
            stable_name: name.to_owned(),
        })
    }

    fn channel_ref(machine: &str) -> FederatedChannelRef {
        FederatedChannelRef {
            owner_machine: machine.to_owned(),
            project_scope: scope(),
            app_scope_id: app(),
            inbound_id: "qqbot-inbound".to_owned(),
        }
    }

    fn origin(op: &str) -> OriginMessageRef {
        OriginMessageRef {
            machine: "studio".to_owned(),
            project_scope: scope(),
            app_scope_id: app(),
            operation_id: OperationId::new(op).unwrap(),
        }
    }

    fn put(op: &str, body: &str) -> FederationPut {
        FederationPut {
            origin: origin(op),
            sender: principal("studio", "alpha", "Alice"),
            target: FederationDestination::Named(LocalNameAddress {
                project: "alpha".to_owned(),
                name: "Bob".to_owned(),
            }),
            mtype: "message".to_owned(),
            subject: "联邦消息".to_owned(),
            body: body.to_owned(),
            delivery: "immediate".to_owned(),
            target_session: None,
            in_reply_to: None,
            original_origin: None,
            reply_policy: None,
            channel: None,
        }
    }

    fn connection() -> AuthenticatedMachine {
        AuthenticatedMachine {
            source: "studio".to_owned(),
            target: "macbookair".to_owned(),
        }
    }

    fn meta(op: &str, body: &str) -> FederatedMessageMeta {
        FederatedMessageMeta {
            origin: origin(op),
            sender: principal("studio", "alpha", "Alice"),
            target: FederationDestination::Named(LocalNameAddress {
                project: "alpha".to_owned(),
                name: "Bob".to_owned(),
            }),
            recipient: principal("macbookair", "alpha", "Bob"),
            reference: MessageRef {
                machine: "macbookair".to_owned(),
                project_scope: scope(),
                app_scope_id: app(),
                msg_id: MessageId::new("m-1").unwrap(),
            },
            mtype: "message".to_owned(),
            subject: "联邦消息".to_owned(),
            delivery: "immediate".to_owned(),
            in_reply_to: None,
            original_origin: None,
            reply_policy: None,
            channel: None,
            accepted_ms: 10,
        }
    }

    #[test]
    fn identical_retry_reuses_original_reference() {
        let mut store = FederationStore::default();
        let record = meta("op-1", "第一次\nsecond");
        let reference = record.reference.clone();
        store.accept("m-1", record).unwrap();
        let disposition = plan_accept(
            &store,
            &connection(),
            &"macbookair".to_owned(),
            &put("op-1", "第一次\nsecond"),
            Some("第一次\nsecond"),
            None,
        )
        .unwrap();
        assert_eq!(disposition, AcceptDisposition::Reuse(reference));
    }

    #[test]
    fn same_origin_with_different_body_is_conflict() {
        let mut store = FederationStore::default();
        store.accept("m-1", meta("op-1", "original")).unwrap();
        let error = plan_accept(
            &store,
            &connection(),
            &"macbookair".to_owned(),
            &put("op-1", "tampered"),
            Some("original"),
            None,
        )
        .unwrap_err();
        assert_eq!(error.code, "FEDERATION_OPERATION_CONFLICT");
        assert_eq!(store.messages.len(), 1);
    }

    #[test]
    fn expired_origin_reports_expired_not_notseen() {
        let mut store = FederationStore::default();
        let record = meta("op-1", "body");
        let reference = record.reference.clone();
        store.accept("m-1", record).unwrap();
        store.expire("m-1", 99);
        let disposition = plan_accept(
            &store,
            &connection(),
            &"macbookair".to_owned(),
            &put("op-1", "body"),
            None,
            None,
        )
        .unwrap();
        assert_eq!(disposition, AcceptDisposition::Expired(reference));
        assert!(store.messages.is_empty());
        assert!(store
            .expired_origins
            .contains_key(&origin_key(&origin("op-1"))));
    }

    #[test]
    fn wrong_source_machine_is_rejected_before_any_state_change() {
        let store = FederationStore::default();
        let mut request = put("op-1", "body");
        request.sender = principal("intruder", "alpha", "Alice");
        let error = plan_accept(
            &store,
            &connection(),
            &"macbookair".to_owned(),
            &request,
            None,
            None,
        )
        .unwrap_err();
        assert_eq!(error.code, "FEDERATION_SOURCE_MISMATCH");
        assert!(store.messages.is_empty());
        assert!(store.origin_index.is_empty());
    }

    #[test]
    fn third_party_reply_is_rejected() {
        let mut store = FederationStore::default();
        let record = meta("op-1", "body");
        let reference = record.reference.clone();
        store.accept("m-1", record).unwrap();
        let meta = store.message_for_reference(&reference).unwrap().clone();
        assert!(reply_participant_ok(
            &meta,
            &principal("studio", "alpha", "Alice")
        ));
        assert!(reply_participant_ok(
            &meta,
            &principal("macbookair", "alpha", "Bob")
        ));
        assert!(!reply_participant_ok(
            &meta,
            &principal("studio", "alpha", "Mallory")
        ));
    }

    #[test]
    fn receipt_separates_accepted_from_consumed() {
        let mut store = FederationStore::default();
        let record = meta("op-1", "body");
        let reference = record.reference.clone();
        store.accept("m-1", record).unwrap();
        let requester = principal("studio", "alpha", "Alice");
        let facts = ConsumptionFacts {
            consumed: false,
            notification: FederationNotificationStatus::Accepted,
        };
        let receipt = project_receipt(&store, &reference, &requester, &facts).unwrap();
        assert!(matches!(
            receipt.notification,
            FederationNotificationStatus::Accepted
        ));
        assert!(!receipt.consumed);
        let consumed = ConsumptionFacts {
            consumed: true,
            notification: FederationNotificationStatus::Accepted,
        };
        let receipt = project_receipt(&store, &reference, &requester, &consumed).unwrap();
        assert!(receipt.consumed);
    }

    #[test]
    fn participant_only_receipt_query() {
        let mut store = FederationStore::default();
        let reference = meta("op-1", "body").reference.clone();
        store.accept("m-1", meta("op-1", "body")).unwrap();
        let stranger = principal("studio", "alpha", "Mallory");
        let error = project_receipt(
            &store,
            &reference,
            &stranger,
            &ConsumptionFacts {
                consumed: false,
                notification: FederationNotificationStatus::NotAttempted,
            },
        )
        .unwrap_err();
        assert_eq!(error.code, "FEDERATION_PARTICIPANT_MISMATCH");
    }

    #[test]
    fn outbound_conflict_and_body_release() {
        let request = AddressSendRequest {
            sender: principal("studio", "alpha", "Alice"),
            target: FederationEndpoint::Named(QualifiedNameAddress {
                machine: "macbookair".to_owned(),
                local: LocalNameAddress {
                    project: "alpha".to_owned(),
                    name: "Bob".to_owned(),
                },
            }),
            mtype: "message".to_owned(),
            subject: "hello".to_owned(),
            body: "body".to_owned(),
            delivery: "immediate".to_owned(),
            operation_id: Some(OperationId::new("op-1").unwrap()),
            in_reply_to: None,
            original_origin: None,
            reply_policy: None,
            target_session: None,
            channel: None,
            now_ms: 5,
        };
        let record = prepare_outbound(request.clone(), &"studio".to_owned()).unwrap();
        assert_eq!(record.body.as_deref(), Some("body"));
        let mut store = FederationStore::default();
        store.record_outbound(record.clone()).unwrap();
        let mut tampered = request;
        tampered.body = "different".to_owned();
        let tampered_record = prepare_outbound(tampered, &"studio".to_owned()).unwrap();
        let error = store.record_outbound(tampered_record).unwrap_err();
        assert_eq!(error.code, "FEDERATION_OPERATION_CONFLICT");

        let accepted = FederationSendOutcome::Accepted {
            origin: record.origin.clone(),
            reference: MessageRef {
                machine: "macbookair".to_owned(),
                project_scope: scope(),
                app_scope_id: app(),
                msg_id: MessageId::new("m-1").unwrap(),
            },
        };
        let released = record_with_outcome(&record, &accepted, 6);
        assert_eq!(released.status, FederatedOutboundRecord::STATUS_ACCEPTED);
        assert!(released.body.is_none());
        assert!(released.to_put().is_none());

        let mut store = FederationStore::default();
        store.record_outbound(record).unwrap();
        store.update_outbound(released.clone()).unwrap();
        assert_eq!(store.outbound_record(&origin("op-1")), Some(&released));
    }

    #[test]
    fn accepted_retry_with_released_body_is_compared_by_lookup_not_recorded() {
        let request = AddressSendRequest {
            sender: principal("studio", "alpha", "Alice"),
            target: FederationEndpoint::Named(QualifiedNameAddress {
                machine: "macbookair".to_owned(),
                local: LocalNameAddress {
                    project: "alpha".to_owned(),
                    name: "Bob".to_owned(),
                },
            }),
            mtype: "message".to_owned(),
            subject: "hello".to_owned(),
            body: "body".to_owned(),
            delivery: "immediate".to_owned(),
            operation_id: Some(OperationId::new("op-retry").unwrap()),
            in_reply_to: None,
            original_origin: None,
            reply_policy: None,
            target_session: None,
            channel: None,
            now_ms: 5,
        };
        let original = prepare_outbound(request.clone(), &"studio".to_owned()).unwrap();
        let retry = prepare_outbound(request, &"studio".to_owned()).unwrap();
        let accepted = FederationSendOutcome::Accepted {
            origin: original.origin.clone(),
            reference: MessageRef {
                machine: "macbookair".to_owned(),
                project_scope: scope(),
                app_scope_id: app(),
                msg_id: MessageId::new("m-retry").unwrap(),
            },
        };
        let released = record_with_outcome(&original, &accepted, 6);
        let mut store = FederationStore::default();
        store.record_outbound(original).unwrap();
        store.update_outbound(released).unwrap();

        let error = store.record_outbound(retry).unwrap_err();
        assert_eq!(error.code, "FEDERATION_OPERATION_CONFLICT");

        let stored = store.outbound_record(&origin("op-retry")).unwrap();
        assert_eq!(stored.status, FederatedOutboundRecord::STATUS_ACCEPTED);
        assert!(stored.body.is_none());
        assert!(stored.to_put().is_none());
        assert_eq!(
            stored
                .accepted
                .as_ref()
                .map(|reference| reference.msg_id.as_str()),
            Some("m-retry")
        );
    }

    #[test]
    fn unknown_outcome_keeps_body_for_recovery() {
        let request = AddressSendRequest {
            sender: principal("studio", "alpha", "Alice"),
            target: FederationEndpoint::Named(QualifiedNameAddress {
                machine: "macbookair".to_owned(),
                local: LocalNameAddress {
                    project: "alpha".to_owned(),
                    name: "Bob".to_owned(),
                },
            }),
            mtype: "message".to_owned(),
            subject: "hello".to_owned(),
            body: "body".to_owned(),
            delivery: "immediate".to_owned(),
            operation_id: Some(OperationId::new("op-9").unwrap()),
            in_reply_to: None,
            original_origin: None,
            reply_policy: None,
            target_session: None,
            channel: None,
            now_ms: 5,
        };
        let record = prepare_outbound(request, &"studio".to_owned()).unwrap();
        let outcome = FederationSendOutcome::Unknown {
            origin: record.origin.clone(),
            detail: "response was not observed".into(),
        };
        let updated = record_with_outcome(&record, &outcome, 7);
        assert_eq!(updated.status, FederatedOutboundRecord::STATUS_UNKNOWN);
        assert!(updated.to_put().is_some());
        assert_eq!(updated.origin, record.origin);
    }

    #[test]
    fn local_machine_target_is_not_federated() {
        let request = AddressSendRequest {
            sender: principal("studio", "alpha", "Alice"),
            target: FederationEndpoint::Named(QualifiedNameAddress {
                machine: "studio".to_owned(),
                local: LocalNameAddress {
                    project: "alpha".to_owned(),
                    name: "Bob".to_owned(),
                },
            }),
            mtype: "message".to_owned(),
            subject: "hi".to_owned(),
            body: "body".to_owned(),
            delivery: "immediate".to_owned(),
            operation_id: None,
            in_reply_to: None,
            original_origin: None,
            reply_policy: None,
            target_session: None,
            channel: None,
            now_ms: 5,
        };
        let error = prepare_outbound(request, &"studio".to_owned()).unwrap_err();
        assert_eq!(error.code, "FEDERATION_LOCAL_TARGET");
    }

    #[test]
    fn reply_updates_accepted_reference_in_receipt() {
        let mut store = FederationStore::default();
        let reference = meta("op-1", "body").reference.clone();
        store.accept("m-1", meta("op-1", "body")).unwrap();
        let reply_origin = OriginMessageRef {
            machine: "macbookair".to_owned(),
            project_scope: scope(),
            app_scope_id: app(),
            operation_id: OperationId::new("reply-op").unwrap(),
        };
        let reply_ref = MessageRef {
            machine: "studio".to_owned(),
            project_scope: scope(),
            app_scope_id: app(),
            msg_id: MessageId::new("reply-msg").unwrap(),
        };
        store
            .record_outbound(FederatedOutboundRecord {
                origin: reply_origin,
                sender: principal("macbookair", "alpha", "Bob"),
                target: FederationEndpoint::Named(QualifiedNameAddress {
                    machine: "studio".to_owned(),
                    local: LocalNameAddress {
                        project: "alpha".to_owned(),
                        name: "Alice".to_owned(),
                    },
                }),
                mtype: "reply".to_owned(),
                subject: "re".to_owned(),
                body: None,
                delivery: "immediate".to_owned(),
                in_reply_to: Some(reference.clone()),
                original_origin: Some(origin("op-1")),
                reply_policy: None,
                target_session: None,
                channel: None,
                status: FederatedOutboundRecord::STATUS_ACCEPTED.to_owned(),
                accepted: Some(reply_ref.clone()),
                error: None,
                created_ms: 20,
                updated_ms: 20,
            })
            .unwrap();
        let receipt = project_receipt(
            &store,
            &reference,
            &principal("studio", "alpha", "Alice"),
            &ConsumptionFacts {
                consumed: false,
                notification: FederationNotificationStatus::NotAttempted,
            },
        )
        .unwrap();
        assert_eq!(receipt.reply_reference, Some(reply_ref));
    }

    #[test]
    fn store_roundtrips_through_json() {
        let mut store = FederationStore::default();
        store.accept("m-1", meta("op-1", "body")).unwrap();
        let encoded = serde_json::to_value(&store).unwrap();
        let decoded: FederationStore = serde_json::from_value(encoded.clone()).unwrap();
        assert_eq!(decoded, store);
        assert_eq!(encoded["origin_index"].as_object().unwrap().len(), 1);
        assert!(matches!(
            serde_json::from_value::<FederationStore>(json!({})),
            Ok(value) if value == FederationStore::default()
        ));
    }

    #[test]
    fn same_origin_with_different_target_session_is_conflict() {
        let mut store = FederationStore::default();
        store.accept("m-1", meta("op-1", "body")).unwrap();
        let mut request = put("op-1", "body");
        request.target_session = Some("session-new".to_owned());
        let error = plan_accept(
            &store,
            &connection(),
            &"macbookair".to_owned(),
            &request,
            Some("body"),
            Some("session-old"),
        )
        .unwrap_err();
        assert_eq!(error.code, "FEDERATION_OPERATION_CONFLICT");
        assert_eq!(store.messages.len(), 1);
    }

    #[test]
    fn lookup_compare_exact_intent_returns_original_reference() {
        let mut store = FederationStore::default();
        let reference = meta("op-1", "body");
        let expected = reference.reference.clone();
        store.accept("m-1", reference).unwrap();
        let mut compare = put("op-1", "body");
        compare.target_session = Some("session-1".to_owned());
        let requester = principal("studio", "alpha", "Alice");
        let matched = compare_accepted_intent(
            &store,
            &origin("op-1"),
            &requester,
            &compare,
            Some("body"),
            Some("session-1"),
        )
        .unwrap();
        assert_eq!(matched, expected);
        let error = compare_accepted_intent(
            &store,
            &origin("op-1"),
            &requester,
            &compare,
            Some("body"),
            Some("session-2"),
        )
        .unwrap_err();
        assert_eq!(error.code, "FEDERATION_OPERATION_CONFLICT");
    }

    #[test]
    fn verify_source_accepts_typed_external_channel_actor_with_channel_fact() {
        let mut request = put("op-channel", "body");
        request.sender = FederationActor::ExternalChannel(channel_ref("studio"));
        request.channel = Some(channel_ref("studio"));
        verify_source(&connection(), &"macbookair".to_owned(), &request).unwrap();
    }

    #[test]
    fn verify_source_rejects_external_channel_without_channel_fact() {
        let mut request = put("op-channel", "body");
        request.sender = FederationActor::ExternalChannel(channel_ref("studio"));
        let error = verify_source(&connection(), &"macbookair".to_owned(), &request).unwrap_err();
        assert_eq!(error.code, "FEDERATION_CHANNEL_MISMATCH");
    }

    #[test]
    fn prepare_outbound_accepts_external_channel_with_matching_channel_fact() {
        let channel = channel_ref("studio");
        let request = AddressSendRequest {
            sender: FederationActor::ExternalChannel(channel.clone()),
            target: FederationEndpoint::Named(QualifiedNameAddress {
                machine: "macbookair".to_owned(),
                local: LocalNameAddress {
                    project: "alpha".to_owned(),
                    name: "Bob".to_owned(),
                },
            }),
            mtype: "message".to_owned(),
            subject: "channel inbound".to_owned(),
            body: "body".to_owned(),
            delivery: "immediate".to_owned(),
            operation_id: Some(OperationId::new("op-channel-out").unwrap()),
            in_reply_to: None,
            original_origin: None,
            reply_policy: None,
            target_session: None,
            channel: Some(channel),
            now_ms: 5,
        };
        prepare_outbound(request, &"studio".to_owned()).unwrap();
    }

    #[test]
    fn prepare_outbound_rejects_external_channel_without_channel_fact() {
        let request = AddressSendRequest {
            sender: FederationActor::ExternalChannel(channel_ref("studio")),
            target: FederationEndpoint::Named(QualifiedNameAddress {
                machine: "macbookair".to_owned(),
                local: LocalNameAddress {
                    project: "alpha".to_owned(),
                    name: "Bob".to_owned(),
                },
            }),
            mtype: "message".to_owned(),
            subject: "channel inbound".to_owned(),
            body: "body".to_owned(),
            delivery: "immediate".to_owned(),
            operation_id: Some(OperationId::new("op-channel-no-fact").unwrap()),
            in_reply_to: None,
            original_origin: None,
            reply_policy: None,
            target_session: None,
            channel: None,
            now_ms: 5,
        };
        let error = prepare_outbound(request, &"studio".to_owned()).unwrap_err();
        assert_eq!(error.code, "FEDERATION_CHANNEL_MISMATCH");
    }

    #[test]
    fn verify_source_rejects_channel_field_mismatch() {
        let mut request = put("op-channel", "body");
        request.channel = Some(channel_ref("studio"));
        let error = verify_source(&connection(), &"macbookair".to_owned(), &request).unwrap_err();
        assert_eq!(error.code, "FEDERATION_CHANNEL_MISMATCH");
    }

    #[test]
    fn target_receipt_must_match_recorded_origin() {
        let request = AddressSendRequest {
            sender: principal("studio", "alpha", "Alice"),
            target: FederationEndpoint::Named(QualifiedNameAddress {
                machine: "macbookair".to_owned(),
                local: LocalNameAddress {
                    project: "alpha".to_owned(),
                    name: "Bob".to_owned(),
                },
            }),
            mtype: "message".to_owned(),
            subject: "hello".to_owned(),
            body: "body".to_owned(),
            delivery: "immediate".to_owned(),
            operation_id: Some(OperationId::new("op-1").unwrap()),
            in_reply_to: None,
            original_origin: None,
            reply_policy: None,
            target_session: None,
            channel: None,
            now_ms: 5,
        };
        let record = prepare_outbound(request, &"studio".to_owned()).unwrap();
        record.to_put().expect("pending outbound keeps its body");
        let mut receipt = FederationMessageReceipt {
            origin: origin("op-other"),
            reference: MessageRef {
                machine: "macbookair".to_owned(),
                project_scope: scope(),
                app_scope_id: app(),
                msg_id: MessageId::new("m-other").unwrap(),
            },
            sender: principal("studio", "alpha", "Alice"),
            recipient: FederationEndpoint::Named(QualifiedNameAddress {
                machine: "macbookair".to_owned(),
                local: LocalNameAddress {
                    project: "alpha".to_owned(),
                    name: "Bob".to_owned(),
                },
            }),
            notification: FederationNotificationStatus::NotAttempted,
            consumed: false,
            channel_delivery: None,
            channel_delivery_error: None,
            reply_reference: None,
        };
        receipt.origin = record.origin.clone();
        receipt.recipient = FederationEndpoint::Named(QualifiedNameAddress {
            machine: "macbookair".into(),
            local: LocalNameAddress {
                project: "alpha".into(),
                name: "another-peer".into(),
            },
        });
        assert!(
            matches!(outcome_from_response(&record, &FederationResponse::Message { receipt }),
            FederationSendOutcome::Rejected { code, .. } if code == "FEDERATION_RESPONSE_MISMATCH")
        );
    }
}
