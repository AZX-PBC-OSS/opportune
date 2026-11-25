//! What is being booked: the goal, its parts, their requirements, and a
//! participant's zone while travelling.

use std::collections::BTreeMap;

use crate::error::Error;
use crate::model::{Participant, Span};
use crate::tags::{Role, Selector, Tag};
use crate::time::{Duration, Grid, Instant, ZoneName};

/// What is being booked: one part, its grid or candidate starts, the search
/// window, and the zones the booking is read in.
///
/// ```
/// use opportune::model::{Goal, Part, Requirement, Span, Term};
/// use opportune::time::{Duration, Instant, ZoneName};
///
/// let zone = ZoneName::new("America/New_York").unwrap();
/// let part = Part::new(
///     Duration::from_minutes(30),
///     Requirement::new(vec![Term::person("rich")]),
/// );
/// let goal = Goal::new(
///     zone.clone(),
///     Span::new(Instant::MIN, Instant::MAX).unwrap(),
///     vec![part],
/// );
/// // The display zone defaults to the event zone.
/// assert_eq!(goal.display_zone, zone);
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Goal {
    /// What is being booked. Version 1 requires exactly one part.
    pub parts: Vec<Part>,
    /// The grid candidate starts are drawn from.
    ///
    /// A goal with neither a grid nor [`Goal::candidates`] uses the default grid:
    /// a step of `min(part.duration, 30m)` — never less than one minute —
    /// anchored at `00:00` in [`Goal::zone`], the event zone. A request's hours
    /// are therefore offered on boundaries counted from local midnight in the
    /// event zone, not in the display zone or a participant's own zone: a
    /// 15-minute booking on quarter hours and a 45-minute one on half hours.
    pub grid: Option<Grid>,
    /// Caller-supplied candidate start instants, instead of a grid. A request
    /// carrying both is rejected.
    pub candidates: Option<Vec<Instant>>,
    /// The range of starts to consider.
    pub search_window: Span,
    /// The event zone: what rules that say `zone: event` are read in.
    pub zone: ZoneName,
    /// The zone offers are grouped by date in.
    pub display_zone: ZoneName,
}

impl Goal {
    /// A goal over `search_window` in `zone`, with the display zone defaulting
    /// to the event zone.
    #[must_use]
    pub fn new(zone: ZoneName, search_window: Span, parts: Vec<Part>) -> Self {
        Self {
            parts,
            grid: None,
            candidates: None,
            search_window,
            display_zone: zone.clone(),
            zone,
        }
    }

    /// The goal drawing candidate starts from `grid`.
    #[must_use]
    pub fn with_grid(mut self, grid: Grid) -> Self {
        self.grid = Some(grid);
        self
    }

    /// The goal drawing candidate starts from the caller's `candidates`.
    #[must_use]
    pub fn with_candidates(mut self, candidates: Vec<Instant>) -> Self {
        self.candidates = Some(candidates);
        self
    }

    /// The goal grouping offers by date in `zone`.
    #[must_use]
    pub fn with_display_zone(mut self, zone: ZoneName) -> Self {
        self.display_zone = zone;
        self
    }

    /// The goal's only part, which version 1 requires. A request with any other
    /// number of parts is rejected by `validate`.
    #[must_use]
    pub fn part(&self) -> Option<&Part> {
        match self.parts.as_slice() {
            [only] => Some(only),
            _ => None,
        }
    }

    /// A goal with one part: the shortcut for the common single-meeting case.
    ///
    /// It builds exactly what [`Goal::new`] with one [`Part::new`] builds, so
    /// nothing is lost by using it. It sets neither a grid nor candidates, so the
    /// search uses the default grid: a step of `min(duration, 30m)` (never less
    /// than one minute) anchored at `00:00` in `zone`.
    ///
    /// ```
    /// use opportune::model::{Goal, Requirement, Span, Term};
    /// use opportune::time::{Duration, Instant, ZoneName};
    ///
    /// let zone = ZoneName::new("UTC").unwrap();
    /// let window = Span::new(Instant::MIN, Instant::MAX).unwrap();
    /// let goal = Goal::single(
    ///     zone.clone(),
    ///     window,
    ///     Duration::from_hours_minutes(0, 45),
    ///     Requirement::new(vec![Term::person("rich")]),
    /// );
    /// assert_eq!(goal.parts.len(), 1);
    /// assert_eq!(goal.parts[0].id, "main");
    /// assert_eq!(goal.display_zone, zone);
    /// assert!(goal.grid.is_none());
    /// ```
    #[must_use]
    pub fn single(
        zone: ZoneName,
        search_window: Span,
        duration: Duration,
        requirement: Requirement,
    ) -> Self {
        Self::new(zone, search_window, vec![Part::new(duration, requirement)])
    }

    /// A goal over `search_window` in `zone` with `part` as its only part,
    /// keeping the part's own ID, tags and travel.
    ///
    /// [`Goal::single`] builds a fresh [`Part::new`], which carries the default
    /// ID, no tags and no travel. A booking that needs any of those — a part
    /// tagged so a scoped rule or cap applies, or one travelling before and after
    /// — is written with this constructor instead of [`Goal::new`] and a
    /// one-element list. Like `single` it sets neither a grid nor candidates, so
    /// the search uses the same default grid.
    ///
    /// ```
    /// use opportune::model::{Goal, Part, Requirement, Span, Term};
    /// use opportune::tags::Tag;
    /// use opportune::time::{Duration, ZoneName};
    ///
    /// let part = Part::new(
    ///     Duration::from_minutes(45),
    ///     Requirement::new(vec![Term::person("ana")]),
    /// )
    /// .with_id("round-1")
    /// .with_tags(vec![Tag::new("interview").unwrap()]);
    ///
    /// let goal = Goal::single_part(ZoneName::utc(), Span::EVERYTHING, part);
    /// assert_eq!(goal.parts.len(), 1);
    /// assert_eq!(goal.parts[0].id, "round-1");
    /// assert_eq!(goal.parts[0].tags.len(), 1);
    /// assert!(goal.grid.is_none());
    /// ```
    #[must_use]
    pub fn single_part(zone: ZoneName, search_window: Span, part: Part) -> Self {
        Self::new(zone, search_window, vec![part])
    }
}

/// One event within a goal, from the event's own rules to the travel the
/// booking needs.
///
/// ```
/// use opportune::model::{Part, Requirement, Term, Travel};
/// use opportune::tags::Tag;
/// use opportune::time::Duration;
///
/// let part = Part::new(
///     Duration::from_hours_minutes(0, 45),
///     Requirement::new(vec![Term::person("ana")]),
/// );
/// assert_eq!(part.id, "main");
/// assert_eq!(part.travel, Travel::default());
///
/// let round = part.with_id("round-1").with_tags(vec![Tag::new("interview").unwrap()]);
/// assert_eq!(round.id, "round-1");
/// assert_eq!(round.tags.len(), 1);
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Part {
    /// The part's ID, `main` unless the caller names another. Reasons cite it.
    pub id: String,
    /// What is being booked: decides which rules apply and which caps it counts
    /// toward.
    pub tags: Vec<Tag>,
    /// The length of the event.
    pub duration: Duration,
    /// The terms that decide who can take it.
    pub requirement: Requirement,
    /// Travel the new booking needs.
    pub travel: Travel,
}

impl Part {
    /// A part with the default ID, no tags and no travel.
    #[must_use]
    pub fn new(duration: Duration, requirement: Requirement) -> Self {
        Self {
            id: "main".to_owned(),
            tags: Vec::new(),
            duration,
            requirement,
            travel: Travel::default(),
        }
    }

    /// The part with its ID replaced.
    #[must_use]
    pub fn with_id(mut self, id: impl Into<String>) -> Self {
        self.id = id.into();
        self
    }

    /// The part tagged with `tags`, which decide the rules and caps that apply.
    #[must_use]
    pub fn with_tags(mut self, tags: Vec<Tag>) -> Self {
        self.tags = tags;
        self
    }

    /// The part needing `travel`.
    #[must_use]
    pub fn with_travel(mut self, travel: Travel) -> Self {
        self.travel = travel;
        self
    }
}

/// Who can take a part: a list of terms, each filled by a distinct host.
///
/// ```
/// use opportune::model::{Requirement, Term};
/// use opportune::tags::{Selector, Tag};
///
/// let requirement = Requirement::new(vec![
///     Term::person("ana"),
///     Term::at_least(1, Selector::Tag(Tag::new("kind/room").unwrap())),
/// ]);
/// assert_eq!(requirement.terms.len(), 2);
/// ```
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Requirement {
    /// The terms. A term naming one participant and a term for "at least n of a
    /// selector" are the two forms.
    pub terms: Vec<Term>,
}

impl Requirement {
    /// A requirement over `terms`.
    #[must_use]
    pub fn new(terms: Vec<Term>) -> Self {
        Self { terms }
    }

    /// The number of host slots the terms ask for, which is the sum of each
    /// term's count, saturated rather than wrapped: a term's count is a `u32` a
    /// caller supplies, and on a target where `usize` is 32 bits two of them at
    /// the bound exceed it. The sum only ever decides whether a requirement asks
    /// for no host at all, which saturation preserves.
    #[must_use]
    pub fn slots(&self) -> usize {
        self.terms
            .iter()
            .map(Term::slots)
            .fold(0_usize, usize::saturating_add)
    }
}

/// One clause of a requirement.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Term {
    /// One named host, by participant ID.
    Person {
        /// The participant's ID.
        id: String,
    },
    /// At least `count` hosts among the participants the selector matches.
    AtLeast {
        /// How many hosts the term needs.
        count: u32,
        /// Which participants can fill it.
        of: Selector,
    },
}

impl Term {
    /// A term filled by the participant with this ID.
    #[must_use]
    pub fn person(id: impl Into<String>) -> Self {
        Self::Person { id: id.into() }
    }

    /// A term needing `count` hosts among the participants `of` selects.
    #[must_use]
    pub fn at_least(count: u32, of: Selector) -> Self {
        Self::AtLeast { count, of }
    }

    /// The number of distinct hosts the term needs: one for a named person, and
    /// the named count for a selector term.
    #[must_use]
    pub fn slots(&self) -> usize {
        match self {
            Self::Person { .. } => 1,
            Self::AtLeast { count, .. } => usize::try_from(*count).unwrap_or(usize::MAX),
        }
    }

    /// Whether `participant` can fill this term: a named person matches by ID, and
    /// a selector term needs a host the selector matches. Only a host fills a term
    /// either way, because an attendee is never assigned a booking. Whether the
    /// request carries the participant at all is `validate`'s question.
    #[must_use]
    pub fn filled_by(&self, participant: &Participant) -> bool {
        if participant.role != Role::Host {
            return false;
        }
        match self {
            Self::Person { id } => participant.id == *id,
            Self::AtLeast { of, .. } => of.matches_participant(participant.context()),
        }
    }
}

/// Travel the new booking needs, in each direction, with per-participant
/// overrides.
///
/// Travel is occupied time: it counts against windows, blackouts and blocks like
/// the booking itself, and an offer returns it as spans to reserve.
///
/// ```
/// use opportune::model::Travel;
/// use opportune::time::Duration;
///
/// let travel = Travel::new(Duration::from_hours_minutes(0, 20), Duration::from_hours_minutes(0, 20))
///     // A host with a longer journey overrides both directions at once.
///     .with_participant(
///         "tom",
///         Duration::from_hours_minutes(0, 35),
///         Duration::from_hours_minutes(0, 35),
///     );
/// assert_eq!(travel.for_participant("tom").0.minutes(), 35);
/// assert_eq!(travel.for_participant("ana").0.minutes(), 20);
/// ```
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Travel {
    /// Travel needed before the event, for participants without an override.
    pub before: Duration,
    /// Travel needed after the event, for participants without an override.
    pub after: Duration,
    /// Per-participant travel, keyed by participant ID. Each entry replaces
    /// both directions for that participant.
    pub by_participant: BTreeMap<String, ParticipantTravel>,
}

impl Travel {
    /// Travel of `before` and `after` for every participant without an override.
    #[must_use]
    pub fn new(before: Duration, after: Duration) -> Self {
        Self {
            before,
            after,
            by_participant: BTreeMap::new(),
        }
    }

    /// The travel with `participant`'s own `before` and `after`, replacing an
    /// entry the participant already had.
    #[must_use]
    pub fn with_participant(
        mut self,
        id: impl Into<String>,
        before: Duration,
        after: Duration,
    ) -> Self {
        self.by_participant
            .insert(id.into(), ParticipantTravel { before, after });
        self
    }

    /// The travel needed by `participant`, as `(before, after)`: the
    /// participant's own entry when it has one, the part's travel otherwise.
    #[must_use]
    pub fn for_participant(&self, participant: &str) -> (Duration, Duration) {
        match self.by_participant.get(participant) {
            Some(travel) => (travel.before, travel.after),
            None => (self.before, self.after),
        }
    }

    /// The largest travel before the event across every travel the part
    /// defines: the part's default folded together with every participant
    /// override, so it includes the default even when every participant
    /// overrides it. That conservativeness is what the judged range pads by; a
    /// search's request-wide envelope is narrower, folding only the effective
    /// travel of the participants a request actually names.
    #[must_use]
    pub fn largest_before(&self) -> Duration {
        self.by_participant
            .values()
            .map(|travel| travel.before)
            .fold(self.before, Duration::max)
    }

    /// The largest travel after the event across every travel the part
    /// defines: the part's default folded together with every participant
    /// override, so it includes the default even when every participant
    /// overrides it. That conservativeness is what the judged range pads by; a
    /// search's request-wide envelope is narrower, folding only the effective
    /// travel of the participants a request actually names.
    #[must_use]
    pub fn largest_after(&self) -> Duration {
        self.by_participant
            .values()
            .map(|travel| travel.after)
            .fold(self.after, Duration::max)
    }

    /// The largest effective travel before the event across `participants`,
    /// each read through [`Travel::for_participant`]: an override replaces the
    /// part's travel for the participant who holds it, so a default nobody
    /// names widens nothing. An empty slice falls back to
    /// [`Travel::largest_before`], the travel the part could still need when no
    /// participant narrows it.
    pub(crate) fn largest_before_for_participants(&self, participants: &[Participant]) -> Duration {
        match participants.split_first() {
            None => self.largest_before(),
            Some((first, rest)) => rest.iter().fold(
                self.for_participant(first.id.as_str()).0,
                |largest, participant| largest.max(self.for_participant(participant.id.as_str()).0),
            ),
        }
    }

    /// The largest effective travel after the event across `participants`, each
    /// read through [`Travel::for_participant`]. An empty slice falls back to
    /// [`Travel::largest_after`], the travel the part could still need when no
    /// participant narrows it.
    pub(crate) fn largest_after_for_participants(&self, participants: &[Participant]) -> Duration {
        match participants.split_first() {
            None => self.largest_after(),
            Some((first, rest)) => rest.iter().fold(
                self.for_participant(first.id.as_str()).1,
                |largest, participant| largest.max(self.for_participant(participant.id.as_str()).1),
            ),
        }
    }
}

/// One participant's travel, overriding the part's travel in both directions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ParticipantTravel {
    /// Travel needed before the event.
    pub before: Duration,
    /// Travel needed after the event.
    pub after: Duration,
}

/// A span of instants during which a participant is in another zone.
///
/// The span is half-open, `[from, until)`, so adjacent legs of a trip join
/// exactly. A participant's zone at an instant is the period covering it, or
/// their home zone.
///
/// ```
/// use opportune::model::ZonePeriod;
/// use opportune::time::{Instant, ZoneName};
///
/// let period = ZonePeriod::new(
///     ZoneName::new("America/Anchorage").unwrap(),
///     Instant::MIN,
///     Instant::MAX,
/// ).unwrap();
/// assert!(period.contains(Instant::MIN));
/// assert!(!period.contains(Instant::MAX));
/// assert_eq!(period.zone().as_str(), "America/Anchorage");
/// assert_eq!(period.from(), Instant::MIN);
/// assert_eq!(period.until(), Instant::MAX);
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ZonePeriod {
    zone: ZoneName,
    from: Instant,
    until: Instant,
}

impl ZonePeriod {
    /// Creates the period `[from, until)`.
    ///
    /// # Errors
    /// Returns [`Error::ZonePeriodEmpty`] when `from` is not before `until`.
    pub fn new(zone: ZoneName, from: Instant, until: Instant) -> Result<Self, Error> {
        if from >= until {
            return Err(Error::ZonePeriodEmpty { from, until });
        }
        Ok(Self { zone, from, until })
    }

    /// The zone the participant is in during this period.
    #[must_use]
    pub fn zone(&self) -> &ZoneName {
        &self.zone
    }

    /// The first instant of the period.
    #[must_use]
    pub const fn from(&self) -> Instant {
        self.from
    }

    /// One past the last instant of the period.
    #[must_use]
    pub const fn until(&self) -> Instant {
        self.until
    }

    /// Whether the period covers `instant`, half-open.
    #[must_use]
    pub fn contains(&self, instant: Instant) -> bool {
        self.from <= instant && instant < self.until
    }
}

#[cfg(test)]
mod tests {
    use super::{Term, Travel};
    use crate::model::Participant;
    use crate::tags::{Role, Selector};
    use crate::time::{Duration, ZoneName};

    /// Only a host fills a term: an attendee is never assigned a booking, so a
    /// named term and a selector term alike reject one however its ID or tags
    /// read.
    #[test]
    fn an_attendee_fills_no_term() {
        let mut participant = Participant::new("tom", ZoneName::new("UTC").unwrap());
        assert!(Term::person("tom").filled_by(&participant));
        assert!(Term::at_least(1, Selector::True).filled_by(&participant));

        participant.role = Role::Attendee;
        assert!(!Term::person("tom").filled_by(&participant));
        assert!(!Term::at_least(1, Selector::True).filled_by(&participant));
    }

    /// The builders write the whole travel document, and an override replaces only
    /// the participant it names.
    #[test]
    fn travel_builders_add_one_participant_beside_the_shared_travel() {
        let travel = Travel::new(Duration::from_minutes(15), Duration::from_minutes(20))
            .with_participant(
                "tom",
                Duration::from_minutes(35),
                Duration::from_minutes(40),
            );

        assert_eq!(
            travel.for_participant("tom"),
            (Duration::from_minutes(35), Duration::from_minutes(40))
        );
        assert_eq!(
            travel.for_participant("ana"),
            (Duration::from_minutes(15), Duration::from_minutes(20))
        );
        assert_eq!(travel.largest_before(), Duration::from_minutes(35));
        assert_eq!(travel.largest_after(), Duration::from_minutes(40));
    }

    /// The judged-range methods remain conservative when the shared default is
    /// larger than every override, rather than taking only the override map's
    /// maximum.
    #[test]
    fn part_wide_travel_includes_a_default_larger_than_every_override() {
        let travel = Travel::new(Duration::from_minutes(35), Duration::from_minutes(40))
            .with_participant("tom", Duration::ZERO, Duration::ZERO);

        assert_eq!(travel.largest_before(), Duration::from_minutes(35));
        assert_eq!(travel.largest_after(), Duration::from_minutes(40));
        assert_eq!(
            travel.largest_before_for_participants(&[Participant::new(
                "tom",
                ZoneName::new("UTC").unwrap(),
            )]),
            Duration::ZERO
        );
        assert_eq!(
            travel.largest_after_for_participants(&[Participant::new(
                "tom",
                ZoneName::new("UTC").unwrap(),
            )]),
            Duration::ZERO
        );
    }

    /// The request-wide envelope folds each request participant's effective
    /// travel; with no participants it falls back to the part-wide maximum,
    /// which folds the default in with every override map entry.
    #[test]
    fn the_request_wide_envelope_falls_back_to_the_part_wide_maximum_without_participants() {
        let travel = Travel::new(Duration::from_minutes(1), Duration::from_minutes(2))
            .with_participant(
                "tom",
                Duration::from_minutes(35),
                Duration::from_minutes(40),
            );

        assert_eq!(
            travel.largest_before_for_participants(&[]),
            travel.largest_before()
        );
        assert_eq!(
            travel.largest_after_for_participants(&[]),
            travel.largest_after()
        );
        assert_eq!(travel.largest_before(), Duration::from_minutes(35));
        assert_eq!(travel.largest_after(), Duration::from_minutes(40));

        // A participant without an override contributes the part default.
        let solo = [Participant::new("ana", ZoneName::new("UTC").unwrap())];
        assert_eq!(
            travel.largest_before_for_participants(&solo),
            Duration::from_minutes(1)
        );
        assert_eq!(
            travel.largest_after_for_participants(&solo),
            Duration::from_minutes(2)
        );

        // An override replaces the default for the participant who holds it.
        let both = [
            solo[0].clone(),
            Participant::new("tom", ZoneName::new("UTC").unwrap()),
        ];
        assert_eq!(
            travel.largest_before_for_participants(&both),
            Duration::from_minutes(35)
        );
        assert_eq!(
            travel.largest_after_for_participants(&both),
            Duration::from_minutes(40)
        );
    }
}
