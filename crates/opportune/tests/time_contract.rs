//! Attacks on the `opportune::time` contract at the edges of the supported
//! range and at the seams where an input is normalised.
//!
//! Every test states the assumption it attacks: the supported range is
//! inclusive at both ends, a local time that has no instant is a *mapping*
//! failure, and a version label is normalised the same way whether it arrives
//! in a file or from the caller.

use opportune::time::{
    Date, Duration, Grid, GridZone, Horizon, Instant, LocalResolution, TimeOfDay, TimeZoneDatabase,
    Weekday, Weekdays, WorkingCalendar, WorkingDays, Zone, ZoneName,
};

/// The instant of a UTC civil date-time, which always exists.
fn utc(y: i16, m: i8, d: i8, hour: u8, minute: u8) -> Instant {
    let date = Date::new(y, m, d).unwrap();
    let time = TimeOfDay::new(hour, minute).unwrap();
    let LocalResolution::Unique(instant) = Zone::utc().resolve_local(date, time).unwrap() else {
        panic!("UTC has no transitions");
    };
    instant
}

/// An hourly grid anchored at local midnight.
fn hourly() -> Grid {
    Grid::new(
        Duration::from_minutes(60),
        TimeOfDay::MIDNIGHT,
        GridZone::Event,
    )
    .unwrap()
}

/// `Instant::MIN` and `Instant::MAX` are supported instants, so a search window
/// may begin or end on them. `Grid::starts` walks the local dates around the
/// window and resolves *every* grid point on them, including the padded day
/// either side; at the edges of the supported range those padded points have no
/// representable instant. A point outside the window is not a start, so the
/// padded day must contribute nothing rather than fail the whole call.
#[test]
fn a_grid_window_that_touches_the_supported_range_gives_its_starts() {
    let zone = Zone::utc();

    // The last day a request may name: 2099-12-31T00:00Z..2100-01-01T00:00Z.
    let last_day = hourly().starts(&zone, utc(2099, 12, 31, 0, 0), Instant::MAX);
    let last_day = last_day.expect("a window inside the supported range gives its starts");
    assert_eq!(last_day.len(), 24, "{last_day:?}");
    assert_eq!(last_day[0], utc(2099, 12, 31, 0, 0));

    // The first day a request may name: 1975-01-01T00:00Z..1975-01-02T00:00Z.
    let first_day = hourly().starts(&zone, Instant::MIN, utc(1975, 1, 2, 0, 0));
    let first_day = first_day.expect("a window inside the supported range gives its starts");
    assert_eq!(first_day.len(), 24, "{first_day:?}");
    assert_eq!(first_day[0], Instant::MIN);
}

/// The skip that makes the edge work must drop *only* the padded points. A
/// zone west of UTC puts two local dates before a window that starts at
/// `Instant::MIN`, and the same two after one that ends at `Instant::MAX`; the
/// window's own hours must all survive, not one fewer.
#[cfg(feature = "bundled-tzdb")]
#[test]
fn a_grid_window_at_either_end_keeps_every_hour_in_a_zone_behind_utc() {
    let db = TimeZoneDatabase::bundled().unwrap();
    let west = db.zone("Etc/GMT+12").unwrap(); // UTC-12

    for (from, until) in [
        (Instant::MIN, shifted(Instant::MIN, 24 * 60)),
        (shifted(Instant::MAX, -24 * 60), Instant::MAX),
    ] {
        let starts = hourly()
            .starts(&west, from, until)
            .expect("a window inside the supported range gives its starts");
        let expected: Vec<Instant> = (0..24).map(|hour| shifted(from, hour * 60)).collect();
        assert_eq!(starts, expected, "every hour of {from}..{until} in UTC-12");
    }
}

/// `now` rounds up and block endpoints round outward, and both must hold at the
/// exact seconds either side of the range's endpoints: rounding the last second
/// of the range lands on `Instant::MAX`, rounding the first lands on
/// `Instant::MIN`, and a value past the range is rejected rather than clamped.
#[test]
fn rounding_at_the_range_endpoints_moves_inward_not_out_of_range() {
    let last_second = Instant::MAX.unix_seconds();
    let first_second = Instant::MIN.unix_seconds();

    assert_eq!(
        Instant::ceil_to_minute(last_second - 1).unwrap(),
        Instant::MAX,
        "a block ending at 2099-12-31T23:59:59Z still ends at the last instant"
    );
    assert_eq!(
        Instant::floor_to_minute(first_second + 59).unwrap(),
        Instant::MIN
    );
    assert_eq!(
        Instant::from_unix_seconds(last_second - 1),
        Err(opportune::time::TimeError::InstantNotAligned {
            seconds: last_second - 1
        })
    );
    for seconds in [first_second - 1, last_second + 1] {
        assert_eq!(
            Instant::ceil_to_minute(seconds).unwrap_err().code(),
            "instant_out_of_range",
            "{seconds} is outside every direction of rounding"
        );
    }
}

/// The instant `minutes` minutes after `at`, walking backwards for a negative
/// count.
#[cfg(feature = "bundled-tzdb")]
fn shifted(at: Instant, minutes: i64) -> Instant {
    Instant::from_minutes(at.minutes() + minutes).unwrap()
}

/// The same defect at the load a real search window has. In a UTC+14 zone the
/// local days a window ending on 2099-12-31 touches run past `Instant::MAX` in
/// UTC terms, so the padded grid points of that window are unresolvable even
/// though every instant the window itself names is supported.
#[cfg(feature = "bundled-tzdb")]
#[test]
fn a_grid_window_near_the_end_of_the_range_works_in_a_zone_ahead_of_utc() {
    let db = TimeZoneDatabase::bundled().unwrap();
    let line_islands = db.zone("Pacific/Kiritimati").unwrap(); // UTC+14
    let from = utc(2099, 12, 30, 0, 0);
    let until = utc(2099, 12, 31, 23, 0);
    let starts = hourly()
        .starts(&line_islands, from, until)
        .expect("a window inside the supported range gives its starts");
    assert!(!starts.is_empty(), "the window holds local hours");
    assert!(
        starts.iter().all(|start| from <= *start && *start < until),
        "every start lies inside the window"
    );
    assert!(
        starts
            .iter()
            .all(|start| start.zoned(&line_islands).time().minute() == 0),
        "an hourly grid anchored at midnight gives on-the-hour local starts"
    );
}

/// `Zone::resolve_local` documents one failure for a local time it cannot map:
/// `CivilToInstant`. A civil time whose instant falls outside the supported
/// range is exactly that — the time exists, the instant does not — so both the
/// day before 1975 and the day after 2100 must report the same code, not
/// `InstantOutOfRange` (which is the constructor's code for a bad input
/// *value*).
#[test]
fn a_local_time_outside_the_supported_range_is_a_mapping_failure() {
    let zone = Zone::utc();
    for (date, time) in [((1974, 12, 31), (12, 0)), ((2100, 1, 2), (12, 0))] {
        let error = zone
            .resolve_local(
                Date::new(date.0, date.1, date.2).unwrap(),
                TimeOfDay::new(time.0, time.1).unwrap(),
            )
            .unwrap_err();
        assert_eq!(
            error.code(),
            "civil_to_instant",
            "{}-{}-{} {}:{} reported {error}",
            date.0,
            date.1,
            date.2,
            time.0,
            time.1
        );
    }
}

/// A database's version label is normalised one way only. `+VERSION` content
/// is trimmed and a blank file counts as no version at all, but the caller's
/// label is taken verbatim — so a blank label constructs a database whose
/// `meta.tzdb` is empty, and a padded label reports the padding. Both are the
/// same value arriving through a different door and must be treated the same.
#[cfg(not(target_family = "wasm"))]
#[test]
fn a_blank_caller_label_is_no_label() {
    let dir = fixture("tzdb-unversioned");
    for label in ["", "   ", "\n"] {
        let outcome = TimeZoneDatabase::from_dir(&dir, Some(label));
        match outcome {
            Ok(db) => panic!(
                "label {label:?} produced version {:?} instead of failing",
                db.version()
            ),
            Err(error) => assert_eq!(error.code(), "tzdb_version_missing", "label {label:?}"),
        }
    }

    // A padded label is the same label: what reaches `meta.tzdb` is normalised.
    let padded = TimeZoneDatabase::from_dir(&dir, Some(" custom-2018\n")).unwrap();
    assert_eq!(padded.version(), "custom-2018");
}

/// A database directory that exists but holds no zone data is a configuration
/// error, so construction must fail rather than hand back a
/// database that resolves no zone at all — every lookup would look like an
/// unknown zone rather than a wrong directory.
#[cfg(not(target_family = "wasm"))]
#[test]
fn a_directory_with_no_zone_data_is_a_construction_error() {
    let error = TimeZoneDatabase::from_dir(fixture("tzdb-empty"), Some("2026f")).unwrap_err();
    assert_eq!(error.code(), "tzdb_directory_unusable", "{error}");
}

/// A horizon resolved against a `now` in the last minutes of the supported
/// range must answer, not overflow. A duration that runs past `Instant::MAX`
/// bounds nothing (every start that can exist is inside it), a working-day
/// count that walks past the last supported local date likewise, and a zero-day
/// horizon still covers the last day rather than rejecting it for being at the
/// edge of the range.
#[test]
fn a_horizon_at_the_end_of_the_range_bounds_nothing_instead_of_overflowing() {
    let zone = Zone::utc();
    let almost_the_end = Instant::from_minutes(Instant::MAX.minutes() - 30).unwrap();

    let duration = Horizon::Duration(Duration::from_hours_minutes(1, 0))
        .bound(almost_the_end, &zone)
        .unwrap();
    assert!(duration.allows(Instant::MAX));

    let calendar = WorkingCalendar::new(zone.clone(), WorkingDays::weekdays(), Vec::new()).unwrap();
    let working = Horizon::WorkingDays { count: 1, calendar }
        .bound(Instant::MAX, &zone)
        .unwrap();
    assert!(working.allows(Instant::MAX));

    let zero_days = Horizon::Days(0).bound(Instant::MAX, &zone).unwrap();
    assert!(zero_days.allows(Instant::MAX));
}

/// A date that begins twice casts a bound whose local date ends there over both
/// occurrences: Newfoundland moved from 00:01 back to 23:01 the day before until
/// 2010, so the previous day's last hour comes *after* the first midnight. A bound
/// that stops at the first midnight is ordered as if it were stricter than a bound
/// ending in between, and the reason then cites the wrong `lead_time` rule.
#[cfg(feature = "bundled-tzdb")]
#[test]
fn a_date_bound_ending_on_a_repeated_midnight_admits_starts_after_the_repeat() {
    let db = TimeZoneDatabase::bundled().unwrap();
    let zone = db.zone("America/St_Johns").unwrap();
    let LocalResolution::Repeated(first_midnight, second_midnight) = zone
        .resolve_local(Date::new(2010, 11, 7).unwrap(), TimeOfDay::MIDNIGHT)
        .unwrap()
    else {
        panic!("2010-11-07 begins twice in Newfoundland");
    };
    assert!(first_midnight < second_midnight);

    // The bound ends on 2010-11-06, the day before the date that begins twice.
    let now = Instant::from_minutes(first_midnight.minutes() - 60).unwrap();
    let by_date = Horizon::Date(Date::new(2010, 11, 6).unwrap())
        .bound(now, &zone)
        .unwrap();
    let last_minute = Instant::from_minutes(second_midnight.minutes() - 1).unwrap();
    assert!(by_date.allows(last_minute));
    assert!(!by_date.allows(second_midnight));

    // A one-hour maximum from `now` ends before that, so it is the stricter of the
    // two and the rule `lead_time` cites.
    let by_instant = Horizon::Duration(Duration::from_hours_minutes(1, 0))
        .bound(now, &zone)
        .unwrap();
    assert!(by_instant.allows(first_midnight));
    assert!(!by_instant.allows(second_midnight));
    assert!(
        by_instant.is_stricter_than(&by_date),
        "a bound ending before the repeated midnight is the stricter one"
    );
    assert!(!by_date.is_stricter_than(&by_instant));
}

/// The same failure one step deeper: a directory of files that are named like
/// zones but are not `TZif` data is just as unusable as an empty one. Construction
/// must reject it, otherwise it hands back a database whose every lookup is
/// `zone_unknown` — the caller reads a data problem rather than a configuration
/// error.
#[cfg(not(target_family = "wasm"))]
#[test]
fn a_directory_whose_zone_files_are_not_tzif_data_is_a_construction_error() {
    let error = TimeZoneDatabase::from_dir(fixture("tzdb-corrupt"), Some("2026f"));
    match error {
        Err(error) => assert_eq!(error.code(), "tzdb_directory_unusable", "{error}"),
        Ok(db) => panic!(
            "a directory of non-TZif files constructed a database reporting version {:?}",
            db.version()
        ),
    }
}

/// The filesystem path of a fixture directory.
#[cfg(not(target_family = "wasm"))]
fn fixture(directory: &str) -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(directory)
}

/// A window or blackout read at either end of the supported instants resolves
/// local times on the padding dates around the range, and some of those map
/// outside the range entirely. They cannot be breakpoints of a range inside the
/// supported instants, so the builder must skip them rather than fail the whole
/// call — and the range's own minutes must all be classified correctly.
#[cfg(feature = "bundled-tzdb")]
#[test]
fn windows_and_blackouts_at_the_range_edges_build_without_failing() {
    use opportune::time::{
        BlackoutSpan, BlackoutSpec, LocalDateTime, RuleZone, WallEnd, WallRange, Weekday, Weekdays,
        WeeklyKey, WindowSpec, ZoneSchedule,
    };

    let every_day = WeeklyKey::new(
        Weekdays::range(Weekday::Monday, Weekday::Sunday),
        vec![WallRange::new(TimeOfDay::MIDNIGHT, WallEnd::Midnight).unwrap()],
    )
    .unwrap();

    // Twelve hours behind UTC and fourteen ahead, so the padded local dates at
    // each end are outside the supported instants.
    for (name, from, until) in [
        ("Etc/GMT+12", Instant::MIN, shifted(Instant::MIN, 24 * 60)),
        (
            "Pacific/Kiritimati",
            shifted(Instant::MAX, -24 * 60),
            Instant::MAX,
        ),
    ] {
        let db = TimeZoneDatabase::bundled().unwrap();
        let zone = db.zone(name).unwrap();
        let schedule = ZoneSchedule::fixed(zone.clone());

        let window = WindowSpec::new(
            RuleZone::Named(ZoneName::new(zone.name()).unwrap()),
            vec![every_day.clone()],
            Vec::new(),
        )
        .unwrap();
        let intervals = window.intervals(&schedule, from, until).unwrap();
        assert_eq!(intervals.spans().len(), 1, "{name}: open around the clock");
        assert_eq!(intervals.spans()[0].start(), from.minutes());
        assert_eq!(intervals.spans()[0].end(), until.minutes());
        assert!(window.contains(from, &schedule));

        // A blackout from well before the supported range to well after this
        // one covers every minute of it, with both boundaries unresolvable in
        // one zone or the other.
        let blackout = BlackoutSpec::new(
            RuleZone::Named(ZoneName::new(zone.name()).unwrap()),
            vec![
                BlackoutSpan::new(
                    LocalDateTime::new(
                        Date::new(1974, 6, 1).unwrap(),
                        opportune::time::TimeOfDay::MIDNIGHT,
                    ),
                    LocalDateTime::new(
                        Date::new(2101, 6, 1).unwrap(),
                        opportune::time::TimeOfDay::MIDNIGHT,
                    ),
                )
                .unwrap(),
            ],
        );
        let blacked = blackout.intervals(&schedule, from, until).unwrap();
        assert_eq!(blacked.spans().len(), 1, "{name}: blacked around the clock");
        assert_eq!(blacked.spans()[0].start(), from.minutes());
        assert_eq!(blacked.spans()[0].end(), until.minutes());
    }
}

/// `Zone::day_start` is the seam an all-day event resolves through, and
/// `Zone::resolve_local_compatible` is the one a provider's local timestamp uses:
/// the earlier instant in a repeated hour and the instant after the gap in a
/// skipped one. Santiago springs forward from midnight, so both meet the hardest
/// case the bundled database holds, and the seconds a caller's timestamp carries
/// move with the reading rather than being added to a minute-aligned instant
/// afterwards.
#[cfg(feature = "bundled-tzdb")]
#[test]
fn a_skipped_midnight_begins_at_the_transition_and_carries_the_seconds_through_the_gap() {
    let db = TimeZoneDatabase::bundled().unwrap();
    let santiago = db.zone("America/Santiago").unwrap();
    let date = Date::new(2027, 9, 5).unwrap();

    // 00:00 does not exist: the date begins at 01:00 local, 04:00Z.
    let begins = santiago.day_start(date).unwrap();
    assert_eq!(begins, utc(2027, 9, 5, 4, 0));
    assert_eq!(santiago.zoned(begins).time(), TimeOfDay::new(1, 0).unwrap());

    // 00:30:15 does not exist either. Compatible resolution names the reading
    // after the gap, 01:30:15 local, which is fifteen seconds past 04:30Z.
    let seconds = santiago
        .resolve_local_compatible(date, TimeOfDay::new(0, 30).unwrap(), 15)
        .unwrap();
    assert_eq!(seconds, begins.unix_seconds() + 30 * 60 + 15);
    assert_eq!(
        Instant::floor_to_minute_nanos(seconds, 0).unwrap(),
        utc(2027, 9, 5, 4, 30)
    );
    assert_eq!(
        Instant::ceil_to_minute_nanos(seconds, 0).unwrap(),
        utc(2027, 9, 5, 4, 31),
        "a stray second carries the end into the next minute"
    );
    assert_eq!(
        santiago
            .resolve_local_compatible(date, TimeOfDay::new(0, 30).unwrap(), 60)
            .unwrap_err()
            .code(),
        "second_out_of_range"
    );
}

/// A repeated hour resolves to its earlier instant, which is the reading before
/// the clocks went back; a skipped hour resolves to the reading after the gap.
#[cfg(feature = "bundled-tzdb")]
#[test]
fn compatible_resolution_takes_the_earlier_repeated_hour_and_the_later_skipped_one() {
    let db = TimeZoneDatabase::bundled().unwrap();
    let new_york = db.zone("America/New_York").unwrap();

    // Spring forward: 02:00 jumps to 03:00, so 02:30 is the reading after the gap.
    let spring = Date::new(2027, 3, 14).unwrap();
    let gap = new_york
        .resolve_local_compatible(spring, TimeOfDay::new(2, 30).unwrap(), 0)
        .unwrap();
    assert_eq!(
        Instant::from_unix_seconds(gap).unwrap(),
        utc(2027, 3, 14, 7, 30)
    );

    // Fall back: 02:00 becomes 01:00, so 01:30 exists twice and the earlier
    // instant is the one the reading names.
    let autumn = Date::new(2027, 11, 7).unwrap();
    let LocalResolution::Repeated(earlier, later) = new_york
        .resolve_local(autumn, TimeOfDay::new(1, 30).unwrap())
        .unwrap()
    else {
        panic!("01:30 occurs twice on the fall-back day")
    };
    let fold = new_york
        .resolve_local_compatible(autumn, TimeOfDay::new(1, 30).unwrap(), 0)
        .unwrap();
    assert_eq!(Instant::from_unix_seconds(fold).unwrap(), earlier);
    assert_ne!(earlier, later, "the repeated hour really is repeated");
}

/// A timestamp with a fractional second rounds outward to the minute the
/// request needs, and only an aligned value is an instant at all.
#[test]
fn fractional_seconds_round_outward_and_only_an_aligned_value_is_an_instant() {
    let first = Instant::MIN.unix_seconds();
    let last = Instant::MAX.unix_seconds();
    let one_minute_in = Instant::from_minutes(Instant::MIN.minutes() + 1).unwrap();

    assert_eq!(
        Instant::from_unix_seconds_nanos(first, 0).unwrap(),
        Instant::MIN
    );
    assert!(Instant::from_unix_seconds_nanos(first, 1).is_err());
    assert!(Instant::from_unix_seconds_nanos(first + 1, 0).is_err());
    assert_eq!(
        Instant::floor_to_minute_nanos(first + 59, 999_999_999).unwrap(),
        Instant::MIN
    );
    assert_eq!(
        Instant::ceil_to_minute_nanos(first + 1, 0).unwrap(),
        one_minute_in
    );
    assert_eq!(
        Instant::ceil_to_minute_nanos(last, 0).unwrap(),
        Instant::MAX,
        "an aligned end at the last supported second stays there"
    );
    assert_eq!(
        Instant::ceil_to_minute_nanos(last - 1, 0).unwrap(),
        Instant::MAX,
        "a second before the end still ends at the last instant"
    );
    assert!(Instant::floor_to_minute_nanos(first - 1, 0).is_err());
    assert!(Instant::ceil_to_minute_nanos(last + 1, 0).is_err());
    assert_eq!(
        Instant::from_unix_seconds_nanos(first, 1_000_000_000)
            .unwrap_err()
            .code(),
        "nanos_out_of_range"
    );
}

/// The day side of a `weekly` key and a working calendar's week are the same set,
/// so one value serves both without conversion, and every weekday is reachable in
/// order.
#[test]
fn the_two_weekday_sets_are_one_type() {
    let business: WorkingDays = Weekdays::weekdays();
    assert_eq!(
        business.iter().collect::<Vec<_>>(),
        Weekday::ALL[..5],
        "the working week is Monday to Friday"
    );
    assert_eq!(Weekday::iter().count(), 7);
    assert_eq!(
        Weekdays::range(Weekday::Friday, Weekday::Monday)
            .iter()
            .collect::<Vec<_>>(),
        [
            Weekday::Monday,
            Weekday::Friday,
            Weekday::Saturday,
            Weekday::Sunday
        ],
        "a wrapping range holds Friday to Monday, read Monday first"
    );
    assert!(WorkingDays::NONE.is_empty());
    assert_eq!(Weekdays::ALL.iter().count(), 7);
}

/// The value constructors build exactly what the validated ones build, and the
/// conversions a caller reaches for are the same rules rather than a second
/// spelling.
#[test]
fn the_value_constructors_agree_with_their_validated_forms() {
    assert_eq!(Duration::from_hours(4), Duration::from_hours_minutes(4, 0));
    assert_eq!(
        ZoneName::try_from("Europe/London").unwrap(),
        ZoneName::new("Europe/London").unwrap()
    );
    let owned = ZoneName::try_from("Europe/London".to_owned()).unwrap();
    assert_eq!(owned.as_ref(), "Europe/London");
    assert_eq!(ZoneName::new("  ").unwrap_err().code(), "zone_name_invalid");
    assert_eq!(
        opportune::time::WallEnd::from(TimeOfDay::new(17, 0).unwrap()),
        opportune::time::WallEnd::Time(TimeOfDay::new(17, 0).unwrap())
    );
}

/// `ceil_to_minute_nanos` rounds outward, so a fraction inside the final
/// supported second names a minute past `Instant::MAX`. Every supported instant
/// is inside the range, so the constructor must refuse rather than build one the
/// rest of the arithmetic trusts.
#[test]
fn a_fractional_second_at_the_last_supported_instant_stays_inside_the_range() {
    let last = Instant::MAX.unix_seconds();
    let rounded = Instant::ceil_to_minute_nanos(last, 500_000_000)
        .expect_err("a fraction inside the final supported second rounds past `Instant::MAX`");
    assert_eq!(rounded.code(), "instant_out_of_range");
    assert_eq!(
        Instant::ceil_to_minute_nanos(last, 0).unwrap(),
        Instant::MAX,
        "an aligned end at the last supported second stays there"
    );
}

/// A date maximum ends where the previous date's last instant ends — not at the
/// last midnight of the next date. Cuba ended daylight saving at 01:00 wall time
/// (00:00 standard) until its recent rule change, so local midnight of 2018-11-04
/// happened twice and the hour between the two midnights already read the 4th. A
/// bound ending on the 3rd therefore stops at the *first* midnight; reading the
/// last one hands it an extra hour and orders it as if it were looser than a
/// duration that actually admits more.
#[cfg(feature = "bundled-tzdb")]
#[test]
fn a_date_bound_ending_before_a_midnight_starting_fold_is_the_stricter_one() {
    let db = TimeZoneDatabase::bundled().unwrap();
    let havana = db.zone("America/Havana").unwrap();
    let next = Date::new(2018, 11, 4).unwrap();
    let LocalResolution::Repeated(first, second) =
        havana.resolve_local(next, TimeOfDay::MIDNIGHT).unwrap()
    else {
        panic!("2018-11-04 begins twice in Havana");
    };
    assert_eq!(second.minutes() - first.minutes(), 60);
    let last_of_the_3rd = Instant::from_minutes(first.minutes() - 1).unwrap();

    // `now` is 23:00 local on the 3rd; the date maximum is the 3rd.
    let now = Instant::from_minutes(first.minutes() - 60).unwrap();
    let by_date = Horizon::Date(Date::new(2018, 11, 3).unwrap())
        .bound(now, &havana)
        .unwrap();
    assert!(by_date.allows(last_of_the_3rd));
    assert!(
        !by_date.allows(first),
        "the first midnight already reads the 4th"
    );

    let duration_to = |instant: Instant| {
        Duration::from_minutes(u32::try_from(instant.minutes() - now.minutes()).unwrap())
    };

    // A duration ending exactly where the date maximum ends is neither stricter nor
    // looser: equal bounds keep the earliest rule in request order.
    let equal = Horizon::Duration(duration_to(last_of_the_3rd))
        .bound(now, &havana)
        .unwrap();
    assert!(!by_date.is_stricter_than(&equal));
    assert!(!equal.is_stricter_than(&by_date));

    // A duration one minute longer admits the first Nov 4 start, so the date
    // maximum is the stricter of the two and the rule `lead_time` must cite.
    let one_past = Horizon::Duration(duration_to(Instant::from_minutes(first.minutes()).unwrap()))
        .bound(now, &havana)
        .unwrap();
    assert!(
        one_past.allows(first),
        "the fixture duration reaches into the 4th"
    );
    assert!(by_date.is_stricter_than(&one_past));
    assert!(!one_past.is_stricter_than(&by_date));
}

/// The strictest `lead_time` maximum decides eligibility whichever order the rules
/// arrive in. The Havana fall-back above is where that used to depend on order: the
/// date maximum ending on the 3rd is stricter than a duration that reaches into the
/// repeated hour, but reading the boundary from the last midnight made it look the
/// looser one when the duration came first.
#[cfg(feature = "bundled-tzdb")]
#[test]
fn the_strictest_lead_time_maximum_does_not_depend_on_rule_order() {
    use opportune::Engine;
    use opportune::model::{
        Goal, LeadTime, LeadTimeMax, Part, Participant, Request, Requirement, Rule, RuleKind, Span,
        Term,
    };

    let db = TimeZoneDatabase::bundled().unwrap();
    let havana = db.zone("America/Havana").unwrap();
    let LocalResolution::Repeated(first, _) = havana
        .resolve_local(Date::new(2018, 11, 4).unwrap(), TimeOfDay::MIDNIGHT)
        .unwrap()
    else {
        panic!("2018-11-04 begins twice in Havana");
    };
    // 23:00 and 23:30 local on the 3rd, then 00:00 local on the 4th.
    let evening = Instant::from_minutes(first.minutes() - 60).unwrap();
    let candidates = vec![
        evening,
        Instant::from_minutes(first.minutes() - 30).unwrap(),
        first,
    ];
    let search = Span::new(
        evening,
        Instant::from_minutes(first.minutes() + 30).unwrap(),
    )
    .unwrap();

    let rule = |id: &str, max: LeadTimeMax| {
        Rule::new(
            id,
            RuleKind::LeadTime(LeadTime {
                min: Duration::ZERO,
                max: Some(max),
            }),
        )
    };
    let by_date = || LeadTimeMax::Date(Date::new(2018, 11, 3).unwrap());
    let by_duration = || LeadTimeMax::Duration(Duration::from_minutes(119));

    let offers = |rules: Vec<Rule>| {
        let part = Part::new(
            Duration::from_minutes(30),
            Requirement::new(vec![Term::person("ana")]),
        );
        let mut request = Request::new(
            evening,
            Goal::new(ZoneName::new("America/Havana").unwrap(), search, vec![part]),
        );
        request.goal.candidates = Some(candidates.clone());
        let mut ana = Participant::new("ana", ZoneName::new("UTC").unwrap());
        ana.calendar = Some(Vec::new());
        request.participants.push(ana);
        request.rules = rules;
        Engine::builder()
            .build()
            .unwrap()
            .find_availability(&request)
            .unwrap()
            .offers
            .iter()
            .filter_map(opportune::model::Offer::start)
            .collect::<Vec<_>>()
    };

    let duration_first = offers(vec![
        rule("duration", by_duration()),
        rule("date", by_date()),
    ]);
    let date_first = offers(vec![
        rule("date", by_date()),
        rule("duration", by_duration()),
    ]);
    assert_eq!(
        duration_first, date_first,
        "the strictest lead_time maximum depends on the order the rules arrive in"
    );
    assert!(
        !duration_first.is_empty(),
        "the fixture should still offer the evening of the 3rd: {duration_first:?}"
    );
    assert!(
        duration_first.iter().all(|start| *start < first),
        "a start at or after 2018-11-04 is past a date maximum of 2018-11-03: {duration_first:?}"
    );
}

/// The last minute within a day either side of `boundary` whose local date is at or
/// before `date`, found by reading every minute back — the definition the bound has
/// to agree with, not the engine's boundary arithmetic.
#[cfg(feature = "bundled-tzdb")]
fn scanned_last_instant_on(zone: &Zone, date: Date, from: Instant, to: Instant) -> Option<Instant> {
    let mut last = None;
    for minute in from.minutes()..=to.minutes() {
        let Ok(instant) = Instant::from_minutes(minute) else {
            continue;
        };
        if zone.zoned(instant).date() <= date {
            last = Some(instant);
        }
    }
    last
}

/// The minutes a scan of `date`'s boundary has to cover when `next` begins at a
/// repeated or skipped midnight, and `None` when it has an ordinary single midnight.
#[cfg(feature = "bundled-tzdb")]
fn boundary_scan_window(zone: &Zone, next: Date) -> Option<(Instant, Instant)> {
    let around = |anchor: Instant| {
        Some((
            Instant::from_minutes(anchor.minutes() - 120).ok()?,
            Instant::from_minutes(anchor.minutes() + 120).ok()?,
        ))
    };
    match zone.resolve_local(next, TimeOfDay::MIDNIGHT).ok()? {
        LocalResolution::Repeated(early, later) => Some((
            Instant::from_minutes(early.minutes() - 120).ok()?,
            Instant::from_minutes(later.minutes() + 120).ok()?,
        )),
        LocalResolution::Skipped => around(zone.day_start(next).ok()?),
        LocalResolution::Unique(_) => None,
    }
}

/// The date maximum on `date` admits exactly up to `exact_last`, and its strictness
/// against instant bounds at that minute and one minute later matches what the
/// bounds actually admit.
#[cfg(feature = "bundled-tzdb")]
fn assert_date_bound_ends_at(zone: &Zone, date: Date, exact_last: Instant) {
    let now = Instant::MIN;
    let by_date = Horizon::Date(date).bound(now, zone).unwrap();
    let by_instant = |instant: Instant| {
        Horizon::Duration(Duration::from_minutes(
            u32::try_from(instant.minutes() - now.minutes()).unwrap(),
        ))
        .bound(now, zone)
        .unwrap()
    };
    let next = Instant::from_minutes(exact_last.minutes() + 1).unwrap();
    let equal = by_instant(exact_last);
    let one_past = by_instant(next);

    let label = |instant: Instant| {
        format!(
            "{instant} reads {:?} in {zone:?}",
            zone.zoned(instant).date()
        )
    };
    assert!(
        by_date.allows(exact_last),
        "{} should be inside {date}",
        label(exact_last)
    );
    assert!(
        !by_date.allows(next),
        "{} should be past {date}",
        label(next)
    );
    // Bounds ending at the same minute are equal, so neither is stricter.
    assert!(
        !by_date.is_stricter_than(&equal),
        "{date}: an equal bound is not stricter"
    );
    assert!(
        !equal.is_stricter_than(&by_date),
        "{date}: the date bound is not stricter than an equal one"
    );
    // A bound one minute longer is looser, so the date bound is the stricter one.
    assert!(
        by_date.is_stricter_than(&one_past),
        "{}: {date} should be stricter than a bound ending at {next}",
        label(next)
    );
    assert!(
        !one_past.is_stricter_than(&by_date),
        "{}: a longer bound is not stricter than {date}",
        label(next)
    );
}

/// A date maximum has to end where the local reading leaves its date, in every
/// zone. This walks the bundled database for the boundaries that make that hard —
/// a midnight a backward transition repeats, on either side of the boundary, and one
/// a forward transition skips — and checks the bound's last allowed minute and its
/// strictness order against a minute-by-minute scan.
#[cfg(feature = "bundled-tzdb")]
#[test]
fn a_date_maximum_ends_at_the_scanned_boundary_wherever_a_midnight_repeats_or_skips() {
    const ZONES: [&str; 8] = [
        "America/Havana",    // 01:00 wall back to 00:00: midnight itself repeats
        "America/St_Johns",  // 00:01 back to 23:01: the previous date resumes
        "America/Sao_Paulo", // 00:00 back to 23:00: the previous date resumes
        "America/Santiago",  // midnight skipped
        "Pacific/Apia",      // a whole date skipped
        "Pacific/Chatham",   // +12:45, a quarter-hour offset control
        "Europe/London",     // an ordinary midnight control
        "UTC",               // a transition-free control
    ];
    let db = TimeZoneDatabase::bundled().unwrap();
    let first = Date::new(2000, 1, 1).unwrap();
    let last = Date::new(2030, 12, 1).unwrap();
    let mut checked = 0usize;

    for name in ZONES {
        let zone = db.zone(name).unwrap();
        let mut day = first;
        while day < last {
            let next_day = day.plus_days(1).unwrap();
            // Only a day whose UTC span contains an offset change can produce a
            // repeated or skipped local midnight.
            if zone.offset_at(Instant::from_utc(day, TimeOfDay::MIDNIGHT).unwrap())
                == zone.offset_at(Instant::from_utc(next_day, TimeOfDay::MIDNIGHT).unwrap())
            {
                day = next_day;
                continue;
            }
            // The local dates either side of the change: one of them is the date
            // whose next midnight is repeated or skipped.
            for probe in [-1, 0, 1] {
                let Some(date) = zone
                    .zoned(Instant::from_utc(day, TimeOfDay::MIDNIGHT).unwrap())
                    .date()
                    .plus_days(probe)
                    .ok()
                else {
                    continue;
                };
                let Some(next) = date.plus_days(1).ok() else {
                    continue;
                };
                // The window the scan has to cover: the first midnight (or the
                // transition that swallows it) to the last midnight of `next`.
                let Some((from, to)) = boundary_scan_window(&zone, next) else {
                    continue;
                };
                let Some(exact_last) = scanned_last_instant_on(&zone, date, from, to) else {
                    continue;
                };
                assert_date_bound_ends_at(&zone, date, exact_last);
                checked += 1;
            }
            day = next_day;
        }
    }

    assert!(
        checked > 20,
        "the sweep found only {checked} repeated or skipped midnights across the zones"
    );
}
