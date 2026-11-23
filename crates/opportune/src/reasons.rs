//! Structured reasons why a candidate start was not offered, and why a proposed
//! booking fails `check_slot`.
//!
//! A [`Reason`] names the participant it applies to, a stable [`ReasonCode`], the
//! rule that produced the exclusion when an explicit rule applies, and — when a
//! calendar event caused it — that event's key and tags, so a block tagged
//! `veto/crm-freeze` explains itself. Every reason prints as a plain English
//! sentence for logs.
//!
//! ```
//! use opportune::reasons::{Reason, ReasonCode};
//!
//! let reason = Reason::new("ana", ReasonCode::OutsideWindow).with_rule("r_hours");
//! assert_eq!(reason.code.as_str(), "outside_window");
//! assert_eq!(reason.to_string(), "ana: outside the participant's window (rule `r_hours`)");
//! ```

use crate::model::Block;
use crate::tags::Tag;
use crate::time::Instant;

/// Why a candidate start was not offered, as a stable machine-readable code.
///
/// [`ReasonCode::ALL`] is the registry, in the canonical precedence order: a
/// participant's first failing reason is the first of these that applies. The
/// variants are declared in that same order, so the derived ordering and the
/// registry are one precedence list rather than two. The two start-level codes
/// come last because they judge the start rather than one participant.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum ReasonCode {
    /// The booking's occupied start is before `now` plus the minimum notice.
    LeadTimeMin,
    /// The booking's start is past the `lead_time` maximum that applies.
    LeadTimeMax,
    /// The booking's own span falls outside a window that applies to it.
    OutsideWindow,
    /// The booking's own span overlaps a blackout.
    Blackout,
    /// The booking's own span overlaps an event on the participant's calendar.
    Blocked,
    /// The booking's own span is clear, but its travel is not.
    Travel,
    /// The booking is too close to an existing event.
    Clearance,
    /// The booking falls inside the rest window a late finish protects.
    Rest,
    /// Adding the booking would take a cap's bucket past its limit.
    CapReached,
    /// An attendee is not available, so a booking they must attend cannot be
    /// offered.
    AttendeeUnavailable,
    /// No set of distinct eligible hosts can fill every requirement term, so the
    /// booking cannot be offered whatever the attendees' availability.
    TermUnfillable,
}

impl ReasonCode {
    /// Every code this version produces: the participant-level codes in the
    /// precedence order the response contract documents — a participant's first
    /// failing reason is the first of those that applies — followed by the two
    /// start-level codes, which judge the start rather than one participant.
    pub const ALL: [Self; 11] = [
        Self::LeadTimeMin,
        Self::LeadTimeMax,
        Self::OutsideWindow,
        Self::Blackout,
        Self::Blocked,
        Self::Travel,
        Self::Clearance,
        Self::Rest,
        Self::CapReached,
        Self::AttendeeUnavailable,
        Self::TermUnfillable,
    ];

    /// The code's wire name, stable across releases.
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::LeadTimeMin => "lead_time_min",
            Self::LeadTimeMax => "lead_time_max",
            Self::OutsideWindow => "outside_window",
            Self::Blackout => "blackout",
            Self::Blocked => "blocked",
            Self::Travel => "travel",
            Self::Clearance => "clearance",
            Self::Rest => "rest",
            Self::CapReached => "cap_reached",
            Self::AttendeeUnavailable => "attendee_unavailable",
            Self::TermUnfillable => "term_unfillable",
        }
    }

    /// The English phrase [`Reason`]'s `Display` reads for this code.
    const fn text(self) -> &'static str {
        match self {
            Self::LeadTimeMin => "starts before the minimum notice allows",
            Self::LeadTimeMax => "starts after the latest start the horizon allows",
            Self::OutsideWindow => "outside the participant's window",
            Self::Blackout => "inside a blackout",
            Self::Blocked => "blocked by an existing event",
            Self::Travel => "travel time is not clear",
            Self::Clearance => "too close to an existing event",
            Self::Rest => "too early after a late finish",
            Self::CapReached => "would take the participant past a workload cap",
            Self::AttendeeUnavailable => "an attendee the booking needs is not available",
            Self::TermUnfillable => "no distinct hosts can fill every requirement term",
        }
    }
}

impl core::fmt::Display for ReasonCode {
    /// Formats the code as its stable wire name.
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One reason a participant cannot take a candidate start.
///
/// The participant is absent for a reason about the start itself rather than
/// about one person; multi-participant bookings are what produce those.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Reason {
    /// The participant the reason applies to, or `None` for a start-level reason.
    pub participant: Option<String>,
    /// Why the start was excluded.
    pub code: ReasonCode,
    /// The rule that produced the exclusion, when an explicit rule applies. It is
    /// absent for an implicit limit and for a calendar event that blocks on its
    /// own.
    pub rule: Option<String>,
    /// The event key of the block that caused the exclusion, when a block did.
    pub event_key: Option<String>,
    /// The tags of the block that caused the exclusion, when a block did.
    pub tags: Vec<Tag>,
    /// The structured parameters a code carries, when it carries any. Only
    /// [`ReasonCode::CapReached`] does, naming the bucket and its usage.
    pub params: Option<ReasonParams>,
}

/// The structured parameters a [`Reason`] carries.
///
/// A `cap_reached` reason reports how much the bucket already holds, the cap's
/// limit, and the bucket's label, so a caller can render it without re-deriving
/// the cap. The enum leaves room for the parameters a multi-part sequence reason
/// will need without changing the wire shape of the ones already here.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum ReasonParams {
    /// A cap's bucket holds no room for the booking's contribution.
    CapReached {
        /// How much the bucket holds before the booking's contribution.
        used: u64,
        /// The cap's limit.
        limit: u32,
        /// The bucket a cap was measured over: the date its civil period begins on.
        bucket: String,
    },
}

impl Reason {
    /// A reason for one participant.
    #[must_use]
    pub fn new(participant: impl Into<String>, code: ReasonCode) -> Self {
        Self {
            participant: Some(participant.into()),
            code,
            rule: None,
            event_key: None,
            tags: Vec::new(),
            params: None,
        }
    }

    /// A reason about the start itself rather than about one participant: which
    /// is what [`ReasonCode::AttendeeUnavailable`] and
    /// [`ReasonCode::TermUnfillable`] are.
    #[must_use]
    pub fn start_level(code: ReasonCode) -> Self {
        Self {
            participant: None,
            code,
            rule: None,
            event_key: None,
            tags: Vec::new(),
            params: None,
        }
    }

    /// The reason citing `rule` as the rule that produced the exclusion.
    #[must_use]
    pub fn with_rule(mut self, rule: impl Into<String>) -> Self {
        self.rule = Some(rule.into());
        self
    }

    /// The reason carrying the key and tags of the block that caused it.
    #[must_use]
    pub fn with_block(mut self, block: &Block) -> Self {
        self.event_key.clone_from(&block.event_key);
        self.tags.clone_from(&block.tags);
        self
    }

    /// The reason carrying a cap's bucket usage, limit and label.
    #[must_use]
    pub fn with_params(mut self, params: ReasonParams) -> Self {
        self.params = Some(params);
        self
    }
}

impl core::fmt::Display for Reason {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        if let Some(participant) = &self.participant {
            write!(f, "{participant}: ")?;
        }
        f.write_str(self.code.text())?;
        if let Some(rule) = &self.rule {
            write!(f, " (rule `{rule}`)")?;
        }
        if let Some(event_key) = &self.event_key {
            write!(f, " (event `{event_key}`)")?;
        }
        if let Some(params) = &self.params {
            match params {
                ReasonParams::CapReached {
                    used,
                    limit,
                    bucket,
                } => write!(f, " (used {used} of {limit} in {bucket})")?,
            }
        }
        if !self.tags.is_empty() {
            let tags: Vec<&str> = self.tags.iter().map(Tag::as_str).collect();
            write!(f, " [{}]", tags.join(", "))?;
        }
        Ok(())
    }
}

/// One candidate start that was not offered, with the reasons it was not.
///
/// A start-level reason has no participant; otherwise there is one reason per
/// attendee and per host the part's terms select, sorted by participant ID.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Excluded {
    /// The start that was not offered.
    pub start: Instant,
    /// The goal part the start belonged to.
    pub part: String,
    /// Why the start was not offered.
    pub reasons: Vec<Reason>,
}

#[cfg(test)]
mod tests {
    use super::{Reason, ReasonCode, ReasonParams};

    #[test]
    fn the_registry_lists_the_codes_in_precedence_order() {
        let codes: Vec<&str> = ReasonCode::ALL.iter().map(ReasonCode::as_str).collect();
        assert_eq!(
            codes,
            [
                "lead_time_min",
                "lead_time_max",
                "outside_window",
                "blackout",
                "blocked",
                "travel",
                "clearance",
                "rest",
                "cap_reached",
                "attendee_unavailable",
                "term_unfillable"
            ]
        );
        // A variant declared out of place would leave the derived ordering and
        // the registry describing two different precedences, so a caller sorting
        // reasons by code would disagree with the response contract.
        assert!(
            ReasonCode::ALL.windows(2).all(|pair| pair[0] < pair[1]),
            "the registry and the derived ordering disagree"
        );
    }

    #[test]
    fn a_cap_reason_reports_its_bucket_and_usage() {
        let reason = Reason::new("ana", ReasonCode::CapReached)
            .with_rule("r_cap")
            .with_params(ReasonParams::CapReached {
                used: 2,
                limit: 2,
                bucket: "2027-03-09".to_owned(),
            });
        let Some(ReasonParams::CapReached {
            used,
            limit,
            bucket,
        }) = reason.params.as_ref()
        else {
            panic!("a cap reason carries cap parameters");
        };
        assert_eq!(*used, 2);
        assert_eq!(*limit, 2);
        assert_eq!(bucket, "2027-03-09");
        assert_eq!(
            reason.to_string(),
            "ana: would take the participant past a workload cap (rule `r_cap`) \
             (used 2 of 2 in 2027-03-09)"
        );
    }

    #[test]
    fn a_reason_reads_as_english() {
        let reason = Reason::new("tom", ReasonCode::Blocked);
        assert_eq!(reason.to_string(), "tom: blocked by an existing event");
        assert_eq!(
            Reason::new("tom", ReasonCode::Travel)
                .with_rule("r_hours")
                .to_string(),
            "tom: travel time is not clear (rule `r_hours`)"
        );
        // A start-level reason names nobody, because the exclusion belongs to the
        // start rather than to one participant.
        let start_level = Reason::start_level(ReasonCode::AttendeeUnavailable);
        assert_eq!(start_level.participant, None);
        assert_eq!(
            start_level.to_string(),
            "an attendee the booking needs is not available"
        );
    }

    #[test]
    fn a_block_caused_reason_carries_the_event_key_and_tags() {
        use crate::model::{Block, Span};
        use crate::tags::Tag;
        use crate::time::Instant;

        let mut block = Block::new(Span::new(Instant::MIN, Instant::MAX).unwrap());
        block.event_key = Some("evt_91".to_owned());
        block.tags = vec![Tag::new("veto/crm-freeze").unwrap()];
        let reason = Reason::new("ana", ReasonCode::Clearance).with_block(&block);
        assert_eq!(reason.event_key.as_deref(), Some("evt_91"));
        assert_eq!(reason.tags, block.tags);
        assert_eq!(
            reason.to_string(),
            "ana: too close to an existing event (event `evt_91`) [veto/crm-freeze]"
        );
    }
}
