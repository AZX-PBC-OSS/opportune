//! Differential probe for the occurrence hull a participant-zone cap needs.
//!
//! A cap buckets an existing event by the date its own start reads in the zone in
//! force at that instant, not by the zone in force during the search window. Two
//! instants the participant reads in different zones can therefore share one
//! bucket: a booking during a leg and an existing event after the participant is
//! home again, later in the same day, week or month. This probe builds that shape
//! from the outside in — a leg over the window, a candidate inside it and a
//! counted event outside it whose own-zone label matches — and checks the two
//! things the range answer promises: the returned occurrence hull contains the
//! counted event, and removing data outside the returned ranges leaves the
//! availability answer alone.
//!
//! The generator cannot be relied on for this shape: it places events at small
//! offsets around the window and cannot arrange the shared label across two
//! zones, which is why this is a probe rather than one more property over
//! generated cases.

use std::sync::OnceLock;

use opportune::Engine;
use opportune::model::{
    Block, Cap, CapMeasure, CapPer, CapPeriod, Explain, Goal, Occurrence, Participant, RangesQuery,
    Request, Requirement, Rule, RuleKind, Span, Term, ZonePeriod,
};
use opportune::reasons::{ReasonCode, ReasonParams};
use opportune::tags::{Selector, Tag};
use opportune::time::{
    Date, Duration, Instant, LocalResolution, RuleZone, TimeOfDay, TimeZoneDatabase, Weekday, Zone,
    ZoneName, ZoneSchedule,
};
use opportune_testkit::generators::ZONE_MATRIX;
use proptest::prelude::*;

fn database() -> &'static TimeZoneDatabase {
    static DATABASE: OnceLock<TimeZoneDatabase> = OnceLock::new();
    DATABASE.get_or_init(|| TimeZoneDatabase::bundled().expect("the bundled database is present"))
}

fn engine() -> &'static Engine {
    static ENGINE: OnceLock<Engine> = OnceLock::new();
    ENGINE.get_or_init(|| {
        Engine::builder()
            .build()
            .expect("the bundled database builds")
    })
}

fn tag(path: &str) -> Tag {
    Tag::new(path).expect("a fixture tag is a valid tag")
}

/// The instant of a UTC date-time.
fn utc(day: Date, hour: u8, minute: u8) -> Instant {
    let LocalResolution::Unique(instant) = Zone::utc()
        .resolve_local(day, TimeOfDay::new(hour, minute).expect("a fixture time"))
        .expect("the fixture instant is inside the supported range")
    else {
        panic!("UTC has no transitions");
    };
    instant
}

/// Monday-first index of a weekday, written out so the label derivation here is
/// independent of the engine's.
fn weekday_index(day: Weekday) -> i64 {
    match day {
        Weekday::Monday => 0,
        Weekday::Tuesday => 1,
        Weekday::Wednesday => 2,
        Weekday::Thursday => 3,
        Weekday::Friday => 4,
        Weekday::Saturday => 5,
        Weekday::Sunday => 6,
    }
}

/// The bucket label a cap reads for `date`: the day itself, the start of its
/// seven-day week, or the first of its month.
fn label(date: Date, period: CapPeriod, week_starts: Weekday) -> Date {
    match period {
        CapPeriod::Day => date,
        CapPeriod::Week => {
            let offset = weekday_index(date.weekday()) - weekday_index(week_starts);
            date.plus_days(-offset.rem_euclid(7))
                .expect("a date near 2027 can step back a few days")
        }
        CapPeriod::Month => Date::new(date.year(), date.month(), 1).expect("the first of a month"),
        // `CapPeriod` is `#[non_exhaustive]`; these fixtures only build 0.1 periods.
        _ => unreachable!("these fixtures only build the cap periods 0.1 evaluates"),
    }
}

fn weekday() -> impl Strategy<Value = Weekday> {
    proptest::sample::select(&[
        Weekday::Monday,
        Weekday::Tuesday,
        Weekday::Wednesday,
        Weekday::Thursday,
        Weekday::Friday,
        Weekday::Saturday,
        Weekday::Sunday,
    ])
}

fn zone_name() -> impl Strategy<Value = String> {
    proptest::sample::select(&ZONE_MATRIX).prop_map(str::to_owned)
}

/// The fixed window, its one candidate, and the start of a counted event `after`
/// minutes past the window.
fn window_bounds(after: u32) -> (Instant, Instant, Instant, Instant, Instant) {
    let day = Date::new(2027, 6, 16).expect("a fixture date");
    let start = utc(day, 0, 0);
    let end = Instant::from_minutes(start.minutes() + 6 * 60).expect("a supported instant");
    let candidate = Instant::from_minutes(start.minutes() + 5 * 60).expect("a supported instant");
    let event_start =
        Instant::from_minutes(end.minutes() + i64::from(after)).expect("a supported instant");
    let event_end = Instant::from_minutes(event_start.minutes() + 30).expect("a supported instant");
    (start, end, candidate, event_start, event_end)
}

/// The bucket labels the candidate (read in the leg zone) and the counted event
/// (read at home) fall in.
fn labels(
    period: CapPeriod,
    week_starts: Weekday,
    home: &str,
    leg: &str,
    after: u32,
) -> (Date, Date) {
    let (_, _, candidate, event_start, _) = window_bounds(after);
    (
        label(
            candidate
                .zoned(
                    &database()
                        .zone(leg)
                        .expect("the leg zone is in the database"),
                )
                .date(),
            period,
            week_starts,
        ),
        label(
            event_start
                .zoned(
                    &database()
                        .zone(home)
                        .expect("the home zone is in the database"),
                )
                .date(),
            period,
            week_starts,
        ),
    )
}

/// Remove every block and occurrence that overlaps neither returned envelope.
fn stripped(request: &Request, ranges: &opportune::model::RequiredRanges) -> Request {
    let mut stripped = request.clone();
    let occurrences = ranges.occurrences.expect("a cap applies");
    let overlaps = |from: Instant, until: Instant| {
        let calendar = ranges.calendar;
        let in_calendar = from < calendar.end() && calendar.start() < until;
        let in_occurrences = from < occurrences.end() && occurrences.start() < until;
        in_calendar || in_occurrences
    };
    for participant in &mut stripped.participants {
        if let Some(blocks) = &mut participant.calendar {
            blocks.retain(|block| overlaps(block.span.start(), block.span.end()));
        }
    }
    stripped
        .occurrences
        .retain(|occurrence| overlaps(occurrence.span.start(), occurrence.span.end()));
    stripped
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]

    #[test]
    fn the_occurrence_hull_holds_a_counted_event_outside_the_window(
        period in prop_oneof![
            Just(CapPeriod::Day),
            Just(CapPeriod::Week),
            Just(CapPeriod::Month),
        ],
        week_starts in weekday(),
        home in zone_name(),
        leg in zone_name(),
        after in 30u32..=1_440,
        as_occurrence in any::<bool>(),
        keyed in any::<bool>(),
    ) {
        let (start, end, candidate, event_start, event_end) = window_bounds(after);

        // The candidate is read in the leg zone and the counted event at home.
        // Where the two readings share a label the cap must count the event; where
        // they do not, the event belongs to another bucket and must not. Asserting
        // both directions keeps every case meaningful and cross-checks the
        // engine's own label derivation against this test's.
        let (candidate_label, event_label) = labels(period, week_starts, &home, &leg, after);
        let shared = candidate_label == event_label;

        let mut participant = Participant::new(
            "ana",
            ZoneName::new(&home).expect("the home zone is a zone name"),
        );
        participant.zone_periods = vec![
            ZonePeriod::new(
                ZoneName::new(&leg).expect("the leg zone is a zone name"),
                start,
                end,
            )
            .expect("the leg is non-empty"),
        ];
        if !as_occurrence {
            let mut block = Block::new(Span::new(event_start, event_end).expect("a block span"));
            block.tags = vec![tag("interview")];
            if keyed {
                block.event_key = Some("evt_91".to_owned());
            }
            participant.calendar = Some(vec![block]);
        }

        let mut goal = Goal::single(
            ZoneName::new("UTC").expect("a zone name"),
            Span::new(start, end).expect("the window is non-empty"),
            Duration::from_minutes(30),
            Requirement::new(vec![Term::person("ana")]),
        );
        goal.candidates = Some(vec![candidate]);
        goal.parts[0].tags = vec![tag("interview")];
        let mut request = Request::new(start, goal);
        request.participants.push(participant);
        request.rules.push(Rule::new(
            "cap",
            RuleKind::Cap(Cap::new(
                Selector::Tag(tag("interview")),
                CapMeasure::Count,
                1,
                period,
                CapPer::Each,
                RuleZone::Participant,
                week_starts,
            )),
        ));
        if as_occurrence {
            let mut occurrence = Occurrence::new(
                "evt_91",
                Span::new(event_start, event_end).expect("an occurrence span"),
                vec!["ana".to_owned()],
            )
            .expect("the occurrence names a participant");
            occurrence.tags = vec![tag("interview")];
            request.occurrences.push(occurrence);
        }
        request.explain = Some(Explain::new(10_000));

        let response = engine().find_availability(&request).expect("the request is valid");
        let mut query = RangesQuery::new(request.now, request.goal.clone());
        query.rules.clone_from(&request.rules);
        query.participants.clone_from(&request.participants);
        let ranges = engine().required_ranges(&query).expect("the query resolves");

        if !shared {
            // A different bucket: the counted event must not exclude the start.
            prop_assert_eq!(
                response.offers.len(),
                1,
                "an event in another bucket must not fill this one"
            );
            return Ok(());
        }

        // The engine keys the counted event to the candidate's bucket, so the
        // start is excluded and its reason names that bucket.
        prop_assert!(
            response.offers.is_empty(),
            "the bucket already holds the counted event"
        );
        let reason = response.excluded[0]
            .reasons
            .iter()
            .find(|reason| reason.code == ReasonCode::CapReached)
            .expect("the cap excludes the start");
        prop_assert_eq!(
            reason.params.as_ref().and_then(|params| match params {
                ReasonParams::CapReached { bucket, .. } => Some(bucket.clone()),
                _ => None,
            }),
            Some(candidate_label.to_string())
        );

        // The hull must include the counted event even though it lies outside the
        // search window and is read in another zone.
        let occurrences = ranges.occurrences.expect("a cap applies");
        prop_assert!(
            occurrences.start() <= event_start && event_start < occurrences.end(),
            "the occurrence hull {occurrences:?} excludes the counted event at {event_start}"
        );

        // And stripping data outside both envelopes cannot change the answer.
        let fetched = stripped(&request, &ranges);
        let stripped_response = engine()
            .find_availability(&fetched)
            .expect("the stripped request is valid");
        prop_assert_eq!(&stripped_response.offers, &response.offers);
        prop_assert_eq!(&stripped_response.excluded, &response.excluded);
        prop_assert_eq!(&stripped_response.dates, &response.dates);
    }
}

/// The property above asserts the hull whenever the candidate and the counted
/// event share a bucket, and the plain exclusion when they do not. This pins that
/// the shared-bucket shape really occurs across the probe's parameter space, so a
/// drift that made it vanish cannot leave the hull assertion passing on the
/// "different bucket" branch alone.
#[test]
fn the_probe_reaches_the_shared_bucket_shape() {
    const WEEKDAY: [Weekday; 7] = [
        Weekday::Monday,
        Weekday::Tuesday,
        Weekday::Wednesday,
        Weekday::Thursday,
        Weekday::Friday,
        Weekday::Saturday,
        Weekday::Sunday,
    ];
    let mut shared = 0_usize;
    for period in [CapPeriod::Day, CapPeriod::Week, CapPeriod::Month] {
        for week_starts in WEEKDAY {
            for home in ZONE_MATRIX {
                for leg in ZONE_MATRIX {
                    for after in [30_u32, 60, 360, 720, 1_440] {
                        let (candidate_label, event_label) =
                            labels(period, week_starts, home, leg, after);
                        if candidate_label == event_label {
                            shared += 1;
                        }
                    }
                }
            }
        }
    }
    assert!(
        shared >= 2_000,
        "only {shared} shared-bucket combination(s): the hull assertion would rarely run"
    );
}

/// One fixture schedule: the trips it takes, each a zone and the hours it spans
/// relative to the window's start.
type Trips = Vec<(&'static str, i64, i64)>;

/// The bucket a label `period` after `date` ends at, derived here rather than by
/// the engine.
fn bucket_after(date: Date, period: CapPeriod) -> Date {
    match period {
        CapPeriod::Day => date.plus_days(1),
        CapPeriod::Week => date.plus_days(7),
        CapPeriod::Month if date.month() == 12 => Date::new(date.year() + 1, 1, 1),
        CapPeriod::Month => Date::new(date.year(), date.month() + 1, 1),
        // `CapPeriod` is `#[non_exhaustive]`; these fixtures only build 0.1 periods.
        _ => unreachable!("these fixtures only build the cap periods 0.1 evaluates"),
    }
    .expect("a fixture bucket starts inside the calendar")
}

/// The schedule `trips` describe, with the instants read from `window_start`.
fn fixture_schedule(home: &str, window_start: Instant, trips: &Trips) -> ZoneSchedule {
    let db = database();
    opportune::time::ZoneSchedule::new(
        db.zone(home).expect("the home zone is in the database"),
        trips
            .iter()
            .map(|(zone, from, until)| {
                opportune::time::ZonePeriod::new(
                    db.zone(zone).expect("the trip zone is in the database"),
                    offset(window_start, *from),
                    offset(window_start, *until),
                )
                .expect("the fixture trips are disjoint")
            })
            .collect(),
    )
    .expect("the fixture trips are disjoint")
}

/// `window_start` shifted by whole hours.
fn offset(window_start: Instant, hours: i64) -> Instant {
    Instant::from_minutes(window_start.minutes() + hours * 60).expect("a supported instant")
}

/// The first and last minute in `window_start..window_end` whose own-zone date,
/// read in `schedule`'s zone at that instant, is inside the bucket labels the
/// window touches for `period` and `week_starts`.
///
/// This is the definition of the hull rather than a second implementation of it:
/// it asks each minute which zone the participant is in and which date that zone
/// reads, so a hull built from the schedule's runs has to agree whatever the run
/// boundaries, the gaps between trips, or a repeated local midnight do.
fn walked_hull(
    schedule: &ZoneSchedule,
    window: (Instant, Instant),
    period: CapPeriod,
    week_starts: Weekday,
) -> ((i64, i64), (Date, Date)) {
    let date_at = |minute: i64| {
        let instant = Instant::from_minutes(minute).expect("a minute inside the walk");
        schedule.zone_at(instant).zoned(instant).date()
    };
    let mut first: Option<Date> = None;
    let mut last: Option<Date> = None;
    for minute in window.0.minutes()..window.1.minutes() {
        let date = date_at(minute);
        first = Some(first.map_or(date, |seen| seen.min(date)));
        last = Some(last.map_or(date, |seen| seen.max(date)));
    }
    let (first, last) = (
        first.expect("the window is non-empty"),
        last.expect("the window is non-empty"),
    );
    let label_min = label(first, period, week_starts);
    let label_after = bucket_after(label(last, period, week_starts), period);
    // Wide enough to hold a month bucket either side of the window, at any
    // offset: the walk is a search, so its bounds only have to contain the answer.
    let walk = offset(window.0, -45 * 24).minutes()..=offset(window.0, 45 * 24).minutes();
    let first_in = walk
        .clone()
        .find(|minute| date_at(*minute) >= label_min)
        .expect("the window's own minutes read a date in the label range");
    let last_in = walk
        .clone()
        .rev()
        .find(|minute| date_at(*minute) < label_after)
        .expect("the window's own minutes read a date in the label range");
    ((first_in, last_in + 1), (label_min, label_after))
}

/// The occurrence hull of one participant-zone cap for `schedule` must be exactly
/// the minutes whose own-zone date falls in a bucket the window touches.
fn assert_hull_is_the_minute_walk(name: &str, window_date: Date, home: &str, trips: &Trips) {
    let window_start = utc(window_date, 9, 0);
    let window_end = offset(window_start, 3);
    let schedule = fixture_schedule(home, window_start, trips);

    let mut participant =
        Participant::new("ana", ZoneName::new(home).expect("a fixture zone name"));
    participant.zone_periods = trips
        .iter()
        .map(|(zone, from, until)| {
            ZonePeriod::new(
                ZoneName::new(zone).expect("a fixture zone"),
                offset(window_start, *from),
                offset(window_start, *until),
            )
            .expect("the fixture trips are disjoint")
        })
        .collect();

    for (period, week_starts) in [
        (CapPeriod::Day, Weekday::Monday),
        (CapPeriod::Week, Weekday::Monday),
        (CapPeriod::Week, Weekday::Sunday),
        (CapPeriod::Month, Weekday::Monday),
    ] {
        let (expected, (label_min, label_after)) =
            walked_hull(&schedule, (window_start, window_end), period, week_starts);

        let mut goal = Goal::single(
            ZoneName::new("UTC").expect("a zone name"),
            Span::new(window_start, window_end).expect("the window is non-empty"),
            Duration::from_minutes(30),
            Requirement::new(vec![Term::person("ana")]),
        );
        goal.parts[0].tags = vec![tag("interview")];
        let mut query = RangesQuery::new(window_start, goal);
        query.participants.push(participant.clone());
        query.rules.push(Rule::new(
            "cap",
            RuleKind::Cap(Cap::new(
                Selector::Tag(tag("interview")),
                CapMeasure::Count,
                1,
                period,
                CapPer::Each,
                RuleZone::Participant,
                week_starts,
            )),
        ));

        let hull = engine()
            .required_ranges(&query)
            .unwrap_or_else(|err| panic!("{name}: the fixture query resolves: {err}"))
            .occurrences
            .expect("the cap applies");
        assert_eq!(
            (hull.start().minutes(), hull.end().minutes()),
            expected,
            "{name}, {period:?} starting {week_starts:?}: the hull is not the minutes whose \
             own-zone date is in {label_min}..{label_after}"
        );
    }
}

/// The hull of every bucket shape, over schedules whose runs repeat, touch, leave
/// gaps and cross offset transitions.
#[test]
fn the_hull_is_every_minute_whose_own_zone_date_is_in_a_touched_bucket() {
    let june = Date::new(2027, 6, 15).expect("a fixture date");
    // The day London springs forward at 01:00, its autumn counterpart, and the
    // day Lord Howe moves by half an hour. A trip that spans one of them puts a
    // real offset change inside the bucket labels the window touches.
    let spring = Date::new(2027, 3, 28).expect("a fixture date");
    let autumn = Date::new(2027, 10, 31).expect("a fixture date");
    // Cuba ends daylight saving at 01:00 wall time (00:00 standard), so local
    // midnight of 2018-11-04 happens twice and the hour between the two midnights
    // already reads the 4th: a day bucket ending on the 3rd ends at the first one.
    let midnight_fold = Date::new(2018, 11, 3).expect("a fixture date");
    // A zone repeated after a gap is the shape a hull built from global per-zone
    // extremes would get wrong; touching periods and a trip across a transition
    // are the edges a run's own boundaries create.
    let fixtures: [(&str, Date, &str, Trips); 10] = [
        ("home only", june, "Europe/London", Vec::new()),
        (
            "a midnight-starting fold",
            midnight_fold,
            "America/Havana",
            Vec::new(),
        ),
        (
            "one trip",
            june,
            "Europe/London",
            vec![("Asia/Tokyo", -24, 48)],
        ),
        (
            "out and back",
            june,
            "Europe/London",
            vec![("Asia/Tokyo", 2, 6), ("America/New_York", 10, 14)],
        ),
        (
            "touching periods",
            june,
            "Europe/London",
            vec![("Asia/Tokyo", -2, 2), ("Asia/Tokyo", 2, 6)],
        ),
        (
            "two trips to one zone",
            june,
            "Europe/London",
            vec![
                ("Pacific/Kiritimati", -72, -48),
                ("Pacific/Kiritimati", 48, 72),
            ],
        ),
        (
            "a trip whose zone takes its own transition",
            spring,
            "Europe/London",
            vec![("America/New_York", -15 * 24, 15 * 24)],
        ),
        (
            "a trip across the window's own transition",
            spring,
            "America/New_York",
            vec![("Europe/London", -15 * 24, 15 * 24)],
        ),
        (
            "a half-hour transition",
            autumn,
            "Australia/Lord_Howe",
            vec![("Europe/London", -15 * 24, 15 * 24)],
        ),
        (
            "a trip that ends where the bucket starts",
            june,
            "Europe/London",
            vec![("Asia/Tokyo", -48, -18), ("Asia/Tokyo", 0, 18)],
        ),
    ];
    for (name, window_date, home, trips) in &fixtures {
        assert_hull_is_the_minute_walk(name, *window_date, home, trips);
    }
}

/// Cuba ended daylight saving at 01:00 wall time (00:00 standard), so local
/// midnight of 2018-11-04 happened twice and the hour between the two midnights
/// already read the 4th. A day bucket ending on the 3rd therefore ends at the
/// *first* midnight: a hull stretched to the second one would hand an adapter an
/// hour whose own-zone date belongs to the next bucket.
#[test]
fn a_hull_ending_on_a_midnight_starting_fold_stops_at_the_first_midnight() {
    let date = Date::new(2018, 11, 3).expect("a fixture date");
    let next = Date::new(2018, 11, 4).expect("a fixture date");
    let havana = database().zone("America/Havana").expect("the fixture zone");
    let LocalResolution::Repeated(first, second) = havana
        .resolve_local(next, TimeOfDay::MIDNIGHT)
        .expect("the fixture midnight resolves")
    else {
        panic!("2018-11-04 begins twice in Havana");
    };
    assert_eq!(second.minutes() - first.minutes(), 60);
    assert_eq!(
        havana.zoned(second).date(),
        next,
        "the second midnight already reads the next date"
    );

    let window = (utc(date, 9, 0), offset(utc(date, 9, 0), 3));
    let no_trips = Trips::new();
    let schedule = fixture_schedule(havana.name(), window.0, &no_trips);
    let (expected, (label_min, label_after)) =
        walked_hull(&schedule, window, CapPeriod::Day, Weekday::Monday);
    assert_eq!(
        expected,
        (
            havana
                .day_start(date)
                .expect("the 3rd begins at a midnight")
                .minutes(),
            first.minutes()
        ),
        "the minute walk should be the whole local day of {label_min} up to {label_after}"
    );

    let participant = Participant::new("ana", ZoneName::new("America/Havana").expect("a name"));
    let mut goal = Goal::single(
        ZoneName::new("UTC").expect("a zone name"),
        Span::new(window.0, window.1).expect("the window is non-empty"),
        Duration::from_minutes(30),
        Requirement::new(vec![Term::person("ana")]),
    );
    goal.parts[0].tags = vec![tag("interview")];
    let mut query = RangesQuery::new(window.0, goal);
    query.participants.push(participant);
    query.rules.push(Rule::new(
        "cap",
        RuleKind::Cap(Cap::new(
            Selector::Tag(tag("interview")),
            CapMeasure::Count,
            1,
            CapPeriod::Day,
            CapPer::Each,
            RuleZone::Participant,
            Weekday::Monday,
        )),
    ));
    let hull = engine()
        .required_ranges(&query)
        .expect("the fixture query resolves")
        .occurrences
        .expect("the cap applies");
    assert_eq!(
        (hull.start().minutes(), hull.end().minutes()),
        expected,
        "the day hull must end at the first midnight {first}, not the repeated one {second}"
    );
}

/// The occurrence hull of a participant-zone cap whose window opens at the first
/// supported instant, walked minute by minute from that instant.
///
/// The run list a hull is read from starts at `Instant::MIN`, and both a bucket's
/// start and its end are clamped there, so this is the edge where a hull built
/// from runs rather than from legs has to agree with the definition: the local day
/// Tokyo reads at the range's first instant began before the range does, so that
/// zone's contribution is clamped to `Instant::MIN` instead of to a midnight
/// outside it.
#[test]
fn a_window_at_the_start_of_the_range_hulls_the_minutes_it_can_hold() {
    let period = CapPeriod::Day;
    let week_starts = Weekday::Monday;
    let window = (Instant::MIN, offset(Instant::MIN, 6));
    // Home only, and a trip that begins at the range's first instant so the
    // schedule holds no leading home run at all.
    let cases: [(&str, Trips); 2] = [
        ("UTC", Vec::new()),
        ("Europe/London", vec![("Asia/Tokyo", 0, 24)]),
    ];
    for (home, trips) in cases {
        let schedule = fixture_schedule(home, Instant::MIN, &trips);
        let mut participant =
            Participant::new("ana", ZoneName::new(home).expect("a fixture zone name"));
        participant.zone_periods = trips
            .iter()
            .map(|(zone, from, until)| {
                ZonePeriod::new(
                    ZoneName::new(zone).expect("a fixture zone"),
                    offset(Instant::MIN, *from),
                    offset(Instant::MIN, *until),
                )
                .expect("the fixture trips are disjoint")
            })
            .collect();

        // The definition, not a second hull: the local dates the window touches,
        // then the first and last minute whose own-zone date is in their bucket.
        // Three days holds a one-day label range at any offset, and a walk cannot
        // begin before the range does.
        let date_at = |minute: i64| {
            let instant = Instant::from_minutes(minute).expect("a minute inside the walk");
            schedule.zone_at(instant).zoned(instant).date()
        };
        let mut first: Option<Date> = None;
        let mut last: Option<Date> = None;
        for minute in window.0.minutes()..window.1.minutes() {
            let date = date_at(minute);
            first = Some(first.map_or(date, |seen| seen.min(date)));
            last = Some(last.map_or(date, |seen| seen.max(date)));
        }
        let label_min = label(first.expect("the window is non-empty"), period, week_starts);
        let label_after = bucket_after(
            label(last.expect("the window is non-empty"), period, week_starts),
            period,
        );
        let walk = Instant::MIN.minutes()..=Instant::MIN.minutes() + 3 * 24 * 60;
        let first_in = walk
            .clone()
            .find(|minute| date_at(*minute) >= label_min)
            .expect("the window's own minutes read a date in the label range");
        let last_in = walk
            .clone()
            .rev()
            .find(|minute| date_at(*minute) < label_after)
            .expect("the window's own minutes read a date in the label range");

        let mut goal = Goal::single(
            ZoneName::new("UTC").expect("a zone name"),
            Span::new(window.0, window.1).expect("the window is non-empty"),
            Duration::from_minutes(30),
            Requirement::new(vec![Term::person("ana")]),
        );
        goal.parts[0].tags = vec![tag("interview")];
        let mut query = RangesQuery::new(window.0, goal);
        query.participants.push(participant);
        query.rules.push(Rule::new(
            "cap",
            RuleKind::Cap(Cap::new(
                Selector::Tag(tag("interview")),
                CapMeasure::Count,
                1,
                period,
                CapPer::Each,
                RuleZone::Participant,
                week_starts,
            )),
        ));
        let hull = engine()
            .required_ranges(&query)
            .unwrap_or_else(|err| panic!("{home}: the fixture query resolves: {err}"))
            .occurrences
            .expect("the cap applies");
        assert_eq!(
            (hull.start().minutes(), hull.end().minutes()),
            (first_in, last_in + 1),
            "{home}: the hull is not the minutes whose own-zone date is in \
             {label_min}..{label_after}"
        );
    }
}
