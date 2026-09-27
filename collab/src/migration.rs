//! Read-only inspection and classification of legacy governance JSONL.
//!
//! This module deliberately has no daemon or filesystem-state owner.  It reads
//! bytes supplied by the caller, keeps each source line verbatim, and returns a
//! deterministic report.  A later migration adapter can use the report to
//! archive, adapt, reset, or stop for operator input without replaying a
//! source journal while it is being inspected.

#[allow(unused_imports)]
pub use crate::server::global_state::{
    MigrationIdentityRebindReceipt, MigrationReceiptSet, MigrationRuntimeRebindReceipt,
    MigrationWriterReceipt,
};
use crate::server::state::Event;
use serde::de::{DeserializeSeed, Deserializer, MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::error::Error;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// The disposition of a source record in a future migration transaction.
#[derive(Debug, Clone, Copy, Eq, Hash, Ord, PartialEq, PartialOrd, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum MappingClass {
    /// The record has enough immutable identity and lifecycle evidence to be
    /// referenced directly.  Direct does not authorize active replay.
    Direct,
    /// The record is useful historical input but needs a schema or semantic
    /// conversion before it can be referenced by the new epoch.
    Adapt,
    /// The record describes state that must be re-bound in a fresh epoch.
    Reset,
    /// The record cannot be safely interpreted without preserving the exact
    /// failure and obtaining an operator decision.  Scope mismatches use this
    /// class because the source may belong to another project and cannot be
    /// safely rebound by this migration.
    Unknown,
}

/// Controller action for one inspected source.  This is intentionally
/// separate from `MappingClass`: the classifier describes what was observed,
/// while the controller decides whether the source may be replayed, adapted,
/// preserved only as evidence, or rebuilt in a new epoch.
#[derive(Debug, Clone, Copy, Eq, Hash, Ord, PartialEq, PartialOrd, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceDisposition {
    DirectReplay,
    AdaptReconcile,
    ArchiveOnly,
    RebuildRequired,
}

impl SourceDisposition {
    pub fn from_mapping_class(classification: MappingClass) -> Self {
        match classification {
            MappingClass::Direct => Self::DirectReplay,
            MappingClass::Adapt => Self::AdaptReconcile,
            MappingClass::Reset => Self::RebuildRequired,
            MappingClass::Unknown => Self::ArchiveOnly,
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::DirectReplay => "direct_replay",
            Self::AdaptReconcile => "adapt_reconcile",
            Self::ArchiveOnly => "archive_only",
            Self::RebuildRequired => "rebuild_required",
        }
    }
}

#[path = "migration_tail.rs"]
mod migration_tail;

pub use migration_tail::*;

#[cfg(test)]
#[path = "migration_tests.rs"]
mod tests;

/// Project admission is a project-level gate and must not be inferred from a
/// single source record or confused with the migration transaction phase.
#[derive(Debug, Clone, Copy, Eq, Hash, Ord, PartialEq, PartialOrd, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectAdmission {
    Verified,
    ResetRequired,
    NeedsOperator,
    Aborted,
}

impl ProjectAdmission {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Verified => "verified",
            Self::ResetRequired => "reset_required",
            Self::NeedsOperator => "needs_operator",
            Self::Aborted => "aborted",
        }
    }
}

/// Durable lifecycle states for the migration transaction boundary.
///
/// These values describe the transaction owned by this module.  They are
/// deliberately separate from the legacy `MigrationRecord::phase` string so
/// that an old `verified` row cannot be mistaken for a verified target epoch.
#[derive(Debug, Clone, Copy, Eq, Hash, Ord, PartialEq, PartialOrd, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MigrationPhase {
    Planned,
    SnapshotCaptured,
    ReplayVerified,
    Rebound,
    Applied,
    Verified,
    ResetRequired,
    NeedsOperator,
    Aborted,
}

impl MigrationPhase {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Planned => "planned",
            Self::SnapshotCaptured => "snapshot_captured",
            Self::ReplayVerified => "replay_verified",
            Self::Rebound => "rebound",
            Self::Applied => "applied",
            Self::Verified => "verified",
            Self::ResetRequired => "reset_required",
            Self::NeedsOperator => "needs_operator",
            Self::Aborted => "aborted",
        }
    }
}

/// Typed failures at the migration transaction boundary.
///
/// The variants intentionally retain the first failed field/boundary.  A
/// caller can expose the error to an operator without converting an unknown
/// or reset-required source into a successful apply.
#[derive(Debug, Clone, Eq, PartialEq)]
pub enum MigrationContractError {
    Missing(&'static str),
    Invalid {
        field: &'static str,
        reason: String,
    },
    DigestMismatch {
        field: &'static str,
        expected: String,
        observed: String,
    },
    EpochMismatch {
        field: &'static str,
        expected: u64,
        observed: u64,
    },
    RevisionMismatch {
        field: &'static str,
        expected: u64,
        observed: u64,
    },
    AdmissionBlocked {
        admission: ProjectAdmission,
    },
    PrefixBlocked {
        line: Option<usize>,
        exact_error: String,
    },
    ReceiptRejected {
        reason: String,
    },
}

impl MigrationContractError {
    fn missing(field: &'static str) -> Self {
        Self::Missing(field)
    }

    fn invalid(field: &'static str, reason: impl Into<String>) -> Self {
        Self::Invalid {
            field,
            reason: reason.into(),
        }
    }

    fn digest_mismatch(
        field: &'static str,
        expected: impl Into<String>,
        observed: impl Into<String>,
    ) -> Self {
        Self::DigestMismatch {
            field,
            expected: expected.into(),
            observed: observed.into(),
        }
    }
}

impl fmt::Display for MigrationContractError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Missing(field) => write!(f, "MIGRATION_CONTRACT_MISSING:{field}"),
            Self::Invalid { field, reason } => {
                write!(f, "MIGRATION_CONTRACT_INVALID:{field}:{reason}")
            }
            Self::DigestMismatch {
                field,
                expected,
                observed,
            } => write!(
                f,
                "MIGRATION_DIGEST_MISMATCH:{field}:expected={expected}:observed={observed}"
            ),
            Self::EpochMismatch {
                field,
                expected,
                observed,
            } => write!(
                f,
                "MIGRATION_EPOCH_MISMATCH:{field}:expected={expected}:observed={observed}"
            ),
            Self::RevisionMismatch {
                field,
                expected,
                observed,
            } => write!(
                f,
                "MIGRATION_REVISION_MISMATCH:{field}:expected={expected}:observed={observed}"
            ),
            Self::AdmissionBlocked { admission } => {
                write!(f, "MIGRATION_APPLY_REJECTED:{}", admission.as_str())
            }
            Self::PrefixBlocked { line, exact_error } => {
                write!(f, "MIGRATION_PREFIX_BLOCKED:line={line:?}:{exact_error}")
            }
            Self::ReceiptRejected { reason } => {
                write!(f, "MIGRATION_RECEIPT_REJECTED:{reason}")
            }
        }
    }
}

impl Error for MigrationContractError {}

/// An immutable source/archive snapshot receipt.
///
/// This value does not write an archive.  It binds the source digest and the
/// archive digest to the same migration, while allowing the archive to contain
/// the source stream plus the immutable migration evidence collected around
/// it.  The owner that creates the archive must separately prove that both
/// referenced byte streams are immutable.  `verify_source_digest` is the
/// read-side check used before any target operation.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImmutableSnapshot {
    pub migration_id: String,
    pub source_project_id: String,
    pub source_epoch: Option<u64>,
    pub source_digest: String,
    pub archive_ref: String,
    pub archive_digest: String,
    pub captured_revision: u64,
}

impl ImmutableSnapshot {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        migration_id: impl Into<String>,
        source_project_id: impl Into<String>,
        source_digest: impl Into<String>,
        archive_ref: impl Into<String>,
        archive_digest: impl Into<String>,
        captured_revision: u64,
    ) -> Result<Self, MigrationContractError> {
        let snapshot = Self {
            migration_id: migration_id.into(),
            source_project_id: source_project_id.into(),
            source_epoch: None,
            source_digest: source_digest.into(),
            archive_ref: archive_ref.into(),
            archive_digest: archive_digest.into(),
            captured_revision,
        };
        snapshot.validate()?;
        Ok(snapshot)
    }

    pub fn with_source_epoch(mut self, source_epoch: Option<u64>) -> Self {
        self.source_epoch = source_epoch;
        self
    }

    pub fn validate(&self) -> Result<(), MigrationContractError> {
        validate_contract_identifier("migration_id", &self.migration_id)?;
        validate_contract_identifier("source_project_id", &self.source_project_id)?;
        validate_contract_identifier("source_digest", &self.source_digest)?;
        validate_contract_reference("archive_ref", &self.archive_ref)?;
        validate_contract_identifier("archive_digest", &self.archive_digest)?;
        if let Some(source_epoch) = self.source_epoch {
            if source_epoch == 0 {
                return Err(MigrationContractError::invalid(
                    "source_epoch",
                    "must be non-zero when present",
                ));
            }
        }
        Ok(())
    }

    pub fn verify_source_digest(&self, observed: &str) -> Result<(), MigrationContractError> {
        self.validate()?;
        if self.source_digest == observed {
            Ok(())
        } else {
            Err(MigrationContractError::digest_mismatch(
                "source_digest",
                self.source_digest.clone(),
                observed,
            ))
        }
    }
}

/// Compatibility name used by migration manifests and archive owners.
pub type SnapshotReceipt = ImmutableSnapshot;

/// Target epoch allocation and the source revision it is fenced against.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TargetEpoch {
    pub source_epoch: Option<u64>,
    pub target_epoch: u64,
    pub expected_active_revision: u64,
}

impl TargetEpoch {
    pub fn new(
        source_epoch: Option<u64>,
        target_epoch: u64,
        expected_active_revision: u64,
    ) -> Result<Self, MigrationContractError> {
        let epoch = Self {
            source_epoch,
            target_epoch,
            expected_active_revision,
        };
        epoch.validate()?;
        Ok(epoch)
    }

    pub fn validate(&self) -> Result<(), MigrationContractError> {
        if let Some(source_epoch) = self.source_epoch {
            if source_epoch == 0 {
                return Err(MigrationContractError::invalid(
                    "source_epoch",
                    "must be non-zero when present",
                ));
            }
        }
        if self.target_epoch == 0 {
            return Err(MigrationContractError::invalid(
                "target_epoch",
                "must be non-zero",
            ));
        }
        if self.source_epoch == Some(self.target_epoch) {
            return Err(MigrationContractError::invalid(
                "source_epoch",
                "must differ from target_epoch",
            ));
        }
        Ok(())
    }

    pub fn validate_against(
        &self,
        active_epoch: u64,
        active_revision: u64,
    ) -> Result<(), MigrationContractError> {
        self.validate()?;
        if let Some(source_epoch) = self.source_epoch {
            if source_epoch != active_epoch {
                return Err(MigrationContractError::EpochMismatch {
                    field: "source_epoch",
                    expected: source_epoch,
                    observed: active_epoch,
                });
            }
        }
        if self.target_epoch <= active_epoch {
            return Err(MigrationContractError::invalid(
                "target_epoch",
                format!(
                    "must be greater than active_epoch {active_epoch}, observed {}",
                    self.target_epoch
                ),
            ));
        }
        if self.expected_active_revision != active_revision {
            return Err(MigrationContractError::RevisionMismatch {
                field: "expected_active_revision",
                expected: self.expected_active_revision,
                observed: active_revision,
            });
        }
        Ok(())
    }

    /// Validate a transaction after its target epoch is active.  Allocation
    /// uses [`Self::validate_against`], which requires a strictly newer target;
    /// apply admission observes the committed target and therefore requires
    /// equality with the active epoch.
    pub fn validate_committed_against(
        &self,
        active_epoch: u64,
        active_revision: u64,
    ) -> Result<(), MigrationContractError> {
        self.validate()?;
        if self.target_epoch != active_epoch {
            return Err(MigrationContractError::EpochMismatch {
                field: "target_epoch",
                expected: active_epoch,
                observed: self.target_epoch,
            });
        }
        if self.expected_active_revision != active_revision {
            return Err(MigrationContractError::RevisionMismatch {
                field: "expected_active_revision",
                expected: self.expected_active_revision,
                observed: active_revision,
            });
        }
        Ok(())
    }
}

/// Compatibility name used by callers that call the epoch allocation an
/// epoch descriptor.
pub type EpochDescriptor = TargetEpoch;

/// One source record in a verified replay prefix.  Raw bytes remain owned by
/// the source/archive; only stable identity and digest evidence is carried in
/// the replay receipt.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VerifiedPrefixRecord {
    pub line_number: usize,
    pub record_id: String,
    pub sequence: u64,
    pub digest: String,
}

impl VerifiedPrefixRecord {
    fn validate(&self) -> Result<(), MigrationContractError> {
        if self.line_number == 0 {
            return Err(MigrationContractError::invalid(
                "verified_prefix.line_number",
                "must be non-zero",
            ));
        }
        if self.sequence == 0 {
            return Err(MigrationContractError::invalid(
                "verified_prefix.sequence",
                "must be non-zero",
            ));
        }
        validate_contract_identifier("verified_prefix.record_id", &self.record_id)?;
        validate_contract_identifier("verified_prefix.digest", &self.digest)
    }
}

/// Evidence for the only source records eligible for direct replay.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VerifiedPrefix {
    pub source_digest: String,
    pub prefix_digest: String,
    pub records: Vec<VerifiedPrefixRecord>,
    pub complete: bool,
    pub stop_line: Option<usize>,
    pub stop_error: Option<String>,
    /// The exact source bytes covered by `prefix_digest`.  This is an
    /// in-memory verification witness: it is deliberately omitted from the
    /// serialized contract so a deserialized prefix must be rebound through
    /// `verify_against_report` or `verify_against_bytes` before apply.
    #[serde(skip)]
    raw_prefix_bytes: Option<Vec<u8>>,
}

impl VerifiedPrefix {
    /// Derive a prefix from the existing read-only classifier.  The method
    /// stops at the first record that is not direct and retains its exact
    /// error.  It never changes the report or attempts replay.
    pub fn from_report(report: &InspectionReport) -> Result<Self, MigrationContractError> {
        let mut records = Vec::new();
        let mut prefix_bytes = Vec::new();
        let mut expected_sequence = None;
        let mut stop_line = None;
        let mut stop_error = None;

        for record in &report.records {
            let Some(record_id) = record.record_id.as_ref() else {
                stop_line = Some(record.line_number);
                stop_error = record.exact_error.clone();
                break;
            };
            let Some(sequence) = record.sequence else {
                stop_line = Some(record.line_number);
                stop_error = record.exact_error.clone();
                break;
            };
            if sequence == 0 {
                stop_line = Some(record.line_number);
                stop_error = Some("INVALID_SEQUENCE:0".to_owned());
                break;
            }
            if record.classification != MappingClass::Direct || record.exact_error.is_some() {
                stop_line = Some(record.line_number);
                stop_error = record
                    .exact_error
                    .clone()
                    .or_else(|| Some("RECORD_NOT_DIRECT".to_owned()));
                break;
            }
            if let Some(expected) = expected_sequence {
                if sequence != expected {
                    stop_line = Some(record.line_number);
                    stop_error = Some(if sequence > expected {
                        format!("SEQUENCE_GAP:expected={expected}:observed={sequence}")
                    } else {
                        format!("SEQUENCE_NON_MONOTONIC:expected={expected}:observed={sequence}")
                    });
                    break;
                }
            }
            expected_sequence = sequence.checked_add(1);
            if expected_sequence.is_none() {
                stop_line = Some(record.line_number);
                stop_error = Some("SEQUENCE_OVERFLOW".to_owned());
                break;
            }
            records.push(VerifiedPrefixRecord {
                line_number: record.line_number,
                record_id: record_id.clone(),
                sequence,
                digest: record.digest.clone(),
            });
            prefix_bytes.extend_from_slice(&record.raw_bytes);
        }

        if stop_line.is_none() {
            if let Some(issue) = report.issues.first() {
                stop_line = issue.line_number;
                stop_error = Some(issue.exact_error.clone());
            }
        }

        let prefix = Self {
            source_digest: report.source_digest.clone(),
            prefix_digest: digest_bytes(&prefix_bytes),
            complete: stop_line.is_none() && stop_error.is_none() && report.issues.is_empty(),
            records,
            stop_line,
            stop_error,
            raw_prefix_bytes: Some(prefix_bytes),
        };
        prefix.validate()?;
        Ok(prefix)
    }

    pub fn validate(&self) -> Result<(), MigrationContractError> {
        validate_contract_identifier("verified_prefix.source_digest", &self.source_digest)?;
        validate_contract_identifier("verified_prefix.prefix_digest", &self.prefix_digest)?;
        if self.records.is_empty() {
            if self.complete {
                return Err(MigrationContractError::missing("verified_prefix.records"));
            }
            if self.stop_error.is_none() {
                return Err(MigrationContractError::invalid(
                    "verified_prefix.stop",
                    "an incomplete empty prefix must retain the exact stop error",
                ));
            }
        }
        let mut expected_sequence = None;
        let mut previous_line = None;
        for record in &self.records {
            record.validate()?;
            if let Some(expected) = expected_sequence {
                if record.sequence != expected {
                    return Err(MigrationContractError::invalid(
                        "verified_prefix.records",
                        format!(
                            "sequence is not contiguous: expected {expected}, observed {}",
                            record.sequence
                        ),
                    ));
                }
            }
            if let Some(previous) = previous_line {
                if record.line_number <= previous {
                    return Err(MigrationContractError::invalid(
                        "verified_prefix.records",
                        format!(
                            "line numbers must increase: previous {previous}, observed {}",
                            record.line_number
                        ),
                    ));
                }
            }
            previous_line = Some(record.line_number);
            expected_sequence = Some(record.sequence.checked_add(1).ok_or_else(|| {
                MigrationContractError::invalid("verified_prefix.sequence", "must not overflow")
            })?);
        }
        if self.complete != (self.stop_line.is_none() && self.stop_error.is_none()) {
            return Err(MigrationContractError::invalid(
                "verified_prefix.complete",
                "complete must be true only when no stop error exists",
            ));
        }
        if self.stop_line.is_some() && self.stop_error.is_none() {
            return Err(MigrationContractError::invalid(
                "verified_prefix.stop",
                "stop_error is required when stop_line is supplied",
            ));
        }
        let Some(raw_prefix_bytes) = self.raw_prefix_bytes.as_deref() else {
            return Err(MigrationContractError::invalid(
                "verified_prefix.evidence",
                "must be rebound against the source report or bytes",
            ));
        };
        if digest_bytes(raw_prefix_bytes) != self.prefix_digest {
            return Err(MigrationContractError::digest_mismatch(
                "verified_prefix.prefix_digest",
                digest_bytes(raw_prefix_bytes),
                self.prefix_digest.clone(),
            ));
        }
        validate_prefix_bytes_against_records(raw_prefix_bytes, &self.records)?;
        Ok(())
    }

    /// Rebind serialized or caller-provided prefix metadata to one inspected
    /// source report.  Every field and every covered line must match the
    /// report-derived prefix before the in-memory byte witness is installed.
    pub fn verify_against_report(
        &mut self,
        report: &InspectionReport,
    ) -> Result<(), MigrationContractError> {
        let expected = Self::from_report(report)?;
        if self.source_digest != expected.source_digest {
            return Err(MigrationContractError::digest_mismatch(
                "verified_prefix.source_digest",
                expected.source_digest,
                self.source_digest.clone(),
            ));
        }
        if self.prefix_digest != expected.prefix_digest {
            return Err(MigrationContractError::digest_mismatch(
                "verified_prefix.prefix_digest",
                expected.prefix_digest,
                self.prefix_digest.clone(),
            ));
        }
        if self.records != expected.records {
            return Err(MigrationContractError::invalid(
                "verified_prefix.records",
                "do not match the report-derived direct prefix",
            ));
        }
        if self.complete != expected.complete
            || self.stop_line != expected.stop_line
            || self.stop_error != expected.stop_error
        {
            return Err(MigrationContractError::invalid(
                "verified_prefix.stop",
                "does not match the report-derived stop boundary",
            ));
        }
        self.raw_prefix_bytes = expected.raw_prefix_bytes;
        self.validate()
    }

    /// Inspect and bind one exact source byte stream.  No filesystem or
    /// daemon operation is performed; the caller remains the source owner.
    pub fn verify_against_bytes(
        &mut self,
        source_bytes: &[u8],
    ) -> Result<(), MigrationContractError> {
        let report = inspect_jsonl(source_bytes);
        self.verify_against_report(&report)
    }

    pub fn verify_source_digest(&self, observed: &str) -> Result<(), MigrationContractError> {
        self.validate()?;
        if self.source_digest == observed {
            Ok(())
        } else {
            Err(MigrationContractError::digest_mismatch(
                "verified_prefix.source_digest",
                self.source_digest.clone(),
                observed,
            ))
        }
    }

    pub fn last_sequence(&self) -> Option<u64> {
        self.records.last().map(|record| record.sequence)
    }

    pub fn record_count(&self) -> usize {
        self.records.len()
    }
}

/// Receipt produced by the pure apply gate.  It is evidence of validation,
/// not a journal commit; callers still need the resident writer to persist it.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MigrationApplyReceipt {
    pub migration_id: String,
    pub source_project_id: String,
    pub target_epoch: u64,
    pub source_snapshot_digest: String,
    pub verified_prefix_digest: String,
    pub writer_operation_id: String,
}

/// Durable lifecycle states for the typed migration manifest.
#[derive(Debug, Clone, Copy, Eq, Hash, Ord, PartialEq, PartialOrd, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MigrationMappingStatus {
    Planned,
    Running,
    Verified,
    NeedsOperator,
    ResetRequired,
    Aborted,
}

impl MigrationMappingStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Planned => "planned",
            Self::Running => "running",
            Self::Verified => "verified",
            Self::NeedsOperator => "needs_operator",
            Self::ResetRequired => "reset_required",
            Self::Aborted => "aborted",
        }
    }
}

/// Durable lifecycle states for one mapped source record.
#[derive(Debug, Clone, Copy, Eq, Hash, Ord, PartialEq, PartialOrd, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ManifestRecordStatus {
    Planned,
    Mapped,
    NeedsReconciliation,
    Blocked,
    Rejected,
    Unknown,
}

impl ManifestRecordStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Planned => "planned",
            Self::Mapped => "mapped",
            Self::NeedsReconciliation => "needs_reconciliation",
            Self::Blocked => "blocked",
            Self::Rejected => "rejected",
            Self::Unknown => "unknown",
        }
    }
}

/// Identity supplied by the migration owner. The manifest builder never
/// fabricates source or target truth from these values.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ManifestIdentity {
    pub migration_id: String,
    pub source_project_id: String,
    pub canonical_project_cwd: String,
    pub owner_authority: String,
    pub source_schema_version: String,
    pub target_schema_version: String,
    pub source_repo: Option<String>,
    pub source_branch: Option<String>,
    pub source_head: Option<String>,
    pub source_tree: Option<String>,
    pub source_epoch: Option<u64>,
    pub target_epoch: u64,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MigrationAdmissionEvidence {
    pub writer_frozen: bool,
    pub live_scope_verified: bool,
    pub current_source_verified: bool,
    pub source_identity_verified: bool,
    pub archive_ref: String,
    pub archive_digest: String,
}

impl MigrationAdmissionEvidence {
    fn validate(&self) -> Result<(), MigrationContractError> {
        if !self.writer_frozen
            || !self.live_scope_verified
            || !self.current_source_verified
            || !self.source_identity_verified
        {
            return Err(MigrationContractError::missing(
                "manifest.project_admission_evidence",
            ));
        }
        validate_contract_reference("manifest.archive_ref", &self.archive_ref)?;
        validate_contract_identifier("manifest.archive_digest", &self.archive_digest)
    }
}

impl ManifestIdentity {
    pub fn new(
        migration_id: impl Into<String>,
        source_project_id: impl Into<String>,
        canonical_project_cwd: impl Into<String>,
    ) -> Result<Self, MigrationContractError> {
        let identity = Self {
            migration_id: migration_id.into(),
            source_project_id: source_project_id.into(),
            canonical_project_cwd: canonical_project_cwd.into(),
            owner_authority: "inspection:typed-manifest".to_owned(),
            source_schema_version: "collab-journal/v1".to_owned(),
            target_schema_version: "collab-global/v1".to_owned(),
            source_repo: None,
            source_branch: None,
            source_head: None,
            source_tree: None,
            source_epoch: None,
            target_epoch: 2,
            created_at: "inspection".to_owned(),
            updated_at: "inspection".to_owned(),
        };
        identity.validate()?;
        Ok(identity)
    }

    fn validate(&self) -> Result<(), MigrationContractError> {
        validate_contract_identifier("migration_id", &self.migration_id)?;
        validate_contract_identifier("source_project_id", &self.source_project_id)?;
        validate_contract_reference("canonical_project_cwd", &self.canonical_project_cwd)?;
        validate_contract_identifier("owner_authority", &self.owner_authority)?;
        validate_contract_identifier("source_schema_version", &self.source_schema_version)?;
        validate_contract_identifier("target_schema_version", &self.target_schema_version)?;
        validate_contract_identifier("created_at", &self.created_at)?;
        validate_contract_identifier("updated_at", &self.updated_at)?;
        if let Some(source_epoch) = self.source_epoch {
            if source_epoch == 0 {
                return Err(MigrationContractError::invalid(
                    "source_epoch",
                    "must be non-zero when present",
                ));
            }
        }
        if self.target_epoch == 0 {
            return Err(MigrationContractError::invalid(
                "target_epoch",
                "must be non-zero",
            ));
        }
        if self.source_epoch == Some(self.target_epoch) {
            return Err(MigrationContractError::invalid(
                "source_epoch",
                "must differ from target_epoch",
            ));
        }
        Ok(())
    }
}

/// One schema-shaped mapping row for a source record. Raw source bytes remain
/// outside the manifest; the row carries only identity, digest, decision and
/// target binding evidence.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MigrationManifestRecord {
    pub source_record_id: String,
    pub source_record_type: String,
    pub source_record_digest: String,
    pub source_disposition: SourceDisposition,
    pub target_epoch: String,
    pub target_sequence: Option<u64>,
    pub target_entity_id: Option<String>,
    pub agent_id: Option<String>,
    pub runtime_id: Option<String>,
    pub binding_id: Option<String>,
    pub endpoint_generation: Option<u64>,
    pub appsdk_record_ref: Option<String>,
    pub appsdk_record_digest: Option<String>,
    pub owner_authority: String,
    pub blocker_code: Option<String>,
    pub mapping_class: MappingClass,
    pub mapping_status: ManifestRecordStatus,
    pub raw_archive_ref: Option<String>,
    pub exact_error: Option<String>,
    pub first_failed_boundary: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

impl MigrationManifestRecord {
    fn from_inspection(
        record: &RecordInspection,
        identity: &ManifestIdentity,
    ) -> Result<Self, MigrationContractError> {
        let source_record_id = record
            .record_id
            .clone()
            .unwrap_or_else(|| format!("line-{}-unidentified", record.line_number));
        let source_record_type = record
            .event
            .clone()
            .unwrap_or_else(|| "unclassified".to_owned());
        let blocker_code = record
            .exact_error
            .as_deref()
            .map(|error| error.split(':').next().unwrap_or(error).to_owned());
        let mapping_status = match record.source_disposition {
            SourceDisposition::DirectReplay => ManifestRecordStatus::Planned,
            SourceDisposition::AdaptReconcile => ManifestRecordStatus::NeedsReconciliation,
            SourceDisposition::ArchiveOnly => ManifestRecordStatus::Blocked,
            SourceDisposition::RebuildRequired => ManifestRecordStatus::Blocked,
        };
        let row = Self {
            source_record_id,
            source_record_type,
            source_record_digest: record.digest.clone(),
            source_disposition: record.source_disposition,
            target_epoch: identity.target_epoch.to_string(),
            target_sequence: None,
            target_entity_id: None,
            agent_id: None,
            runtime_id: None,
            binding_id: None,
            endpoint_generation: None,
            appsdk_record_ref: None,
            appsdk_record_digest: None,
            owner_authority: identity.owner_authority.clone(),
            blocker_code,
            mapping_class: record.classification,
            mapping_status,
            raw_archive_ref: None,
            exact_error: record.exact_error.clone(),
            first_failed_boundary: record.first_failed_boundary.clone(),
            created_at: identity.created_at.clone(),
            updated_at: identity.updated_at.clone(),
        };
        row.validate()?;
        Ok(row)
    }

    pub fn validate(&self) -> Result<(), MigrationContractError> {
        validate_contract_identifier("manifest.record.source_record_id", &self.source_record_id)?;
        validate_contract_identifier(
            "manifest.record.source_record_type",
            &self.source_record_type,
        )?;
        validate_contract_identifier(
            "manifest.record.source_record_digest",
            &self.source_record_digest,
        )?;
        validate_contract_identifier("manifest.record.target_epoch", &self.target_epoch)?;
        validate_contract_identifier("manifest.record.owner_authority", &self.owner_authority)?;
        validate_contract_identifier("manifest.record.created_at", &self.created_at)?;
        validate_contract_identifier("manifest.record.updated_at", &self.updated_at)?;
        if self.mapping_status == ManifestRecordStatus::Mapped {
            if self.mapping_class == MappingClass::Unknown {
                return Err(MigrationContractError::invalid(
                    "manifest.record.mapping_status",
                    "unknown records cannot be mapped",
                ));
            }
            if self.target_sequence.is_none() || self.target_entity_id.is_none() {
                return Err(MigrationContractError::missing(
                    "manifest.record.target_sequence_or_entity",
                ));
            }
        }
        if self.mapping_class == MappingClass::Unknown {
            if self.mapping_status == ManifestRecordStatus::Mapped {
                return Err(MigrationContractError::invalid(
                    "manifest.record.mapping_status",
                    "unknown records cannot be mapped",
                ));
            }
            if self.blocker_code.is_none()
                || self.exact_error.is_none()
                || self.first_failed_boundary.is_none()
            {
                return Err(MigrationContractError::missing(
                    "manifest.record.unknown_evidence",
                ));
            }
        }
        if self.mapping_class == MappingClass::Reset {
            if self.mapping_status == ManifestRecordStatus::Mapped {
                return Err(MigrationContractError::invalid(
                    "manifest.record.mapping_status",
                    "reset records cannot be mapped",
                ));
            }
            if self.blocker_code.is_none()
                || self.exact_error.is_none()
                || self.first_failed_boundary.is_none()
            {
                return Err(MigrationContractError::missing(
                    "manifest.record.reset_evidence",
                ));
            }
        }
        Ok(())
    }
}

/// Typed manifest derived from one inspection report. This is the in-memory
/// schema owner for a future journaled manifest fact; deriving it performs no
/// archive, epoch, projection or daemon write.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MigrationManifest {
    pub migration_id: String,
    pub source_project_id: String,
    pub canonical_project_cwd: String,
    pub source_schema_version: String,
    pub target_schema_version: String,
    pub source_snapshot_digest: String,
    pub source_epoch: Option<String>,
    pub target_epoch: String,
    pub project_admission: ProjectAdmission,
    pub owner_authority: String,
    pub blocker_code: Option<String>,
    pub first_failed_boundary: Option<String>,
    pub mapping_status: MigrationMappingStatus,
    pub archive_ref: Option<String>,
    pub archive_digest: Option<String>,
    pub admission_evidence: Option<MigrationAdmissionEvidence>,
    pub created_at: String,
    pub updated_at: String,
    pub source_repo: Option<String>,
    pub source_branch: Option<String>,
    pub source_head: Option<String>,
    pub source_tree: Option<String>,
    pub records: Vec<MigrationManifestRecord>,
}

impl MigrationManifest {
    pub fn from_report(
        report: &InspectionReport,
        identity: ManifestIdentity,
    ) -> Result<Self, MigrationContractError> {
        Self::from_report_with_admission(report, identity, None)
    }

    pub fn from_report_with_admission(
        report: &InspectionReport,
        identity: ManifestIdentity,
        admission_evidence: Option<MigrationAdmissionEvidence>,
    ) -> Result<Self, MigrationContractError> {
        identity.validate()?;
        let mut records = Vec::with_capacity(report.records.len());
        for record in &report.records {
            records.push(MigrationManifestRecord::from_inspection(record, &identity)?);
        }
        let (blocker_code, first_failed_boundary) = if let Some(issue) = report.issues.first() {
            (
                Some(
                    issue
                        .exact_error
                        .split(':')
                        .next()
                        .unwrap_or(&issue.exact_error)
                        .to_owned(),
                ),
                Some(issue.first_failed_boundary.clone()),
            )
        } else if let Some(record) = records.iter().find(|record| {
            matches!(
                record.mapping_class,
                MappingClass::Reset | MappingClass::Unknown
            ) && record.exact_error.is_some()
                && record.first_failed_boundary.is_some()
        }) {
            (
                record.blocker_code.clone(),
                record.first_failed_boundary.clone(),
            )
        } else if let Some(record) = records
            .iter()
            .find(|record| record.exact_error.is_some() && record.first_failed_boundary.is_some())
        {
            (
                record.blocker_code.clone(),
                record.first_failed_boundary.clone(),
            )
        } else if admission_evidence.is_none() {
            (
                Some("PROJECT_ADMISSION_EVIDENCE_ABSENT".to_owned()),
                Some("project_admission".to_owned()),
            )
        } else {
            (None, None)
        };
        let project_admission = if admission_evidence.is_some()
            && report.issues.is_empty()
            && report
                .records
                .iter()
                .all(|record| record.classification == MappingClass::Direct)
        {
            ProjectAdmission::Verified
        } else if report
            .records
            .iter()
            .any(|record| record.classification == MappingClass::Reset)
        {
            ProjectAdmission::ResetRequired
        } else {
            ProjectAdmission::NeedsOperator
        };
        let manifest = Self {
            migration_id: identity.migration_id,
            source_project_id: identity.source_project_id,
            canonical_project_cwd: identity.canonical_project_cwd,
            source_schema_version: identity.source_schema_version,
            target_schema_version: identity.target_schema_version,
            source_snapshot_digest: report.source_digest.clone(),
            source_epoch: identity.source_epoch.map(|epoch| epoch.to_string()),
            target_epoch: identity.target_epoch.to_string(),
            project_admission,
            owner_authority: identity.owner_authority,
            blocker_code,
            first_failed_boundary,
            mapping_status: MigrationMappingStatus::Planned,
            archive_ref: admission_evidence
                .as_ref()
                .map(|evidence| evidence.archive_ref.clone()),
            archive_digest: admission_evidence
                .as_ref()
                .map(|evidence| evidence.archive_digest.clone()),
            admission_evidence,
            created_at: identity.created_at,
            updated_at: identity.updated_at,
            source_repo: identity.source_repo,
            source_branch: identity.source_branch,
            source_head: identity.source_head,
            source_tree: identity.source_tree,
            records,
        };
        manifest.validate()?;
        Ok(manifest)
    }

    pub fn validate(&self) -> Result<(), MigrationContractError> {
        validate_contract_identifier("manifest.migration_id", &self.migration_id)?;
        validate_contract_identifier("manifest.source_project_id", &self.source_project_id)?;
        validate_contract_reference(
            "manifest.canonical_project_cwd",
            &self.canonical_project_cwd,
        )?;
        validate_contract_identifier(
            "manifest.source_schema_version",
            &self.source_schema_version,
        )?;
        validate_contract_identifier(
            "manifest.target_schema_version",
            &self.target_schema_version,
        )?;
        validate_contract_identifier(
            "manifest.source_snapshot_digest",
            &self.source_snapshot_digest,
        )?;
        validate_contract_identifier("manifest.target_epoch", &self.target_epoch)?;
        validate_contract_identifier("manifest.owner_authority", &self.owner_authority)?;
        validate_contract_identifier("manifest.created_at", &self.created_at)?;
        validate_contract_identifier("manifest.updated_at", &self.updated_at)?;
        if self.records.is_empty() {
            return Err(MigrationContractError::missing("manifest.records"));
        }
        if self.project_admission == ProjectAdmission::Verified {
            let evidence = self.admission_evidence.as_ref().ok_or_else(|| {
                MigrationContractError::missing("manifest.project_admission_evidence")
            })?;
            evidence.validate()?;
            if self.archive_ref.as_deref() != Some(evidence.archive_ref.as_str())
                || self.archive_digest.as_deref() != Some(evidence.archive_digest.as_str())
            {
                return Err(MigrationContractError::invalid(
                    "manifest.project_admission_evidence",
                    "archive reference and digest must match the verified admission",
                ));
            }
            validate_contract_reference(
                "manifest.source_repo",
                self.source_repo.as_deref().unwrap_or_default(),
            )?;
            if self.source_branch.is_some() != self.source_head.is_some()
                || self.source_head.is_some() != self.source_tree.is_some()
            {
                return Err(MigrationContractError::invalid(
                    "manifest.source_identity",
                    "branch, head, and tree must be either all present or all null",
                ));
            }
        }
        let mut keys = std::collections::BTreeSet::new();
        for record in &self.records {
            record.validate()?;
            if record.target_epoch != self.target_epoch {
                return Err(MigrationContractError::EpochMismatch {
                    field: "manifest.record.target_epoch",
                    expected: self.target_epoch.parse::<u64>().map_err(|error| {
                        MigrationContractError::invalid("manifest.target_epoch", error.to_string())
                    })?,
                    observed: record.target_epoch.parse::<u64>().map_err(|error| {
                        MigrationContractError::invalid(
                            "manifest.record.target_epoch",
                            error.to_string(),
                        )
                    })?,
                });
            }
            let key = format!(
                "{}:{}:{}:{}",
                self.source_project_id,
                record.source_record_id,
                record.source_record_digest,
                record.target_epoch
            );
            if !keys.insert(key) {
                return Err(MigrationContractError::invalid(
                    "manifest.records",
                    "duplicate source record mapping key",
                ));
            }
        }
        if matches!(
            self.project_admission,
            ProjectAdmission::ResetRequired
                | ProjectAdmission::NeedsOperator
                | ProjectAdmission::Aborted
        ) && (self.blocker_code.is_none() || self.first_failed_boundary.is_none())
        {
            return Err(MigrationContractError::missing(
                "manifest.project_admission_evidence",
            ));
        }
        Ok(())
    }

    pub fn record_idempotency_key(&self, record: &MigrationManifestRecord) -> String {
        format!(
            "{}:{}:{}:{}",
            self.source_project_id,
            record.source_record_id,
            record.source_record_digest,
            record.target_epoch
        )
    }
}

/// Evidence from a no-write rehearsal. It binds the verified-prefix replay,
/// projection rebuild and fenced rollback decision to one immutable source.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct MigrationRehearsal {
    pub manifest: MigrationManifest,
    pub verified_prefix_digest: String,
    pub replayed_records: usize,
    pub projection_bytes: Vec<u8>,
    pub projection_digest: String,
    pub rebuilt_projection_tasks: Vec<String>,
    pub rollback_fence: TargetEpoch,
    pub stop_error: String,
}

impl MigrationRehearsal {
    pub fn no_write(
        report: &InspectionReport,
        prefix: &VerifiedPrefix,
        identity: ManifestIdentity,
    ) -> Result<Self, MigrationContractError> {
        prefix.validate()?;
        let manifest = MigrationManifest::from_report(report, identity)?;
        prefix.verify_source_digest(&manifest.source_snapshot_digest)?;
        let mut state = crate::server::state::State::default();
        for record in &prefix.records {
            let line = report
                .records
                .iter()
                .find(|candidate| {
                    candidate.line_number == record.line_number
                        && candidate.record_id.as_deref() == Some(record.record_id.as_str())
                        && candidate.sequence == Some(record.sequence)
                })
                .ok_or_else(|| {
                    MigrationContractError::invalid(
                        "rehearsal.verified_prefix",
                        "prefix record is not present in the inspection report",
                    )
                })?;
            let content = trim_line_ending(&line.raw_bytes);
            let event: Event = serde_json::from_slice(content).map_err(|error| {
                MigrationContractError::invalid(
                    "rehearsal.event",
                    format!(
                        "verified prefix line {} does not deserialize: {error}",
                        line.line_number
                    ),
                )
            })?;
            state.apply_checked(&event).map_err(|error| {
                MigrationContractError::invalid(
                    "rehearsal.reducer",
                    format!(
                        "verified prefix line {} was rejected by the reducer: {error}",
                        line.line_number
                    ),
                )
            })?;
        }
        let projection_bytes = serde_json::to_vec(&serde_json::json!({
            "tasks": state.tasks.values().map(|task| task.id.clone()).collect::<Vec<_>>(),
            "workers": state.workers.keys().cloned().collect::<Vec<_>>(),
            "messages": state.msgs.keys().cloned().collect::<Vec<_>>(),
        }))
        .map_err(|error| {
            MigrationContractError::invalid("rehearsal.projection", error.to_string())
        })?;
        let mut rebuilt_projection_tasks: Vec<String> =
            state.tasks.keys().cloned().collect::<Vec<_>>();
        rebuilt_projection_tasks.sort();
        let projection_digest = digest_bytes(&projection_bytes);
        let rollback_fence = TargetEpoch::new(
            None,
            manifest.target_epoch.parse::<u64>().map_err(|error| {
                MigrationContractError::invalid("manifest.target_epoch", error.to_string())
            })?,
            0,
        )?;
        let stop_error = prefix
            .stop_error
            .clone()
            .or_else(|| {
                manifest
                    .records
                    .iter()
                    .find_map(|record| record.blocker_code.clone())
            })
            .unwrap_or_else(|| "VERIFIED_PREFIX_COMPLETE".to_owned());
        Ok(Self {
            manifest,
            verified_prefix_digest: prefix.prefix_digest.clone(),
            replayed_records: prefix.records.len(),
            projection_bytes,
            projection_digest,
            rebuilt_projection_tasks,
            rollback_fence,
            stop_error,
        })
    }
}
