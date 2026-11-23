//! The rule envelope and the seven rule kinds version 1 evaluates.

use crate::error::Error;
use crate::model::BlockStatus;
use crate::tags::{Role, Selector};
use crate::time::{BlackoutSpec, Date, Duration, RuleZone, TimeOfDay, Weekday, WindowSpec};
/// One rule: an ID, which participants it constrains, which bookings it applies
/// to, and the kind-specific parameters.
///
/// A rule with no `who` applies to every host, and a rule with no `when` applies
/// to every booking. Rules are the caller's final list: the engine combines them
/// by fixed semantics and never decides which rules apply in a governance sense.
///
/// # How rules of one kind combine
///
/// Every rule that applies to the participant and to the booking is applied; a
/// later rule never replaces an earlier one. Combination is per kind:
///
/// - `window` rules **intersect**: a booking has to lie inside every applicable
///   window, so two windows that both apply leave only the hours they share. A
///   week with different hours per day is therefore one window (see
///   [`crate::presets::weekly_hours`]), not one rule per day.
/// - `blackout` rules **union**: a booking is blocked when any applicable
///   blackout covers it.
/// - `clearance` rules take the **field-wise maximum**: `before`, `after` and
///   `min_gap` are each the largest value any applicable rule asks for, never a
///   sum, so ten minutes of buffer beside a five-minute buffer stays ten.
/// - `lead_time` rules take the **strictest** of every bound: the largest `min`
///   is the notice required, and of every `max` the one that permits the fewest
///   starts wins, so the shortest allowed range results.
/// - `status_policy` entries are tried **in request order across every
///   applicable policy**, and the first entry that matches a block decides
///   whether it blocks.
/// - `rest` rules are **independent**: one rule's `late_after` is never paired
///   with another rule's `not_before`.
/// - `cap` rules are **independent and all binding**: a booking has to fit under
///   every applicable cap's own bucket. A cap applies only when its `when` and
///   its `count` both select the booking, so a cap measuring another kind of
///   event does not constrain this one.
///
/// ```
/// use opportune::model::{Rule, RuleKind, StatusEffect, StatusPolicy, StatusPolicyEntry};
///
/// let policy = StatusPolicy {
///     entries: vec![StatusPolicyEntry::status(
///         opportune::model::BlockStatus::Tentative,
///         StatusEffect::Ignore,
///     )],
/// };
/// let rule = Rule::new("r_7f3c", RuleKind::StatusPolicy(policy));
/// assert_eq!(rule.who, opportune::tags::Selector::Role(opportune::tags::Role::Host));
/// assert_eq!(rule.when, opportune::tags::Selector::True);
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rule {
    /// The rule's ID, unique in the request. Reasons cite it.
    pub id: String,
    /// The kind and its parameters.
    pub kind: RuleKind,
    /// Which participants the rule constrains. Defaults to every host.
    pub who: Selector,
    /// Which bookings it applies to, matched against the part's tags. Defaults
    /// to all of them.
    pub when: Selector,
}

impl Rule {
    /// A rule applying to every host and every booking.
    #[must_use]
    pub fn new(id: impl Into<String>, kind: RuleKind) -> Self {
        Self {
            id: id.into(),
            kind,
            who: Selector::Role(Role::Host),
            when: Selector::True,
        }
    }

    /// The rule with its ID replaced.
    #[must_use]
    pub fn with_id(mut self, id: impl Into<String>) -> Self {
        self.id = id.into();
        self
    }

    /// The rule with its `who` selector replaced.
    #[must_use]
    pub fn with_who(mut self, who: Selector) -> Self {
        self.who = who;
        self
    }

    /// The rule with its `when` selector replaced.
    #[must_use]
    pub fn with_when(mut self, when: Selector) -> Self {
        self.when = when;
        self
    }

    /// The rule with `who` narrowed to the participants `selector` also selects.
    ///
    /// This is the participant-side twin of [`Rule::unless`]: it **intersects**
    /// with the rule's existing `who` instead of replacing it, so a rule that
    /// already applies to every host keeps applying to hosts and only to those
    /// hosts the selector picks out. [`Rule::with_who`] replaces the default
    /// `{"role": "host"}` outright, so `with_who(Selector::kind(Person))` reaches
    /// every person, attendees included; `and_who` keeps the `Role::Host` clause
    /// beside the new one and reaches human hosts alone.
    ///
    /// An unconditional `who` becomes `selector`; an `all` gains the selector as
    /// one more clause, so chained calls stay flat rather than nesting an `all`
    /// in an `all`; anything else becomes `all([existing, selector])`.
    ///
    /// ```
    /// use opportune::model::ParticipantKind;
    /// use opportune::presets;
    /// use opportune::tags::{ParticipantContext, Role, Selector};
    /// use opportune::time::{RuleZone, TimeOfDay, WallEnd, Weekday, Weekdays};
    ///
    /// let hours = presets::business_hours(
    ///     RuleZone::Participant,
    ///     Weekdays::single(Weekday::Monday),
    ///     TimeOfDay::new(9, 0).unwrap(),
    ///     WallEnd::Time(TimeOfDay::new(17, 0).unwrap()),
    /// )
    /// .unwrap()
    /// .and_who(Selector::kind(ParticipantKind::Person));
    /// assert_eq!(
    ///     hours.who,
    ///     Selector::all([
    ///         Selector::role(Role::Host),
    ///         Selector::kind(ParticipantKind::Person),
    ///     ]),
    /// );
    ///
    /// let human_host = ParticipantContext {
    ///     id: "ana",
    ///     kind: ParticipantKind::Person,
    ///     role: Role::Host,
    ///     tags: &[],
    /// };
    /// let room_host = ParticipantContext {
    ///     id: "room-4",
    ///     kind: ParticipantKind::Resource,
    ///     role: Role::Host,
    ///     tags: &[],
    /// };
    /// let human_attendee = ParticipantContext {
    ///     id: "tom",
    ///     kind: ParticipantKind::Person,
    ///     role: Role::Attendee,
    ///     tags: &[],
    /// };
    /// assert!(hours.who.matches_participant(human_host));
    /// assert!(!hours.who.matches_participant(room_host), "the room is not a person");
    /// assert!(!hours.who.matches_participant(human_attendee), "an attendee is not a host");
    /// ```
    #[must_use]
    pub fn and_who(mut self, selector: Selector) -> Self {
        let existing = core::mem::replace(&mut self.who, Selector::True);
        self.who = match existing {
            Selector::True => selector,
            Selector::All(mut clauses) => {
                clauses.push(selector);
                Selector::All(clauses)
            }
            other => Selector::all([other, selector]),
        };
        self
    }

    /// The rule with the participants `selector` selects excepted from `who`.
    ///
    /// This is the participant-side twin of [`Rule::unless`] and is written in
    /// terms of [`Rule::and_who`]: it narrows `who` by `not(selector)`, so the
    /// rule keeps applying to every host but the excepted ones.
    ///
    /// ```
    /// use opportune::presets;
    /// use opportune::tags::{ParticipantContext, Role, Selector, Tag};
    ///
    /// let cap = presets::cap_per_day(Selector::Tag(Tag::new("interview").unwrap()), 2)
    ///     .except_who(Selector::ids(["ceo"]));
    /// assert_eq!(
    ///     cap.who,
    ///     Selector::all([Selector::role(Role::Host), Selector::not(Selector::ids(["ceo"]))]),
    /// );
    ///
    /// let host = ParticipantContext {
    ///     id: "ana",
    ///     kind: opportune::model::ParticipantKind::Person,
    ///     role: Role::Host,
    ///     tags: &[],
    /// };
    /// let ceo = ParticipantContext { id: "ceo", ..host };
    /// assert!(cap.who.matches_participant(host), "the cap still reaches ordinary hosts");
    /// assert!(!cap.who.matches_participant(ceo), "the cap exempts the CEO");
    /// ```
    #[must_use]
    pub fn except_who(self, selector: Selector) -> Self {
        self.and_who(Selector::not(selector))
    }

    /// The rule with `selector` excepted from the bookings it applies to.
    ///
    /// This is how a broad tier is loosened for a narrow one. A rule whose
    /// `when` is unconditional becomes `not(selector)`; one that is already
    /// scoped keeps its scope and adds the exception, as
    /// `all([existing, not(selector)])`.
    ///
    /// Excluding the exception from the broad rule is what makes a hierarchy
    /// work: a window rule intersected with a second window leaves only the hours
    /// the two share, so an exception written as another window on the narrow
    /// tier could only narrow the booking further. A tier that should apply to
    /// everything *but* the exception excludes it here instead.
    ///
    /// [`Rule::and_who`] and [`Rule::except_who`] are the participant-side twins
    /// of this method, narrowing `who` rather than `when`.
    ///
    /// ```
    /// use opportune::presets;
    /// use opportune::tags::{Selector, Tag};
    /// use opportune::time::{Duration, RuleZone, TimeOfDay, WallEnd, Weekday, Weekdays};
    ///
    /// let vip = Selector::tag(Tag::new("vip").unwrap());
    /// let hours = presets::business_hours(
    ///     RuleZone::Participant,
    ///     Weekdays::single(Weekday::Monday),
    ///     TimeOfDay::new(9, 0).unwrap(),
    ///     WallEnd::Time(TimeOfDay::new(17, 0).unwrap()),
    /// )
    /// .unwrap()
    /// .unless(vip.clone());
    /// assert_eq!(hours.when, Selector::not(vip.clone()));
    ///
    /// let interview = Selector::tag(Tag::new("interview").unwrap());
    /// let scoped = presets::buffers(Duration::from_minutes(10), Duration::from_minutes(10))
    ///     .with_when(interview.clone())
    ///     .unless(vip.clone());
    /// assert_eq!(scoped.when, Selector::all([interview, Selector::not(vip)]));
    /// ```
    #[must_use]
    pub fn unless(mut self, selector: Selector) -> Self {
        let excluded = Selector::not(selector);
        let existing = core::mem::replace(&mut self.when, Selector::True);
        self.when = if matches!(existing, Selector::True) {
            excluded
        } else {
            Selector::all([existing, excluded])
        };
        self
    }

    /// The window specification, when the rule is a window.
    #[must_use]
    pub fn window(&self) -> Option<&WindowSpec> {
        match &self.kind {
            RuleKind::Window(spec) => Some(spec),
            _ => None,
        }
    }

    /// The blackout specification, when the rule is a blackout.
    #[must_use]
    pub fn blackout(&self) -> Option<&BlackoutSpec> {
        match &self.kind {
            RuleKind::Blackout(spec) => Some(spec),
            _ => None,
        }
    }

    /// The clearance parameters, when the rule is a clearance.
    #[must_use]
    pub fn clearance(&self) -> Option<&Clearance> {
        match &self.kind {
            RuleKind::Clearance(clearance) => Some(clearance),
            _ => None,
        }
    }

    /// The lead-time parameters, when the rule is a lead-time rule.
    #[must_use]
    pub fn lead_time(&self) -> Option<&LeadTime> {
        match &self.kind {
            RuleKind::LeadTime(lead_time) => Some(lead_time),
            _ => None,
        }
    }

    /// The status policy, when the rule is a status policy.
    #[must_use]
    pub fn status_policy(&self) -> Option<&StatusPolicy> {
        match &self.kind {
            RuleKind::StatusPolicy(policy) => Some(policy),
            _ => None,
        }
    }

    /// The rest parameters, when the rule is a rest rule.
    #[must_use]
    pub fn rest(&self) -> Option<&Rest> {
        match &self.kind {
            RuleKind::Rest(rest) => Some(rest),
            _ => None,
        }
    }

    /// The cap parameters, when the rule is a cap.
    #[must_use]
    pub fn cap(&self) -> Option<&Cap> {
        match &self.kind {
            RuleKind::Cap(cap) => Some(cap),
            _ => None,
        }
    }
}

/// The kinds of rule the engine evaluates. Each kind has one fixed meaning,
/// which is what makes rules validatable, combinable and explainable.
///
/// The enum is `#[non_exhaustive]`: `0.2+` may add kinds without a breaking Rust
/// bump, so a `match` outside this crate needs a `_` arm. Every kind listed here
/// keeps its meaning.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum RuleKind {
    /// When bookings may happen.
    Window(WindowSpec),
    /// When bookings may not happen.
    Blackout(BlackoutSpec),
    /// The gaps the new booking needs between meetings.
    Clearance(Clearance),
    /// How soon and how far ahead a booking may start.
    LeadTime(LeadTime),
    /// How statuses and tags affect availability.
    StatusPolicy(StatusPolicy),
    /// A late finish blocks an early start.
    Rest(Rest),
    /// A limit on how much of a period may be booked.
    Cap(Cap),
}

impl RuleKind {
    /// The kind's wire name: the `kind` a rule stores.
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Window(_) => "window",
            Self::Blackout(_) => "blackout",
            Self::Clearance(_) => "clearance",
            Self::LeadTime(_) => "lead_time",
            Self::StatusPolicy(_) => "status_policy",
            Self::Rest(_) => "rest",
            Self::Cap(_) => "cap",
        }
    }
}

impl core::fmt::Display for RuleKind {
    /// Formats the kind as its wire name.
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A `cap` rule: a limit on how much of a civil period a participant, or a pool
/// of them, may already have booked.
///
/// `count` selects the events the cap measures, matched against occurrence and
/// block tags whatever their status or `status_policy`: a cap measures workload,
/// not availability. An all-day block is time away from work rather than meeting
/// workload, so it never counts toward a cap itself, whatever its key or tags,
/// though its tags still join the event an occurrence identifies. `measure` is the
/// number of events or their minutes, where minutes are each event's own duration
/// without travel. `period` is a civil day, week or month in `zone`, and a week
/// starts on `week_starts` and runs seven civil days rather than following ISO
/// weeks.
///
/// `per: each` limits each participant in `who` on their own; `per: pooled`
/// limits them together and therefore needs a zone every event shares, so
/// `pooled` with `zone: participant` is a validation error.
///
/// A cap applies to a booking only when the part's tags match both the rule's
/// `when` and this `count`.
///
/// ```
/// use opportune::model::{Cap, CapMeasure, CapPer, CapPeriod};
/// use opportune::tags::{Selector, Tag};
/// use opportune::time::{RuleZone, Weekday};
///
/// let cap = Cap::new(
///     Selector::Tag(Tag::new("interview").unwrap()),
///     CapMeasure::Count,
///     2,
///     CapPeriod::Day,
///     CapPer::Each,
///     RuleZone::Participant,
///     Weekday::Monday,
/// );
/// assert_eq!(cap.limit, 2);
/// assert_eq!(cap.period, CapPeriod::Day);
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cap {
    /// Which events the cap measures, matched against their tags.
    pub count: Selector,
    /// Whether events or their minutes count.
    pub measure: CapMeasure,
    /// The most the period may hold, once the booking's contribution is added.
    pub limit: u32,
    /// The civil period a bucket covers.
    pub period: CapPeriod,
    /// Whether the limit applies to each participant or to the pool together.
    pub per: CapPer,
    /// The zone the bucket labels are read in. `pooled` requires `event` or a
    /// named zone.
    pub zone: RuleZone,
    /// The weekday a week bucket starts on.
    pub week_starts: Weekday,
}

impl Cap {
    /// A cap over the events `count` selects, with the given measure, limit,
    /// period, per-participant scope and bucket zone.
    #[must_use]
    pub const fn new(
        count: Selector,
        measure: CapMeasure,
        limit: u32,
        period: CapPeriod,
        per: CapPer,
        zone: RuleZone,
        week_starts: Weekday,
    ) -> Self {
        Self {
            count,
            measure,
            limit,
            period,
            per,
            zone,
            week_starts,
        }
    }
}

/// What a `cap` measures: the number of events, or their minutes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CapMeasure {
    /// Each matching event counts once.
    Count,
    /// Each matching event contributes its own duration, excluding travel.
    Minutes,
}

impl CapMeasure {
    /// The measure's wire name: `count` or `minutes`.
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Count => "count",
            Self::Minutes => "minutes",
        }
    }
}

impl core::fmt::Display for CapMeasure {
    /// Formats the measure as its wire name.
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The civil period a `cap` bucket covers.
///
/// The enum is `#[non_exhaustive]`: `0.2+` may add periods — a quarter, a rolling
/// window — without a breaking Rust bump, so a `match` outside this crate needs a
/// `_` arm.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum CapPeriod {
    /// One civil day.
    Day,
    /// Seven civil days from the configured `week_starts` weekday.
    Week,
    /// One civil month.
    Month,
}

impl CapPeriod {
    /// The period's wire name: `day`, `week` or `month`.
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Day => "day",
            Self::Week => "week",
            Self::Month => "month",
        }
    }
}

impl core::fmt::Display for CapPeriod {
    /// Formats the period as its wire name.
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Whether a `cap` limits each participant or the pool together.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CapPer {
    /// Each participant in `who` is limited on their own.
    Each,
    /// The participants in `who` are limited together, which needs one bucket
    /// zone they all share.
    Pooled,
}

impl CapPer {
    /// The scope's wire name: `each` or `pooled`.
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Each => "each",
            Self::Pooled => "pooled",
        }
    }
}

impl core::fmt::Display for CapPer {
    /// Formats the scope as its wire name.
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A `rest` rule: a late finish blocks an early start.
///
/// Several rest rules in one request never combine: each is evaluated on its own
/// pair of times (see [`Rule`] for the combination semantics per kind).
///
/// *Activity* is the occupied spans of a participant's blocking, non-all-day
/// blocks plus the booking being offered, where an occupied span is widened by
/// its travel. For a local date `D`, `late(D)` is the part of `D` at or after
/// `late_after` and `early(D)` the part before `not_before`, both read in the
/// participant's zone at each instant. A booking is excluded when activity
/// overlaps both `late(D)` and `early(D + 1)` for some `D`, and the booking
/// itself is part of one of those overlaps.
///
/// All-day blocks and blackouts never count as activity, `status_policy`
/// decides which blocks block, and each rest rule is evaluated independently of
/// the others: two rules never combine one rule's `late_after` with another
/// rule's `not_before`.
///
/// ```
/// use opportune::model::Rest;
/// use opportune::time::TimeOfDay;
///
/// let rest = Rest::new(TimeOfDay::new(18, 0).unwrap(), TimeOfDay::new(10, 0).unwrap());
/// assert_eq!(rest.late_after.to_string(), "18:00");
/// assert_eq!(rest.not_before.to_string(), "10:00");
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rest {
    /// The local time from which a finish counts as late.
    pub late_after: TimeOfDay,
    /// The local time before which a start counts as early.
    pub not_before: TimeOfDay,
}

impl Rest {
    /// A rest rule protecting the hours before `not_before` after any activity
    /// at or after `late_after` the day before.
    #[must_use]
    pub const fn new(late_after: TimeOfDay, not_before: TimeOfDay) -> Self {
        Self {
            late_after,
            not_before,
        }
    }
}

/// The gaps a booking needs between it and the participant's other events.
///
/// `before` and `after` are the gaps the new booking needs; `min_gap` is the
/// least gap between any two events. The gap required between an event and the
/// booking is the largest that applies, never a sum.
///
/// ```
/// use opportune::model::Clearance;
/// use opportune::time::Duration;
///
/// let buffers = Clearance {
///     before: Duration::from_hours_minutes(0, 10),
///     after: Duration::from_hours_minutes(0, 10),
///     min_gap: Duration::from_hours_minutes(0, 5),
/// };
/// assert_eq!(buffers.before.minutes(), 10);
/// ```
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Clearance {
    /// The gap the booking needs before it.
    pub before: Duration,
    /// The gap the booking needs after it.
    pub after: Duration,
    /// The least gap between any two events.
    pub min_gap: Duration,
}

/// How soon and how far ahead a booking may start.
///
/// ```
/// use opportune::model::{LeadTime, LeadTimeMax};
/// use opportune::time::Duration;
///
/// let notice = LeadTime {
///     min: Duration::from_hours_minutes(4, 0),
///     max: Some(LeadTimeMax::Days(3)),
/// };
/// assert_eq!(notice.min.minutes(), 240);
/// ```
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LeadTime {
    /// The booking's occupied start must be at least `now + min`, so travel
    /// counts.
    pub min: Duration,
    /// The latest start allowed, in one of the four forms. `None` leaves the
    /// search window as the only limit.
    pub max: Option<LeadTimeMax>,
}

/// A `lead_time` maximum: how far ahead a start may be.
///
/// A working-day count names its calendar, so the request stays independent of
/// the calendar's resolved zone.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LeadTimeMax {
    /// At most `now + duration`.
    Duration(Duration),
    /// At most `today + days` calendar days, where today is `now`'s local date
    /// in the event zone.
    Days(u32),
    /// At most the `count`-th working day after today, on the named calendar.
    WorkingDays {
        /// How many working days after today remain inside the horizon.
        count: u32,
        /// The name of the request calendar that decides which days are working
        /// days.
        calendar: String,
    },
    /// At most this local date in the event zone.
    Date(Date),
}

/// How statuses and tags affect availability.
///
/// Entries are tried in order, across rules in request order: the first entry
/// that matches a block decides whether it blocks.
///
/// ```
/// use opportune::model::{BlockStatus, StatusEffect, StatusPolicy, StatusPolicyEntry};
///
/// let policy = StatusPolicy {
///     entries: vec![StatusPolicyEntry::status(BlockStatus::Tentative, StatusEffect::Ignore)],
/// };
/// assert_eq!(policy.decide(BlockStatus::Tentative, &[]), Some(StatusEffect::Ignore));
/// assert_eq!(policy.decide(BlockStatus::Busy, &[]), None);
/// ```
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StatusPolicy {
    /// The entries, in the order the caller wrote them.
    pub entries: Vec<StatusPolicyEntry>,
}

impl StatusPolicy {
    /// The effect the first matching entry names for a block with `status` and
    /// `tags`, or `None` when no entry matches.
    #[must_use]
    pub fn decide(&self, status: BlockStatus, tags: &[crate::tags::Tag]) -> Option<StatusEffect> {
        self.entries
            .iter()
            .find(|entry| entry.matches(status, tags))
            .map(|entry| entry.effect)
    }
}

/// One status-policy entry: what it matches and what it decides.
///
/// An entry matches on a status, a tag selector, or both. An entry that matches
/// neither is rejected at construction, because it could never apply while
/// reading as if it would.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StatusPolicyEntry {
    /// The block status this entry matches, if it matches on status.
    pub status: Option<BlockStatus>,
    /// The tag selector this entry matches, if it matches on tags.
    pub tags: Option<Selector>,
    /// What happens to a block the entry matches.
    pub effect: StatusEffect,
}

impl StatusPolicyEntry {
    /// An entry matching one status.
    #[must_use]
    pub fn status(status: BlockStatus, effect: StatusEffect) -> Self {
        Self {
            status: Some(status),
            tags: None,
            effect,
        }
    }

    /// An entry matching a tag selector.
    #[must_use]
    pub fn tags(tags: Selector, effect: StatusEffect) -> Self {
        Self {
            status: None,
            tags: Some(tags),
            effect,
        }
    }

    /// An entry matching both a status and a tag selector.
    #[must_use]
    pub fn status_and_tags(status: BlockStatus, tags: Selector, effect: StatusEffect) -> Self {
        Self {
            status: Some(status),
            tags: Some(tags),
            effect,
        }
    }

    /// Whether the entry matches a block.
    #[must_use]
    pub fn matches(&self, status: BlockStatus, tags: &[crate::tags::Tag]) -> bool {
        if self.status.is_some_and(|matched| matched != status) {
            return false;
        }
        self.tags
            .as_ref()
            .is_none_or(|selector| selector.matches_item(tags))
    }

    /// Checks that the entry matches something at all.
    ///
    /// # Errors
    /// Returns [`Error::StatusPolicyEntryEmpty`] when the entry names neither a
    /// status nor a tag selector.
    pub fn validate(&self) -> Result<(), Error> {
        if self.status.is_none() && self.tags.is_none() {
            return Err(Error::StatusPolicyEntryEmpty);
        }
        Ok(())
    }
}

/// What a matching status-policy entry does to a block.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum StatusEffect {
    /// The block is ignored: its occupied span does not block, and its travel is
    /// ignored with it.
    Ignore,
    /// The block blocks. The default for `tentative` and `busy` blocks, and the
    /// effect that makes a `free` block block.
    Block,
}

impl StatusEffect {
    /// The effect's wire name: `ignore` or `block`.
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Ignore => "ignore",
            Self::Block => "block",
        }
    }
}

impl core::fmt::Display for StatusEffect {
    /// Formats the effect as its wire name.
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(self.as_str())
    }
}
