//! The assembled suite through every entry point a caller can reach: the bundled
//! engine, an engine loaded from a database directory, and the feature-gated free
//! functions.
//!
//! The scenario suites already assert what each answer is. This file asserts that
//! the same requests reach the same answers whichever way a caller builds the
//! engine, so an entry point that quietly resolves zones differently — a
//! directory database read in the wrong place, or a free function whose default
//! engine was not the one the tests use — shows up here rather than in production.

use std::sync::OnceLock;

use opportune::model::{Goal, Participant, Request, Requirement, Response, Span, Term};
use opportune::presets;
use opportune::time::{
    Date, Duration, Instant, LocalResolution, RuleZone, TimeOfDay, TimeZoneDatabase, WallEnd,
    Weekday, Weekdays, ZoneName,
};
use opportune::{Engine, SlotCheck};
use opportune_testkit::scenarios::{a1_scenarios, rule_kind_scenarios, zone_matrix};
use opportune_testkit::scenarios_a2::a2_scenarios;

/// A database directory carried with the core crate's own tests: a real `TZif`
/// tree with its `+VERSION` file.
const DIRECTORY: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../opportune/tests/fixtures/tzdb-2018"
);

fn database() -> &'static TimeZoneDatabase {
    static DATABASE: OnceLock<TimeZoneDatabase> = OnceLock::new();
    DATABASE.get_or_init(|| TimeZoneDatabase::bundled().expect("the bundled database is present"))
}

fn bundled() -> &'static Engine {
    static ENGINE: OnceLock<Engine> = OnceLock::new();
    ENGINE.get_or_init(|| {
        Engine::builder()
            .build()
            .expect("the bundled database builds")
    })
}

fn from_directory() -> &'static Engine {
    static ENGINE: OnceLock<Engine> = OnceLock::new();
    ENGINE.get_or_init(|| {
        Engine::builder()
            .tzdb_dir(DIRECTORY, None)
            .build()
            .expect("the fixture database directory loads")
    })
}

/// Every request the scenario suite runs, in every zone of the matrix, with the
/// scenario's own name so a failure says which case disagreed.
fn suite() -> Vec<(String, Request)> {
    let db = database();
    let mut requests = Vec::new();
    for zone in zone_matrix() {
        let named = |id: &str| format!("{id} in {zone}");
        requests.extend(
            a1_scenarios(db, zone)
                .into_iter()
                .map(|item| (named(item.id), item.request)),
        );
        requests.extend(
            rule_kind_scenarios(db, zone)
                .into_iter()
                .map(|item| (named(item.id), item.request)),
        );
        requests.extend(
            a2_scenarios(db, zone)
                .into_iter()
                .map(|item| (named(item.id), item.request)),
        );
    }
    requests
}

/// Asserts two answers agree on everything but the database version they name.
///
/// `meta.tzdb` is the one field that must differ between two databases, so
/// comparing whole `Response`s would only be testing that the bundled version
/// string is not the directory's.
fn assert_same_answer(left: &Response, right: &Response, context: &str) {
    assert_eq!(left.offers, right.offers, "{context}: offers");
    assert_eq!(left.dates, right.dates, "{context}: dates");
    assert_eq!(left.excluded, right.excluded, "{context}: exclusions");
    assert_eq!(left.truncated, right.truncated, "{context}: truncated");
    assert_eq!(
        left.excluded_truncated, right.excluded_truncated,
        "{context}: excluded_truncated"
    );
    assert_eq!(left.warnings, right.warnings, "{context}: warnings");
    assert_eq!(left.meta.engine, right.meta.engine, "{context}: engine");
    assert_eq!(left.meta.schema, right.meta.schema, "{context}: schema");
}

fn assert_same_confirmation(left: &SlotCheck, right: &SlotCheck, context: &str) {
    assert_eq!(left.valid, right.valid, "{context}: valid");
    assert_eq!(left.reasons, right.reasons, "{context}: reasons");
    assert_eq!(left.flags, right.flags, "{context}: flags");
    assert_eq!(left.reserve, right.reserve, "{context}: reserve");
    assert_eq!(left.warnings, right.warnings, "{context}: warnings");
}

#[test]
fn a_directory_loaded_engine_answers_the_suite_like_the_bundled_one() {
    let directory = from_directory();
    assert!(
        directory.time_zone_database().version() != bundled().time_zone_database().version(),
        "the fixture directory must be a different database, or this proves nothing"
    );
    let mut compared = 0;
    for (context, request) in suite() {
        let validation = directory.validate(&request);
        if !validation.errors.is_empty() {
            // The fixture directory carries one zone (`America/Sao_Paulo`), so a
            // scenario that names any other cannot be judged by it at all. Those
            // are the bundled engine's business; they are counted as unreachable
            // here rather than compared against a database that has never heard
            // of them.
            //
            // What stays comparable is narrower than "inside the fixture's
            // zones": the requests that survive carry only `UTC`, which a
            // directory database answers without reading any `TZif` file, so
            // nothing compared here depends on the fixture's offsets. The
            // fixture's own data is pinned by
            // `a_directory_loaded_engine_answers_from_the_database_it_was_given`.
            assert!(
                validation
                    .errors
                    .iter()
                    .all(|error| error.code() == "zone_unknown"),
                "{context}: the fixture database rejected the request for something other than a \
                 zone: {:?}",
                validation.errors
            );
            continue;
        }
        compared += 1;
        assert_eq!(
            validation,
            bundled().validate(&request),
            "{context}: validation"
        );
        let from_dir = directory.find_availability(&request).unwrap();
        let from_bundled = bundled().find_availability(&request).unwrap();
        assert_same_answer(&from_dir, &from_bundled, &context);
        for (offer, expected) in from_dir.offers.iter().zip(&from_bundled.offers) {
            let check = directory
                .check_slot(&request, &offer.to_proposal())
                .unwrap();
            let expected = bundled()
                .check_slot(&request, &expected.to_proposal())
                .unwrap();
            assert_same_confirmation(&check, &expected, &context);
        }
    }
    // The comparison has to have run on something: a suite whose every request
    // named another zone would pass without judging the directory engine once.
    assert!(
        compared >= 20,
        "only {compared} scenario requests were comparable against the fixture database"
    );
}

/// The one zone the fixture directory carries.
const FIXTURE_ZONE: &str = "America/Sao_Paulo";

/// A request in the fixture's own zone over a day its data and the bundled data
/// disagree about.
///
/// The fixture's rules start in 2018, so it holds no daylight saving for
/// `America/Sao_Paulo` on 2018-02-15 and the zone is a flat UTC-03:00. The
/// bundled database has that date inside Brazil's 2017-18 daylight saving
/// period, at UTC-02:00, so a 09:00-17:00 window covers 12:00-20:00 UTC in one
/// database and 11:00-19:00 UTC in the other.
fn fixture_zone_request(fixture: &TimeZoneDatabase) -> Request {
    let zone = ZoneName::new(FIXTURE_ZONE).expect("the fixture zone is a zone name");
    let fixture_zone = fixture
        .zone(FIXTURE_ZONE)
        .expect("the fixture directory carries the zone");
    let midnight = |month, day| {
        let date = Date::new(2018, month, day).expect("the fixture date is a real date");
        let midnight_time = TimeOfDay::new(0, 0).expect("midnight is a time of day");
        match fixture_zone.resolve_local(date, midnight_time).unwrap() {
            LocalResolution::Unique(instant) => instant,
            other => panic!("the fixture's midnight on {date} must be unique: {other:?}"),
        }
    };
    let window = Span::new(midnight(2, 15), midnight(2, 16)).expect("the day is non-empty");
    let mut request = Request::new(
        Instant::from_minutes(window.start().minutes() - 24 * 60).expect("a valid instant"),
        Goal::single(
            zone.clone(),
            window,
            Duration::from_minutes(30),
            Requirement::new(vec![Term::person("rich")]),
        ),
    );
    let mut participant = Participant::new("rich", zone);
    participant.calendar = Some(Vec::new());
    request.participants.push(participant);
    request.rules.push(
        presets::business_hours(
            RuleZone::Participant,
            Weekdays::range(Weekday::Monday, Weekday::Friday),
            TimeOfDay::new(9, 0).expect("a valid time of day"),
            WallEnd::Time(TimeOfDay::new(17, 0).expect("a valid time of day")),
        )
        .expect("the hours are a non-empty range"),
    );
    request
}

/// A directory-loaded engine answers from the `TZif` files it was pointed at,
/// not from the database compiled into the binary.
///
/// The suite comparison above cannot see this: every request it manages to
/// compare carries only `UTC`, which both databases answer identically, so an
/// engine that read the directory's `+VERSION` label and the bundled data would
/// pass it. This test makes the two databases disagree about the request's own
/// zone and pins the answer to the fixture's offsets.
#[test]
fn a_directory_loaded_engine_answers_from_the_database_it_was_given() {
    let fixture = TimeZoneDatabase::from_dir(DIRECTORY, None).expect("the fixture directory loads");
    let request = fixture_zone_request(&fixture);
    let directory = from_directory();

    let answer = directory
        .find_availability(&request)
        .expect("the fixture database can answer in its own zone");
    assert_eq!(answer.meta.tzdb, fixture.version());
    let starts: Vec<Instant> = answer
        .offers
        .iter()
        .map(|offer| offer.start().expect("an offer has an event"))
        .collect();

    // Half-hourly grid from local midnight, window 09:00-17:00: 09:00 to 16:30,
    // read in the fixture's zone, which is what the fixture's data makes of them.
    let fixture_zone = fixture.zone(FIXTURE_ZONE).expect("the fixture zone");
    let expected: Vec<Instant> = (0..16)
        .map(|step| {
            let hour = 9 + step / 2;
            let minute = 30 * (step % 2);
            let time = TimeOfDay::new(hour, minute).expect("a valid time of day");
            let date = Date::new(2018, 2, 15).expect("a real date");
            match fixture_zone
                .resolve_local(date, time)
                .expect("a representable time")
            {
                LocalResolution::Unique(instant) => instant,
                other => panic!("{date} {time} must be unique in the fixture: {other:?}"),
            }
        })
        .collect();
    assert_eq!(
        starts, expected,
        "the answers are the fixture's own offsets"
    );

    // The bundled database answering the same request disagrees, and its first
    // offer is an instant the fixture database reads as 08:00 — the window has
    // not opened in the fixture's terms — so the two answers cannot both come
    // from the fixture's data, and the one asserted above is the fixture's.
    let bundled_starts: Vec<Instant> = bundled()
        .find_availability(&request)
        .expect("the bundled database answers")
        .offers
        .iter()
        .map(|offer| offer.start().expect("an offer has an event"))
        .collect();
    assert_ne!(starts, bundled_starts, "the two databases disagree here");
    let first_bundled = bundled_starts[0];
    assert_eq!(
        fixture_zone.zoned(first_bundled).time(),
        TimeOfDay::new(8, 0).expect("a valid time of day"),
        "the bundled answer's first start is outside the window in the fixture's terms"
    );
    assert!(
        !starts.contains(&first_bundled),
        "an engine reading the bundled data would have offered {first_bundled}"
    );

    // The confirmation agrees with the offer it was handed back.
    let check = directory
        .check_slot(&request, &answer.offers[0].to_proposal())
        .expect("the offer is a well-formed proposal");
    assert!(
        check.valid,
        "the offer this engine made confirms: {:?}",
        check.reasons
    );
}

#[test]
fn the_free_functions_answer_like_the_bundled_engine() {
    for (context, request) in suite() {
        assert_eq!(
            opportune::validate(&request),
            bundled().validate(&request),
            "{context}: validate"
        );
        let response = opportune::find_availability(&request).unwrap();
        assert_eq!(
            response,
            bundled().find_availability(&request).unwrap(),
            "{context}: find_availability"
        );
        for offer in &response.offers {
            assert_eq!(
                opportune::check_slot(&request, &offer.to_proposal()).unwrap(),
                bundled()
                    .check_slot(&request, &offer.to_proposal())
                    .unwrap(),
                "{context}: check_slot"
            );
        }
        let mut query = opportune::model::RangesQuery::new(request.now, request.goal.clone());
        query.rules.clone_from(&request.rules);
        query.participants.clone_from(&request.participants);
        assert_eq!(
            opportune::required_ranges(&query).unwrap(),
            bundled().required_ranges(&query).unwrap(),
            "{context}: required_ranges"
        );
    }
}
