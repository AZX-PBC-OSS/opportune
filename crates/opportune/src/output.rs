//! Response shaping: offers in start order, their date counts before the limit
//! truncates them, the exclusions the request asked to have explained, and the
//! answer to checking one proposed booking.

use std::collections::BTreeMap;

use crate::Engine;
use crate::error::{Diagnostic, Error};
use crate::eval::{
    MatchingCache, Settings, Wanted, any_term_short, eligible_per_term, exclusion_reasons,
    failing_reasons, start_eligibility,
};
use crate::intervals::Span as MinuteSpan;
use crate::model::{
    Offer, OfferEvent, OfferFlag, ReserveKind, ReserveSpan, Response, SCHEMA_VERSION, SlotCheck,
    Span,
};
use crate::reasons::{Excluded, Reason, ReasonCode};
use crate::time::{Date, Instant};

/// Builds the response: offers in start order, their date counts before the limit
/// truncates the list, the exclusions the request asked to have explained, and
/// the versions the answer came from.
pub(crate) fn response(
    engine: &Engine,
    request: &crate::model::Request,
    settings: &Settings<'_>,
    warnings: Vec<Diagnostic>,
) -> Result<Response, Error> {
    let mut explain = request.explain.as_ref();
    let limit = request.limit();
    let mut offers = Vec::new();
    let mut starts = Vec::new();
    let mut truncated = false;
    let mut excluded = Vec::new();
    let mut reason_count = 0_usize;
    let mut excluded_truncated = false;

    let mut eligible = Vec::new();
    let mut failed = Vec::new();
    // Reuses the joint matching answer for consecutive starts whose eligible hosts
    // are the same, which is what a run of unchanged calendars produces.
    let mut matching = MatchingCache::default();

    let duration = settings.part.duration.minutes();
    for start in &settings.candidates {
        // The booking's own span is the same for every participant, so it is
        // built once per start rather than once per participant per start.
        // Validation rejects a part with no length, so this cannot be empty.
        let own = MinuteSpan::new(start.minutes(), start.minutes().saturating_add(duration))?;
        // One pass over the participants the offer depends on decides who can
        // take the start.
        start_eligibility(settings, *start, &own, &mut eligible, &mut failed);
        let short = any_term_short(settings, &eligible);
        let attendee_blocked = settings.attendees.iter().any(|index| !eligible[*index]);
        // Whether the offer list is already full, so this start can only add to
        // the date counts and to the explanation.
        let past_limit = offers.len() >= limit;

        // Past the limit or when an attendee already blocks the offer, no
        // concrete assignment is reported, so only joint fillability is needed
        // (avoiding host-ID cloning and full lexicographic tie-breaking).
        let (assignment, fillable) = if short {
            (None, false)
        } else if past_limit || attendee_blocked {
            (None, matching.can_fill(settings, &eligible))
        } else {
            let assignment = matching.assignment_at(settings, &eligible);
            let fillable = assignment.is_some();
            (assignment, fillable)
        };
        if fillable && !attendee_blocked {
            if let Some(assignment) = assignment {
                let assigned: Vec<&str> = assignment.iter().flatten().map(String::as_str).collect();
                let flags = calendar_flags(settings, &assigned);
                let reserve = travel_reserve(settings, *start, &assigned)?;
                let eligible_hosts = eligible_per_term(settings, &eligible);
                let event = offer_event(settings, *start, eligible_hosts, assignment, reserve)?;
                offers.push(Offer {
                    events: vec![event],
                    flags,
                });
            } else {
                truncated = true;
            }
            starts.push(*start);
            continue;
        }
        let Some(limits) = explain else {
            continue;
        };
        // A term short of eligible hosts is already explained by each blocked
        // host's first failing reason, so a start-level reason is added only when
        // no participant's reason says it: the joint case, where every term has
        // enough eligible hosts and no assignment fits them together, and the
        // case where the requirement fits but an attendee does not.
        let start_reason = match (fillable, attendee_blocked) {
            (false, _) if !short => Some(ReasonCode::TermUnfillable),
            (true, true) => Some(ReasonCode::AttendeeUnavailable),
            _ => None,
        };
        // The start-level reason comes last, after every participant's first
        // failing reason in ascending ID order, which is what the response
        // contract's exclusion entry describes.
        let mut reasons = exclusion_reasons(settings, *start, &own, &failed);
        if let Some(code) = start_reason {
            reasons.push(Reason::start_level(code));
        }
        // Entries are atomic: a start is reported whole or not at all, and the
        // explanation stops at the first one that would not fit. The search goes
        // on, because which starts a caller can book is not the explanation's to
        // truncate.
        if reason_count.saturating_add(reasons.len()) > limits.limit {
            excluded_truncated = true;
            explain = None;
            continue;
        }
        reason_count = reason_count.saturating_add(reasons.len());
        excluded.push(Excluded {
            start: *start,
            part: settings.part.id.clone(),
            reasons,
        });
    }

    let dates = date_counts(settings, &starts)?;

    Ok(Response {
        offers,
        dates,
        truncated,
        excluded,
        excluded_truncated,
        warnings,
        meta: meta(engine),
    })
}

/// The answer to checking one proposed booking: every reason it fails, the flags
/// and reserve spans a confirmed booking would carry, and the versions it was
/// judged against.
///
/// Every assigned host and every attendee is judged, in ascending participant ID
/// order, so a proposal's answer does not depend on the order the assignment was
/// written in.
///
/// # Errors
/// Returns [`Error::MalformedProposal`] for an assigned host the request does not
/// carry, rather than answering for a booking nobody takes. Proposal validation
/// reports the same failure before evaluation, so this is the guard that keeps a
/// missing host from reading as a valid slot. Returns an [`Error::Time`] failure
/// for a reserve span the model cannot represent, which proposal validation
/// rejects as out of range before the booking reaches this stage.
pub(crate) fn slot_check(
    engine: &Engine,
    settings: &Settings<'_>,
    start: Instant,
    assignment: &[&str],
    warnings: Vec<Diagnostic>,
) -> Result<SlotCheck, Error> {
    let mut judged: Vec<&str> = settings
        .attendees
        .iter()
        .map(|index| settings.participants[*index].participant.id.as_str())
        .collect();
    judged.extend_from_slice(assignment);
    judged.sort_unstable();
    judged.dedup();

    let mut reasons = Vec::new();
    let mut scratch = Vec::new();
    // A part with no length is a validation error, reported before this runs, so
    // the span is built rather than guessed at.
    let own = MinuteSpan::new(
        start.minutes(),
        start
            .minutes()
            .saturating_add(settings.part.duration.minutes()),
    )?;
    for id in judged {
        let Some(participant) = settings
            .participants
            .iter()
            .find(|participant| participant.participant.id == id)
        else {
            return Err(Error::malformed_proposal(
                "proposal_host_unknown",
                format!("the request carries no participant `{id}`"),
                "/events".to_owned(),
            ));
        };
        scratch.clear();
        failing_reasons(participant, start, &own, Wanted::All, &mut scratch);
        reasons.append(&mut scratch);
    }
    Ok(SlotCheck {
        valid: reasons.is_empty(),
        reasons,
        flags: calendar_flags(settings, assignment),
        reserve: travel_reserve(settings, start, assignment)?,
        warnings,
        meta: meta(engine),
    })
}

/// The versions every answer reports, so a stored answer can be recognised later.
fn meta(engine: &Engine) -> crate::model::Meta {
    crate::model::Meta {
        engine: crate::VERSION.to_owned(),
        schema: SCHEMA_VERSION,
        tzdb: engine.time_zone_database().version().to_owned(),
    }
}

/// The flags an assignment carries: `calendar_unknown` when an assigned host has
/// no calendar, so their availability is unknown.
fn calendar_flags(settings: &Settings<'_>, assigned: &[&str]) -> Vec<OfferFlag> {
    if settings.unknown_calendars.is_empty() {
        return Vec::new();
    }
    let unknown = assigned
        .iter()
        .any(|id| settings.unknown_calendars.contains(*id));
    if unknown {
        vec![OfferFlag::CalendarUnknown]
    } else {
        Vec::new()
    }
}

/// The travel spans the booking occupies, for every participant it needs, sorted
/// by participant ID then start.
///
/// The booking's occupied span is widened by *each* participant's travel, so an
/// attendee's travel is occupied time too: it is held like an assigned host's,
/// even though an attendee is never assigned.
///
/// Every span is built rather than dropped when it does not fit: a search withholds
/// a start whose request-wide travel envelope is unrepresentable and a
/// confirmation rejects one whose assigned hosts' or attendees' travel is, so a
/// failure here means the two entry points disagreed. It is surfaced with its own
/// cause rather than silently leaving a required hold out of the answer.
///
/// # Errors
/// Returns the [`crate::time::TimeError`] of an occupied range that has no
/// representable instant at either edge.
fn travel_reserve(
    settings: &Settings<'_>,
    start: Instant,
    assigned: &[&str],
) -> Result<Vec<ReserveSpan>, Error> {
    if !settings.has_any_travel {
        return Ok(Vec::new());
    }
    let mut reserve = Vec::new();
    let end = start
        .minutes()
        .saturating_add(settings.part.duration.minutes());
    let attendees = settings
        .attendees
        .iter()
        .map(|index| settings.participants[*index].participant.id.as_str());
    for id in assigned.iter().copied().chain(attendees) {
        let (travel_before, travel_after) = settings.part.travel.for_participant(id);
        if travel_before.minutes() > 0 {
            let from =
                Instant::from_minutes(start.minutes().saturating_sub(travel_before.minutes()))?;
            reserve.push(ReserveSpan {
                participant: id.to_owned(),
                kind: ReserveKind::Travel,
                span: Span::new(from, start)?,
            });
        }
        if travel_after.minutes() > 0 {
            let from = Instant::from_minutes(end)?;
            let until = Instant::from_minutes(end.saturating_add(travel_after.minutes()))?;
            reserve.push(ReserveSpan {
                participant: id.to_owned(),
                kind: ReserveKind::Travel,
                span: Span::new(from, until)?,
            });
        }
    }
    reserve.sort_by(|left, right| {
        left.participant
            .cmp(&right.participant)
            .then(left.span.start().cmp(&right.span.start()))
    });
    Ok(reserve)
}

/// One offer's event: its span, the eligible hosts per term, the chosen
/// assignment and the travel to reserve.
fn offer_event(
    settings: &Settings<'_>,
    start: Instant,
    eligible: Vec<Vec<String>>,
    assignment: Vec<Vec<String>>,
    reserve: Vec<ReserveSpan>,
) -> Result<OfferEvent, Error> {
    let end = Instant::from_minutes(start.minutes() + settings.part.duration.minutes())?;
    Ok(OfferEvent {
        part: settings.part.id.clone(),
        start,
        end,
        eligible,
        assignment,
        reserve,
    })
}

/// Every civil date in the display zone the search window touches, with the
/// number of offers starting on it.
///
/// The window is half-open, so it touches local dates up to the minute before its
/// end: a window ending at local midnight does not add the following day. Counts
/// come from every offer, before the limit truncates the list.
///
/// The window's first and last local dates are not always the dates of its two
/// endpoints: a backward offset transition that repeats a midnight can leave the
/// last minute of the window on an earlier local date than its first, as
/// Newfoundland's historical fall-back did. The zone answers for the bounds over
/// the whole window, and a local date that no instant falls on — one skipped by
/// crossing the date line — is left out.
fn date_counts(
    settings: &Settings<'_>,
    starts: &[Instant],
) -> Result<Vec<crate::model::DateCount>, Error> {
    let mut by_date: BTreeMap<Date, usize> = BTreeMap::new();
    for &start in starts {
        *by_date
            .entry(settings.display.zoned(start).date())
            .or_default() += 1;
    }
    let (mut date, last) = settings
        .display
        .local_date_bounds(settings.window.start(), settings.window.end())?;
    let mut counts = Vec::new();
    loop {
        if settings.display.date_exists(date) {
            let offers = by_date.get(&date).copied().unwrap_or(0);
            counts.push(crate::model::DateCount { date, offers });
        }
        if date >= last {
            break;
        }
        date = date.plus_days(1)?;
    }
    Ok(counts)
}
