//! The availability pipeline: resolve a request against an engine, combine the
//! applicable rules per participant, and hand the resolved settings to the stage
//! that answers with them.
//!
//! This module owns the request-scoped resolution: the judged range, each
//! participant's applicable rules, their allowed and blocked time, the strictest
//! `lead_time` maximum, the candidate starts and the requirement's terms. The
//! stages that decide eligibility live in [`eligibility`], rest evaluation in
//! [`crate::rest`], the interval sets rules compile to in [`allowed`] and
//! [`blocked`], candidate generation in [`candidates`], and response shaping in
//! [`crate::output`].
//!
//! Every stage is a pure function of the settings resolved here. One function
//! decides why a participant cannot take a start, and both eligibility and the
//! reported reasons read it, so a search and a `check_slot` cannot disagree about
//! a start.

mod allowed;
mod blocked;
mod candidates;
mod eligibility;

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

pub(crate) use allowed::{FixedMinutes, WindowContext};

use crate::Engine;
use crate::error::Error;
use crate::intervals::{IntervalSet, Span as MinuteSpan};
use crate::model::{Goal, LeadTimeMax, Part, Participant};
use crate::rest::RestMemo;
use crate::rules::{Applicable, RuleMatcher};
use crate::tags::Role;
use crate::time::{
    Horizon, Instant, StartBound, TimeZoneDatabase, WorkingCalendar, Zone, ZoneSchedule,
};

pub(crate) use blocked::{BlockedIndex, BlockedSpan};
pub(crate) use candidates::candidate_starts_limited;
pub(crate) use eligibility::{
    MatchingCache, Wanted, any_term_short, eligible_per_term, exclusion_reasons, failing_reasons,
    start_eligibility,
};

/// Request-scoped resolution products already computed during validation,
/// forwarded to evaluation so `find_availability` and `check_slot` never
/// re-resolve calendars, zone schedules, rule selections, or lead bounds.
pub(crate) struct PreResolved<'a> {
    pub(crate) calendars: BTreeMap<String, WorkingCalendar>,
    pub(crate) schedules: Vec<ZoneSchedule>,
    pub(crate) rules: Vec<Applicable<'a>>,
    pub(crate) lead_bounds: BTreeMap<usize, StartBound>,
}

/// Everything the pipeline needs, resolved once per request.
pub(crate) struct Settings<'a> {
    pub(crate) part: &'a Part,
    pub(crate) display: Zone,
    /// The search window: which starts a search considers, and whose local dates
    /// the response reports.
    pub(crate) window: crate::model::Span,
    pub(crate) candidates: Vec<Instant>,
    pub(crate) participants: Vec<ParticipantSettings<'a>>,
    /// The requirement's terms, in request order, each with the participant
    /// indices that can fill it, ascending by ID.
    pub(crate) terms: Vec<ResolvedTerm>,
    /// Whether no participant fills two terms, so each term's assignment can be
    /// chosen on its own instead of solved as a joint one.
    pub(crate) terms_disjoint: bool,
    /// The total number of hosts the requirement asks for: the most any
    /// assignment can use, and so the most of each term's ranked hosts that
    /// matter.
    pub(crate) slots: usize,
    /// The attendees' participant indices, ascending by ID.
    pub(crate) attendees: Vec<usize>,
    /// Every participant an offer and its explanation depend on: the attendees
    /// and the hosts at least one term selects, ascending by ID.
    pub(crate) reported: Vec<usize>,
    /// Each participant's assignment weight, by the same index as
    /// `participants`.
    pub(crate) weights: Vec<i32>,
    /// Participant IDs whose calendar was not supplied, so `calendar_flags`
    /// avoids scanning all participants per offer.
    pub(crate) unknown_calendars: BTreeSet<&'a str>,
    /// Whether the booking requires non-zero travel for any participant, so
    /// `travel_reserve` returns immediately when no travel applies.
    pub(crate) has_any_travel: bool,
}

/// One requirement term, resolved to the participants that can fill it.
pub(crate) struct ResolvedTerm {
    /// How many distinct hosts the term needs.
    pub(crate) count: usize,
    /// The participant indices that fill it, ascending by participant ID, so a
    /// term's eligible list needs no further sorting.
    pub(crate) hosts: Vec<usize>,
    /// The same participant indices ordered by weight descending then ID
    /// ascending: the order a term takes its hosts from when no participant fills
    /// two terms, and the order the matcher's top-`slots` cut is taken in.
    pub(crate) hosts_by_weight: Vec<usize>,
}

/// One participant, resolved for this request.
pub(crate) struct ParticipantSettings<'a> {
    pub(crate) participant: &'a Participant,
    pub(crate) rules: Applicable<'a>,
    /// The travel this participant needs for the booking, before and after, in
    /// minutes: the eligibility decision reads it once per candidate start.
    pub(crate) travel: (i64, i64),
    /// The total occupied length (`travel.0 + duration + travel.1`) in minutes,
    /// matching the length `rest_self_blocked` was precomputed for.
    pub(crate) occupied_len: i64,
    /// The minutes inside every applicable window, or `None` when no window
    /// applies: a participant with no window has no window restriction, which no
    /// interval set inside the supported range could express for a span that
    /// reaches past its edges.
    pub(crate) allowed: Option<Arc<IntervalSet>>,
    /// The minutes a booking may occupy: `allowed`, or every representable minute
    /// when no window applies, less every blackout, clearance-widened blocking
    /// block and existing `rest` exclusion. Checking `usable.contains_span` of the
    /// representable part of the occupied span answers all of those in one binary
    /// search; `failing_reasons` remains the authority when a reason is built.
    pub(crate) usable: Arc<IntervalSet>,
    /// Occupied-start minutes excluded because a booking of length
    /// `occupied_len` would itself overlap both `late(D)` and `early(D + 1)`
    /// under some applicable `rest` rule.
    pub(crate) rest_self_blocked: IntervalSet,
    /// Start minutes inside the search window excluded by at least one
    /// applicable cap whose bucket has no room left for this booking. Empty for a
    /// confirmation, which judges its one start against `caps` directly and so
    /// never reads this set: only a search's eligibility pass does.
    pub(crate) cap_blocked: IntervalSet,
    /// Whether an applicable cap rejects even an empty bucket on its own. `false`
    /// for a confirmation, for the same reason as `cap_blocked`.
    pub(crate) cap_always_blocked: bool,
    /// Each applicable window's minutes over the judged range, in request order,
    /// so a reason can cite the rule that excluded a span. Empty when no
    /// explanation is requested during a search.
    pub(crate) windows: allowed::RuleMinutes<'a>,
    /// Each applicable blackout's minutes over the judged range, in request
    /// order. Empty when no explanation is requested during a search.
    pub(crate) blackouts: allowed::RuleMinutes<'a>,
    /// The blocking blocks in canonical order, indexed for `O(log B)` overlap
    /// and clearance queries so `blocked` and `clearance` can name the event
    /// behind the exclusion.
    pub(crate) blocked: BlockedIndex<'a>,
    /// The strictest `lead_time` maximum and the rule that supplied it.
    pub(crate) lead_bound: Option<(&'a str, StartBound)>,
    /// The earliest minute the booking's occupied start may fall on: `now` plus
    /// the strictest minimum notice, resolved once per request.
    pub(crate) lead_min_at: i64,
    /// The participant's zone as a function of the instant, which rest
    /// membership reads.
    pub(crate) schedule: ZoneSchedule,
    /// Each applicable `rest` rule, evaluated independently.
    pub(crate) rests: Vec<crate::rest::RestActivity<'a>>,
    /// The instants the rest date walk stops at besides local midnight. Empty
    /// when no rest rule applies, so no walk runs.
    pub(crate) date_jumps: Vec<i64>,
    /// Every cap that applies to this participant, in request order, with the
    /// usage each already holds.
    pub(crate) caps: Vec<crate::caps::ParticipantCap<'a>>,
}

impl ParticipantSettings<'_> {
    /// Whether every representable minute of `span` is inside every applicable
    /// window.
    ///
    /// With no window applying, nothing restricts the participant. A span
    /// reaching past the supported range is judged on the part the model can
    /// represent, because the minutes beyond it are ones no rule a caller wrote
    /// can describe.
    fn inside_windows(&self, span: &MinuteSpan) -> bool {
        let Some(judged) = allowed::representable(*span) else {
            return true;
        };
        self.allowed
            .as_ref()
            .is_none_or(|allowed| allowed.contains_span(&judged))
    }

    /// The earliest applicable window that does not cover `span`: the rule an
    /// `outside_window` or `travel` reason cites, or `None` when no window applies
    /// or every one covers the representable part.
    fn excluding_window(&self, span: &MinuteSpan) -> Option<&str> {
        let judged = allowed::representable(*span)?;
        self.windows
            .iter()
            .find(|(_, set)| !set.contains_span(&judged))
            .map(|(id, _)| *id)
    }
}

/// Resolves a search, reusing the calendars, schedules, selected rules and lead
/// bounds validation already resolved.
///
/// # Errors
/// Returns the failures [`settings`] documents.
pub(crate) fn search_settings_resolved<'a>(
    engine: &Engine,
    request: &'a crate::model::Request,
    candidates: Vec<Instant>,
    pre: Option<PreResolved<'a>>,
) -> Result<Settings<'a>, Error> {
    let Some(part) = request.goal.part() else {
        return Err(Error::PartCount {
            parts: request.goal.parts.len(),
        });
    };
    let judged = padded_range(&request.goal, part)?;
    let window = request.goal.search_window;
    let last_start = Instant::from_minutes(window.end().minutes() - 1).unwrap_or(window.start());
    settings(
        engine,
        request,
        candidates,
        judged,
        Some(window),
        (window.start(), last_start),
        request.explain.is_some(),
        // A search only judges starts inside the window, so a `working_days`
        // horizon past its end allows every one of them.
        Some(window.end()),
        pre,
    )
}

/// Resolves a single proposed booking, judging the booking's own range rather
/// than the search window, which `check_slot` ignores.
///
/// # Errors
/// Returns the failures [`settings`] documents, and [`Error::PartCount`] for a
/// goal that does not have exactly one part.
#[cfg(all(test, feature = "bundled-tzdb"))]
pub(crate) fn slot_settings<'a>(
    engine: &Engine,
    request: &'a crate::model::Request,
    start: Instant,
) -> Result<Settings<'a>, Error> {
    slot_settings_resolved(engine, request, start, None)
}

/// Resolves a single proposed booking, reusing the calendars, schedules,
/// selected rules and lead bounds validation already resolved.
///
/// # Errors
/// Returns the failures [`settings`] documents, and [`Error::PartCount`] for a
/// goal that does not have exactly one part.
pub(crate) fn slot_settings_resolved<'a>(
    engine: &Engine,
    request: &'a crate::model::Request,
    start: Instant,
    pre: Option<PreResolved<'a>>,
) -> Result<Settings<'a>, Error> {
    let Some(part) = request.goal.part() else {
        return Err(Error::PartCount {
            parts: request.goal.parts.len(),
        });
    };
    let judged = booking_range(part, start)?;
    settings(
        engine,
        request,
        vec![start],
        judged,
        None,
        (start, start),
        true,
        // A confirmation judges only the proposed start, which may lie outside the
        // search window, so its horizon is resolved without a cutoff.
        None,
        pre,
    )
}

/// Resolves a request into the settings the pipeline reads, with candidate starts
/// supplied by the caller and allowed and blocked time built over `judged`.
///
/// A search judges its padded search window; `check_slot` judges the proposed
/// booking, because it ignores the search window. `cap_blocked_over` names the
/// span the precomputed `cap_blocked` set covers, or `None` when no caller will
/// ask which starts a cap has already used up: a confirmation judges its one
/// start against the caps themselves, so it does not pay for a set it never
/// reads.
///
/// `horizon_end` is the latest instant the caller will judge, which bounds how far
/// a `working_days` calendar walk has to reach; `None` resolves each horizon
/// exactly, which is what a confirmation with an unknown start needs.
///
/// # Errors
/// Returns an [`Error::Time`] failure when a zone, period or calendar cannot be
/// resolved — all of which `validate` reports first.
#[allow(clippy::too_many_arguments)]
fn settings<'a>(
    engine: &Engine,
    request: &'a crate::model::Request,
    candidates: Vec<Instant>,
    judged: MinuteSpan,
    cap_blocked_over: Option<crate::model::Span>,
    target_starts: (Instant, Instant),
    keep_per_rule: bool,
    horizon_end: Option<Instant>,
    pre: Option<PreResolved<'a>>,
) -> Result<Settings<'a>, Error> {
    let Some(part) = request.goal.part() else {
        // `validate` reports this first; evaluation still refuses rather than
        // guessing which part was meant.
        return Err(Error::PartCount {
            parts: request.goal.parts.len(),
        });
    };

    let db = engine.time_zone_database();
    let event = db.zone(request.goal.zone.as_str())?;
    let display = db.zone(request.goal.display_zone.as_str())?;
    let (calendars, mut pre_schedules, mut pre_rules, lead_bounds) = match pre {
        Some(pre) => (
            pre.calendars,
            Some(pre.schedules.into_iter()),
            Some(pre.rules.into_iter()),
            pre.lead_bounds,
        ),
        None => (resolve_calendars(db, request)?, None, None, BTreeMap::new()),
    };

    let mut participants = Vec::with_capacity(request.participants.len());
    let mut weights = Vec::with_capacity(request.participants.len());
    let mut attendees = Vec::new();
    let mut unknown_calendars = BTreeSet::new();
    // The booking half of every rule's applicability is decided once here, and
    // every participant's `who` is matched against a selector prepared for repeated
    // matching, so a request's cost follows its rules rather than its
    // rules times its participants.
    let matcher = RuleMatcher::new(&request.rules, &part.tags);
    let mut shared = Shared {
        db,
        request,
        part,
        event: &event,
        judged,
        keep_per_rule,
        calendars: &calendars,
        excluded: crate::exclude::ExcludedKeys::new(request),
        fixed_minutes: FixedMinutes::default(),
        rest_memo: RestMemo::default(),
        lead_bounds,
        matcher,
        horizon_end,
    };
    for (index, participant) in request.participants.iter().enumerate() {
        if participant.role == Role::Attendee {
            attendees.push(index);
        }
        if participant.calendar.is_none() {
            unknown_calendars.insert(participant.id.as_str());
        }
        weights.push(request.weight(&participant.id));
        let schedule = match pre_schedules.as_mut().and_then(Iterator::next) {
            Some(schedule) => schedule,
            None => resolve_schedule(db, participant)?,
        };
        let rules = match pre_rules.as_mut().and_then(Iterator::next) {
            Some(rules) => rules,
            None => shared.matcher.select(participant.context()),
        };
        let settings = participant_settings(&mut shared, participant, schedule, rules)?;
        participants.push(settings);
    }

    // Caps read every participant's zone to bucket events and this booking, so
    // they are built once for the whole request rather than per participant.
    crate::caps::populate_caps(
        request,
        part,
        &event,
        db,
        &mut participants,
        &shared.excluded,
        cap_blocked_over,
        target_starts,
    )?;

    attendees.sort_by(|left, right| {
        participants[*left]
            .participant
            .id
            .cmp(&participants[*right].participant.id)
    });

    let resolved = requirement_terms(part, &participants, &weights);
    let reported = reported_participants(&participants, &attendees, &resolved.terms);
    let has_any_travel = part_has_travel(part);

    Ok(Settings {
        part,
        display,
        window: request.goal.search_window,
        candidates,
        participants,
        terms: resolved.terms,
        terms_disjoint: resolved.disjoint,
        slots: resolved.slots,
        attendees,
        reported,
        weights,
        unknown_calendars,
        has_any_travel,
    })
}

/// Every participant an offer or explanation depends on (attendees plus every
/// host filling at least one term), ascending by ID and deduplicated.
fn reported_participants(
    participants: &[ParticipantSettings<'_>],
    attendees: &[usize],
    terms: &[ResolvedTerm],
) -> Vec<usize> {
    let mut reported: Vec<usize> = attendees.to_vec();
    reported.extend(terms.iter().flat_map(|term| term.hosts.iter().copied()));
    reported.sort_by(|left, right| {
        participants[*left]
            .participant
            .id
            .cmp(&participants[*right].participant.id)
    });
    reported.dedup();
    reported
}

/// Whether `part` requires any non-zero travel for any participant.
fn part_has_travel(part: &Part) -> bool {
    part.travel.before.minutes() > 0
        || part.travel.after.minutes() > 0
        || part
            .travel
            .by_participant
            .values()
            .any(|travel| travel.before.minutes() > 0 || travel.after.minutes() > 0)
}

/// The requirement's terms resolved against the participants, with the two facts
/// the per-start assignment reads: how many hosts it asks for in total, and
/// whether one participant can fill two of its terms.
struct RequirementTerms {
    terms: Vec<ResolvedTerm>,
    /// Whether no participant fills two terms, so each term's assignment can be
    /// chosen on its own.
    disjoint: bool,
    /// The total number of hosts the requirement asks for: the most any
    /// assignment can use, and so the most of each term's ranked hosts that
    /// matter.
    slots: usize,
}

/// Resolves each term to the participants that can fill it, in participant ID
/// order and again ranked by weight.
fn requirement_terms(
    part: &Part,
    participants: &[ParticipantSettings<'_>],
    weights: &[i32],
) -> RequirementTerms {
    let terms: Vec<ResolvedTerm> = part
        .requirement
        .terms
        .iter()
        .map(|term| {
            let mut hosts: Vec<usize> = participants
                .iter()
                .enumerate()
                .filter(|(_, settings)| term.filled_by(settings.participant))
                .map(|(index, _)| index)
                .collect();
            hosts.sort_by(|left, right| {
                participants[*left]
                    .participant
                    .id
                    .cmp(&participants[*right].participant.id)
            });
            let mut hosts_by_weight = hosts.clone();
            hosts_by_weight.sort_by(|left, right| {
                weights[*right].cmp(&weights[*left]).then_with(|| {
                    participants[*left]
                        .participant
                        .id
                        .cmp(&participants[*right].participant.id)
                })
            });
            ResolvedTerm {
                count: term.slots(),
                hosts,
                hosts_by_weight,
            }
        })
        .collect();

    let slots = terms
        .iter()
        .fold(0_usize, |total, term| total.saturating_add(term.count));
    let mut seen = vec![false; participants.len()];
    let mut disjoint = true;
    for host in terms.iter().flat_map(|term| term.hosts.iter().copied()) {
        if std::mem::replace(&mut seen[host], true) {
            disjoint = false;
            break;
        }
    }
    RequirementTerms {
        terms,
        disjoint,
        slots,
    }
}

/// The request-wide inputs every participant's settings are resolved against,
/// together with the memos those resolutions share.
///
/// One participant's resolution needs the database, the request, its part, the
/// event zone, the judged range, the resolved calendars and the event keys the
/// request excludes, and pays for two answers that depend on none of those
/// participants: the minutes a fixed-zone window or blackout covers, and the bound
/// each `lead_time` maximum places on a start. Bundling them keeps the resolution's
/// signature about the participant it answers for.
struct Shared<'a, 's> {
    db: &'s TimeZoneDatabase,
    request: &'a crate::model::Request,
    part: &'a Part,
    event: &'s Zone,
    judged: MinuteSpan,
    keep_per_rule: bool,
    calendars: &'s BTreeMap<String, WorkingCalendar>,
    /// The events the caller is moving, resolved once so every participant's
    /// blocked time and the cap workload read one set rather than one per event.
    excluded: crate::exclude::ExcludedKeys<'a>,
    fixed_minutes: FixedMinutes<'a>,
    rest_memo: RestMemo<'a>,
    lead_bounds: BTreeMap<usize, StartBound>,
    /// Every rule with the booking half of its applicability decided once and its
    /// `who` prepared for repeated matching.
    matcher: RuleMatcher<'a>,
    /// The latest instant this resolution judges, which bounds a `working_days`
    /// calendar walk; see [`resolve_lead_bound`].
    horizon_end: Option<Instant>,
}

/// One participant's settings: the rules that apply to them and the minutes
/// those rules allow and block over the judged range.
///
/// # Errors
/// Returns an [`Error::Time`] failure when a zone, period or calendar cannot be
/// resolved, all of which `validate` reports first.
fn participant_settings<'a>(
    shared: &mut Shared<'a, '_>,
    participant: &'a Participant,
    schedule: ZoneSchedule,
    rules: Applicable<'a>,
) -> Result<ParticipantSettings<'a>, Error> {
    let Shared {
        db,
        request,
        part,
        event,
        judged,
        keep_per_rule,
        calendars,
        excluded,
        fixed_minutes,
        rest_memo,
        lead_bounds,
        matcher: _,
        horizon_end,
    } = shared;
    let judged = *judged;
    let keep_per_rule = *keep_per_rule;
    let horizon_end = *horizon_end;
    let home_zone_name = participant.zone.as_str();
    let travel = part.travel.for_participant(&participant.id);
    let travel_minutes = (travel.0.minutes(), travel.1.minutes());
    let occupied_len = travel_minutes
        .0
        .saturating_add(part.duration.minutes())
        .saturating_add(travel_minutes.1);
    let (allowed, windows) = allowed::allowed_time(
        db,
        &rules,
        &schedule,
        home_zone_name,
        event,
        judged,
        keep_per_rule,
        fixed_minutes,
    )?;
    let (blackouts, blackout_spans) = allowed::blackout_time(
        db,
        &rules,
        &schedule,
        home_zone_name,
        event,
        judged,
        keep_per_rule,
        fixed_minutes,
    )?;
    let blocked = blocked::blocked_time(&rules, participant, excluded);
    let (rests, date_jumps, rest_self_blocked) = crate::rest::rest_time(
        &rules,
        blocked.spans(),
        &schedule,
        home_zone_name,
        judged,
        occupied_len,
        rest_memo,
    )?;
    let usable = allowed::build_usable(
        allowed.as_ref(),
        blackout_spans,
        blocked.spans(),
        &rests,
        &schedule,
        &date_jumps,
    );
    let lead_bound = strictest_bound(
        request.now,
        calendars,
        &rules,
        event,
        lead_bounds,
        horizon_end,
    )?;
    let lead_min_at = request.now.minutes() + rules.lead_min.minutes();
    Ok(ParticipantSettings {
        participant,
        rules,
        travel: travel_minutes,
        occupied_len,
        lead_min_at,
        allowed,
        usable,
        rest_self_blocked,
        cap_blocked: IntervalSet::new(),
        cap_always_blocked: false,
        windows,
        blackouts,
        blocked,
        lead_bound,
        schedule,
        rests,
        date_jumps,
        caps: Vec::new(),
    })
}

/// The search window widened by the largest travel before it and by the
/// duration plus the largest travel after it.
///
/// Allowed and blocked time are built over this range so an occupied span that
/// legitimately extends past the window is judged, rather than clipped. The range
/// reaches the first minute past the last supported instant rather than stopping
/// at it, so that minute — the one a booking at the end of the range still
/// occupies — is inside every set built from it.
pub(crate) fn padded_range(goal: &Goal, part: &Part) -> Result<MinuteSpan, Error> {
    let window = goal.search_window;
    judged_range(part, window.start(), window.end())
}

/// The range a single proposed booking is judged over: its occupied span plus the
/// largest travel either side, which is where its windows and blackouts must be
/// resolved. `check_slot` ignores the search window, so the range follows the
/// booking.
///
/// Like a search's, the range reaches the first minute past the last supported
/// instant rather than stopping at it, so a booking that runs past the end of the
/// range still judges the minute at `Instant::MAX`.
fn booking_range(part: &Part, start: Instant) -> Result<MinuteSpan, Error> {
    judged_range(part, start, start)
}

/// Widens `from..until` by the largest travel before `from` and by the part's
/// duration plus the largest travel after `until`, clamped to the representable
/// minutes.
fn judged_range(part: &Part, from: Instant, until: Instant) -> Result<MinuteSpan, Error> {
    let start = from
        .minutes()
        .saturating_sub(part.travel.largest_before().minutes())
        .max(Instant::MIN.minutes());
    let end = until
        .minutes()
        .saturating_add(part.duration.minutes() + part.travel.largest_after().minutes())
        .min(Instant::MAX.minutes() + 1);
    Ok(MinuteSpan::new(start, end.max(start + 1))?)
}

/// The strictest of the applicable `lead_time` maximums, with the rule that
/// supplied it.
///
/// A start outside one bound is outside every looser one, so the strictest bound
/// decides eligibility. Equal bounds keep the earliest rule in request order,
/// which is what the reason cites.
///
/// `bounds` memoizes the resolution per rule position: a bound depends on the
/// request's `now`, calendars, event zone and judging horizon, so every
/// participant the same rule applies to reads one resolution rather than walking
/// its calendar again.
#[allow(clippy::too_many_arguments)]
fn strictest_bound<'a>(
    now: Instant,
    calendars: &BTreeMap<String, WorkingCalendar>,
    rules: &Applicable<'a>,
    event: &Zone,
    bounds: &mut BTreeMap<usize, StartBound>,
    horizon_end: Option<Instant>,
) -> Result<Option<(&'a str, StartBound)>, Error> {
    let mut strictest: Option<(&'a str, StartBound)> = None;
    for lead in &rules.lead_max {
        let bound = if let Some(bound) = bounds.get(&lead.position) {
            bound.clone()
        } else {
            let bound = resolve_lead_bound(now, calendars, lead.max, event, horizon_end)?;
            bounds.insert(lead.position, bound.clone());
            bound
        };
        match &strictest {
            Some((_, current)) if !bound.is_stricter_than(current) => {}
            _ => strictest = Some((lead.id, bound)),
        }
    }
    Ok(strictest)
}

/// Resolves one `lead_time` maximum into a bound on start instants, for a request
/// whose calendars are already resolved.
///
/// `horizon_end` is the latest instant the caller will judge. A `working_days`
/// count is resolved against that cutoff, so a count far past it costs a walk
/// bounded by the horizon rather than one over the whole supported calendar; the
/// bound stays exact for every start inside it. `None` resolves the count exactly.
///
/// # Errors
/// Returns an [`Error::Time`] failure for a calendar the request does not carry
/// or a zone whose civil dates cannot be walked.
pub(crate) fn resolve_lead_bound(
    now: Instant,
    calendars: &BTreeMap<String, WorkingCalendar>,
    max: &LeadTimeMax,
    event: &Zone,
    horizon_end: Option<Instant>,
) -> Result<StartBound, Error> {
    match max {
        LeadTimeMax::Duration(duration) => Ok(Horizon::Duration(*duration).bound(now, event)?),
        LeadTimeMax::Days(days) => Ok(Horizon::Days(*days).bound(now, event)?),
        LeadTimeMax::Date(date) => Ok(Horizon::Date(*date).bound(now, event)?),
        LeadTimeMax::WorkingDays { count, calendar } => {
            let calendar = calendars.get(calendar).ok_or_else(|| {
                // `validate` rejects an unknown calendar before evaluation, so
                // reaching this is a request that bypassed validation; report
                // the same `calendar_unknown` finding rather than misclassifying
                // it as an unknown time zone.
                Error::invalid_request(vec![crate::error::Diagnostic::new(
                    "calendar_unknown",
                    format!("calendar `{calendar}` is not carried by the request"),
                )])
            })?;
            Ok(match horizon_end {
                Some(horizon) => calendar.working_days_bound_until(now, *count, horizon)?,
                None => calendar.working_days_bound(now, *count)?,
            })
        }
    }
}

/// Every working calendar the request names, resolved once so each `lead_time`
/// maximum's horizon and each participant's rules read the same calendars.
///
/// # Errors
/// Returns an [`Error::Time`] failure for a calendar zone the database does not
/// carry, or one that cannot be a working calendar.
pub(crate) fn resolve_calendars(
    db: &TimeZoneDatabase,
    request: &crate::model::Request,
) -> Result<BTreeMap<String, WorkingCalendar>, Error> {
    let mut calendars = BTreeMap::new();
    for (name, spec) in &request.calendars {
        calendars.insert(
            name.clone(),
            WorkingCalendar::new(
                db.zone(spec.zone.as_str())?,
                spec.working_days,
                spec.holidays.clone(),
            )?,
        );
    }
    Ok(calendars)
}

/// Resolves a participant's home zone and zone periods into the zone function
/// the evaluation reads.
///
/// # Errors
/// Returns a [`crate::time::TimeError`] when a zone is unknown, a period is
/// empty, or two periods overlap.
pub(crate) fn resolve_schedule(
    db: &TimeZoneDatabase,
    participant: &Participant,
) -> Result<ZoneSchedule, crate::time::TimeError> {
    let home = db.zone(participant.zone.as_str())?;
    let mut periods = Vec::with_capacity(participant.zone_periods.len());
    for period in &participant.zone_periods {
        let zone = db.zone(period.zone().as_str())?;
        periods.push(crate::time::ZonePeriod::new(
            zone,
            period.from(),
            period.until(),
        )?);
    }
    ZoneSchedule::new(home, periods)
}
