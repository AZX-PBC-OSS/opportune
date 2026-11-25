//! The benchmark workloads carry the shape their measurement claims.
//!
//! A benchmark over an empty calendar flatters itself: nothing to read, nobody
//! blocked, no rule to apply. These tests run every workload through the real
//! engine and assert they match the target scale, that every rule kind
//! really decides something in them, that a search answers with offers rather
//! than an empty page, and that the bounded-limit and confirm-time workloads
//! measure what their names claim.

use std::collections::{BTreeMap, BTreeSet};

use opportune::Engine;
use opportune::model::{CapPer, CapPeriod, Request, Rule, RuleKind, Span};
use opportune::reasons::ReasonCode;
use opportune::time::{Date, Duration, GridZone, Instant, RuleZone, ZonePeriod, ZoneSchedule};
use opportune_testkit::workloads::{
    DENSE_EXTRA_BLOCKS_PER_DAY, GRID_STEP_MINUTES, PARTICIPANTS, REST_ZONE_PERIOD_SIZES,
    TRUNCATED_LIMIT, WINDOW_DAYS, Workload, limit_workloads, rest_zone_period_workload,
    slot_workload, spec_workloads,
};

fn engine() -> Engine {
    Engine::builder()
        .build()
        .expect("the bundled database builds")
}

fn rule<'a>(rules: &'a [Rule], id: &str) -> &'a Rule {
    rules
        .iter()
        .find(|rule| rule.id == id)
        .unwrap_or_else(|| panic!("the workloads carry a `{id}` rule"))
}

/// The reason codes the engine reports for `request`.
fn codes(engine: &Engine, request: &Request) -> BTreeSet<ReasonCode> {
    let response = engine
        .find_availability(request)
        .expect("a workload request is valid");
    response
        .excluded
        .iter()
        .flat_map(|excluded| excluded.reasons.iter().map(|reason| reason.code))
        .collect()
}

/// `request` read one day of its window at a time, which keeps each answer's
/// explanation inside the reason budget the whole window would exceed.
fn per_day(engine: &Engine, workload: &Workload) -> BTreeSet<ReasonCode> {
    let window = workload.request.goal.search_window;
    let mut reported = BTreeSet::new();
    let mut from = window.start();
    while from < window.end() {
        let until = Instant::from_minutes((from.minutes() + 24 * 60).min(window.end().minutes()))
            .expect("a day of the window is inside the supported instants");
        let mut request = workload.request.clone();
        request.goal.search_window = Span::new(from, until).expect("a day is non-empty");
        reported.extend(codes(engine, &request));
        from = until;
    }
    reported
}

#[test]
fn both_workloads_are_the_specification_scale() {
    let engine = engine();
    let workloads = spec_workloads(&engine);
    assert_eq!(workloads.len(), 2);
    let ids: BTreeSet<&str> = workloads.iter().map(|workload| workload.id).collect();
    assert_eq!(ids, BTreeSet::from(["overlapping_groups", "single_term"]));

    for workload in &workloads {
        let request = &workload.request;
        let db = engine.time_zone_database();

        // Fifty participants, and not one of them without a calendar: a
        // participant with no calendar is free without being read at all.
        assert_eq!(request.participants.len(), PARTICIPANTS);
        for participant in &request.participants {
            let calendar = participant
                .calendar
                .as_ref()
                .unwrap_or_else(|| panic!("{} has a calendar", participant.id));
            assert!(
                !calendar.is_empty(),
                "{} has meetings to read",
                participant.id
            );
        }

        // Fourteen days of five-minute starts, read in the event's clock.
        let goal = &request.goal;
        let window = goal.search_window;
        assert_eq!(window.minutes(), WINDOW_DAYS * 24 * 60);
        let grid = goal.grid.as_ref().expect("the goal has a grid");
        assert_eq!(grid.step(), Duration::from_minutes(GRID_STEP_MINUTES));
        assert_eq!(grid.zone_choice(), &GridZone::Event);

        // One part, tagged so the caps and the `when` of every rule measure it.
        assert_eq!(goal.parts.len(), 1);
        assert_eq!(goal.parts[0].tags.len(), 1);

        // No static complaint: the request is one the engine accepts.
        let diagnostics = engine.validate(request);
        assert!(
            diagnostics.errors.is_empty(),
            "{} is a valid request: {:?}",
            workload.id,
            diagnostics.errors
        );

        let response = engine
            .find_availability(request)
            .expect("a workload request is valid");
        assert!(
            response.offers.len() > 100,
            "{} offers a working page, not an empty one",
            workload.id
        );
        assert!(!response.dates.is_empty(), "{} reports dates", workload.id);
        assert_eq!(response.meta.tzdb, db.version());

        // Every offer is a thirty-minute event on the grid inside the window.
        for offer in &response.offers {
            let event = &offer.events[0];
            assert!(window.contains(event.start));
            assert_eq!(event.end.minutes() - event.start.minutes(), 30);
            assert_eq!(event.start.minutes() % i64::from(GRID_STEP_MINUTES), 0);
        }
    }
}

#[test]
fn every_rule_kind_decides_something_in_the_workloads() {
    let engine = engine();
    for workload in spec_workloads(&engine) {
        let rules = &workload.request.rules;
        let kinds: BTreeSet<&str> = rules
            .iter()
            .map(|rule| match rule.kind {
                RuleKind::Window(_) => "window",
                RuleKind::Blackout(_) => "blackout",
                RuleKind::Clearance(_) => "clearance",
                RuleKind::LeadTime(_) => "lead_time",
                RuleKind::StatusPolicy(_) => "status_policy",
                RuleKind::Rest(_) => "rest",
                RuleKind::Cap(_) => "cap",
                // `RuleKind` is `#[non_exhaustive]`: a kind added after 0.1 is not
                // one of the seven this workload test asserts.
                _ => unreachable!("a rule kind added after 0.1 has no workload yet"),
            })
            .collect();
        assert_eq!(
            kinds,
            BTreeSet::from([
                "window",
                "blackout",
                "clearance",
                "lead_time",
                "status_policy",
                "rest",
                "cap",
            ]),
            "{} carries all seven rule kinds",
            workload.id
        );
        // The two caps are the two shapes: a daily limit on each participant and
        // a daily limit on the pool, bucketed in the event's clock.
        let daily = rule(rules, "cap_per_day")
            .cap()
            .expect("`cap_per_day` is a cap rule");
        assert_eq!(daily.per, CapPer::Each);
        assert_eq!(daily.period, CapPeriod::Day);
        let pooled = rule(rules, "pool_day")
            .cap()
            .expect("`pool_day` is a cap rule");
        assert_eq!(pooled.per, CapPer::Pooled);
        assert_eq!(pooled.period, CapPeriod::Day);
        assert_eq!(pooled.zone, RuleZone::Event);

        // Every participant-level rule kind and every start-level one decides at
        // least one start: notice, the grid's dead hours, the holiday, existing
        // meetings, their journeys, clearance, a late finish, and a full cap.
        let codes = per_day(&engine, &workload);
        for code in [
            ReasonCode::LeadTimeMin,
            ReasonCode::OutsideWindow,
            ReasonCode::Blackout,
            ReasonCode::Blocked,
            ReasonCode::Travel,
            ReasonCode::Clearance,
            ReasonCode::Rest,
            ReasonCode::CapReached,
        ] {
            assert!(
                codes.contains(&code),
                "{} exercises `{code:?}`: {codes:?}",
                workload.id
            );
        }
    }
}

/// The offer limit drops offers and nothing else: dates, exclusions and the
/// truncation flag are computed as if every fillable start had been offered.
/// Past the limit the search answers fillability without the concrete
/// assignment, and only a limited search over a multi-term requirement — the
/// overlapping groups — exercises that path at all.
#[test]
fn the_offer_limit_drops_offers_and_nothing_else() {
    let engine = engine();
    for workload in spec_workloads(&engine) {
        let request = &workload.request;
        let full = engine
            .find_availability(request)
            .expect("a workload request is valid");
        for limit in [0_usize, 1, 7] {
            let mut cut = request.clone();
            cut.limit = Some(limit);
            let cut = engine
                .find_availability(&cut)
                .expect("a workload request is valid");
            assert_eq!(cut.dates, full.dates, "{}: limit {limit}", workload.id);
            assert_eq!(
                cut.excluded, full.excluded,
                "{}: limit {limit}",
                workload.id
            );
            assert_eq!(
                cut.excluded_truncated, full.excluded_truncated,
                "{}: limit {limit}",
                workload.id
            );
            assert_eq!(
                cut.offers.as_slice(),
                &full.offers[..limit.min(full.offers.len())],
                "{}: limit {limit}",
                workload.id
            );
            assert_eq!(
                cut.truncated,
                full.offers.len() > limit,
                "{}: limit {limit}",
                workload.id
            );
        }
    }
}

/// The confirm-time workload is a real confirmation: the slot the search offered,
/// proposed back with the assignment it came with, and a `check_slot` answer that
/// agrees with the offer.
#[test]
fn the_confirm_workload_proposes_the_first_slot_the_search_offers() {
    let engine = engine();
    let confirm = slot_workload(&engine);
    assert_eq!(confirm.id, "check_slot");

    let search = &spec_workloads(&engine)[0];
    assert_eq!(confirm.request.participants.len(), PARTICIPANTS);
    assert_eq!(
        confirm.request.goal.parts[0].requirement,
        search.request.goal.parts[0].requirement
    );

    let offered = engine
        .find_availability(&search.request)
        .expect("a workload request is valid")
        .offers
        .into_iter()
        .next()
        .expect("the single-term workload offers starts");
    assert_eq!(confirm.proposal.events.len(), 1);
    let proposed = &confirm.proposal.events[0];
    assert_eq!(proposed.part, "main");
    assert_eq!(
        proposed.start,
        offered.start().expect("an offer has an event")
    );
    assert_eq!(proposed.assignment, offered.events[0].assignment);

    let check = engine
        .check_slot(&confirm.request, &confirm.proposal)
        .expect("an offered slot is a well-formed proposal");
    assert!(check.valid, "an offered slot confirms: {:?}", check.reasons);
    assert_eq!(check.flags, offered.flags);
    assert_eq!(check.reserve, offered.events[0].reserve);
}

/// The bounded-limit workloads are the base benchmark calendars made dense, and
/// the truncated variant really answers past the offer limit.
#[test]
fn the_bounded_limit_workloads_are_dense_and_truncate() {
    let engine = engine();
    let base = &spec_workloads(&engine)[0];
    let workloads = limit_workloads(&engine);
    assert_eq!(workloads.len(), 2);
    assert_eq!(
        workloads
            .iter()
            .map(|workload| workload.id)
            .collect::<Vec<_>>(),
        ["dense_blocks", "dense_blocks_truncated"]
    );

    for workload in &workloads {
        let request = &workload.request;
        assert_eq!(request.participants.len(), PARTICIPANTS);
        assert_eq!(
            request.goal.search_window, base.request.goal.search_window,
            "{} searches the same window as the base workload",
            workload.id
        );
        assert_eq!(request.rules.len(), base.request.rules.len());

        // The fourteen-day window holds ten weekdays and the company holiday
        // removes one, so every host's calendar carries nine days of focus blocks.
        // A host's leave day keeps them: the day's focus time is added before the
        // leave that replaces the rest of the day. Nine days of eight blocks takes
        // a host from 33 blocks to 105 and the request from 1,623 to 5,223, so
        // every block scan and the cap counting read about three times as much.
        let focus_days = 9;
        for (dense, sparse) in request.participants.iter().zip(&base.request.participants) {
            let extra = dense.blocks().len() - sparse.blocks().len();
            assert_eq!(
                extra,
                DENSE_EXTRA_BLOCKS_PER_DAY * focus_days,
                "{}: `{}` carries every focus block the window holds",
                workload.id,
                dense.id
            );
            assert_eq!(
                dense
                    .blocks()
                    .iter()
                    .filter(|block| block.tags.iter().any(|tag| tag.as_str() == "focus"))
                    .count(),
                DENSE_EXTRA_BLOCKS_PER_DAY * focus_days,
                "{}: `{}`'s extra blocks are exactly the ones tagged focus",
                workload.id,
                dense.id
            );
        }

        // The benchmark's documented density is a ratio of totals, not a count of
        // the blocks it adds: a base calendar that grew while the focus blocks
        // stayed put would leave this group measuring the base shape twice.
        let base_blocks: usize = base
            .request
            .participants
            .iter()
            .map(|participant| participant.blocks().len())
            .sum();
        let dense_blocks: usize = request
            .participants
            .iter()
            .map(|participant| participant.blocks().len())
            .sum();
        assert!(
            dense_blocks > base_blocks * 2,
            "{} carries {dense_blocks} blocks against the base workload's \
             {base_blocks}",
            workload.id
        );

        assert!(
            engine.validate(request).errors.is_empty(),
            "{} is a valid request: {:?}",
            workload.id,
            engine.validate(request).errors
        );
        let response = engine
            .find_availability(request)
            .expect("a workload request is valid");
        assert!(
            response.offers.len() >= TRUNCATED_LIMIT,
            "{} offers a working page",
            workload.id
        );
        assert!(!response.dates.is_empty(), "{} reports dates", workload.id);
        if workload.id == "dense_blocks" {
            // Dense calendars leave fewer bookable starts, and the default limit
            // still has room for all of them: the truncated variant is what
            // measures the search past the limit.
            let bookable: usize = response.dates.iter().map(|count| count.offers).sum();
            assert_eq!(response.offers.len(), bookable);
            assert!(!response.truncated);
        }
    }

    // The truncated variant asks for twenty-five offers out of the same window:
    // it reports fewer offers and nothing else differently.
    let dense = &workloads[0];
    let truncated = &workloads[1];
    assert_eq!(dense.request.limit, None);
    assert_eq!(truncated.request.limit, Some(TRUNCATED_LIMIT));
    let full = engine
        .find_availability(&dense.request)
        .expect("a workload request is valid");
    let cut = engine
        .find_availability(&truncated.request)
        .expect("a workload request is valid");
    assert_eq!(cut.offers.len(), TRUNCATED_LIMIT);
    assert_eq!(cut.dates, full.dates);
    assert_eq!(cut.excluded, full.excluded);
}

/// How many separate segments of the rule's late minutes and of its early minutes
/// each local date holds.
#[derive(Clone, Copy, Debug, Default)]
struct Segments {
    /// Runs of consecutive minutes at or after 17:30 local.
    late: usize,
    /// Runs of consecutive minutes before 10:00 local.
    early: usize,
}

/// One segment per run of consecutive minutes that share a local date and the same
/// late/early classification under the rule bounded by `late_after` and
/// `not_before`: the unit `rest` pairs one date's late minutes against the next
/// date's early minutes in.
///
/// Counted from the participant's own clock rather than through the engine's
/// interval construction, so the count says what the schedule carries and not what
/// the code under measurement made of it. A run the zone walk would split again at
/// a zone-period boundary counts once here, so these counts are a lower bound on
/// the segments `rest` reads.
fn segments_per_date(
    schedule: &ZoneSchedule,
    from: i64,
    until: i64,
    late_after: u16,
    not_before: u16,
) -> BTreeMap<Date, Segments> {
    let mut counts: BTreeMap<Date, Segments> = BTreeMap::new();
    let mut run: Option<(Date, bool, bool)> = None;
    for minute in from..until {
        let instant = Instant::from_minutes(minute)
            .expect("the workload's reach is inside the supported range");
        let local = schedule.zone_at(instant).zoned(instant);
        let date = local.date();
        let time = local.time().minute_of_day();
        let late = time >= late_after;
        let early = time < not_before;
        if run != Some((date, late, early)) {
            let segments = counts.entry(date).or_default();
            if late {
                segments.late += 1;
            }
            if early {
                segments.early += 1;
            }
            run = Some((date, late, early));
        }
    }
    counts
}

/// The rest-scaling workload's participant schedule, resolved the way the engine
/// resolves it.
fn participant_schedule(engine: &Engine, request: &Request) -> ZoneSchedule {
    let db = engine.time_zone_database();
    let participant = &request.participants[0];
    let periods = participant
        .zone_periods
        .iter()
        .map(|period| {
            ZonePeriod::new(
                db.zone(period.zone().as_str())
                    .unwrap_or_else(|err| panic!("`{err}`: a workload zone is in the database")),
                period.from(),
                period.until(),
            )
            .expect("a workload leg is non-empty")
        })
        .collect();
    ZoneSchedule::new(
        db.zone(participant.zone.as_str())
            .expect("the workload's home zone is in the database"),
        periods,
    )
    .expect("the workload's legs are disjoint")
}

/// The rest-scaling workload carries the shape its measurement claims: the
/// schedule cuts the rule's late and early minutes into many segments per local
/// date, so the number of late/early pairs `rest` reads per date pair grows far
/// faster than the size does. A workload that let its legs spread evenly over the
/// dates, or that stopped alternating zones, would measure nothing.
#[test]
fn the_rest_scaling_workload_carries_many_late_and_early_segments_per_day() {
    let engine = engine();

    // The most late/early pairs any one date pair presents, per size.
    let mut pairs = Vec::new();
    for zone_periods in REST_ZONE_PERIOD_SIZES {
        let request = rest_zone_period_workload(&engine, zone_periods);
        assert!(
            engine.validate(&request).errors.is_empty(),
            "{zone_periods} zone periods is a valid request: {:?}",
            engine.validate(&request).errors
        );
        assert_eq!(
            request.participants[0].zone_periods.len(),
            zone_periods,
            "every leg the size asks for is in the schedule"
        );

        // Three days either side of the search window, so every date the legs
        // land on and the home windows around them are counted.
        let rest = request.rules[0]
            .rest()
            .expect("the rest-scaling workload's only rule is a rest rule");
        let schedule = participant_schedule(&engine, &request);
        let day = 24 * 60;
        let window = request.goal.search_window;
        let segments = segments_per_date(
            &schedule,
            window.start().minutes() - 3 * day,
            window.end().minutes() + 4 * day,
            rest.late_after.minute_of_day(),
            rest.not_before.minute_of_day(),
        );

        // The legs land on more than one date, so the fragments really are the
        // date line moving the reading, not one long daily window.
        assert!(
            segments.len() >= 4,
            "{zone_periods}: the legs read as several local dates: {segments:?}"
        );

        // `self_conflict_spans` pairs `late(D)` with `early(D + 1)`, so the day
        // pair with the most segments on each side is what the workload makes.
        let best = segments
            .iter()
            .filter_map(|(date, late)| {
                let next = date.plus_days(1).ok()?;
                Some(late.late * segments.get(&next)?.early)
            })
            .max()
            .unwrap_or(0);
        let late_peak = segments.values().map(|of| of.late).max().unwrap_or(0);
        let early_peak = segments.values().map(|of| of.early).max().unwrap_or(0);
        assert!(
            late_peak >= zone_periods / 8 && early_peak >= zone_periods / 8,
            "{zone_periods}: each side carries many segments on one date: \
             late peak {late_peak}, early peak {early_peak}"
        );
        assert!(
            best >= (zone_periods / 2) * (zone_periods / 2) / 16,
            "{zone_periods}: the busiest date pair presents {best} late/early pairs"
        );
        if zone_periods
            == *REST_ZONE_PERIOD_SIZES
                .last()
                .expect("the sizes are non-empty")
        {
            // The workload is a live search, not an inert schedule: the one
            // candidate start is offered.
            let response = engine
                .find_availability(&request)
                .expect("the rest-scaling workload is a valid request");
            assert_eq!(
                response.offers.len(),
                1,
                "{zone_periods}: the start is offered"
            );
        }
        pairs.push(best);
    }

    // Quadratically more pairs for about thirty times the legs: an evenly spread
    // schedule would have grown linearly.
    let smallest = pairs.first().copied().expect("the sizes are non-empty");
    let largest = pairs.last().copied().expect("the sizes are non-empty");
    assert!(
        largest >= smallest * 8,
        "the pairs per date pair grow faster than the sizes do: {pairs:?}"
    );
}
