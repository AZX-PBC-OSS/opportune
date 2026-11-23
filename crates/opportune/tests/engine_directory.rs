//! The engine over a database directory: building one, its failures, and a search
//! that never falls back to the host's database.
//!
//! These tests use the `TZif` fixtures rather than the bundled data, so they run
//! with `--no-default-features` too — the build a strict adopter uses.

#![cfg(not(target_family = "wasm"))]

use opportune::Engine;
use opportune::model::{Goal, Part, Participant, Request, Requirement, Span, Term};
use opportune::time::{Duration, Instant, ZoneName};

fn name(zone: &str) -> ZoneName {
    ZoneName::new(zone).unwrap()
}

/// `count` minutes after the earliest supported instant, `1975-01-01T00:00Z`.
///
/// This is not the frame `Instant::minutes` returns, which counts from the Unix
/// epoch: to shift an instant by `n` minutes use `Instant::from_minutes`, not
/// this helper.
fn minutes(count: i64) -> Instant {
    Instant::from_minutes(Instant::MIN.minutes() + count).unwrap()
}

fn span(start: Instant, end: Instant) -> Span {
    Span::new(start, end).unwrap()
}

/// A participant with an empty calendar: known to be free.
fn free_participant(id: &str) -> Participant {
    let mut participant = Participant::new(id, name("UTC"));
    participant.calendar = Some(Vec::new());
    participant
}

/// A minimal request: one 30-minute part for the participant, over `search`.
fn single_host_request(
    now: Instant,
    search: Span,
    participant: Participant,
    rules: Vec<opportune::model::Rule>,
) -> Request {
    let part = Part::new(
        Duration::from_minutes(30),
        Requirement::new(vec![Term::person(participant.id.clone())]),
    );
    let mut request = Request::new(now, Goal::new(name("UTC"), search, vec![part]));
    request.participants.push(participant);
    request.rules = rules;
    request
}

#[test]
fn a_directory_database_is_enough_and_never_falls_back_to_the_system_one() {
    let directory =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/tzdb-2018");
    let engine = Engine::builder()
        .tzdb_dir(&directory, None)
        .build()
        .unwrap();
    assert_eq!(engine.time_zone_database().version(), "2018f");

    // The fixture holds only Sao Paulo, so a search there works and one in a
    // zone the fixture lacks is a validation error rather than the host's data.
    let mut participant = free_participant("rich");
    participant.zone = name("America/Sao_Paulo");
    let request = single_host_request(
        minutes(0),
        span(minutes(0), minutes(60)),
        participant,
        Vec::new(),
    );
    let response = engine.find_availability(&request).unwrap();
    assert_eq!(response.meta.tzdb, "2018f");
    assert!(!response.offers.is_empty());

    let mut participant = free_participant("rich");
    participant.zone = name("Europe/London");
    let request = single_host_request(
        minutes(0),
        span(minutes(0), minutes(60)),
        participant,
        Vec::new(),
    );
    assert!(
        engine
            .validate(&request)
            .errors
            .iter()
            .any(|error| error.code() == "zone_unknown")
    );
}

#[test]
fn a_directory_database_reports_its_own_failure() {
    let directory =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/tzdb-unversioned");
    let error = Engine::builder()
        .tzdb_dir(&directory, None)
        .build()
        .unwrap_err();
    assert_eq!(
        error.code(),
        "tzdb_version_missing",
        "the builder reports the directory's failure, not a missing configuration"
    );

    let missing =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/no-such-database");
    let error = Engine::builder()
        .tzdb_dir(&missing, Some("2026f"))
        .build()
        .unwrap_err();
    assert_eq!(error.code(), "tzdb_directory_unusable");
}

#[test]
fn an_engine_without_a_database_refuses_to_build() {
    let built = Engine::builder().build();
    if cfg!(feature = "bundled-tzdb") {
        assert!(built.is_ok(), "the bundled feature supplies a database");
    } else {
        assert_eq!(
            built.unwrap_err().code(),
            "tzdb_not_configured",
            "without the bundled feature there is no fallback"
        );
    }
}

#[test]
fn a_calendar_from_a_labelled_directory_is_enough() {
    let directory =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/tzdb-unversioned");
    let engine = Engine::builder()
        .tzdb_dir(&directory, Some("custom-2018"))
        .build()
        .unwrap();
    assert_eq!(engine.time_zone_database().version(), "custom-2018");
    let mut participant = free_participant("rich");
    participant.zone = name("America/Sao_Paulo");
    let request = single_host_request(
        minutes(0),
        span(minutes(0), minutes(60)),
        participant,
        Vec::new(),
    );
    assert_eq!(
        engine.find_availability(&request).unwrap().meta.tzdb,
        "custom-2018"
    );
}
