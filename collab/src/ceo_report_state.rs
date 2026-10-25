//! Canonical CEO progress coalescing and report acknowledgement state.
//!
//! This module keeps the minimum business state needed to deduplicate project
//! progress by project, prepare a caller-supplied report batch, and advance only
//! revisions covered by an explicit acknowledgement after actual host output.
//! The runtime owner stores this aggregate as one canonical CEO state record and
//! wires authentication, policy, queue and native delivery around it.
//!
//! Progress identity is business content only. Arrival time, message IDs and
//! correlation bookkeeping are not accepted here and therefore cannot affect
//! deduplication. There are no hashes, keyword classifiers, second queues,
//! board-history copies or external I/O. An acknowledgement is a caller
//! declaration after visible host output; this module does not independently
//! prove user delivery or reading. Time eligibility is a separate policy helper.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Substantive progress reported for one project.
///
/// Equality of this value is the dedup key. Arrival time, message ID and reply
/// correlation are deliberately absent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CeoProgressContent {
    /// Progress subject, trimmed on record.
    pub subject: String,
    /// Progress body, trimmed on record. Body text is never classified here.
    pub body: String,
}

impl CeoProgressContent {
    /// Build normalized content. Edge whitespace only is trimmed.
    pub fn new(subject: impl Into<String>, body: impl Into<String>) -> Self {
        Self {
            subject: subject.into().trim().to_owned(),
            body: body.into().trim().to_owned(),
        }
    }

    fn normalized(self) -> Self {
        Self::new(self.subject, self.body)
    }
}

/// Latest known progress for one project.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CeoProjectProgress {
    /// Per-project revision starts at 1 and advances only on meaningful business
    /// change or a new explicit typed urgent signal for equal content.
    pub revision: u64,
    /// Latest business content.
    pub content: CeoProgressContent,
    /// Explicit typed urgent marker for this revision. Never inferred from text.
    pub urgent: bool,
    /// Revision already covered by a successful report acknowledgement. `0`
    /// means no revision has been acknowledged. Revisions are never reused, so a
    /// later update always remains pending after an older acknowledgement.
    #[serde(default)]
    pub reported_revision: u64,
}

/// Project-tagged pending revision returned by read-only queries.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CeoPendingRevision {
    pub project: String,
    pub revision: u64,
    pub content: CeoProgressContent,
    pub urgent: bool,
}

/// One prepared report batch. Only project revision coverage is retained; the
/// report body itself belongs to the caller.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CeoReportBatch {
    /// Caller-supplied batch identifier.
    pub batch_id: String,
    /// Project revision coverage included in this report.
    pub coverage: BTreeMap<String, u64>,
    /// Whether this batch was prepared as an explicit urgent report.
    pub urgent: bool,
}

/// Canonical CEO progress and report-batch state.
///
/// Persist this whole value. It contains one latest progress record per tracked
/// project and only unacknowledged batches, so it stays bounded by tracked
/// projects and active work rather than by update history.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CeoReportState {
    /// Latest known progress per project, including already reported revisions.
    #[serde(default)]
    pub projects: BTreeMap<String, CeoProjectProgress>,
    /// Unacknowledged prepared batches keyed by batch ID.
    #[serde(default)]
    pub batches: BTreeMap<String, CeoReportBatch>,
    /// Last actual-report timestamp supplied by the caller on successful ack.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_actual_report_ms: Option<u64>,
    /// Durable generation counter for meaningful typed urgent updates. Observers
    /// use this to detect a new actual urgent signal without replaying content.
    /// It advances only on a meaningful typed urgent revision and never on
    /// acknowledgement, read, ordinary progress or a duplicate typed urgent.
    #[serde(default)]
    pub urgent_generation: u64,
}

impl CeoReportState {
    /// Create empty state.
    pub fn new() -> Self {
        Self::default()
    }

    /// Record the latest business progress for one project.
    ///
    /// Equal business content is a no-op unless a new explicit typed urgent
    /// signal upgrades the same content. The return value says whether a
    /// meaningful revision was recorded. Body text never implies urgency.
    pub fn record_progress(
        &mut self,
        project: impl Into<String>,
        content: CeoProgressContent,
        typed_urgent: bool,
    ) -> bool {
        let project = project.into();
        let content = content.normalized();
        let (same_content, existing_urgent) = match self.projects.get(&project) {
            Some(existing) => (existing.content == content, existing.urgent),
            None => (false, false),
        };
        let urgent_upgrade = same_content && typed_urgent && !existing_urgent;
        if same_content && !urgent_upgrade {
            return false;
        }

        let revision = self
            .projects
            .get(&project)
            .map_or(0, |existing| existing.revision)
            + 1;
        let urgent = if same_content { true } else { typed_urgent };
        match self.projects.get_mut(&project) {
            Some(existing) => {
                existing.revision = revision;
                existing.content = content;
                existing.urgent = urgent;
            }
            None => {
                self.projects.insert(
                    project,
                    CeoProjectProgress {
                        revision,
                        content,
                        urgent,
                        reported_revision: 0,
                    },
                );
            }
        }
        // urgent_generation tracks only meaningful typed urgent updates: a new
        // revision caused by the typed urgent signal (new content or an urgent
        // upgrade of equal content). A duplicate typed urgent that does not
        // change business state is a no-op above and never reaches this point.
        if urgent {
            self.urgent_generation += 1;
        }
        true
    }

    /// Latest pending revision for every project.
    pub fn pending_revisions(&self) -> Vec<CeoPendingRevision> {
        self.projects
            .iter()
            .filter(|(_, progress)| progress.revision > progress.reported_revision)
            .map(|(project, progress)| CeoPendingRevision {
                project: project.clone(),
                revision: progress.revision,
                content: progress.content.clone(),
                urgent: progress.urgent,
            })
            .collect()
    }

    /// Latest pending revision for one project.
    pub fn pending_revision(&self, project: &str) -> Option<CeoPendingRevision> {
        self.projects
            .get(project)
            .filter(|progress| progress.revision > progress.reported_revision)
            .map(|progress| CeoPendingRevision {
                project: project.to_owned(),
                revision: progress.revision,
                content: progress.content.clone(),
                urgent: progress.urgent,
            })
    }

    /// Whether any pending latest revision carries an explicit typed urgent mark.
    pub fn has_pending_urgent(&self) -> bool {
        self.projects
            .values()
            .any(|progress| progress.urgent && progress.revision > progress.reported_revision)
    }

    /// Pending latest revisions carrying an explicit typed urgent mark.
    pub fn pending_urgent_revisions(&self) -> Vec<CeoPendingRevision> {
        self.pending_revisions()
            .into_iter()
            .filter(|revision| revision.urgent)
            .collect()
    }

    /// Prepare a report batch without marking anything reported.
    ///
    /// The caller supplies the batch ID and project revision coverage. Preparing
    /// or reading never advances `last_actual_report_ms` and never marks a
    /// revision reported. Repeating the same pending batch content returns the
    /// existing batch rather than creating a duplicate. A batch ID reused with
    /// different coverage, or with an already acknowledged batch, is rejected.
    pub fn prepare_batch(
        &mut self,
        batch_id: impl Into<String>,
        coverage: BTreeMap<String, u64>,
        urgent: bool,
    ) -> Result<CeoReportBatch, CeoReportStateError> {
        let batch_id = batch_id.into();
        if coverage.is_empty() {
            return Err(CeoReportStateError::EmptyCoverage { batch_id });
        }
        if let Some(existing) = self.batches.get(&batch_id) {
            if existing.coverage == coverage && existing.urgent == urgent {
                return Ok(existing.clone());
            }
            return Err(CeoReportStateError::BatchConflict { batch_id });
        }
        if let Some(existing) = self
            .batches
            .values()
            .find(|batch| batch.coverage == coverage && batch.urgent == urgent)
        {
            return Ok(existing.clone());
        }
        let batch = CeoReportBatch {
            batch_id: batch_id.clone(),
            coverage,
            urgent,
        };
        self.batches.insert(batch_id, batch.clone());
        Ok(batch)
    }

    /// Explicitly acknowledge a prepared batch after actual host output.
    ///
    /// An acknowledgement advances the maximum acknowledged covered revision for
    /// each project, bounded by that project's current revision:
    /// `reported_revision = max(reported_revision, min(covered, revision))`.
    /// Acknowledging an older batch after a newer revision still advances the
    /// older watermark without regressing the newer revision, which stays
    /// pending. The latest canonical `urgent` marker is never cleared by an
    /// acknowledgement. After advancing, only batches fully covered by the new
    /// reported watermarks are pruned; preparing, reading and recording never
    /// invalidate a still-valid batch. On the first successful acknowledgement
    /// the caller-supplied timestamp becomes `last_actual_report_ms`. A
    /// subsequent acknowledgement of a removed batch is an exact rejection and
    /// cannot move the timestamp again or clear later progress.
    pub fn acknowledge_batch(
        &mut self,
        batch_id: &str,
        reported_ms: u64,
    ) -> Result<CeoAckResult, CeoReportStateError> {
        let Some(batch) = self.batches.remove(batch_id) else {
            return Err(CeoReportStateError::UnknownBatch {
                batch_id: batch_id.to_owned(),
            });
        };

        let mut advanced_revisions = 0;
        for (project, revision) in batch.coverage {
            let Some(progress) = self.projects.get_mut(&project) else {
                continue;
            };
            let covered = revision.min(progress.revision);
            let new_reported = progress.reported_revision.max(covered);
            if new_reported > progress.reported_revision {
                progress.reported_revision = new_reported;
                advanced_revisions += 1;
            }
            // The latest canonical urgent marker is never cleared by ack.
        }

        // Only ACK may prune. Retain every batch that still contains at least one
        // project revision above its reported watermark.
        let reported: BTreeMap<&String, u64> = self
            .projects
            .iter()
            .map(|(project, progress)| (project, progress.reported_revision))
            .collect();
        self.batches.retain(|_, kept| {
            kept.coverage
                .iter()
                .any(|(project, revision)| reported.get(project).copied().unwrap_or(0) < *revision)
        });

        self.last_actual_report_ms = Some(reported_ms);
        Ok(CeoAckResult::Advanced { advanced_revisions })
    }

    /// Last actual-report timestamp supplied by the caller.
    pub fn last_actual_report_ms(&self) -> Option<u64> {
        self.last_actual_report_ms
    }

    /// Number of unacknowledged prepared batches.
    pub fn batch_count(&self) -> usize {
        self.batches.len()
    }
}

/// Outcome of one successful acknowledgement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CeoAckResult {
    /// The batch was acknowledged and advanced the stated number of covered
    /// revisions that still matched their recorded revision.
    Advanced { advanced_revisions: usize },
}

/// Rejection reasons for batch preparation and acknowledgement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CeoReportStateError {
    /// A report batch must cover at least one project revision.
    EmptyCoverage { batch_id: String },
    /// The batch ID already exists with different coverage or urgency.
    BatchConflict { batch_id: String },
    /// No unacknowledged batch exists under the supplied batch ID.
    UnknownBatch { batch_id: String },
}

impl std::fmt::Display for CeoReportStateError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CeoReportStateError::EmptyCoverage { batch_id } => write!(
                formatter,
                "batch {batch_id} has no project revision coverage"
            ),
            CeoReportStateError::BatchConflict { batch_id } => write!(
                formatter,
                "batch {batch_id} already exists with different coverage"
            ),
            CeoReportStateError::UnknownBatch { batch_id } => {
                write!(formatter, "no unacknowledged batch named {batch_id}")
            }
        }
    }
}

impl std::error::Error for CeoReportStateError {}
