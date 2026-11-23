//! Requests written to the edges of the published `Limits::default()` are
//! answered, and are answered the same way twice.
//!
//! The standard benchmarks measure what a realistic request costs; these verify
//! that the inputs the published limits allow are reachable at all — five hundred
//! participants, one holding the whole per-participant block allowance and once
//! that allowance spread across the whole pool, and a search window whose
//! one-minute grid produces nearly the whole candidate-start allowance with an
//! explanation budget that runs out. Nothing here asserts a wall-clock figure: a
//! request that overflows, panics, allocates without bound or silently drops its
//! own answer fails on its observable result instead.

use std::collections::BTreeSet;

use opportune::Engine;
use opportune::model::{Block, Explain, Goal, Participant, Request, Requirement, Rule, Span, Term};
use opportune::presets;
use opportune::tags::{Selector, Tag};
use opportune::time::{
    Duration, Grid, GridZone, Instant, RuleZone, TimeOfDay, TimeZoneDatabase, WallEnd, Weekday,
    Weekdays, ZoneName,
};

const ZONE: &str = "Europe/London";

fn engine() -> Engine {
    Engine::builder()
        .build()
        .expect("the bundled database builds")
}

fn zone_name(name: &str) -> ZoneName {
    ZoneName::new(name).expect("a request zone is in the bundled database")
}

fn tag(path: &str) -> Tag {
    Tag::new(path).expect("a request tag is a valid path")
}

/// The day every ceiling request searches from: the first supported instant's
/// local date, which leaves whole weeks of working days inside any window.
fn first_day(db: &TimeZoneDatabase) -> opportune::time::Date {
    let zone = db.zone(ZONE).expect("the request zone is bundled");
    Instant::MIN.zoned(&zone).date()
}

fn day_start(db: &TimeZoneDatabase, date: opportune::time::Date) -> Instant {
    db.zone(ZONE)
        .expect("the request zone is bundled")
        .day_start(date)
        .expect("a request date starts at a supported instant")
}

/// The working-hours window every participant of a ceiling request carries, so a
/// candidate start nobody can take is excluded rather than offered.
fn business_hours() -> Rule {
    presets::business_hours(
        RuleZone::Participant,
        Weekdays::range(Weekday::Monday, Weekday::Friday),
        TimeOfDay::from_minute_of_day(9 * 60).expect("nine o'clock is a time of day"),
        WallEnd::Time(
            TimeOfDay::from_minute_of_day(17 * 60).expect("five o'clock is a time of day"),
        ),
    )
    .expect("working hours cover the day")
}

/// A goal searching `days` days from the first supported date, on a
/// `grid_minutes` grid anchored at local midnight in the event's zone.
fn goal(db: &TimeZoneDatabase, days: i64, grid_minutes: u32) -> Goal {
    let first = first_day(db);
    let window = Span::new(
        day_start(db, first),
        day_start(db, first.plus_days(days).expect("a window end is a date")),
    )
    .expect("a window of whole days is non-empty");
    let mut goal = Goal::single(
        zone_name(ZONE),
        window,
        Duration::from_minutes(30),
        Requirement::new(vec![Term::at_least(1, Selector::Tag(tag("pool/hosts")))]),
    );
    goal.grid = Some(
        Grid::new(
            Duration::from_minutes(grid_minutes),
            TimeOfDay::MIDNIGHT,
            GridZone::Event,
        )
        .expect("a grid step of a day or less is inside the grid's range"),
    );
    goal
}

/// A participant with `blocks` minute-long blocks spread evenly over `days` days,
/// each on its own event key, so the block scan and the cap counting read every
/// one of them.
fn dense_participant(db: &TimeZoneDatabase, id: &str, days: i64, blocks: usize) -> Participant {
    let mut participant = Participant::new(id.to_owned(), zone_name(ZONE));
    participant.tags = vec![tag("pool/hosts")];
    let first = first_day(db);
    let span_minutes = days * 24 * 60;
    // At least a minute apart, so a caller cannot stack every block on one
    // instant whatever it passes.
    let step = (span_minutes / i64::try_from(blocks).expect("a block count fits an i64")).max(1);
    let mut calendar = Vec::with_capacity(blocks);
    for index in 0..blocks {
        let offset = i64::try_from(index).expect("a block index fits an i64") * step;
        let start =
            Instant::from_minutes(day_start(db, first).minutes() + offset.min(span_minutes - 1))
                .expect("a block inside the window is a supported instant");
        let mut block = Block::new(
            Span::new(
                start,
                Instant::from_minutes(start.minutes() + 1).expect("one minute later is supported"),
            )
            .expect("a one-minute block is non-empty"),
        );
        block.event_key = Some(format!("{id}/{index}"));
        block.tags = vec![tag("interview")];
        calendar.push(block);
    }
    participant.calendar = Some(calendar);
    participant
}

/// A participant with an empty calendar, tagged so the ceiling requests' term
/// selects them.
fn free_participant(id: &str) -> Participant {
    let mut participant = Participant::new(id, zone_name(ZONE));
    participant.tags = vec![tag("pool/hosts")];
    participant.calendar = Some(Vec::new());
    participant
}

/// A ceiling request: the configured maximum number of participants, the first of
/// them holding the configured maximum number of blocks, and a window of the
/// configured maximum length, searched on an hourly grid for twenty-five offers.
fn ceiling_request(engine: &Engine) -> Request {
    let db = engine.time_zone_database();
    let limits = engine.limits();
    let days = i64::from(limits.max_search_window_days);
    let mut request = Request::new(day_start(db, first_day(db)), goal(db, days, 60));
    request.limit = Some(25);
    request.rules = vec![business_hours()];
    request.explain = Some(Explain::new(limits.max_explain_limit));
    for index in 0..limits.max_participants {
        let id = format!("host-{index:04}");
        let blocks = if index == 0 {
            limits.max_blocks_per_participant
        } else {
            4
        };
        request
            .participants
            .push(dense_participant(db, &id, days, blocks));
    }
    request
}

/// The dates a request's window touches, in the event's zone. The windows here
/// start at a local midnight, so a date is touched exactly when its own start is
/// inside the window.
fn window_dates(engine: &Engine, request: &Request) -> Vec<opportune::time::Date> {
    let zone = engine
        .time_zone_database()
        .zone(request.goal.zone.as_str())
        .expect("the request zone is bundled");
    let window = request.goal.search_window;
    let is_outside = |instant: Instant| !window.contains(instant);
    let mut dates = Vec::new();
    let mut date = zone.zoned(window.start()).date();
    while !is_outside(
        zone.day_start(date)
            .expect("a window date starts at some instant"),
    ) {
        dates.push(date);
        date = date.plus_days(1).expect("a window date has a next day");
    }
    dates
}

#[test]
fn the_participant_and_block_ceilings_are_answered() {
    let engine = engine();
    let limits = engine.limits();
    let request = ceiling_request(&engine);
    assert_eq!(request.participants.len(), limits.max_participants);
    assert_eq!(
        request.participants[0].blocks().len(),
        limits.max_blocks_per_participant
    );
    assert!(
        engine.validate(&request).errors.is_empty(),
        "a request at every configured ceiling is valid: {:?}",
        engine.validate(&request).errors
    );

    let response = engine
        .find_availability(&request)
        .expect("a ceiling request is answered");
    assert_eq!(response.offers.len(), request.limit());
    assert!(
        response.truncated,
        "the window holds more bookable starts than the request asked for"
    );

    let known: BTreeSet<&str> = request
        .participants
        .iter()
        .map(|participant| participant.id.as_str())
        .collect();
    let window = request.goal.search_window;
    for offer in &response.offers {
        let event = &offer.events[0];
        assert!(window.contains(event.start));
        assert_eq!(event.end.minutes() - event.start.minutes(), 30);
        for assigned in event.assignment.iter().flatten() {
            assert!(
                known.contains(assigned.as_str()),
                "`{assigned}` is one of the request's participants"
            );
        }
    }

    // The date counts cover the window before the limit drops anything, so the
    // participant holding the block ceiling costs the answer nothing but the
    // offers it is part of.
    let dates = window_dates(&engine, &request);
    assert_eq!(
        response
            .dates
            .iter()
            .map(|count| count.date)
            .collect::<Vec<_>>(),
        dates
    );
    let bookable: usize = response.dates.iter().map(|count| count.offers).sum();
    assert!(
        bookable >= response.offers.len(),
        "every offered start is counted on its date"
    );

    // An offered slot confirms, at the same ceiling.
    let offer = &response.offers[0];
    let check = engine
        .check_slot(&request, &offer.to_proposal())
        .expect("a proposal built from an offer is well formed");
    assert!(check.valid, "an offered slot confirms: {:?}", check.reasons);
    assert_eq!(check.flags, offer.flags);
    assert_eq!(check.reserve, offer.events[0].reserve);

    // The same request answers the same way twice.
    let again = engine
        .find_availability(&request)
        .expect("a ceiling request is answered");
    assert_eq!(again.offers, response.offers);
    assert_eq!(again.dates, response.dates);
    assert_eq!(again.excluded, response.excluded);
}

#[test]
fn the_candidate_ceiling_is_answered_without_dropping_its_explanation() {
    let engine = engine();
    let limits = engine.limits();
    let db = engine.time_zone_database();
    // A one-minute grid over enough days to put nearly the whole candidate-start
    // allowance in the window.
    let days = 69;
    let starts = days * 24 * 60;
    assert!(
        usize::try_from(starts).expect("a day count fits a usize") < limits.max_candidate_starts
    );

    let mut request = Request::new(day_start(db, first_day(db)), goal(db, days, 1));
    request.rules = vec![business_hours()];
    request.explain = Some(Explain::new(limits.max_explain_limit));
    for id in ["ana", "bo"] {
        request.participants.push(free_participant(id));
    }
    assert!(
        engine.validate(&request).errors.is_empty(),
        "a candidate-ceiling request is valid: {:?}",
        engine.validate(&request).errors
    );

    let response = engine
        .find_availability(&request)
        .expect("a request at the candidate ceiling is answered");
    assert_eq!(response.offers.len(), request.limit());
    assert!(response.truncated);
    assert!(!response.dates.is_empty());

    // Most starts are outside a working day or before the notice period, so the
    // explanation runs out of budget rather than answering anyway: entries are
    // atomic and the count never passes the limit the request asked for.
    let reasons: usize = response
        .excluded
        .iter()
        .map(|excluded| excluded.reasons.len())
        .sum();
    assert!(!response.excluded.is_empty());
    assert!(response.excluded_truncated);
    assert!(reasons <= limits.max_explain_limit);

    // Every reason names a participant of the request or is a start-level one,
    // and every excluded start is a candidate start inside the window.
    let window = request.goal.search_window;
    for entry in &response.excluded {
        assert!(window.contains(entry.start));
        for reason in &entry.reasons {
            if let Some(participant) = &reason.participant {
                assert!(["ana", "bo"].contains(&participant.as_str()));
            }
        }
    }

    let again = engine
        .find_availability(&request)
        .expect("a ceiling request is answered");
    assert_eq!(again.excluded, response.excluded);
    assert_eq!(again.excluded_truncated, response.excluded_truncated);
}

#[test]
fn a_window_over_the_candidate_ceiling_is_a_validation_error_rather_than_a_slow_search() {
    let engine = engine();
    let limits = engine.limits();
    let db = engine.time_zone_database();
    // Two minutes under the grid's shortest step would already be too many
    // starts; a year of them is far past the allowance.
    let days = i64::from(limits.max_search_window_days);
    let mut request = Request::new(day_start(db, first_day(db)), goal(db, days, 1));
    request.rules = vec![business_hours()];
    request.participants.push(free_participant("ana"));

    let diagnostics = engine.validate(&request);
    assert!(
        diagnostics
            .errors
            .iter()
            .any(|error| error.code() == "candidates_limit"),
        "a window past the candidate-start allowance is rejected: {:?}",
        diagnostics.errors
    );
    match engine.find_availability(&request) {
        Err(opportune::error::Error::InvalidRequest { .. }) => {}
        other => panic!("the search refuses rather than running: {other:?}"),
    }
}

#[test]
fn a_participant_over_the_block_ceiling_is_a_validation_error() {
    let engine = engine();
    let limits = engine.limits();
    let db = engine.time_zone_database();
    let mut request = Request::new(day_start(db, first_day(db)), goal(db, 7, 60));
    request.rules = vec![business_hours()];
    request.participants.push(dense_participant(
        db,
        "ana",
        7,
        limits.max_blocks_per_participant + 1,
    ));

    let diagnostics = engine.validate(&request);
    assert!(
        diagnostics
            .errors
            .iter()
            .any(|error| error.code() == "blocks_limit"),
        "one block past the allowance is rejected: {:?}",
        diagnostics.errors
    );
}

/// A request whose pools are disjoint assigns each term on its own; one whose
/// pools are the same fifty participants solves them jointly. Both are answered
/// at the participant ceiling, and neither may put one host in two terms.
#[test]
fn the_participant_ceiling_is_answered_for_several_terms() {
    let engine = engine();
    let limits = engine.limits();
    let db = engine.time_zone_database();
    let days = 14;
    for disjoint in [true, false] {
        let mut request = Request::new(day_start(db, first_day(db)), goal_with_terms(db, days, 3));
        request.limit = Some(25);
        request.rules = vec![business_hours()];
        for index in 0..limits.max_participants {
            let id = format!("host-{index:04}");
            let mut participant = free_participant(&id);
            // A disjoint request puts each host in one term's pool; a joined one
            // puts every host in every pool.
            let groups: Vec<usize> = if disjoint {
                vec![index % 3]
            } else {
                (0..3).collect()
            };
            for group in &groups {
                participant.tags.push(tag(&format!("grp/{group}")));
            }
            request.participants.push(participant);
        }
        assert!(
            engine.validate(&request).errors.is_empty(),
            "a multi-term request at the participant ceiling is valid: {:?}",
            engine.validate(&request).errors
        );

        let response = engine
            .find_availability(&request)
            .expect("a multi-term ceiling request is answered");
        assert!(!response.offers.is_empty(), "disjoint: {disjoint}");
        assert!(response.truncated, "disjoint: {disjoint}");
        for offer in &response.offers {
            let event = &offer.events[0];
            assert_eq!(event.assignment.len(), 3);
            let mut assigned: Vec<&str> = Vec::new();
            for (group, hosts) in event.assignment.iter().enumerate() {
                // Every term asks for one host out of its pool.
                assert_eq!(hosts.len(), 1, "disjoint: {disjoint}");
                for host in hosts {
                    assert!(
                        !assigned.contains(&host.as_str()),
                        "`{host}` fills only one term"
                    );
                    assigned.push(host.as_str());
                    let participant = request
                        .participants
                        .iter()
                        .find(|participant| participant.id == *host)
                        .expect("an assigned host is one of the request's participants");
                    assert!(
                        participant
                            .tags
                            .iter()
                            .any(|tag| tag.as_str() == format!("grp/{group}")),
                        "`{host}` fills term {group}, whose pool it is in"
                    );
                }
            }
        }

        // Past the offer limit the search answers feasibility without the
        // assignment. Raising the limit reports those same starts as offers and
        // nothing else differently, so the two answers agree about which starts
        // the requirement can be filled for at all.
        let mut uncut = request.clone();
        uncut.limit = Some(usize::MAX);
        let full = engine
            .find_availability(&uncut)
            .expect("a multi-term ceiling request is answered");
        assert!(!full.truncated, "disjoint: {disjoint}");
        assert_eq!(full.dates, response.dates, "disjoint: {disjoint}");
        assert_eq!(full.excluded, response.excluded, "disjoint: {disjoint}");
        let bookable: usize = full.dates.iter().map(|count| count.offers).sum();
        assert_eq!(full.offers.len(), bookable, "disjoint: {disjoint}");
        assert_eq!(
            response.offers.as_slice(),
            &full.offers[..response.offers.len()],
            "disjoint: {disjoint}"
        );
    }
}

/// A goal asking for one host from each of three tagged pools.
fn goal_with_terms(db: &TimeZoneDatabase, days: i64, terms: usize) -> Goal {
    let mut goal = goal(db, days, 60);
    goal.parts[0].requirement = Requirement::new(
        (0..terms)
            .map(|group| Term::at_least(1, Selector::Tag(tag(&format!("grp/{group}")))))
            .collect(),
    );
    goal
}

/// The block allowance spread across the whole pool rather than held by one
/// participant: the axis the ceiling request above cannot reach, because 500
/// participants each at the 10,000-block maximum is five million blocks and no
/// test should hold that. A hundred thousand over the whole pool is, and every
/// published limit allows it.
///
/// Nothing here asserts a wall-clock figure. What it pins is that a request of
/// that shape is answered with the offers, the date counts and the confirmation
/// it asks for, and that two runs agree — so a block scan or a pooled cap count
/// that stopped being proportional to the blocks shows up as a wrong answer or as
/// the runner's slow timeout, not as a flaky timing assertion.
#[test]
fn the_block_allowance_spread_across_the_pool_is_answered() {
    let engine = engine();
    let limits = engine.limits();
    let db = engine.time_zone_database();
    let days = 14;
    for blocks in [50_usize, 200] {
        let mut request = Request::new(day_start(db, first_day(db)), goal(db, days, 60));
        request.limit = Some(25);
        request.rules = vec![business_hours()];
        request.explain = Some(Explain::new(limits.max_explain_limit));
        for index in 0..limits.max_participants {
            let id = format!("host-{index:04}");
            request
                .participants
                .push(dense_participant(db, &id, days, blocks));
        }
        let total: usize = request
            .participants
            .iter()
            .map(|participant| participant.blocks().len())
            .sum();
        assert_eq!(
            total,
            blocks * limits.max_participants,
            "{blocks} blocks on each of the pool"
        );
        assert!(
            engine.validate(&request).errors.is_empty(),
            "{blocks} blocks on every participant is inside each published limit: {:?}",
            engine.validate(&request).errors
        );

        let response = engine
            .find_availability(&request)
            .expect("a request spread across the pool is answered");
        assert_eq!(
            response.offers.len(),
            request.limit(),
            "{blocks} blocks on each of the pool"
        );
        assert!(
            response.truncated,
            "{blocks} blocks on each of the pool leaves more bookable starts than the limit"
        );

        let known: BTreeSet<&str> = request
            .participants
            .iter()
            .map(|participant| participant.id.as_str())
            .collect();
        let window = request.goal.search_window;
        for offer in &response.offers {
            let event = &offer.events[0];
            assert!(window.contains(event.start));
            for assigned in event.assignment.iter().flatten() {
                assert!(
                    known.contains(assigned.as_str()),
                    "`{assigned}` is one of the request's participants"
                );
            }
        }
        assert_eq!(
            response
                .dates
                .iter()
                .map(|count| count.date)
                .collect::<Vec<_>>(),
            window_dates(&engine, &request),
            "{blocks} blocks on each of the pool"
        );
        let bookable: usize = response.dates.iter().map(|count| count.offers).sum();
        assert!(
            bookable > response.offers.len(),
            "{blocks} blocks on each of the pool still leaves {bookable} bookable starts"
        );

        // A confirmation reads every block in the request, so it is the part of
        // this shape whose cost grows with the pool's blocks rather than with the
        // booking it judges.
        let offer = &response.offers[0];
        let check = engine
            .check_slot(&request, &offer.to_proposal())
            .expect("a proposal built from an offer is well formed");
        assert!(
            check.valid,
            "{blocks} blocks on each of the pool: an offered slot confirms: {:?}",
            check.reasons
        );
        assert_eq!(check.flags, offer.flags);
        assert_eq!(check.reserve, offer.events[0].reserve);

        let again = engine
            .find_availability(&request)
            .expect("a request spread across the pool is answered");
        assert_eq!(again.offers, response.offers);
        assert_eq!(again.dates, response.dates);
        assert_eq!(again.excluded, response.excluded);
    }
}
