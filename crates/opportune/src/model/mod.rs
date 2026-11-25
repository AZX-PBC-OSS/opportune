//! The scheduling domain model: what a request is and what a response is.
//!
//! These are the types every surface parses into and the engine evaluates. They
//! are independent of any time zone database and of the engine's limits: a
//! request names zones and calendars, and [`crate::Engine`] resolves the names
//! when it evaluates. That is what lets a request be built, stored and parsed
//! before any database is chosen, and what lets `validate` report an unknown
//! zone instead of a constructor failing.
//!
//! Shapes that will grow already allow it, so growth is additive within one major
//! version: a goal has a list of parts, an offer a list of events, and an
//! assignment one list per term. What is not list-shaped is closed against
//! construction but open to new members: [`Response`], [`SlotCheck`] and the
//! diagnostics are `#[non_exhaustive]`, [`Error`] and its time and
//! interval companions are non-exhaustive enums, and [`crate::Limits`] is a
//! non-exhaustive struct built through its `with_*` methods. A new response
//! field, reason code, error variant or limit is therefore a minor release, not a
//! breaking one.
//!
//! ```
//! use opportune::model::{Goal, Part, Requirement, Request, Span, Term};
//! use opportune::time::{Duration, Instant, ZoneName};
//!
//! let search = Span::new(Instant::MIN, Instant::MAX).unwrap();
//! let part = Part::new(
//!     Duration::from_minutes(30),
//!     Requirement::new(vec![Term::person("rich")]),
//! );
//! let request = Request::new(Instant::MIN, Goal::new(ZoneName::new("UTC").unwrap(), search, vec![part]));
//! assert_eq!(request.limit(), 1_000);
//! assert_eq!(request.goal.parts[0].id, "main");
//! ```

mod goal;
mod output;
mod participant;
mod rule;

pub use goal::{Goal, Part, ParticipantTravel, Requirement, Term, Travel, ZonePeriod};
pub use output::{
    DateCount, Meta, Offer, OfferEvent, OfferFlag, ReserveKind, ReserveSpan, Response,
    SCHEMA_VERSION, SlotCheck,
};
pub use participant::{Block, BlockStatus, CalendarSpec, Occurrence, Participant, ParticipantKind};
pub use rule::{
    Cap, CapMeasure, CapPer, CapPeriod, Clearance, LeadTime, LeadTimeMax, Rest, Rule, RuleKind,
    StatusEffect, StatusPolicy, StatusPolicyEntry,
};
// The tag vocabulary's three types a model value names: `Participant.role`, and
// `Rule.who`, `Term::AtLeast.of` and `StatusPolicyEntry.tags`. They travel with
// the domain types so a caller can name every field type of every value here
// from `opportune::model` alone, as it already can for `ParticipantKind`.
pub use crate::tags::{Role, Selector, Tag};

use std::collections::BTreeMap;

use crate::error::Error;
use crate::time::Instant;

/// The default maximum number of offers returned (`1,000`).
pub const DEFAULT_LIMIT: usize = 1_000;

/// A half-open span of instants, `[start, end)`, at minute resolution.
///
/// Spans are non-empty by construction, so every span covers at least one
/// minute and a zero-length or inverted pair is rejected where it is written
/// rather than silently covering nothing.
///
/// ```
/// use opportune::model::Span;
/// use opportune::time::Instant;
///
/// let span = Span::new(Instant::MIN, Instant::MAX).unwrap();
/// assert!(span.contains(Instant::MIN));
/// assert!(!span.contains(Instant::MAX));
/// assert!(Span::new(Instant::MAX, Instant::MIN).is_err());
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Span {
    start: Instant,
    end: Instant,
}

impl Span {
    /// The widest span the model can express: [`Instant::MIN`] up to
    /// [`Instant::MAX`], the whole supported instant range.
    ///
    /// It is the window for a request whose only bounds are the rules, and the
    /// span to store when a part has no narrower search range. A [`Span`] is
    /// non-empty by construction and the two bounds differ, so this needs no
    /// fallible constructor.
    ///
    /// ```
    /// use opportune::model::Span;
    /// use opportune::time::Instant;
    ///
    /// assert_eq!(Span::EVERYTHING.start(), Instant::MIN);
    /// assert_eq!(Span::EVERYTHING.end(), Instant::MAX);
    /// assert_eq!(
    ///     Span::new(Instant::MIN, Instant::MAX).unwrap(),
    ///     Span::EVERYTHING
    /// );
    /// ```
    pub const EVERYTHING: Self = Self {
        start: Instant::MIN,
        end: Instant::MAX,
    };

    /// Creates the span `[start, end)`.
    ///
    /// # Errors
    /// Returns [`Error::SpanEmpty`] when `start` and `end` are equal, and
    /// [`Error::SpanReversed`] when `start` is after `end`.
    pub fn new(start: Instant, end: Instant) -> Result<Self, Error> {
        if start == end {
            return Err(Error::SpanEmpty { start, end });
        }
        if start > end {
            return Err(Error::SpanReversed { start, end });
        }
        Ok(Self { start, end })
    }

    /// The span's first instant.
    #[must_use]
    pub const fn start(&self) -> Instant {
        self.start
    }

    /// One past the span's last instant.
    #[must_use]
    pub const fn end(&self) -> Instant {
        self.end
    }

    /// Whether the span covers `instant`, half-open.
    #[must_use]
    pub fn contains(&self, instant: Instant) -> bool {
        self.start <= instant && instant < self.end
    }

    /// How many whole minutes the span covers.
    #[must_use]
    pub fn minutes(&self) -> i64 {
        self.end.minutes() - self.start.minutes()
    }

    /// This span as a whole-minute interval span.
    ///
    /// A `Span` is non-empty by construction (`start < end`), so the conversion
    /// cannot fail.
    #[must_use]
    pub(crate) const fn to_minute_span(self) -> crate::intervals::Span {
        crate::intervals::Span::from_valid(self.start.minutes(), self.end.minutes())
    }
}

/// One scheduling request: the current instant, what is being booked, who is
/// involved and the rules that decide availability.
///
/// `calendars` holds the named working calendars that a `lead_time` maximum's
/// working-day form refers to. The time zone database and the engine's limits
/// belong to the engine, not to the request, so a request is a value that can be
/// built and stored without either.
///
/// ```
/// use opportune::model::{Explain, Goal, Part, Requirement, Request, Span, Term};
/// use opportune::time::{Duration, Instant, ZoneName};
///
/// let part = Part::new(
///     Duration::from_minutes(30),
///     Requirement::new(vec![Term::person("rich")]),
/// );
/// let goal = Goal::new(
///     ZoneName::new("Europe/London").unwrap(),
///     Span::new(Instant::MIN, Instant::MAX).unwrap(),
///     vec![part],
/// );
/// let mut request = Request::new(Instant::MAX, goal);
/// request.limit = Some(20);
/// request.explain = Some(Explain::new(200));
/// request.weights.insert("rich".to_owned(), 3);
/// assert_eq!(request.limit(), 20);
/// assert_eq!(request.explain.map(|explain| explain.limit), Some(200));
/// assert_eq!(request.weight("rich"), 3);
/// assert_eq!(request.weight("ana"), 0);
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request {
    /// The current instant, rounded up to the next minute.
    pub now: Instant,
    /// What is being booked.
    pub goal: Goal,
    /// Everyone the request knows about.
    pub participants: Vec<Participant>,
    /// The final rule list, combined by the engine's fixed semantics.
    pub rules: Vec<Rule>,
    /// The caller's bookings and sessions, and any other events it can identify.
    pub occurrences: Vec<Occurrence>,
    /// The caller's event keys for the events it is moving, which the engine
    /// ignores wherever an existing event would otherwise block time, trigger
    /// `rest` activity or count toward a cap. Empty by default, so naming none
    /// changes nothing.
    pub exclude_event_keys: Vec<String>,
    /// Named working calendars, by name.
    pub calendars: BTreeMap<String, CalendarSpec>,
    /// The weight each participant's assignment carries, by participant ID.
    /// Assignments maximise the total, and a participant the map omits weighs
    /// zero.
    pub weights: BTreeMap<String, i32>,
    /// The maximum number of offers to return, or `None` for the default.
    pub limit: Option<usize>,
    /// Whether to report why candidate starts were not offered, or `None` for no
    /// explanation at all.
    pub explain: Option<Explain>,
}

impl Request {
    /// A request with no participants, rules or occurrences, the default offer
    /// limit, and no explanation.
    #[must_use]
    pub fn new(now: Instant, goal: Goal) -> Self {
        Self {
            now,
            goal,
            participants: Vec::new(),
            rules: Vec::new(),
            occurrences: Vec::new(),
            exclude_event_keys: Vec::new(),
            calendars: BTreeMap::new(),
            weights: BTreeMap::new(),
            limit: None,
            explain: None,
        }
    }

    /// The request with `participants` replacing the list it carries.
    #[must_use]
    pub fn with_participants(mut self, participants: Vec<Participant>) -> Self {
        self.participants = participants;
        self
    }

    /// The request with `rules` replacing the final rule list it carries.
    #[must_use]
    pub fn with_rules(mut self, rules: Vec<Rule>) -> Self {
        self.rules = rules;
        self
    }

    /// The request with `occurrences` replacing the ones it carries.
    #[must_use]
    pub fn with_occurrences(mut self, occurrences: Vec<Occurrence>) -> Self {
        self.occurrences = occurrences;
        self
    }

    /// The request with `keys` replacing the event keys it excludes.
    ///
    /// ```
    /// use opportune::model::{Goal, Request, Span};
    /// use opportune::time::{Instant, ZoneName};
    ///
    /// let goal = Goal::new(
    ///     ZoneName::new("UTC").unwrap(),
    ///     Span::new(Instant::MIN, Instant::MAX).unwrap(),
    ///     Vec::new(),
    /// );
    /// let request = Request::new(Instant::MIN, goal)
    ///     .with_exclude_event_keys(vec!["evt_1".to_owned()])
    ///     .exclude_event_key("evt_2");
    /// assert_eq!(request.exclude_event_keys, ["evt_1", "evt_2"]);
    /// ```
    #[must_use]
    pub fn with_exclude_event_keys(mut self, keys: Vec<String>) -> Self {
        self.exclude_event_keys = keys;
        self
    }

    /// The request excluding one more event key: the event being rescheduled.
    #[must_use]
    pub fn exclude_event_key(mut self, key: impl Into<String>) -> Self {
        self.exclude_event_keys.push(key.into());
        self
    }

    /// The request with `calendars` replacing the working calendars it carries.
    #[must_use]
    pub fn with_calendars(mut self, calendars: BTreeMap<String, CalendarSpec>) -> Self {
        self.calendars = calendars;
        self
    }

    /// The request with `weights` replacing the assignment weights it carries.
    #[must_use]
    pub fn with_weights(mut self, weights: BTreeMap<String, i32>) -> Self {
        self.weights = weights;
        self
    }

    /// The request asking for at most `limit` offers.
    ///
    /// ```
    /// use opportune::model::{Goal, Participant, Request, Span};
    /// use opportune::time::{Instant, ZoneName};
    ///
    /// let zone = ZoneName::new("UTC").unwrap();
    /// let goal = Goal::new(zone.clone(), Span::new(Instant::MIN, Instant::MAX).unwrap(), Vec::new());
    /// let request = Request::new(Instant::MIN, goal)
    ///     .with_participants(vec![Participant::new("ana", zone)])
    ///     .with_limit(20)
    ///     .with_explain(50);
    /// assert_eq!(request.limit(), 20);
    /// assert_eq!(request.explain.map(|explain| explain.limit), Some(50));
    /// ```
    #[must_use]
    pub fn with_limit(mut self, limit: usize) -> Self {
        self.limit = Some(limit);
        self
    }

    /// The request asking for exclusion reasons, at most `limit` of them.
    #[must_use]
    pub fn with_explain(mut self, limit: usize) -> Self {
        self.explain = Some(Explain::new(limit));
        self
    }

    /// The maximum number of offers to return: [`DEFAULT_LIMIT`] unless the
    /// request names its own.
    #[must_use]
    pub fn limit(&self) -> usize {
        self.limit.unwrap_or(DEFAULT_LIMIT)
    }

    /// The weight `participant`'s assignment carries (defaults to `0` when omitted).
    #[must_use]
    pub fn weight(&self, participant: &str) -> i32 {
        self.weights.get(participant).copied().unwrap_or(0)
    }
}

/// How much exclusion detail a search returns.
///
/// Entries are atomic: a start is reported with every reason it has, or not at
/// all, so an explanation is never cut in the middle of one start's reasons.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Explain {
    /// The maximum number of reasons returned across every excluded start.
    pub limit: usize,
}

impl Explain {
    /// Asks for at most `limit` reasons in total.
    #[must_use]
    pub const fn new(limit: usize) -> Self {
        Self { limit }
    }
}

/// A booking a caller proposes to confirm, checked with
/// [`Engine::check_slot`](crate::Engine::check_slot).
///
/// ```
/// use opportune::model::Proposal;
/// use opportune::time::Instant;
///
/// let proposal = Proposal::single("main", Instant::MIN, vec![vec!["ana".to_owned()]]);
/// assert_eq!(proposal.events.len(), 1);
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Proposal {
    /// One proposed event per goal part.
    pub events: Vec<ProposalEvent>,
}

impl Proposal {
    /// A proposal with one event, which version 1's single-part goals need.
    ///
    /// It builds exactly what the struct literal builds, so nothing is lost by
    /// using it.
    #[must_use]
    pub fn single(part: impl Into<String>, start: Instant, assignment: Vec<Vec<String>>) -> Self {
        Self {
            events: vec![ProposalEvent {
                part: part.into(),
                start,
                assignment,
            }],
        }
    }
}

/// One event of a [`Proposal`]: which part it books, when, and who takes it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProposalEvent {
    /// The goal part's ID.
    pub part: String,
    /// The proposed start.
    pub start: Instant,
    /// The hosts to assign, one list per term.
    pub assignment: Vec<Vec<String>>,
}

/// What calendar and occurrence data a request needs, before fetching any.
///
/// The query carries the request's `now`, goal, rules and participants (without
/// calendars), because the range depends on the booking's travel and the
/// clearance between events — not on the blocks themselves.
///
/// ```
/// use opportune::model::{Goal, RangesQuery, Span};
/// use opportune::time::{Instant, ZoneName};
///
/// let goal = Goal::new(
///     ZoneName::new("UTC").unwrap(),
///     Span::new(Instant::MIN, Instant::MAX).unwrap(),
///     Vec::new(),
/// );
/// let query = RangesQuery::new(Instant::MIN, goal);
/// assert_eq!(query.now, Instant::MIN);
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RangesQuery {
    /// The current instant, rounded up to the next minute.
    pub now: Instant,
    /// What is being booked.
    pub goal: Goal,
    /// The final rule list.
    pub rules: Vec<Rule>,
    /// The participants, without their calendars.
    pub participants: Vec<Participant>,
}

impl RangesQuery {
    /// A query with no participants and no rules.
    #[must_use]
    pub fn new(now: Instant, goal: Goal) -> Self {
        Self {
            now,
            goal,
            rules: Vec::new(),
            participants: Vec::new(),
        }
    }

    /// The query with `participants` replacing the list it carries.
    #[must_use]
    pub fn with_participants(mut self, participants: Vec<Participant>) -> Self {
        self.participants = participants;
        self
    }

    /// The query with `rules` replacing the final rule list it carries.
    #[must_use]
    pub fn with_rules(mut self, rules: Vec<Rule>) -> Self {
        self.rules = rules;
        self
    }
}

/// One UTC envelope of data a request needs.
///
/// `calendar` is the hull every participant's local dates in the padded search
/// window fall inside, covering blocking events, clearance, part travel and
/// neighbouring `rest` days. `occurrences` is `None` when no cap applies; when a
/// cap is present it is the hull of every bucket the search window touches in the
/// cap's own zone. The engine returns it from
/// [`Engine::required_ranges`](crate::Engine::required_ranges).
///
/// # The fetch is two-phase, not one pass
///
/// A cap merges every record sharing an event key across all supplied time
/// ranges, so these hulls are the input to a contract, not the whole fetch. With
/// `occurrences` present:
///
/// 1. Fetch every availability block whose occupied span — its own span widened
///    by its travel — overlaps `calendar`, and every occurrence and calendar block
///    whose own span overlaps `occurrences`, on every request participant.
/// 2. Collect the deduplicated event keys of every **occurrence-range** record
///    from step 1 — every occurrence and every calendar block whose own span
///    overlaps `occurrences`, on every participant.
/// 3. Fetch every occurrence and every block carrying one of those keys, on every
///    request participant, across the supported time domain. Include all-day and
///    otherwise nonmatching records, and deduplicate by provider record identity
///    rather than by event key. One round settles because every record carries at
///    most one event key.
/// 4. Evaluate only the closed request, against a consistent provider snapshot.
///
/// Do not prefilter step 1 by cap tags, status or `all_day`: a cap's `count`
/// matches the union of an occurrence's tags with every same-key block's, so a
/// record another participant holds can supply the counted tag. A one-pass fetch
/// can therefore produce a false offer — an out-of-hull same-key block missing
/// from a minutes cap — or a false exclusion — an out-of-hull same-key block that
/// would have moved a count event to another bucket.
///
/// A one-pass fetch is key-complete only under a caller-enforced invariant: every
/// occurrence and block sharing an event key has the identical own span.
/// Per-instance keys alone are not enough when split copies can carry different
/// spans. When closure cannot be completed exactly, fetch all occurrences and
/// blocks across the full supported time domain (`Span::EVERYTHING`,
/// `1975-01-01T00:00Z..2100-01-01T00:00Z`) or fail closed; never evaluate partial
/// cap data as an empty workload. The full supported domain is a fallback data
/// requirement, not a replacement for the narrow `occurrences` hull.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct RequiredRanges {
    /// The UTC range every participant's calendar must cover for availability,
    /// clearance, part travel and rest.
    ///
    /// A caller that reports per-block travel ([`Block::travel_before`] and
    /// [`Block::travel_after`]) has to fetch every block whose **occupied** span
    /// — the block's own span widened by that travel — overlaps this range. The
    /// engine widens each block by the travel the caller put on it, so a block
    /// ending just before the range can still occupy a minute inside it.
    pub calendar: Span,
    /// The UTC range of the initial occurrence-range fetch, or `None` when no cap
    /// applies.
    ///
    /// Fetch every occurrence and every calendar block whose own span overlaps
    /// this range, on every participant — including attendees and participants no
    /// cap covers — without prefiltering by tags, status or `all_day`. Then apply
    /// the one-round key closure described on the type: every record carrying an
    /// observed event key is needed even when it lies outside this range, and a
    /// one-pass fetch is key-complete only under the identical-own-span
    /// invariant. When that closure cannot be completed, fetch the full supported
    /// time domain (`Span::EVERYTHING`) or fail closed.
    pub occurrences: Option<Span>,
}

#[cfg(test)]
mod tests {
    use super::{DEFAULT_LIMIT, Span};
    use crate::time::Instant;

    #[test]
    fn a_span_is_half_open_and_non_empty() {
        let start = Instant::MIN;
        let end = Instant::from_minutes(start.minutes() + 30).unwrap();
        let span = Span::new(start, end).unwrap();
        assert!(span.contains(start));
        assert!(!span.contains(end));
        assert_eq!(span.minutes(), 30);
        assert_eq!(Span::new(start, start).unwrap_err().code(), "span_empty");
        assert_eq!(Span::new(end, start).unwrap_err().code(), "span_reversed");
    }

    #[test]
    fn the_default_limit_is_the_published_one() {
        assert_eq!(DEFAULT_LIMIT, 1_000);
    }
}
