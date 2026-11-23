//! Static validation: what is wrong with a request, found without computing
//! availability.
//!
//! Errors are reserved for requests that are structurally impossible. Anything
//! that depends on `now`, on date overrides or on calendars produces empty
//! results and a warning instead, because it may well be answerable later.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use crate::Engine;
use crate::error::{Diagnostic, Diagnostics, Error, escape_pointer_token};
use crate::eval::{
    FixedMinutes, PreResolved, Settings, WindowContext, candidate_starts_limited, padded_range,
    resolve_lead_bound, resolve_schedule,
};
use crate::intervals::{IntervalSet, Span as MinuteSpan};
use crate::limits::Limits;
use crate::matching;
use crate::model::{
    Block, Cap, CapPer, LeadTimeMax, Part, Participant, ParticipantKind, Proposal, ProposalEvent,
    Request, Rule, RuleKind, Span, Term,
};
use crate::occupancy;
use crate::rules::{Applicable, RuleMatcher, named_ids, named_tags, names_participants};
use crate::tags::{Role, Tag};
use crate::time::{
    Duration, GridZone, Instant, RuleZone, StartBound, TimeZoneDatabase, WorkingCalendar,
    ZoneSchedule,
};

/// Validates `request` against `engine`'s database and limits.
///
/// A search has to generate candidate starts, so it runs [`common_checks`]
/// and then the checks that bound and explain that generation — unless the
/// cheap caps or the duration bounds already rejected the request, in which case
/// that generation is exactly the work the rejection refused to pay for.
pub(crate) fn validate(engine: &Engine, request: &Request) -> Diagnostics {
    validated_search(engine, request, true).diagnostics
}

/// What a search's validation found, together with the candidate starts it
/// generated and any request-scoped resolutions evaluation can reuse.
pub(crate) struct ValidatedSearch<'a> {
    pub(crate) diagnostics: Diagnostics,
    pub(crate) candidates: Vec<Instant>,
    pub(crate) pre_resolved: Option<PreResolved<'a>>,
}

/// Validates a search for `find_availability`: skips the standalone
/// `no_candidate_in_window` pass because `find_availability` rejects invalid
/// requests before emitting warnings and computes `no_candidate_in_window`
/// directly from the `allowed` sets `eval::search_settings_resolved` builds.
pub(crate) fn validate_for_search<'a>(
    engine: &Engine,
    request: &'a Request,
) -> ValidatedSearch<'a> {
    validated_search(engine, request, false)
}

/// Validates a search, optionally running the standalone
/// `no_candidate_in_window` pass.
fn validated_search<'a>(
    engine: &Engine,
    request: &'a Request,
    check_windows: bool,
) -> ValidatedSearch<'a> {
    // A search judges only the starts inside its window, so a `working_days`
    // horizon past the window's end allows every one of them: the calendar walk is
    // bounded there rather than run to the end of the supported calendar.
    let mut common = common_checks(engine, request, Some(request.goal.search_window.end()));
    // A request the cheap caps and duration bounds rejected is answered by that
    // phase alone: the candidate generation a search check would pay for is
    // exactly the work the cap just refused.
    let candidates = if common.stopped_at_cap {
        Vec::new()
    } else {
        validate_search(
            engine,
            request,
            &common.schedules,
            &common.rules,
            check_windows,
            &mut common.diagnostics,
        )
    };
    let pre_resolved = common.take_pre_resolved(request.participants.len());
    ValidatedSearch {
        diagnostics: common.diagnostics,
        candidates,
        pre_resolved,
    }
}

/// The checks `check_slot` needs, returning both diagnostics and the resolved
/// calendars/schedules/rules/lead bounds so `slot_settings_resolved` does not
/// repeat them.
pub(crate) fn validate_for_slot<'a>(
    engine: &Engine,
    request: &'a Request,
) -> (Diagnostics, Option<PreResolved<'a>>) {
    // A confirmation judges the start it proposes, which it may have chosen
    // outside the search window, so its horizons are resolved exactly.
    let mut common = common_checks(engine, request, None);
    let pre = common.take_pre_resolved(request.participants.len());
    (common.diagnostics, pre)
}

/// Appends `no_candidate_in_window` warnings for a valid search directly from
/// the `allowed` sets already built by `eval::search_settings_resolved`.
pub(crate) fn append_window_warnings(settings: &Settings<'_>, warnings: &mut Vec<Diagnostic>) {
    if settings.candidates.is_empty() {
        return;
    }
    for participant in &settings.participants {
        let Some(allowed) = &participant.allowed else {
            continue;
        };
        let mut any_span = false;
        let mut inside = false;
        for &start in &settings.candidates {
            let Some(span) = occupied_span(settings.part, &participant.participant.id, start)
            else {
                continue;
            };
            let Ok(minutes) = MinuteSpan::new(span.start().minutes(), span.end().minutes()) else {
                continue;
            };
            any_span = true;
            if allowed.contains_span(&minutes) {
                inside = true;
                break;
            }
        }
        if any_span && !inside {
            push_no_candidate_in_window(&participant.participant.id, warnings);
        }
    }
}

/// What the checks every request needs found, and whether the cheap phase stopped
/// them.
#[derive(Default)]
struct CommonChecks<'a> {
    diagnostics: Diagnostics,
    /// Whether the request was rejected by the cheap caps or duration bounds, so
    /// no later check saw it. [`validate`] then has no search to add.
    stopped_at_cap: bool,
    calendars: Option<BTreeMap<String, WorkingCalendar>>,
    schedules: Vec<Option<ZoneSchedule>>,
    rules: Vec<Applicable<'a>>,
    lead_bounds: BTreeMap<usize, StartBound>,
}

impl<'a> CommonChecks<'a> {
    /// Extracts [`PreResolved`] when every calendar, schedule and rule selection
    /// was resolved cleanly.
    fn take_pre_resolved(&mut self, participant_count: usize) -> Option<PreResolved<'a>> {
        let calendars = self.calendars.take()?;
        if self.schedules.len() != participant_count || self.rules.len() != participant_count {
            return None;
        }
        let schedules: Option<Vec<ZoneSchedule>> =
            std::mem::take(&mut self.schedules).into_iter().collect();
        Some(PreResolved {
            calendars,
            schedules: schedules?,
            rules: std::mem::take(&mut self.rules),
            lead_bounds: std::mem::take(&mut self.lead_bounds),
        })
    }
}

/// Resolution artifacts gathered while validating a request's structure.
struct StructureResolved<'a> {
    participant_ids: BTreeSet<&'a str>,
    schedules: Vec<Option<ZoneSchedule>>,
    calendars: Option<BTreeMap<String, WorkingCalendar>>,
}

fn common_checks<'a>(
    engine: &Engine,
    request: &'a Request,
    horizon_end: Option<Instant>,
) -> CommonChecks<'a> {
    let mut common = CommonChecks::default();

    // The configured caps and the durations a request may name are cheap to check
    // and bound every later pass, so they run before the structural, requirement
    // and matching work. A request past one is reported without paying for the
    // work it would have justified: a huge rule set or requirement graph never
    // reaches the flow construction that made it expensive.
    check_limits(engine, request, &mut common.diagnostics);
    check_durations(engine, request, &mut common.diagnostics);
    if !common.diagnostics.is_valid() {
        common.stopped_at_cap = true;
        return common;
    }

    let structure = check_structure(engine, request, &mut common.diagnostics);
    common.schedules = structure.schedules;
    common.calendars = structure.calendars;
    // A goal that is not exactly one part is rejected by `part_count`, and the
    // requirement of a request the engine will not book is work nobody asked for:
    // a large multi-part goal would pay for every part's matching to answer a
    // question the part count already settled.
    if request.goal.parts.len() == 1 {
        check_requirement(request, &structure.participant_ids, &mut common.diagnostics);
    }

    let Some(part) = request.goal.part() else {
        return common;
    };
    let db = engine.time_zone_database();
    let Ok(event) = db.zone(request.goal.zone.as_str()) else {
        return common;
    };

    check_rule_reachability(request, &part.tags, &mut common.diagnostics);
    warn_about_ignored_resource_holds(request, &part.tags, &mut common.diagnostics);
    warn_about_keyless_blocks(request, &part.tags, &mut common.diagnostics);

    // Select applicable rules once per participant and reuse them across the
    // horizon warnings, standalone window warnings, and evaluation. The booking
    // half of applicability — `when`, and a cap's `count` — is decided once here
    // rather than once per participant, and every `who` is prepared for repeated
    // matching, so the request's cost follows its rules rather than its rules
    // times its participants.
    let matcher = RuleMatcher::new(&request.rules, &part.tags);
    let mut applicable_rules = Vec::with_capacity(request.participants.len());
    for participant in &request.participants {
        applicable_rules.push(matcher.select(participant.context()));
    }

    // The calendars were already resolved once by `check_calendars`. One that
    // could not be resolved is an error `check_calendars` already reported, so
    // the horizon warnings are skipped rather than re-resolving every calendar.
    if let Some(calendars) = &common.calendars {
        // A horizon's bound depends on `now`, the calendars and the event zone, and
        // not on the participant whose rule it came from: resolving it per rule
        // position means a `working_days` maximum walks its calendar once however
        // many participants the rule applies to.
        let mut bounds: BTreeMap<usize, StartBound> = BTreeMap::new();
        for (participant, applicable) in request.participants.iter().zip(&applicable_rules) {
            warn_if_horizon_ends_first(
                calendars,
                request,
                applicable,
                &participant.id,
                &event,
                horizon_end,
                &mut bounds,
                &mut common.diagnostics,
            );
        }
        common.lead_bounds = bounds;
    }
    common.rules = applicable_rules;

    common
}

/// The checks a search needs and a single proposed booking does not: the shape and
/// the limits of candidate generation, and (when `check_windows` is set) whether
/// any candidate's occupied range fits inside every window that applies.
///
/// The starts it generated come back to the caller, which evaluates that same
/// list. It is empty for a request that produces no start and for one these checks
/// stopped short of, which they only do when they reject it.
fn validate_search<'a>(
    engine: &Engine,
    request: &'a Request,
    schedules: &[Option<ZoneSchedule>],
    applicable_rules: &[Applicable<'a>],
    check_windows: bool,
    diagnostics: &mut Diagnostics,
) -> Vec<Instant> {
    check_search_shape(engine, request, diagnostics);
    // Candidate generation is bounded by the configured maximum plus one: the
    // limits check only needs to know whether a request exceeds it, and a search
    // window a caller wrote past every limit must not allocate a start per minute
    // of it twice. A caller's own start list states its size up front, so one past
    // the maximum is reported without sorting or copying a single start.
    let limit = engine.limits().max_candidate_starts;
    let declared_too_long = request
        .goal
        .candidates
        .as_ref()
        .is_some_and(|candidates| candidates.len() > limit);
    let candidates = if declared_too_long {
        Vec::new()
    } else {
        match candidate_starts_limited(engine, request, limit.saturating_add(1)) {
            Ok(candidates) => candidates,
            // Generation can only fail where the request names something the
            // database cannot resolve, or a grid whose dates cannot be walked.
            // That is reported here rather than replaced by an empty search, so a
            // caller reads the cause instead of "nothing is bookable". It is
            // reported at the field that wrote it even where a structural check
            // already named the same mistake, which is what keeps one bad name
            // from hiding another. Generation resolves the goal's zone first and a
            // grid that names no zone uses it, so a failure past that point is the
            // grid's: the zone it names, or its walk.
            Err(err) => {
                let at = if engine
                    .time_zone_database()
                    .zone(request.goal.zone.as_str())
                    .is_err()
                {
                    "/goal/zone".to_owned()
                } else {
                    "/goal/grid".to_owned()
                };
                diagnostics
                    .errors
                    .push(Diagnostic::new(err.code(), err.to_string()).with_path(at));
                return Vec::new();
            }
        }
    };

    check_search_limits(engine, request, &candidates, diagnostics);

    // The warning is a claim about the starts generation produced, so it is made
    // only for a list that is complete and non-empty. Generation stopped at the
    // configured maximum plus one never produced the starts past that point — one
    // of which might have fitted — so their absence proves nothing; and a list
    // with no start (a declared list past the cap, or a grid that landed nowhere)
    // has nothing to judge. Either way no participant's schedule or windows are
    // resolved to answer a question the list cannot.
    if !check_windows
        || candidates.is_empty()
        || candidates.len() > engine.limits().max_candidate_starts
    {
        return candidates;
    }
    let within_window_limit =
        search_window_within_limit(engine.limits(), request.goal.search_window);
    // An over-cap search window driven by a grid (e.g. 125 years at 1440m step)
    // can stay under `max_candidate_starts` while spanning hundreds of legal
    // envelopes: refuse window building here just as an over-cap candidate list
    // is skipped. Explicit candidate lists on an over-cap window are still judged
    // when their total grouped envelope work stays inside `envelope_limit`.
    if !within_window_limit && request.goal.candidates.is_none() {
        return candidates;
    }
    let envelope_limit = search_envelope_limit(engine.limits());

    let Some(part) = request.goal.part() else {
        return candidates;
    };
    let db = engine.time_zone_database();
    let Ok(event) = db.zone(request.goal.zone.as_str()) else {
        return candidates;
    };

    // Precompute candidate occupied spans once for participants using the part's
    // default travel (no `by_participant` override), avoiding up to
    // `participants × candidates` redundant span allocations.
    let default_spans = candidate_occupied_spans(part, None, &candidates);
    if within_window_limit {
        warn_windows_within_limit(
            db,
            &event,
            request,
            part,
            schedules,
            applicable_rules,
            &candidates,
            &default_spans,
            diagnostics,
        );
        return candidates;
    }

    // Over-cap search window with explicit `goal.candidates`: reuse memoized
    // fixed-zone/fixed-home windows per envelope span and refuse any participant
    // whose total grouped envelope work exceeds `envelope_limit`.
    let mut memos: BTreeMap<MinuteSpan, FixedMinutes<'a>> = BTreeMap::new();
    for (index, participant) in request.participants.iter().enumerate() {
        let Some(schedule) = schedules.get(index).and_then(Option::as_ref) else {
            continue;
        };
        let Some(applicable) = applicable_rules.get(index) else {
            continue;
        };
        if applicable.windows.is_empty() {
            continue;
        }
        let override_spans;
        let spans = if part.travel.by_participant.contains_key(&participant.id) {
            override_spans = candidate_occupied_spans(part, Some(&participant.id), &candidates);
            &override_spans
        } else {
            &default_spans
        };
        warn_if_no_candidate_fits_window_over_cap(
            db,
            &event,
            schedule,
            participant.zone.as_str(),
            &participant.id,
            &applicable.windows,
            spans,
            envelope_limit,
            &mut memos,
            diagnostics,
        );
    }
    candidates
}

/// Checks `no_candidate_in_window` for a search within `max_search_window_days`,
/// sharing one [`FixedMinutes`] memo over `padded_range` across every participant.
#[allow(clippy::too_many_arguments)]
fn warn_windows_within_limit<'a>(
    db: &TimeZoneDatabase,
    event: &crate::time::Zone,
    request: &'a Request,
    part: &Part,
    schedules: &[Option<ZoneSchedule>],
    applicable_rules: &[Applicable<'a>],
    candidates: &[Instant],
    default_spans: &[MinuteSpan],
    diagnostics: &mut Diagnostics,
) {
    let Ok(judged) = padded_range(&request.goal, part) else {
        return;
    };
    let mut fixed = FixedMinutes::default();
    for (index, participant) in request.participants.iter().enumerate() {
        let Some(schedule) = schedules.get(index).and_then(Option::as_ref) else {
            continue;
        };
        let Some(applicable) = applicable_rules.get(index) else {
            continue;
        };
        if applicable.windows.is_empty() {
            continue;
        }
        let override_spans;
        let spans = if part.travel.by_participant.contains_key(&participant.id) {
            override_spans = candidate_occupied_spans(part, Some(&participant.id), candidates);
            &override_spans
        } else {
            default_spans
        };
        if spans.is_empty() {
            continue;
        }
        let ctx = WindowContext {
            db,
            schedule,
            home_zone_name: participant.zone.as_str(),
            event,
            judged,
        };
        let mut allowed: Option<Arc<IntervalSet>> = None;
        let mut failed_build = false;
        for &(_, position, spec) in &applicable.windows {
            let Ok(minutes) = fixed.window(position, spec, &ctx) else {
                failed_build = true;
                break;
            };
            allowed = Some(match allowed {
                Some(current) => Arc::new(current.intersection(&minutes)),
                None => minutes,
            });
            if allowed.as_ref().is_some_and(|set| set.is_empty()) {
                break;
            }
        }
        let inside = !failed_build
            && allowed.is_some_and(|allowed| spans.iter().any(|span| allowed.contains_span(span)));
        if !inside {
            push_no_candidate_in_window(&participant.id, &mut diagnostics.warnings);
        }
    }
}

/// The structural checks: what can be wrong with a request's shape.
///
/// Returns the set of declared participant IDs, each participant's resolved
/// schedule (when valid), and the resolved working calendars (when every
/// calendar is valid) so later passes never re-scan or re-resolve them.
fn check_structure<'a>(
    engine: &Engine,
    request: &'a Request,
    diagnostics: &mut Diagnostics,
) -> StructureResolved<'a> {
    let db = engine.time_zone_database();
    let participant_ids = check_participant_shapes(request, diagnostics);
    check_goal_shape(db, request, diagnostics);
    let schedules = check_participant_zones(db, request, diagnostics);
    let calendars = check_calendars(db, request, diagnostics);
    check_rules(db, request, &participant_ids, diagnostics);
    check_occurrences(request, diagnostics);
    check_weights(request, &participant_ids, diagnostics);
    StructureResolved {
        participant_ids,
        schedules,
        calendars,
    }
}

/// One participant a weight names must be in the request: a weight for somebody
/// absent is a caller's mistake, not an assignment the engine can honour.
fn check_weights(
    request: &Request,
    participant_ids: &BTreeSet<&str>,
    diagnostics: &mut Diagnostics,
) {
    for id in request.weights.keys() {
        if !participant_ids.contains(id.as_str()) {
            diagnostics.errors.push(
                Diagnostic::new(
                    "participant_unknown",
                    format!("`weights` names participant `{id}`, which the request does not carry"),
                )
                .with_path(format!("/weights/{}", escape_pointer_token(id))),
            );
        }
    }
}

/// Participant IDs must be non-blank and unique, and every block that names an
/// event must name one. Returns the set of declared participant IDs.
fn check_participant_shapes<'a>(
    request: &'a Request,
    diagnostics: &mut Diagnostics,
) -> BTreeSet<&'a str> {
    let mut seen = BTreeSet::new();
    for (index, participant) in request.participants.iter().enumerate() {
        if participant.id.trim().is_empty() {
            diagnostics.errors.push(
                Diagnostic::new(
                    "participant_id_empty",
                    format!(
                        "participant {index} has an empty or whitespace-only ID, so no reason, \
                         weight or assignment could name it"
                    ),
                )
                .with_path(format!("/participants/{index}/id")),
            );
        }
        if !seen.insert(participant.id.as_str()) {
            diagnostics.errors.push(
                Diagnostic::new(
                    "participant_duplicate",
                    format!("participant ID `{}` appears more than once", participant.id),
                )
                .with_path(format!("/participants/{index}/id")),
            );
        }
        for (block_index, block) in participant.blocks().iter().enumerate() {
            if block
                .event_key
                .as_deref()
                .is_some_and(|key| key.trim().is_empty())
            {
                diagnostics.errors.push(
                    Diagnostic::new(
                        "event_key_empty",
                        format!(
                            "block {block_index} of participant `{}` has an empty or \
                             whitespace-only event key, which would merge unrelated events \
                             into one cap count",
                            participant.id
                        ),
                    )
                    .with_path(format!(
                        "/participants/{index}/calendar/{block_index}/event_key"
                    )),
                );
            }
        }
    }
    seen
}

/// The goal: one part and resolvable zones.
fn check_goal_shape(db: &TimeZoneDatabase, request: &Request, diagnostics: &mut Diagnostics) {
    match request.goal.parts.len() {
        1 => {}
        count => diagnostics.errors.push(
            Diagnostic::new("part_count", Error::PartCount { parts: count }.to_string())
                .with_path("/goal/parts".to_owned()),
        ),
    }
    for (index, part) in request.goal.parts.iter().enumerate() {
        if part.id.trim().is_empty() {
            diagnostics.errors.push(
                Diagnostic::new(
                    "part_id_empty",
                    format!(
                        "part {index} has an empty or whitespace-only ID, so no proposal event \
                         could name it"
                    ),
                )
                .with_path(format!("/goal/parts/{index}/id")),
            );
        }
        if part.duration.minutes() < 1 {
            diagnostics.errors.push(
                Diagnostic::new(
                    "part_duration_empty",
                    format!(
                        "part `{}` has no length, so it can never be booked",
                        part.id
                    ),
                )
                .with_path(format!("/goal/parts/{index}/duration")),
            );
        }
    }
    for (what, name, field) in [
        ("goal zone", &request.goal.zone, "/goal/zone"),
        (
            "display zone",
            &request.goal.display_zone,
            "/goal/display_zone",
        ),
    ] {
        if let Err(err) = db.zone(name.as_str()) {
            diagnostics.errors.push(
                Diagnostic::new(err.code(), format!("{what} `{name}`: {err}"))
                    .with_path(field.to_owned()),
            );
        }
    }
}

/// The shape of the search a goal describes: one of a grid or candidate starts,
/// and a grid zone that resolves.
///
/// A confirmation names the start it proposes, so neither the grid nor the search
/// window decides anything for it and neither has to be well formed.
fn check_search_shape(engine: &Engine, request: &Request, diagnostics: &mut Diagnostics) {
    let db = engine.time_zone_database();
    if request.goal.grid.is_some() && request.goal.candidates.is_some() {
        diagnostics.errors.push(
            Diagnostic::new(
                "grid_and_candidates",
                "a goal cannot have both a grid and candidate starts".to_owned(),
            )
            .with_path("/goal/candidates".to_owned()),
        );
    }
    if let Some(GridZone::Named(name)) = request
        .goal
        .grid
        .as_ref()
        .map(crate::time::Grid::zone_choice)
        && let Err(err) = db.zone(name.as_str())
    {
        diagnostics.errors.push(
            Diagnostic::new(err.code(), format!("grid zone: {err}"))
                .with_path("/goal/grid/zone".to_owned()),
        );
    }
}

/// Each participant's home zone and zone periods, returning the resolved
/// schedules so later validation and evaluation stages never re-resolve them.
fn check_participant_zones(
    db: &TimeZoneDatabase,
    request: &Request,
    diagnostics: &mut Diagnostics,
) -> Vec<Option<ZoneSchedule>> {
    let mut schedules = Vec::with_capacity(request.participants.len());
    for (index, participant) in request.participants.iter().enumerate() {
        // The home zone and the periods are checked separately so one unknown
        // name is reported once, against the place it was written.
        match db.zone(participant.zone.as_str()) {
            Err(err) => {
                diagnostics.errors.push(
                    Diagnostic::new(
                        err.code(),
                        format!("participant `{}` zone: {err}", participant.id),
                    )
                    .with_path(format!("/participants/{index}/zone")),
                );
                schedules.push(None);
            }
            Ok(_) => match resolve_schedule(db, participant) {
                Ok(schedule) => schedules.push(Some(schedule)),
                Err(err) => {
                    diagnostics.errors.push(
                        Diagnostic::new(
                            err.code(),
                            format!("participant `{}` zone periods: {err}", participant.id),
                        )
                        .with_path(format!("/participants/{index}/zone_periods")),
                    );
                    schedules.push(None);
                }
            },
        }
    }
    schedules
}

/// The named working calendars: a resolvable zone and a working day to count.
///
/// Returns `Some(calendars)` when every calendar resolved cleanly, so the
/// horizon warnings and evaluation reuse the same `WorkingCalendar`s without
/// re-sorting holidays.
fn check_calendars(
    db: &TimeZoneDatabase,
    request: &Request,
    diagnostics: &mut Diagnostics,
) -> Option<BTreeMap<String, WorkingCalendar>> {
    let mut calendars = BTreeMap::new();
    let mut all_valid = true;
    for (name, calendar) in &request.calendars {
        let key = escape_pointer_token(name);
        match db.zone(calendar.zone.as_str()) {
            Ok(zone) => {
                match WorkingCalendar::new(zone, calendar.working_days, calendar.holidays.clone()) {
                    Ok(resolved) => {
                        if all_valid {
                            calendars.insert(name.clone(), resolved);
                        }
                    }
                    Err(err) => {
                        all_valid = false;
                        diagnostics.errors.push(
                            Diagnostic::new(err.code(), format!("calendar `{name}`: {err}"))
                                .with_path(format!("/calendars/{key}/working_days")),
                        );
                    }
                }
            }
            Err(err) => {
                all_valid = false;
                diagnostics.errors.push(
                    Diagnostic::new(err.code(), format!("calendar `{name}` zone: {err}"))
                        .with_path(format!("/calendars/{key}/zone")),
                );
            }
        }
    }
    all_valid.then_some(calendars)
}

/// Rule IDs, zones, and clauses that can never match.
fn check_rules(
    db: &TimeZoneDatabase,
    request: &Request,
    participant_ids: &BTreeSet<&str>,
    diagnostics: &mut Diagnostics,
) {
    let mut rule_ids = BTreeSet::new();
    for (position, rule) in request.rules.iter().enumerate() {
        if rule.id.trim().is_empty() {
            diagnostics.errors.push(
                Diagnostic::new(
                    "rule_id_empty",
                    format!(
                        "rule {position} has an empty or whitespace-only ID, so no finding or reason \
                         could cite it"
                    ),
                )
                .with_path(format!("/rules/{position}/id")),
            );
        }
        if !rule_ids.insert(rule.id.as_str()) {
            diagnostics.errors.push(
                Diagnostic::new(
                    "rule_id_duplicate",
                    format!("rule ID `{}` appears more than once", rule.id),
                )
                .with_path(format!("/rules/{position}/id")),
            );
        }
        check_rule_zones(db, request, position, rule, diagnostics);
        // `late_after` is the evening threshold and `not_before` the next
        // morning's, so a rule whose late threshold is not after its early one has
        // its two arguments the wrong way round far more often than it means the
        // window it describes.
        if let RuleKind::Rest(rest) = &rule.kind
            && rest.late_after <= rest.not_before
        {
            diagnostics.warnings.push(
                Diagnostic::new(
                    "rest_window_overlaps_day",
                    format!(
                        "rest rule `{}` has `late_after` ({}) at or before `not_before` ({}): \
                         `late_after` is the evening threshold and `not_before` the next \
                         morning's, so this order does not protect the night between a late \
                         finish and an early start",
                        rule.id, rest.late_after, rest.not_before
                    ),
                )
                .with_path(format!("/rules/{position}/params")),
            );
        }
        if names_participants(&rule.when) {
            diagnostics.errors.push(
                Diagnostic::new(
                    "selector_not_for_items",
                    format!(
                        "rule `{}` matches part tags with an `ids`, `role` or `kind` clause, which \
                         can never match a part's tags",
                        rule.id
                    ),
                )
                .with_path(format!("/rules/{position}/when")),
            );
        }
        for id in named_ids(&rule.who) {
            if !participant_ids.contains(id) {
                diagnostics.errors.push(
                    Diagnostic::new(
                        "participant_unknown",
                        format!(
                            "rule `{}` names participant `{id}` in `who`, which the request does not \
                             carry",
                            rule.id
                        ),
                    )
                    .with_path(format!("/rules/{position}/who")),
                );
            }
        }
    }
}

/// The zones a rule names, and the entries a status policy has to be able to
/// match.
fn check_rule_zones(
    db: &TimeZoneDatabase,
    request: &Request,
    position: usize,
    rule: &crate::model::Rule,
    diagnostics: &mut Diagnostics,
) {
    let mut named_zone = |name: &crate::time::ZoneName, what: &str| {
        if let Err(err) = db.zone(name.as_str()) {
            diagnostics.errors.push(
                Diagnostic::new(err.code(), format!("rule `{}` {what} zone: {err}", rule.id))
                    .with_path(format!("/rules/{position}/params/zone")),
            );
        }
    };
    match &rule.kind {
        RuleKind::Window(spec) => {
            if let RuleZone::Named(name) = spec.zone() {
                named_zone(name, "window");
            }
        }
        RuleKind::Blackout(spec) => {
            if let RuleZone::Named(name) = spec.zone() {
                named_zone(name, "blackout");
            }
        }
        RuleKind::LeadTime(lead_time) => {
            if let Some(LeadTimeMax::WorkingDays { calendar, .. }) = &lead_time.max
                && !request.calendars.contains_key(calendar)
            {
                diagnostics.errors.push(
                    Diagnostic::new(
                        "calendar_unknown",
                        format!(
                            "rule `{}` names calendar `{calendar}`, which the request does not \
                             carry",
                            rule.id
                        ),
                    )
                    .with_path(format!("/rules/{position}/params/max/calendar")),
                );
            }
        }
        RuleKind::StatusPolicy(policy) => {
            for (entry_index, entry) in policy.entries.iter().enumerate() {
                if let Err(err) = entry.validate() {
                    diagnostics.errors.push(
                        Diagnostic::new(
                            err.code(),
                            format!("rule `{}` status policy: {err}", rule.id),
                        )
                        .with_path(format!("/rules/{position}/params/entries/{entry_index}")),
                    );
                }
                if let Some(selector) = &entry.tags
                    && names_participants(selector)
                {
                    diagnostics.errors.push(
                        Diagnostic::new(
                            "selector_not_for_items",
                            format!(
                                "rule `{}` matches block tags with an `ids`, `role` or `kind` clause, \
                                 which can never match a block's tags",
                                rule.id
                            ),
                        )
                        .with_path(format!(
                            "/rules/{position}/params/entries/{entry_index}/match"
                        )),
                    );
                }
            }
        }
        RuleKind::Cap(cap) => {
            if let RuleZone::Named(name) = &cap.zone {
                named_zone(name, "cap");
            }
            if cap.per == CapPer::Pooled && cap.zone == RuleZone::Participant {
                diagnostics.errors.push(
                    Diagnostic::new(
                        "cap_pooled_participant_zone",
                        format!(
                            "rule `{}` is pooled, so it must read its buckets in `event` or a named \
                             zone: a pool has no single participant zone to bucket in",
                            rule.id
                        ),
                    )
                    .with_path(format!("/rules/{position}/params/zone")),
                );
            }
            if names_participants(&cap.count) {
                diagnostics.errors.push(
                    Diagnostic::new(
                        "selector_not_for_items",
                        format!(
                            "rule `{}` counts events with an `ids`, `role` or `kind` clause, which can \
                             never match an event's tags",
                            rule.id
                        ),
                    )
                    .with_path(format!("/rules/{position}/params/count")),
                );
            }
        }
        // Neither a clearance nor a rest rule names a zone or a calendar: rest's
        // late and early hours are wall-clock times in the participant's own zone
        // at each instant.
        RuleKind::Clearance(_) | RuleKind::Rest(_) => {}
    }
}

/// Occurrences must name at least one participant, their keys must be real, and
/// their keys are unique: two occurrences with one key would be two different
/// spans for the same event.
fn check_occurrences(request: &Request, diagnostics: &mut Diagnostics) {
    let mut seen = BTreeSet::new();
    for (index, occurrence) in request.occurrences.iter().enumerate() {
        let key = occurrence.event_key.trim();
        if key.is_empty() {
            diagnostics.errors.push(
                Diagnostic::new(
                    "event_key_empty",
                    format!(
                        "occurrence {index} has an empty or whitespace-only event key, which \
                         would merge unrelated events into one cap count"
                    ),
                )
                .with_path(format!("/occurrences/{index}/event_key")),
            );
        } else if !seen.insert(occurrence.event_key.as_str()) {
            diagnostics.errors.push(
                Diagnostic::new(
                    "occurrence_key_duplicate",
                    format!(
                        "occurrence event key `{}` appears more than once, so the event would have \
                         two spans",
                        occurrence.event_key
                    ),
                )
                .with_path(format!("/occurrences/{index}/event_key")),
            );
        }
        if occurrence.participants.is_empty() {
            diagnostics.errors.push(
                Diagnostic::new(
                    "occurrence_without_participants",
                    format!(
                        "occurrence `{}` names no participants",
                        occurrence.event_key
                    ),
                )
                .with_path(format!("/occurrences/{index}/participants")),
            );
        }
    }
}

/// The configured limits that bound a request's size, which hold whether or not a
/// search runs.
fn check_limits(engine: &Engine, request: &Request, diagnostics: &mut Diagnostics) {
    let limits = engine.limits();

    if let Some(explain) = &request.explain
        && explain.limit > limits.max_explain_limit
    {
        diagnostics.errors.push(
            Diagnostic::new(
                "explain_limit",
                format!(
                    "the request asks for {} explanation reasons, above the configured maximum \
                     of {}",
                    explain.limit, limits.max_explain_limit
                ),
            )
            .with_path("/explain/limit".to_owned()),
        );
    }
    diagnostics.errors.extend(size_limit_diagnostics(
        limits,
        &request.participants,
        &request.rules,
    ));
    for (index, participant) in request.participants.iter().enumerate() {
        if participant.blocks().len() > limits.max_blocks_per_participant {
            diagnostics.errors.push(
                Diagnostic::new(
                    "blocks_limit",
                    format!(
                        "participant `{}` holds {} blocks, above the configured maximum of {}",
                        participant.id,
                        participant.blocks().len(),
                        limits.max_blocks_per_participant
                    ),
                )
                .with_path(format!("/participants/{index}/calendar")),
            );
        }
    }
    if request.occurrences.len() > limits.max_occurrences {
        diagnostics.errors.push(
            Diagnostic::new(
                "occurrences_limit",
                format!(
                    "the request holds {} occurrences, above the configured maximum of {}",
                    request.occurrences.len(),
                    limits.max_occurrences
                ),
            )
            .with_path("/occurrences".to_owned()),
        );
    }
    check_exclude_event_keys(limits, request, diagnostics);
    check_requirement_limits(limits, request, diagnostics);
}

/// The checks on the event keys a request excludes: how many it may name, and that
/// each names an event.
///
/// A key with no content would match no event, and a list longer than the
/// occurrence cap is more exclusion work than any request may ask for. That length
/// is reported on its own, before the entries are read, so a document validation
/// rejects is never scanned one key at a time.
fn check_exclude_event_keys(limits: &Limits, request: &Request, diagnostics: &mut Diagnostics) {
    let keys = &request.exclude_event_keys;
    if keys.len() > limits.max_occurrences {
        diagnostics.errors.push(
            Diagnostic::new(
                "exclude_event_keys_limit",
                format!(
                    "the request excludes {} event keys, above the configured maximum of {}",
                    keys.len(),
                    limits.max_occurrences
                ),
            )
            .with_path("/exclude_event_keys".to_owned()),
        );
        return;
    }
    for (index, key) in keys.iter().enumerate() {
        if key.trim().is_empty() {
            diagnostics.errors.push(
                Diagnostic::new(
                    "event_key_empty",
                    format!(
                        "excluded event key {index} is empty or whitespace-only, so it would \
                         exclude no event"
                    ),
                )
                .with_path(format!("/exclude_event_keys/{index}")),
            );
        }
    }
}

/// The caps on one requirement: the hosts it asks for, and the terms it names.
///
/// The slot count bounds the hosts, and a zero-count term asks for none: without
/// a cap on the term count any number of them would reach the joint fillability
/// check, which builds a candidate list per term.
fn check_requirement_limits(limits: &Limits, request: &Request, diagnostics: &mut Diagnostics) {
    // Only a single-part goal has a requirement the engine will evaluate. Any
    // other shape is answered by `part_count` in the structural pass, without
    // scanning terms that can never reach matching.
    if request.goal.parts.len() != 1 {
        return;
    }
    for (index, part) in request.goal.parts.iter().enumerate() {
        let path = format!("/goal/parts/{index}/requirement/all");
        let slots = part.requirement.slots();
        if slots > limits.max_requirement_slots {
            diagnostics.errors.push(
                Diagnostic::new(
                    "requirement_slots_limit",
                    format!(
                        "part `{}` asks for {slots} hosts, above the configured maximum of {}",
                        part.id, limits.max_requirement_slots
                    ),
                )
                .with_path(path.clone()),
            );
        }
        let terms = part.requirement.terms.len();
        if terms > limits.max_requirement_slots {
            diagnostics.errors.push(
                Diagnostic::new(
                    "requirement_terms_limit",
                    format!(
                        "part `{}` names {terms} requirement terms, above the configured maximum \
                         of {}",
                        part.id, limits.max_requirement_slots
                    ),
                )
                .with_path(path),
            );
        }
    }
}

/// The findings for the collections whose length a request's size is measured by:
/// how many participants the document names, how many zone periods each of them
/// holds, and how many rules it carries.
///
/// These are the lists an evaluation multiplies its work by, so the findings are
/// shared with [`crate::ranges`], which bounds the same counts on the query it is
/// handed rather than trusting a caller to have validated the document first.
pub(crate) fn size_limit_diagnostics<'a>(
    limits: &'a Limits,
    participants: &'a [Participant],
    rules: &'a [Rule],
) -> impl Iterator<Item = Diagnostic> + 'a {
    let over_participants = (participants.len() > limits.max_participants).then(|| {
        Diagnostic::new(
            "participants_limit",
            format!(
                "the request holds {} participants, above the configured maximum of {}",
                participants.len(),
                limits.max_participants
            ),
        )
        .with_path("/participants".to_owned())
    });
    let over_rules = (rules.len() > limits.max_rules).then(|| {
        Diagnostic::new(
            "rules_limit",
            format!(
                "the request holds {} rules, above the configured maximum of {}",
                rules.len(),
                limits.max_rules
            ),
        )
        .with_path("/rules".to_owned())
    });
    over_participants
        .into_iter()
        .chain(zone_periods_diagnostics(
            limits.max_zone_periods_per_participant,
            participants,
        ))
        .chain(over_rules)
}

/// One `zone_periods_limit` finding for each participant whose zone periods
/// exceed `cap`.
///
/// The cap bounds a list nothing else bounds: a schedule reads every period, so an
/// untrusted request would otherwise be able to name unbounded work in one field.
fn zone_periods_diagnostics(
    cap: usize,
    participants: &[Participant],
) -> impl Iterator<Item = Diagnostic> + '_ {
    participants
        .iter()
        .enumerate()
        .filter_map(move |(index, participant)| {
            let periods = participant.zone_periods.len();
            (periods > cap).then(|| {
                Diagnostic::new(
                    "zone_periods_limit",
                    format!(
                        "participant `{}` holds {periods} zone periods, above the configured \
                         maximum of {cap}",
                        participant.id
                    ),
                )
                .with_path(format!("/participants/{index}/zone_periods"))
            })
        })
}

/// The durations a request may name: a booking, the travel a booking or a block
/// needs, and a clearance.
///
/// A duration bounds the range a request is judged over, and every interval set
/// the engine builds spans it, so one far past the calendar reaches a range no
/// search can pay to build. The bound is the same for a booking, either kind of
/// travel and a clearance because each one widens that range.
fn check_durations(engine: &Engine, request: &Request, diagnostics: &mut Diagnostics) {
    let maximum = i64::from(engine.limits().max_duration_minutes);
    // A block's travel widens the span it holds, and the span a booking is judged
    // against, so it is bounded like the booking's own travel.
    for (index, participant) in request.participants.iter().enumerate() {
        for (block_index, block) in participant.blocks().iter().enumerate() {
            for (field, duration) in [
                ("before", block.travel_before),
                ("after", block.travel_after),
            ] {
                // `check_duration`'s own bound, tested first: a calendar inside it
                // is the common case and can hold ten thousand blocks per
                // participant, so the two names a finding needs are built only for
                // a block that fails it.
                if duration.minutes() <= maximum {
                    continue;
                }
                check_duration(
                    diagnostics,
                    maximum,
                    duration,
                    "block_travel_duration_too_long",
                    &format!(
                        "the travel {field} block {block_index} of participant `{}`",
                        participant.id
                    ),
                    format!("/participants/{index}/calendar/{block_index}/travel_{field}"),
                );
            }
        }
    }
    for (index, part) in request.goal.parts.iter().enumerate() {
        check_duration(
            diagnostics,
            maximum,
            part.duration,
            "part_duration_too_long",
            &format!("part `{}`'s booking", part.id),
            format!("/goal/parts/{index}/duration"),
        );
        for (field, duration) in [("before", part.travel.before), ("after", part.travel.after)] {
            check_duration(
                diagnostics,
                maximum,
                duration,
                "travel_duration_too_long",
                &format!("part `{}`'s travel {field} the booking", part.id),
                format!("/goal/parts/{index}/travel/{field}"),
            );
        }
        for (id, travel) in &part.travel.by_participant {
            for (field, duration) in [("before", travel.before), ("after", travel.after)] {
                check_duration(
                    diagnostics,
                    maximum,
                    duration,
                    "travel_duration_too_long",
                    &format!("participant `{id}`'s travel {field} the booking"),
                    format!(
                        "/goal/parts/{index}/travel/by_participant/{}/{field}",
                        escape_pointer_token(id)
                    ),
                );
            }
        }
    }
    for (index, rule) in request.rules.iter().enumerate() {
        let RuleKind::Clearance(clearance) = &rule.kind else {
            continue;
        };
        for (field, duration) in [
            ("before", clearance.before),
            ("after", clearance.after),
            ("min_gap", clearance.min_gap),
        ] {
            check_duration(
                diagnostics,
                maximum,
                duration,
                "clearance_duration_too_long",
                &format!("clearance rule `{}`'s `{field}`", rule.id),
                format!("/rules/{index}/params/{field}"),
            );
        }
    }
}

/// Whether a search window is inside the configured day bound.
fn search_window_within_limit(limits: &Limits, window: Span) -> bool {
    window.minutes() <= i64::from(limits.max_search_window_days) * 24 * 60
}

/// The widest candidate envelope a valid search can produce: its window plus the
/// booking and the travel on either side. An over-limit search is judged in
/// envelopes of this size too, rather than in one unbounded span or one per
/// candidate. The one-minute floor keeps a zero configured window bound total.
fn search_envelope_limit(limits: &Limits) -> i64 {
    (i64::from(limits.max_search_window_days) * 24 * 60)
        .saturating_add(i64::from(limits.max_duration_minutes).saturating_mul(3))
        .max(1)
}

/// Reports `code` at `path` when `duration` is past `maximum`.
fn check_duration(
    diagnostics: &mut Diagnostics,
    maximum: i64,
    duration: Duration,
    code: &'static str,
    what: &str,
    path: String,
) {
    let minutes = duration.minutes();
    if minutes > maximum {
        diagnostics.errors.push(
            Diagnostic::new(
                code,
                format!(
                    "{what} lasts {minutes} minutes, above the configured maximum of {maximum}"
                ),
            )
            .with_path(path),
        );
    }
}

/// The configured limits that bound the work a search can demand: the length of
/// the search window and the number of candidate starts it produces.
///
/// `candidates` is the request's candidate starts, already bounded by the
/// configured maximum plus one, so a request past it is reported without
/// materialising every start it would produce.
fn check_search_limits(
    engine: &Engine,
    request: &Request,
    candidates: &[Instant],
    diagnostics: &mut Diagnostics,
) {
    let limits = engine.limits();

    let window_minutes = request.goal.search_window.minutes();
    if !search_window_within_limit(limits, request.goal.search_window) {
        diagnostics.errors.push(
            Diagnostic::new(
                "search_window_limit",
                format!(
                    "the search window is {window_minutes} minutes, above the configured maximum \
                     of {} days",
                    limits.max_search_window_days
                ),
            )
            .with_path("/goal/search_window".to_owned()),
        );
    }

    let exact = request.goal.candidates.is_some();
    let count = request
        .goal
        .candidates
        .as_ref()
        .map_or(candidates.len(), Vec::len);
    if count > limits.max_candidate_starts {
        let field = if exact {
            "/goal/candidates"
        } else {
            "/goal/grid"
        };
        diagnostics.errors.push(
            Diagnostic::new(
                "candidates_limit",
                format!(
                    "the search produces {} candidate starts, above the configured maximum of {}",
                    if exact {
                        count.to_string()
                    } else {
                        format!("more than {}", limits.max_candidate_starts)
                    },
                    limits.max_candidate_starts
                ),
            )
            .with_path(field.to_owned()),
        );
    }
}

/// The requirement's own checks: known participants, assignable hosts, terms
/// that could be filled on their own, and terms that can be filled jointly.
fn check_requirement(
    request: &Request,
    participant_ids: &BTreeSet<&str>,
    diagnostics: &mut Diagnostics,
) {
    // First-seen lookup preserves `.iter().find(...)` semantics even if a
    // malformed request carried duplicate participant IDs.
    let mut by_id: BTreeMap<&str, &Participant> = BTreeMap::new();
    for participant in &request.participants {
        by_id.entry(participant.id.as_str()).or_insert(participant);
    }
    for (part_index, part) in request.goal.parts.iter().enumerate() {
        let terms_path = format!("/goal/parts/{part_index}/requirement/all");
        // A part with no host slot cannot be booked at all: there is nobody to
        // assign, so the search has nothing to answer with.
        if part.requirement.slots() == 0 {
            diagnostics.errors.push(
                Diagnostic::new(
                    "requirement_unfillable",
                    format!(
                        "part `{}` asks for no host, so no host can be assigned to it",
                        part.id
                    ),
                )
                .with_path(terms_path),
            );
        }
        // Whether any term is short of candidates on its own. When one is, its
        // own error already says so and the joint check would only repeat it.
        let mut short = false;
        for (index, term) in part.requirement.terms.iter().enumerate() {
            short |= check_term(
                request,
                &by_id,
                participant_ids,
                part,
                part_index,
                index,
                term,
                diagnostics,
            );
        }
        if !short {
            check_joint_fillability(request, part_index, part, diagnostics);
        }
        for id in part.travel.by_participant.keys() {
            if !participant_ids.contains(id.as_str()) {
                diagnostics.errors.push(
                    Diagnostic::new(
                        "participant_unknown",
                        format!(
                            "part `{}` travel override names participant `{id}`, which the request \
                             does not carry",
                            part.id
                        ),
                    )
                    .with_path(format!(
                        "/goal/parts/{part_index}/travel/by_participant/{}",
                        escape_pointer_token(id)
                    )),
                );
            }
        }
    }
}

/// Checks one requirement term's participants and count, and answers whether it
/// is short on its own: a term naming an absent or non-host participant, or one
/// asking for more hosts than any participant matches.
#[allow(clippy::too_many_arguments)]
fn check_term(
    request: &Request,
    by_id: &BTreeMap<&str, &Participant>,
    participant_ids: &BTreeSet<&str>,
    part: &Part,
    part_index: usize,
    index: usize,
    term: &Term,
    diagnostics: &mut Diagnostics,
) -> bool {
    let term_path = format!("/goal/parts/{part_index}/requirement/all/{index}");
    match term {
        Term::Person { id } => match by_id.get(id.as_str()).copied() {
            None => {
                diagnostics.errors.push(
                    Diagnostic::new(
                        "participant_unknown",
                        format!(
                            "part `{}` term {index} names participant `{id}`, which the request \
                             does not carry",
                            part.id
                        ),
                    )
                    .with_path(term_path),
                );
                true
            }
            Some(participant) if participant.role != Role::Host => {
                diagnostics.errors.push(
                    Diagnostic::new(
                        "requirement_unfillable",
                        format!(
                            "part `{}` term {index} names `{id}`, who is an attendee and so cannot \
                             be assigned",
                            part.id
                        ),
                    )
                    .with_path(term_path),
                );
                true
            }
            Some(_) => false,
        },
        Term::AtLeast { count, of } => {
            for id in named_ids(of) {
                if !participant_ids.contains(id) {
                    diagnostics.errors.push(
                        Diagnostic::new(
                            "participant_unknown",
                            format!(
                                "part `{}` term {index} names participant `{id}`, which the \
                                 request does not carry",
                                part.id
                            ),
                        )
                        .with_path(term_path.clone()),
                    );
                }
            }
            // A term is filled by hosts: an attendee matching the selector is
            // never assigned, so counting one would hide an unfillable
            // requirement.
            let hosts = request
                .participants
                .iter()
                .filter(|participant| participant.role == Role::Host)
                .filter(|participant| of.matches_participant(participant.context()))
                .count();
            if hosts < *count as usize {
                diagnostics.errors.push(
                    Diagnostic::new(
                        "requirement_unfillable",
                        format!(
                            "part `{}` term {index} needs {count} hosts, but only {hosts} \
                             participant(s) match it",
                            part.id
                        ),
                    )
                    .with_path(term_path),
                );
                true
            } else {
                false
            }
        }
    }
}

/// Checks that distinct hosts can fill every term together, not just each term on
/// its own: overlapping pools can satisfy every count and still be unable to
/// supply enough distinct people across the whole requirement.
fn check_joint_fillability(
    request: &Request,
    part_index: usize,
    part: &Part,
    diagnostics: &mut Diagnostics,
) {
    let candidates: Vec<Vec<matching::Candidate<'_>>> = part
        .requirement
        .terms
        .iter()
        .map(|term| {
            request
                .participants
                .iter()
                .filter(|participant| term.filled_by(participant))
                .map(|participant| matching::Candidate {
                    id: &participant.id,
                    weight: 0,
                })
                .collect()
        })
        .collect();
    let terms: Vec<matching::Term<'_>> = part
        .requirement
        .terms
        .iter()
        .zip(&candidates)
        .map(|(term, candidates)| matching::Term {
            count: term.slots(),
            candidates: candidates.as_slice(),
        })
        .collect();
    if !matching::jointly_fillable(&terms) {
        diagnostics.errors.push(
            Diagnostic::new(
                "requirement_unfillable",
                format!(
                    "part `{}` cannot be filled: its terms cannot jointly be given distinct hosts, \
                     even if every participant were free",
                    part.id
                ),
            )
            .with_path(format!("/goal/parts/{part_index}/requirement/all")),
        );
    }
}

/// Warns about selectors and `when` clauses that can never match.
fn check_rule_reachability(request: &Request, part_tags: &[Tag], diagnostics: &mut Diagnostics) {
    let mut reported = BTreeSet::new();
    let mut present = BTreeSet::new();
    for (position, rule) in request.rules.iter().enumerate() {
        if !rule.when.matches_item(part_tags) {
            diagnostics.warnings.push(
                Diagnostic::new(
                    "when_matches_no_part",
                    format!(
                        "rule `{}` can never apply: its `when` matches no part of this request",
                        rule.id
                    ),
                )
                .with_path(format!("/rules/{position}/when")),
            );
        }
        // A cap applies only when its `when` and its `count` both match the
        // booking, so a `count` matching no part leaves a cap that selects this
        // booking counting nothing at all.
        if let RuleKind::Cap(cap) = &rule.kind
            && rule.when.matches_item(part_tags)
            && !cap.count.matches_item(part_tags)
        {
            diagnostics.warnings.push(
                Diagnostic::new(
                    "count_matches_no_part",
                    format!(
                        "cap `{}` selects this booking but its `count` matches no part of it, so \
                         it constrains nothing: check `params/count`",
                        rule.id
                    ),
                )
                .with_path(format!("/rules/{position}/params/count")),
            );
        }
        warn_about_absent_tags(
            named_tags(&rule.who)
                .into_iter()
                .map(|tag| (tag, format!("/rules/{position}/who"))),
            request,
            &mut reported,
            &mut present,
            diagnostics,
        );
    }
    // A requirement's selector names participant tags too, and it does so
    // whether or not the request carries a rule: a `not` over a mistyped tag
    // matches every host without saying so.
    let requirement_tags: Vec<(&Tag, String)> = request
        .goal
        .parts
        .iter()
        .enumerate()
        .flat_map(|(part_index, part)| {
            part.requirement
                .terms
                .iter()
                .enumerate()
                .filter_map(move |(term_index, term)| match term {
                    Term::AtLeast { of, .. } => Some((
                        of,
                        format!("/goal/parts/{part_index}/requirement/all/{term_index}"),
                    )),
                    Term::Person { .. } => None,
                })
        })
        .flat_map(|(selector, path)| {
            named_tags(selector)
                .into_iter()
                .map(move |tag| (tag, path.clone()))
        })
        .collect();
    warn_about_absent_tags(
        requirement_tags,
        request,
        &mut reported,
        &mut present,
        diagnostics,
    );
}

/// Warns about a status policy that ignores a resource participant's blocks.
///
/// An entry that ignores a status is usually a statement about people — an
/// unconfirmed invite does not take a human's time — but the default `who` is every
/// host, so the same entry also drops a conference room's hold and a second booking
/// can be offered over it. A policy scoped to resources on purpose is reported too,
/// because the guard reads the rule and not the caller's intent, and it reads
/// neither the entry's status nor whether the rule also reaches a person: the
/// message therefore states what the policy does to a room and offers the narrower
/// `who` conditionally rather than claiming a reach it never checked.
///
/// Only a rule that can apply to this booking and selects a resource participant
/// the request actually carries is reported, so a request naming only people is
/// never warned about and needs no `kind` clause to stay quiet.
fn warn_about_ignored_resource_holds(
    request: &Request,
    part_tags: &[Tag],
    diagnostics: &mut Diagnostics,
) {
    for (position, rule) in request.rules.iter().enumerate() {
        let RuleKind::StatusPolicy(policy) = &rule.kind else {
            continue;
        };
        if !rule.when.matches_item(part_tags)
            || !policy
                .entries
                .iter()
                .any(|entry| entry.effect == crate::model::StatusEffect::Ignore)
        {
            continue;
        }
        let mut selected = request
            .participants
            .iter()
            .filter(|participant| participant.kind == ParticipantKind::Resource)
            .filter(|participant| rule.who.matches_participant(participant.context()))
            .map(|participant| participant.id.as_str());
        let Some(first) = selected.next() else {
            continue;
        };
        let others = match selected.count() {
            0 => String::new(),
            count => format!(" and {count} more"),
        };
        diagnostics.warnings.push(
            Diagnostic::new(
                "status_policy_ignores_resource",
                format!(
                    "status policy `{}` ignores blocks for resource participant `{first}`{others}, \
                     so a block it matches on that room is read as free rather than as taken and a \
                     second booking can be offered over it: if the policy is meant for people, \
                     scope `who` to `{{ \"kind\": \"person\" }}`",
                    rule.id
                ),
            )
            .with_path(format!("/rules/{position}")),
        );
    }
}

/// Warns about blocks with no event key that a pooled cap counts.
///
/// A keyless block counts once per calendar it appears on, so the same event
/// copied onto two calendars without a shared key counts twice toward a pooled
/// cap. One warning per participant and cap keeps a large calendar from burying
/// the rest of the diagnostics.
///
/// The work is bounded by what the warning can be about rather than by the size of
/// the request: a request with no pooled cap that counts this booking is answered
/// before any calendar is read, and a participant's keyless blocks are collected
/// once, the first time a pooled cap selects them, so a participant no pooled cap
/// names is never scanned and nothing is retained for them.
fn warn_about_keyless_blocks(request: &Request, part_tags: &[Tag], diagnostics: &mut Diagnostics) {
    let pooled: Vec<(&Rule, &Cap)> = request
        .rules
        .iter()
        .filter_map(|rule| match &rule.kind {
            RuleKind::Cap(cap)
                if cap.per == CapPer::Pooled
                    && rule.when.matches_item(part_tags)
                    && cap.count.matches_item(part_tags) =>
            {
                Some((rule, cap))
            }
            _ => None,
        })
        .collect();
    if pooled.is_empty() {
        return;
    }
    let mut keyless: Vec<Option<Vec<&Block>>> = vec![None; request.participants.len()];
    for (rule, cap) in pooled {
        for (index, participant) in request.participants.iter().enumerate() {
            if !rule.who.matches_participant(participant.context()) {
                continue;
            }
            let blocks = keyless[index].get_or_insert_with(|| {
                participant
                    .blocks()
                    .iter()
                    // An all-day block never counts toward a cap, so it cannot
                    // be counted one by one however it is keyed.
                    .filter(|block| !block.all_day && block.event_key.is_none())
                    .collect()
            });
            let missing = blocks
                .iter()
                .filter(|block| cap.count.matches_item(&block.tags))
                .count();
            if missing == 0 {
                continue;
            }
            diagnostics.warnings.push(
                Diagnostic::new(
                    "block_missing_event_key",
                    format!(
                        "participant `{}` holds {missing} block(s) with no event key that pooled cap \
                         `{}` counts one by one; give them a key or an occurrence, or a copy on \
                         another calendar counts twice",
                        participant.id, rule.id
                    ),
                )
                .with_path(format!("/participants/{index}/calendar")),
            );
        }
    }
}

/// Warns once per tag that no participant carries, which makes every selector
/// naming it match nobody — or, under `not`, everybody.
fn warn_about_absent_tags<'a>(
    tags: impl IntoIterator<Item = (&'a Tag, String)>,
    request: &Request,
    reported: &mut BTreeSet<&'a str>,
    present: &mut BTreeSet<&'a str>,
    diagnostics: &mut Diagnostics,
) {
    for (tag, path) in tags {
        let name = tag.as_str();
        // Check cached presence/absence before scanning participants so repeated
        // tags across rules and terms scan the participant list at most once.
        if reported.contains(name) || present.contains(name) {
            continue;
        }
        let carried = request.participants.iter().any(|participant| {
            participant
                .tags
                .iter()
                .any(|item| tag.is_ancestor_or_self_of(item))
        });
        if carried {
            present.insert(name);
        } else {
            reported.insert(name);
            diagnostics.warnings.push(
                Diagnostic::new(
                    "selector_matches_no_participant",
                    format!(
                        "no participant carries `{tag}`, so a selector naming it matches nobody"
                    ),
                )
                .with_path(path),
            );
        }
    }
}

/// Candidate occupied spans for `participant` (or the part's default travel
/// when `participant` is `None`).
fn candidate_occupied_spans(
    part: &Part,
    participant: Option<&str>,
    candidates: &[Instant],
) -> Vec<MinuteSpan> {
    let (before, after) = match participant {
        Some(id) => part.travel.for_participant(id),
        None => (part.travel.before, part.travel.after),
    };
    candidates
        .iter()
        .filter_map(|&start| {
            if occupancy::overflow(start, part.duration, before, after).is_some() {
                return None;
            }
            let span = Span::new(
                Instant::from_minutes(start.minutes() - before.minutes()).ok()?,
                Instant::from_minutes(start.minutes() + part.duration.minutes() + after.minutes())
                    .ok()?,
            )
            .ok()?;
            MinuteSpan::new(span.start().minutes(), span.end().minutes()).ok()
        })
        .collect()
}

/// Pushes one `no_candidate_in_window` warning for `participant`.
fn push_no_candidate_in_window(participant: &str, warnings: &mut Vec<Diagnostic>) {
    warnings.push(
        Diagnostic::new(
            "no_candidate_in_window",
            format!(
                "no candidate's booking and travel fits inside every window that applies to \
                 `{participant}`"
            ),
        )
        .with_path("/goal/search_window".to_owned()),
    );
}

/// Over-cap search window warning check for explicit `goal.candidates`.
///
/// Groups distant candidates into bounded envelopes of width `<= envelope_limit`
/// and refuses to build windows when total grouped work exceeds `envelope_limit`
/// (preventing a 125-year candidate list from bypassing `max_search_window_days`).
#[allow(clippy::too_many_arguments)]
fn warn_if_no_candidate_fits_window_over_cap<'a>(
    db: &TimeZoneDatabase,
    event: &crate::time::Zone,
    schedule: &ZoneSchedule,
    home_zone_name: &'a str,
    participant: &str,
    windows: &[(&'a str, usize, &'a crate::time::WindowSpec)],
    spans: &[MinuteSpan],
    envelope_limit: i64,
    memos: &mut BTreeMap<MinuteSpan, FixedMinutes<'a>>,
    diagnostics: &mut Diagnostics,
) {
    if windows.is_empty() || spans.is_empty() {
        return;
    }
    let mut groups: Vec<(usize, usize, MinuteSpan)> = Vec::new();
    let mut total_work = 0_i64;
    let mut from = 0;
    while from < spans.len() {
        let mut until = from + 1;
        while until < spans.len()
            && spans[until].end().saturating_sub(spans[from].start()) <= envelope_limit
        {
            until += 1;
        }
        let Ok(occupied) = MinuteSpan::new(spans[from].start(), spans[until - 1].end()) else {
            from = until;
            continue;
        };
        let group_work = occupied.end().saturating_sub(occupied.start()).max(1_440);
        total_work = total_work.saturating_add(group_work);
        if total_work > envelope_limit {
            return;
        }
        groups.push((from, until, occupied));
        from = until;
    }
    let mut inside = false;
    for (from, until, occupied) in groups {
        let group = &spans[from..until];
        let memo = memos.entry(occupied).or_default();
        let ctx = WindowContext {
            db,
            schedule,
            home_zone_name,
            event,
            judged: occupied,
        };
        let mut allowed: Option<Arc<IntervalSet>> = None;
        let mut failed_build = false;
        for &(_, position, spec) in windows {
            let Ok(minutes) = memo.window(position, spec, &ctx) else {
                failed_build = true;
                break;
            };
            allowed = Some(match allowed {
                Some(current) => Arc::new(current.intersection(&minutes)),
                None => minutes,
            });
            if allowed.as_ref().is_some_and(|set| set.is_empty()) {
                break;
            }
        }
        if !failed_build
            && allowed.is_some_and(|allowed| group.iter().any(|span| allowed.contains_span(span)))
        {
            inside = true;
            break;
        }
    }
    if !inside {
        push_no_candidate_in_window(participant, &mut diagnostics.warnings);
    }
}

/// The range a candidate start occupies for `participant`: the booking plus the
/// travel they need, or `None` when the model cannot name one of its endpoints
/// and no offer could report it.
fn occupied_span(part: &Part, participant: &str, start: Instant) -> Option<Span> {
    let (before, after) = part.travel.for_participant(participant);
    if occupancy::overflow(start, part.duration, before, after).is_some() {
        return None;
    }
    Span::new(
        Instant::from_minutes(start.minutes() - before.minutes()).ok()?,
        Instant::from_minutes(start.minutes() + part.duration.minutes() + after.minutes()).ok()?,
    )
    .ok()
}

/// Warns when the `lead_time` maximum ends before the earliest start the minimum
/// notice allows, which is a request no start can satisfy.
///
/// `horizon_end` is the latest instant this pipeline will judge, passed to a
/// `working_days` maximum so its calendar walk stops at the horizon; it is `None`
/// for a confirmation, whose start is not known yet.
#[allow(clippy::too_many_arguments)]
fn warn_if_horizon_ends_first(
    calendars: &BTreeMap<String, WorkingCalendar>,
    request: &Request,
    applicable: &crate::rules::Applicable<'_>,
    participant: &str,
    event: &crate::time::Zone,
    horizon_end: Option<Instant>,
    bounds: &mut BTreeMap<usize, StartBound>,
    diagnostics: &mut Diagnostics,
) {
    if applicable.lead_max.is_empty() {
        return;
    }
    let travel_before = request.goal.parts.first().map_or(0, |part| {
        part.travel.for_participant(participant).0.minutes()
    });
    // The earliest start a booking can take. Past the last supported instant no
    // start exists at all, so the comparison moves to that instant rather than
    // being dropped: a finite maximum that ends earlier still explains why the
    // search cannot return an offer.
    let earliest = Instant::from_minutes(
        request
            .now
            .minutes()
            .saturating_add(applicable.lead_min.minutes())
            .saturating_add(travel_before)
            .min(Instant::MAX.minutes()),
    )
    .expect("a start no earlier than `now` and no later than `Instant::MAX` is supported");
    for lead in &applicable.lead_max {
        let bound = if let Some(bound) = bounds.get(&lead.position) {
            bound.clone()
        } else {
            let Ok(bound) =
                resolve_lead_bound(request.now, calendars, lead.max, event, horizon_end)
            else {
                continue;
            };
            bounds.insert(lead.position, bound.clone());
            bound
        };
        if !bound.allows(earliest) {
            diagnostics.warnings.push(
                Diagnostic::new(
                    "horizon_before_earliest_start",
                    format!(
                        "the `lead_time` maximum ends before `{earliest}`, the earliest start \
                         `{participant}`'s notice allows"
                    ),
                )
                .with_path(format!("/rules/{}/params/max", lead.position)),
            );
            return;
        }
    }
}

/// Checks a proposal's shape against the request it is checked with, and answers
/// with the event for the goal's part.
///
/// A proposal that does not fit the request is an error rather than an invalid
/// slot: the caller asked about a booking the request does not describe, which is
/// a mistake to report rather than an answer to give.
///
/// # Errors
/// Returns [`Error::MalformedProposal`] naming the shape failure: a part the goal
/// does not carry or lists twice, a part with no event, a term count or host count
/// that does not match the requirement, a host the request does not carry, an
/// attendee assigned a booking, a host listed twice, a host that does not fill
/// the term it is listed under, or an occupied range — the booking plus the travel
/// of every participant it needs — the model cannot represent as a span.
pub(crate) fn check_proposal<'a>(
    request: &Request,
    proposal: &'a Proposal,
) -> Result<&'a ProposalEvent, Error> {
    // A set rather than a list: a proposal may name many events, and a duplicate
    // is found in `log(events)` instead of by scanning every event already seen.
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    for (index, event) in proposal.events.iter().enumerate() {
        if !request.goal.parts.iter().any(|part| part.id == event.part) {
            return Err(Error::malformed_proposal(
                "proposal_part_unknown",
                format!("the goal has no part `{}`", event.part),
                format!("/events/{index}/part"),
            ));
        }
        if !seen.insert(event.part.as_str()) {
            return Err(Error::malformed_proposal(
                "proposal_part_duplicated",
                format!("part `{}` is proposed more than once", event.part),
                format!("/events/{index}/part"),
            ));
        }
    }

    let mut only: Option<&'a ProposalEvent> = None;
    for part in &request.goal.parts {
        let Some((index, event)) = proposal
            .events
            .iter()
            .enumerate()
            .find(|(_, event)| event.part == part.id)
        else {
            return Err(Error::malformed_proposal(
                "proposal_part_missing",
                format!("the proposal has no event for part `{}`", part.id),
                "/events".to_owned(),
            ));
        };
        check_assignment(request, part, index, event)?;
        check_occupied_range(request, part, index, event)?;
        only = Some(event);
    }
    only.ok_or_else(|| {
        Error::malformed_proposal(
            "proposal_part_missing",
            "the request has no part to propose".to_owned(),
            "/events".to_owned(),
        )
    })
}

/// Checks one proposed event's terms against the part's requirement.
fn check_assignment(
    request: &Request,
    part: &crate::model::Part,
    event_index: usize,
    event: &ProposalEvent,
) -> Result<(), Error> {
    let terms = &part.requirement.terms;
    let assignment_path = format!("/events/{event_index}/assignment");
    if event.assignment.len() != terms.len() {
        return Err(Error::malformed_proposal(
            "proposal_term_count",
            format!(
                "part `{}` has {} term(s), but the proposal lists {}",
                part.id,
                terms.len(),
                event.assignment.len()
            ),
            assignment_path,
        ));
    }
    // A set rather than a list: a requirement may name many hosts, and a repeated
    // assignment is found in `log(hosts)` instead of by scanning every host already
    // assigned.
    let mut assigned: BTreeSet<&str> = BTreeSet::new();
    for (index, (term, hosts)) in terms.iter().zip(&event.assignment).enumerate() {
        let term_path = format!("{assignment_path}/{index}");
        let expected = match term {
            Term::Person { .. } => 1,
            Term::AtLeast { count, .. } => *count as usize,
        };
        if hosts.len() != expected {
            return Err(Error::malformed_proposal(
                "proposal_host_count",
                format!(
                    "part `{}` term {index} needs {expected} host(s), but the proposal lists {}",
                    part.id,
                    hosts.len()
                ),
                term_path,
            ));
        }
        for (slot, id) in hosts.iter().enumerate() {
            let host_path = format!("{term_path}/{slot}");
            let malformed =
                |code, message| Error::malformed_proposal(code, message, host_path.clone());
            let Some(participant) = request.participants.iter().find(|item| item.id == *id) else {
                return Err(malformed(
                    "proposal_host_unknown",
                    format!("the request carries no participant `{id}`"),
                ));
            };
            if participant.role != Role::Host {
                return Err(malformed(
                    "proposal_host_attendee",
                    format!("participant `{id}` is an attendee, who is never assigned a booking"),
                ));
            }
            if !term.filled_by(participant) {
                return Err(malformed(
                    "proposal_host_mismatch",
                    format!(
                        "participant `{id}` does not fill term {index} of part `{}`",
                        part.id
                    ),
                ));
            }
            if !assigned.insert(id.as_str()) {
                return Err(malformed(
                    "proposal_host_duplicated",
                    format!("participant `{id}` is assigned more than once"),
                ));
            }
        }
    }
    Ok(())
}

/// Checks that the event's occupied range — the booking plus the travel of every
/// participant it needs — is representable as a [`crate::model::Span`] at both
/// edges of the supported range.
///
/// The participants are the assigned hosts and every attendee, because the travel
/// to hold belongs to all of them. They are judged in ascending ID order, so a
/// proposal that crosses an edge for more than one participant reports the same
/// failure whoever wrote the assignment.
///
/// # Errors
/// Returns [`Error::MalformedProposal`] with code `proposal_out_of_range` at
/// `/events/{index}/start` when the booking or a participant's required travel
/// reaches past either edge of the supported range.
fn check_occupied_range(
    request: &Request,
    part: &crate::model::Part,
    event_index: usize,
    event: &ProposalEvent,
) -> Result<(), Error> {
    let path = format!("/events/{event_index}/start");
    let out_of_range =
        |message: String| Error::malformed_proposal("proposal_out_of_range", message, path.clone());

    // The booking's own span first: when it runs past the range on its own, no
    // participant's travel is to blame.
    if occupancy::overflow(event.start, part.duration, Duration::ZERO, Duration::ZERO).is_some() {
        return Err(out_of_range(format!(
            "the booking at {} lasts {}, which reaches past the last supported instant {}",
            event.start,
            part.duration,
            Instant::MAX
        )));
    }

    let mut needed: Vec<&str> = event
        .assignment
        .iter()
        .flatten()
        .map(String::as_str)
        .collect();
    needed.extend(
        request
            .participants
            .iter()
            .filter(|participant| participant.role == Role::Attendee)
            .map(|participant| participant.id.as_str()),
    );
    needed.sort_unstable();
    needed.dedup();

    for id in needed {
        let (before, after) = part.travel.for_participant(id);
        let Some(edge) = occupancy::overflow(event.start, part.duration, before, after) else {
            continue;
        };
        let message = match edge {
            occupancy::Edge::Before => format!(
                "participant `{id}` needs {before} of travel before the booking at {}, which \
                 reaches before the first supported instant {}",
                event.start,
                Instant::MIN
            ),
            occupancy::Edge::After => format!(
                "participant `{id}` needs {after} of travel after the booking at {}, which \
                 reaches past the last supported instant {}",
                event.start,
                Instant::MAX
            ),
        };
        return Err(out_of_range(message));
    }
    Ok(())
}

/// Validation findings point at the exact request field they are about, so a
/// wire layer or an editor can open it. These pin the pointer for each shape of
/// finding — a list element, a nested field, and a key that needs RFC 6901
/// escaping — and pin the pointer a malformed proposal carries.
#[cfg(all(test, feature = "bundled-tzdb"))]
mod tests {
    use crate::Engine;
    use crate::error::{Diagnostics, Error};
    use crate::model::{
        Block, CalendarSpec, Cap, CapMeasure, CapPer, CapPeriod, Clearance, Explain, Goal,
        LeadTime, LeadTimeMax, Occurrence, Part, Participant, ParticipantTravel, Proposal, Request,
        Requirement, Rule, RuleKind, Span, StatusEffect, StatusPolicy, StatusPolicyEntry, Term,
        Travel, ZonePeriod,
    };
    use crate::tags::{Selector, Tag};
    use crate::time::{
        Date, Duration, Instant, RuleZone, TimeOfDay, WallEnd, WallRange, Weekday, Weekdays,
        WindowSpec, ZoneName,
    };

    fn engine() -> Engine {
        Engine::builder()
            .build()
            .expect("the bundled database builds")
    }

    fn zone() -> ZoneName {
        ZoneName::new("UTC").unwrap()
    }

    fn at(minutes: i64) -> Instant {
        Instant::from_minutes(Instant::MIN.minutes() + minutes).unwrap()
    }

    fn span(from: i64, until: i64) -> Span {
        Span::new(at(from), at(until)).unwrap()
    }

    fn tag(path: &str) -> Tag {
        Tag::new(path).unwrap()
    }

    /// A request that validates cleanly, so each test can break exactly one
    /// thing.
    fn base() -> Request {
        let mut participant = Participant::new("ana", zone());
        participant.calendar = Some(Vec::new());
        let part = Part::new(
            Duration::from_minutes(30),
            Requirement::new(vec![Term::person("ana")]),
        );
        let mut request = Request::new(at(0), Goal::new(zone(), span(0, 24 * 60), vec![part]));
        request.participants = vec![participant];
        request
    }

    /// The path of the first finding with `code`, whether it is an error or a
    /// warning.
    fn path(diagnostics: &Diagnostics, code: &str) -> Option<String> {
        diagnostics
            .errors
            .iter()
            .chain(&diagnostics.warnings)
            .find(|finding| finding.code() == code)
            .and_then(|finding| finding.path().map(str::to_owned))
    }

    #[test]
    fn a_participant_finding_points_at_the_participant_field() {
        let engine = engine();

        // A participant at an index other than the first.
        let mut request = base();
        request.participants.push(Participant::new(
            "bo",
            ZoneName::new("Mars/Olympus").unwrap(),
        ));
        assert_eq!(
            path(&engine.validate(&request), "zone_unknown").as_deref(),
            Some("/participants/1/zone")
        );

        // A duplicate ID is reported at the later entry.
        let mut request = base();
        request.participants.push(Participant::new("ana", zone()));
        assert_eq!(
            path(&engine.validate(&request), "participant_duplicate").as_deref(),
            Some("/participants/1/id")
        );

        // An occurrence that names nobody.
        let mut request = base();
        request.occurrences = vec![Occurrence {
            event_key: "evt".to_owned(),
            span: span(0, 60),
            tags: Vec::new(),
            participants: Vec::new(),
        }];
        assert_eq!(
            path(
                &engine.validate(&request),
                "occurrence_without_participants"
            )
            .as_deref(),
            Some("/occurrences/0/participants")
        );
    }

    #[test]
    fn a_goal_finding_points_at_the_goal_field() {
        let engine = engine();

        // The goal's own fields.
        let mut request = base();
        request.goal.zone = ZoneName::new("Mars/Olympus").unwrap();
        assert_eq!(
            path(&engine.validate(&request), "zone_unknown").as_deref(),
            Some("/goal/zone")
        );

        let mut request = base();
        request.goal.parts[0].duration = Duration::ZERO;
        assert_eq!(
            path(&engine.validate(&request), "part_duration_empty").as_deref(),
            Some("/goal/parts/0/duration")
        );

        let mut request = base();
        request.goal.parts.clear();
        assert_eq!(
            path(&engine.validate(&request), "part_count").as_deref(),
            Some("/goal/parts")
        );

        // A goal that names both a grid and candidate starts is reported at the
        // field that makes the pair impossible.
        let mut request = base();
        request.goal.grid = Some(
            crate::time::Grid::new(
                Duration::from_minutes(15),
                TimeOfDay::MIDNIGHT,
                crate::time::GridZone::Event,
            )
            .unwrap(),
        );
        request.goal.candidates = Some(vec![at(0)]);
        assert_eq!(
            path(&engine.validate(&request), "grid_and_candidates").as_deref(),
            Some("/goal/candidates")
        );

        // A requirement term and a travel override, at their indices.
        let mut request = base();
        request.goal.parts[0]
            .requirement
            .terms
            .push(Term::person("bo"));
        assert_eq!(
            path(&engine.validate(&request), "participant_unknown").as_deref(),
            Some("/goal/parts/0/requirement/all/1")
        );

        let mut request = base();
        request.goal.parts[0].travel = Travel {
            before: Duration::ZERO,
            after: Duration::ZERO,
            by_participant: [(
                "x/y".to_owned(),
                ParticipantTravel {
                    before: Duration::ZERO,
                    after: Duration::ZERO,
                },
            )]
            .into_iter()
            .collect(),
        };
        assert_eq!(
            path(&engine.validate(&request), "participant_unknown").as_deref(),
            Some("/goal/parts/0/travel/by_participant/x~1y")
        );
    }

    #[test]
    fn a_rule_finding_points_at_the_rule_and_field() {
        let engine = engine();

        // A rule's ID, zone, calendar and status-policy entry, at their index.
        let mut request = base();
        request.rules = vec![
            Rule::new("r", RuleKind::Clearance(Clearance::default())),
            Rule::new("r", RuleKind::Clearance(Clearance::default())),
        ];
        assert_eq!(
            path(&engine.validate(&request), "rule_id_duplicate").as_deref(),
            Some("/rules/1/id")
        );

        let mut request = base();
        request.rules = vec![
            Rule::new("good", RuleKind::Clearance(Clearance::default())),
            Rule::new(
                "zoned",
                RuleKind::Window(
                    WindowSpec::new(
                        RuleZone::Named(ZoneName::new("Mars/Olympus").unwrap()),
                        Vec::new(),
                        Vec::new(),
                    )
                    .unwrap(),
                ),
            ),
        ];
        assert_eq!(
            path(&engine.validate(&request), "zone_unknown").as_deref(),
            Some("/rules/1/params/zone")
        );

        let mut request = base();
        request.rules = vec![Rule::new(
            "horizon",
            RuleKind::LeadTime(LeadTime {
                min: Duration::ZERO,
                max: Some(LeadTimeMax::WorkingDays {
                    count: 3,
                    calendar: "org".to_owned(),
                }),
            }),
        )];
        assert_eq!(
            path(&engine.validate(&request), "calendar_unknown").as_deref(),
            Some("/rules/0/params/max/calendar")
        );

        let mut request = base();
        request.rules = vec![Rule::new(
            "policy",
            RuleKind::StatusPolicy(StatusPolicy {
                entries: vec![
                    StatusPolicyEntry::status(
                        crate::model::BlockStatus::Tentative,
                        StatusEffect::Ignore,
                    ),
                    StatusPolicyEntry {
                        status: None,
                        tags: None,
                        effect: StatusEffect::Ignore,
                    },
                ],
            }),
        )];
        assert_eq!(
            path(&engine.validate(&request), "status_policy_entry_empty").as_deref(),
            Some("/rules/0/params/entries/1")
        );

        // A status-policy entry's tag selector is the entry's `match`.
        let mut request = base();
        request.rules = vec![Rule::new(
            "policy",
            RuleKind::StatusPolicy(StatusPolicy {
                entries: vec![StatusPolicyEntry::tags(
                    Selector::ids(["ana"]),
                    StatusEffect::Block,
                )],
            }),
        )];
        assert_eq!(
            path(&engine.validate(&request), "selector_not_for_items").as_deref(),
            Some("/rules/0/params/entries/0/match")
        );
    }

    #[test]
    fn a_keyed_finding_escapes_the_pointer_token() {
        let engine = engine();

        // A calendar key and a weight key are opaque strings, so a `/` or `~` in
        // one is escaped in the pointer.
        let mut request = base();
        request.calendars.insert(
            "org/eu".to_owned(),
            CalendarSpec {
                zone: ZoneName::new("Mars/Olympus").unwrap(),
                working_days: Weekdays::weekdays(),
                holidays: Vec::new(),
            },
        );
        assert_eq!(
            path(&engine.validate(&request), "zone_unknown").as_deref(),
            Some("/calendars/org~1eu/zone")
        );

        let mut request = base();
        request.calendars.insert(
            "org~eu".to_owned(),
            CalendarSpec {
                zone: ZoneName::new("Mars/Olympus").unwrap(),
                working_days: Weekdays::weekdays(),
                holidays: Vec::new(),
            },
        );
        assert_eq!(
            path(&engine.validate(&request), "zone_unknown").as_deref(),
            Some("/calendars/org~0eu/zone")
        );

        let mut request = base();
        request.weights.insert("a/b".to_owned(), 1);
        assert_eq!(
            path(&engine.validate(&request), "participant_unknown").as_deref(),
            Some("/weights/a~1b")
        );
    }

    #[test]
    fn a_limit_finding_points_at_the_limited_field() {
        // The request-level limits.
        let mut request = base();
        request.explain = Some(Explain::new(1));
        let limited = Engine::builder()
            .limits(crate::Limits::default().with_max_explain_limit(0))
            .build()
            .unwrap();
        assert_eq!(
            path(&limited.validate(&request), "explain_limit").as_deref(),
            Some("/explain/limit")
        );

        let mut request = base();
        request.participants[0].calendar = Some(vec![Block::new(span(0, 30))]);
        let limited = Engine::builder()
            .limits(crate::Limits::default().with_max_blocks_per_participant(0))
            .build()
            .unwrap();
        assert_eq!(
            path(&limited.validate(&request), "blocks_limit").as_deref(),
            Some("/participants/0/calendar")
        );

        let request = base();
        let limited = Engine::builder()
            .limits(
                crate::Limits::default()
                    .with_max_search_window_days(0)
                    .with_max_candidate_starts(0),
            )
            .build()
            .unwrap();
        let diagnostics = limited.validate(&request);
        assert_eq!(
            path(&diagnostics, "search_window_limit").as_deref(),
            Some("/goal/search_window")
        );
        assert_eq!(
            path(&diagnostics, "candidates_limit").as_deref(),
            Some("/goal/grid"),
            "a grid that generates too many starts is reported at the grid"
        );

        // Caller-supplied candidates are reported at their own field.
        let mut request = base();
        request.goal.grid = None;
        request.goal.candidates = Some(vec![at(0)]);
        let diagnostics = limited.validate(&request);
        assert_eq!(
            path(&diagnostics, "candidates_limit").as_deref(),
            Some("/goal/candidates")
        );
    }

    #[test]
    fn a_warning_points_at_the_clause_it_is_about() {
        let engine = engine();

        let mut request = base();
        request.rules = vec![
            Rule::new("interview", RuleKind::Clearance(Clearance::default()))
                .with_when(Selector::tag(tag("interview"))),
        ];
        assert_eq!(
            path(&engine.validate(&request), "when_matches_no_part").as_deref(),
            Some("/rules/0/when")
        );

        let mut request = base();
        request.rules = vec![
            Rule::new("sales", RuleKind::Clearance(Clearance::default()))
                .with_who(Selector::tag(tag("team/sales"))),
        ];
        assert_eq!(
            path(
                &engine.validate(&request),
                "selector_matches_no_participant"
            )
            .as_deref(),
            Some("/rules/0/who")
        );

        let mut request = base();
        request.goal.parts[0].requirement.terms =
            vec![Term::at_least(1, Selector::tag(tag("pool/missing")))];
        assert_eq!(
            path(
                &engine.validate(&request),
                "selector_matches_no_participant"
            )
            .as_deref(),
            Some("/goal/parts/0/requirement/all/0")
        );
    }

    #[test]
    fn a_malformed_proposal_points_at_the_field_that_does_not_fit() {
        let engine = engine();
        let mut request = base();
        request
            .participants
            .push(Participant::attendee("bo", zone()));
        request.goal.parts[0].requirement.terms = vec![Term::person("ana")];

        let check = |proposal: &Proposal| {
            engine
                .check_slot(&request, proposal)
                .expect_err("the proposal does not fit")
        };

        let unknown_part = check(&Proposal::single("other", at(0), vec![Vec::new()]));
        assert_eq!(unknown_part.path(), Some("/events/0/part"));
        assert_eq!(unknown_part.code(), "proposal_part_unknown");

        let wrong_terms = check(&Proposal::single("main", at(0), Vec::new()));
        assert_eq!(wrong_terms.path(), Some("/events/0/assignment"));
        assert_eq!(wrong_terms.code(), "proposal_term_count");

        let wrong_hosts = check(&Proposal::single("main", at(0), vec![Vec::new()]));
        assert_eq!(wrong_hosts.path(), Some("/events/0/assignment/0"));
        assert_eq!(wrong_hosts.code(), "proposal_host_count");

        let unknown_host = check(&Proposal::single(
            "main",
            at(0),
            vec![vec!["ghost".to_owned()]],
        ));
        assert_eq!(unknown_host.path(), Some("/events/0/assignment/0/0"));
        assert_eq!(unknown_host.code(), "proposal_host_unknown");

        let attendee = check(&Proposal::single(
            "main",
            at(0),
            vec![vec!["bo".to_owned()]],
        ));
        assert_eq!(attendee.path(), Some("/events/0/assignment/0/0"));
        assert_eq!(attendee.code(), "proposal_host_attendee");

        // A proposal missing the request's only part.
        let missing = engine
            .check_slot(&request, &Proposal { events: Vec::new() })
            .expect_err("the proposal carries no event");
        assert_eq!(missing.path(), Some("/events"));
        assert_eq!(missing.code(), "proposal_part_missing");
    }

    #[test]
    fn a_cap_reason_path_points_at_the_cap_rule() {
        // A pooled cap in the participant zone is reported at the zone it named,
        // which is the field the rule cannot combine.
        let mut request = base();
        let cap = Cap::new(
            Selector::True,
            CapMeasure::Count,
            2,
            CapPeriod::Day,
            CapPer::Pooled,
            RuleZone::Participant,
            Weekday::Monday,
        );
        request.rules = vec![Rule::new("pooled", RuleKind::Cap(cap))];
        assert_eq!(
            path(&engine().validate(&request), "cap_pooled_participant_zone").as_deref(),
            Some("/rules/0/params/zone")
        );
    }

    /// A participant's zone periods are reported where they were written.
    #[test]
    fn an_overlapping_zone_period_points_at_the_periods_list() {
        let mut request = base();
        request.participants[0].zone_periods = vec![
            ZonePeriod::new(zone(), at(60), at(120)).unwrap(),
            ZonePeriod::new(zone(), at(90), at(180)).unwrap(),
        ];
        assert_eq!(
            path(&engine().validate(&request), "zone_periods_overlap").as_deref(),
            Some("/participants/0/zone_periods")
        );
    }

    /// The wall-clock types a request carries are exported error types too, and
    /// `Error` wraps them so `?` reads cleanly. Each keeps its own stable code.
    #[test]
    fn the_model_errors_convert_into_the_crate_error() {
        let window =
            WallRange::new(TimeOfDay::MIDNIGHT, WallEnd::Time(TimeOfDay::MIDNIGHT)).unwrap_err();
        assert_eq!(Error::from(window).code(), "window_range_empty");

        let tag = Tag::new("").unwrap_err();
        assert_eq!(Error::from(tag).code(), "tag_empty");

        let blackout = crate::time::BlackoutSpan::new(
            crate::time::LocalDateTime::new(Date::new(2027, 3, 10).unwrap(), TimeOfDay::MIDNIGHT),
            crate::time::LocalDateTime::new(Date::new(2027, 3, 10).unwrap(), TimeOfDay::MIDNIGHT),
        )
        .unwrap_err();
        assert_eq!(Error::from(blackout).code(), "blackout_span_empty");
    }
}
