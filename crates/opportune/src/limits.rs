//! Configurable caps on request size.
//!
//! Every limit bounds the work one request can demand: participants, their blocks
//! and their zone periods bound the input, the search window and candidate starts
//! bound the space the engine explores, and the `explain.limit` maximum bounds one
//! explanation's detail. Exceeding a limit in a request is a validation error;
//! reaching the `explain.limit` during evaluation only truncates that explanation.
//!
//! [`Limits::default()`] carries the engine's published defaults. The engine
//! reads these caps as part of request validation, before any structural or
//! matching work runs; callers who need larger inputs supply their own
//! [`Limits`] rather than relying on the defaults.
//!
//! [`Limits::max_duration_minutes`] is the one cap on a *duration* rather than a
//! count: a booking, a booking or block's travel and a clearance each have one,
//! and a duration past the cap would widen the range a request is judged over to
//! a size no search can pay for. It is measured in exact minutes, so seven days
//! is `7 * 24 * 60`.

/// Configurable caps on request size.
///
/// Construct with [`Limits::default()`] and raise the caps a request needs with
/// the `with_*` methods:
///
/// ```
/// use opportune::Limits;
///
/// let limits = Limits::default().with_max_participants(50);
/// assert_eq!(limits.max_participants, 50);
/// assert_eq!(limits.max_search_window_days, 92);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct Limits {
    /// Maximum number of participants in one request. Default: 500.
    pub max_participants: usize,
    /// Maximum number of blocks per participant. Default: 10,000.
    pub max_blocks_per_participant: usize,
    /// Maximum number of zone periods one participant may carry. Default: 1,000.
    ///
    /// Every period is resolved into the schedule a booking is judged against, so
    /// an unbounded list would be work one request can demand in a single field.
    pub max_zone_periods_per_participant: usize,
    /// Maximum search-window length in days. Default: 92.
    pub max_search_window_days: u32,
    /// Maximum number of candidate starts in one search. Default: 100,000.
    pub max_candidate_starts: usize,
    /// Maximum `explain.limit` a request may ask for. Default: 10,000.
    pub max_explain_limit: usize,
    /// Maximum number of rules in one request. Default: 500.
    pub max_rules: usize,
    /// Maximum number of occurrences in one request. Default: 50,000.
    pub max_occurrences: usize,
    /// Maximum number of host slots a requirement may ask for, across every
    /// term, and the number of terms it may name. Default: 64.
    pub max_requirement_slots: usize,
    /// Maximum length of a booking, a booking or block's travel and a clearance,
    /// in minutes. Default: 10,080 (seven days).
    pub max_duration_minutes: u32,
}

impl Default for Limits {
    /// The engine's published defaults.
    fn default() -> Self {
        Self {
            max_participants: 500,
            max_blocks_per_participant: 10_000,
            max_zone_periods_per_participant: 1_000,
            max_search_window_days: 92,
            max_candidate_starts: 100_000,
            max_explain_limit: 10_000,
            max_rules: 500,
            max_occurrences: 50_000,
            max_requirement_slots: 64,
            max_duration_minutes: 7 * 24 * 60,
        }
    }
}

impl Limits {
    /// The limits with the participant cap replaced.
    #[must_use]
    pub const fn with_max_participants(mut self, max: usize) -> Self {
        self.max_participants = max;
        self
    }

    /// The limits with the per-participant block cap replaced.
    #[must_use]
    pub const fn with_max_blocks_per_participant(mut self, max: usize) -> Self {
        self.max_blocks_per_participant = max;
        self
    }

    /// The limits with the per-participant zone-period cap replaced.
    #[must_use]
    pub const fn with_max_zone_periods_per_participant(mut self, max: usize) -> Self {
        self.max_zone_periods_per_participant = max;
        self
    }

    /// The limits with the search-window cap replaced.
    #[must_use]
    pub const fn with_max_search_window_days(mut self, days: u32) -> Self {
        self.max_search_window_days = days;
        self
    }

    /// The limits with the candidate-start cap replaced.
    #[must_use]
    pub const fn with_max_candidate_starts(mut self, max: usize) -> Self {
        self.max_candidate_starts = max;
        self
    }

    /// The limits with the `explain.limit` cap replaced.
    #[must_use]
    pub const fn with_max_explain_limit(mut self, max: usize) -> Self {
        self.max_explain_limit = max;
        self
    }

    /// The limits with the rule-count cap replaced.
    #[must_use]
    pub const fn with_max_rules(mut self, max: usize) -> Self {
        self.max_rules = max;
        self
    }

    /// The limits with the occurrence-count cap replaced.
    #[must_use]
    pub const fn with_max_occurrences(mut self, max: usize) -> Self {
        self.max_occurrences = max;
        self
    }

    /// The limits with the requirement-slot cap replaced.
    #[must_use]
    pub const fn with_max_requirement_slots(mut self, max: usize) -> Self {
        self.max_requirement_slots = max;
        self
    }

    /// The limits with the duration cap replaced.
    #[must_use]
    pub const fn with_max_duration_minutes(mut self, minutes: u32) -> Self {
        self.max_duration_minutes = minutes;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::Limits;

    #[test]
    fn defaults_match_the_published_limits() {
        let limits = Limits::default();
        assert_eq!(limits.max_participants, 500);
        assert_eq!(limits.max_blocks_per_participant, 10_000);
        assert_eq!(limits.max_zone_periods_per_participant, 1_000);
        assert_eq!(limits.max_search_window_days, 92);
        assert_eq!(limits.max_candidate_starts, 100_000);
        assert_eq!(limits.max_explain_limit, 10_000);
        assert_eq!(limits.max_rules, 500);
        assert_eq!(limits.max_occurrences, 50_000);
        assert_eq!(limits.max_requirement_slots, 64);
        assert_eq!(limits.max_duration_minutes, 7 * 24 * 60);
    }
}
