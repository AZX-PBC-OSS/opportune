//! Differential probes for the two halves of a cap's fetch closure.
//!
//! **Occurrence-hull half.** A cap buckets an existing event by the date its own
//! start reads in the zone in force at that instant, not by the zone in force
//! during the search window. Two instants the participant reads in different
//! zones can therefore share one bucket: a booking during a leg and an existing
//! event after the participant is home again, later in the same day, week or
//! month. These probes build that shape from the outside in — a leg over the
//! window, a candidate inside it and a counted event outside it whose own-zone
//! label matches — and check the two things the range answer promises: the
//! returned occurrence hull contains the counted event, and removing data outside
//! the returned ranges leaves the availability answer alone.
//!
//! **Key-closure half.** A cap merges every record sharing an event key across all
//! supplied time ranges, so the initial hulls are not key-complete. A record just
//! outside a hull can extend an event into the candidate's bucket, or move its
//! start out of it, and it may be a block, an occurrence or an all-day record the
//! hull did not select. These fixtures build each shape from the outside in, prove
//! the one-pass fetch diverges, and require the documented two-phase fetch —
//! initial hulls plus one round of event-key closure — to match the full answer.
//!
//! The generator cannot be relied on for either shape: it places events at small
//! offsets around the window and cannot arrange a shared label across two zones or
//! a record carrying an observed key outside the hull, which is why these are
//! probes rather than properties over generated cases.

use std::sync::OnceLock;

use opportune::Engine;
use opportune::model::{
    Block, Cap, CapMeasure, CapPer, CapPeriod, Explain, Goal, Occurrence, OfferFlag, Participant,
    Proposal, RangesQuery, Request, RequiredRanges, Requirement, ReserveSpan, Response, Rule,
    RuleKind, Span, Term, ZonePeriod,
};
use opportune::reasons::{Reason, ReasonCode, ReasonParams};
use opportune::tags::{Selector, Tag};
use opportune::time::{
    Date, Duration, Instant, LocalResolution, RuleZone, TimeOfDay, TimeZoneDatabase, Weekday, Zone,
    ZoneName, ZoneSchedule,
};
use opportune_testkit::fetch::{key_closed_request, one_pass_request};
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

/// The request after the documented two-phase fetch: the initial calendar and
/// occurrence hulls plus one round of event-key closure.
fn stripped(request: &Request, ranges: &RequiredRanges) -> Request {
    key_closed_request(request, ranges)
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

// ---------------------------------------------------------------------------
// Key-closure half: cap inputs are key-complete, not one-pass (see the header).
// ---------------------------------------------------------------------------

/// The ranges the engine asks for `request`.
fn required_ranges_of(request: &Request) -> RequiredRanges {
    let mut query = RangesQuery::new(request.now, request.goal.clone());
    query.rules.clone_from(&request.rules);
    query.participants.clone_from(&request.participants);
    engine()
        .required_ranges(&query)
        .expect("the fixture query resolves")
}

/// What `SlotCheck` reports, without the warning list: stripping a keyless block
/// outside the hull can legitimately change a warning, but never the answer.
type SlotAnswer = (bool, Vec<Reason>, Vec<OfferFlag>, Vec<ReserveSpan>);

fn slot_answer(request: &Request, start: Instant, hosts: &[&str]) -> SlotAnswer {
    let assignment = vec![hosts.iter().map(|host| (*host).to_owned()).collect()];
    let check = engine()
        .check_slot(request, &Proposal::single("main", start, assignment))
        .expect("the fixture request and proposal are valid");
    (check.valid, check.reasons, check.flags, check.reserve)
}

/// The full request, the documented one-pass fetch, and the documented key-closed
/// fetch, each answered and each checked at `start`.
struct FetchOutcome {
    full: Response,
    one_pass: Response,
    closed: Response,
    full_slot: SlotAnswer,
    one_pass_slot: SlotAnswer,
    closed_slot: SlotAnswer,
    ranges: RequiredRanges,
    one_pass_request: Request,
    closed_request: Request,
}

fn fetch_outcome(request: &Request, start: Instant, hosts: &[&str]) -> FetchOutcome {
    let ranges = required_ranges_of(request);
    let full = engine()
        .find_availability(request)
        .expect("the full fixture request is valid");
    let one_pass_request = one_pass_request(request, &ranges);
    let closed_request = key_closed_request(request, &ranges);
    let one_pass = engine()
        .find_availability(&one_pass_request)
        .expect("the one-pass fixture request is valid");
    let closed = engine()
        .find_availability(&closed_request)
        .expect("the key-closed fixture request is valid");
    let full_slot = slot_answer(request, start, hosts);
    let one_pass_slot = slot_answer(&one_pass_request, start, hosts);
    let closed_slot = slot_answer(&closed_request, start, hosts);
    FetchOutcome {
        full,
        one_pass,
        closed,
        full_slot,
        one_pass_slot,
        closed_slot,
        ranges,
        one_pass_request,
        closed_request,
    }
}

/// The key-closed fetch must answer exactly as the full request did.
fn assert_closed_matches_full(outcome: &FetchOutcome) {
    assert_eq!(outcome.closed.offers, outcome.full.offers, "offers differ");
    assert_eq!(
        outcome.closed.dates, outcome.full.dates,
        "date counts differ"
    );
    assert_eq!(
        outcome.closed.excluded, outcome.full.excluded,
        "exclusions differ"
    );
    assert_eq!(
        outcome.closed_slot, outcome.full_slot,
        "SlotCheck (valid, reasons, flags, reserve) differs"
    );
}

fn interview_cap(measure: CapMeasure, limit: u32, period: CapPeriod) -> Rule {
    Rule::new(
        "cap",
        RuleKind::Cap(Cap::new(
            Selector::Tag(tag("interview")),
            measure,
            limit,
            period,
            CapPer::Each,
            RuleZone::Participant,
            Weekday::Monday,
        )),
    )
}

fn fixture_goal(zone: &str, window: Span, candidate: Instant, host: &str, duration: u32) -> Goal {
    let mut goal = Goal::single(
        ZoneName::new(zone).expect("a fixture zone name"),
        window,
        Duration::from_minutes(duration),
        Requirement::new(vec![Term::person(host)]),
    );
    goal.candidates = Some(vec![candidate]);
    goal.parts[0].tags = vec![tag("interview")];
    goal
}

/// A block over a fixture span, carrying `key` and the `interview` tag.
fn keyed_block(span: Span, key: &str, tagged: bool) -> Block {
    let mut block = Block::new(span);
    block.event_key = Some(key.to_owned());
    if tagged {
        block.tags = vec![tag("interview")];
    }
    block
}

/// A later block sharing a counted event's key extends the event beyond the
/// occurrence hull. Full data charges those minutes to the candidate's bucket and
/// rejects the start; a one-pass fetch never sees them and offers it.
#[test]
fn a_later_out_of_hull_block_extends_a_minutes_capped_event() {
    let day = Date::new(2027, 6, 16).expect("a fixture date");
    // Four days on: past the occurrence hull's day and past the calendar hull's
    // one-day padding either side of the window.
    let later_day = Date::new(2027, 6, 20).expect("a fixture date");
    let window = Span::new(utc(day, 9, 0), utc(day, 15, 0)).expect("a window");
    let candidate = utc(day, 12, 0);

    let mut ana = Participant::new("ana", ZoneName::new("UTC").expect("a zone name"));
    ana.calendar = Some(vec![
        keyed_block(
            Span::new(utc(day, 10, 0), utc(day, 10, 15)).expect("a span"),
            "evt_late",
            true,
        ),
        // Outside both the occurrence day hull and the calendar padding, so a
        // one-pass fetch drops it.
        keyed_block(
            Span::new(utc(later_day, 1, 0), utc(later_day, 1, 30)).expect("a span"),
            "evt_late",
            true,
        ),
    ]);

    let mut request = Request::new(
        utc(day, 9, 0),
        fixture_goal("UTC", window, candidate, "ana", 30),
    );
    request.participants.push(ana);
    request
        .rules
        .push(interview_cap(CapMeasure::Minutes, 60, CapPeriod::Day));

    let outcome = fetch_outcome(&request, candidate, &["ana"]);
    assert_closed_matches_full(&outcome);
    assert!(
        outcome.full.offers.is_empty(),
        "the merged event's minutes fill the bucket: {:?}",
        outcome.full.offers
    );
    assert!(!outcome.full_slot.0, "the full request rejects the start");
    assert_eq!(
        outcome.one_pass.offers.len(),
        1,
        "a one-pass fetch sees only the in-hull copy and offers the start"
    );
    assert!(
        outcome.one_pass_slot.0,
        "the one-pass request accepts the start"
    );
}

/// A block earlier than the occurrence hull shares a counted event's key and
/// moves the event's start into an earlier bucket. Full data offers the start;
/// a one-pass fetch sees only the in-hull copy in the candidate's bucket,
/// reaches the cap, and rejects it.
#[test]
fn an_earlier_out_of_hull_block_moves_a_count_capped_event_to_an_earlier_bucket() {
    let day = Date::new(2027, 6, 16).expect("a fixture date");
    // The Saturday before the Monday that starts the candidate's week.
    let earlier = Date::new(2027, 6, 12).expect("a fixture date");
    let window = Span::new(utc(day, 9, 0), utc(day, 15, 0)).expect("a window");
    let candidate = utc(day, 13, 0);

    let mut ana = Participant::new("ana", ZoneName::new("UTC").expect("a zone name"));
    ana.calendar = Some(vec![
        keyed_block(
            Span::new(utc(day, 10, 0), utc(day, 10, 30)).expect("a span"),
            "evt_early",
            true,
        ),
        keyed_block(
            Span::new(utc(earlier, 10, 0), utc(earlier, 10, 30)).expect("a span"),
            "evt_early",
            true,
        ),
    ]);

    let mut request = Request::new(
        utc(day, 9, 0),
        fixture_goal("UTC", window, candidate, "ana", 30),
    );
    request.participants.push(ana);
    request
        .rules
        .push(interview_cap(CapMeasure::Count, 1, CapPeriod::Week));

    let outcome = fetch_outcome(&request, candidate, &["ana"]);
    assert_closed_matches_full(&outcome);
    assert_eq!(
        outcome.full.offers.len(),
        1,
        "the merged event starts in the previous week, leaving this bucket free"
    );
    assert!(outcome.full_slot.0, "the full request accepts the start");
    assert!(
        outcome.one_pass.offers.is_empty(),
        "a one-pass fetch sees the in-hull copy in this week's bucket and rejects"
    );
    assert!(
        !outcome.one_pass_slot.0,
        "the one-pass request rejects the start"
    );
}

/// An occurrence fixes an event's span and participants, but a same-key all-day
/// block on another participant still contributes its tags. The block lies far
/// outside the hull, so a one-pass fetch loses the counted tag and offers the
/// start that full data rejects.
#[test]
fn an_all_day_copy_on_another_participant_supplies_the_counted_tag() {
    let day = Date::new(2027, 6, 16).expect("a fixture date");
    let window = Span::new(utc(day, 9, 0), utc(day, 15, 0)).expect("a window");
    let candidate = utc(day, 13, 0);
    // Past both the day hull and the calendar hull's one-day padding.
    let far_day = Date::new(2027, 6, 20).expect("a fixture date");
    let far_next = Date::new(2027, 6, 21).expect("a fixture date");

    let mut ana = Participant::new("ana", ZoneName::new("UTC").expect("a zone name"));
    ana.calendar = Some(Vec::new());
    let mut bo = Participant::attendee("bo", ZoneName::new("UTC").expect("a zone name"));
    let mut all_day = keyed_block(
        Span::new(utc(far_day, 0, 0), utc(far_next, 0, 0)).expect("a span"),
        "evt_tag",
        true,
    );
    all_day.all_day = true;
    bo.calendar = Some(vec![all_day]);

    let mut request = Request::new(
        utc(day, 9, 0),
        fixture_goal("UTC", window, candidate, "ana", 30),
    );
    request.participants.push(ana);
    request.participants.push(bo);
    request.occurrences.push(
        Occurrence::new(
            "evt_tag",
            Span::new(utc(day, 10, 0), utc(day, 10, 30)).expect("a span"),
            vec!["ana".to_owned()],
        )
        .expect("the occurrence names a participant"),
    );
    request
        .rules
        .push(interview_cap(CapMeasure::Count, 1, CapPeriod::Day));

    let outcome = fetch_outcome(&request, candidate, &["ana"]);
    assert_closed_matches_full(&outcome);
    assert!(
        outcome.full.offers.is_empty(),
        "the all-day copy's tag brings the occurrence into the cap"
    );
    assert!(!outcome.full_slot.0, "the full request rejects the start");
    assert_eq!(
        outcome.one_pass.offers.len(),
        1,
        "a one-pass fetch drops the all-day copy and loses the counted tag"
    );
    assert!(
        outcome.one_pass_slot.0,
        "the one-pass request accepts the start"
    );
}

/// Event-key closure scans every request participant, not just the ones a cap
/// covers: the in-hull copy of an event sits on an attendee, and the out-of-hull
/// copy that makes it count sits on the cap-covered host. Full data merges them
/// and rejects the start; a one-pass fetch keeps only the attendee's copy, which
/// the cap never counts, and offers it.
#[test]
fn closure_scans_a_participant_no_cap_covers() {
    let day = Date::new(2027, 6, 16).expect("a fixture date");
    // Past both the day hull and the calendar hull's one-day padding.
    let far_day = Date::new(2027, 6, 20).expect("a fixture date");
    let window = Span::new(utc(day, 9, 0), utc(day, 9, 30)).expect("a window");
    let candidate = utc(day, 9, 0);

    // The cap covers hosts, so it covers ana and never bo.
    let mut ana = Participant::new("ana", ZoneName::new("UTC").expect("a zone name"));
    ana.calendar = Some(vec![keyed_block(
        Span::new(utc(far_day, 12, 0), utc(far_day, 12, 30)).expect("a span"),
        "evt_cross",
        true,
    )]);
    let mut bo = Participant::attendee("bo", ZoneName::new("UTC").expect("a zone name"));
    bo.calendar = Some(vec![keyed_block(
        Span::new(utc(day, 12, 0), utc(day, 12, 30)).expect("a span"),
        "evt_cross",
        true,
    )]);

    let mut request = Request::new(
        utc(day, 9, 0),
        fixture_goal("UTC", window, candidate, "ana", 30),
    );
    request.participants.push(ana);
    request.participants.push(bo);
    request
        .rules
        .push(interview_cap(CapMeasure::Count, 1, CapPeriod::Day));

    let outcome = fetch_outcome(&request, candidate, &["ana"]);
    assert_closed_matches_full(&outcome);
    assert!(
        outcome.full.offers.is_empty(),
        "the merged event names the cap-covered host"
    );
    assert!(!outcome.full_slot.0, "the full request rejects the start");
    assert_eq!(
        outcome.one_pass.offers.len(),
        1,
        "a one-pass fetch drops the host's copy and no longer counts the event"
    );
    assert!(
        outcome.one_pass_slot.0,
        "the one-pass request accepts the start"
    );
}

/// A keyless block outside the hull is unrelated data: closure must not keep it,
/// and the fetch stays at the occurrence hull rather than widening to the full
/// supported range.
#[test]
fn a_keyless_out_of_hull_block_stays_stripped_and_the_hull_stays_narrow() {
    let day = Date::new(2027, 6, 16).expect("a fixture date");
    let next = Date::new(2027, 6, 17).expect("a fixture date");
    let far = Date::new(2027, 12, 1).expect("a fixture date");
    let window = Span::new(utc(day, 9, 0), utc(day, 15, 0)).expect("a window");
    let candidate = utc(day, 13, 0);

    let mut ana = Participant::new("ana", ZoneName::new("UTC").expect("a zone name"));
    let mut keyless = Block::new(Span::new(utc(far, 10, 0), utc(far, 10, 30)).expect("a span"));
    keyless.tags = vec![tag("interview")];
    ana.calendar = Some(vec![keyless]);

    let mut request = Request::new(
        utc(day, 9, 0),
        fixture_goal("UTC", window, candidate, "ana", 30),
    );
    request.participants.push(ana);
    request
        .rules
        .push(interview_cap(CapMeasure::Count, 1, CapPeriod::Day));

    let outcome = fetch_outcome(&request, candidate, &["ana"]);
    assert_closed_matches_full(&outcome);
    let hull = outcome.ranges.occurrences.expect("the cap applies");
    assert_eq!(
        (hull.start(), hull.end()),
        (utc(day, 0, 0), utc(next, 0, 0)),
        "the fetch is the day bucket, not the full supported range"
    );
    assert_eq!(
        outcome.one_pass_request, outcome.closed_request,
        "closure must not retain a keyless record outside the hull"
    );
    assert!(
        outcome.closed_request.participants[0]
            .calendar
            .as_ref()
            .is_some_and(Vec::is_empty),
        "the keyless far block is stripped, leaving a known-empty calendar"
    );
    assert_eq!(outcome.full.offers.len(), 1, "the candidate is offered");
}

// The bounded shared-key property: an in-hull record and an out-of-hull record
// carry one event key, and a tag on the out-of-hull record is what makes the
// event count. Closure must retrieve the out-of-hull record, leave an unrelated
// out-of-hull key stripped, and answer exactly as the full request did across
// cap shapes, representations, all-day records and exclusions.
proptest! {
    #![proptest_config(ProptestConfig::with_cases(96))]

    #[test]
    fn key_closure_retrieves_out_of_hull_records_and_matches_full_data(
        period in prop_oneof![
            Just(CapPeriod::Day),
            Just(CapPeriod::Week),
            Just(CapPeriod::Month),
        ],
        measure in prop_oneof![Just(CapMeasure::Count), Just(CapMeasure::Minutes)],
        pooled in any::<bool>(),
        in_hull_is_occurrence in any::<bool>(),
        all_day in any::<bool>(),
        excluded in any::<bool>(),
        zone in prop_oneof![
            Just("UTC"),
            Just("Europe/London"),
            Just("Asia/Tokyo"),
        ],
    ) {
        let day = Date::new(2027, 6, 16).expect("a fixture date");
        let window_start = utc(day, 9, 0);
        let window_end = Instant::from_minutes(window_start.minutes() + 6 * 60)
            .expect("a supported instant");
        let candidate = window_start;
        let in_hull_start = Instant::from_minutes(window_start.minutes() + 2 * 60)
            .expect("a supported instant");
        let in_hull_end = Instant::from_minutes(in_hull_start.minutes() + 60)
            .expect("a supported instant");
        // Far outside any day, week or month bucket the window can touch.
        let far_start = Instant::from_minutes(window_start.minutes() + 400 * 1_440)
            .expect("a supported instant");
        let far_end = Instant::from_minutes(far_start.minutes() + 60)
            .expect("a supported instant");
        let unrelated_start = Instant::from_minutes(far_start.minutes() + 120)
            .expect("a supported instant");
        let unrelated_end = Instant::from_minutes(unrelated_start.minutes() + 60)
            .expect("a supported instant");

        let window = Span::new(window_start, window_end).expect("a window");
        let in_hull = Span::new(in_hull_start, in_hull_end).expect("a span");
        let far = Span::new(far_start, far_end).expect("a span");

        let mut ana = Participant::new("ana", ZoneName::new(zone).expect("a zone name"));
        ana.calendar = Some(vec![keyed_block(
            Span::new(unrelated_start, unrelated_end).expect("a span"),
            "evt_unrelated",
            true,
        )]);
        let mut bo = Participant::attendee("bo", ZoneName::new(zone).expect("a zone name"));
        let mut far_copy = keyed_block(far, "evt_shared", true);
        far_copy.all_day = all_day;
        bo.calendar = Some(vec![far_copy]);
        if !in_hull_is_occurrence {
            ana.calendar.as_mut().expect("a calendar").push(keyed_block(
                in_hull,
                "evt_shared",
                false,
            ));
        }

        let mut request = Request::new(
            window_start,
            fixture_goal(zone, window, candidate, "ana", 30),
        );
        request.participants.push(ana);
        request.participants.push(bo);
        if in_hull_is_occurrence {
            request.occurrences.push(
                Occurrence::new("evt_shared", in_hull, vec!["ana".to_owned()])
                    .expect("the occurrence names a participant"),
            );
        }
        let (per, cap_zone) = if pooled {
            (CapPer::Pooled, RuleZone::Event)
        } else {
            (CapPer::Each, RuleZone::Participant)
        };
        let limit = match measure {
            CapMeasure::Count => 1,
            // The in-hull event holds 60 minutes; the booking adds 30.
            CapMeasure::Minutes => 60,
        };
        request.rules.push(Rule::new(
            "cap",
            RuleKind::Cap(Cap::new(
                Selector::Tag(tag("interview")),
                measure,
                limit,
                period,
                per,
                cap_zone,
                Weekday::Monday,
            )),
        ));
        if excluded {
            request.exclude_event_keys.push("evt_shared".to_owned());
        }

        let outcome = fetch_outcome(&request, candidate, &["ana"]);
        assert_closed_matches_full(&outcome);

        let hull = outcome.ranges.occurrences.expect("the cap applies");
        let far_retrieved = outcome
            .closed_request
            .participants
            .iter()
            .flat_map(Participant::blocks)
            .any(|block| {
                block.event_key.as_deref() == Some("evt_shared")
                    && block.span.start() >= hull.end()
            });
        prop_assert!(
            far_retrieved,
            "closure must retrieve the out-of-hull shared-key record ({:?}..{:?})",
            hull.start(),
            hull.end()
        );
        let unrelated_present = outcome
            .closed_request
            .participants
            .iter()
            .flat_map(Participant::blocks)
            .any(|block| block.event_key.as_deref() == Some("evt_unrelated"));
        prop_assert!(
            !unrelated_present,
            "an out-of-hull record with an unobserved key must stay stripped"
        );
    }
}

/// Without a cap no occurrence data is fetched, and a participant with no
/// calendar stays unknown rather than becoming a known-empty calendar.
#[test]
fn the_fetch_model_preserves_unknown_calendars_and_clears_occurrences_without_a_cap() {
    let day = Date::new(2027, 6, 16).expect("a fixture date");
    let window = Span::new(utc(day, 9, 0), utc(day, 15, 0)).expect("a window");
    let candidate = utc(day, 13, 0);

    let mut request = Request::new(
        utc(day, 9, 0),
        fixture_goal("UTC", window, candidate, "ana", 30),
    );
    // No calendar at all: unknown, never known-empty.
    request.participants.push(Participant::new(
        "ana",
        ZoneName::new("UTC").expect("a zone name"),
    ));
    request.occurrences.push(
        Occurrence::new(
            "evt_nocap",
            Span::new(utc(day, 10, 0), utc(day, 10, 30)).expect("a span"),
            vec!["ana".to_owned()],
        )
        .expect("the occurrence names a participant"),
    );

    let ranges = required_ranges_of(&request);
    assert!(
        ranges.occurrences.is_none(),
        "no cap applies, so no occurrence range is returned"
    );
    let closed = key_closed_request(&request, &ranges);
    assert!(
        closed.participants[0].calendar.is_none(),
        "an unknown calendar must stay unknown"
    );
    assert!(
        closed.occurrences.is_empty(),
        "occurrences are cleared when there is no cap"
    );
}

/// With no cap there is no occurrence hull and therefore no key observation or
/// closure: a same-key copy outside the calendar hull stays stripped even though
/// its in-hull relative would have seeded closure had a cap applied.
#[test]
fn no_cap_does_not_close_over_a_calendar_blocks_key() {
    let day = Date::new(2027, 6, 16).expect("a fixture date");
    let far_day = Date::new(2027, 6, 20).expect("a fixture date");
    let window = Span::new(utc(day, 9, 0), utc(day, 15, 0)).expect("a window");
    let candidate = utc(day, 13, 0);
    let in_hull = keyed_block(
        Span::new(utc(day, 10, 0), utc(day, 10, 30)).expect("a span"),
        "evt_uncapped",
        true,
    );
    let far = keyed_block(
        Span::new(utc(far_day, 10, 0), utc(far_day, 10, 30)).expect("a span"),
        "evt_uncapped",
        true,
    );

    let mut ana = Participant::new("ana", ZoneName::new("UTC").expect("a zone name"));
    ana.calendar = Some(vec![in_hull.clone(), far]);
    let mut request = Request::new(
        utc(day, 9, 0),
        fixture_goal("UTC", window, candidate, "ana", 30),
    );
    request.participants.push(ana);
    request.occurrences.push(
        Occurrence::new("evt_uncapped", in_hull.span, vec!["ana".to_owned()])
            .expect("the occurrence names a participant"),
    );

    let ranges = required_ranges_of(&request);
    assert!(ranges.occurrences.is_none());
    let closed = key_closed_request(&request, &ranges);
    assert!(closed.occurrences.is_empty());
    assert_eq!(
        closed.participants[0].blocks(),
        &[in_hull],
        "without a cap, the far same-key block is not an observed-key fetch"
    );
}

/// The cap/closure path must preserve an unknown calendar too: turning it into a
/// known-empty calendar would remove the offer's `calendar_unknown` flag.
#[test]
fn cap_closure_preserves_an_unknown_calendar() {
    let day = Date::new(2027, 6, 16).expect("a fixture date");
    let window = Span::new(utc(day, 9, 0), utc(day, 15, 0)).expect("a window");
    let candidate = utc(day, 13, 0);

    let mut request = Request::new(
        utc(day, 9, 0),
        fixture_goal("UTC", window, candidate, "ana", 30),
    );
    request.participants.push(Participant::new(
        "ana",
        ZoneName::new("UTC").expect("a zone name"),
    ));
    // Make closure non-vacuous on another participant: preserving `None` only
    // when there are no observed keys would not prove the contract.
    let far_day = Date::new(2027, 6, 20).expect("a fixture date");
    let mut bo = Participant::attendee("bo", ZoneName::new("UTC").expect("a zone name"));
    bo.calendar = Some(vec![
        keyed_block(
            Span::new(utc(day, 10, 0), utc(day, 10, 30)).expect("a span"),
            "evt_seed",
            true,
        ),
        keyed_block(
            Span::new(utc(far_day, 10, 0), utc(far_day, 10, 30)).expect("a span"),
            "evt_seed",
            true,
        ),
    ]);
    request.participants.push(bo);
    request
        .rules
        .push(interview_cap(CapMeasure::Count, 1, CapPeriod::Day));

    let outcome = fetch_outcome(&request, candidate, &["ana"]);
    assert!(outcome.ranges.occurrences.is_some());
    assert_closed_matches_full(&outcome);
    assert!(
        outcome.closed_request.participants[0].calendar.is_none(),
        "closure must not turn an unknown calendar into a known-empty one"
    );
    assert!(
        outcome.closed_request.participants[1]
            .blocks()
            .iter()
            .any(|block| block.span.start() == utc(far_day, 10, 0)),
        "the other participant's far copy proves closure actually ran"
    );
    assert!(
        outcome.closed.offers[0].calendar_unknown(),
        "the closed request retains the unknown-calendar offer flag"
    );
}

/// Occurrences stay only when their own span overlaps the occurrence hull or they
/// carry a key the occurrence-range fetch observed; overlapping the wider calendar
/// hull is not enough.
#[test]
fn the_fetch_model_keeps_occurrences_only_for_the_occurrence_hull_or_an_observed_key() {
    let day = Date::new(2027, 6, 16).expect("a fixture date");
    let calendar_only = Date::new(2027, 6, 17).expect("a fixture date");
    let window = Span::new(utc(day, 9, 0), utc(day, 15, 0)).expect("a window");
    let candidate = utc(day, 13, 0);

    let mut ana = Participant::new("ana", ZoneName::new("UTC").expect("a zone name"));
    // Inside the calendar hull's one-day padding, outside the day occurrence
    // hull: a copy sharing the observed key, which closure must retrieve.
    ana.calendar = Some(vec![keyed_block(
        Span::new(utc(calendar_only, 10, 0), utc(calendar_only, 10, 30)).expect("a span"),
        "evt_in",
        true,
    )]);
    let mut request = Request::new(
        utc(day, 9, 0),
        fixture_goal("UTC", window, candidate, "ana", 30),
    );
    request.participants.push(ana);
    request.occurrences.push(
        Occurrence::new(
            "evt_in",
            Span::new(utc(day, 10, 0), utc(day, 10, 30)).expect("a span"),
            vec!["ana".to_owned()],
        )
        .expect("the occurrence names a participant"),
    );
    // Overlaps the calendar hull only, and carries a key nobody observed.
    request.occurrences.push(
        Occurrence::new(
            "evt_out",
            Span::new(utc(calendar_only, 12, 0), utc(calendar_only, 12, 30)).expect("a span"),
            vec!["ana".to_owned()],
        )
        .expect("the occurrence names a participant"),
    );
    request
        .rules
        .push(interview_cap(CapMeasure::Count, 1, CapPeriod::Day));

    let ranges = required_ranges_of(&request);
    let one_pass = one_pass_request(&request, &ranges);
    let closed = key_closed_request(&request, &ranges);

    let keys = |request: &Request| -> Vec<String> {
        request
            .occurrences
            .iter()
            .map(|occurrence| occurrence.event_key.clone())
            .collect()
    };
    assert_eq!(
        keys(&one_pass),
        ["evt_in"],
        "an occurrence overlapping only the calendar hull is never fetched"
    );
    assert_eq!(
        keys(&closed),
        ["evt_in"],
        "closure does not resurrect an occurrence with an unobserved key"
    );
    assert!(
        closed.participants[0]
            .blocks()
            .iter()
            .any(|block| block.event_key.as_deref() == Some("evt_in")),
        "closure retrieves the block carrying the observed key"
    );
}

/// A block retained only because it overlaps the calendar hull is not an
/// occurrence-range observation. Its key must not pull otherwise irrelevant
/// copies into the closed request.
#[test]
fn a_calendar_only_block_does_not_seed_closure() {
    let day = Date::new(2027, 6, 16).expect("a fixture date");
    let calendar_only_day = Date::new(2027, 6, 17).expect("a fixture date");
    let far_day = Date::new(2027, 6, 20).expect("a fixture date");
    let window = Span::new(utc(day, 9, 0), utc(day, 15, 0)).expect("a window");
    let candidate = utc(day, 13, 0);

    let mut ana = Participant::new("ana", ZoneName::new("UTC").expect("a zone name"));
    ana.calendar = Some(vec![
        keyed_block(
            Span::new(
                utc(calendar_only_day, 10, 0),
                utc(calendar_only_day, 10, 30),
            )
            .expect("a span"),
            "evt_calendar_only",
            true,
        ),
        keyed_block(
            Span::new(utc(far_day, 10, 0), utc(far_day, 10, 30)).expect("a span"),
            "evt_calendar_only",
            true,
        ),
    ]);
    let mut request = Request::new(
        utc(day, 9, 0),
        fixture_goal("UTC", window, candidate, "ana", 30),
    );
    request.participants.push(ana);
    request
        .rules
        .push(interview_cap(CapMeasure::Count, 1, CapPeriod::Day));

    let ranges = required_ranges_of(&request);
    let one_pass = one_pass_request(&request, &ranges);
    let closed = key_closed_request(&request, &ranges);
    assert_eq!(
        one_pass.participants[0].blocks().len(),
        1,
        "the fixture's first copy overlaps only the calendar hull"
    );
    assert_eq!(
        closed, one_pass,
        "a calendar-only observation must not retrieve the far copy sharing its key"
    );
}

/// Travel makes a block relevant to the calendar hull only when that occupied
/// span overlaps it. It must not make the block an occurrence-range observation:
/// the occurrence hull always reads the block's own span.
#[test]
fn travel_overlap_with_the_occurrence_hull_does_not_seed_closure() {
    let day = Date::new(2027, 6, 16).expect("a fixture date");
    let before_month = Date::new(2027, 5, 31).expect("a fixture date");
    let after_month = Date::new(2027, 7, 20).expect("a fixture date");
    let window = Span::new(utc(day, 9, 0), utc(day, 15, 0)).expect("a window");
    let candidate = utc(day, 13, 0);

    let mut travel_copy = keyed_block(
        Span::new(utc(before_month, 23, 0), utc(before_month, 23, 30)).expect("a span"),
        "evt_travel_only",
        true,
    );
    travel_copy.travel_after = Duration::from_minutes(60);
    let mut ana = Participant::new("ana", ZoneName::new("UTC").expect("a zone name"));
    ana.calendar = Some(vec![
        travel_copy,
        keyed_block(
            Span::new(utc(after_month, 10, 0), utc(after_month, 10, 30)).expect("a span"),
            "evt_travel_only",
            true,
        ),
    ]);

    let mut request = Request::new(
        utc(day, 9, 0),
        fixture_goal("UTC", window, candidate, "ana", 30),
    );
    request.participants.push(ana);
    request
        .rules
        .push(interview_cap(CapMeasure::Count, 1, CapPeriod::Month));

    let ranges = required_ranges_of(&request);
    let occurrences = ranges.occurrences.expect("the month cap applies");
    let travel = &request.participants[0].blocks()[0];
    let occupied_end = travel.span.end().minutes() + travel.travel_after.minutes();
    assert!(travel.span.end() <= occurrences.start());
    assert!(occupied_end > occurrences.start().minutes());
    assert!(
        occupied_end <= ranges.calendar.start().minutes(),
        "the fixture's occupied span reaches only the occurrence hull"
    );

    let one_pass = one_pass_request(&request, &ranges);
    let closed = key_closed_request(&request, &ranges);
    assert!(
        one_pass.participants[0].blocks().is_empty(),
        "travel overlap with the occurrence hull is not an initial-fetch match"
    );
    assert_eq!(
        closed, one_pass,
        "a dropped travel copy must not seed retrieval of its far same-key copy"
    );
}

/// Closure is a record filter, not an event-key projection: every same-key
/// record survives exactly once, in its original position.
#[test]
fn closure_preserves_same_key_record_identity_and_request_order() {
    let day = Date::new(2027, 6, 16).expect("a fixture date");
    let first_far_day = Date::new(2027, 6, 20).expect("a fixture date");
    let second_far_day = Date::new(2027, 6, 21).expect("a fixture date");
    let unobserved_day = Date::new(2027, 12, 1).expect("a fixture date");
    let window = Span::new(utc(day, 9, 0), utc(day, 15, 0)).expect("a window");
    let candidate = utc(day, 13, 0);

    // Deliberately not chronological: a key-indexed rebuild or a sort would
    // change provider record order even though it preserved the event's hull.
    // The last far copy is untagged, so selector-based retention fails here too.
    let expected = vec![
        keyed_block(
            Span::new(utc(first_far_day, 12, 0), utc(first_far_day, 12, 30)).expect("a span"),
            "evt_shared",
            true,
        ),
        keyed_block(
            Span::new(utc(day, 10, 0), utc(day, 10, 30)).expect("a span"),
            "evt_shared",
            false,
        ),
        keyed_block(
            Span::new(utc(second_far_day, 11, 0), utc(second_far_day, 11, 45)).expect("a span"),
            "evt_shared",
            false,
        ),
    ];
    let mut ana = Participant::new("ana", ZoneName::new("UTC").expect("a zone name"));
    ana.calendar = Some(expected.clone());
    ana.calendar.as_mut().expect("a calendar").push(keyed_block(
        Span::new(utc(unobserved_day, 10, 0), utc(unobserved_day, 10, 30)).expect("a span"),
        "evt_unobserved",
        true,
    ));

    let mut request = Request::new(
        utc(day, 9, 0),
        fixture_goal("UTC", window, candidate, "ana", 30),
    );
    request.participants.push(ana);
    request
        .rules
        .push(interview_cap(CapMeasure::Count, 1, CapPeriod::Day));

    let ranges = required_ranges_of(&request);
    let one_pass = one_pass_request(&request, &ranges);
    let closed = key_closed_request(&request, &ranges);
    assert_eq!(
        one_pass.participants[0].blocks(),
        &expected[1..2],
        "only the occurrence-hull copy survives the initial fetch"
    );
    assert_eq!(
        closed.participants[0].blocks(),
        expected,
        "closure must retain every same-key record in request order, without key deduplication"
    );
}

/// Occurrences are request-global and may name only people outside the request.
/// Once a participant's in-hull block observes their key, closure must retain the
/// occurrence: its authoritative participant list still suppresses the block's
/// participant, even though none of the occurrence's own names are request IDs.
#[test]
fn closure_retains_an_occurrence_naming_only_participants_outside_the_request() {
    let day = Date::new(2027, 6, 16).expect("a fixture date");
    let window = Span::new(utc(day, 9, 0), utc(day, 15, 0)).expect("a window");
    let candidate = utc(day, 13, 0);

    let mut ana = Participant::new("ana", ZoneName::new("UTC").expect("a zone name"));
    ana.calendar = Some(vec![keyed_block(
        Span::new(utc(day, 10, 0), utc(day, 10, 30)).expect("a span"),
        "evt_external",
        true,
    )]);
    let mut request = Request::new(
        utc(day, 9, 0),
        fixture_goal("UTC", window, candidate, "ana", 30),
    );
    request.participants.push(ana);
    request.occurrences.push(
        Occurrence::new(
            "evt_external",
            Span::new(utc(day, 10, 0), utc(day, 11, 0)).expect("a span"),
            vec!["external".to_owned()],
        )
        .expect("an occurrence may name someone outside the request"),
    );
    request
        .rules
        .push(interview_cap(CapMeasure::Count, 1, CapPeriod::Day));

    let outcome = fetch_outcome(&request, candidate, &["ana"]);
    assert_closed_matches_full(&outcome);
    assert_eq!(
        outcome.closed_request.occurrences, request.occurrences,
        "closure must not participant-filter the authoritative occurrence away"
    );
    assert_eq!(
        outcome.full.offers.len(),
        1,
        "the occurrence names no request participant, so the event counts for nobody"
    );

    // The reachable failure a participant-scoped occurrence closure produces:
    // without the occurrence, its same-key block becomes a standalone event on
    // ana and silently fills her cap instead.
    let mut participant_filtered = outcome.closed_request.clone();
    participant_filtered.occurrences.clear();
    let filtered = engine()
        .find_availability(&participant_filtered)
        .expect("the filtered control request is valid");
    assert!(
        filtered.offers.is_empty(),
        "dropping the authoritative occurrence falsely charges the block to ana"
    );
    assert!(
        !slot_answer(&participant_filtered, candidate, &["ana"]).0,
        "the same false exclusion reaches confirmation"
    );
}

/// An occurrence is a request-global record, so the **initial** fetch must not
/// participant-filter it either. This one names only somebody outside the
/// request, and its same-key block lies inside the calendar hull but outside the
/// occurrence hull: the initial fetch retrieves the block, the block seeds no
/// closure key, and a participant-filtered occurrence query therefore loses the
/// occurrence with nothing to recover it. Full data charges the event to nobody;
/// the filtered fetch charges the block to `ana` on its own day and rejects a
/// booking there.
#[test]
fn an_occurrence_naming_only_outsiders_is_needed_by_the_initial_fetch_too() {
    let day = Date::new(2027, 6, 16).expect("a fixture date");
    let next = Date::new(2027, 6, 17).expect("a fixture date");
    let window = Span::new(utc(day, 9, 0), utc(day, 15, 0)).expect("a window");
    let candidate = utc(day, 13, 0);
    // A booking on the day the stripped block sits on: the bucket a
    // participant-filtered fetch wrongly fills.
    let next_day_start = utc(next, 13, 0);

    let mut ana = Participant::new("ana", ZoneName::new("UTC").expect("a zone name"));
    ana.calendar = Some(vec![keyed_block(
        Span::new(utc(next, 10, 0), utc(next, 10, 30)).expect("a span"),
        "evt_outsider",
        true,
    )]);
    let mut request = Request::new(
        utc(day, 9, 0),
        fixture_goal("UTC", window, candidate, "ana", 30),
    );
    request.participants.push(ana);
    request.occurrences.push(
        Occurrence::new(
            "evt_outsider",
            Span::new(utc(day, 10, 0), utc(day, 11, 0)).expect("a span"),
            vec!["external".to_owned()],
        )
        .expect("an occurrence may name someone outside the request")
        .with_tags(vec![tag("interview")]),
    );
    request
        .rules
        .push(interview_cap(CapMeasure::Count, 1, CapPeriod::Day));

    let outcome = fetch_outcome(&request, candidate, &["ana"]);
    let hull = outcome.ranges.occurrences.expect("the cap applies");
    let block = &request.participants[0].blocks()[0];
    assert!(
        block.span.start() >= hull.end() || hull.start() >= block.span.end(),
        "the block lies outside the occurrence hull, so it seeds no closure key"
    );
    assert!(
        block.span.start() < outcome.ranges.calendar.end()
            && outcome.ranges.calendar.start() < block.span.end(),
        "the block lies inside the calendar hull, so the initial fetch retrieves it"
    );

    assert_closed_matches_full(&outcome);
    assert_eq!(
        outcome.closed_request.occurrences, request.occurrences,
        "the initial fetch keeps an occurrence no request participant is named on"
    );
    assert!(
        slot_answer(&request, next_day_start, &["ana"]).0,
        "full data charges the event to nobody, so the block's own day is free"
    );
    assert!(
        slot_answer(&outcome.closed_request, next_day_start, &["ana"]).0,
        "the closed fetch agrees on that day too"
    );

    // What a participant-filtered occurrence query leaves behind: the same closed
    // request with the occurrence gone. No key was observed on an
    // occurrence-range record, so the closure retrieves nothing and the block
    // stands alone as `ana`'s event.
    let mut filtered = outcome.closed_request.clone();
    filtered.occurrences.clear();
    let answer = slot_answer(&filtered, next_day_start, &["ana"]);
    assert!(
        !answer.0,
        "dropping the authoritative occurrence falsely charges the block to ana"
    );
    assert!(
        answer
            .1
            .iter()
            .any(|reason| reason.code == ReasonCode::CapReached),
        "and reports it as the cap: {:?}",
        answer.1
    );
}
