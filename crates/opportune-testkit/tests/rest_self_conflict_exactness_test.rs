//! Exactness probe for the `rest` self-conflict construction.
//!
//! `rest::self_conflict_spans` answers "at which occupied starts does a booking of
//! this length conflict with *itself* across one `(late(D), early(D + 1))` pair".
//! It builds that answer by widening each local date's late and early minute
//! segments and intersecting the two widened sets, which is only equivalent to the
//! pairwise product it replaced while the widening, the coalescing and the date
//! walk are all exact.
//!
//! Two fixtures drive the same question from the definition instead: walk the
//! booking's own minutes, classify each in the zone in force at that instant, and
//! ask whether some local date `D` has a late minute and `D + 1` has an early one
//! *inside the booking*.
//!
//! * The first shuttles the clock across the date line every other minute for part
//!   of the window, so each local date carries many separate late and early
//!   segments and the widened segments of one date pair overlap each other. It
//!   exercises the widen-then-canonicalise order.
//! * The second uses a long booking on a plain UTC clock, which reaches its own
//!   date's early *and* late minutes without ever reaching the next date's early
//!   ones. It pins the `D` / `D + 1` pairing itself, which the first fixture cannot
//!   see: there every late date happens to pair with every early date, so dropping
//!   the pairing would go unnoticed.

use std::collections::BTreeSet;
use std::sync::OnceLock;

use opportune::model::{Explain, Goal, Participant, Request, Requirement, Span, Term};
use opportune::presets;
use opportune::reasons::ReasonCode;
use opportune::time::ZonePeriod as TimeZonePeriod;
use opportune::time::{
    Date, Duration, Instant, LocalResolution, TimeOfDay, TimeZoneDatabase, Zone, ZoneName,
    ZoneSchedule,
};
use opportune::{Engine, Limits, ZonePeriod as ModelZonePeriod};

/// The rule's late boundary: at or after 17:30 local.
const LATE_AFTER_MINUTE_OF_DAY: u16 = 17 * 60 + 30;

/// The rule's early boundary: before 10:00 local.
const NOT_BEFORE_MINUTE_OF_DAY: u16 = 10 * 60;

fn database() -> &'static TimeZoneDatabase {
    static DATABASE: OnceLock<TimeZoneDatabase> = OnceLock::new();
    DATABASE.get_or_init(|| TimeZoneDatabase::bundled().expect("the bundled database is present"))
}

fn engine() -> &'static Engine {
    static ENGINE: OnceLock<Engine> = OnceLock::new();
    ENGINE.get_or_init(|| {
        Engine::builder()
            .limits(Limits::default().with_max_explain_limit(1_000_000))
            .build()
            .expect("the bundled database builds")
    })
}

fn zone(name: &str) -> Zone {
    database()
        .zone(name)
        .unwrap_or_else(|err| panic!("`{name}` is in the bundled database: {err}"))
}

fn zone_name(name: &str) -> ZoneName {
    ZoneName::new(name).expect("a zone name")
}

fn date(day: i8) -> Date {
    Date::new(2027, 6, day).expect("a June 2027 date")
}

fn time(hour: u8, minute: u8) -> TimeOfDay {
    TimeOfDay::new(hour, minute).expect("a time of day")
}

/// The instant a UTC date-time falls on.
fn utc(day: Date, hour: u8, minute: u8) -> Instant {
    match Zone::utc()
        .resolve_local(day, time(hour, minute))
        .expect("a fixture instant is inside the supported range")
    {
        LocalResolution::Unique(instant) => instant,
        LocalResolution::Skipped | LocalResolution::Repeated(_, _) => {
            panic!("UTC has no skipped or repeated local times")
        }
    }
}

fn instants(from: Instant, until: Instant) -> Vec<Instant> {
    (from.minutes()..until.minutes())
        .map(|minute| Instant::from_minutes(minute).expect("a candidate is inside the range"))
        .collect()
}

/// One-minute legs every other minute across `minutes` minutes from `hour:minute`
/// on `day`, alternating between the two sides of the date line.
///
/// Pitched at two minutes so consecutive legs are separated by a minute at home,
/// which cuts each local date's late and early minutes into many short segments —
/// the shape the widen-and-intersect construction exists to handle.
fn date_line_legs(
    day: Date,
    hour: u8,
    minute: u8,
    minutes: i64,
    out: &mut Vec<(ZoneName, Instant, Instant)>,
) {
    let base = utc(day, hour, minute).minutes();
    let mut slot = 0_i64;
    while slot * 2 < minutes {
        let from = base + slot * 2;
        out.push((
            zone_name(if slot % 2 == 0 {
                "Pacific/Kiritimati"
            } else {
                "Etc/GMT+12"
            }),
            Instant::from_minutes(from).expect("a leg start is inside the range"),
            Instant::from_minutes(from + 1).expect("a leg end is inside the range"),
        ));
        slot += 1;
    }
}

/// A request booking one event of `duration` minutes on a host whose zone periods
/// are `legs`, judging exactly `candidates`.
fn build_request(
    duration_minutes: u32,
    search: Span,
    candidates: Vec<Instant>,
    legs: &[(ZoneName, Instant, Instant)],
) -> Request {
    let mut participant = Participant::new("rich", zone_name("UTC")).with_calendar(Vec::new());
    participant.zone_periods = legs
        .iter()
        .map(|(name, from, until)| {
            ModelZonePeriod::new(name.clone(), *from, *until)
                .expect("a one-minute leg is non-empty")
        })
        .collect();

    let mut goal = Goal::single(
        zone_name("UTC"),
        search,
        Duration::from_minutes(duration_minutes),
        Requirement::new(vec![Term::person("rich")]),
    );
    goal.candidates = Some(candidates);

    let mut request = Request::new(
        Instant::from_minutes(search.start().minutes() - 60).expect("a minute before the window"),
        goal,
    );
    request.participants.push(participant);
    request.rules = vec![presets::rest(time(17, 30), time(10, 0))];
    request.explain = Some(Explain::new(1_000_000));
    request
}

/// The local dates an occupied span reads as a late date and as an early date,
/// minute by minute in the zone in force at each instant.
fn classified(schedule: &ZoneSchedule, occupied: (i64, i64)) -> (BTreeSet<Date>, BTreeSet<Date>) {
    let mut late = BTreeSet::new();
    let mut early = BTreeSet::new();
    for minute in occupied.0..occupied.1 {
        let instant = Instant::from_minutes(minute).expect("an occupied minute is in range");
        let local = schedule.zone_at(instant).zoned(instant);
        let date = local.date();
        let minute_of_day = local.time().minute_of_day();
        if minute_of_day >= LATE_AFTER_MINUTE_OF_DAY {
            late.insert(date);
        }
        if minute_of_day < NOT_BEFORE_MINUTE_OF_DAY {
            early.insert(date);
        }
    }
    (late, early)
}

/// The specification's own answer: the booking conflicts with itself when some
/// local date `D` has a late minute of the booking and `D + 1` has an early one.
fn self_conflicts(schedule: &ZoneSchedule, start: Instant, duration_minutes: i64) -> bool {
    let occupied = (start.minutes(), start.minutes() + duration_minutes);
    let (late, early) = classified(schedule, occupied);
    late.iter()
        .any(|day| day.plus_days(1).is_ok_and(|next| early.contains(&next)))
}

/// The engine's schedule for these legs: the same periods, resolved the way the
/// engine resolves them.
fn schedule(legs: &[(ZoneName, Instant, Instant)]) -> ZoneSchedule {
    let periods: Vec<TimeZonePeriod> = legs
        .iter()
        .map(|(name, from, until)| {
            TimeZonePeriod::new(zone(name.as_str()), *from, *until)
                .expect("a fixture leg is non-empty")
        })
        .collect();
    ZoneSchedule::new(zone("UTC"), periods).expect("the fixture legs are disjoint")
}

/// Compares the engine's offers against the definition at every candidate start.
fn assert_offers_match_the_definition(
    request: &Request,
    candidates: &[Instant],
    legs: &[(ZoneName, Instant, Instant)],
    duration_minutes: i64,
) {
    let schedule = schedule(legs);
    let response = engine()
        .find_availability(request)
        .expect("the fixture is a valid request");

    let offered: Vec<Instant> = response
        .offers
        .iter()
        .map(|offer| offer.start().expect("an offer has an event"))
        .collect();
    let excluded: Vec<Instant> = response.excluded.iter().map(|entry| entry.start).collect();
    let expected: Vec<Instant> = candidates
        .iter()
        .copied()
        .filter(|start| !self_conflicts(&schedule, *start, duration_minutes))
        .collect();

    assert_eq!(
        offered, expected,
        "every start the definition clears is offered, and no other"
    );
    assert_eq!(
        offered.len() + excluded.len(),
        candidates.len(),
        "every candidate start is judged one way or the other"
    );
    for entry in &response.excluded {
        assert!(
            entry
                .reasons
                .iter()
                .all(|reason| reason.code == ReasonCode::Rest),
            "the fixture carries no block, cap or window, so rest is the only exclusion: {:?}",
            entry.reasons
        );
    }
}

#[test]
fn the_self_conflict_set_is_exactly_the_starts_that_reach_late_then_early_across_the_date_line() {
    let day = date(14);
    let mut legs = Vec::new();
    // 05:30-08:00Z reads as an evening on one side of the line and the previous
    // date's evening on the other.
    date_line_legs(day, 5, 30, 150, &mut legs);
    // 12:00-14:00Z reads as a morning on one side and the next date's morning on
    // the other, so a booking spanning both reaches `late(D)` and `early(D + 1)`
    // for two different `D` at once.
    date_line_legs(day, 12, 0, 120, &mut legs);

    let search = Span::new(utc(day, 4, 0), utc(day, 9, 0)).expect("the window is non-empty");
    let candidates = instants(search.start(), search.end());
    let request = build_request(300, search, candidates.clone(), &legs);

    assert_offers_match_the_definition(&request, &candidates, &legs, 300);

    let response = engine()
        .find_availability(&request)
        .expect("the fixture is a valid request");
    // Non-vacuity: the fixture has to reach both outcomes, or it says nothing.
    assert!(
        response.excluded.len() >= 30,
        "the fixture excludes many starts, not {}",
        response.excluded.len()
    );
    assert!(
        !response.offers.is_empty(),
        "the fixture also offers starts the definition clears"
    );

    // Control: without the legs the same window clears every start, so the
    // exclusions above come from the fragmented clock and not from the window.
    let plain = build_request(300, search, candidates.clone(), &[]);
    let plain = engine()
        .find_availability(&plain)
        .expect("the control is a valid request");
    assert_eq!(
        plain.offers.len(),
        candidates.len(),
        "a UTC host in this window self-conflicts at no start"
    );
    assert!(plain.excluded.is_empty(), "the control excludes nothing");
}

#[test]
fn a_booking_that_reaches_only_its_own_dates_late_and_early_minutes_is_offered() {
    // A ten-hour booking can hold an early minute of local date `D` and a late
    // minute of the same `D` without ever reaching an early minute of `D + 1`, so
    // the `D` / `D + 1` pairing keeps it: the rule pairs an evening with the *next*
    // morning, not with the same day's. Starts from 20:00 onward do reach the next
    // date's early minutes and are excluded, so the fixture pins both directions.
    let day = date(14);
    let search = Span::new(utc(day, 8, 0), utc(day, 21, 0)).expect("the window is non-empty");
    let mut candidates = instants(utc(day, 8, 0), utc(day, 9, 30));
    candidates.extend(instants(utc(day, 20, 0), utc(day, 20, 30)));
    let request = build_request(600, search, candidates.clone(), &[]);

    assert_offers_match_the_definition(&request, &candidates, &[], 600);

    let response = engine()
        .find_availability(&request)
        .expect("the fixture is a valid request");
    let offered: Vec<Instant> = response
        .offers
        .iter()
        .map(|offer| offer.start().expect("an offer has an event"))
        .collect();
    assert_eq!(
        offered,
        instants(utc(day, 8, 0), utc(day, 9, 30)),
        "the same-date starts are offered"
    );
    assert_eq!(
        response.excluded.len(),
        instants(utc(day, 20, 0), utc(day, 20, 30)).len(),
        "the starts that reach the next date's early minutes are excluded"
    );
}
