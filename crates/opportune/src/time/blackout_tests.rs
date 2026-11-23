//! Hand-computed blackout fixtures across the zone matrix.
//!
//! Blackouts are local date-time spans, so a daylight saving transition can
//! move a boundary, repeat it, or skip it. Each fixture pins the exact
//! half-open UTC intervals a validated blackout must produce.

#![cfg(feature = "bundled-tzdb")]

use crate::intervals::Span;
use crate::time::{
    BlackoutSpan, BlackoutSpec, Date, Instant, LocalDateTime, LocalResolution, RuleZone, TimeOfDay,
    TimeZoneDatabase, Zone, ZoneName, ZonePeriod, ZoneSchedule,
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

fn at(y: i16, m: i8, d: i8, hour: u8, minute: u8) -> LocalDateTime {
    LocalDateTime::new(Date::new(y, m, d).unwrap(), time(hour, minute))
}

fn span(start: (i16, i8, i8, u8, u8), end: (i16, i8, i8, u8, u8)) -> BlackoutSpan {
    BlackoutSpan::new(
        at(start.0, start.1, start.2, start.3, start.4),
        at(end.0, end.1, end.2, end.3, end.4),
    )
    .unwrap()
}

/// A rule zone naming `zone`.
fn named(zone: &Zone) -> RuleZone {
    RuleZone::Named(ZoneName::new(zone.name()).unwrap())
}

fn built(
    spec: &BlackoutSpec,
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

fn pair(start: Instant, end: Instant) -> (i64, i64) {
    (start.minutes(), end.minutes())
}

#[test]
fn a_boundary_inside_the_repeated_hour_blackens_both_occurrences() {
    // New York falls back on 2027-11-07, so 01:30 to 01:45 happens twice.
    let new_york = zone("America/New_York");
    let spec = BlackoutSpec::new(
        named(&new_york),
        vec![span((2027, 11, 7, 1, 30), (2027, 11, 7, 1, 45))],
    );
    let zones = ZoneSchedule::fixed(new_york);
    assert_eq!(
        built(
            &spec,
            &zones,
            utc((2027, 11, 7), (0, 0)),
            utc((2027, 11, 8), (0, 0)),
        ),
        [
            pair(utc((2027, 11, 7), (5, 30)), utc((2027, 11, 7), (5, 45))),
            pair(utc((2027, 11, 7), (6, 30)), utc((2027, 11, 7), (6, 45))),
        ]
    );
}

#[test]
fn a_blackout_spanning_the_skipped_hour_is_one_interval() {
    // New York springs forward on 2027-03-14: 02:00 to 03:00 never happens, so
    // 01:30 to 03:30 local is one continuous stretch of instant time.
    let new_york = zone("America/New_York");
    let spec = BlackoutSpec::new(
        named(&new_york),
        vec![span((2027, 3, 14, 1, 30), (2027, 3, 14, 3, 30))],
    );
    assert_eq!(
        built(
            &spec,
            &ZoneSchedule::fixed(new_york.clone()),
            utc((2027, 3, 14), (0, 0)),
            utc((2027, 3, 15), (0, 0)),
        ),
        // 01:30 EST is 06:30Z and 03:30 EDT is 07:30Z.
        [pair(
            utc((2027, 3, 14), (6, 30)),
            utc((2027, 3, 14), (7, 30))
        )]
    );
}

#[test]
fn a_blackout_starting_inside_the_skipped_hour_starts_at_the_transition() {
    // 02:30 local does not exist on 2027-03-14 in New York; the blackout begins
    // when the clock jumps to 03:00 and runs to 04:00.
    let new_york = zone("America/New_York");
    let spec = BlackoutSpec::new(
        named(&new_york),
        vec![span((2027, 3, 14, 2, 30), (2027, 3, 14, 4, 0))],
    );
    assert_eq!(
        built(
            &spec,
            &ZoneSchedule::fixed(new_york.clone()),
            utc((2027, 3, 14), (0, 0)),
            utc((2027, 3, 15), (0, 0)),
        ),
        [pair(utc((2027, 3, 14), (7, 0)), utc((2027, 3, 14), (8, 0)))]
    );
}

#[test]
fn lord_howes_half_hour_fall_back_splits_a_blackout_boundary() {
    // Lord Howe moves back at local 02:00 on 2027-04-04 (15:00Z on 3 April), so
    // 01:30 to 01:45 happens once before the change and once after it.
    let lord_howe = zone("Australia/Lord_Howe");
    let spec = BlackoutSpec::new(
        named(&lord_howe),
        vec![span((2027, 4, 4, 1, 30), (2027, 4, 4, 1, 45))],
    );
    assert_eq!(
        built(
            &spec,
            &ZoneSchedule::fixed(lord_howe.clone()),
            utc((2027, 4, 3), (0, 0)),
            utc((2027, 4, 4), (0, 0)),
        ),
        [
            pair(utc((2027, 4, 3), (14, 30)), utc((2027, 4, 3), (14, 45))),
            pair(utc((2027, 4, 3), (15, 0)), utc((2027, 4, 3), (15, 15))),
        ]
    );
}

#[test]
fn a_multi_day_blackout_in_a_named_zone_has_its_own_boundaries() {
    let london = zone("Europe/London");
    let spec = BlackoutSpec::new(
        named(&london),
        vec![span((2026, 12, 24, 0, 0), (2027, 1, 2, 0, 0))],
    );
    let start = local(&london, (2026, 12, 24), (0, 0));
    let end = local(&london, (2027, 1, 2), (0, 0));
    assert_eq!(
        built(
            &spec,
            &ZoneSchedule::fixed(london.clone()),
            utc((2026, 12, 20), (0, 0)),
            utc((2027, 1, 5), (0, 0)),
        ),
        [pair(start, end)]
    );
}

#[test]
fn a_blackout_span_blacks_out_two_windows_when_the_participants_zone_moves() {
    // A participant whose home is Tokyo is in Los Angeles from 16:00Z to 00:00Z
    // on 9 March, so the local span 09:00-10:00 on 9 March names two different
    // UTC stretches: Tokyo's in the morning and Los Angeles' in the afternoon.
    let tokyo = zone("Asia/Tokyo");
    let los_angeles = zone("America/Los_Angeles");
    let schedule = ZoneSchedule::new(
        tokyo,
        vec![
            ZonePeriod::new(
                los_angeles,
                utc((2027, 3, 9), (16, 0)),
                utc((2027, 3, 10), (0, 0)),
            )
            .unwrap(),
        ],
    )
    .unwrap();
    let spec = BlackoutSpec::new(
        RuleZone::Participant,
        vec![span((2027, 3, 9, 9, 0), (2027, 3, 9, 10, 0))],
    );
    assert_eq!(
        built(
            &spec,
            &schedule,
            utc((2027, 3, 9), (0, 0)),
            utc((2027, 3, 10), (2, 0)),
        ),
        [
            pair(utc((2027, 3, 9), (0, 0)), utc((2027, 3, 9), (1, 0))),
            pair(utc((2027, 3, 9), (17, 0)), utc((2027, 3, 9), (18, 0))),
        ]
    );
}

/// A blackout span may name any civil date, and a span that runs from before
/// the supported range to after it is a legitimate way to say "closed
/// throughout". Its endpoints are not breakpoints of a slice that never reads
/// their dates, so building the slice must not try to resolve them: 9999-12-31
/// 23:59 at UTC-12 lies past the last instant any timestamp can hold.
#[test]
fn an_extreme_span_boundary_outside_the_slice_is_not_resolved() {
    let west = zone("Etc/GMT+12"); // UTC-12, where the span's end overflows
    let spec = BlackoutSpec::new(
        named(&west),
        vec![
            BlackoutSpan::new(
                LocalDateTime::new(Date::new(1974, 1, 1).unwrap(), TimeOfDay::MIDNIGHT),
                LocalDateTime::new(Date::new(9999, 12, 31).unwrap(), time(23, 59)),
            )
            .unwrap(),
        ],
    );
    let from = utc((2027, 3, 9), (0, 0));
    let until = utc((2027, 3, 10), (0, 0));
    assert_eq!(
        built(&spec, &ZoneSchedule::fixed(west), from, until),
        [pair(from, until)],
        "the whole slice is blacked out"
    );
}
