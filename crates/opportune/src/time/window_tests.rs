//! Hand-computed window fixtures across the zone matrix.
//!
//! Each fixture states the half-open UTC spans a validated window must produce,
//! so a daylight saving transition, a leap day or a trip across the date line
//! is pinned by a value that did not come from the builder itself. The
//! property tests compare the builder with minute-by-minute membership on
//! generated input; these cover the corners where a shared misreading would
//! hide.

#![cfg(feature = "bundled-tzdb")]

use crate::intervals::Span;
use crate::time::{
    Date, DateKey, DateRange, Instant, LocalResolution, RuleZone, TimeOfDay, TimeZoneDatabase,
    WallEnd, WallRange, Weekday, Weekdays, WeeklyKey, WindowSpec, Zone, ZoneName, ZonePeriod,
    ZoneSchedule,
};

fn database() -> TimeZoneDatabase {
    TimeZoneDatabase::bundled().expect("the bundled database is present")
}

fn zone(name: &str) -> Zone {
    database()
        .zone(name)
        .unwrap_or_else(|err| panic!("{name} is in the bundled database: {err}"))
}

fn time(hour: u8, minute: u8) -> TimeOfDay {
    TimeOfDay::new(hour, minute).unwrap()
}

/// The instant a local date-time exists at, failing on a skipped or repeated one.
fn local(zone: &Zone, date: (i16, i8, i8), at: (u8, u8)) -> Instant {
    let date = Date::new(date.0, date.1, date.2).unwrap();
    match zone.resolve_local(date, time(at.0, at.1)).unwrap() {
        LocalResolution::Unique(instant) => instant,
        other => panic!(
            "{date} {}:{} in {} is not a single instant: {other:?}",
            at.0,
            at.1,
            zone.name()
        ),
    }
}

fn utc(date: (i16, i8, i8), at: (u8, u8)) -> Instant {
    local(&Zone::utc(), date, at)
}

fn range(start: (u8, u8), end: (u8, u8)) -> WallRange {
    WallRange::new(time(start.0, start.1), WallEnd::Time(time(end.0, end.1))).unwrap()
}

/// A rule zone naming `zone`.
fn named(zone: &Zone) -> RuleZone {
    RuleZone::Named(ZoneName::new(zone.name()).unwrap())
}

fn window(zone: RuleZone, keys: Vec<(Weekdays, Vec<WallRange>)>) -> WindowSpec {
    WindowSpec::new(
        zone,
        keys.into_iter()
            .map(|(days, hours)| WeeklyKey::new(days, hours).unwrap())
            .collect(),
        Vec::new(),
    )
    .unwrap()
}

/// The built spans, as minute pairs, in the event zone `event`.
fn built(
    spec: &WindowSpec,
    zones: &ZoneSchedule,
    from: Instant,
    until: Instant,
) -> Vec<(i64, i64)> {
    spec.intervals(zones, from, until)
        .unwrap()
        .spans()
        .iter()
        .map(|span: &Span| (span.start(), span.end()))
        .collect()
}

/// A named-zone window's spans, read at a fixed zone.
fn built_named(
    spec: &WindowSpec,
    at_zone: &Zone,
    from: Instant,
    until: Instant,
) -> Vec<(i64, i64)> {
    built(spec, &ZoneSchedule::fixed(at_zone.clone()), from, until)
}

fn pair(start: Instant, end: Instant) -> (i64, i64) {
    (start.minutes(), end.minutes())
}

#[test]
fn a_fall_back_window_starting_inside_the_repeated_hour_splits_in_two() {
    // New York falls back at 02:00 local on Sunday 2027-11-07, so 01:30 happens
    // twice: once on daylight time and once on standard time.
    let new_york = zone("America/New_York");
    let spec = window(
        named(&new_york),
        vec![(
            Weekdays::single(Weekday::Sunday),
            vec![range((1, 30), (3, 30))],
        )],
    );
    let spans = built_named(
        &spec,
        &new_york,
        utc((2027, 11, 7), (0, 0)),
        utc((2027, 11, 8), (0, 0)),
    );
    // The first 01:30 (EDT) runs to the transition at 06:00Z. The repeated
    // 01:00-01:30 on standard time is outside, and the second 01:30 runs to
    // 03:30 (EST).
    assert_eq!(
        spans,
        [
            pair(utc((2027, 11, 7), (5, 30)), utc((2027, 11, 7), (6, 0))),
            pair(utc((2027, 11, 7), (6, 30)), utc((2027, 11, 7), (8, 30))),
        ]
    );

    let zones = ZoneSchedule::fixed(new_york.clone());
    // The excluded stretch is exactly the repeated 01:00-01:30.
    assert!(!spec.contains(utc((2027, 11, 7), (6, 15)), &zones));
    assert_eq!(
        new_york.zoned(utc((2027, 11, 7), (6, 15))).time(),
        time(1, 15)
    );
}

#[test]
fn a_spring_forward_window_spans_the_skipped_hour_without_a_hole() {
    // New York springs forward from 02:00 to 03:00 on Sunday 2027-03-14, so
    // 01:00 to 04:00 local is a single continuous stretch of instant time.
    let new_york = zone("America/New_York");
    let spec = window(
        named(&new_york),
        vec![(
            Weekdays::single(Weekday::Sunday),
            vec![range((1, 0), (4, 0))],
        )],
    );
    assert_eq!(
        built_named(
            &spec,
            &new_york,
            utc((2027, 3, 14), (0, 0)),
            utc((2027, 3, 15), (0, 0)),
        ),
        [pair(utc((2027, 3, 14), (6, 0)), utc((2027, 3, 14), (8, 0)))]
    );

    let zones = ZoneSchedule::fixed(new_york.clone());
    // 01:30 EST and 03:30 EDT are both inside; nothing lands on 02:30.
    assert!(spec.contains(utc((2027, 3, 14), (6, 30)), &zones));
    assert!(spec.contains(utc((2027, 3, 14), (7, 30)), &zones));
    assert_eq!(
        new_york.zoned(utc((2027, 3, 14), (7, 30))).time(),
        time(3, 30)
    );
}

#[test]
fn lord_howes_half_hour_fall_back_splits_a_window_by_thirty_minutes() {
    // Lord Howe moves its clocks back from 02:00 to 01:30 on Sunday 2027-04-04,
    // so the repeated stretch is only half an hour.
    let lord_howe = zone("Australia/Lord_Howe");
    let spec = window(
        named(&lord_howe),
        vec![(
            Weekdays::single(Weekday::Sunday),
            vec![range((1, 45), (3, 0))],
        )],
    );
    // Local 02:00 on 4 April is 15:00Z on 3 April, so the fixture is read a
    // day earlier in UTC.
    let spans = built_named(
        &spec,
        &lord_howe,
        utc((2027, 4, 3), (0, 0)),
        utc((2027, 4, 4), (0, 0)),
    );
    assert_eq!(
        spans,
        [
            pair(utc((2027, 4, 3), (14, 45)), utc((2027, 4, 3), (15, 0))),
            pair(utc((2027, 4, 3), (15, 15)), utc((2027, 4, 3), (16, 30))),
        ]
    );
}

#[test]
fn a_skipped_local_midnight_starts_the_date_at_the_transition() {
    // Santiago springs forward from midnight to 01:00 on Sunday 2027-09-05, so
    // that date has no local midnight at all.
    let santiago = zone("America/Santiago");
    assert_eq!(
        santiago
            .resolve_local(Date::new(2027, 9, 5).unwrap(), TimeOfDay::MIDNIGHT)
            .unwrap(),
        LocalResolution::Skipped
    );
    let spec = window(
        named(&santiago),
        vec![(
            Weekdays::single(Weekday::Sunday),
            vec![range((0, 0), (2, 0))],
        )],
    );
    assert_eq!(
        built_named(
            &spec,
            &santiago,
            utc((2027, 9, 5), (0, 0)),
            utc((2027, 9, 6), (0, 0)),
        ),
        [pair(utc((2027, 9, 5), (4, 0)), utc((2027, 9, 5), (5, 0)))]
    );
}

#[test]
fn a_leap_day_override_covers_the_date_inside_its_range() {
    let date = DateKey::new(
        DateRange::new(
            Date::new(2028, 2, 28).unwrap(),
            Date::new(2028, 3, 1).unwrap(),
        )
        .unwrap(),
        vec![range((10, 0), (12, 0))],
    )
    .unwrap();
    let spec = WindowSpec::new(named(&Zone::utc()), Vec::new(), vec![date]).unwrap();
    assert_eq!(
        built_named(
            &spec,
            &Zone::utc(),
            utc((2028, 2, 27), (0, 0)),
            utc((2028, 3, 2), (0, 0)),
        ),
        [
            pair(utc((2028, 2, 28), (10, 0)), utc((2028, 2, 28), (12, 0))),
            pair(utc((2028, 2, 29), (10, 0)), utc((2028, 2, 29), (12, 0))),
            pair(utc((2028, 3, 1), (10, 0)), utc((2028, 3, 1), (12, 0))),
        ]
    );
    assert!(spec.contains(
        utc((2028, 2, 29), (11, 0)),
        &ZoneSchedule::fixed(Zone::utc()),
    ));
}

#[test]
fn a_mid_day_zone_period_switch_reads_the_window_in_each_zones_own_clock() {
    // A participant whose home is Tokyo flies to Los Angeles from 16:00Z to
    // 00:00Z, when Tokyo is late on 9 March and Los Angeles is early on 9 March
    // again. A daily 09:00-10:00 window follows each zone's local clock.
    let tokyo = zone("Asia/Tokyo");
    let los_angeles = zone("America/Los_Angeles");
    let schedule = ZoneSchedule::new(
        tokyo.clone(),
        vec![
            ZonePeriod::new(
                los_angeles.clone(),
                utc((2027, 3, 9), (16, 0)),
                utc((2027, 3, 10), (0, 0)),
            )
            .unwrap(),
        ],
    )
    .unwrap();
    let spec = window(
        RuleZone::Participant,
        vec![(
            Weekdays::range(Weekday::Monday, Weekday::Sunday),
            vec![range((9, 0), (10, 0))],
        )],
    );
    assert_eq!(
        built(
            &spec,
            &schedule,
            utc((2027, 3, 9), (0, 0)),
            utc((2027, 3, 10), (2, 0)),
        ),
        [
            // Tokyo 09:00-10:00 on 9 March.
            pair(utc((2027, 3, 9), (0, 0)), utc((2027, 3, 9), (1, 0))),
            // Los Angeles 09:00-10:00 on 9 March, inside the trip.
            pair(utc((2027, 3, 9), (17, 0)), utc((2027, 3, 9), (18, 0))),
            // Tokyo 09:00-10:00 on 10 March, after the trip ended.
            pair(utc((2027, 3, 10), (0, 0)), utc((2027, 3, 10), (1, 0))),
        ]
    );
}

#[test]
fn a_period_crossing_the_date_line_repeats_a_civil_date() {
    // Auckland 23:00 on 9 March is Honolulu's midnight on the same date label,
    // so the date runs backward and 9 March happens twice.
    let auckland = zone("Pacific/Auckland");
    let honolulu = zone("Pacific/Honolulu");
    let schedule = ZoneSchedule::new(
        auckland.clone(),
        vec![
            ZonePeriod::new(
                honolulu.clone(),
                local(&auckland, (2027, 3, 9), (23, 0)),
                local(&auckland, (2027, 3, 11), (23, 0)),
            )
            .unwrap(),
        ],
    )
    .unwrap();
    let spec = window(
        RuleZone::Participant,
        vec![(
            Weekdays::list([Weekday::Monday, Weekday::Tuesday, Weekday::Wednesday]),
            vec![range((9, 0), (10, 0))],
        )],
    );
    assert_eq!(
        built(
            &spec,
            &schedule,
            utc((2027, 3, 8), (0, 0)),
            utc((2027, 3, 11), (12, 0)),
        ),
        [
            // Auckland 09:00-10:00 on 9 March.
            pair(utc((2027, 3, 8), (20, 0)), utc((2027, 3, 8), (21, 0))),
            // Honolulu 09:00-10:00 on the same label, now repeated.
            pair(utc((2027, 3, 9), (19, 0)), utc((2027, 3, 9), (20, 0))),
            // Honolulu 09:00-10:00 on 10 March.
            pair(utc((2027, 3, 10), (19, 0)), utc((2027, 3, 10), (20, 0))),
        ]
    );
}

#[test]
fn a_period_crossing_the_date_line_skips_a_civil_date() {
    // UTC-12 and UTC+14 are 26 hours apart, so switching between them leaps over
    // a whole date label; a daily window must produce nothing on it.
    let west = zone("Etc/GMT+12");
    let kiritimati = zone("Pacific/Kiritimati");
    let switch = local(&west, (2027, 3, 9), (23, 0));
    let schedule = ZoneSchedule::new(
        west.clone(),
        vec![
            ZonePeriod::new(
                kiritimati.clone(),
                switch,
                local(&west, (2027, 3, 12), (0, 0)),
            )
            .unwrap(),
        ],
    )
    .unwrap();
    let spec = window(
        RuleZone::Participant,
        vec![(
            Weekdays::list([
                Weekday::Tuesday,
                Weekday::Wednesday,
                Weekday::Thursday,
                Weekday::Friday,
            ]),
            vec![range((9, 0), (10, 0))],
        )],
    );
    assert_eq!(
        built(
            &spec,
            &schedule,
            utc((2027, 3, 9), (12, 0)),
            utc((2027, 3, 11), (12, 0)),
        ),
        [
            // UTC-12 on 9 March, before the flight.
            pair(utc((2027, 3, 9), (21, 0)), utc((2027, 3, 9), (22, 0))),
            // Kiritimati on 11 March, the date after the one that was skipped.
            pair(utc((2027, 3, 10), (19, 0)), utc((2027, 3, 10), (20, 0))),
        ]
    );
    // The skipped label is 10 March in neither zone's clock.
    let zones = &schedule;
    let mut minute = switch.minutes();
    while minute < utc((2027, 3, 10), (19, 0)).minutes() {
        let instant = Instant::from_minutes(minute).unwrap();
        assert_ne!(
            zones.zone_at(instant).zoned(instant).date(),
            Date::new(2027, 3, 10).unwrap()
        );
        minute += 30;
    }
}

#[test]
fn a_window_range_ending_at_24_00_stops_at_midnight() {
    let spec = window(
        named(&Zone::utc()),
        vec![(
            Weekdays::single(Weekday::Friday),
            vec![WallRange::new(time(20, 0), WallEnd::Midnight).unwrap()],
        )],
    );
    assert_eq!(
        built_named(
            &spec,
            &Zone::utc(),
            utc((2027, 3, 12), (0, 0)),
            utc((2027, 3, 13), (0, 0)),
        ),
        [pair(
            utc((2027, 3, 12), (20, 0)),
            utc((2027, 3, 13), (0, 0))
        )]
    );
    let zones = ZoneSchedule::fixed(Zone::utc());
    assert!(!spec.contains(utc((2027, 3, 13), (0, 0)), &zones));
}

/// A zone period that begins on a daylight saving day, with the window's hours
/// straddling the transition: the participant reads part of the day in their home
/// clock and the rest in the new zone, and the skipped hour makes the new zone's
/// 01:00-04:00 one continuous stretch of instant time.
#[test]
fn a_zone_period_starting_on_a_dst_day_reads_each_zone_at_its_own_offset() {
    let tokyo = zone("Asia/Tokyo");
    let new_york = zone("America/New_York");
    // New York springs forward at 02:00 local on Sunday 2027-03-14, which is
    // 07:00Z. The trip begins at 05:00Z that day, before the change.
    let schedule = ZoneSchedule::new(
        tokyo,
        vec![
            ZonePeriod::new(
                new_york.clone(),
                utc((2027, 3, 14), (5, 0)),
                utc((2027, 3, 15), (5, 0)),
            )
            .unwrap(),
        ],
    )
    .unwrap();
    let spec = window(
        RuleZone::Participant,
        vec![(
            Weekdays::range(Weekday::Monday, Weekday::Sunday),
            vec![range((1, 0), (4, 0))],
        )],
    );
    assert_eq!(
        built(
            &spec,
            &schedule,
            utc((2027, 3, 13), (0, 0)),
            utc((2027, 3, 15), (0, 0)),
        ),
        [
            // Tokyo 01:00-04:00 on 14 March, before the trip starts.
            pair(utc((2027, 3, 13), (16, 0)), utc((2027, 3, 13), (19, 0))),
            // New York 01:00-04:00 on 14 March: 01:00-02:00 EST runs straight
            // into 03:00-04:00 EDT, because 02:00-03:00 never happens.
            pair(utc((2027, 3, 14), (6, 0)), utc((2027, 3, 14), (8, 0))),
        ]
    );
}
