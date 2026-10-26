//! Time eligibility for CEO progress reports.
//!
//! The caller supplies the current time and the last time a report actually
//! reached the user. This module only answers whether a nonurgent report may be
//! emitted now, and when it next may be. Deduplication, pending batches, queueing
//! and acknowledgements live in the runtime owner.
//!
//! Urgency is an explicit typed input. Report subject and body text are never
//! inspected here.

use std::fmt;

/// Default minimum spacing between two reports to the user.
pub const DEFAULT_MIN_INTERVAL_MS: u64 = 3_600_000;
/// Beijing is a fixed UTC+8 offset with no daylight saving time.
pub const BEIJING_UTC_OFFSET_MS: u64 = 28_800_000;
/// Default quiet window start, local minute of day (00:00).
pub const DEFAULT_QUIET_START_MINUTE: u32 = 0;
/// Default quiet window end, local minute of day (08:00).
pub const DEFAULT_QUIET_END_MINUTE: u32 = 480;

const MS_PER_MINUTE: u64 = 60_000;
const MS_PER_DAY: u64 = 86_400_000;
const MINUTES_PER_DAY: u32 = 1_440;

/// Why a nonurgent report is not allowed at the evaluated instant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CeoReportSuppression {
    /// A report may be emitted now.
    Allowed,
    /// The minimum interval since the last actual report has not elapsed.
    Cooldown,
    /// The current local time is inside the quiet window.
    QuietHours,
}

/// Outcome of one time-eligibility evaluation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CeoReportDecision {
    /// Whether a nonurgent report may be emitted at `now_ms`.
    pub allowed: bool,
    /// Reason for the outcome.
    pub suppression: CeoReportSuppression,
    /// Earliest instant that satisfies both the interval and the quiet window.
    /// Equals `now_ms` when `allowed` is true.
    pub next_eligible_ms: u64,
}

/// Invalid policy configuration supplied by the caller.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CeoReportPolicyError {
    /// A quiet window boundary is not a minute of day in `0..1440`.
    QuietMinuteOutOfRange { minute: u32 },
}

impl fmt::Display for CeoReportPolicyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::QuietMinuteOutOfRange { minute } => write!(
                f,
                "quiet window minute {minute} is outside 0..{MINUTES_PER_DAY}"
            ),
        }
    }
}

impl std::error::Error for CeoReportPolicyError {}

/// Time limits for CEO reports: a minimum interval plus a local quiet window.
///
/// The quiet window is `[start, end)` in Beijing local time. `start < end` is an
/// ordinary window; `start > end` wraps past local midnight; `start == end`
/// disables the quiet window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CeoReportPolicy {
    min_interval_ms: u64,
    quiet_start_minute: u32,
    quiet_end_minute: u32,
}

impl Default for CeoReportPolicy {
    fn default() -> Self {
        Self::beijing_default()
    }
}

impl CeoReportPolicy {
    /// Hourly interval, quiet `[00:00, 08:00)` Beijing.
    pub const fn beijing_default() -> Self {
        Self {
            min_interval_ms: DEFAULT_MIN_INTERVAL_MS,
            quiet_start_minute: DEFAULT_QUIET_START_MINUTE,
            quiet_end_minute: DEFAULT_QUIET_END_MINUTE,
        }
    }

    /// Replace the quiet window boundaries with caller-supplied minutes of day.
    pub fn with_quiet_window(
        self,
        start_minute: u32,
        end_minute: u32,
    ) -> Result<Self, CeoReportPolicyError> {
        for minute in [start_minute, end_minute] {
            if minute >= MINUTES_PER_DAY {
                return Err(CeoReportPolicyError::QuietMinuteOutOfRange { minute });
            }
        }
        Ok(Self {
            quiet_start_minute: start_minute,
            quiet_end_minute: end_minute,
            ..self
        })
    }

    pub const fn min_interval_ms(&self) -> u64 {
        self.min_interval_ms
    }

    /// Configured quiet window as `(start_minute, end_minute)`.
    pub const fn quiet_window(&self) -> (u32, u32) {
        (self.quiet_start_minute, self.quiet_end_minute)
    }

    /// Evaluate one instant.
    ///
    /// `last_actual_report_ms` is the last time a report actually reached the
    /// user, or `None` when no report has been made yet. `typed_urgent` is the
    /// explicit urgency signal; it bypasses both ordinary time limits.
    pub fn evaluate(
        &self,
        now_ms: u64,
        last_actual_report_ms: Option<u64>,
        typed_urgent: bool,
    ) -> CeoReportDecision {
        if typed_urgent {
            return CeoReportDecision {
                allowed: true,
                suppression: CeoReportSuppression::Allowed,
                next_eligible_ms: now_ms,
            };
        }

        let cooldown_deadline = last_actual_report_ms.map(|last| last + self.min_interval_ms);
        let candidate_ms = match cooldown_deadline {
            Some(deadline) if deadline > now_ms => deadline,
            _ => now_ms,
        };

        let now_is_quiet = self.is_quiet_local(now_ms);
        let next_eligible_ms = if self.is_quiet_local(candidate_ms) {
            self.next_quiet_end_ms(candidate_ms)
        } else {
            candidate_ms
        };

        if now_is_quiet {
            CeoReportDecision {
                allowed: false,
                suppression: CeoReportSuppression::QuietHours,
                next_eligible_ms,
            }
        } else if candidate_ms > now_ms {
            CeoReportDecision {
                allowed: false,
                suppression: CeoReportSuppression::Cooldown,
                next_eligible_ms,
            }
        } else {
            CeoReportDecision {
                allowed: true,
                suppression: CeoReportSuppression::Allowed,
                next_eligible_ms: now_ms,
            }
        }
    }

    fn is_quiet_local(&self, at_ms: u64) -> bool {
        let (start, end) = self.quiet_window();
        if start == end {
            return false;
        }
        let minute = local_minute_of_day(at_ms);
        if start < end {
            minute >= start && minute < end
        } else {
            minute >= start || minute < end
        }
    }

    /// Next quiet window end strictly after `at_ms`, which must be quiet.
    fn next_quiet_end_ms(&self, at_ms: u64) -> u64 {
        let local_ms = at_ms + BEIJING_UTC_OFFSET_MS;
        let day_start_ms = local_ms - local_ms % MS_PER_DAY;
        let minute = local_minute_of_day(at_ms);
        let end_ms = self.quiet_end_minute as u64 * MS_PER_MINUTE;
        let end_local_ms = if self.quiet_end_minute > minute {
            day_start_ms + end_ms
        } else {
            day_start_ms + MS_PER_DAY + end_ms
        };
        end_local_ms - BEIJING_UTC_OFFSET_MS
    }
}

fn local_minute_of_day(at_ms: u64) -> u32 {
    let local_ms = at_ms + BEIJING_UTC_OFFSET_MS;
    ((local_ms % MS_PER_DAY) / MS_PER_MINUTE) as u32
}
