//! Bidirectional conversions between the wire shapes and the domain types.
//!
//! Decoding walks the wire value with the RFC 6901 pointer of the field it is
//! reading, so a rejection names the exact place in the document even when the
//! rule it broke is a domain constructor's. Encoding reads the domain value and
//! writes the wire shape, resolving every wire format default so the
//! output never depends on what the caller left out.
//!
//! No conversion validates the request as a whole: naming a zone, a calendar or a
//! participant that does not exist is [`Engine::validate`]'s question, because
//! only the engine has the time zone database and the limits. What is enforced
//! here is structural — a value the domain type cannot hold is rejected at the
//! field that held it.
//!
//! [`Engine::validate`]: opportune::Engine::validate

use std::collections::BTreeMap;

use opportune::model::{
    Block, BlockStatus, CalendarSpec, Cap, CapMeasure, CapPer, CapPeriod, Clearance, DateCount,
    Explain, Goal, LeadTime, LeadTimeMax, Meta, Occurrence, Offer, OfferEvent, Part, Participant,
    ParticipantKind, ParticipantTravel, Proposal, ProposalEvent, RangesQuery, Request,
    RequiredRanges, Requirement, ReserveSpan, Response, Rest, Rule, RuleKind, SlotCheck, Span,
    StatusEffect, StatusPolicy, StatusPolicyEntry, Term, Travel, ZonePeriod,
};
use opportune::tags::{Role, Selector, Tag};
use opportune::time::{
    BlackoutSpan, BlackoutSpec, DateKey, DateRange, Duration, Grid, GridZone, Instant, RuleZone,
    TimeOfDay, WallRange, Weekday, Weekdays, WeeklyKey, WindowSpec, ZoneName,
};
use opportune::{Diagnostic, Diagnostics, Excluded, Limits, Reason, ReasonParams};

use crate::error::{JsonError, join_index, join_pointer};
use crate::primitives::{
    InstantRounding, MatchAll, format_duration, parse_date, parse_date_range, parse_duration,
    parse_instant, parse_local_date_time, parse_time_of_day, parse_wall_range, parse_weekday,
    parse_weekdays, parse_working_days,
};
use crate::wire::{
    AtLeastTerm, BlackoutParams, BlackoutRule, CapParams, CapRule, ClearanceParams, ClearanceRule,
    DateMax, DaysMax, DurationMax, LeadTimeParams, LeadTimeRule, PersonTerm, RestParams, RestRule,
    StatusPolicyParams, StatusPolicyRule, WindowParams, WindowRule, WireBlock, WireBlockStatus,
    WireCalendarSpec, WireCapMeasure, WireCapPer, WireCapPeriod, WireDateCount, WireDiagnostic,
    WireDiagnostics, WireExcluded, WireExplain, WireGoal, WireGrid, WireLeadTimeMax, WireLimits,
    WireMeta, WireOccurrence, WireOffer, WireOfferEvent, WirePart, WireParticipant,
    WireParticipantKind, WireParticipantTravel, WireProposal, WireProposalEvent, WireRangesQuery,
    WireReason, WireReasonParams, WireRequest, WireRequiredRanges, WireRequirement,
    WireReserveSpan, WireResponse, WireRole, WireRule, WireSelector, WireSlotCheck, WireSpan,
    WireStatus, WireStatusEffect, WireStatusPolicyEntry, WireTerm, WireTravel, WireZonePeriod,
    WorkingDaysMax,
};

// ---------------------------------------------------------------------------
// Wire to domain
// ---------------------------------------------------------------------------

/// A count the document wrote, in the width this target can hold.
///
/// The wire reads every count as a `u64`, so a document means the same thing on
/// `wasm32` as on a 64-bit server. A value this target cannot hold saturates
/// instead of failing the parse: it is larger than anything the request could
/// name. An `explain.limit` that large is still the configured-limits finding,
/// and an offer limit that large simply keeps every offer — either way both
/// targets give the same answer.
fn count_as_usize(value: u64) -> usize {
    usize::try_from(value).unwrap_or(usize::MAX)
}

/// A `usize` the domain holds, as the `u64` a document carries.
fn count_as_u64(value: usize) -> u64 {
    u64::try_from(value).unwrap_or(u64::MAX)
}

/// The request a wire document describes.
pub(crate) fn request(wire: &WireRequest) -> Result<Request, JsonError> {
    let now = parse_instant(&wire.now, InstantRounding::Ceil, "/now")?;
    let mut request = Request::new(now, goal(&wire.goal, "/goal")?);
    request.participants = participants(&wire.participants, "/participants")?;
    request.rules = rules(&wire.rules, "/rules")?;
    request.occurrences = occurrences(&wire.occurrences, "/occurrences")?;
    request
        .exclude_event_keys
        .clone_from(&wire.exclude_event_keys);
    request.calendars = calendars(&wire.calendars, "/calendars")?;
    request.weights.clone_from(&wire.weights);
    request.limit = wire.limit.map(count_as_usize);
    request.explain = wire
        .explain
        .map(|explain| Explain::new(count_as_usize(explain.limit)));
    Ok(request)
}

/// The goal a wire document describes.
pub(crate) fn goal(wire: &WireGoal, path: &str) -> Result<Goal, JsonError> {
    let event_zone = zone(&wire.zone, &join_pointer(path, "zone"))?;
    let search_window = span(
        &wire.search_window,
        InstantRounding::Exact,
        InstantRounding::Exact,
        &join_pointer(path, "search_window"),
    )?;
    let parts = wire
        .parts
        .iter()
        .enumerate()
        .map(|(index, part)| part_of(part, &join_index(&join_pointer(path, "parts"), index)))
        .collect::<Result<Vec<_>, _>>()?;
    let mut goal = Goal::new(event_zone, search_window, parts);
    goal.grid = wire
        .grid
        .as_ref()
        .map(|grid| grid_of(grid, &join_pointer(path, "grid")))
        .transpose()?;
    goal.candidates = wire
        .candidates
        .as_ref()
        .map(|candidates| {
            let candidates_path = join_pointer(path, "candidates");
            candidates
                .iter()
                .enumerate()
                .map(|(index, candidate)| {
                    parse_instant(
                        candidate,
                        InstantRounding::Exact,
                        &join_index(&candidates_path, index),
                    )
                })
                .collect::<Result<Vec<_>, _>>()
        })
        .transpose()?;
    if let Some(display_zone) = &wire.display_zone {
        goal.display_zone = zone(display_zone, &join_pointer(path, "display_zone"))?;
    }
    Ok(goal)
}

/// The goal part a wire document describes.
fn part_of(wire: &WirePart, path: &str) -> Result<Part, JsonError> {
    let duration = parse_duration(&wire.duration, &join_pointer(path, "duration"))?;
    let requirement = requirement(&wire.requirement, &join_pointer(path, "requirement"))?;
    let mut part = Part::new(duration, requirement);
    if let Some(id) = &wire.id {
        part.id.clone_from(id);
    }
    part.tags = tags(&wire.tags, &join_pointer(path, "tags"))?;
    part.travel = wire
        .travel
        .as_ref()
        .map(|travel| travel_of(travel, &join_pointer(path, "travel")))
        .transpose()?
        .unwrap_or_default();
    Ok(part)
}

/// The requirement a wire document describes.
fn requirement(wire: &WireRequirement, path: &str) -> Result<Requirement, JsonError> {
    let all_path = join_pointer(path, "all");
    let terms = wire
        .all
        .iter()
        .enumerate()
        .map(|(index, term)| term_of(term, &join_index(&all_path, index)))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Requirement::new(terms))
}

/// One requirement term.
fn term_of(wire: &WireTerm, path: &str) -> Result<Term, JsonError> {
    Ok(match wire {
        WireTerm::Person(PersonTerm { person }) => Term::person(person.clone()),
        WireTerm::AtLeast(AtLeastTerm { at_least, of }) => {
            Term::at_least(*at_least, selector(of, &join_pointer(path, "of"))?)
        }
    })
}

/// The travel a wire document describes.
fn travel_of(wire: &WireTravel, path: &str) -> Result<Travel, JsonError> {
    let mut travel = Travel {
        before: parse_duration(&wire.before, &join_pointer(path, "before"))?,
        after: parse_duration(&wire.after, &join_pointer(path, "after"))?,
        by_participant: BTreeMap::new(),
    };
    let overrides_path = join_pointer(path, "by_participant");
    for (id, override_travel) in &wire.by_participant {
        let override_path = join_pointer(&overrides_path, id);
        travel.by_participant.insert(
            id.clone(),
            ParticipantTravel {
                before: parse_duration(
                    &override_travel.before,
                    &join_pointer(&override_path, "before"),
                )?,
                after: parse_duration(
                    &override_travel.after,
                    &join_pointer(&override_path, "after"),
                )?,
            },
        );
    }
    Ok(travel)
}

/// The selector a wire document describes.
///
/// The walk is bounded: a selector is the one recursive wire shape, so a service
/// that builds a [`WireSelector`] in memory and converts it with
/// `Rule::try_from` — without `serde_json`'s own nesting limit in the way — must get
/// a rejection rather than a stack overflow.
fn selector(wire: &WireSelector, path: &str) -> Result<Selector, JsonError> {
    selector_at(wire, path, 0)
}

/// How deeply a selector may nest before it is rejected.
///
/// A `not`, `any` or `all` adds one level, so the bound is what stops a
/// converted-in-memory chain from exhausting the stack. Sixty-four levels is far
/// past anything a caller writes and is the same order as `serde_json`'s own limit.
const MAX_SELECTOR_DEPTH: usize = 64;

/// The selector a wire document describes, `depth` levels into the tree.
fn selector_at(wire: &WireSelector, path: &str, depth: usize) -> Result<Selector, JsonError> {
    if depth >= MAX_SELECTOR_DEPTH {
        return Err(JsonError::new(
            "selector_too_deep",
            format!("a selector may nest at most {MAX_SELECTOR_DEPTH} levels"),
            path,
        ));
    }
    Ok(match wire {
        WireSelector::MatchAll(_) => Selector::True,
        WireSelector::Tag(tag) => Selector::Tag(
            Tag::new(tag)
                .map_err(|err| JsonError::from_domain(err.into(), &join_pointer(path, "tag")))?,
        ),
        WireSelector::Any(selectors) => {
            Selector::Any(nested(selectors, &join_pointer(path, "any"), depth)?)
        }
        WireSelector::All(selectors) => {
            Selector::All(nested(selectors, &join_pointer(path, "all"), depth)?)
        }
        WireSelector::Not(selector) => Selector::Not(Box::new(selector_at(
            selector,
            &join_pointer(path, "not"),
            depth + 1,
        )?)),
        WireSelector::Ids(ids) => Selector::Ids(ids.clone()),
        WireSelector::Role(role) => Selector::Role(role_of(*role)),
        WireSelector::Kind(kind) => Selector::Kind(kind_of(*kind)),
    })
}

/// The nested selectors of an `any` or `all`, one level below `depth`.
fn nested(
    selectors: &[WireSelector],
    path: &str,
    depth: usize,
) -> Result<Vec<Selector>, JsonError> {
    let below = depth + 1;
    selectors
        .iter()
        .enumerate()
        .map(|(index, selector)| selector_at(selector, &join_index(path, index), below))
        .collect()
}

/// The participants a wire document describes.
fn participants(wire: &[WireParticipant], path: &str) -> Result<Vec<Participant>, JsonError> {
    wire.iter()
        .enumerate()
        .map(|(index, participant)| participant_of(participant, &join_index(path, index)))
        .collect()
}

/// One participant.
fn participant_of(wire: &WireParticipant, path: &str) -> Result<Participant, JsonError> {
    let mut participant = Participant::new(
        wire.id.clone(),
        zone(&wire.zone, &join_pointer(path, "zone"))?,
    );
    participant.kind = wire.kind.map_or(ParticipantKind::Person, kind_of);
    participant.role = match wire.role {
        Some(WireRole::Attendee) => Role::Attendee,
        Some(WireRole::Host) | None => Role::Host,
    };
    participant.tags = tags(&wire.tags, &join_pointer(path, "tags"))?;
    participant.zone_periods =
        zone_periods(&wire.zone_periods, &join_pointer(path, "zone_periods"))?;
    participant.calendar = wire
        .calendar
        .as_ref()
        .map(|blocks| {
            let blocks_path = join_pointer(path, "calendar");
            blocks
                .iter()
                .enumerate()
                .map(|(index, block)| block_of(block, &join_index(&blocks_path, index)))
                .collect::<Result<Vec<_>, _>>()
        })
        .transpose()?;
    Ok(participant)
}

/// The zone periods a wire document describes.
fn zone_periods(wire: &[WireZonePeriod], path: &str) -> Result<Vec<ZonePeriod>, JsonError> {
    wire.iter()
        .enumerate()
        .map(|(index, period)| {
            let period_path = join_index(path, index);
            ZonePeriod::new(
                zone(&period.zone, &join_pointer(&period_path, "zone"))?,
                parse_instant(
                    &period.from,
                    InstantRounding::Exact,
                    &join_pointer(&period_path, "from"),
                )?,
                parse_instant(
                    &period.until,
                    InstantRounding::Exact,
                    &join_pointer(&period_path, "until"),
                )?,
            )
            .map_err(|err| JsonError::from_domain(err, &period_path))
        })
        .collect()
}

/// One block.
fn block_of(wire: &WireBlock, path: &str) -> Result<Block, JsonError> {
    let mut block = Block::new(span(
        &wire.span,
        InstantRounding::Floor,
        InstantRounding::Ceil,
        &join_pointer(path, "span"),
    )?);
    block.status = block_status(wire.status.as_str());
    block.all_day = wire.all_day;
    block.tags = tags(&wire.tags, &join_pointer(path, "tags"))?;
    if let Some(key) = wire.event_key.clone() {
        block = block
            .with_event_key(key)
            .map_err(|err| JsonError::from_domain(err, &join_pointer(path, "event_key")))?;
    }
    block.travel_before = travel_duration(
        wire.travel_before.as_deref(),
        &join_pointer(path, "travel_before"),
    )?;
    block.travel_after = travel_duration(
        wire.travel_after.as_deref(),
        &join_pointer(path, "travel_after"),
    )?;
    Ok(block)
}

/// The status a policy entry names, as the domain status it means.
fn status_of(status: WireStatus) -> BlockStatus {
    match status {
        WireStatus::Free => BlockStatus::Free,
        WireStatus::Tentative => BlockStatus::Tentative,
        WireStatus::Busy => BlockStatus::Busy,
    }
}

/// The status a block string names: `free`, `tentative`, or `busy`, with any
/// unrecognized provider status defaulting fail-closed to `busy`. A
/// status-policy entry's status is a closed set instead — see
/// [`WireStatus`](crate::wire::WireStatus).
fn block_status(wire: &str) -> BlockStatus {
    match wire {
        "free" => BlockStatus::Free,
        "tentative" => BlockStatus::Tentative,
        _ => BlockStatus::Busy,
    }
}

/// The travel a block carries, which is none when the field is absent.
fn travel_duration(wire: Option<&str>, path: &str) -> Result<Duration, JsonError> {
    match wire {
        Some(duration) => parse_duration(duration, path),
        None => Ok(Duration::ZERO),
    }
}

/// The occurrences a wire document describes.
fn occurrences(wire: &[WireOccurrence], path: &str) -> Result<Vec<Occurrence>, JsonError> {
    wire.iter()
        .enumerate()
        .map(|(index, occurrence)| occurrence_of(occurrence, &join_index(path, index)))
        .collect()
}

/// One occurrence.
fn occurrence_of(wire: &WireOccurrence, path: &str) -> Result<Occurrence, JsonError> {
    let mut occurrence = Occurrence::new(
        wire.event_key.clone(),
        span(
            &wire.span,
            InstantRounding::Exact,
            InstantRounding::Exact,
            &join_pointer(path, "span"),
        )?,
        wire.participants.clone(),
    )
    .map_err(|err| {
        // A key failure is about the key's own field; a requirement with no
        // participants is about the list.
        let at = if err.code() == "event_key_empty" {
            join_pointer(path, "event_key")
        } else {
            join_pointer(path, "participants")
        };
        JsonError::from_domain(err, &at)
    })?;
    occurrence.tags = tags(&wire.tags, &join_pointer(path, "tags"))?;
    Ok(occurrence)
}

/// The working calendars a wire document describes.
fn calendars(
    wire: &BTreeMap<String, WireCalendarSpec>,
    path: &str,
) -> Result<BTreeMap<String, CalendarSpec>, JsonError> {
    wire.iter()
        .map(|(name, calendar)| {
            Ok((
                name.clone(),
                calendar_of(calendar, &join_pointer(path, name))?,
            ))
        })
        .collect()
}

/// One working calendar.
pub(crate) fn calendar_of(wire: &WireCalendarSpec, path: &str) -> Result<CalendarSpec, JsonError> {
    Ok(CalendarSpec {
        zone: zone(&wire.zone, &join_pointer(path, "zone"))?,
        working_days: match &wire.working_days {
            Some(days) => parse_working_days(days, &join_pointer(path, "working_days"))?,
            None => Weekdays::NONE,
        },
        holidays: wire
            .holidays
            .iter()
            .enumerate()
            .map(|(index, holiday)| {
                parse_date(holiday, &join_index(&join_pointer(path, "holidays"), index))
            })
            .collect::<Result<Vec<_>, _>>()?,
    })
}

/// The rules a wire document describes.
fn rules(wire: &[WireRule], path: &str) -> Result<Vec<Rule>, JsonError> {
    wire.iter()
        .enumerate()
        .map(|(index, rule)| self::rule(rule, &join_index(path, index)))
        .collect()
}

/// One rule.
pub(crate) fn rule(wire: &WireRule, path: &str) -> Result<Rule, JsonError> {
    let params_path = join_pointer(path, "params");
    let (id, who, when, kind) = match wire {
        WireRule::Window(WindowRule {
            id,
            who,
            when,
            params,
        }) => (
            id,
            who,
            when,
            RuleKind::Window(window_params(params, &params_path)?),
        ),
        WireRule::Blackout(BlackoutRule {
            id,
            who,
            when,
            params,
        }) => (
            id,
            who,
            when,
            RuleKind::Blackout(blackout_params(params, &params_path)?),
        ),
        WireRule::Clearance(ClearanceRule {
            id,
            who,
            when,
            params,
        }) => (
            id,
            who,
            when,
            RuleKind::Clearance(clearance(params, &params_path)?),
        ),
        WireRule::LeadTime(LeadTimeRule {
            id,
            who,
            when,
            params,
        }) => (
            id,
            who,
            when,
            RuleKind::LeadTime(lead_time(params, &params_path)?),
        ),
        WireRule::StatusPolicy(StatusPolicyRule {
            id,
            who,
            when,
            params,
        }) => (
            id,
            who,
            when,
            RuleKind::StatusPolicy(status_policy(params, &params_path)?),
        ),
        WireRule::Rest(RestRule {
            id,
            who,
            when,
            params,
        }) => (id, who, when, RuleKind::Rest(rest(params, &params_path)?)),
        WireRule::Cap(CapRule {
            id,
            who,
            when,
            params,
        }) => (id, who, when, RuleKind::Cap(cap(params, &params_path)?)),
    };
    Ok(Rule {
        id: id.clone(),
        kind,
        who: selector(who, &join_pointer(path, "who"))?,
        when: selector(when, &join_pointer(path, "when"))?,
    })
}

/// A window specification.
fn window_params(wire: &WindowParams, path: &str) -> Result<WindowSpec, JsonError> {
    let zone = rule_zone(&wire.zone, &join_pointer(path, "zone"))?;
    let weekly_path = join_pointer(path, "weekly");
    let weekly = wire
        .weekly
        .iter()
        .map(|(key, hours)| {
            let key_path = join_pointer(&weekly_path, key);
            WeeklyKey::new(
                parse_weekdays(key, &key_path)?,
                wall_ranges(hours, &key_path)?,
            )
            .map_err(|err| JsonError::from_domain(err.into(), &key_path))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let dates_path = join_pointer(path, "dates");
    let dates = wire
        .dates
        .iter()
        .map(|(key, hours)| {
            let key_path = join_pointer(&dates_path, key);
            DateKey::new(
                parse_date_range(key, &key_path)?,
                wall_ranges(hours, &key_path)?,
            )
            .map_err(|err| JsonError::from_domain(err.into(), &key_path))
        })
        .collect::<Result<Vec<_>, _>>()?;
    WindowSpec::new(zone, weekly, dates).map_err(|err| {
        // A repeated weekday is a fact about the whole `weekly` map, which is
        // where a caller has to look; a repeated date likewise for `dates`.
        let at = if err.code() == "window_weekday_repeated" {
            &weekly_path
        } else {
            &dates_path
        };
        JsonError::from_domain(err.into(), at)
    })
}

/// A blackout specification.
fn blackout_params(wire: &BlackoutParams, path: &str) -> Result<BlackoutSpec, JsonError> {
    let zone = rule_zone(&wire.zone, &join_pointer(path, "zone"))?;
    let spans_path = join_pointer(path, "spans");
    let spans = wire
        .spans
        .iter()
        .enumerate()
        .map(|(index, span)| {
            let span_path = join_index(&spans_path, index);
            BlackoutSpan::new(
                parse_local_date_time(&span.0, &join_index(&span_path, 0))?,
                parse_local_date_time(&span.1, &join_index(&span_path, 1))?,
            )
            .map_err(|err| JsonError::from_domain(err.into(), &span_path))
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(BlackoutSpec::new(zone, spans))
}

/// The gaps a booking needs.
fn clearance(wire: &ClearanceParams, path: &str) -> Result<Clearance, JsonError> {
    Ok(Clearance {
        before: parse_duration(&wire.before, &join_pointer(path, "before"))?,
        after: parse_duration(&wire.after, &join_pointer(path, "after"))?,
        min_gap: parse_duration(&wire.min_gap, &join_pointer(path, "min_gap"))?,
    })
}

/// How soon and how far ahead a booking may start.
fn lead_time(wire: &LeadTimeParams, path: &str) -> Result<LeadTime, JsonError> {
    let max = wire
        .max
        .as_ref()
        .map(|max| lead_time_max(max, &join_pointer(path, "max")))
        .transpose()?;
    Ok(LeadTime {
        min: parse_duration(&wire.min, &join_pointer(path, "min"))?,
        max,
    })
}

/// A `lead_time` maximum.
fn lead_time_max(wire: &WireLeadTimeMax, path: &str) -> Result<LeadTimeMax, JsonError> {
    Ok(match wire {
        WireLeadTimeMax::Duration(DurationMax { duration }) => {
            LeadTimeMax::Duration(parse_duration(duration, &join_pointer(path, "duration"))?)
        }
        WireLeadTimeMax::Days(DaysMax { days }) => LeadTimeMax::Days(*days),
        WireLeadTimeMax::WorkingDays(WorkingDaysMax {
            working_days,
            calendar,
        }) => LeadTimeMax::WorkingDays {
            count: *working_days,
            calendar: calendar.clone(),
        },
        WireLeadTimeMax::Date(DateMax { date }) => {
            LeadTimeMax::Date(parse_date(date, &join_pointer(path, "date"))?)
        }
    })
}

/// How statuses and tags affect availability.
fn status_policy(wire: &StatusPolicyParams, path: &str) -> Result<StatusPolicy, JsonError> {
    let entries_path = join_pointer(path, "entries");
    let entries = wire
        .entries
        .iter()
        .enumerate()
        .map(|(index, entry)| status_policy_entry(entry, &join_index(&entries_path, index)))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(StatusPolicy { entries })
}

/// One status-policy entry.
fn status_policy_entry(
    wire: &WireStatusPolicyEntry,
    path: &str,
) -> Result<StatusPolicyEntry, JsonError> {
    let entry = StatusPolicyEntry {
        status: wire.status.map(status_of),
        tags: wire
            .matched
            .as_ref()
            .map(|matched| self::selector(matched, &join_pointer(path, "match")))
            .transpose()?,
        effect: match wire.effect {
            WireStatusEffect::Ignore => StatusEffect::Ignore,
            WireStatusEffect::Block => StatusEffect::Block,
        },
    };
    // An entry that names neither a status nor a selector could never apply while
    // reading as if it would, so it is rejected where it was written.
    entry
        .validate()
        .map_err(|err| JsonError::from_domain(err, path))?;
    Ok(entry)
}

/// A rest rule.
fn rest(wire: &RestParams, path: &str) -> Result<Rest, JsonError> {
    Ok(Rest::new(
        parse_time_of_day(&wire.late_after, &join_pointer(path, "late_after"))?,
        parse_time_of_day(&wire.not_before, &join_pointer(path, "not_before"))?,
    ))
}

/// A cap.
fn cap(wire: &CapParams, path: &str) -> Result<Cap, JsonError> {
    Ok(Cap::new(
        selector(&wire.count, &join_pointer(path, "count"))?,
        match wire.measure {
            WireCapMeasure::Count => CapMeasure::Count,
            WireCapMeasure::Minutes => CapMeasure::Minutes,
        },
        wire.limit,
        match wire.period {
            WireCapPeriod::Day => CapPeriod::Day,
            WireCapPeriod::Week => CapPeriod::Week,
            WireCapPeriod::Month => CapPeriod::Month,
        },
        match wire.per {
            WireCapPer::Each => CapPer::Each,
            WireCapPer::Pooled => CapPer::Pooled,
        },
        rule_zone(&wire.zone, &join_pointer(path, "zone"))?,
        match &wire.week_starts {
            Some(day) => parse_weekday(day, &join_pointer(path, "week_starts"))?,
            None => Weekday::Monday,
        },
    ))
}

/// A candidate grid.
fn grid_of(wire: &WireGrid, path: &str) -> Result<Grid, JsonError> {
    let zone = match wire.zone.as_deref() {
        None | Some("event") => GridZone::Event,
        Some(name) => GridZone::Named(zone(name, &join_pointer(path, "zone"))?),
    };
    let anchor = match &wire.anchor {
        Some(anchor) => parse_time_of_day(anchor, &join_pointer(path, "anchor"))?,
        None => TimeOfDay::MIDNIGHT,
    };
    Grid::new(
        parse_duration(&wire.step, &join_pointer(path, "step"))?,
        anchor,
        zone,
    )
    .map_err(|err| JsonError::from_domain(err.into(), &join_pointer(path, "step")))
}

/// The booking a wire proposal describes.
pub(crate) fn proposal(wire: &WireProposal) -> Result<Proposal, JsonError> {
    let events = wire
        .events
        .iter()
        .enumerate()
        .map(|(index, event)| proposal_event(event, &join_index("/events", index)))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Proposal { events })
}

/// One proposed event.
fn proposal_event(wire: &WireProposalEvent, path: &str) -> Result<ProposalEvent, JsonError> {
    Ok(ProposalEvent {
        part: wire.part.clone(),
        start: parse_instant(
            &wire.start,
            InstantRounding::Exact,
            &join_pointer(path, "start"),
        )?,
        assignment: wire.assignment.clone(),
    })
}

/// The query a wire document describes.
pub(crate) fn ranges_query(wire: &WireRangesQuery) -> Result<RangesQuery, JsonError> {
    let mut query = RangesQuery::new(
        parse_instant(&wire.now, InstantRounding::Ceil, "/now")?,
        goal(&wire.goal, "/goal")?,
    );
    query.rules = rules(&wire.rules, "/rules")?;
    query.participants = participants(&wire.participants, "/participants")?;
    Ok(query)
}

/// The limits a wire document describes.
///
/// Every field is optional, and an omitted one — or an explicit `null` — takes
/// [`Limits::default`]'s value, so a document may name only the caps it changes.
pub(crate) fn limits(wire: &WireLimits) -> Limits {
    let defaults = Limits::default();
    defaults
        .with_max_participants(
            wire.max_participants
                .map_or(defaults.max_participants, count_as_usize),
        )
        .with_max_blocks_per_participant(
            wire.max_blocks_per_participant
                .map_or(defaults.max_blocks_per_participant, count_as_usize),
        )
        .with_max_search_window_days(
            wire.max_search_window_days
                .unwrap_or(defaults.max_search_window_days),
        )
        .with_max_candidate_starts(
            wire.max_candidate_starts
                .map_or(defaults.max_candidate_starts, count_as_usize),
        )
        .with_max_explain_limit(
            wire.max_explain_limit
                .map_or(defaults.max_explain_limit, count_as_usize),
        )
        .with_max_rules(wire.max_rules.map_or(defaults.max_rules, count_as_usize))
        .with_max_occurrences(
            wire.max_occurrences
                .map_or(defaults.max_occurrences, count_as_usize),
        )
        .with_max_requirement_slots(
            wire.max_requirement_slots
                .map_or(defaults.max_requirement_slots, count_as_usize),
        )
        .with_max_duration_minutes(
            wire.max_duration_minutes
                .unwrap_or(defaults.max_duration_minutes),
        )
        .with_max_zone_periods_per_participant(
            wire.max_zone_periods_per_participant
                .map_or(defaults.max_zone_periods_per_participant, count_as_usize),
        )
}

/// A span of instants, rounding each endpoint as `start` and `end` say.
fn span(
    wire: &WireSpan,
    start: InstantRounding,
    end: InstantRounding,
    path: &str,
) -> Result<Span, JsonError> {
    Span::new(
        parse_instant(&wire.0, start, &join_index(path, 0))?,
        parse_instant(&wire.1, end, &join_index(path, 1))?,
    )
    .map_err(|err| JsonError::from_domain(err, path))
}

/// The zone a wire document names.
fn zone(name: &str, path: &str) -> Result<ZoneName, JsonError> {
    ZoneName::new(name).map_err(|err| JsonError::from_domain(err.into(), path))
}

/// The rule zone a wire document names.
fn rule_zone(name: &str, path: &str) -> Result<RuleZone, JsonError> {
    match name {
        "participant" => Ok(RuleZone::Participant),
        "event" => Ok(RuleZone::Event),
        other => Ok(RuleZone::Named(zone(other, path)?)),
    }
}

/// The wall-clock ranges a wire document names.
fn wall_ranges(wire: &[String], path: &str) -> Result<Vec<WallRange>, JsonError> {
    wire.iter()
        .enumerate()
        .map(|(index, range)| parse_wall_range(range, &join_index(path, index)))
        .collect()
}

/// The tags a wire document names.
fn tags(wire: &[String], path: &str) -> Result<Vec<Tag>, JsonError> {
    wire.iter()
        .enumerate()
        .map(|(index, tag)| {
            Tag::new(tag)
                .map_err(|err| JsonError::from_domain(err.into(), &join_index(path, index)))
        })
        .collect()
}

/// The role a wire name means.
fn role_of(role: WireRole) -> Role {
    match role {
        WireRole::Host => Role::Host,
        WireRole::Attendee => Role::Attendee,
    }
}

/// The participant kind a wire name means.
fn kind_of(kind: WireParticipantKind) -> ParticipantKind {
    match kind {
        WireParticipantKind::Person => ParticipantKind::Person,
        WireParticipantKind::Resource => ParticipantKind::Resource,
    }
}

// ---------------------------------------------------------------------------
// Wire to domain as public conversions
// ---------------------------------------------------------------------------
//
// The `parse_*` entry points decode a document and convert it; these are the same
// conversions without the document, so a service that already holds a wire value —
// one it built itself, or deserialized as a field of its own request body — needs no
// JSON round trip. The owned forms forward to the borrowed ones, so a caller that
// owns its wire value does not have to keep it alive to convert it.

impl TryFrom<&WireRequest> for Request {
    type Error = JsonError;

    fn try_from(wire: &WireRequest) -> Result<Self, Self::Error> {
        request(wire)
    }
}

impl TryFrom<WireRequest> for Request {
    type Error = JsonError;

    fn try_from(wire: WireRequest) -> Result<Self, Self::Error> {
        Self::try_from(&wire)
    }
}

impl TryFrom<&WireGoal> for Goal {
    type Error = JsonError;

    fn try_from(wire: &WireGoal) -> Result<Self, Self::Error> {
        goal(wire, "")
    }
}

impl TryFrom<WireGoal> for Goal {
    type Error = JsonError;

    fn try_from(wire: WireGoal) -> Result<Self, Self::Error> {
        Self::try_from(&wire)
    }
}

impl TryFrom<&WireRule> for Rule {
    type Error = JsonError;

    fn try_from(wire: &WireRule) -> Result<Self, Self::Error> {
        rule(wire, "")
    }
}

impl TryFrom<WireRule> for Rule {
    type Error = JsonError;

    fn try_from(wire: WireRule) -> Result<Self, Self::Error> {
        Self::try_from(&wire)
    }
}

impl TryFrom<&WireCalendarSpec> for CalendarSpec {
    type Error = JsonError;

    fn try_from(wire: &WireCalendarSpec) -> Result<Self, Self::Error> {
        calendar_of(wire, "")
    }
}

impl TryFrom<WireCalendarSpec> for CalendarSpec {
    type Error = JsonError;

    fn try_from(wire: WireCalendarSpec) -> Result<Self, Self::Error> {
        Self::try_from(&wire)
    }
}

impl TryFrom<&WireProposal> for Proposal {
    type Error = JsonError;

    fn try_from(wire: &WireProposal) -> Result<Self, Self::Error> {
        proposal(wire)
    }
}

impl TryFrom<WireProposal> for Proposal {
    type Error = JsonError;

    fn try_from(wire: WireProposal) -> Result<Self, Self::Error> {
        Self::try_from(&wire)
    }
}

impl TryFrom<&WireRangesQuery> for RangesQuery {
    type Error = JsonError;

    fn try_from(wire: &WireRangesQuery) -> Result<Self, Self::Error> {
        ranges_query(wire)
    }
}

impl TryFrom<WireRangesQuery> for RangesQuery {
    type Error = JsonError;

    fn try_from(wire: WireRangesQuery) -> Result<Self, Self::Error> {
        Self::try_from(&wire)
    }
}

impl TryFrom<&WireLimits> for Limits {
    type Error = JsonError;

    /// A limits document has no value the domain cannot hold — every field is a
    /// count, and the search-window days is read as the `u32` the domain uses — so
    /// this conversion never fails; it is a `TryFrom` so every wire-to-domain
    /// conversion reads the same way.
    fn try_from(wire: &WireLimits) -> Result<Self, Self::Error> {
        Ok(limits(wire))
    }
}

impl TryFrom<WireLimits> for Limits {
    type Error = JsonError;

    fn try_from(wire: WireLimits) -> Result<Self, Self::Error> {
        Self::try_from(&wire)
    }
}

impl TryFrom<&WireParticipant> for Participant {
    type Error = JsonError;

    fn try_from(wire: &WireParticipant) -> Result<Self, Self::Error> {
        participant_of(wire, "")
    }
}

impl TryFrom<WireParticipant> for Participant {
    type Error = JsonError;

    fn try_from(wire: WireParticipant) -> Result<Self, Self::Error> {
        Self::try_from(&wire)
    }
}

impl TryFrom<&WireBlock> for Block {
    type Error = JsonError;

    fn try_from(wire: &WireBlock) -> Result<Self, Self::Error> {
        block_of(wire, "")
    }
}

impl TryFrom<WireBlock> for Block {
    type Error = JsonError;

    fn try_from(wire: WireBlock) -> Result<Self, Self::Error> {
        Self::try_from(&wire)
    }
}

impl TryFrom<&WireOccurrence> for Occurrence {
    type Error = JsonError;

    fn try_from(wire: &WireOccurrence) -> Result<Self, Self::Error> {
        occurrence_of(wire, "")
    }
}

impl TryFrom<WireOccurrence> for Occurrence {
    type Error = JsonError;

    fn try_from(wire: WireOccurrence) -> Result<Self, Self::Error> {
        Self::try_from(&wire)
    }
}

// ---------------------------------------------------------------------------
// Domain to wire
// ---------------------------------------------------------------------------

impl From<&Request> for WireRequest {
    fn from(request: &Request) -> Self {
        Self {
            now: request.now.to_string(),
            goal: WireGoal::from(&request.goal),
            participants: request
                .participants
                .iter()
                .map(WireParticipant::from)
                .collect(),
            rules: request.rules.iter().map(WireRule::from).collect(),
            occurrences: request
                .occurrences
                .iter()
                .map(WireOccurrence::from)
                .collect(),
            exclude_event_keys: request.exclude_event_keys.clone(),
            calendars: request
                .calendars
                .iter()
                .map(|(name, calendar)| (name.clone(), WireCalendarSpec::from(calendar)))
                .collect(),
            weights: request.weights.clone(),
            limit: request.limit.map(count_as_u64),
            explain: request.explain.map(|explain| WireExplain {
                limit: count_as_u64(explain.limit),
            }),
        }
    }
}

impl From<&Goal> for WireGoal {
    fn from(goal: &Goal) -> Self {
        Self {
            parts: goal.parts.iter().map(WirePart::from).collect(),
            grid: goal.grid.as_ref().map(WireGrid::from),
            candidates: goal
                .candidates
                .as_ref()
                .map(|candidates| candidates.iter().map(Instant::to_string).collect()),
            search_window: WireSpan::from(&goal.search_window),
            zone: goal.zone.to_string(),
            display_zone: Some(goal.display_zone.to_string()),
        }
    }
}

impl From<&Grid> for WireGrid {
    fn from(grid: &Grid) -> Self {
        Self {
            step: format_duration(grid.step()),
            anchor: Some(grid.anchor().to_string()),
            zone: Some(match grid.zone_choice() {
                GridZone::Event => "event".to_owned(),
                GridZone::Named(name) => name.to_string(),
            }),
        }
    }
}

impl From<&Part> for WirePart {
    fn from(part: &Part) -> Self {
        Self {
            id: Some(part.id.clone()),
            tags: tag_names(&part.tags),
            duration: format_duration(part.duration),
            requirement: WireRequirement::from(&part.requirement),
            travel: Some(WireTravel::from(&part.travel)),
        }
    }
}

impl From<&Requirement> for WireRequirement {
    fn from(requirement: &Requirement) -> Self {
        Self {
            all: requirement.terms.iter().map(WireTerm::from).collect(),
        }
    }
}

impl From<&Term> for WireTerm {
    fn from(term: &Term) -> Self {
        match term {
            Term::Person { id } => Self::Person(PersonTerm { person: id.clone() }),
            Term::AtLeast { count, of } => Self::AtLeast(AtLeastTerm {
                at_least: *count,
                of: WireSelector::from(of),
            }),
        }
    }
}

impl From<&Travel> for WireTravel {
    fn from(travel: &Travel) -> Self {
        Self {
            before: format_duration(travel.before),
            after: format_duration(travel.after),
            by_participant: travel
                .by_participant
                .iter()
                .map(|(id, travel)| (id.clone(), WireParticipantTravel::from(travel)))
                .collect(),
        }
    }
}

impl From<&ParticipantTravel> for WireParticipantTravel {
    fn from(travel: &ParticipantTravel) -> Self {
        Self {
            before: format_duration(travel.before),
            after: format_duration(travel.after),
        }
    }
}

impl From<&Selector> for WireSelector {
    fn from(selector: &Selector) -> Self {
        match selector {
            Selector::True => Self::MatchAll(MatchAll),
            Selector::Tag(tag) => Self::Tag(tag.as_str().to_owned()),
            Selector::Any(selectors) => Self::Any(selectors.iter().map(Self::from).collect()),
            Selector::All(selectors) => Self::All(selectors.iter().map(Self::from).collect()),
            Selector::Not(selector) => Self::Not(Box::new(Self::from(selector.as_ref()))),
            Selector::Ids(ids) => Self::Ids(ids.clone()),
            Selector::Role(role) => Self::Role(match role {
                Role::Host => WireRole::Host,
                Role::Attendee => WireRole::Attendee,
            }),
            Selector::Kind(kind) => Self::Kind(match kind {
                ParticipantKind::Person => WireParticipantKind::Person,
                ParticipantKind::Resource => WireParticipantKind::Resource,
            }),
        }
    }
}

impl From<&Participant> for WireParticipant {
    fn from(participant: &Participant) -> Self {
        Self {
            id: participant.id.clone(),
            kind: Some(match participant.kind {
                ParticipantKind::Person => WireParticipantKind::Person,
                ParticipantKind::Resource => WireParticipantKind::Resource,
            }),
            role: Some(match participant.role {
                Role::Host => WireRole::Host,
                Role::Attendee => WireRole::Attendee,
            }),
            tags: tag_names(&participant.tags),
            zone: participant.zone.to_string(),
            zone_periods: participant
                .zone_periods
                .iter()
                .map(WireZonePeriod::from)
                .collect(),
            calendar: participant
                .calendar
                .as_ref()
                .map(|blocks| blocks.iter().map(WireBlock::from).collect()),
        }
    }
}

impl From<&ZonePeriod> for WireZonePeriod {
    fn from(period: &ZonePeriod) -> Self {
        Self {
            zone: period.zone().to_string(),
            from: period.from().to_string(),
            until: period.until().to_string(),
        }
    }
}

impl From<&Block> for WireBlock {
    fn from(block: &Block) -> Self {
        Self {
            span: WireSpan::from(&block.span),
            status: WireBlockStatus::from(block.status.as_str()),
            all_day: block.all_day,
            tags: tag_names(&block.tags),
            event_key: block.event_key.clone(),
            travel_before: (block.travel_before != Duration::ZERO)
                .then(|| format_duration(block.travel_before)),
            travel_after: (block.travel_after != Duration::ZERO)
                .then(|| format_duration(block.travel_after)),
        }
    }
}

impl From<&Occurrence> for WireOccurrence {
    fn from(occurrence: &Occurrence) -> Self {
        Self {
            event_key: occurrence.event_key.clone(),
            span: WireSpan::from(&occurrence.span),
            tags: tag_names(&occurrence.tags),
            participants: occurrence.participants.clone(),
        }
    }
}

impl From<&CalendarSpec> for WireCalendarSpec {
    fn from(calendar: &CalendarSpec) -> Self {
        Self {
            zone: calendar.zone.to_string(),
            working_days: (!calendar.working_days.is_empty())
                .then(|| weekday_names(calendar.working_days)),
            holidays: calendar.holidays.iter().map(ToString::to_string).collect(),
        }
    }
}

impl From<&Rule> for WireRule {
    fn from(rule: &Rule) -> Self {
        let id = rule.id.clone();
        let who = WireSelector::from(&rule.who);
        let when = WireSelector::from(&rule.when);
        match &rule.kind {
            RuleKind::Window(spec) => Self::Window(WindowRule {
                id,
                who,
                when,
                params: WindowParams::from(spec),
            }),
            RuleKind::Blackout(spec) => Self::Blackout(BlackoutRule {
                id,
                who,
                when,
                params: BlackoutParams::from(spec),
            }),
            RuleKind::Clearance(clearance) => Self::Clearance(ClearanceRule {
                id,
                who,
                when,
                params: ClearanceParams::from(clearance),
            }),
            RuleKind::LeadTime(lead_time) => Self::LeadTime(LeadTimeRule {
                id,
                who,
                when,
                params: LeadTimeParams::from(lead_time),
            }),
            RuleKind::StatusPolicy(policy) => Self::StatusPolicy(StatusPolicyRule {
                id,
                who,
                when,
                params: StatusPolicyParams::from(policy),
            }),
            RuleKind::Rest(rest) => Self::Rest(RestRule {
                id,
                who,
                when,
                params: RestParams::from(rest),
            }),
            RuleKind::Cap(cap) => Self::Cap(CapRule {
                id,
                who,
                when,
                params: CapParams::from(cap),
            }),
            // `RuleKind` is `#[non_exhaustive]`: a kind added after 0.1 has no wire
            // spelling yet, so a rule carrying it cannot be written.
            _ => unreachable!("a rule kind added after 0.1 cannot be written"),
        }
    }
}

impl From<&WindowSpec> for WindowParams {
    fn from(spec: &WindowSpec) -> Self {
        // The domain keeps the hours of each weekday rather than the keys they
        // were written under, so the keys are written back one day at a time.
        let weekly = Weekday::ALL
            .iter()
            .zip(spec.weekly())
            .filter(|(_, hours)| !hours.is_empty())
            .map(|(day, hours)| (day.to_string(), wall_range_names(hours)))
            .collect();
        let dates = spec
            .dates()
            .iter()
            .map(|key| (date_range_name(*key.dates()), wall_range_names(key.hours())))
            .collect();
        Self {
            zone: rule_zone_name(spec.zone()),
            weekly,
            dates,
        }
    }
}

impl From<&BlackoutSpec> for BlackoutParams {
    fn from(sorted: &BlackoutSpec) -> Self {
        Self {
            zone: rule_zone_name(sorted.zone()),
            spans: sorted
                .spans()
                .iter()
                .map(|span| WireSpan(span.start().to_string(), span.end().to_string()))
                .collect(),
        }
    }
}

impl From<&Clearance> for ClearanceParams {
    fn from(clearance: &Clearance) -> Self {
        Self {
            before: format_duration(clearance.before),
            after: format_duration(clearance.after),
            min_gap: format_duration(clearance.min_gap),
        }
    }
}

impl From<&LeadTime> for LeadTimeParams {
    fn from(lead_time: &LeadTime) -> Self {
        Self {
            min: format_duration(lead_time.min),
            max: lead_time.max.as_ref().map(WireLeadTimeMax::from),
        }
    }
}

impl From<&LeadTimeMax> for WireLeadTimeMax {
    fn from(max: &LeadTimeMax) -> Self {
        match max {
            LeadTimeMax::Duration(duration) => Self::Duration(DurationMax {
                duration: format_duration(*duration),
            }),
            LeadTimeMax::Days(days) => Self::Days(DaysMax { days: *days }),
            LeadTimeMax::WorkingDays { count, calendar } => Self::WorkingDays(WorkingDaysMax {
                working_days: *count,
                calendar: calendar.clone(),
            }),
            LeadTimeMax::Date(date) => Self::Date(DateMax {
                date: date.to_string(),
            }),
        }
    }
}

impl From<&StatusPolicy> for StatusPolicyParams {
    fn from(policy: &StatusPolicy) -> Self {
        Self {
            entries: policy
                .entries
                .iter()
                .map(WireStatusPolicyEntry::from)
                .collect(),
        }
    }
}

impl From<&StatusPolicyEntry> for WireStatusPolicyEntry {
    fn from(entry: &StatusPolicyEntry) -> Self {
        Self {
            status: entry.status.map(|status| match status {
                BlockStatus::Free => WireStatus::Free,
                BlockStatus::Tentative => WireStatus::Tentative,
                BlockStatus::Busy => WireStatus::Busy,
            }),
            matched: entry.tags.as_ref().map(WireSelector::from),
            effect: match entry.effect {
                StatusEffect::Ignore => WireStatusEffect::Ignore,
                StatusEffect::Block => WireStatusEffect::Block,
            },
        }
    }
}

impl From<&Rest> for RestParams {
    fn from(rest: &Rest) -> Self {
        Self {
            late_after: rest.late_after.to_string(),
            not_before: rest.not_before.to_string(),
        }
    }
}

impl From<&Cap> for CapParams {
    fn from(cap: &Cap) -> Self {
        Self {
            count: WireSelector::from(&cap.count),
            measure: match cap.measure {
                CapMeasure::Count => WireCapMeasure::Count,
                CapMeasure::Minutes => WireCapMeasure::Minutes,
            },
            limit: cap.limit,
            period: match cap.period {
                CapPeriod::Day => WireCapPeriod::Day,
                CapPeriod::Week => WireCapPeriod::Week,
                CapPeriod::Month => WireCapPeriod::Month,
                // `CapPeriod` is `#[non_exhaustive]`: a period added after 0.1 has
                // no wire spelling yet.
                _ => unreachable!("a cap period added after 0.1 cannot be written"),
            },
            per: match cap.per {
                CapPer::Each => WireCapPer::Each,
                CapPer::Pooled => WireCapPer::Pooled,
            },
            zone: rule_zone_name(&cap.zone),
            week_starts: Some(cap.week_starts.to_string()),
        }
    }
}

impl From<&Proposal> for WireProposal {
    fn from(proposal: &Proposal) -> Self {
        Self {
            events: proposal
                .events
                .iter()
                .map(WireProposalEvent::from)
                .collect(),
        }
    }
}

impl From<&ProposalEvent> for WireProposalEvent {
    fn from(event: &ProposalEvent) -> Self {
        Self {
            part: event.part.clone(),
            start: event.start.to_string(),
            assignment: event.assignment.clone(),
        }
    }
}

impl From<&RangesQuery> for WireRangesQuery {
    fn from(query: &RangesQuery) -> Self {
        Self {
            now: query.now.to_string(),
            goal: WireGoal::from(&query.goal),
            rules: query.rules.iter().map(WireRule::from).collect(),
            participants: query
                .participants
                .iter()
                .map(WireParticipant::from)
                .collect(),
        }
    }
}

impl From<&Limits> for WireLimits {
    fn from(limits: &Limits) -> Self {
        Self {
            max_participants: Some(count_as_u64(limits.max_participants)),
            max_blocks_per_participant: Some(count_as_u64(limits.max_blocks_per_participant)),
            max_search_window_days: Some(limits.max_search_window_days),
            max_candidate_starts: Some(count_as_u64(limits.max_candidate_starts)),
            max_explain_limit: Some(count_as_u64(limits.max_explain_limit)),
            max_rules: Some(count_as_u64(limits.max_rules)),
            max_occurrences: Some(count_as_u64(limits.max_occurrences)),
            max_requirement_slots: Some(count_as_u64(limits.max_requirement_slots)),
            max_duration_minutes: Some(limits.max_duration_minutes),
            max_zone_periods_per_participant: Some(count_as_u64(
                limits.max_zone_periods_per_participant,
            )),
        }
    }
}

impl From<&Span> for WireSpan {
    fn from(span: &Span) -> Self {
        Self(span.start().to_string(), span.end().to_string())
    }
}

impl From<&Response> for WireResponse {
    fn from(response: &Response) -> Self {
        Self {
            offers: response.offers.iter().map(WireOffer::from).collect(),
            dates: response.dates.iter().map(WireDateCount::from).collect(),
            truncated: response.truncated,
            excluded: response.excluded.iter().map(WireExcluded::from).collect(),
            excluded_truncated: response.excluded_truncated,
            warnings: response.warnings.iter().map(WireDiagnostic::from).collect(),
            meta: WireMeta::from(&response.meta),
        }
    }
}

impl From<&Offer> for WireOffer {
    fn from(offer: &Offer) -> Self {
        Self {
            events: offer.events.iter().map(WireOfferEvent::from).collect(),
            flags: offer
                .flags
                .iter()
                .map(|flag| flag.as_str().to_owned())
                .collect(),
        }
    }
}

impl From<&OfferEvent> for WireOfferEvent {
    fn from(event: &OfferEvent) -> Self {
        Self {
            part: event.part.clone(),
            start: event.start.to_string(),
            end: event.end.to_string(),
            eligible: event.eligible.clone(),
            assignment: event.assignment.clone(),
            reserve: event.reserve.iter().map(WireReserveSpan::from).collect(),
        }
    }
}

impl From<&ReserveSpan> for WireReserveSpan {
    fn from(span: &ReserveSpan) -> Self {
        Self {
            participant: span.participant.clone(),
            kind: span.kind.as_str().to_owned(),
            span: WireSpan::from(&span.span),
        }
    }
}

impl From<&DateCount> for WireDateCount {
    fn from(count: &DateCount) -> Self {
        Self {
            date: count.date.to_string(),
            offers: count_as_u64(count.offers),
        }
    }
}

impl From<&Excluded> for WireExcluded {
    fn from(excluded: &Excluded) -> Self {
        Self {
            start: excluded.start.to_string(),
            part: excluded.part.clone(),
            reasons: excluded.reasons.iter().map(WireReason::from).collect(),
        }
    }
}

impl From<&Reason> for WireReason {
    fn from(reason: &Reason) -> Self {
        Self {
            participant: reason.participant.clone(),
            code: reason.code.as_str().to_owned(),
            rule: reason.rule.clone(),
            event_key: reason.event_key.clone(),
            tags: tag_names(&reason.tags),
            params: reason.params.as_ref().and_then(reason_params),
        }
    }
}

/// The wire form of a reason's parameters.
///
/// [`ReasonParams`] is non-exhaustive, so a variant added to the core needs its
/// wire counterpart here before it can be reported; an unknown one is reported
/// without parameters rather than guessed at.
fn reason_params(params: &ReasonParams) -> Option<WireReasonParams> {
    if let ReasonParams::CapReached {
        used,
        limit,
        bucket,
    } = params
    {
        Some(WireReasonParams {
            used: *used,
            limit: *limit,
            bucket: bucket.clone(),
        })
    } else {
        None
    }
}

impl From<&Diagnostic> for WireDiagnostic {
    fn from(diagnostic: &Diagnostic) -> Self {
        Self {
            code: diagnostic.code().to_owned(),
            message: diagnostic.message().to_owned(),
            path: diagnostic.path().map(ToOwned::to_owned),
        }
    }
}

impl From<&Meta> for WireMeta {
    fn from(meta: &Meta) -> Self {
        Self {
            engine: meta.engine.clone(),
            schema: meta.schema,
            tzdb: meta.tzdb.clone(),
        }
    }
}

impl From<&SlotCheck> for WireSlotCheck {
    fn from(check: &SlotCheck) -> Self {
        Self {
            valid: check.valid,
            reasons: check.reasons.iter().map(WireReason::from).collect(),
            flags: check
                .flags
                .iter()
                .map(|flag| flag.as_str().to_owned())
                .collect(),
            reserve: check.reserve.iter().map(WireReserveSpan::from).collect(),
            warnings: check.warnings.iter().map(WireDiagnostic::from).collect(),
            meta: WireMeta::from(&check.meta),
        }
    }
}

impl From<&RequiredRanges> for WireRequiredRanges {
    fn from(ranges: &RequiredRanges) -> Self {
        Self {
            calendar: WireSpan::from(&ranges.calendar),
            occurrences: ranges.occurrences.as_ref().map(WireSpan::from),
        }
    }
}

impl From<&Diagnostics> for WireDiagnostics {
    fn from(diagnostics: &Diagnostics) -> Self {
        Self {
            errors: diagnostics
                .errors
                .iter()
                .map(WireDiagnostic::from)
                .collect(),
            warnings: diagnostics
                .warnings
                .iter()
                .map(WireDiagnostic::from)
                .collect(),
        }
    }
}

/// The wire names of a list of tags.
fn tag_names(tags: &[Tag]) -> Vec<String> {
    tags.iter().map(|tag| tag.as_str().to_owned()).collect()
}

/// The wire names of a set of weekdays, in Monday-first order.
fn weekday_names(days: Weekdays) -> String {
    let names: Vec<String> = days.iter().map(|day| day.to_string()).collect();
    names.join(",")
}

/// The wire name of a wall-clock range.
fn wall_range_name(range: WallRange) -> String {
    format!("{}-{}", range.start(), range.end())
}

/// The wire names of a list of wall-clock ranges.
fn wall_range_names(ranges: &[WallRange]) -> Vec<String> {
    ranges.iter().copied().map(wall_range_name).collect()
}

/// The wire key of a date range: the date itself when it covers one.
fn date_range_name(range: DateRange) -> String {
    if range.start() == range.end() {
        range.start().to_string()
    } else {
        format!("{}..{}", range.start(), range.end())
    }
}

/// The wire name of a rule zone selection.
fn rule_zone_name(zone: &RuleZone) -> String {
    match zone {
        RuleZone::Participant => "participant".to_owned(),
        RuleZone::Event => "event".to_owned(),
        RuleZone::Named(name) => name.to_string(),
    }
}
