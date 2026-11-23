//! Hand-computed fixtures across the zone matrix.
//!
//! These exercise the pieces that only meet at the module boundary: grids over
//! real daylight saving transitions, zone periods across the date line, and
//! horizons in their configured zone. They compare against a
//! minute-by-minute model that walks instants forward and asks what the local
//! clock says — the opposite direction from the grid, so a shared misreading of
//! a transition shows up as a disagreement.

#![cfg(feature = "bundled-tzdb")]

use crate::time::{
    Date, Duration, Grid, GridZone, Horizon, Instant, LocalResolution, TimeOfDay, TimeZoneDatabase,
    WorkingCalendar, WorkingDays, Zone, ZonePeriod, ZoneSchedule,
};

/// The zone matrix every time fixture runs across: both hemispheres, a
/// half-hour daylight saving shift, 5:30 and 5:45 offsets, and UTC.
const ZONE_MATRIX: [&str; 9] = [
    "America/Los_Angeles",
    "Europe/London",
    "Australia/Sydney",
    "Australia/Lord_Howe",
    "Asia/Kolkata",
    "Asia/Kathmandu",
    "America/Anchorage",
    "Asia/Tokyo",
    "UTC",
];

fn database() -> TimeZoneDatabase {
    TimeZoneDatabase::bundled().expect("the bundled database is present")
}

fn zone(name: &str) -> Zone {
    database()
        .zone(name)
        .unwrap_or_else(|err| panic!("{name} is in the bundled database: {err}"))
}

/// The instant a local date-time exists at, failing on a skipped local time.
fn local(zone: &Zone, date: (i16, i8, i8), time: (u8, u8)) -> Instant {
    let date = Date::new(date.0, date.1, date.2).unwrap();
    let time = TimeOfDay::new(time.0, time.1).unwrap();
    match zone.resolve_local(date, time).unwrap() {
        LocalResolution::Unique(instant) => instant,
        other => panic!(
            "{date} {time} in {} is not a single instant: {other:?}",
            zone.name()
        ),
    }
}

fn utc(date: (i16, i8, i8), time: (u8, u8)) -> Instant {
    local(&Zone::utc(), date, time)
}

/// Resolves a horizon against `now` in `event_zone`, then answers for one start.
fn allows(horizon: &Horizon, start: Instant, now: Instant, event_zone: &Zone) -> bool {
    horizon.bound(now, event_zone).unwrap().allows(start)
}

/// Every minute in `[from, until)` whose local clock says it is on the grid.
///
/// This is the model the grid must agree with: it converts each instant to the
/// grid zone's local time and applies the remainder rule there, so it needs no
/// knowledge of transitions at all.
fn grid_by_minute(
    zone: &Zone,
    from: Instant,
    until: Instant,
    step: i64,
    anchor: u16,
) -> Vec<Instant> {
    let mut starts = Vec::new();
    for minute in from.minutes()..until.minutes() {
        let instant = Instant::from_minutes(minute).unwrap();
        let minute_of_day = i64::from(instant.zoned(zone).time().minute_of_day());
        if (minute_of_day - i64::from(anchor)).rem_euclid(step) == 0 {
            starts.push(instant);
        }
    }
    starts
}

#[test]
fn grid_agrees_with_minute_by_minute_membership_across_the_zone_matrix() {
    let windows = [
        // North American spring forward.
        (utc((2027, 3, 13), (0, 0)), utc((2027, 3, 15), (0, 0))),
        // European spring forward.
        (utc((2027, 3, 27), (0, 0)), utc((2027, 3, 29), (0, 0))),
        // Southern autumn fall back, including Lord Howe's half hour.
        (utc((2027, 4, 3), (0, 0)), utc((2027, 4, 5), (0, 0))),
        // Southern spring forward.
        (utc((2027, 10, 1), (0, 0)), utc((2027, 10, 4), (0, 0))),
        // North American fall back.
        (utc((2027, 11, 6), (0, 0)), utc((2027, 11, 8), (0, 0))),
        // A search window across 29 February in a leap year.
        (utc((2028, 2, 27), (0, 0)), utc((2028, 3, 2), (0, 0))),
    ];
    for name in ZONE_MATRIX {
        let zone = zone(name);
        for (from, until) in windows {
            for step in [7, 25] {
                for anchor in [0, 13] {
                    let grid = Grid::new(
                        Duration::from_minutes(step),
                        TimeOfDay::from_minute_of_day(anchor).unwrap(),
                        GridZone::Event,
                    )
                    .unwrap();
                    let starts = grid.starts(&zone, from, until).unwrap();
                    assert_eq!(
                        starts,
                        grid_by_minute(&zone, from, until, i64::from(step), anchor),
                        "{name} step {step} anchor {anchor} over {from}..{until}"
                    );
                }
            }
        }
    }
}

#[test]
fn a_skipped_local_hour_gives_no_starts() {
    // New York springs forward at 02:00 local on 2027-03-14.
    let new_york = zone("America/New_York");
    let day = (
        local(&new_york, (2027, 3, 14), (0, 0)),
        local(&new_york, (2027, 3, 15), (0, 0)),
    );
    let grid = Grid::new(
        Duration::from_minutes(60),
        TimeOfDay::MIDNIGHT,
        GridZone::Event,
    )
    .unwrap();
    let starts = grid.starts(&new_york, day.0, day.1).unwrap();

    // The local day is 23 hours long and 02:00 never happens.
    assert_eq!(starts.len(), 23);
    let times: Vec<TimeOfDay> = starts.iter().map(|s| s.zoned(&new_york).time()).collect();
    assert!(!times.contains(&TimeOfDay::new(2, 0).unwrap()));
    assert!(times.contains(&TimeOfDay::new(1, 0).unwrap()));
    assert!(times.contains(&TimeOfDay::new(3, 0).unwrap()));
}

#[test]
fn a_repeated_local_hour_gives_two_starts() {
    // New York falls back at 02:00 local on 2027-11-07, so 01:00 happens twice.
    let new_york = zone("America/New_York");
    let day = (
        local(&new_york, (2027, 11, 7), (0, 0)),
        local(&new_york, (2027, 11, 8), (0, 0)),
    );
    let grid = Grid::new(
        Duration::from_minutes(60),
        TimeOfDay::MIDNIGHT,
        GridZone::Event,
    )
    .unwrap();
    let starts = grid.starts(&new_york, day.0, day.1).unwrap();

    // 24 local hours, one of which happens twice.
    assert_eq!(starts.len(), 25);
    let one_am: Vec<Instant> = starts
        .iter()
        .copied()
        .filter(|start| start.zoned(&new_york).time() == TimeOfDay::new(1, 0).unwrap())
        .collect();
    assert_eq!(one_am.len(), 2, "01:00 local happens twice");
    assert_eq!(one_am[0].to_string(), "2027-11-07T05:00Z"); // still on DST
    assert_eq!(one_am[1].to_string(), "2027-11-07T06:00Z"); // standard time
    assert!(starts.windows(2).all(|pair| pair[0] <= pair[1]));
}

#[test]
fn lord_howes_half_hour_fall_back_gives_two_starts() {
    // Lord Howe moves its clocks back by 30 minutes at 02:00 on 2027-04-04, so
    // 01:30 and 01:45 each happen twice.
    let lord_howe = zone("Australia/Lord_Howe");
    let day = (
        local(&lord_howe, (2027, 4, 4), (0, 0)),
        local(&lord_howe, (2027, 4, 5), (0, 0)),
    );
    let grid = Grid::new(
        Duration::from_minutes(15),
        TimeOfDay::MIDNIGHT,
        GridZone::Event,
    )
    .unwrap();
    let starts = grid.starts(&lord_howe, day.0, day.1).unwrap();

    // 96 quarter hours, two of which happen twice.
    assert_eq!(starts.len(), 98);
    assert_eq!(
        starts
            .iter()
            .filter(|start| start.zoned(&lord_howe).time() == TimeOfDay::new(1, 30).unwrap())
            .count(),
        2
    );
    assert_eq!(
        starts,
        grid_by_minute(&lord_howe, day.0, day.1, 15, 0),
        "the half-hour shift agrees with minute-by-minute membership"
    );
}

#[test]
fn zone_periods_repeat_a_civil_date_crossing_the_date_line_eastward() {
    let auckland = zone("Pacific/Auckland");
    let honolulu = zone("Pacific/Honolulu");

    // 2027-03-09 23:00 in Auckland, then 01:00 the same date in Honolulu an
    // hour later: the participant flies east and the date runs backwards.
    let before = local(&auckland, (2027, 3, 9), (22, 0));
    let after = local(&honolulu, (2027, 3, 9), (1, 0));
    assert!(after > before);

    let schedule = ZoneSchedule::new(
        auckland.clone(),
        vec![ZonePeriod::new(honolulu, after, local(&auckland, (2027, 3, 12), (0, 0))).unwrap()],
    )
    .unwrap();

    assert_eq!(
        schedule.zone_at(before).zoned(before).date(),
        Date::new(2027, 3, 9).unwrap()
    );
    // The same date label occurs again after the switch.
    assert_eq!(
        schedule.zone_at(after).zoned(after).date(),
        Date::new(2027, 3, 9).unwrap()
    );
    // And it is the switch that brings it back: home, the date has moved on.
    assert_eq!(
        auckland.zoned(after).date(),
        Date::new(2027, 3, 10).unwrap()
    );
}

#[test]
fn zone_periods_skip_a_civil_date_crossing_the_date_line_westward() {
    // UTC-12 and UTC+14 are 26 hours apart, so a switch between them can skip a
    // whole civil date.
    let west = zone("Etc/GMT+12");
    let east = zone("Pacific/Kiritimati");
    let switch = local(&west, (2027, 3, 9), (23, 0));
    let schedule = ZoneSchedule::new(
        west.clone(),
        vec![ZonePeriod::new(east.clone(), switch, local(&west, (2027, 3, 13), (0, 0))).unwrap()],
    )
    .unwrap();

    let one_minute_before = Instant::from_minutes(switch.minutes() - 1).unwrap();
    assert_eq!(
        schedule
            .zone_at(one_minute_before)
            .zoned(one_minute_before)
            .date(),
        Date::new(2027, 3, 9).unwrap()
    );
    assert_eq!(
        schedule.zone_at(switch).zoned(switch).date(),
        Date::new(2027, 3, 11).unwrap()
    );
    // Nowhere in the trip does the skipped date appear.
    let trip_end = local(&west, (2027, 3, 12), (0, 0));
    let mut minute = switch.minutes();
    while minute < trip_end.minutes() {
        let instant = Instant::from_minutes(minute).unwrap();
        assert_ne!(
            schedule.zone_at(instant).zoned(instant).date(),
            Date::new(2027, 3, 10).unwrap()
        );
        minute += 6 * 60;
    }
}

#[test]
fn zone_period_lookup_agrees_with_the_span_membership_rule() {
    // `zone_at` binary-searches and `contains` tests one span; both encode the
    // same half-open rule, so they must answer identically at every minute —
    // including the instant two legs share, where the later leg's zone wins.
    let home = zone("Europe/London");
    let first = zone("America/Anchorage");
    let second = zone("Asia/Tokyo");
    let start = utc((2027, 3, 9), (0, 0));
    let first_leg = ZonePeriod::new(
        first.clone(),
        Instant::from_minutes(start.minutes() + 6 * 60).unwrap(),
        Instant::from_minutes(start.minutes() + 30 * 60).unwrap(),
    )
    .unwrap();
    let second_leg = ZonePeriod::new(
        second.clone(),
        first_leg.until(),
        Instant::from_minutes(start.minutes() + 70 * 60).unwrap(),
    )
    .unwrap();
    let schedule =
        ZoneSchedule::new(home.clone(), vec![second_leg.clone(), first_leg.clone()]).unwrap();

    for minute in start.minutes()..start.minutes() + 80 * 60 {
        let instant = Instant::from_minutes(minute).unwrap();
        let expected = if first_leg.contains(instant) {
            &first
        } else if second_leg.contains(instant) {
            &second
        } else {
            &home
        };
        assert_eq!(
            schedule.zone_at(instant),
            expected,
            "the lookup and the span membership rule disagree at {instant}"
        );
    }
}

#[test]
fn adjacent_trip_legs_join_exactly() {
    // Two legs with no gap between them: the instant they share belongs to the
    // second leg, and the instant the trip ends belongs to the home zone.
    let anchorage = zone("America/Anchorage");
    let london = zone("Europe/London");
    let home = zone("Asia/Tokyo");
    let trip_start = local(&anchorage, (2027, 3, 7), (22, 0));
    let next_leg_start = local(&anchorage, (2027, 3, 11), (15, 0));
    let trip_end = local(&london, (2027, 3, 15), (12, 0));
    let schedule = ZoneSchedule::new(
        home.clone(),
        vec![
            ZonePeriod::new(anchorage.clone(), trip_start, next_leg_start).unwrap(),
            ZonePeriod::new(london.clone(), next_leg_start, trip_end).unwrap(),
        ],
    )
    .unwrap();

    assert_eq!(schedule.zone_at(trip_start).name(), "America/Anchorage");
    assert_eq!(
        schedule
            .zone_at(Instant::from_minutes(next_leg_start.minutes() - 1).unwrap())
            .name(),
        "America/Anchorage"
    );
    assert_eq!(
        schedule.zone_at(next_leg_start).name(),
        "Europe/London",
        "the instant two adjacent legs share belongs to the later leg"
    );
    assert_eq!(
        schedule
            .zone_at(Instant::from_minutes(trip_end.minutes() - 1).unwrap())
            .name(),
        "Europe/London"
    );
    assert_eq!(schedule.zone_at(trip_end).name(), "Asia/Tokyo");
}

#[test]
fn calendar_day_horizons_use_the_event_zone() {
    let auckland = zone("Pacific/Auckland");
    // 09:00 on 10 March in Auckland is still 9 March in UTC.
    let now = local(&auckland, (2027, 3, 10), (9, 0));
    assert_eq!(
        Zone::utc().zoned(now).date(),
        Date::new(2027, 3, 9).unwrap()
    );

    let horizon = Horizon::Days(0);
    let start = utc((2027, 3, 10), (0, 30)); // 13:30 on 10 March in Auckland
    assert!(
        allows(&horizon, start, now, &auckland),
        "the start is still today in the event zone"
    );
    assert!(
        !allows(&horizon, start, now, &Zone::utc()),
        "the same start would be past today if the event zone were ignored"
    );
}

#[test]
fn working_day_horizons_count_in_the_calendar_zone() {
    let auckland = zone("Pacific/Auckland");
    let calendar =
        WorkingCalendar::new(auckland.clone(), WorkingDays::weekdays(), Vec::new()).unwrap();
    let horizon = Horizon::WorkingDays { count: 1, calendar };
    // Friday 12 March, 09:00 in Auckland.
    let now = local(&auckland, (2027, 3, 12), (9, 0));

    // Saturday is inside the horizon, because the one counted working day is
    // Monday the 15th.
    assert!(allows(
        &horizon,
        local(&auckland, (2027, 3, 13), (12, 0)),
        now,
        &Zone::utc()
    ));
    assert!(allows(
        &horizon,
        local(&auckland, (2027, 3, 15), (23, 0)),
        now,
        &Zone::utc()
    ));
    // Tuesday is past it.
    assert!(!allows(
        &horizon,
        local(&auckland, (2027, 3, 16), (0, 0)),
        now,
        &Zone::utc()
    ));
}

#[test]
fn working_day_horizons_hold_across_a_skipped_midnight() {
    // Santiago springs forward from midnight to 01:00 on Sunday 2027-09-05, so
    // that day has no local midnight at all.
    let santiago = zone("America/Santiago");
    assert_eq!(
        santiago
            .resolve_local(Date::new(2027, 9, 5).unwrap(), TimeOfDay::MIDNIGHT)
            .unwrap(),
        LocalResolution::Skipped
    );

    let calendar =
        WorkingCalendar::new(santiago.clone(), WorkingDays::weekdays(), Vec::new()).unwrap();
    let horizon = Horizon::WorkingDays { count: 1, calendar };
    // Friday 3 September, 20:00 in Santiago.
    let now = local(&santiago, (2027, 9, 3), (20, 0));
    // One working day after Friday is Monday the 6th.
    assert!(allows(
        &horizon,
        local(&santiago, (2027, 9, 6), (23, 0)),
        now,
        &Zone::utc()
    ));
    assert!(!allows(
        &horizon,
        local(&santiago, (2027, 9, 7), (0, 0)),
        now,
        &Zone::utc()
    ));
}

#[test]
fn a_date_horizon_allows_up_to_the_last_minute_of_its_last_date() {
    // No transition sits near this boundary, so the bound resolves to a single
    // instant cutoff: the last allowed minute is the one before the next date
    // begins, and the first instant of that date is out.
    let new_york = zone("America/New_York");
    let latest = Date::new(2027, 6, 20).unwrap();
    let horizon = Horizon::Date(latest);
    let now = local(&new_york, (2027, 6, 1), (12, 0));
    let bound = horizon.bound(now, &new_york).unwrap();

    let day_begins = new_york.day_start(Date::new(2027, 6, 21).unwrap()).unwrap();
    let last_allowed = Instant::from_minutes(day_begins.minutes() - 1).unwrap();
    assert!(
        bound.allows(last_allowed),
        "the final minute of {latest} in New York is inside"
    );
    assert!(
        !bound.allows(day_begins),
        "the first minute of the next date in New York is past the horizon"
    );
    // Midnight UTC on the 21st is still the evening of the 20th in New York.
    assert!(bound.allows(utc((2027, 6, 21), (0, 0))));
}

#[test]
fn a_date_horizon_holds_across_a_backward_transition_repeating_its_last_date() {
    // Newfoundland ended daylight saving at 00:01 local until 2010, so the
    // 2009-11-01 fall-back repeats the evening of 31 October: an hour of
    // instants after local midnight reads the 31st again. A date horizon whose
    // last allowed date that is must keep allowing starts through the repeat,
    // which no single instant cutoff can express — the bound has to read local
    // dates there. Compare against the minute-by-minute rule the fast path
    // stands in for.
    let st_johns = zone("America/St_Johns");
    let latest = Date::new(2009, 10, 31).unwrap();
    let horizon = Horizon::Date(latest);
    let now = local(&st_johns, (2009, 10, 1), (12, 0));
    let bound = horizon.bound(now, &st_johns).unwrap();

    let from = utc((2009, 10, 31), (20, 0)).minutes();
    let until = utc((2009, 11, 1), (6, 0)).minutes();
    let mut repeated = 0_usize;
    for minute in from..until {
        let start = Instant::from_minutes(minute).unwrap();
        let expected = start.zoned(&st_johns).date() <= latest;
        assert_eq!(
            bound.allows(start),
            expected,
            "{start} reads {} in St John's against a horizon of {latest}",
            start.zoned(&st_johns).date()
        );
        if expected && start.zoned(&Zone::utc()).date() > latest {
            repeated += 1;
        }
    }
    assert!(
        repeated > 0,
        "the fall-back no longer repeats {latest} in St John's, so this test has \
         nothing left to prove"
    );
}

#[test]
fn working_day_horizons_skip_holidays_and_leap_days() {
    let utc_zone = Zone::utc();
    let calendar = WorkingCalendar::new(
        utc_zone.clone(),
        WorkingDays::weekdays(),
        vec![Date::new(2028, 2, 29).unwrap()],
    )
    .unwrap();
    let horizon = Horizon::WorkingDays { count: 2, calendar };
    // Friday 25 February 2028: Monday the 28th is the first working day, and
    // the leap day is a holiday, so the second is 1 March.
    let now = utc((2028, 2, 25), (12, 0));
    assert!(allows(
        &horizon,
        utc((2028, 3, 1), (23, 59)),
        now,
        &utc_zone
    ));
    assert!(!allows(&horizon, utc((2028, 3, 2), (0, 0)), now, &utc_zone));
}
