//! Boundary safety: the extreme sizes and durations a request can name fail
//! closed with stable codes before any expensive work runs, a confirmation and a
//! search agree on which bookings the model can describe, `Response::dates` covers
//! every local date a window touches, and an event key that names no event is
//! rejected rather than merging unrelated events into one cap count.
//!
//! Every case here drives a real public entry point — `validate`,
//! `find_availability`, `check_slot` or `required_ranges` — and asserts the answer
//! or finding a caller acts on.

#![cfg(feature = "bundled-tzdb")]

use opportune::model::{
    Block, Cap, CapMeasure, CapPer, CapPeriod, Clearance, Goal, Occurrence, Part, Participant,
    ParticipantTravel, Proposal, ProposalEvent, RangesQuery, Request, Requirement, Rest, Rule,
    RuleKind, Span, Term, ZonePeriod,
};
use opportune::tags::{Selector, Tag};
use opportune::time::{
    Date, Duration, Grid, GridZone, Instant, LocalResolution, RuleZone, TimeOfDay, WallEnd,
    Weekday, Weekdays, Zone, ZoneName,
};
use opportune::{Diagnostics, Engine, Error, Limits, presets};

fn name(zone: &str) -> ZoneName {
    ZoneName::new(zone).expect("a valid IANA zone name")
}

fn span(start: Instant, end: Instant) -> Span {
    Span::new(start, end).expect("a non-empty span")
}

fn utc(year: i16, month: i8, day: i8, hour: u8, minute: u8) -> Instant {
    let date = Date::new(year, month, day).expect("a real date");
    let time = TimeOfDay::new(hour, minute).expect("a real time of day");
    match Zone::utc()
        .resolve_local(date, time)
        .expect("a UTC reading is representable")
    {
        LocalResolution::Unique(instant) => instant,
        LocalResolution::Skipped | LocalResolution::Repeated(_, _) => {
            unreachable!("UTC has no transitions")
        }
    }
}

/// A request for one 30-minute booking by a single free host, over `window` in
/// `zone`.
fn booking(zone: &str, window: Span) -> Request {
    let mut participant = Participant::new("ana", name(zone));
    participant.calendar = Some(Vec::new());
    let mut request = Request::new(
        window.start(),
        Goal::single(
            name(zone),
            window,
            Duration::from_minutes(30),
            Requirement::new(vec![Term::person("ana")]),
        ),
    );
    request.goal.display_zone = name(zone);
    request.participants = vec![participant];
    request
}

fn with_duration(mut request: Request, duration: Duration) -> Request {
    request.goal.parts[0].duration = duration;
    request
}

fn proposal(start: Instant) -> Proposal {
    Proposal {
        events: vec![ProposalEvent {
            part: "main".to_owned(),
            start,
            assignment: vec![vec!["ana".to_owned()]],
        }],
    }
}

fn codes(diagnostics: &Diagnostics) -> Vec<&str> {
    diagnostics
        .errors
        .iter()
        .map(opportune::Diagnostic::code)
        .collect()
}

fn warning_codes(diagnostics: &Diagnostics) -> Vec<&str> {
    diagnostics
        .warnings
        .iter()
        .map(opportune::Diagnostic::code)
        .collect()
}

/// The path of the first warning with `code`.
fn warning_path(diagnostics: &Diagnostics, code: &str) -> Option<String> {
    diagnostics
        .warnings
        .iter()
        .find(|finding| finding.code() == code)
        .and_then(|finding| finding.path().map(str::to_owned))
}

fn tag(path: &str) -> Tag {
    Tag::new(path).expect("a valid tag path")
}

/// The path of the first error with `code`.
fn path(diagnostics: &Diagnostics, code: &str) -> Option<String> {
    diagnostics
        .errors
        .iter()
        .find(|finding| finding.code() == code)
        .and_then(|finding| finding.path().map(str::to_owned))
}

fn day() -> Span {
    span(utc(2027, 6, 14, 0, 0), utc(2027, 6, 15, 0, 0))
}

fn engine() -> Engine {
    Engine::builder()
        .build()
        .expect("the bundled database builds")
}

#[test]
fn an_extreme_booking_duration_is_rejected_by_every_entry_point() {
    let engine = engine();
    for duration in [
        Duration::from_hours(2_000_000),
        Duration::from_minutes(u32::MAX),
    ] {
        let request = with_duration(booking("UTC", day()), duration);

        let diagnostics = engine.validate(&request);
        assert_eq!(
            path(&diagnostics, "part_duration_too_long").as_deref(),
            Some("/goal/parts/0/duration"),
            "{duration} is rejected by validation: {:?}",
            diagnostics.errors
        );

        assert_eq!(
            engine.find_availability(&request).unwrap_err().code(),
            "invalid_request",
            "{duration} never reaches the search"
        );
        assert_eq!(
            engine
                .check_slot(&request, &proposal(utc(2027, 6, 14, 9, 0)))
                .unwrap_err()
                .code(),
            "invalid_request",
            "{duration} never reaches the confirmation"
        );
    }
}

#[test]
fn an_extreme_travel_duration_is_rejected_where_it_was_written() {
    let engine = engine();

    let mut request = booking("UTC", day());
    request.goal.parts[0].travel.before = Duration::from_minutes(u32::MAX);
    assert_eq!(
        path(&engine.validate(&request), "travel_duration_too_long").as_deref(),
        Some("/goal/parts/0/travel/before")
    );

    let mut request = booking("UTC", day());
    request.goal.parts[0].travel.after = Duration::from_hours(2_000_000);
    assert_eq!(
        path(&engine.validate(&request), "travel_duration_too_long").as_deref(),
        Some("/goal/parts/0/travel/after")
    );

    // A participant's own override is reported at its own field, with the ID
    // escaped the way a JSON pointer escapes a key.
    let mut request = booking("UTC", day());
    request.goal.parts[0].travel.by_participant.insert(
        "a/b".to_owned(),
        ParticipantTravel {
            before: Duration::from_minutes(u32::MAX),
            after: Duration::ZERO,
        },
    );
    assert_eq!(
        path(&engine.validate(&request), "travel_duration_too_long").as_deref(),
        Some("/goal/parts/0/travel/by_participant/a~1b/before")
    );
    assert_eq!(
        engine.find_availability(&request).unwrap_err().code(),
        "invalid_request"
    );
}

#[test]
fn an_extreme_clearance_duration_is_rejected_where_it_was_written() {
    let engine = engine();

    let mut request = booking("UTC", day());
    request.rules = vec![Rule::new(
        "gaps",
        RuleKind::Clearance(Clearance {
            before: Duration::ZERO,
            after: Duration::ZERO,
            min_gap: Duration::from_minutes(u32::MAX),
        }),
    )];
    assert_eq!(
        path(&engine.validate(&request), "clearance_duration_too_long").as_deref(),
        Some("/rules/0/params/min_gap")
    );

    let mut request = booking("UTC", day());
    request.rules = vec![Rule::new(
        "gaps",
        RuleKind::Clearance(Clearance {
            before: Duration::from_hours(2_000_000),
            after: Duration::ZERO,
            min_gap: Duration::ZERO,
        }),
    )];
    assert_eq!(
        path(&engine.validate(&request), "clearance_duration_too_long").as_deref(),
        Some("/rules/0/params/before")
    );
    assert_eq!(
        engine
            .check_slot(&request, &proposal(utc(2027, 6, 14, 9, 0)))
            .unwrap_err()
            .code(),
        "invalid_request"
    );
}

#[test]
fn the_duration_cap_is_the_configured_one() {
    let engine = Engine::builder()
        .limits(Limits::default().with_max_duration_minutes(60))
        .build()
        .expect("the bundled database builds");

    let request = with_duration(booking("UTC", day()), Duration::from_minutes(90));
    assert!(codes(&engine.validate(&request)).contains(&"part_duration_too_long"));

    // The cap is the boundary, not a hint: a booking exactly at it is fine.
    let request = with_duration(booking("UTC", day()), Duration::from_minutes(60));
    let diagnostics = engine.validate(&request);
    assert!(
        diagnostics.is_valid(),
        "a booking of exactly the configured maximum passes: {:?}",
        diagnostics.errors
    );
}

#[test]
fn the_size_limits_are_reported_before_the_structural_work() {
    let engine = Engine::builder()
        .limits(
            Limits::default()
                .with_max_rules(0)
                .with_max_occurrences(0)
                .with_max_requirement_slots(0),
        )
        .build()
        .expect("the bundled database builds");

    let mut request = booking("UTC", day());
    request.rules = vec![Rule::new("gaps", RuleKind::Clearance(Clearance::default()))];
    request.occurrences = vec![
        Occurrence::new(
            "evt",
            span(utc(2027, 6, 14, 8, 0), utc(2027, 6, 14, 9, 0)),
            vec!["ana".to_owned()],
        )
        .expect("a keyed occurrence with a participant"),
    ];
    let diagnostics = engine.validate(&request);
    assert_eq!(path(&diagnostics, "rules_limit").as_deref(), Some("/rules"));
    assert_eq!(
        path(&diagnostics, "occurrences_limit").as_deref(),
        Some("/occurrences")
    );
    assert_eq!(
        path(&diagnostics, "requirement_slots_limit").as_deref(),
        Some("/goal/parts/0/requirement/all")
    );

    // A request past a size cap is answered by that cap alone: the matching a
    // requirement graph large enough to exceed it would have demanded never runs,
    // so the unfillable terms below are not reported alongside it.
    let mut request = booking("UTC", day());
    request.rules = vec![Rule::new("gaps", RuleKind::Clearance(Clearance::default()))];
    request.goal.parts[0].requirement.terms = (0..65)
        .map(|index| Term::person(format!("ghost-{index}")))
        .collect();
    let diagnostics = engine.validate(&request);
    assert!(
        codes(&diagnostics).contains(&"rules_limit"),
        "{:?}",
        diagnostics.errors
    );
    assert!(
        !codes(&diagnostics).contains(&"participant_unknown"),
        "the structural pass does not run once a size cap rejects the request: {:?}",
        diagnostics.errors
    );
}

/// A request the cheap caps rejected is answered by that phase alone: the search
/// checks would have walked the window and the grid to bound the starts, and that
/// is exactly the work the cap refused to pay for.
#[test]
fn a_cap_rejected_request_pays_for_no_search_it_refused() {
    let limited = Engine::builder()
        .limits(
            Limits::default()
                .with_max_duration_minutes(60)
                .with_max_search_window_days(0)
                .with_max_candidate_starts(0),
        )
        .build()
        .expect("the bundled database builds");

    // A booking past the duration bound, over a window and a grid a search would
    // report as past their own limits too.
    let mut request = with_duration(booking("UTC", day()), Duration::from_minutes(90));
    request.goal.grid = Some(
        Grid::new(
            Duration::from_minutes(1),
            TimeOfDay::MIDNIGHT,
            GridZone::Named(name("UTC")),
        )
        .expect("a one-minute grid is valid"),
    );

    let diagnostics = limited.validate(&request);
    assert!(
        codes(&diagnostics).contains(&"part_duration_too_long"),
        "{:?}",
        diagnostics.errors
    );
    for search_only in ["search_window_limit", "candidates_limit"] {
        assert!(
            !codes(&diagnostics).contains(&search_only),
            "a capped request is not also searched for {search_only}: {:?}",
            diagnostics.errors
        );
    }
    assert_eq!(
        limited.find_availability(&request).unwrap_err().code(),
        "invalid_request"
    );

    // A structural failure is not a cap, so the checks a search needs still run
    // and report alongside it.
    let mut structural = booking("UTC", day());
    structural.participants[0].zone = name("Mars/Olympus_Mons");
    structural.goal.grid = Some(
        Grid::new(
            Duration::from_minutes(30),
            TimeOfDay::MIDNIGHT,
            GridZone::Named(name("UTC")),
        )
        .expect("a half-hour grid is valid"),
    );
    structural.goal.candidates = Some(vec![utc(2027, 6, 14, 9, 0)]);
    let diagnostics = engine().validate(&structural);
    assert_eq!(
        path(&diagnostics, "zone_unknown").as_deref(),
        Some("/participants/0/zone"),
        "{:?}",
        diagnostics.errors
    );
    assert!(
        codes(&diagnostics).contains(&"grid_and_candidates"),
        "a structural failure does not silence the search checks: {:?}",
        diagnostics.errors
    );
}

/// The short-circuit covers the search's warnings too, not only its error codes.
/// The same request warns `no_candidate_in_window` when the search runs, so the
/// silence under a cap is the candidate walk not happening rather than a request
/// that could never have warned.
#[test]
fn a_cap_rejected_request_reports_no_search_warning_it_did_not_earn() {
    let nine = TimeOfDay::new(9, 0).expect("a real time of day");
    let seventeen = TimeOfDay::new(17, 0).expect("a real time of day");
    let mut request = booking("UTC", day());
    request.goal.grid = None;
    request.goal.candidates = Some(vec![utc(2027, 6, 14, 6, 0)]);
    request.rules.push(
        presets::business_hours(
            RuleZone::Participant,
            Weekdays::range(Weekday::Monday, Weekday::Friday),
            nine,
            WallEnd::Time(seventeen),
        )
        .expect("the fixture hours are valid"),
    );

    let searched = engine().validate(&request);
    assert!(searched.is_valid(), "{:?}", searched.errors);
    assert!(
        searched
            .warnings
            .iter()
            .any(|warning| warning.code() == "no_candidate_in_window"),
        "the only candidate falls outside the hours, which a search warns about: {:?}",
        searched.warnings
    );

    let capped = Engine::builder()
        .limits(Limits::default().with_max_participants(0))
        .build()
        .expect("the bundled database builds");
    let diagnostics = capped.validate(&request);
    assert_eq!(codes(&diagnostics), ["participants_limit"]);
    assert!(
        diagnostics.warnings.is_empty(),
        "the cap rejected the request before the walk that warns: {:?}",
        diagnostics.warnings
    );
}

#[test]
fn an_oversized_explicit_candidate_list_is_rejected_at_its_own_field() {
    let engine = engine();
    let limit = engine.limits().max_candidate_starts;
    let count = limit + 1;
    let mut request = booking("UTC", span(Instant::MIN, Instant::MAX));
    request.goal.candidates = Some(
        (0..count)
            .map(|offset| {
                Instant::from_minutes(Instant::MIN.minutes() + i64::try_from(offset).unwrap())
                    .expect("the offset stays inside the range")
            })
            .collect(),
    );
    let diagnostics = engine.validate(&request);
    assert_eq!(
        path(&diagnostics, "candidates_limit").as_deref(),
        Some("/goal/candidates")
    );
    // The finding names the list's own length, not a truncated start count.
    let finding = diagnostics
        .errors
        .iter()
        .find(|finding| finding.code() == "candidates_limit")
        .expect("the candidate cap is reported");
    assert!(
        finding.message().contains(&count.to_string()),
        "the finding names {count}: {}",
        finding.message()
    );
    assert_eq!(
        engine.find_availability(&request).unwrap_err().code(),
        "invalid_request"
    );
}

/// A grid that names a zone the database does not carry cannot produce a start.
/// Validation says so rather than answering with a silently empty search the caller
/// would read as "nothing is bookable"; the grid's own shape check names the zone
/// too, which is this validator's rule for a mistake written in more than one place.
#[test]
fn a_candidate_generation_failure_is_reported_rather_than_swallowed() {
    let engine = engine();
    let mut request = booking("UTC", day());
    request.goal.candidates = None;
    request.goal.grid = Some(
        Grid::new(
            Duration::from_minutes(30),
            TimeOfDay::MIDNIGHT,
            GridZone::Named(name("Mars/Olympus_Mons")),
        )
        .expect("a half-hour step is a valid grid"),
    );
    let diagnostics = engine.validate(&request);
    assert!(
        codes(&diagnostics).contains(&"zone_unknown"),
        "the unresolvable grid zone is reported: {:?}",
        diagnostics.errors
    );

    // One bad zone name does not hide another: a participant's unknown zone and
    // the grid's are both named, however many findings share the code.
    request.participants[0].zone = name("Mars/Olympus_Mons");
    let diagnostics = engine.validate(&request);
    let named: Vec<Option<String>> = diagnostics
        .errors
        .iter()
        .filter(|finding| finding.code() == "zone_unknown")
        .map(|finding| finding.path().map(str::to_owned))
        .collect();
    assert!(
        named.contains(&Some("/participants/0/zone".to_owned()))
            && named.contains(&Some("/goal/grid".to_owned())),
        "both unknown zones are reported: {named:?}"
    );

    assert_eq!(
        engine.find_availability(&request).unwrap_err().code(),
        "invalid_request"
    );
}

/// A generation failure is named at the field that wrote it. A goal with no grid
/// generates from the default grid, whose zone is the goal's own, so the failure
/// belongs to `/goal/zone`: pointing at `/goal/grid` would send the caller to a
/// field their document does not carry.
#[test]
fn a_generation_failure_is_reported_at_the_field_that_wrote_it() {
    let engine = engine();
    let mut request = booking("UTC", day());
    request.goal.candidates = None;
    request.goal.grid = None;
    request.goal.zone = name("Mars/Olympus_Mons");

    let diagnostics = engine.validate(&request);
    assert_eq!(
        path(&diagnostics, "zone_unknown").as_deref(),
        Some("/goal/zone"),
        "the unknown goal zone is reported at the zone field: {:?}",
        diagnostics.errors
    );
    assert!(
        !diagnostics
            .errors
            .iter()
            .any(|finding| finding.path() == Some("/goal/grid")),
        "no finding names a grid the request does not carry: {:?}",
        diagnostics.errors
    );
}

/// A start whose booking ends past the last supported instant has no end the model
/// can report, so a search neither offers it nor lists it as excluded: nothing in
/// its answer claims the slot is bookable. Every offer a search does make is one a
/// confirmation accepts, so the two entry points agree on every start a caller can
/// reach from an offer.
#[test]
fn a_booking_the_model_cannot_describe_is_never_offered() {
    let engine = engine();
    let last_minute = Instant::from_minutes(Instant::MAX.minutes() - 1).expect("a real instant");
    let window = span(
        Instant::from_minutes(Instant::MAX.minutes() - 24 * 60).expect("a real instant"),
        Instant::MAX,
    );
    let mut request = booking("UTC", window);
    request.goal.candidates = Some(vec![last_minute, Instant::MAX]);
    request.explain = Some(opportune::model::Explain::new(100));

    let response = engine.find_availability(&request).expect("the search runs");
    assert!(
        response.offers.is_empty(),
        "no offer can name an end past the range: {:?}",
        response.offers
    );
    assert!(
        response.excluded.is_empty(),
        "and no exclusion claims the search judged it: {:?}",
        response.excluded
    );

    // A start the search can describe is offered and confirms, which is the pair a
    // caller acts on.
    let describable = Instant::from_minutes(Instant::MAX.minutes() - 30).expect("a real instant");
    request.goal.candidates = Some(vec![describable]);
    let response = engine.find_availability(&request).expect("the search runs");
    let offer = response
        .offers
        .first()
        .expect("the describable start is offered");
    let check = engine
        .check_slot(&request, &offer.to_proposal())
        .expect("the offer confirms");
    assert!(check.valid, "{:?}", check.reasons);
}

#[test]
fn date_counts_include_the_local_dates_a_window_touches_at_the_range_edges() {
    let engine = engine();

    // 1975-01-01T00:00Z is 1975-01-01T09:00 in Asia/Tokyo, and that local date's
    // midnight lies before the range: the date is still touched, so it is counted.
    let window = span(
        Instant::MIN,
        Instant::from_minutes(Instant::MIN.minutes() + 2 * 24 * 60).expect("a real instant"),
    );
    let response = engine
        .find_availability(&booking("Asia/Tokyo", window))
        .expect("the search runs");
    assert_eq!(
        response.dates.first().map(|count| count.date),
        Some(Date::new(1975, 1, 1).unwrap()),
        "the first local date is the date the range's first instant falls on"
    );

    // 2100-01-01T00:00Z is 2099-12-31T19:00 in America/New_York, so the last local
    // date the window touches is 2099-12-31.
    let window = span(
        Instant::from_minutes(Instant::MAX.minutes() - 2 * 24 * 60).expect("a real instant"),
        Instant::MAX,
    );
    let response = engine
        .find_availability(&booking("America/New_York", window))
        .expect("the search runs");
    assert_eq!(
        response.dates.last().map(|count| count.date),
        Some(Date::new(2099, 12, 31).unwrap()),
        "the last local date is the date the range's last minute falls on"
    );
}

/// Newfoundland ended daylight saving at 00:01 local until 2010, so 2010-11-07
/// begins twice and the previous date's last hour follows its first midnight. A
/// window spanning the hand-back touches both dates even though it starts and ends
/// on the later one.
#[test]
fn date_counts_cover_a_backward_midnight_transition() {
    let engine = engine();
    let window = span(utc(2010, 11, 7, 2, 30), utc(2010, 11, 7, 3, 31));
    let response = engine
        .find_availability(&booking("America/St_Johns", window))
        .expect("the search runs");
    assert_eq!(
        response
            .dates
            .iter()
            .map(|count| count.date)
            .collect::<Vec<_>>(),
        vec![
            Date::new(2010, 11, 6).unwrap(),
            Date::new(2010, 11, 7).unwrap(),
        ],
        "both local dates the window touches are counted"
    );
}

/// Samoa crossed the date line eastward at the end of 2011, skipping 30 December
/// entirely. The window touches 29 and 31 December and 30 December, a date no
/// instant falls on, stays out.
#[test]
fn date_counts_omit_a_date_the_zone_skipped() {
    let engine = engine();
    let window = span(utc(2011, 12, 29, 10, 0), utc(2011, 12, 31, 10, 0));
    let response = engine
        .find_availability(&booking("Pacific/Apia", window))
        .expect("the search runs");
    assert_eq!(
        response
            .dates
            .iter()
            .map(|count| count.date)
            .collect::<Vec<_>>(),
        vec![
            Date::new(2011, 12, 29).unwrap(),
            Date::new(2011, 12, 31).unwrap(),
        ],
        "the skipped date is not counted"
    );
}

/// The dates a window touches, walked minute by minute: the reference the answer's
/// own summary must equal.
fn dates_walked(zone: &str, window: Span) -> Vec<Date> {
    let db = opportune::time::TimeZoneDatabase::bundled().expect("the bundled database builds");
    let zone = db.zone(name(zone).as_str()).expect("the zone is carried");
    let mut dates = Vec::new();
    let mut minute = window.start().minutes();
    while minute < window.end().minutes() {
        let instant = Instant::from_minutes(minute).expect("the window stays in range");
        let date = zone.zoned(instant).date();
        if !dates.contains(&date) {
            dates.push(date);
        }
        minute += 1;
    }
    dates
}

/// `dates` must be exactly the local dates the window's own minutes fall on — no
/// date the window does not touch, and none it does missing — and every offer
/// counted on exactly one of them. The documented cases pin named dates at the
/// edges and across two transitions; this walks every minute of each window in a
/// spread of zones, so a short walk, a lost date, an extra date and a double count
/// all surface as a difference from the reference rather than as a lucky pass on an
/// endpoint.
#[test]
fn date_counts_are_the_local_dates_the_windows_minutes_fall_on() {
    let engine = engine();
    let day = 24 * 60;
    let floor = span(
        Instant::MIN,
        Instant::from_minutes(Instant::MIN.minutes() + day).expect("a real instant"),
    );
    let ceiling = span(
        Instant::from_minutes(Instant::MAX.minutes() - day).expect("a real instant"),
        Instant::MAX,
    );
    let cases = [
        // Both range corners against both offset directions: a date whose midnight
        // lies outside the supported range is still touched by the instants after
        // it, and the date at `Instant::MAX` is the date its last minute reads.
        ("Asia/Tokyo", floor),
        ("America/New_York", floor),
        ("Asia/Tokyo", ceiling),
        ("America/New_York", ceiling),
        ("Asia/Kathmandu", floor),
        ("Pacific/Kiritimati", ceiling),
        ("Pacific/Apia", floor),
        ("Pacific/Apia", ceiling),
        // A one-minute window at each edge.
        (
            "America/New_York",
            span(
                Instant::MIN,
                Instant::from_minutes(Instant::MIN.minutes() + 1).expect("a real instant"),
            ),
        ),
        (
            "Asia/Tokyo",
            span(
                Instant::from_minutes(Instant::MAX.minutes() - 1).expect("a real instant"),
                Instant::MAX,
            ),
        ),
        // A window that ends at local midnight, at a transition into a skipped
        // midnight, and just past a repeated one: the walk is half-open, so the
        // minute before its end is the last one it touches.
        ("UTC", span(utc(2027, 6, 14, 0, 0), utc(2027, 6, 15, 0, 0))),
        (
            "America/Santiago",
            span(utc(2027, 9, 3, 4, 0), utc(2027, 9, 5, 4, 0)),
        ),
        (
            "America/St_Johns",
            span(utc(2010, 11, 6, 3, 0), utc(2010, 11, 7, 3, 1)),
        ),
        // Forward and backward transitions, a repeated midnight, a skipped date,
        // and the half-hour and three-quarter-hour zones.
        (
            "America/St_Johns",
            span(utc(2010, 11, 5, 0, 0), utc(2010, 11, 8, 0, 0)),
        ),
        (
            "Pacific/Apia",
            span(utc(2011, 12, 28, 0, 0), utc(2012, 1, 2, 0, 0)),
        ),
        (
            "Pacific/Kiritimati",
            span(utc(1994, 12, 28, 0, 0), utc(1995, 1, 2, 0, 0)),
        ),
        (
            "America/Santiago",
            span(utc(2027, 9, 3, 0, 0), utc(2027, 9, 7, 0, 0)),
        ),
        (
            "America/Sao_Paulo",
            span(utc(2018, 2, 16, 0, 0), utc(2018, 2, 20, 0, 0)),
        ),
        (
            "Australia/Lord_Howe",
            span(utc(2027, 4, 2, 0, 0), utc(2027, 4, 6, 0, 0)),
        ),
        (
            "Pacific/Chatham",
            span(utc(2027, 4, 2, 0, 0), utc(2027, 4, 6, 0, 0)),
        ),
        (
            "Africa/Casablanca",
            span(utc(2027, 2, 5, 0, 0), utc(2027, 2, 9, 0, 0)),
        ),
        (
            "Asia/Tehran",
            span(utc(2027, 3, 19, 0, 0), utc(2027, 3, 23, 0, 0)),
        ),
    ];
    for (zone, window) in cases {
        let response = engine
            .find_availability(&booking(zone, window))
            .expect("the search runs");
        let counted: Vec<Date> = response.dates.iter().map(|count| count.date).collect();
        assert_eq!(
            counted,
            dates_walked(zone, window),
            "{zone} {window:?}: the summary is the dates the window's minutes fall on"
        );
        assert_eq!(
            response
                .dates
                .iter()
                .map(|count| count.offers)
                .sum::<usize>(),
            response.offers.len(),
            "{zone} {window:?}: every offer is counted on exactly one date"
        );
    }
}

/// `Response::dates` is the *display* zone's reading of the window, and it must be
/// that zone's on both sides: the bounds the walk is taken between and the date each
/// offer is counted on. A summary whose bounds came from the event zone while the
/// counts came from the display zone would name dates no offer falls on (or count
/// offers on dates it never lists) and the totals would still agree, so the zones are
/// made to differ here and the reference is walked in the display zone.
#[test]
fn date_counts_are_the_display_zones_dates_when_it_differs_from_the_event_zone() {
    let engine = engine();
    let two_days = 2 * 24 * 60;
    let cases = [
        (
            // The display zone's first date begins before the supported range, which
            // the event zone never asks about.
            "Asia/Tokyo",
            "America/New_York",
            span(
                Instant::MIN,
                Instant::from_minutes(Instant::MIN.minutes() + two_days).expect("a real instant"),
            ),
        ),
        (
            "America/New_York",
            "Asia/Tokyo",
            span(
                Instant::from_minutes(Instant::MAX.minutes() - two_days).expect("a real instant"),
                Instant::MAX,
            ),
        ),
        (
            // A backward midnight in the display zone, invisible to the event zone.
            "UTC",
            "America/St_Johns",
            span(utc(2010, 11, 5, 0, 0), utc(2010, 11, 8, 0, 0)),
        ),
    ];
    for (event, display, window) in cases {
        let mut request = booking(event, window);
        request.goal.display_zone = name(display);
        request.participants[0].zone = name(event);
        let response = engine.find_availability(&request).expect("the search runs");
        let counted: Vec<Date> = response.dates.iter().map(|count| count.date).collect();
        assert_eq!(
            counted,
            dates_walked(display, window),
            "{event} displayed in {display}: the summary is the display zone's dates"
        );
        assert_eq!(
            response
                .dates
                .iter()
                .map(|count| count.offers)
                .sum::<usize>(),
            response.offers.len(),
            "{event} displayed in {display}: every offer is counted on a listed date"
        );
    }
}

#[test]
fn an_empty_event_key_is_rejected_by_the_domain_constructors() {
    let span = span(utc(2027, 6, 14, 8, 0), utc(2027, 6, 14, 9, 0));
    for key in ["", " ", "  \t "] {
        let error = Occurrence::new(key, span, vec!["ana".to_owned()])
            .expect_err("an event key that names no event is rejected");
        assert_eq!(error.code(), "event_key_empty", "{key:?}");

        let error = Block::new(span)
            .with_event_key(key)
            .expect_err("an event key that names no event is rejected");
        assert_eq!(error.code(), "event_key_empty", "{key:?}");
    }
    assert!(
        Block::new(span).with_event_key("evt_1").is_ok(),
        "a real key is kept"
    );
}

#[test]
fn an_empty_event_key_is_rejected_by_validation() {
    let engine = engine();
    let mut request = booking("UTC", day());
    request.occurrences = vec![Occurrence {
        event_key: "  ".to_owned(),
        span: span(utc(2027, 6, 14, 8, 0), utc(2027, 6, 14, 9, 0)),
        tags: Vec::new(),
        participants: vec!["ana".to_owned()],
    }];
    request.participants[0].calendar = Some(vec![Block {
        event_key: Some(String::new()),
        ..Block::new(span(utc(2027, 6, 14, 9, 0), utc(2027, 6, 14, 10, 0)))
    }]);

    let diagnostics = engine.validate(&request);
    assert_eq!(
        path(&diagnostics, "event_key_empty").as_deref(),
        Some("/participants/0/calendar/0/event_key")
    );
    assert!(
        diagnostics.errors.iter().any(|finding| {
            finding.code() == "event_key_empty"
                && finding.path() == Some("/occurrences/0/event_key")
        }),
        "the occurrence's key is reported at its own field: {:?}",
        diagnostics.errors
    );
    assert_eq!(
        engine.find_availability(&request).unwrap_err().code(),
        "invalid_request"
    );
}

/// A zero-count term asks for no host slot, so the slot cap alone lets any number
/// of them reach the joint fillability check, which builds a candidate list per
/// term. The term count is bounded beside it, and the request is answered before
/// the matching pass runs.
#[test]
fn a_goal_with_more_requirement_terms_than_the_cap_is_rejected_before_matching() {
    let engine = engine();
    let cap = engine.limits().max_requirement_slots;

    let terms = |extra: bool| {
        let mut terms: Vec<Term> = (1..cap)
            .map(|_| Term::at_least(0, Selector::True))
            .collect();
        terms.push(Term::person("ghost"));
        if extra {
            terms.push(Term::at_least(0, Selector::True));
        }
        terms
    };

    let mut request = booking("UTC", day());
    request.goal.parts[0].requirement.terms = terms(true);
    let diagnostics = engine.validate(&request);
    assert_eq!(
        path(&diagnostics, "requirement_terms_limit").as_deref(),
        Some("/goal/parts/0/requirement/all")
    );
    assert_eq!(
        codes(&diagnostics),
        ["requirement_terms_limit"],
        "the matching pass never runs past the cap: {:?}",
        diagnostics.errors
    );
    assert_eq!(
        engine.find_availability(&request).unwrap_err().code(),
        "invalid_request"
    );

    // Exactly the cap is inside it, and the request then reaches the pass that
    // names the host its requirement cannot fill.
    request.goal.parts[0].requirement.terms = terms(false);
    let diagnostics = engine.validate(&request);
    assert!(
        !codes(&diagnostics).contains(&"requirement_terms_limit"),
        "{:?}",
        diagnostics.errors
    );
    assert!(
        codes(&diagnostics).contains(&"participant_unknown"),
        "the matching pass runs at the cap: {:?}",
        diagnostics.errors
    );
}

/// A goal that is not exactly one part is rejected by `part_count`, so neither
/// the per-part limit scan nor requirement matching is work anybody asked for.
/// The requirements name hosts the request does not carry, and the second is
/// also past the term cap: both passes would report if either one ran.
#[test]
fn a_multi_part_goal_skips_the_requirement_pass() {
    let engine = engine();
    let mut request = booking("UTC", day());
    request.goal.parts[0].requirement = Requirement::new(vec![Term::person("ghost")]);
    let mut terms = vec![Term::person("spectre")];
    terms.extend(
        (0..engine.limits().max_requirement_slots).map(|_| Term::at_least(0, Selector::True)),
    );
    request.goal.parts.push(Part::new(
        Duration::from_minutes(30),
        Requirement::new(terms),
    ));

    let diagnostics = engine.validate(&request);
    assert_eq!(
        codes(&diagnostics),
        ["part_count"],
        "{:?}",
        diagnostics.errors
    );
    assert_eq!(
        path(&diagnostics, "part_count").as_deref(),
        Some("/goal/parts")
    );
    assert_eq!(
        engine.find_availability(&request).unwrap_err().code(),
        "invalid_request"
    );
}

/// A block's travel widens the span it holds as well as the span the booking is
/// judged against, so it is bounded like the booking's own travel.
#[test]
fn an_extreme_block_travel_is_rejected_where_it_was_written() {
    let engine = engine();
    let held = Block::new(span(utc(2027, 6, 14, 8, 0), utc(2027, 6, 14, 9, 0)));
    let too_long = Duration::from_minutes(engine.limits().max_duration_minutes + 1);
    let maximum = Duration::from_minutes(engine.limits().max_duration_minutes);

    for (block, field) in [
        (held.clone().with_travel_before(too_long), "before"),
        (held.clone().with_travel_after(too_long), "after"),
    ] {
        let mut request = booking("UTC", day());
        request.participants[0].calendar = Some(vec![block]);
        let diagnostics = engine.validate(&request);
        assert_eq!(
            path(&diagnostics, "block_travel_duration_too_long"),
            Some(format!("/participants/0/calendar/0/travel_{field}")),
            "{:?}",
            diagnostics.errors
        );
        assert_eq!(
            engine.find_availability(&request).unwrap_err().code(),
            "invalid_request"
        );
    }

    // The bound itself is allowed, as it is for the booking's own travel.
    let mut request = booking("UTC", day());
    request.participants[0].calendar = Some(vec![held.with_travel_before(maximum)]);
    let diagnostics = engine.validate(&request);
    assert!(
        !codes(&diagnostics).contains(&"block_travel_duration_too_long"),
        "{:?}",
        diagnostics.errors
    );
}

/// A duration finding names the part, rule, participant and block it was written
/// on, not the first entry of each list: a plan that carries several parts, rules
/// or hosts sends an operator to the wrong one if the pointer keeps any index at
/// zero.
#[test]
fn a_duration_finding_names_the_entry_it_was_written_on() {
    let engine = engine();
    let too_long = Duration::from_minutes(engine.limits().max_duration_minutes + 1);

    // The second part of a multi-part goal, reported by the cap phase before the
    // part count it also fails.
    let mut request = booking("UTC", day());
    request.goal.parts.push(Part::new(
        too_long,
        Requirement::new(vec![Term::person("bo")]),
    ));
    let diagnostics = engine.validate(&request);
    assert_eq!(
        path(&diagnostics, "part_duration_too_long").as_deref(),
        Some("/goal/parts/1/duration"),
        "{:?}",
        diagnostics.errors
    );

    // The second rule's clearance.
    let mut request = booking("UTC", day());
    request.rules = vec![
        Rule::new(
            "fine",
            RuleKind::Clearance(Clearance {
                before: Duration::ZERO,
                after: Duration::ZERO,
                min_gap: Duration::ZERO,
            }),
        ),
        Rule::new(
            "wide",
            RuleKind::Clearance(Clearance {
                before: too_long,
                after: Duration::ZERO,
                min_gap: Duration::ZERO,
            }),
        ),
    ];
    let diagnostics = engine.validate(&request);
    assert_eq!(
        path(&diagnostics, "clearance_duration_too_long").as_deref(),
        Some("/rules/1/params/before"),
        "{:?}",
        diagnostics.errors
    );

    // The third block of the second participant.
    let held = |hour: u8| {
        Block::new(span(
            utc(2027, 6, 14, hour, 0),
            utc(2027, 6, 14, hour + 1, 0),
        ))
    };
    let mut request = booking("UTC", day());
    let mut host = Participant::new("bo", name("UTC"));
    host.calendar = Some(vec![
        held(8),
        held(10),
        held(12).with_travel_after(too_long),
    ]);
    request.participants.push(host);
    let diagnostics = engine.validate(&request);
    assert_eq!(
        path(&diagnostics, "block_travel_duration_too_long").as_deref(),
        Some("/participants/1/calendar/2/travel_after"),
        "{:?}",
        diagnostics.errors
    );
}

/// One participant carrying `count` consecutive one-minute zone periods, which are
/// disjoint by construction.
fn with_zone_periods(mut request: Request, count: usize) -> Request {
    let base = utc(2027, 6, 14, 0, 0).minutes();
    request.participants[0].zone_periods = (0..count)
        .map(|index| {
            let offset = i64::try_from(index).expect("the count fits in an i64");
            ZonePeriod::new(
                name("UTC"),
                Instant::from_minutes(base + offset).expect("the period starts in range"),
                Instant::from_minutes(base + offset + 1).expect("the period ends in range"),
            )
            .expect("one-minute periods are disjoint")
        })
        .collect();
    request
}

/// The zone-period cap is enforced by every entry point, before any schedule is
/// resolved from the list it bounds.
#[test]
fn the_zone_period_cap_is_enforced_by_every_entry_point() {
    let engine = engine();
    let cap = engine.limits().max_zone_periods_per_participant;
    assert_eq!(cap, 1_000);

    let request = with_zone_periods(booking("UTC", day()), cap + 1);
    let diagnostics = engine.validate(&request);
    assert_eq!(
        path(&diagnostics, "zone_periods_limit").as_deref(),
        Some("/participants/0/zone_periods")
    );
    assert_eq!(codes(&diagnostics), ["zone_periods_limit"]);
    assert_eq!(
        engine.find_availability(&request).unwrap_err().code(),
        "invalid_request"
    );

    let proposed = proposal(utc(2027, 6, 14, 9, 0));
    let error = engine
        .check_slot(&request, &proposed)
        .expect_err("a confirmation is refused by the same cap");
    let Error::InvalidRequest { errors, .. } = error else {
        panic!("the cap is a rejected request, not a malformed proposal");
    };
    assert_eq!(errors[0].code(), "zone_periods_limit");

    // The cap bounds the list rather than the request: a participant exactly at it
    // is valid.
    let at_cap = with_zone_periods(booking("UTC", day()), cap);
    let diagnostics = engine.validate(&at_cap);
    assert!(
        !codes(&diagnostics).contains(&"zone_periods_limit"),
        "{:?}",
        diagnostics.errors
    );
}

/// The ranges entry point answers the same cap the same way, before it resolves a
/// schedule from the periods it refuses.
#[test]
fn required_ranges_refuses_zone_periods_past_the_cap() {
    let tiny = Engine::builder()
        .limits(Limits::default().with_max_zone_periods_per_participant(1))
        .build()
        .expect("the bundled database builds");
    let mut over = with_zone_periods(booking("UTC", day()), 2);
    // A second participant over the cap is named too, not only the first.
    let mut second = over.participants[0].clone();
    second.id = "bo".to_owned();
    over.participants.push(second);
    let mut query = RangesQuery::new(over.now, over.goal.clone());
    query.participants = over.participants.clone();

    let error = tiny
        .required_ranges(&query)
        .expect_err("the cap is enforced before any schedule is resolved");
    assert_eq!(error.code(), "invalid_request");
    let Error::InvalidRequest { errors, .. } = error else {
        panic!("the cap is a rejected query, not a time zone failure");
    };
    let named: Vec<_> = errors
        .iter()
        .map(|finding| (finding.code(), finding.path()))
        .collect();
    assert_eq!(
        named,
        [
            ("zone_periods_limit", Some("/participants/0/zone_periods")),
            ("zone_periods_limit", Some("/participants/1/zone_periods")),
        ]
    );
    // The request validator names the same findings, so the two entry points tell
    // a caller the same thing about the same document.
    assert_eq!(
        tiny.validate(&over)
            .errors
            .iter()
            .map(|finding| (finding.code(), finding.path()))
            .collect::<Vec<_>>(),
        named
    );

    // At the cap the query is answered, so the rejection is the list's length and
    // not the request's shape.
    let mut at_cap = with_zone_periods(booking("UTC", day()), 1);
    at_cap.goal = query.goal.clone();
    let mut within = RangesQuery::new(at_cap.now, at_cap.goal.clone());
    within.participants = at_cap.participants;
    assert!(
        tiny.required_ranges(&within).is_ok(),
        "a participant at the cap is not refused"
    );
}

/// The counts a query multiplies are bounded on the ranges entry point as they are
/// on a request: a document past the participant or rule cap is refused before the
/// loops that multiply the two.
#[test]
fn required_ranges_refuses_the_counts_it_multiplies_past_the_cap() {
    let tiny = Engine::builder()
        .limits(Limits::default().with_max_participants(1).with_max_rules(1))
        .build()
        .expect("the bundled database builds");

    let base = utc(2027, 6, 14, 0, 0).minutes();
    let period = |start: i64, end: i64| {
        ZonePeriod::new(
            name("UTC"),
            Instant::from_minutes(base + start).expect("the period starts in range"),
            Instant::from_minutes(base + end).expect("the period ends in range"),
        )
        .expect("a period covers whole minutes")
    };
    let mut over = booking("UTC", day());
    // Periods that overlap: resolving a schedule from them fails, so the caps
    // below answered before a schedule was read.
    over.participants[0].zone_periods = vec![period(0, 120), period(60, 180)];
    let mut second = over.participants[0].clone();
    second.id = "bo".to_owned();
    over.participants.push(second);
    over.rules = vec![
        Rule::new(
            "weekly",
            RuleKind::Cap(Cap::new(
                Selector::True,
                CapMeasure::Count,
                5,
                CapPeriod::Day,
                CapPer::Each,
                RuleZone::Participant,
                Weekday::Monday,
            )),
        ),
        Rule::new("gaps", RuleKind::Clearance(Clearance::default())),
    ];
    let mut query = RangesQuery::new(over.now, over.goal.clone());
    query.participants = over.participants.clone();
    query.rules = over.rules.clone();

    let error = tiny
        .required_ranges(&query)
        .expect_err("both counts are past the caps");
    assert_eq!(error.code(), "invalid_request");
    let Error::InvalidRequest { errors, .. } = error else {
        panic!("the counts are a rejected query, not a time zone failure");
    };
    let named: Vec<_> = errors
        .iter()
        .map(|finding| (finding.code(), finding.path()))
        .collect();
    assert_eq!(
        named,
        [
            ("participants_limit", Some("/participants")),
            ("rules_limit", Some("/rules")),
        ]
    );
    // The request validator names the same findings, so the two entry points tell
    // a caller the same thing about the same document.
    assert_eq!(
        tiny.validate(&over)
            .errors
            .iter()
            .map(|finding| (finding.code(), finding.path()))
            .collect::<Vec<_>>(),
        named
    );

    // With the counts inside their caps, resolving a schedule from the overlapping
    // periods is what fails, so the caps above reached the query first.
    let roomy = Engine::builder()
        .build()
        .expect("the bundled database builds");
    assert_eq!(
        roomy.required_ranges(&query).unwrap_err().code(),
        "zone_periods_overlap"
    );

    // At the caps the query is answered, so the rejection is the counts and not the
    // document's shape.
    let at_cap = booking("UTC", day());
    let mut within = RangesQuery::new(at_cap.now, query.goal.clone());
    within.participants = at_cap.participants;
    within.rules = vec![Rule::new("gaps", RuleKind::Clearance(Clearance::default()))];
    assert!(
        tiny.required_ranges(&within).is_ok(),
        "a query at both caps is not refused"
    );
}

/// An ID that names nothing cannot be cited by a reason, a weight or an
/// assignment, so it is rejected where it was written.
#[test]
fn an_empty_id_is_rejected_where_it_was_written() {
    let engine = engine();
    for id in ["", "   ", " \t "] {
        let mut request = booking("UTC", day());
        request.participants[0].id = id.to_owned();
        assert_eq!(
            path(&engine.validate(&request), "participant_id_empty").as_deref(),
            Some("/participants/0/id"),
            "{id:?}"
        );
        // The finding is a validation diagnostic, not a code of the rejection: the
        // entry points answer `invalid_request` and this rides inside it, so a handler
        // that matches `err.code()` on `participant_id_empty` matches nothing.
        match engine.find_availability(&request) {
            Err(Error::InvalidRequest { errors, .. }) => assert!(
                errors
                    .iter()
                    .any(|error| error.code() == "participant_id_empty"
                        && error.path() == Some("/participants/0/id")),
                "the diagnostic rides inside the rejection: {errors:?}"
            ),
            other => panic!("a blank participant ID is rejected by the search: {other:?}"),
        }

        let mut request = booking("UTC", day());
        request.rules = vec![Rule::new(id, RuleKind::Clearance(Clearance::default()))];
        assert_eq!(
            path(&engine.validate(&request), "rule_id_empty").as_deref(),
            Some("/rules/0/id"),
            "{id:?}"
        );

        let mut request = booking("UTC", day());
        request.goal.parts[0].id = id.to_owned();
        assert_eq!(
            path(&engine.validate(&request), "part_id_empty").as_deref(),
            Some("/goal/parts/0/id"),
            "{id:?}"
        );
    }
}

/// A cap applies only when its `when` and its `count` both select the booking, so a
/// `count` that matches no part leaves the cap inert — a typo worth naming rather
/// than silently ignoring.
#[test]
fn a_cap_whose_count_matches_no_part_warns_at_its_selector() {
    let engine = engine();
    let cap = Cap::new(
        Selector::tag(tag("interviwe")),
        CapMeasure::Count,
        2,
        CapPeriod::Day,
        CapPer::Each,
        RuleZone::Participant,
        Weekday::Monday,
    );
    let mut request = booking("UTC", day());
    request.goal.parts[0].tags = vec![tag("interview")];
    request.rules = vec![Rule::new("interviews", RuleKind::Cap(cap))];

    let diagnostics = engine.validate(&request);
    assert!(diagnostics.is_valid(), "{:?}", diagnostics.errors);
    assert_eq!(
        warning_path(&diagnostics, "count_matches_no_part").as_deref(),
        Some("/rules/0/params/count")
    );

    // A `count` that matches the part is a cap that constrains it, so it is quiet.
    let matching = Cap::new(
        Selector::tag(tag("interview")),
        CapMeasure::Count,
        2,
        CapPeriod::Day,
        CapPer::Each,
        RuleZone::Participant,
        Weekday::Monday,
    );
    request.rules = vec![Rule::new("interviews", RuleKind::Cap(matching))];
    let diagnostics = engine.validate(&request);
    assert_eq!(warning_path(&diagnostics, "count_matches_no_part"), None);
}

/// `late_after` is the evening threshold and `not_before` the next morning's, so a
/// rule with them the wrong way round can protect daytime hours rather than the
/// intended night. That is a warning, not a rejection: the rule still means what it
/// says.
#[test]
fn a_rest_rule_with_swapped_thresholds_warns_without_failing() {
    let engine = engine();
    let ten = TimeOfDay::new(10, 0).expect("a real time of day");
    let eighteen = TimeOfDay::new(18, 0).expect("a real time of day");

    let mut request = booking("UTC", day());
    request.rules = vec![Rule::new("rest", RuleKind::Rest(Rest::new(ten, eighteen)))];
    let diagnostics = engine.validate(&request);
    assert!(diagnostics.is_valid(), "{:?}", diagnostics.errors);
    assert_eq!(
        warning_path(&diagnostics, "rest_window_overlaps_day").as_deref(),
        Some("/rules/0/params")
    );

    // The documented order — evening, then the next morning — is quiet.
    request.rules = vec![Rule::new("rest", RuleKind::Rest(Rest::new(eighteen, ten)))];
    let diagnostics = engine.validate(&request);
    assert_eq!(warning_path(&diagnostics, "rest_window_overlaps_day"), None);
}

/// The hours a booking's start needs, as a rule that applies to every host.
fn weekday_hours(from: TimeOfDay, until: TimeOfDay, days: Weekdays) -> Rule {
    presets::business_hours(RuleZone::Participant, days, from, WallEnd::Time(until))
        .expect("the fixture hours are valid")
}

fn nine_to_five() -> TimeOfDay {
    TimeOfDay::new(9, 0).expect("a real time of day")
}

fn five_pm() -> TimeOfDay {
    TimeOfDay::new(17, 0).expect("a real time of day")
}

/// A start inside a window is not a booking inside it: the hours have to hold the
/// whole occupied range, which is the booking plus the participant's travel.
#[test]
fn no_candidate_in_window_judges_the_whole_occupied_range() {
    let engine = engine();
    let week = Weekdays::range(Weekday::Monday, Weekday::Friday);

    // The start is inside the hours and the booking's end is not.
    let mut late = booking("UTC", day());
    late.goal.parts[0].duration = Duration::from_minutes(60);
    late.goal.candidates = Some(vec![utc(2027, 6, 14, 16, 30)]);
    late.rules = vec![weekday_hours(nine_to_five(), five_pm(), week)];
    let diagnostics = engine.validate(&late);
    assert!(
        warning_codes(&diagnostics).contains(&"no_candidate_in_window"),
        "a booking that ends past the hours does not fit: {:?}",
        diagnostics.warnings
    );

    // The same booking finishing exactly at the close of the hours does fit.
    let mut exactly = late.clone();
    exactly.goal.candidates = Some(vec![utc(2027, 6, 14, 16, 0)]);
    let diagnostics = engine.validate(&exactly);
    assert!(
        !warning_codes(&diagnostics).contains(&"no_candidate_in_window"),
        "a booking that ends when the hours do fits: {:?}",
        diagnostics.warnings
    );

    // Travel belongs to the range: a start inside the hours whose travel reaches
    // before they open is a start no search can offer.
    let mut travelled = booking("UTC", day());
    travelled.goal.candidates = Some(vec![utc(2027, 6, 14, 9, 15)]);
    travelled.goal.parts[0].travel.before = Duration::from_minutes(30);
    travelled.rules = vec![weekday_hours(nine_to_five(), five_pm(), week)];
    let diagnostics = engine.validate(&travelled);
    assert!(
        warning_codes(&diagnostics).contains(&"no_candidate_in_window"),
        "the travel before the booking falls outside the hours: {:?}",
        diagnostics.warnings
    );
}

/// Candidate generation stops at the configured maximum plus one, so a truncated
/// walk never produced the starts that might have fitted. The absence of one then
/// proves nothing and no warning is claimed.
#[test]
fn no_candidate_in_window_is_not_claimed_when_generation_was_truncated() {
    let mut request = booking("UTC", day());
    request.goal.candidates = None;
    request.goal.grid = Some(
        Grid::new(
            Duration::from_minutes(1),
            TimeOfDay::MIDNIGHT,
            GridZone::Named(name("UTC")),
        )
        .expect("a one-minute grid is valid"),
    );
    // The search window is a Monday and the only window that applies works
    // Sundays, so no generated start can fit.
    request.rules = vec![weekday_hours(
        nine_to_five(),
        five_pm(),
        Weekdays::single(Weekday::Sunday),
    )];

    let truncated = Engine::builder()
        .limits(Limits::default().with_max_candidate_starts(1))
        .build()
        .expect("the bundled database builds");
    let diagnostics = truncated.validate(&request);
    assert!(
        codes(&diagnostics).contains(&"candidates_limit"),
        "{:?}",
        diagnostics.errors
    );
    assert!(
        !warning_codes(&diagnostics).contains(&"no_candidate_in_window"),
        "a truncated generation claims nothing about the starts it never produced: {:?}",
        diagnostics.warnings
    );

    // With room for every start the search sees that none fits, so the warning is
    // earned rather than merely unprovable.
    let complete = engine();
    let diagnostics = complete.validate(&request);
    assert!(
        warning_codes(&diagnostics).contains(&"no_candidate_in_window"),
        "{:?}",
        diagnostics.warnings
    );
}

/// A search window the configured limit rejects still earns the window warning,
/// but two explicit starts at the edges must not make that warning build minutes
/// over the whole supported range between them.
#[test]
fn an_over_cap_search_window_still_reports_the_window_warning() {
    let engine = engine();
    let mut request = booking("UTC", span(Instant::MIN, Instant::MAX));
    request.goal.candidates = Some(vec![
        Instant::MIN,
        Instant::from_minutes(Instant::MAX.minutes() - 30)
            .expect("the booking ends at the supported edge"),
    ]);
    request.rules = vec![weekday_hours(
        nine_to_five(),
        five_pm(),
        Weekdays::single(Weekday::Sunday),
    )];

    let diagnostics = engine.validate(&request);
    assert!(
        codes(&diagnostics).contains(&"search_window_limit"),
        "{:?}",
        diagnostics.errors
    );
    assert!(
        warning_codes(&diagnostics).contains(&"no_candidate_in_window"),
        "the rejected search still reports why none of its starts fits: {:?}",
        diagnostics.warnings
    );

    // A fitting start in a later envelope group answers for the whole search:
    // the first group's failure must not produce a false warning.
    request.goal.candidates = Some(vec![Instant::MIN, utc(2027, 6, 20, 10, 0)]);
    let diagnostics = engine.validate(&request);
    assert!(codes(&diagnostics).contains(&"search_window_limit"));
    assert!(
        !warning_codes(&diagnostics).contains(&"no_candidate_in_window"),
        "the Sunday-morning candidate fits even though the first group does not: {:?}",
        diagnostics.warnings
    );
}

/// A window open around the clock, every day.
fn all_day_window() -> opportune::time::WindowSpec {
    presets::business_hours(
        RuleZone::Participant,
        Weekdays::range(Weekday::Monday, Weekday::Sunday),
        TimeOfDay::MIDNIGHT,
        WallEnd::Midnight,
    )
    .expect("a full day is a valid range")
    .window()
    .expect("the preset is a window rule")
    .clone()
}

/// The same hours, on Sundays only, so a booking of a day or more never fits it.
fn sunday_window() -> opportune::time::WindowSpec {
    presets::business_hours(
        RuleZone::Participant,
        Weekdays::single(Weekday::Sunday),
        TimeOfDay::MIDNIGHT,
        WallEnd::Midnight,
    )
    .expect("a full day is a valid range")
    .window()
    .expect("the preset is a window rule")
    .clone()
}

/// A complete candidate list at the configured ceiling is still judged against
/// every applicable window: the all-day windows do not make a week-long booking
/// fit the Sunday-only one.
#[test]
fn a_search_at_the_candidate_limit_is_judged_against_every_window() {
    let engine = engine();
    // A legal 91-day search window, so every candidate start lies inside it.
    let mut request = booking("UTC", span(utc(2027, 6, 14, 0, 0), utc(2027, 9, 13, 0, 0)));
    request.goal.parts[0].duration = Duration::from_minutes(engine.limits().max_duration_minutes);
    let first = utc(2027, 6, 14, 0, 0).minutes();
    request.goal.candidates = Some(
        (0..engine.limits().max_candidate_starts)
            .map(|offset| {
                Instant::from_minutes(
                    first + i64::try_from(offset).expect("the candidate count fits in an i64"),
                )
                .expect("the starts are inside the supported range")
            })
            .collect(),
    );
    let open = all_day_window();
    let mut rules: Vec<Rule> = (0..49)
        .map(|index| Rule::new(format!("open{index}"), RuleKind::Window(open.clone())))
        .collect();
    rules.push(Rule::new("sundays", RuleKind::Window(sunday_window())));
    request.rules = rules;

    let diagnostics = engine.validate(&request);
    assert!(
        warning_codes(&diagnostics).contains(&"no_candidate_in_window"),
        "a week-long booking fits no Sunday: {:?}",
        diagnostics.warnings
    );
}

/// The cap is answered before a schedule is resolved from the list it refuses: a
/// participant whose periods also overlap is named by the cap, not by the overlap
/// resolving the schedule would have reported.
#[test]
fn required_ranges_answers_the_zone_cap_before_resolving_the_schedule() {
    let tiny = Engine::builder()
        .limits(Limits::default().with_max_zone_periods_per_participant(1))
        .build()
        .expect("the bundled database builds");
    let base = utc(2027, 6, 14, 0, 0).minutes();
    let period = |start: i64, end: i64| {
        ZonePeriod::new(
            name("UTC"),
            Instant::from_minutes(base + start).expect("the period starts in range"),
            Instant::from_minutes(base + end).expect("the period ends in range"),
        )
        .expect("a period covers whole minutes")
    };
    let mut request = booking("UTC", day());
    request.participants[0].zone_periods = vec![period(0, 120), period(60, 180)];
    let mut query = RangesQuery::new(request.now, request.goal.clone());
    query.participants = request.participants.clone();

    let error = tiny
        .required_ranges(&query)
        .expect_err("the cap refuses the list before the schedule is read");
    let Error::InvalidRequest { errors, .. } = error else {
        panic!("the cap is a rejected query, not a time zone failure");
    };
    assert_eq!(
        errors
            .iter()
            .map(|finding| (finding.code(), finding.path()))
            .collect::<Vec<_>>(),
        [("zone_periods_limit", Some("/participants/0/zone_periods"))]
    );
    assert_eq!(codes(&tiny.validate(&request)), ["zone_periods_limit"]);

    // The periods really do overlap: with the cap lifted, resolving them is what
    // fails, so the cap above answered a question it reached first.
    let roomy = Engine::builder()
        .limits(Limits::default().with_max_zone_periods_per_participant(4))
        .build()
        .expect("the bundled database builds");
    assert_eq!(codes(&roomy.validate(&request)), ["zone_periods_overlap"]);
}

/// The window minutes are built once over the range the candidates occupy, so
/// that range can cross a zone transition. A start in the participant's summer
/// offset still has to be judged against the hours it falls on: London moves at
/// 01:00Z on 2027-03-28, and a booking at 16:00 local fits hours ending at 17:00.
#[test]
fn no_candidate_in_window_reads_windows_across_a_spring_forward() {
    let engine = engine();
    let mut request = booking(
        "Europe/London",
        span(utc(2027, 3, 28, 0, 0), utc(2027, 3, 29, 0, 0)),
    );
    request.goal.parts[0].duration = Duration::from_minutes(60);
    // 00:30Z is before the hours in the winter offset; 15:00Z is 16:00 in the
    // summer offset, so the booking ends exactly when the hours close.
    request.goal.candidates = Some(vec![utc(2027, 3, 28, 0, 30), utc(2027, 3, 28, 15, 0)]);
    request.rules = vec![weekday_hours(
        nine_to_five(),
        five_pm(),
        Weekdays::single(Weekday::Sunday),
    )];
    let diagnostics = engine.validate(&request);
    assert!(
        !warning_codes(&diagnostics).contains(&"no_candidate_in_window"),
        "the summer-offset start fits the hours: {:?}",
        diagnostics.warnings
    );

    // Thirty minutes later the same booking ends past the close, so the warning
    // is earned.
    request.goal.candidates = Some(vec![utc(2027, 3, 28, 0, 30), utc(2027, 3, 28, 15, 30)]);
    let diagnostics = engine.validate(&request);
    assert!(
        warning_codes(&diagnostics).contains(&"no_candidate_in_window"),
        "a booking ending past the close fits nothing: {:?}",
        diagnostics.warnings
    );
}

/// Caps that share one bucket shape ask for the same occurrence hull, however
/// many of them a query carries. At the participant and rule caps, the crowded
/// query must therefore return exactly the one-cap answer.
#[test]
fn required_ranges_with_repeated_cap_shapes_matches_one_cap() {
    let engine = engine();
    let catalogue = |index: usize| {
        Rule::new(
            format!("cap{index}"),
            RuleKind::Cap(Cap::new(
                Selector::True,
                CapMeasure::Count,
                5,
                CapPeriod::Week,
                CapPer::Each,
                RuleZone::Participant,
                Weekday::Monday,
            )),
        )
    };
    let base = utc(2027, 6, 14, 0, 0).minutes();
    let mut request = booking("UTC", day());
    request.goal.parts[0].requirement = Requirement::new(vec![Term::person("p0")]);
    request.participants = (0..engine.limits().max_participants)
        .map(|index| {
            let mut participant = Participant::new(format!("p{index}"), name("UTC"));
            participant.calendar = Some(Vec::new());
            participant.zone_periods = (0..100)
                .map(|offset| {
                    let start = base
                        + i64::try_from(index * 1_000 + offset * 2)
                            .expect("the fixture fits in an i64");
                    ZonePeriod::new(
                        name("UTC"),
                        Instant::from_minutes(start).expect("the period starts in range"),
                        Instant::from_minutes(start + 1).expect("the period ends in range"),
                    )
                    .expect("one-minute periods are disjoint")
                })
                .collect();
            participant
        })
        .collect();
    request.rules = (0..engine.limits().max_rules).map(catalogue).collect();
    let mut query = RangesQuery::new(request.now, request.goal.clone());
    query.participants = request.participants.clone();
    query.rules = request.rules.clone();

    let crowded = engine
        .required_ranges(&query)
        .expect("a query inside every cap is answered");

    // Every rule shares one bucket shape, so the hull must be the one a single cap
    // produces: same bucket, same reach, whatever the rule count.
    let mut single = query.clone();
    single.rules.truncate(1);
    let alone = engine
        .required_ranges(&single)
        .expect("the same query with one cap is answered");
    assert!(
        crowded.occurrences.is_some(),
        "a weekly cap over the window touches a bucket"
    );
    assert_eq!(crowded, alone);
}

/// One query can carry caps whose buckets differ, and the fetch hull has to cover
/// every one of them. The hull is computed once per participant and bucket shape,
/// so a shape the cache forgot would answer a later cap with an earlier cap's
/// bucket and hand the caller a range that reads a busy day as free.
#[test]
fn required_ranges_covers_every_caps_bucket_shape_in_one_query() {
    let engine = engine();
    let cap = |id: &str, period: CapPeriod, week_starts: Weekday| {
        Rule::new(
            id,
            RuleKind::Cap(Cap::new(
                Selector::True,
                CapMeasure::Count,
                1,
                period,
                CapPer::Each,
                RuleZone::Participant,
                week_starts,
            )),
        )
    };
    let request = booking("UTC", day());
    let mut query = RangesQuery::new(request.now, request.goal.clone());
    query.participants = request.participants.clone();
    let hull = |rules: Vec<Rule>| {
        let mut query = query.clone();
        query.rules = rules;
        engine
            .required_ranges(&query)
            .expect("the fixture query resolves")
            .occurrences
    };

    let day_bucket = Some(span(utc(2027, 6, 14, 0, 0), utc(2027, 6, 15, 0, 0)));
    let month_bucket = Some(span(utc(2027, 6, 1, 0, 0), utc(2027, 7, 1, 0, 0)));
    let sunday_week = Some(span(utc(2027, 6, 13, 0, 0), utc(2027, 6, 20, 0, 0)));
    assert_eq!(
        hull(vec![cap("day", CapPeriod::Day, Weekday::Monday)]),
        day_bucket
    );
    assert_eq!(
        hull(vec![cap("month", CapPeriod::Month, Weekday::Monday)]),
        month_bucket
    );
    assert_eq!(
        hull(vec![cap("week", CapPeriod::Week, Weekday::Sunday)]),
        sunday_week
    );

    // Every shape in one query reaches the widest bucket, which is the month: a
    // cache keyed by the participant alone would report the day bucket here.
    assert_eq!(
        hull(vec![
            cap("day", CapPeriod::Day, Weekday::Monday),
            cap("month", CapPeriod::Month, Weekday::Monday),
            cap("week", CapPeriod::Week, Weekday::Sunday),
        ]),
        month_bucket
    );
}
