use super::*;

/// Pure migration transaction gate.  It can be constructed and validated in
/// a copied fixture; no method here opens a live source, freezes a daemon,
/// creates an archive, allocates a target sequence, or rebinds a runtime.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MigrationTransaction {
    pub migration_id: String,
    pub source_project_id: String,
    pub project_admission: ProjectAdmission,
    pub phase: MigrationPhase,
    pub snapshot: Option<ImmutableSnapshot>,
    pub target_epoch: Option<TargetEpoch>,
    pub verified_prefix: Option<VerifiedPrefix>,
    pub receipts: Option<MigrationReceiptSet>,
}

impl MigrationTransaction {
    pub fn new(
        migration_id: impl Into<String>,
        source_project_id: impl Into<String>,
    ) -> Result<Self, MigrationContractError> {
        let transaction = Self {
            migration_id: migration_id.into(),
            source_project_id: source_project_id.into(),
            project_admission: ProjectAdmission::NeedsOperator,
            phase: MigrationPhase::Planned,
            snapshot: None,
            target_epoch: None,
            verified_prefix: None,
            receipts: None,
        };
        transaction.validate_identity()?;
        Ok(transaction)
    }

    pub fn validate(&self) -> Result<(), MigrationContractError> {
        self.validate_identity()?;
        if self.phase == MigrationPhase::ResetRequired
            && self.project_admission != ProjectAdmission::ResetRequired
        {
            return Err(MigrationContractError::invalid(
                "phase",
                "reset_required phase must carry reset_required project admission",
            ));
        }

        if let Some(snapshot) = self.snapshot.as_ref() {
            snapshot.validate()?;
            if snapshot.migration_id != self.migration_id {
                return Err(MigrationContractError::invalid(
                    "snapshot.migration_id",
                    "does not match migration transaction",
                ));
            }
            if snapshot.source_project_id != self.source_project_id {
                return Err(MigrationContractError::invalid(
                    "snapshot.source_project_id",
                    "does not match migration transaction",
                ));
            }
        }

        if let Some(target_epoch) = self.target_epoch.as_ref() {
            target_epoch.validate()?;
            if let Some(snapshot) = self.snapshot.as_ref() {
                if snapshot.source_epoch != target_epoch.source_epoch {
                    return Err(match (target_epoch.source_epoch, snapshot.source_epoch) {
                        (Some(expected), observed) => MigrationContractError::EpochMismatch {
                            field: "target_epoch.source_epoch",
                            expected,
                            observed: observed.unwrap_or_default(),
                        },
                        (None, Some(observed)) => MigrationContractError::invalid(
                            "target_epoch.source_epoch",
                            format!("snapshot source epoch {observed} is not bound to the target"),
                        ),
                        (None, None) => MigrationContractError::invalid(
                            "target_epoch.source_epoch",
                            "source epoch values differ",
                        ),
                    });
                }
                if snapshot.source_epoch == Some(target_epoch.target_epoch) {
                    return Err(MigrationContractError::invalid(
                        "target_epoch.target_epoch",
                        "must differ from the snapshot source epoch",
                    ));
                }
            }
        }

        if let Some(prefix) = self.verified_prefix.as_ref() {
            prefix.validate()?;
            if let Some(snapshot) = self.snapshot.as_ref() {
                if prefix.source_digest != snapshot.source_digest {
                    return Err(MigrationContractError::digest_mismatch(
                        "verified_prefix.source_digest",
                        snapshot.source_digest.clone(),
                        prefix.source_digest.clone(),
                    ));
                }
            }
        }

        if let Some(receipts) = self.receipts.as_ref() {
            let snapshot = self
                .snapshot
                .as_ref()
                .ok_or_else(|| MigrationContractError::missing("snapshot"))?;
            let target_epoch = self
                .target_epoch
                .as_ref()
                .ok_or_else(|| MigrationContractError::missing("target_epoch"))?;
            receipts
                .validate_for(
                    &self.migration_id,
                    &self.source_project_id,
                    &snapshot.source_digest,
                    target_epoch.target_epoch,
                )
                .map_err(|error| MigrationContractError::ReceiptRejected {
                    reason: error.to_string(),
                })?;
            if let Some(writer) = receipts.writer.as_ref() {
                if writer.source_epoch != snapshot.source_epoch {
                    return Err(MigrationContractError::invalid(
                        "receipts.writer.source_epoch",
                        format!(
                            "does not match snapshot source epoch {:?}",
                            snapshot.source_epoch
                        ),
                    ));
                }
            }
        }

        match self.phase {
            MigrationPhase::SnapshotCaptured if self.snapshot.is_none() => {
                return Err(MigrationContractError::missing("snapshot"));
            }
            MigrationPhase::ReplayVerified if self.verified_prefix.is_none() => {
                return Err(MigrationContractError::missing("verified_prefix"));
            }
            MigrationPhase::Rebound if self.receipts.is_none() => {
                return Err(MigrationContractError::missing("receipts"));
            }
            MigrationPhase::Applied | MigrationPhase::Verified => {
                for (field, present) in [
                    ("snapshot", self.snapshot.is_some()),
                    ("target_epoch", self.target_epoch.is_some()),
                    ("verified_prefix", self.verified_prefix.is_some()),
                    ("receipts", self.receipts.is_some()),
                ] {
                    if !present {
                        return Err(MigrationContractError::missing(field));
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }

    /// Validate every precondition required before an apply side effect.
    /// `reset_required`, `needs_operator`, and `aborted` are terminal stops;
    /// they are rejected before receipt or source validation is attempted.
    /// Validate every precondition against the currently active epoch and
    /// revision before an apply side effect is admitted.
    pub fn validate_for_apply(
        &self,
        active_epoch: u64,
        active_revision: u64,
    ) -> Result<(), MigrationContractError> {
        self.validate_identity()?;
        if self.project_admission != ProjectAdmission::Verified {
            return Err(MigrationContractError::AdmissionBlocked {
                admission: self.project_admission,
            });
        }
        self.validate()?;
        if !matches!(
            self.phase,
            MigrationPhase::Rebound | MigrationPhase::Applied | MigrationPhase::Verified
        ) {
            return Err(MigrationContractError::invalid(
                "phase",
                format!("{} cannot be applied", self.phase.as_str()),
            ));
        }
        for (field, present) in [
            ("snapshot", self.snapshot.is_some()),
            ("target_epoch", self.target_epoch.is_some()),
            ("verified_prefix", self.verified_prefix.is_some()),
            ("receipts", self.receipts.is_some()),
        ] {
            if !present {
                return Err(MigrationContractError::missing(field));
            }
        }
        self.target_epoch
            .as_ref()
            .expect("target epoch presence checked above")
            .validate_committed_against(active_epoch, active_revision)?;
        Ok(())
    }

    /// Return a typed apply receipt without mutating source or target state.
    /// The active epoch and revision are required so the target cannot be
    /// reused or moved backward between validation and apply admission.
    pub fn apply(
        &self,
        active_epoch: u64,
        active_revision: u64,
    ) -> Result<MigrationApplyReceipt, MigrationContractError> {
        self.validate_for_apply(active_epoch, active_revision)?;
        let snapshot = self
            .snapshot
            .as_ref()
            .ok_or_else(|| MigrationContractError::missing("snapshot"))?;
        let target_epoch = self
            .target_epoch
            .as_ref()
            .ok_or_else(|| MigrationContractError::missing("target_epoch"))?;
        let prefix = self
            .verified_prefix
            .as_ref()
            .ok_or_else(|| MigrationContractError::missing("verified_prefix"))?;
        if !prefix.complete {
            return Err(MigrationContractError::PrefixBlocked {
                line: prefix.stop_line,
                exact_error: prefix
                    .stop_error
                    .clone()
                    .unwrap_or_else(|| "VERIFIED_PREFIX_INCOMPLETE".to_owned()),
            });
        }
        let receipts = self
            .receipts
            .as_ref()
            .ok_or_else(|| MigrationContractError::missing("receipts"))?;
        let writer_operation_id = receipts
            .writer
            .as_ref()
            .map(|writer| writer.operation_id.as_str().to_owned())
            .ok_or_else(|| MigrationContractError::missing("receipts.writer"))?;
        Ok(MigrationApplyReceipt {
            migration_id: self.migration_id.clone(),
            source_project_id: self.source_project_id.clone(),
            target_epoch: target_epoch.target_epoch,
            source_snapshot_digest: snapshot.source_digest.clone(),
            verified_prefix_digest: prefix.prefix_digest.clone(),
            writer_operation_id,
        })
    }

    fn validate_identity(&self) -> Result<(), MigrationContractError> {
        validate_contract_identifier("migration_id", &self.migration_id)?;
        validate_contract_identifier("source_project_id", &self.source_project_id)
    }
}

pub(crate) fn validate_contract_identifier(
    field: &'static str,
    value: &str,
) -> Result<(), MigrationContractError> {
    if value.trim().is_empty() {
        return Err(MigrationContractError::missing(field));
    }
    if value
        .chars()
        .any(|character| character.is_control() || character.is_whitespace())
    {
        return Err(MigrationContractError::invalid(
            field,
            "must not contain whitespace or control characters",
        ));
    }
    Ok(())
}

pub(crate) fn validate_contract_reference(
    field: &'static str,
    value: &str,
) -> Result<(), MigrationContractError> {
    if value.trim().is_empty() {
        return Err(MigrationContractError::missing(field));
    }
    if value.chars().any(char::is_control) {
        return Err(MigrationContractError::invalid(
            field,
            "must not contain control characters",
        ));
    }
    Ok(())
}

/// Bind a serialized prefix receipt to the exact source bytes that it claims
/// to cover.  The receipt only carries stable record metadata, so the bytes
/// must be inspected again before the prefix can pass validation or apply.
pub(crate) fn validate_prefix_bytes_against_records(
    raw_prefix_bytes: &[u8],
    expected_records: &[VerifiedPrefixRecord],
) -> Result<(), MigrationContractError> {
    let report = inspect_jsonl(raw_prefix_bytes);
    if report.records.len() != expected_records.len() {
        return Err(MigrationContractError::invalid(
            "verified_prefix.records",
            format!(
                "source prefix contains {} records, expected {}",
                report.records.len(),
                expected_records.len()
            ),
        ));
    }

    for (expected, observed) in expected_records.iter().zip(&report.records) {
        if observed.line_number != expected.line_number {
            return Err(MigrationContractError::invalid(
                "verified_prefix.records.line_number",
                format!(
                    "expected {}, observed {}",
                    expected.line_number, observed.line_number
                ),
            ));
        }
        if observed.record_id.as_deref() != Some(expected.record_id.as_str()) {
            return Err(MigrationContractError::invalid(
                "verified_prefix.records.record_id",
                format!(
                    "expected {}, observed {:?}",
                    expected.record_id, observed.record_id
                ),
            ));
        }
        if observed.sequence != Some(expected.sequence) {
            return Err(MigrationContractError::invalid(
                "verified_prefix.records.sequence",
                format!(
                    "expected {}, observed {:?}",
                    expected.sequence, observed.sequence
                ),
            ));
        }

        let observed_digest = digest_bytes(&observed.raw_bytes);
        if observed_digest != expected.digest {
            return Err(MigrationContractError::digest_mismatch(
                "verified_prefix.records.digest",
                expected.digest.clone(),
                observed_digest,
            ));
        }
        if observed.classification != MappingClass::Direct || observed.exact_error.is_some() {
            return Err(MigrationContractError::PrefixBlocked {
                line: Some(observed.line_number),
                exact_error: observed
                    .exact_error
                    .clone()
                    .unwrap_or_else(|| "RECORD_NOT_DIRECT".to_owned()),
            });
        }
    }
    Ok(())
}

impl MappingClass {
    fn combine(self, other: Self) -> Self {
        self.max(other)
    }
}

/// Optional context supplied by the migration owner.  The context is used for
/// validation only; it never changes the preserved source bytes.
#[derive(Debug, Clone, Default, Eq, PartialEq)]
pub struct InspectOptions {
    pub source_path: Option<PathBuf>,
    pub canonical_project_cwd: Option<PathBuf>,
}

/// One deterministic issue found while inspecting a source or replay shape.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct HistoryIssue {
    pub line_number: Option<usize>,
    pub classification: MappingClass,
    pub exact_error: String,
    pub first_failed_boundary: String,
}

/// A parsed source line with the original bytes and line digest retained.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct RecordInspection {
    pub line_number: usize,
    pub raw_bytes: Vec<u8>,
    /// SHA-256 of this complete source line, including its line ending when
    /// one was present in the source stream.
    pub digest: String,
    pub record_id: Option<String>,
    pub sequence: Option<u64>,
    pub entity_id: Option<String>,
    pub event: Option<String>,
    pub owner: Option<String>,
    pub cwd: Option<String>,
    pub worktree: Option<String>,
    pub base_commit: Option<String>,
    pub status: Option<String>,
    pub classification: MappingClass,
    pub source_disposition: SourceDisposition,
    pub exact_error: Option<String>,
    pub first_failed_boundary: Option<String>,
}

/// Read-only result for one source JSONL stream.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct InspectionReport {
    pub source_path: Option<PathBuf>,
    /// SHA-256 of the exact inspected byte stream.  A migration manifest may
    /// use this as its source snapshot digest after the caller records the
    /// corresponding source path and project context.
    pub source_digest: String,
    pub byte_len: usize,
    pub line_count: usize,
    pub final_newline: bool,
    pub classification: MappingClass,
    pub records: Vec<RecordInspection>,
    pub issues: Vec<HistoryIssue>,
}

/// A source digest mismatch is a migration admission failure, not a retryable
/// parse error.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct DigestDrift {
    pub expected: String,
    pub observed: String,
}

impl fmt::Display for DigestDrift {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "DIGEST_DRIFT:expected={}:observed={}",
            self.expected, self.observed
        )
    }
}

impl Error for DigestDrift {}

impl InspectionReport {
    /// Verify that the bytes inspected are the bytes admitted by the caller.
    pub fn verify_digest(&self, expected: &str) -> Result<(), DigestDrift> {
        if self.source_digest == expected {
            Ok(())
        } else {
            Err(DigestDrift {
                expected: expected.to_owned(),
                observed: self.source_digest.clone(),
            })
        }
    }

    pub fn has_error(&self, code: &str) -> bool {
        self.issues.iter().any(|issue| {
            issue.exact_error == code || issue.exact_error.starts_with(&format!("{code}:"))
        }) || self.records.iter().any(|record| {
            record
                .exact_error
                .as_deref()
                .is_some_and(|error| error == code || error.starts_with(&format!("{code}:")))
        })
    }
}

/// Inspect and classify a JSONL byte stream with default context.
pub fn inspect_jsonl(bytes: &[u8]) -> InspectionReport {
    inspect_jsonl_with_options(bytes, &InspectOptions::default())
}

/// Alias used by callers that want to emphasize the classification step.
pub fn classify_jsonl(bytes: &[u8]) -> InspectionReport {
    inspect_jsonl(bytes)
}

/// Inspect and classify a JSONL byte stream without writing to its source or
/// to any target state.
pub fn inspect_jsonl_with_options(bytes: &[u8], options: &InspectOptions) -> InspectionReport {
    let source_digest = digest_bytes(bytes);
    let final_newline = bytes.last() == Some(&b'\n');
    let lines: Vec<&[u8]> = if bytes.is_empty() {
        Vec::new()
    } else {
        bytes.split_inclusive(|byte| *byte == b'\n').collect()
    };
    let line_count = lines.len();
    let mut records = Vec::new();
    let mut issues = Vec::new();

    if bytes.is_empty() {
        issues.push(issue(None, MappingClass::Unknown, "EMPTY_SOURCE", "source"));
    }
    if let Some(canonical_cwd) = options.canonical_project_cwd.as_ref() {
        if !canonical_cwd.is_absolute() {
            issues.push(issue(
                None,
                MappingClass::Unknown,
                format!("INVALID_CANONICAL_PROJECT_CWD:{}", canonical_cwd.display()),
                "source.context",
            ));
        }
    }

    for (index, raw_bytes) in lines.iter().enumerate() {
        let line_number = index + 1;
        let content = trim_line_ending(raw_bytes);
        if content.iter().all(u8::is_ascii_whitespace) {
            let exact_error = "EMPTY_LINE";
            issues.push(issue(
                Some(line_number),
                MappingClass::Unknown,
                exact_error,
                "parse",
            ));
            records.push(invalid_record(
                line_number,
                raw_bytes.to_vec(),
                exact_error,
                "parse",
            ));
            continue;
        }

        let value = match serde_json::from_slice::<Value>(content) {
            Ok(value) => value,
            Err(error) => {
                let (code, boundary) = if line_number == line_count {
                    ("MALFORMED_JSON_TAIL", "parse.tail")
                } else {
                    ("MALFORMED_JSON_MIDDLE", "parse.middle")
                };
                issues.push(issue(
                    Some(line_number),
                    MappingClass::Unknown,
                    format!("{code}:{error}"),
                    boundary,
                ));
                records.push(invalid_record(
                    line_number,
                    raw_bytes.to_vec(),
                    format!("{code}:{error}"),
                    boundary,
                ));
                continue;
            }
        };

        if !value.is_object() {
            let exact_error = "INVALID_RECORD_SHAPE";
            issues.push(issue(
                Some(line_number),
                MappingClass::Unknown,
                exact_error,
                "schema",
            ));
            records.push(invalid_record(
                line_number,
                raw_bytes.to_vec(),
                exact_error,
                "schema",
            ));
            continue;
        }

        let mut record = inspect_record(line_number, raw_bytes.to_vec(), content, &value, options);
        if record.record_id.is_none() {
            record.classification = record.classification.combine(MappingClass::Adapt);
            record.exact_error = record
                .exact_error
                .or_else(|| Some("LEGACY_RECORD_ID_ABSENT".to_owned()));
            record.first_failed_boundary = record
                .first_failed_boundary
                .or_else(|| Some("schema".to_owned()));
        }
        if record.sequence.is_none() {
            record.classification = record.classification.combine(MappingClass::Adapt);
            record.exact_error = record
                .exact_error
                .or_else(|| Some("LEGACY_SEQUENCE_ABSENT".to_owned()));
            record.first_failed_boundary = record
                .first_failed_boundary
                .or_else(|| Some("schema".to_owned()));
        }
        records.push(record);
    }

    if !bytes.is_empty() && !final_newline {
        issues.push(issue(
            line_count.checked_sub(1).map(|_| line_count),
            MappingClass::Unknown,
            "MISSING_FINAL_NEWLINE",
            "source.integrity",
        ));
    }

    mark_duplicate_record_ids(&mut records, &mut issues);
    mark_duplicate_registrations(&mut records, &mut issues);
    mark_sequence_issues(&mut records, &mut issues);
    for record in &mut records {
        record.source_disposition = SourceDisposition::from_mapping_class(record.classification);
    }

    let has_valid_record = records.iter().any(|record| !is_parse_invalid(record));
    if has_valid_record
        && records
            .iter()
            .filter(|record| !is_parse_invalid(record))
            .all(|record| record.sequence.is_none())
    {
        issues.push(issue(
            None,
            MappingClass::Adapt,
            "LEGACY_SEQUENCE_ABSENT",
            "schema",
        ));
    }

    let mut classification = if records.is_empty() {
        MappingClass::Unknown
    } else {
        records
            .iter()
            .map(|record| record.classification)
            .fold(MappingClass::Direct, MappingClass::combine)
    };
    classification = issues
        .iter()
        .map(|issue| issue.classification)
        .fold(classification, MappingClass::combine);

    InspectionReport {
        source_path: options.source_path.clone(),
        source_digest,
        byte_len: bytes.len(),
        line_count,
        final_newline,
        classification,
        records,
        issues,
    }
}

/// Read a source file and inspect its bytes.  This function has no migration
/// side effect; the caller decides where an archive or target should go.
pub fn inspect_path(path: &Path, options: &InspectOptions) -> io::Result<InspectionReport> {
    let bytes = fs::read(path)?;
    let mut effective = options.clone();
    effective.source_path = Some(path.to_path_buf());
    Ok(inspect_jsonl_with_options(&bytes, &effective))
}

/// A stable SHA-256 digest suitable for snapshot and drift checks.
pub fn digest_bytes(bytes: &[u8]) -> String {
    let mut state = [
        0x6a09e667_u32,
        0xbb67ae85,
        0x3c6ef372,
        0xa54ff53a,
        0x510e527f,
        0x9b05688c,
        0x1f83d9ab,
        0x5be0cd19,
    ];
    let mut padded = Vec::with_capacity((bytes.len() + 72) / 64 * 64);
    padded.extend_from_slice(bytes);
    padded.push(0x80);
    while padded.len() % 64 != 56 {
        padded.push(0);
    }
    padded.extend_from_slice(&((bytes.len() as u64) * 8).to_be_bytes());

    for chunk in padded.chunks_exact(64) {
        let mut words = [0_u32; 64];
        for (index, word) in words[..16].iter_mut().enumerate() {
            let offset = index * 4;
            *word = u32::from_be_bytes([
                chunk[offset],
                chunk[offset + 1],
                chunk[offset + 2],
                chunk[offset + 3],
            ]);
        }
        for index in 16..64 {
            let s0 = words[index - 15].rotate_right(7)
                ^ words[index - 15].rotate_right(18)
                ^ (words[index - 15] >> 3);
            let s1 = words[index - 2].rotate_right(17)
                ^ words[index - 2].rotate_right(19)
                ^ (words[index - 2] >> 10);
            words[index] = words[index - 16]
                .wrapping_add(s0)
                .wrapping_add(words[index - 7])
                .wrapping_add(s1);
        }

        let mut working = state;
        for (index, constant) in SHA256_CONSTANTS.iter().enumerate() {
            let s1 = working[4].rotate_right(6)
                ^ working[4].rotate_right(11)
                ^ working[4].rotate_right(25);
            let choose = (working[4] & working[5]) ^ ((!working[4]) & working[6]);
            let temp1 = working[7]
                .wrapping_add(s1)
                .wrapping_add(choose)
                .wrapping_add(*constant)
                .wrapping_add(words[index]);
            let s0 = working[0].rotate_right(2)
                ^ working[0].rotate_right(13)
                ^ working[0].rotate_right(22);
            let majority =
                (working[0] & working[1]) ^ (working[0] & working[2]) ^ (working[1] & working[2]);
            let temp2 = s0.wrapping_add(majority);
            working[7] = working[6];
            working[6] = working[5];
            working[5] = working[4];
            working[4] = working[3].wrapping_add(temp1);
            working[3] = working[2];
            working[2] = working[1];
            working[1] = working[0];
            working[0] = temp1.wrapping_add(temp2);
        }
        for (target, value) in state.iter_mut().zip(working) {
            *target = target.wrapping_add(value);
        }
    }

    let mut hex = String::with_capacity(64);
    for word in state {
        use std::fmt::Write;
        write!(&mut hex, "{word:08x}").expect("writing to String cannot fail");
    }
    format!("sha256:{hex}")
}

const SHA256_CONSTANTS: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

fn inspect_record(
    line_number: usize,
    raw_bytes: Vec<u8>,
    raw_content: &[u8],
    value: &Value,
    options: &InspectOptions,
) -> RecordInspection {
    let event = string_at(value, &["ev", "type", "event"]);
    let record_id = string_at(value, &["record_id", "event_id", "entry_id"]);
    let sequence = integer_at(value, &["sequence", "seq"]);
    let entity_id = first_nested_string(
        value,
        &[
            (&["task"][..], &["id"][..]),
            (&["worker"][..], &["id"][..]),
            (&["msg"][..], &["id"][..]),
            (&["migration"][..], &["id"][..]),
            (&["subscription"][..], &["id"][..]),
        ],
    )
    .or_else(|| string_at(value, &["message_id", "task_id", "worker_id", "msg_id"]));
    let owner = first_nested_string(
        value,
        &[
            (&["task"][..], &["owner"][..]),
            (&["worker"][..], &["owner"][..]),
        ],
    )
    .or_else(|| string_at(value, &["owner", "created_by", "worker_id"]));
    let cwd = first_nested_string(
        value,
        &[
            (&["task"][..], &["cwd"][..]),
            (&["worker"][..], &["cwd"][..]),
        ],
    )
    .or_else(|| string_at(value, &["cwd", "project_cwd", "canonical_project_cwd"]));
    let worktree = first_nested_string(
        value,
        &[(&["task"][..], &["worktree_path", "worktree"][..])],
    )
    .or_else(|| string_at(value, &["worktree_path", "worktree"]));
    let base_commit = first_nested_string(value, &[(&["task"][..], &["base_commit"][..])])
        .or_else(|| string_at(value, &["base_commit"]));
    let status = first_nested_string(value, &[(&["task"][..], &["status"][..])])
        .or_else(|| string_at(value, &["status", "phase"]));

    let is_task = event
        .as_deref()
        .is_some_and(|name| name.starts_with("Task"))
        || value.get("task").is_some();
    let event_error = event_schema_error(raw_content, value);
    let mut classification = if record_id.is_some() && sequence.is_some() && event_error.is_none() {
        MappingClass::Direct
    } else {
        MappingClass::Adapt
    };
    let mut exact_error = event_error;
    let mut first_failed_boundary = exact_error.as_ref().map(|_| "schema".to_owned());
    if exact_error.is_some() {
        classification = MappingClass::Unknown;
    }

    if is_task {
        if let Some(status_name) = status.as_deref() {
            if !is_known_task_status(status_name) {
                classification = MappingClass::Unknown;
                exact_error = Some(format!("UNKNOWN_TASK_STATUS:{status_name}"));
                first_failed_boundary = Some("schema".to_owned());
            }
        }
    }
    if is_task && status.as_deref() == Some("available") {
        classification = classification.combine(MappingClass::Adapt);
        if exact_error.is_none() {
            exact_error = Some("DEPRECATED_TASK_STATUS:available".to_owned());
        }
        if first_failed_boundary.is_none() {
            first_failed_boundary = Some("schema".to_owned());
        }
    }

    if is_task {
        if owner.is_none() {
            classification = MappingClass::Unknown;
            if exact_error.is_none() {
                exact_error = Some("MISSING_OWNER".to_owned());
            }
            if first_failed_boundary.is_none() {
                first_failed_boundary = Some("ownership".to_owned());
            }
        } else if let Some(canonical_cwd) = options
            .canonical_project_cwd
            .as_deref()
            .filter(|cwd| cwd.is_absolute())
        {
            if let Some(record_cwd) = cwd.as_deref() {
                if !cwd_matches_project(canonical_cwd, record_cwd) {
                    classification = MappingClass::Unknown;
                    exact_error = Some(scope_error(
                        "CWD_OUTSIDE_PROJECT_SCOPE",
                        canonical_cwd,
                        record_cwd,
                    ));
                    first_failed_boundary = Some("scope".to_owned());
                }
            }
            if let Some(record_worktree) = worktree.as_deref() {
                if !worktree_matches_project(canonical_cwd, record_worktree) {
                    classification = MappingClass::Unknown;
                    exact_error = Some(scope_error(
                        "WORKTREE_OUTSIDE_PROJECT_SCOPE",
                        canonical_cwd,
                        record_worktree,
                    ));
                    first_failed_boundary = Some("binding".to_owned());
                }
            }
        }

        if owner.is_some() && is_active_status(status.as_deref()) {
            // The canonical context validates the record; it never supplies
            // missing binding evidence from the source record.
            let has_cwd = cwd.is_some();
            if worktree.is_none() || base_commit.is_none() || !has_cwd {
                classification = classification.combine(MappingClass::Reset);
                if exact_error.is_none() {
                    exact_error = Some(missing_binding_error(
                        worktree.is_some(),
                        base_commit.is_some(),
                        has_cwd,
                    ));
                }
                if first_failed_boundary.is_none() {
                    first_failed_boundary = Some("binding".to_owned());
                }
            }
        } else if is_terminal_status(status.as_deref())
            && (worktree.is_none() || base_commit.is_none() || cwd.is_none())
        {
            classification = classification.combine(MappingClass::Adapt);
            if exact_error.is_none() {
                exact_error = Some("LEGACY_BINDING_EVIDENCE_ABSENT".to_owned());
            }
            if first_failed_boundary.is_none() {
                first_failed_boundary = Some("binding".to_owned());
            }
        }
    } else if let Some(canonical_cwd) = options
        .canonical_project_cwd
        .as_deref()
        .filter(|cwd| cwd.is_absolute())
    {
        if let Some(record_cwd) = cwd.as_deref() {
            if !cwd_matches_project(canonical_cwd, record_cwd) {
                classification = MappingClass::Unknown;
                exact_error = Some(scope_error(
                    "CWD_OUTSIDE_PROJECT_SCOPE",
                    canonical_cwd,
                    record_cwd,
                ));
                first_failed_boundary = Some("scope".to_owned());
            }
        }
        if let Some(record_worktree) = worktree.as_deref() {
            if !worktree_matches_project(canonical_cwd, record_worktree) {
                classification = MappingClass::Unknown;
                exact_error = Some(scope_error(
                    "WORKTREE_OUTSIDE_PROJECT_SCOPE",
                    canonical_cwd,
                    record_worktree,
                ));
                first_failed_boundary = Some("binding".to_owned());
            }
        }
    }

    RecordInspection {
        line_number,
        digest: digest_bytes(&raw_bytes),
        raw_bytes,
        record_id,
        sequence,
        entity_id,
        event,
        owner,
        cwd,
        worktree,
        base_commit,
        status,
        classification,
        source_disposition: SourceDisposition::from_mapping_class(classification),
        exact_error,
        first_failed_boundary,
    }
}

fn mark_duplicate_record_ids(records: &mut [RecordInspection], issues: &mut Vec<HistoryIssue>) {
    let mut seen = BTreeMap::<String, usize>::new();
    for index in 0..records.len() {
        let Some(record_id) = records[index].record_id.clone() else {
            continue;
        };
        if let Some(previous_index) = seen.insert(record_id.clone(), index) {
            let error = format!("DUPLICATE_RECORD_ID:{record_id}");
            mark_record(
                &mut records[previous_index],
                MappingClass::Unknown,
                &error,
                "replay",
            );
            mark_record(&mut records[index], MappingClass::Unknown, &error, "replay");
            issues.push(issue(
                Some(records[index].line_number),
                MappingClass::Unknown,
                error,
                "replay",
            ));
        }
    }
}

fn mark_duplicate_registrations(records: &mut [RecordInspection], issues: &mut Vec<HistoryIssue>) {
    let mut seen = BTreeMap::<String, usize>::new();
    for index in 0..records.len() {
        if records[index].event.as_deref() != Some("Registered") {
            continue;
        }
        let Some(entity_id) = records[index].entity_id.clone() else {
            continue;
        };
        if let Some(previous_index) = seen.insert(entity_id.clone(), index) {
            let error = format!("DUPLICATE_REGISTRATION:{entity_id}");
            mark_record(
                &mut records[previous_index],
                MappingClass::Reset,
                &error,
                "identity",
            );
            mark_record(&mut records[index], MappingClass::Reset, &error, "identity");
            issues.push(issue(
                Some(records[index].line_number),
                MappingClass::Reset,
                error,
                "identity",
            ));
        }
    }
}

fn mark_sequence_issues(records: &mut [RecordInspection], issues: &mut Vec<HistoryIssue>) {
    let has_sequence = records.iter().any(|record| record.sequence.is_some());
    if !has_sequence {
        return;
    }

    let mut expected = None;
    let mut exhausted = false;
    for record in records {
        if is_parse_invalid(record) {
            continue;
        }
        if exhausted {
            mark_record(record, MappingClass::Unknown, "SEQUENCE_OVERFLOW", "replay");
            issues.push(issue(
                Some(record.line_number),
                MappingClass::Unknown,
                "SEQUENCE_OVERFLOW",
                "replay",
            ));
            continue;
        }
        let Some(sequence) = record.sequence else {
            mark_record(record, MappingClass::Unknown, "MISSING_SEQUENCE", "replay");
            issues.push(issue(
                Some(record.line_number),
                MappingClass::Unknown,
                "MISSING_SEQUENCE",
                "replay",
            ));
            continue;
        };
        let Some(next) = expected else {
            expected = sequence.checked_add(1);
            if expected.is_none() {
                exhausted = true;
                mark_record(record, MappingClass::Unknown, "SEQUENCE_OVERFLOW", "replay");
                issues.push(issue(
                    Some(record.line_number),
                    MappingClass::Unknown,
                    "SEQUENCE_OVERFLOW",
                    "replay",
                ));
            }
            continue;
        };
        if sequence != next {
            let error = if sequence > next {
                format!("SEQUENCE_GAP:expected={next}:observed={sequence}")
            } else {
                format!("SEQUENCE_NON_MONOTONIC:expected={next}:observed={sequence}")
            };
            mark_record(record, MappingClass::Unknown, &error, "replay");
            issues.push(issue(
                Some(record.line_number),
                MappingClass::Unknown,
                error,
                "replay",
            ));
        }
        expected = sequence.checked_add(1);
        if expected.is_none() {
            exhausted = true;
            mark_record(record, MappingClass::Unknown, "SEQUENCE_OVERFLOW", "replay");
            issues.push(issue(
                Some(record.line_number),
                MappingClass::Unknown,
                "SEQUENCE_OVERFLOW",
                "replay",
            ));
        }
    }
}

fn invalid_record(
    line_number: usize,
    raw_bytes: Vec<u8>,
    exact_error: impl Into<String>,
    first_failed_boundary: impl Into<String>,
) -> RecordInspection {
    RecordInspection {
        line_number,
        digest: digest_bytes(&raw_bytes),
        raw_bytes,
        record_id: None,
        sequence: None,
        entity_id: None,
        event: None,
        owner: None,
        cwd: None,
        worktree: None,
        base_commit: None,
        status: None,
        classification: MappingClass::Unknown,
        source_disposition: SourceDisposition::ArchiveOnly,
        exact_error: Some(exact_error.into()),
        first_failed_boundary: Some(first_failed_boundary.into()),
    }
}

fn is_parse_invalid(record: &RecordInspection) -> bool {
    record
        .exact_error
        .as_deref()
        .is_some_and(|error| error == "EMPTY_LINE" || error.starts_with("MALFORMED_JSON_"))
}

fn event_schema_error(raw_content: &[u8], value: &Value) -> Option<String> {
    if let Some(error) = duplicate_json_key_error(raw_content) {
        return Some(error);
    }
    let Some(event_value) = value.get("ev") else {
        return Some("EVENT_ABSENT".to_owned());
    };
    let event_name = event_value.as_str();
    // Validate the canonical event against the original bytes.  Parsing the
    // Value first is still useful for extracting legacy fields, but Value
    // collapses duplicate object keys and therefore cannot be the schema
    // authority for a migration decision.
    match serde_json::from_slice::<Event>(raw_content) {
        Ok(_) => None,
        Err(error) => {
            let detail = error.to_string();
            if let Some(event_name) = event_name {
                if detail.contains("unknown variant") {
                    Some(format!("UNKNOWN_EVENT:{event_name}"))
                } else {
                    Some(format!("INVALID_EVENT:{event_name}:{detail}"))
                }
            } else {
                Some(format!("INVALID_EVENT:ev:{detail}"))
            }
        }
    }
}

fn duplicate_json_key_error(raw_content: &[u8]) -> Option<String> {
    let mut deserializer = serde_json::Deserializer::from_slice(raw_content);
    match deserializer.deserialize_any(DuplicateKeyVisitor) {
        Ok(()) => None,
        Err(error) => {
            let detail = error.to_string();
            let stable_detail = detail
                .split(" at line ")
                .next()
                .unwrap_or(detail.as_str())
                .to_owned();
            if stable_detail.starts_with("DUPLICATE_JSON_KEY:") {
                Some(stable_detail)
            } else {
                Some(format!("DUPLICATE_KEY_SCAN_FAILED:{stable_detail}"))
            }
        }
    }
}

struct DuplicateKeyVisitor;

impl<'de> Visitor<'de> for DuplicateKeyVisitor {
    type Value = ();

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("any JSON value")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut keys = BTreeMap::new();
        while let Some(key) = map.next_key::<String>()? {
            if keys.insert(key.clone(), ()).is_some() {
                return Err(serde::de::Error::custom(format!(
                    "DUPLICATE_JSON_KEY:{key}"
                )));
            }
            map.next_value_seed(DuplicateValueSeed)?;
        }
        Ok(())
    }

    fn visit_seq<A>(self, mut sequence: A) -> Result<Self::Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        while sequence.next_element_seed(DuplicateValueSeed)?.is_some() {}
        Ok(())
    }

    fn visit_bool<E>(self, _value: bool) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        Ok(())
    }

    fn visit_i64<E>(self, _value: i64) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        Ok(())
    }

    fn visit_u64<E>(self, _value: u64) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        Ok(())
    }

    fn visit_f64<E>(self, _value: f64) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        Ok(())
    }

    fn visit_str<E>(self, _value: &str) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        Ok(())
    }

    fn visit_string<E>(self, _value: String) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        Ok(())
    }

    fn visit_unit<E>(self) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        Ok(())
    }
}

struct DuplicateValueSeed;

impl<'de> DeserializeSeed<'de> for DuplicateValueSeed {
    type Value = ();

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_any(DuplicateKeyVisitor)
    }
}

fn is_known_task_status(status: &str) -> bool {
    matches!(
        status,
        "available"
            | "assigned"
            | "working"
            | "blocked"
            | "waiting"
            | "verifying"
            | "reviewed"
            | "delivered"
            | "accepted"
            | "rework"
            | "merged"
            | "closed"
            | "cancelled"
    )
}

fn cwd_matches_project(canonical_cwd: &Path, record_cwd: &str) -> bool {
    let record_cwd = Path::new(record_cwd);
    record_cwd.is_absolute() && normalize_path(record_cwd) == normalize_path(canonical_cwd)
}

fn worktree_matches_project(canonical_cwd: &Path, record_worktree: &str) -> bool {
    let raw = Path::new(record_worktree);
    if raw
        .components()
        .any(|component| matches!(component, std::path::Component::ParentDir))
    {
        return false;
    }
    let candidate = if raw.is_absolute() {
        raw.to_path_buf()
    } else {
        let relative = raw.strip_prefix(Path::new("./")).unwrap_or(raw);
        canonical_cwd.join(relative)
    };
    let candidate = normalize_path(&candidate);
    let playground = normalize_path(&canonical_cwd.join("playground"));
    candidate.starts_with(&playground) && candidate != playground
}

fn normalize_path(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                normalized.pop();
            }
            _ => normalized.push(component.as_os_str()),
        }
    }
    normalized
}

fn scope_error(kind: &str, canonical_cwd: &Path, observed: &str) -> String {
    format!(
        "{kind}:expected={}:observed={observed}",
        canonical_cwd.display()
    )
}

fn mark_record(record: &mut RecordInspection, class: MappingClass, error: &str, boundary: &str) {
    record.classification = record.classification.combine(class);
    if record.exact_error.is_none() {
        record.exact_error = Some(error.to_owned());
    }
    if record.first_failed_boundary.is_none() {
        record.first_failed_boundary = Some(boundary.to_owned());
    }
}

fn issue(
    line_number: Option<usize>,
    classification: MappingClass,
    exact_error: impl Into<String>,
    first_failed_boundary: impl Into<String>,
) -> HistoryIssue {
    HistoryIssue {
        line_number,
        classification,
        exact_error: exact_error.into(),
        first_failed_boundary: first_failed_boundary.into(),
    }
}

fn missing_binding_error(has_worktree: bool, has_base: bool, has_cwd: bool) -> String {
    let mut missing = Vec::new();
    if !has_worktree {
        missing.push("worktree");
    }
    if !has_base {
        missing.push("base_commit");
    }
    if !has_cwd {
        missing.push("cwd");
    }
    format!("MISSING_BINDING_EVIDENCE:{}", missing.join(","))
}

fn is_active_status(status: Option<&str>) -> bool {
    status.is_none_or(|status| {
        matches!(
            status,
            "assigned"
                | "working"
                | "blocked"
                | "waiting"
                | "verifying"
                | "reviewed"
                | "delivered"
                | "accepted"
                | "rework"
                | "cleanup_pending"
        )
    })
}

fn is_terminal_status(status: Option<&str>) -> bool {
    status.is_some_and(|status| matches!(status, "closed" | "merged" | "cancelled"))
}

pub(crate) fn trim_line_ending(raw_bytes: &[u8]) -> &[u8] {
    let without_lf = raw_bytes.strip_suffix(b"\n").unwrap_or(raw_bytes);
    without_lf.strip_suffix(b"\r").unwrap_or(without_lf)
}

fn string_at(value: &Value, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|key| value.get(*key).and_then(as_non_empty_string))
}

fn integer_at(value: &Value, keys: &[&str]) -> Option<u64> {
    keys.iter().find_map(|key| {
        value.get(*key).and_then(|candidate| {
            candidate
                .as_u64()
                .or_else(|| candidate.as_str().and_then(|text| text.parse().ok()))
        })
    })
}

fn first_nested_string(value: &Value, paths: &[(&[&str], &[&str])]) -> Option<String> {
    paths.iter().find_map(|(parents, keys)| {
        parents.iter().find_map(|parent| {
            value
                .get(*parent)
                .and_then(|nested| string_at(nested, keys))
        })
    })
}

fn as_non_empty_string(value: &Value) -> Option<String> {
    value
        .as_str()
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(ToOwned::to_owned)
}
