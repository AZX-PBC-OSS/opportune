//! End-to-end single-participant scenarios: every use case and every rule-kind
//! fixture runs through the engine in all nine zones of the specification's
//! matrix.
//!
//! Two runners share one execution path. The breadth runner pins scenarios whose
//! windows span days: each names the starts it must offer and the starts it must
//! exclude with a specific reason, and the same request is re-checked with
//! `check_slot` for every offer and every probed exclusion. The exact runner pins
//! compact scenarios with a short explicit candidate list, so the search's whole
//! answer is compared with `assert_eq!` on both sides: a start silently swapped
//! between offered and excluded cannot pass either runner.

use std::sync::OnceLock;

use opportune::Engine;
use opportune::model::{
    Goal, Participant, Proposal, ProposalEvent, Request, Requirement, Response, Span, Term,
};
use opportune::presets;
use opportune::reasons::ReasonCode;
use opportune::time::{
    Duration, Grid, GridZone, Instant, RuleZone, TimeOfDay, TimeZoneDatabase, WallEnd, Weekday,
    Weekdays, ZoneName,
};
use opportune_testkit::scenarios::{
    ExactScenario, Scenario, a1_scenarios, exact_scenarios, rule_kind_scenarios, zone_matrix,
};

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

fn proposal(start: Instant, assignment: Vec<Vec<String>>) -> Proposal {
    Proposal {
        events: vec![ProposalEvent {
            part: "main".to_owned(),
            start,
            assignment,
        }],
    }
}

/// The candidate starts a scenario's search judges: the caller's own list when it
/// names one, the goal's grid when it names one, and otherwise the default grid —
/// less the ones whose booking would end past the last supported instant.
///
/// A scenario's window spans days, so the answer's exhaustive offer and exclusion
/// lists are hundreds of entries that no reader could check by hand. The fixture
/// names the starts it pins and this set supplies the rest of the accounting, from
/// the documented candidate contract: the caller's list, the goal's grid, or the
/// part's duration capped at thirty minutes anchored at midnight in the goal's
/// zone.
fn judged_starts(scenario: &Scenario) -> Vec<Instant> {
    let goal = &scenario.request.goal;
    let part = &goal.parts[0];
    let db = database();
    let window = goal.search_window;
    let mut starts: Vec<Instant> = if let Some(candidates) = &goal.candidates {
        candidates.clone()
    } else if let Some(grid) = &goal.grid {
        let zone = match grid.zone_choice() {
            GridZone::Event => goal.zone.clone(),
            GridZone::Named(name) => name.clone(),
        };
        grid.starts(
            &db.zone(zone.as_str())
                .expect("the grid's zone is in the database"),
            window.start(),
            window.end(),
        )
        .expect("the scenario's window walks")
    } else {
        let capped = part.duration.min(Duration::from_minutes(30));
        let step = if capped.minutes() < 1 {
            Duration::from_minutes(1)
        } else {
            capped
        };
        let grid = Grid::new(
            step,
            TimeOfDay::MIDNIGHT,
            GridZone::Named(goal.zone.clone()),
        )
        .expect("the default grid's step is between 1 and 1440 minutes");
        grid.starts(
            &db.zone(goal.zone.as_str())
                .expect("the scenario's zone is in the database"),
            window.start(),
            window.end(),
        )
        .expect("the scenario's window walks")
    };
    let latest = Instant::MAX.minutes() - part.duration.minutes();
    starts.retain(|start| window.contains(*start) && start.minutes() <= latest);
    starts.sort_unstable();
    starts.dedup();
    starts
}

/// What a fixture pins, in the shape both A1 runners share: the request, the host
/// it assigns, and the starts it names on each side.
struct Case<'a> {
    id: &'a str,
    zone: &'a str,
    host: &'a str,
    request: &'a Request,
    offered: &'a [Instant],
    excluded: &'a [(Instant, ReasonCode)],
}

impl<'a> Case<'a> {
    fn of(scenario: &'a Scenario) -> Self {
        Self {
            id: scenario.id,
            zone: &scenario.zone,
            host: &scenario.host,
            request: &scenario.request,
            offered: &scenario.offered,
            excluded: &scenario.excluded,
        }
    }

    fn of_exact(scenario: &'a ExactScenario) -> Self {
        Self {
            id: scenario.id,
            zone: &scenario.zone,
            host: &scenario.host,
            request: &scenario.request,
            offered: &scenario.offered,
            excluded: &scenario.excluded,
        }
    }

    fn context(&self) -> String {
        format!("{} in {}", self.id, self.zone)
    }
}

/// Runs the case's request through validation and the search, naming the scenario
/// when either refuses it.
fn execute(case: &Case<'_>) -> Response {
    let context = case.context();
    let diagnostics = engine().validate(case.request);
    assert!(
        diagnostics.is_valid(),
        "{context}: invalid request: {:?}",
        diagnostics.errors
    );
    engine()
        .find_availability(case.request)
        .unwrap_or_else(|err| panic!("{context}: the search refuses the request: {err}"))
}

fn offered_starts(response: &Response) -> Vec<Instant> {
    response
        .offers
        .iter()
        .map(|offer| offer.start().expect("an offer has an event"))
        .collect()
}

/// Every excluded start with the reason it reports, in report order. A one-host
/// request has one host to blame at each start, so each entry carries one reason.
fn exclusions(response: &Response, context: &str) -> Vec<(Instant, ReasonCode)> {
    response
        .excluded
        .iter()
        .map(|entry| {
            assert_eq!(
                entry.reasons.len(),
                1,
                "{context}: one host gives one reason at {}",
                entry.start
            );
            (entry.start, entry.reasons[0].code)
        })
        .collect()
}

/// Every offer confirms with `check_slot`, carrying the same reserve and flags.
fn confirm_offers(case: &Case<'_>, response: &Response) {
    let context = case.context();
    for offer in &response.offers {
        let start = offer.start().expect("an offer has an event");
        let assignment = offer.events[0].assignment.clone();
        let slot = engine()
            .check_slot(case.request, &proposal(start, assignment))
            .unwrap();
        assert!(slot.valid, "{context}: offered {start} fails check_slot");
        assert_eq!(slot.reserve, offer.events[0].reserve, "{context}: reserve");
        assert_eq!(slot.flags, offer.flags, "{context}: flags");
    }
}

/// Every start the fixture excludes is refused by `check_slot`, for the reason the
/// fixture names.
fn confirm_exclusions(case: &Case<'_>) {
    let context = case.context();
    for (start, code) in case.excluded {
        let slot = engine()
            .check_slot(
                case.request,
                &proposal(*start, vec![vec![case.host.to_owned()]]),
            )
            .unwrap();
        assert!(!slot.valid, "{context}: excluded {start} passes check_slot");
        assert_eq!(
            slot.reasons.first().map(|reason| reason.code),
            Some(*code),
            "{context}: check_slot's first reason at {start}"
        );
    }
}

/// A breadth scenario: the fixture names representative starts out of a window
/// that spans days, so the named ones are checked in place and the offers and
/// exclusions together must account for every judged start exactly once.
fn assert_scenario(scenario: &Scenario) {
    let case = Case::of(scenario);
    let context = case.context();
    let response = execute(&case);

    let offered = offered_starts(&response);
    for start in case.offered {
        assert!(
            offered.contains(start),
            "{context}: {start} must be offered; offers are {offered:?}"
        );
    }

    let excluded = exclusions(&response, &context);
    for (start, code) in case.excluded {
        assert!(
            !offered.contains(start),
            "{context}: {start} must not be offered"
        );
        let entry = excluded
            .iter()
            .find(|(excluded, _)| excluded == start)
            .unwrap_or_else(|| {
                panic!(
                    "{context}: {start} must be excluded; excluded starts are {:?}",
                    excluded.iter().map(|(start, _)| *start).collect::<Vec<_>>()
                )
            });
        assert_eq!(entry.1, *code, "{context}: reason code at {start}");
    }

    let mut accounted: Vec<Instant> = offered.clone();
    accounted.extend(excluded.iter().map(|(start, _)| *start));
    accounted.sort_unstable();
    assert_eq!(
        accounted,
        judged_starts(scenario),
        "{context}: the offers and exclusions account for every judged start once"
    );

    confirm_offers(&case, &response);
    confirm_exclusions(&case);
}

/// An exact scenario: the request names every start the search judges, short
/// enough that the whole answer is a handful of entries a reader can check by
/// hand, so both lists are compared as wholes.
fn assert_exact_scenario(scenario: &ExactScenario) {
    let case = Case::of_exact(scenario);
    let context = case.context();
    let mut accounted: Vec<Instant> = case.offered.to_vec();
    accounted.extend(case.excluded.iter().map(|(start, _)| *start));
    accounted.sort_unstable();
    let mut declared = case
        .request
        .goal
        .candidates
        .clone()
        .expect("an exact scenario names its candidates");
    declared.sort_unstable();
    assert_eq!(
        accounted, declared,
        "{context}: the fixture pins every candidate it declares, once"
    );
    let response = execute(&case);

    assert_eq!(
        offered_starts(&response),
        case.offered,
        "{context}: the whole offer list"
    );
    assert_eq!(
        exclusions(&response, &context),
        case.excluded,
        "{context}: the whole exclusion list, with each start's reason"
    );

    confirm_offers(&case, &response);
    confirm_exclusions(&case);
}

#[test]
fn every_a1_use_case_passes_in_every_zone() {
    for zone in zone_matrix() {
        let scenarios = a1_scenarios(database(), zone);
        let use_cases: Vec<&str> = scenarios.iter().map(|scenario| scenario.use_case).collect();
        for use_case in ["U1", "U2", "U14", "U16", "U18", "U21", "U22"] {
            assert!(
                use_cases.contains(&use_case),
                "{use_case} has a scenario in {zone}"
            );
        }
        for scenario in scenarios {
            assert_scenario(&scenario);
        }
    }
}

#[test]
fn every_a1_rule_kind_has_including_and_excluding_fixtures_in_every_zone() {
    for zone in zone_matrix() {
        let scenarios = rule_kind_scenarios(database(), zone);
        let kinds: Vec<&str> = scenarios.iter().map(|scenario| scenario.use_case).collect();
        // The seven names are spelled out here rather than read from
        // `RULE_KINDS`, so the list the fixtures must satisfy is pinned
        // independently of the constant the report and the stress run share.
        for kind in [
            "window",
            "blackout",
            "clearance",
            "lead_time",
            "cap",
            "rest",
            "status_policy",
        ] {
            assert!(kinds.contains(&kind), "{kind} has a fixture in {zone}");
        }
        for scenario in scenarios {
            assert!(
                !scenario.offered.is_empty() && !scenario.excluded.is_empty(),
                "{} in {zone} proves both answers",
                scenario.id
            );
            assert_scenario(&scenario);
        }
    }
}

#[test]
fn every_exact_a1_scenario_pins_its_whole_answer() {
    for zone in zone_matrix() {
        for scenario in exact_scenarios(database(), zone) {
            assert!(
                !scenario.offered.is_empty() && !scenario.excluded.is_empty(),
                "{} in {zone} proves both answers",
                scenario.id
            );
            assert_exact_scenario(&scenario);
        }
    }
}

#[test]
fn every_a1_core_use_case_and_rule_kind_has_an_exact_scenario() {
    for zone in zone_matrix() {
        let scenarios = exact_scenarios(database(), zone);
        let use_cases: Vec<&str> = scenarios.iter().map(|scenario| scenario.use_case).collect();
        // Spelled out here rather than read from the fixtures, so the list the
        // exact suite must satisfy is pinned independently of the cases that
        // satisfy it.
        for expected in [
            "U1",
            "U2",
            "U16",
            "window",
            "blackout",
            "clearance",
            "lead_time",
            "status_policy",
            "rest",
            "cap",
        ] {
            assert!(
                use_cases.contains(&expected),
                "{expected} has an exact scenario in {zone}"
            );
        }
        // Two names are carried by cases that pin different shapes: `U2` by the
        // split shift and the closed-date override, and `lead_time` by the
        // minimum notice and the latest start. A name an earlier case already
        // satisfies cannot notice the loss of the other, so those cases are named
        // themselves.
        let ids: Vec<&str> = scenarios.iter().map(|scenario| scenario.id).collect();
        for expected in [
            "exact-u2-weekly-hours",
            "exact-u2-closed-date",
            "exact-lead-time-min",
            "exact-lead-time-max",
        ] {
            assert!(
                ids.contains(&expected),
                "{expected} pins a shape its use case's name cannot: missing in {zone}"
            );
        }
    }
}

#[test]
fn u1_takes_about_ten_lines_with_the_sugar() {
    let zone = ZoneName::new("Europe/London").unwrap();
    let from = midnight("Europe/London", 2027, 6, 14);
    let until = midnight("Europe/London", 2027, 6, 19);
    let now = Instant::from_minutes(from.minutes() + 8 * 60).unwrap();
    let mut request = Request::new(
        now,
        Goal::single(
            zone.clone(),
            Span::new(from, until).unwrap(),
            Duration::from_minutes(30),
            Requirement::new(vec![Term::person("rich")]),
        ),
    );
    request.participants.push(Participant::new("rich", zone));
    request.rules.push(
        presets::business_hours(
            RuleZone::Participant,
            Weekdays::range(Weekday::Monday, Weekday::Friday),
            TimeOfDay::new(9, 0).unwrap(),
            WallEnd::Time(TimeOfDay::new(17, 0).unwrap()),
        )
        .unwrap(),
    );
    request.rules.push(presets::buffers(
        Duration::from_minutes(10),
        Duration::from_minutes(10),
    ));
    request
        .rules
        .push(presets::min_notice(Duration::from_hours_minutes(4, 0)));

    let response = engine().find_availability(&request).unwrap();
    assert!(!response.offers.is_empty());
}

/// Local midnight on a fixture date, which exists in every zone of the matrix.
fn midnight(zone: &str, year: i16, month: i8, day: i8) -> Instant {
    let date = opportune::time::Date::new(year, month, day).expect("a fixture date");
    let resolved = database()
        .zone(zone)
        .expect("the zone is in the database")
        .resolve_local(date, TimeOfDay::MIDNIGHT)
        .expect("the date is inside the supported range");
    match resolved {
        opportune::time::LocalResolution::Unique(instant) => instant,
        opportune::time::LocalResolution::Skipped
        | opportune::time::LocalResolution::Repeated(_, _) => panic!("the fixture midnight exists"),
    }
}
