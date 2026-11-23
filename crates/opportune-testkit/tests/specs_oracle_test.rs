//! Property tests: the engine's window and blackout interval builders must
//! agree minute for minute with direct instant-to-local membership, and instant
//! membership must agree with the oracle's reading of the same rule.
//!
//! The oracle is `opportune_testkit::specs`, which applies the specification text
//! to an instant's local date and time without the engine's specification types
//! or breakpoint builder, so a shared shortcut shows up as a disagreement.
//!
//! The generated cases cover the rule shapes; the hand-built cases after them
//! cover what generation does not reach — a range edge landing exactly on an
//! offset transition or zone-period boundary, a range far longer than three
//! days, the ends of the supported instant range, and zones whose transitions
//! fall at midnight, on a half hour, or across a whole date label.

use std::sync::OnceLock;

use opportune::intervals::IntervalSet;
use opportune::time::{
    BlackoutSpan, Date, DateKey, DateRange, Instant, LocalDateTime, LocalResolution, RuleZone,
    TimeOfDay, TimeZoneDatabase, WallEnd, WallRange, Weekday, Weekdays, WeeklyKey, WindowSpec,
    Zone, ZoneName, ZonePeriod, ZoneSchedule,
};
use opportune_testkit::generators::{blackout_case, window_case};
use opportune_testkit::specs::{BlackoutCase, PlanZone, WindowCase, ZonePlan};
use proptest::prelude::*;

fn database() -> &'static TimeZoneDatabase {
    static DATABASE: OnceLock<TimeZoneDatabase> = OnceLock::new();
    DATABASE.get_or_init(|| TimeZoneDatabase::bundled().expect("the bundled database is present"))
}

fn weekdays(mask: u8) -> Weekdays {
    const WEEK: [Weekday; 7] = [
        Weekday::Monday,
        Weekday::Tuesday,
        Weekday::Wednesday,
        Weekday::Thursday,
        Weekday::Friday,
        Weekday::Saturday,
        Weekday::Sunday,
    ];
    Weekdays::list(
        WEEK.into_iter()
            .enumerate()
            .filter(|(index, _)| mask & (1 << index) != 0)
            .map(|(_, day)| day),
    )
}

fn wall_range(start: u16, end: u16) -> WallRange {
    let start = TimeOfDay::from_minute_of_day(start).expect("a start below 1440");
    let end = if end == TimeOfDay::MINUTES_PER_DAY {
        WallEnd::Midnight
    } else {
        WallEnd::Time(TimeOfDay::from_minute_of_day(end).expect("an end at most 1440"))
    };
    WallRange::new(start, end).expect("the generator emits non-empty, non-overlapping ranges")
}

fn ranges(hours: &[(u16, u16)]) -> Vec<WallRange> {
    hours
        .iter()
        .map(|&(start, end)| wall_range(start, end))
        .collect()
}

/// A rule zone naming `zone`.
fn named_zone(zone: &Zone) -> RuleZone {
    RuleZone::Named(ZoneName::new(zone.name()).expect("a resolved zone has a name"))
}

fn engine_window(case: &WindowCase, named: &Zone) -> WindowSpec {
    let zone = match case.zone {
        PlanZone::Participant => RuleZone::Participant,
        PlanZone::Event => RuleZone::Event,
        PlanZone::Named => named_zone(named),
    };
    let weekly = case
        .weekly
        .iter()
        .map(|(mask, hours)| {
            WeeklyKey::new(weekdays(*mask), ranges(hours))
                .expect("the generator emits disjoint weekly keys")
        })
        .collect();
    let dates = case
        .dates
        .iter()
        .map(|(start, end, hours)| {
            DateKey::new(
                DateRange::new(*start, *end).expect("the generator emits ordered dates"),
                ranges(hours),
            )
            .expect("the generator emits non-overlapping ranges")
        })
        .collect();
    WindowSpec::new(zone, weekly, dates).expect("the generator emits disjoint date keys")
}

fn engine_blackout(case: &BlackoutCase, named: &Zone) -> opportune::time::BlackoutSpec {
    let zone = match case.zone {
        PlanZone::Participant => RuleZone::Participant,
        PlanZone::Event => RuleZone::Event,
        PlanZone::Named => named_zone(named),
    };
    let spans = case
        .spans
        .iter()
        .map(|(start, end)| {
            BlackoutSpan::new(
                LocalDateTime::new(start.0, start.1),
                LocalDateTime::new(end.0, end.1),
            )
            .expect("the generator emits ascending spans")
        })
        .collect();
    opportune::time::BlackoutSpec::new(zone, spans)
}

/// Every built span must lie inside the range the caller asked for: the builder
/// clips only to that range, never to midnight or a padded bound.
fn assert_within(intervals: &IntervalSet, from: Instant, until: Instant) {
    for span in intervals.spans() {
        assert!(
            from.minutes() <= span.start() && span.end() <= until.minutes(),
            "span {}..{} leaves the requested range {from}..{until}",
            span.start(),
            span.end()
        );
    }
}

fn engine_schedule(plan: &ZonePlan) -> ZoneSchedule {
    let home = database()
        .zone(&plan.home)
        .unwrap_or_else(|err| panic!("`{}` is in the bundled database: {err}", plan.home));
    let periods = plan
        .periods
        .iter()
        .map(|(name, from, until)| {
            let zone = database()
                .zone(name)
                .unwrap_or_else(|err| panic!("`{name}` is in the bundled database: {err}"));
            ZonePeriod::new(zone, *from, *until).expect("the generator emits disjoint legs")
        })
        .collect();
    ZoneSchedule::new(home, periods).expect("the generator emits disjoint legs")
}

/// Every minute of the range must agree three ways: the built interval set, the
/// specification's own instant membership, and the oracle's reading of the
/// specification text.
fn assert_window_agrees(case: &WindowCase) {
    let event = database().zone(&case.event).unwrap();
    let named = database().zone(&case.named).unwrap();
    let resolved = case.zones.resolve(database());
    let schedule = engine_schedule(&case.zones);
    let spec = engine_window(case, &named);
    // The rule's own zone choice, resolved the way the engine resolves it
    // before reading the specification.
    let zones = spec
        .zone()
        .resolve(database(), &schedule, &event)
        .expect("the generator draws zone names from the database");
    let intervals = spec
        .intervals(&zones, case.from, case.until)
        .expect("a whole-minute database builds intervals");
    assert_within(&intervals, case.from, case.until);

    for minute in case.from.minutes()..case.until.minutes() {
        let instant = Instant::from_minutes(minute).unwrap();
        let oracle = case.contains(instant, &resolved, &event, &named);
        assert_eq!(
            intervals.contains_minute(minute),
            oracle,
            "builder at {instant} over {}..{} ({:?})",
            case.from,
            case.until,
            case.zone
        );
        assert_eq!(
            spec.contains(instant, &zones),
            oracle,
            "instant membership at {instant} over {}..{} ({:?})",
            case.from,
            case.until,
            case.zone
        );
    }
}

/// The blackout counterpart of [`assert_window_agrees`].
fn assert_blackout_agrees(case: &BlackoutCase) {
    let event = database().zone(&case.event).unwrap();
    let named = database().zone(&case.named).unwrap();
    let resolved = case.zones.resolve(database());
    let schedule = engine_schedule(&case.zones);
    let spec = engine_blackout(case, &named);
    let zones = spec
        .zone()
        .resolve(database(), &schedule, &event)
        .expect("the generator draws zone names from the database");
    let intervals = spec
        .intervals(&zones, case.from, case.until)
        .expect("a whole-minute database builds intervals");
    assert_within(&intervals, case.from, case.until);

    for minute in case.from.minutes()..case.until.minutes() {
        let instant = Instant::from_minutes(minute).unwrap();
        let oracle = case.contains(instant, &resolved, &event, &named);
        assert_eq!(
            intervals.contains_minute(minute),
            oracle,
            "builder at {instant} over {}..{} ({:?})",
            case.from,
            case.until,
            case.zone
        );
        assert_eq!(
            spec.contains(instant, &zones),
            oracle,
            "instant membership at {instant} over {}..{} ({:?})",
            case.from,
            case.until,
            case.zone
        );
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    #[test]
    fn window_intervals_agree_with_minute_by_minute_membership(case in window_case()) {
        assert_window_agrees(&case);
    }

    #[test]
    fn blackout_intervals_agree_with_minute_by_minute_membership(case in blackout_case()) {
        assert_blackout_agrees(&case);
    }
}

// ---------------------------------------------------------------------------
// Hand-built cases
//
// The generated cases pick a range edge at an arbitrary minute, stay within
// three days, never reach the ends of the supported instant range, and draw zone
// names from nine zones — none of which moves at local midnight, has a sub-hour
// daylight saving offset, or jumps a whole date label. Each case below attacks
// one of those limits.
// ---------------------------------------------------------------------------

/// The instant of a UTC civil date-time, which always exists.
fn utc(year: i16, month: i8, day: i8, hour: u8, minute: u8) -> Instant {
    let date = Date::new(year, month, day).unwrap();
    let time = TimeOfDay::new(hour, minute).unwrap();
    let LocalResolution::Unique(instant) = Zone::utc().resolve_local(date, time).unwrap() else {
        panic!("UTC has no transitions")
    };
    instant
}

/// `instant` moved by `minutes`, staying inside the supported range.
fn shifted(instant: Instant, minutes: i64) -> Instant {
    Instant::from_minutes(instant.minutes() + minutes).unwrap()
}

/// A participant plan that never leaves `home`.
fn resident(home: &str) -> ZonePlan {
    ZonePlan {
        home: home.to_owned(),
        periods: Vec::new(),
    }
}

/// Every day open 01:30-03:30 and 22:00-01:00 (crossing midnight): a range
/// boundary close to both a midnight and a transition's usual 01:00-03:00
/// window, so a missed breakpoint anywhere near them shows up.
const AROUND_THE_CLOCK_BOUNDARIES: &[(u16, u16)] = &[(90, 210), (1_320, 60)];

/// A window open 01:30-03:30 and 22:00-01:00 every day, read over
/// `from..until` in the named `name` zone.
fn window_case_around(name: &str, zone: PlanZone, from: Instant, until: Instant) -> WindowCase {
    WindowCase {
        zone,
        zones: resident(name),
        event: name.to_owned(),
        named: name.to_owned(),
        weekly: vec![(0b111_1111, AROUND_THE_CLOCK_BOUNDARIES.to_vec())],
        dates: Vec::new(),
        from,
        until,
    }
}

#[test]
fn window_edges_aligned_to_a_transition_agree_with_membership() {
    // New York falls back at 2027-11-07T06:00Z and springs forward at
    // 2027-03-14T07:00Z; Lord Howe moves by half an hour at 2027-04-03T15:00Z.
    for (name, transition) in [
        ("America/New_York", utc(2027, 11, 7, 6, 0)),
        ("America/New_York", utc(2027, 3, 14, 7, 0)),
        ("Australia/Lord_Howe", utc(2027, 4, 3, 15, 0)),
    ] {
        let local_date = database().zone(name).unwrap().zoned(transition).date();
        let mut case = window_case_around(name, PlanZone::Named, transition, transition);
        // An override on the transition's own local date replaces that day's
        // key, so the date boundary and the offset transition coincide too.
        case.dates = vec![(local_date, local_date, vec![(0, 300)])];
        for (from, until) in [
            (shifted(transition, -120), shifted(transition, 120)),
            (transition, shifted(transition, 120)),
            (shifted(transition, -120), transition),
            (shifted(transition, -1), shifted(transition, 1)),
            (transition, shifted(transition, 1)),
            (shifted(transition, -1), transition),
        ] {
            case.from = from;
            case.until = until;
            assert_window_agrees(&case);
        }
    }
}

#[test]
fn blackout_spans_coinciding_with_a_transition_agree_with_membership() {
    // Span boundaries inside the repeated New York hour, and around Lord Howe's
    // half-hour shift, read over ranges whose edges sit on the transition.
    let new_york = utc(2027, 11, 7, 6, 0);
    let spans = [
        // [01:59, 02:00) exists once on each side of the fall back.
        (
            (
                Date::new(2027, 11, 7).unwrap(),
                TimeOfDay::new(1, 59).unwrap(),
            ),
            (
                Date::new(2027, 11, 7).unwrap(),
                TimeOfDay::new(2, 0).unwrap(),
            ),
        ),
        // [01:00, 02:00) is the whole repeated hour, twice over.
        (
            (
                Date::new(2027, 11, 7).unwrap(),
                TimeOfDay::new(1, 0).unwrap(),
            ),
            (
                Date::new(2027, 11, 7).unwrap(),
                TimeOfDay::new(2, 0).unwrap(),
            ),
        ),
    ];
    for span in spans {
        for (from, until) in [
            (shifted(new_york, -120), shifted(new_york, 120)),
            (new_york, shifted(new_york, 120)),
            (shifted(new_york, -120), new_york),
            (shifted(new_york, -1), shifted(new_york, 1)),
        ] {
            let case = BlackoutCase {
                zone: PlanZone::Named,
                zones: resident("America/New_York"),
                event: "America/New_York".to_owned(),
                named: "America/New_York".to_owned(),
                spans: vec![span],
                from,
                until,
            };
            assert_blackout_agrees(&case);
        }
    }
}

#[test]
fn a_zone_period_switch_inside_a_repeated_hour_agrees_with_membership() {
    // The participant is in Tokyo for one minute of the repeated New York hour,
    // so the switch lands inside a fold: the local reading jumps from 01:45 EDT
    // to 14:45 JST and back to 01:15 EST.
    let transition = utc(2027, 11, 7, 6, 0);
    for (leg_start, leg_end) in [
        (transition, shifted(transition, 1)),
        (shifted(transition, -1), transition),
        (shifted(transition, -15), shifted(transition, 15)),
        (transition, shifted(transition, 30)),
    ] {
        let case = WindowCase {
            zone: PlanZone::Participant,
            zones: ZonePlan {
                home: "America/New_York".to_owned(),
                periods: vec![("Asia/Tokyo".to_owned(), leg_start, leg_end)],
            },
            event: "America/New_York".to_owned(),
            named: "America/New_York".to_owned(),
            weekly: vec![(0b111_1111, AROUND_THE_CLOCK_BOUNDARIES.to_vec())],
            dates: Vec::new(),
            from: shifted(transition, -180),
            until: shifted(transition, 180),
        };
        assert_window_agrees(&case);

        let blackout = BlackoutCase {
            zone: PlanZone::Participant,
            zones: case.zones.clone(),
            event: case.event.clone(),
            named: case.named.clone(),
            spans: vec![(
                (
                    Date::new(2027, 11, 7).unwrap(),
                    TimeOfDay::new(1, 30).unwrap(),
                ),
                (
                    Date::new(2027, 11, 7).unwrap(),
                    TimeOfDay::new(2, 30).unwrap(),
                ),
            )],
            from: case.from,
            until: case.until,
        };
        assert_blackout_agrees(&blackout);
    }
}

#[test]
fn a_multi_month_window_agrees_with_membership_across_every_transition() {
    // Four hundred days in a zone with two transitions a year, with a two-week
    // trip, weekend hours that cross midnight and two overrides. `dates_covering`
    // has to stay exact over a range two orders of magnitude longer than the
    // generated three days.
    let case = WindowCase {
        zone: PlanZone::Participant,
        zones: ZonePlan {
            home: "Europe/London".to_owned(),
            periods: vec![(
                "Australia/Lord_Howe".to_owned(),
                utc(2027, 6, 1, 0, 0),
                utc(2027, 6, 20, 0, 0),
            )],
        },
        event: "Europe/London".to_owned(),
        named: "Europe/London".to_owned(),
        weekly: vec![
            (0b000_1111, vec![(540, 1_020)]),
            (0b111_0000, vec![(480, 720), (1_320, 120)]),
        ],
        dates: vec![
            (
                Date::new(2027, 3, 1).unwrap(),
                Date::new(2027, 5, 31).unwrap(),
                Vec::new(),
            ),
            (
                Date::new(2027, 12, 24).unwrap(),
                Date::new(2027, 12, 31).unwrap(),
                Vec::new(),
            ),
            (
                Date::new(2027, 6, 10).unwrap(),
                Date::new(2027, 6, 10).unwrap(),
                vec![(0, 1_440)],
            ),
        ],
        from: utc(2027, 1, 1, 0, 0),
        until: utc(2028, 2, 1, 0, 0),
    };
    assert_window_agrees(&case);

    // A blackout over the same range, with one span longer than the generated
    // four days and two boundaries inside a transition.
    let blackout = BlackoutCase {
        zone: PlanZone::Participant,
        zones: case.zones.clone(),
        event: case.event.clone(),
        named: case.named.clone(),
        spans: vec![
            (
                (Date::new(2026, 12, 1).unwrap(), TimeOfDay::MIDNIGHT),
                (Date::new(2027, 3, 1).unwrap(), TimeOfDay::MIDNIGHT),
            ),
            (
                (
                    Date::new(2027, 10, 20).unwrap(),
                    TimeOfDay::new(23, 30).unwrap(),
                ),
                (
                    Date::new(2027, 11, 5).unwrap(),
                    TimeOfDay::new(1, 30).unwrap(),
                ),
            ),
            (
                (
                    Date::new(2027, 6, 9).unwrap(),
                    TimeOfDay::new(12, 0).unwrap(),
                ),
                (
                    Date::new(2027, 6, 11).unwrap(),
                    TimeOfDay::new(12, 0).unwrap(),
                ),
            ),
        ],
        from: case.from,
        until: case.until,
    };
    assert_blackout_agrees(&blackout);
}

#[test]
fn a_window_at_the_ends_of_the_supported_range_agrees_with_membership() {
    // A padded search range can begin at `Instant::MIN` or end at `Instant::MAX`,
    // where the local date of the first and last minutes falls outside the
    // supported instants in zones twelve hours behind and fourteen ahead.
    for name in ["Etc/GMT+12", "Pacific/Kiritimati", "Australia/Lord_Howe"] {
        for (from, until) in [
            (Instant::MIN, shifted(Instant::MIN, 3 * 1_440)),
            (shifted(Instant::MAX, -3 * 1_440), Instant::MAX),
        ] {
            assert_window_agrees(&window_case_around(name, PlanZone::Named, from, until));

            let blackout = BlackoutCase {
                zone: PlanZone::Named,
                zones: resident(name),
                event: name.to_owned(),
                named: name.to_owned(),
                spans: vec![
                    (
                        (
                            Date::new(1974, 12, 31).unwrap(),
                            TimeOfDay::new(22, 0).unwrap(),
                        ),
                        (
                            Date::new(1975, 1, 2).unwrap(),
                            TimeOfDay::new(2, 0).unwrap(),
                        ),
                    ),
                    (
                        (
                            Date::new(2099, 12, 30).unwrap(),
                            TimeOfDay::new(22, 0).unwrap(),
                        ),
                        (
                            Date::new(2101, 1, 2).unwrap(),
                            TimeOfDay::new(2, 0).unwrap(),
                        ),
                    ),
                ],
                from,
                until,
            };
            assert_blackout_agrees(&blackout);
        }
    }
}

#[test]
fn a_date_line_trip_with_a_crossing_range_agrees_with_membership() {
    // UTC-12 to UTC+14 is a twenty-six hour jump: the participant skips a whole
    // civil date, so the crossing range of a date that never occurs still spills
    // into the date the jump lands on. Coming back repeats a label instead.
    let switch = utc(2027, 3, 10, 11, 0); // 2027-03-09T23:00 at UTC-12
    let back = utc(2027, 3, 12, 10, 0); // 2027-03-13T00:00 at UTC+14
    let case = WindowCase {
        zone: PlanZone::Participant,
        zones: ZonePlan {
            home: "Etc/GMT+12".to_owned(),
            periods: vec![("Pacific/Kiritimati".to_owned(), switch, back)],
        },
        event: "Etc/GMT+12".to_owned(),
        named: "Etc/GMT+12".to_owned(),
        weekly: vec![(0b111_1111, AROUND_THE_CLOCK_BOUNDARIES.to_vec())],
        dates: Vec::new(),
        from: shifted(switch, -1_440),
        until: shifted(back, 1_440),
    };
    assert_window_agrees(&case);

    let blackout = BlackoutCase {
        zone: PlanZone::Participant,
        zones: case.zones.clone(),
        event: case.event.clone(),
        named: case.named.clone(),
        spans: vec![(
            (
                Date::new(2027, 3, 9).unwrap(),
                TimeOfDay::new(22, 0).unwrap(),
            ),
            (
                Date::new(2027, 3, 10).unwrap(),
                TimeOfDay::new(2, 0).unwrap(),
            ),
        )],
        from: case.from,
        until: case.until,
    };
    assert_blackout_agrees(&blackout);
}

#[test]
fn a_window_read_over_an_empty_or_inverted_range_is_empty() {
    let case = window_case_around(
        "America/New_York",
        PlanZone::Named,
        utc(2027, 11, 7, 6, 0),
        utc(2027, 11, 7, 6, 0),
    );
    let spec = engine_window(&case, &database().zone(&case.named).unwrap());
    let schedule = engine_schedule(&case.zones);
    for (from, until) in [(case.from, case.until), (case.until, case.from)] {
        let intervals = spec.intervals(&schedule, from, until).unwrap();
        assert!(
            intervals.is_empty(),
            "{from}..{until} built {} spans",
            intervals.spans().len()
        );
    }
}

#[test]
fn zone_leg_edges_on_the_range_bounds_and_touching_legs_agree_with_membership() {
    // The generated cases place leg edges at arbitrary minutes, so an edge
    // landing exactly on `from` or `until` — where `slices` deliberately drops
    // it and `zone_at` alone decides the zone — is effectively never generated,
    // and two legs touching at one instant never are. Both are ordinary shapes
    // for a trip that begins or ends at a search-window bound.
    let home = "America/New_York";
    let from = utc(2027, 3, 8, 0, 0);
    let until = utc(2027, 3, 14, 0, 0);
    let periods = vec![
        // A leg whose first instant is the range's first instant.
        ("Asia/Tokyo".to_owned(), from, shifted(from, 600)),
        // Two legs touching at one instant, so the boundary belongs to the
        // later leg alone.
        (
            "Pacific/Kiritimati".to_owned(),
            shifted(from, 2_000),
            shifted(from, 2_600),
        ),
        (
            "Etc/GMT+12".to_owned(),
            shifted(from, 2_600),
            shifted(from, 3_200),
        ),
        // A leg whose end is the range's exclusive end.
        (
            "Australia/Lord_Howe".to_owned(),
            shifted(until, -600),
            until,
        ),
    ];
    let plan = ZonePlan {
        home: home.to_owned(),
        periods,
    };

    let mut case = window_case_around(home, PlanZone::Participant, from, until);
    case.zones = plan.clone();
    // An override on a date label the backward legs repeat, so it governs two
    // separate stretches of instants, and a crossing range so the spill of a
    // repeated label lands on the label that follows it twice too.
    let first_date = database().zone(home).unwrap().zoned(from).date();
    case.dates = vec![(
        first_date.plus_days(2).unwrap(),
        first_date.plus_days(3).unwrap(),
        vec![(1_380, 120)],
    )];
    assert_window_agrees(&case);

    // The same legs read through a window that is open at every instant, so a
    // boundary the builder mishandles emits a span rather than happening to
    // fall where the hours above are closed.
    let always_open = WindowCase {
        weekly: vec![(0b111_1111, vec![(0, 1_440)])],
        dates: Vec::new(),
        ..case.clone()
    };
    assert_window_agrees(&always_open);

    let blackout = BlackoutCase {
        zone: PlanZone::Participant,
        zones: plan,
        event: home.to_owned(),
        named: home.to_owned(),
        spans: vec![
            (
                (
                    first_date.plus_days(1).unwrap(),
                    TimeOfDay::new(23, 30).unwrap(),
                ),
                (
                    first_date.plus_days(2).unwrap(),
                    TimeOfDay::new(1, 30).unwrap(),
                ),
            ),
            (
                (first_date.plus_days(4).unwrap(), TimeOfDay::MIDNIGHT),
                (first_date.plus_days(5).unwrap(), TimeOfDay::MIDNIGHT),
            ),
        ],
        from,
        until,
    };
    assert_blackout_agrees(&blackout);
}

#[test]
fn windows_and_blackouts_agree_in_zones_outside_the_generated_matrix() {
    // The generated cases draw zone names from nine zones, none of which moves
    // at local midnight, has a sub-hour daylight saving offset, or jumps a whole
    // date label across the date line. A rule read in such a zone is where a
    // boundary at midnight, or a local reading that runs backwards, meets the
    // builder. Each range below contains a transition that behaves that way.
    let cases = [
        // DST ended at local midnight, repeating 23:00-23:59 across the date
        // label.
        ("America/Sao_Paulo", (2018, 2, 15), (2018, 2, 20)),
        // The date label springs forward past midnight, so it has no 00:00.
        ("America/Santiago", (2027, 9, 3), (2027, 9, 7)),
        // Iran's last daylight saving change happened at local midnight.
        ("Asia/Tehran", (2022, 9, 20), (2022, 9, 24)),
        // +12:45, and daylight saving on top of it: every boundary is off the
        // hour and the offset is not a whole hour.
        ("Pacific/Chatham", (2027, 9, 24), (2027, 9, 28)),
        ("Pacific/Chatham", (2027, 4, 2), (2027, 4, 6)),
        // Samoa skipped 30 December 2011 entirely.
        ("Pacific/Apia", (2011, 12, 28), (2012, 1, 2)),
        // A two-hour jump, and a 2027 transition inside the range.
        ("Antarctica/Troll", (2027, 3, 26), (2027, 3, 30)),
        // Egypt reinstated daylight saving with a midnight transition.
        ("Africa/Cairo", (2023, 4, 27), (2023, 5, 2)),
    ];
    for (name, (y0, m0, d0), (y1, m1, d1)) in cases {
        let from = utc(y0, m0, d0, 0, 0);
        let until = utc(y1, m1, d1, 0, 0);
        let mut window = window_case_around(name, PlanZone::Named, from, until);
        // A `24:00` end every day, so the day's boundary is the local midnight a
        // midnight transition may skip or repeat, and a crossing range on the
        // dates the range touches, so a spill end lands near it too.
        window.weekly = vec![(0b111_1111, vec![(90, 210), (480, 1_440)])];
        let first_date = database().zone(name).unwrap().zoned(from).date();
        window.dates = vec![(
            first_date,
            first_date.plus_days(1).unwrap(),
            vec![(0, 60), (1_320, 120)],
        )];
        assert_window_agrees(&window);

        let blackout = BlackoutCase {
            zone: PlanZone::Named,
            zones: resident(name),
            event: name.to_owned(),
            named: name.to_owned(),
            spans: vec![
                (
                    (first_date, TimeOfDay::new(22, 0).unwrap()),
                    (
                        first_date.plus_days(2).unwrap(),
                        TimeOfDay::new(2, 0).unwrap(),
                    ),
                ),
                (
                    (
                        first_date.plus_days(3).unwrap(),
                        TimeOfDay::new(1, 30).unwrap(),
                    ),
                    (
                        first_date.plus_days(3).unwrap(),
                        TimeOfDay::new(1, 45).unwrap(),
                    ),
                ),
            ],
            from,
            until,
        };
        assert_blackout_agrees(&blackout);
    }
}

/// Zones whose offset is not a whole hour — `-03:30` with a `-02:30` daylight
/// variant, `+05:45` with no daylight saving at all, and `+12:45` with `+13:45` in
/// summer — read against a range that crosses midnight plus several disjoint ranges
/// a day.
///
/// Every boundary is off the hour relative to UTC, the crossing range's end has to
/// be collected on the next local date, and one date carries an override with a
/// different set of ranges, so a breakpoint builder that assumes whole-hour offsets
/// or one range per day disagrees with instant membership here.
#[test]
fn off_hour_zones_agree_with_membership_across_midnight_crossing_ranges() {
    // 02:00-04:00, 10:00-11:00 and 22:00-02:00 every day.
    const SPILLING: &[(u16, u16)] = &[(120, 240), (600, 660), (1_320, 120)];
    let cases = [
        // Daylight saving starts at 02:00 local on 2027-03-14.
        ("America/St_Johns", (2027, 3, 12), (2027, 3, 16)),
        // and ends at 02:00 local on 2027-11-07.
        ("America/St_Johns", (2027, 11, 5), (2027, 11, 9)),
        // A constant +05:45: only the offset is unusual, so the crossing end lands
        // 15 minutes into the next UTC hour all year.
        ("Asia/Kathmandu", (2027, 6, 1), (2027, 6, 5)),
        // +13:45 until daylight saving ends at 03:45 local on 2027-04-04.
        ("Pacific/Chatham", (2027, 4, 2), (2027, 4, 6)),
        // and +12:45 until it starts at 02:45 local on 2027-09-26.
        ("Pacific/Chatham", (2027, 9, 24), (2027, 9, 28)),
    ];
    for (name, (y0, m0, d0), (y1, m1, d1)) in cases {
        let from = utc(y0, m0, d0, 0, 0);
        let until = utc(y1, m1, d1, 0, 0);
        let zone = database().zone(name).unwrap();
        let overridden = zone.zoned(from).date().plus_days(1).unwrap();
        let case = WindowCase {
            zone: PlanZone::Named,
            zones: resident(name),
            event: name.to_owned(),
            named: name.to_owned(),
            weekly: vec![(0b111_1111, SPILLING.to_vec())],
            // One date replaced by three short ranges of its own, one of them
            // crossing midnight as well.
            dates: vec![(
                overridden,
                overridden,
                vec![(0, 45), (690, 705), (1_380, 60)],
            )],
            from,
            until,
        };
        assert_window_agrees(&case);
    }
}
