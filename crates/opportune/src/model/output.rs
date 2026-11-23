//! What a search returns: offers in start order, date counts, the exclusions the
//! request asked to have explained, the versions the answer was produced from, and
//! the answer to checking one proposed booking.

use crate::error::Diagnostic;
use crate::model::{Proposal, ProposalEvent, Span};
use crate::reasons::{Excluded, Reason};
use crate::time::{Date, Instant};

/// One bookable time, with who could take it and who was chosen.
///
/// The engine produces an offer; a caller reads it, and hands it back for
/// confirmation with [`Offer::to_proposal`].
///
/// ```
/// # #[cfg(feature = "bundled-tzdb")] {
/// use opportune::Engine;
/// use opportune::model::{Goal, Participant, Request, Requirement, Span, Term};
/// use opportune::time::{Duration, Instant, ZoneName};
///
/// let zone = ZoneName::new("UTC").unwrap();
/// let window = Span::new(Instant::MIN, Instant::from_minutes(Instant::MIN.minutes() + 60).unwrap()).unwrap();
/// let goal = Goal::single(
///     zone.clone(),
///     window,
///     Duration::from_minutes(30),
///     Requirement::new(vec![Term::person("ana")]),
/// );
/// let mut request = Request::new(Instant::MIN, goal);
/// request.participants.push(Participant::new("ana", zone).with_calendar(vec![]));
///
/// let response = Engine::builder().build().unwrap().find_availability(&request).unwrap();
/// let offer = &response.offers[0];
/// assert_eq!(offer.start(), Some(Instant::MIN));
/// assert_eq!(offer.end(), Some(Instant::from_minutes(Instant::MIN.minutes() + 30).unwrap()));
/// assert_eq!(offer.assigned_hosts(), ["ana"]);
/// assert!(!offer.calendar_unknown());
/// # }
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Offer {
    /// One event per goal part, each with its own assignment.
    pub events: Vec<OfferEvent>,
    /// What the caller should know about the offer.
    pub flags: Vec<OfferFlag>,
}

impl Offer {
    /// The offer's start: the start of its first event, since version 1 has one
    /// part.
    #[must_use]
    pub fn start(&self) -> Option<Instant> {
        self.events.first().map(|event| event.start)
    }

    /// The offer's end: the end of its last event.
    #[must_use]
    pub fn end(&self) -> Option<Instant> {
        self.events.last().map(|event| event.end)
    }

    /// The hosts the offer assigns, one per slot, in term and ID order.
    #[must_use]
    pub fn assigned_hosts(&self) -> Vec<&str> {
        self.events
            .iter()
            .flat_map(|event| event.assignment.iter().flatten().map(String::as_str))
            .collect()
    }

    /// The spans a confirmed offer would hold on its participants' calendars,
    /// in event order.
    ///
    /// Travel is part of the booking the caller must write, so this is what to
    /// put on each participant's calendar beside the events themselves.
    #[must_use]
    pub fn reserve(&self) -> Vec<&ReserveSpan> {
        self.events
            .iter()
            .flat_map(|event| event.reserve.iter())
            .collect()
    }

    /// Whether a host assigned to this offer has no calendar, so the engine could
    /// not tell whether they are free.
    #[must_use]
    pub fn calendar_unknown(&self) -> bool {
        self.flags.contains(&OfferFlag::CalendarUnknown)
    }

    /// The booking this offer describes, as the proposal
    /// [`Engine::check_slot`](crate::Engine::check_slot) confirms.
    ///
    /// ```
    /// # #[cfg(feature = "bundled-tzdb")] {
    /// use opportune::Engine;
    /// use opportune::model::{Goal, Participant, Request, Requirement, Span, Term};
    /// use opportune::time::{Duration, Instant, ZoneName};
    ///
    /// let zone = ZoneName::new("UTC").unwrap();
    /// let window = Span::new(Instant::MIN, Instant::from_minutes(Instant::MIN.minutes() + 60).unwrap()).unwrap();
    /// let goal = Goal::single(
    ///     zone.clone(),
    ///     window,
    ///     Duration::from_minutes(30),
    ///     Requirement::new(vec![Term::person("ana")]),
    /// );
    /// let mut request = Request::new(Instant::MIN, goal);
    /// request
    ///     .participants
    ///     .push(Participant::new("ana", zone).with_calendar(Vec::new()));
    ///
    /// let engine = Engine::builder().build().unwrap();
    /// let offer = engine.find_availability(&request).unwrap().offers[0].clone();
    /// let proposal = offer.to_proposal();
    /// assert_eq!(proposal.events[0].part, "main");
    /// assert!(engine.check_slot(&request, &proposal).unwrap().valid);
    /// # }
    /// ```
    #[must_use]
    pub fn to_proposal(&self) -> Proposal {
        Proposal {
            events: self
                .events
                .iter()
                .map(|event| ProposalEvent {
                    part: event.part.clone(),
                    start: event.start,
                    assignment: event.assignment.clone(),
                })
                .collect(),
        }
    }
}

/// One booked event: its part, its span, who could take it and who was chosen.
///
/// The engine produces the event inside an [`Offer`].
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct OfferEvent {
    /// The goal part's ID.
    pub part: String,
    /// The booking's start.
    pub start: Instant,
    /// The booking's end.
    pub end: Instant,
    /// Who could fill each term, sorted by ID. One list per term.
    pub eligible: Vec<Vec<String>>,
    /// The chosen assignment: one list per term, sorted by ID.
    pub assignment: Vec<Vec<String>>,
    /// Travel every participant the booking needs occupies, to be reserved on
    /// the calendars: an assigned host's and an attendee's alike.
    pub reserve: Vec<ReserveSpan>,
}

/// One span of time the caller should hold on a participant's calendar.
///
/// The engine returns these in an [`OfferEvent::reserve`].
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct ReserveSpan {
    /// The participant to hold it for.
    pub participant: String,
    /// What the hold is for.
    pub kind: ReserveKind,
    /// The span to hold.
    pub span: Span,
}

/// What a reserve span is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ReserveKind {
    /// Travel to or from the booking.
    Travel,
}

impl ReserveKind {
    /// The wire name of the kind.
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Travel => "travel",
        }
    }
}

impl core::fmt::Display for ReserveKind {
    /// Formats the kind as its wire name.
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Something the caller should know about an offer that does not make it
/// invalid.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum OfferFlag {
    /// An assigned host has no calendar, so their availability is unknown: they
    /// are treated as free within their rules.
    CalendarUnknown,
}

impl OfferFlag {
    /// The wire name of the flag.
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::CalendarUnknown => "calendar_unknown",
        }
    }
}

impl core::fmt::Display for OfferFlag {
    /// Formats the flag as its wire name.
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// How many offers fall on one civil date in the display zone.
///
/// Counts are computed before the offer limit truncates the list, so a calendar
/// picker is never told a day is empty when it is only cut off.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct DateCount {
    /// The date in the display zone.
    pub date: Date,
    /// How many offers start on it.
    pub offers: usize,
}

/// What a search returns.
///
/// ```
/// use opportune::model::Response;
///
/// let response = Response::default();
/// assert!(response.offers.is_empty());
/// assert!(response.excluded.is_empty());
/// assert!(!response.truncated);
/// assert!(!response.excluded_truncated);
/// ```
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct Response {
    /// The bookable times, in start order.
    pub offers: Vec<Offer>,
    /// Every civil date in the display zone the search window touches, with its
    /// offer count.
    pub dates: Vec<DateCount>,
    /// Whether the offer limit cut the list short.
    pub truncated: bool,
    /// The candidate starts that were not offered, in start order. Empty unless
    /// the request asked for an explanation.
    pub excluded: Vec<Excluded>,
    /// Whether the `explain.limit` cut the exclusion list short. Always `false`
    /// without an explanation.
    pub excluded_truncated: bool,
    /// Request warnings, such as a window no candidate's booking fits inside.
    pub warnings: Vec<Diagnostic>,
    /// The versions the answer was produced from.
    pub meta: Meta,
}

/// The versions every response reports, so a stored answer can be recognised
/// later.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct Meta {
    /// The engine's version.
    pub engine: String,
    /// The schema version of the response shape.
    pub schema: u32,
    /// The time zone database's version.
    pub tzdb: String,
}

/// The response schema version this crate produces.
pub const SCHEMA_VERSION: u32 = 1;

/// The answer to checking one proposed booking: whether it can be booked, and if
/// not, why not.
///
/// `reasons` carries every failing reason, whether or not the request asked for
/// an explanation, so a caller confirming a booking always learns what stands in
/// the way. The engine returns it from
/// [`Engine::check_slot`](crate::Engine::check_slot).
///
/// ```
/// # #[cfg(feature = "bundled-tzdb")] {
/// use opportune::Engine;
/// use opportune::model::{Goal, Participant, Request, Requirement, Span, Term};
/// use opportune::time::{Duration, Instant, ZoneName};
///
/// let zone = ZoneName::new("UTC").unwrap();
/// let window = Span::new(Instant::MIN, Instant::from_minutes(Instant::MIN.minutes() + 60).unwrap()).unwrap();
/// let goal = Goal::single(
///     zone.clone(),
///     window,
///     Duration::from_minutes(30),
///     Requirement::new(vec![Term::person("ana")]),
/// );
/// let mut request = Request::new(Instant::MIN, goal);
/// request
///     .participants
///     .push(Participant::new("ana", zone).with_calendar(Vec::new()));
///
/// let engine = Engine::builder().build().unwrap();
/// let offer = engine.find_availability(&request).unwrap().offers[0].clone();
/// let check = engine.check_slot(&request, &offer.to_proposal()).unwrap();
/// assert!(check.valid);
/// assert!(check.reasons.is_empty());
/// # }
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct SlotCheck {
    /// Whether the booking can be made as proposed.
    pub valid: bool,
    /// Every reason the booking fails: each judged participant's reasons in the
    /// canonical precedence order, grouped by participant ID ascending.
    pub reasons: Vec<Reason>,
    /// What the caller should know about the booking that does not make it
    /// invalid.
    pub flags: Vec<OfferFlag>,
    /// Travel every participant the booking needs occupies, to be reserved on
    /// the calendars: an assigned host's and an attendee's alike.
    pub reserve: Vec<ReserveSpan>,
    /// The request validation warnings that do not depend on candidate
    /// generation: the request-scoped passes (shape, rules, horizons) produce
    /// the same findings [`Engine::validate`](crate::Engine::validate) reports.
    /// The search-only `no_candidate_in_window` warning is absent, because
    /// `check_slot` judges one proposed start and ignores the grid and the search
    /// window.
    pub warnings: Vec<Diagnostic>,
    /// The versions the answer was produced from.
    pub meta: Meta,
}

#[cfg(test)]
mod tests {
    use super::{Offer, OfferEvent, ReserveKind, ReserveSpan};
    use crate::model::Span;
    use crate::time::Instant;

    fn instant(minute: i64) -> Instant {
        Instant::from_minutes(Instant::MIN.minutes() + minute).unwrap()
    }

    fn hold(participant: &str, from: i64, until: i64) -> ReserveSpan {
        ReserveSpan {
            participant: participant.to_owned(),
            kind: ReserveKind::Travel,
            span: Span::new(instant(from), instant(until)).unwrap(),
        }
    }

    /// The accessor flattens every event's reserve, so a caller writes the holds
    /// for a multi-part offer without reaching into `events[0]`.
    #[test]
    fn an_offers_reserve_flattens_every_event_in_order() {
        let offer = Offer {
            events: vec![
                OfferEvent {
                    part: "first".to_owned(),
                    start: instant(60),
                    end: instant(90),
                    eligible: Vec::new(),
                    assignment: Vec::new(),
                    reserve: vec![hold("ana", 30, 60)],
                },
                OfferEvent {
                    part: "second".to_owned(),
                    start: instant(120),
                    end: instant(150),
                    eligible: Vec::new(),
                    assignment: Vec::new(),
                    reserve: vec![hold("tom", 90, 120), hold("tom", 150, 180)],
                },
            ],
            flags: Vec::new(),
        };

        let reserved = offer.reserve();
        assert_eq!(
            reserved
                .iter()
                .map(|span| span.participant.as_str())
                .collect::<Vec<_>>(),
            ["ana", "tom", "tom"]
        );
        assert_eq!(reserved[2].span.end(), instant(180));
    }
}
