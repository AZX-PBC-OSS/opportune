//! Persistence round-trip proof for JSON document stores (document databases,
//! object storage, JSON columns, etc.): rules, goals and working calendars
//! serialize to canonical wire JSON, survive versioned `upgrade_*` calls, parse
//! back into strict domain types, and produce byte-identical engine answers.

// The byte-identical-answer half needs an engine, so this target runs where the
// crate compiles in a time zone database; the stripped build's parse, write and
// upgrade paths are covered by the other suites.
#![cfg(feature = "bundled-tzdb")]

use std::collections::BTreeMap;

use opportune::Engine;
use opportune::model::{
    CalendarSpec, Goal, Part, Participant, Request, Requirement, Rule, Span, Term,
};
use opportune::presets;
use opportune::tags::{Selector, Tag};
use opportune::time::{
    Date, Duration, Instant, LocalResolution, RuleZone, TimeOfDay, WallEnd, WallRange, Weekday,
    Weekdays, WeeklyKey, WindowSpec, WorkingDays, Zone, ZoneName,
};
use opportune_json::{
    JsonError, WireCalendarSpec, WireGoal, WireRequest, WireRule, find_availability,
    parse_calendar, parse_goal, parse_rule, to_json, upgrade_calendar, upgrade_goal, upgrade_rule,
};

fn utc(year: i16, month: i8, day: i8, hour: u8, minute: u8) -> Instant {
    let date = Date::new(year, month, day).expect("date");
    let time = TimeOfDay::new(hour, minute).expect("time");
    match Zone::utc().resolve_local(date, time).expect("UTC") {
        LocalResolution::Unique(instant) => instant,
        LocalResolution::Skipped | LocalResolution::Repeated(_, _) => unreachable!(),
    }
}

/// Asserts that a stored document's current-version form is canonical and a fixed
/// point: the bytes a first upgrade writes are the bytes an upgrade of them writes.
type Upgrade = fn(&str, u32) -> Result<String, JsonError>;

fn assert_canonical(stored: &str, upgraded: &str, upgrade: Upgrade, what: &str) {
    assert_eq!(
        upgraded, stored,
        "{what}: the freshly written document is canonical"
    );
    assert_eq!(
        upgrade(upgraded, 1).expect("the canonical document upgrades"),
        upgraded,
        "{what}: upgrading the canonical document rewrites nothing"
    );
}

#[test]
fn stored_rules_goals_and_calendars_round_trip_byte_identically() {
    let ny = ZoneName::new("America/New_York").expect("zone");
    let engine = Engine::builder().build().expect("bundled tzdb");

    let split_window = WindowSpec::new(
        RuleZone::Participant,
        vec![
            WeeklyKey::new(
                Weekdays::range(Weekday::Monday, Weekday::Friday),
                vec![
                    WallRange::new(
                        TimeOfDay::new(9, 0).expect("t"),
                        WallEnd::Time(TimeOfDay::new(12, 0).expect("t")),
                    )
                    .expect("r1"),
                    WallRange::new(
                        TimeOfDay::new(13, 0).expect("t"),
                        WallEnd::Time(TimeOfDay::new(17, 0).expect("t")),
                    )
                    .expect("r2"),
                ],
            )
            .expect("wk"),
        ],
        Vec::new(),
    )
    .expect("window");

    let original_rules: Vec<Rule> = vec![
        Rule::new(
            "business_hours",
            opportune::model::RuleKind::Window(split_window),
        ),
        presets::buffers(Duration::from_minutes(10), Duration::from_minutes(15)),
        presets::min_notice(Duration::from_minutes(120)),
        presets::cap_per_day(Selector::tag(Tag::new("interview").expect("tag")), 2),
    ];

    let mut part = Part::new(
        Duration::from_minutes(45),
        Requirement::new(vec![Term::person("ana")]),
    );
    part.id = "interview".to_owned();
    part.tags = vec![Tag::new("interview").expect("tag")];

    let original_goal = Goal::new(
        ny.clone(),
        Span::new(utc(2027, 3, 8, 0, 0), utc(2027, 3, 10, 0, 0)).expect("span"),
        vec![part],
    );

    let original_calendar = CalendarSpec {
        zone: ny.clone(),
        working_days: WorkingDays::weekdays(),
        holidays: vec![Date::new(2027, 3, 9).expect("holiday")],
    };

    // Persist each piece to canonical JSON, upgrade on read, and hydrate back
    // into strict domain values. The upgraded document is the canonical one and
    // upgrading it again rewrites nothing, so a stored row's bytes are stable
    // across re-reads: a restart, a retry or a replica cannot make it drift.
    let reloaded_rules: Vec<Rule> = original_rules
        .iter()
        .map(|rule| {
            let stored = to_json(&WireRule::from(rule)).expect("serialize rule");
            let upgraded = upgrade_rule(&stored, 1).expect("upgrade rule");
            assert_canonical(&stored, &upgraded, upgrade_rule, "rule");
            parse_rule(&upgraded).expect("parse rule")
        })
        .collect();
    let reloaded_goal = {
        let stored = to_json(&WireGoal::from(&original_goal)).expect("serialize goal");
        let upgraded = upgrade_goal(&stored, 1).expect("upgrade goal");
        assert_canonical(&stored, &upgraded, upgrade_goal, "goal");
        parse_goal(&upgraded).expect("parse goal")
    };
    let reloaded_calendar = {
        let stored =
            to_json(&WireCalendarSpec::from(&original_calendar)).expect("serialize calendar");
        let upgraded = upgrade_calendar(&stored, 1).expect("upgrade calendar");
        assert_canonical(&stored, &upgraded, upgrade_calendar, "calendar");
        parse_calendar(&upgraded).expect("parse calendar")
    };

    assert_eq!(reloaded_rules, original_rules);
    assert_eq!(reloaded_goal, original_goal);
    assert_eq!(reloaded_calendar, original_calendar);

    let mut ana = Participant::new("ana", ny);
    ana.calendar = Some(Vec::new());

    let mut before = Request::new(utc(2027, 3, 6, 12, 0), original_goal);
    before.participants = vec![ana.clone()];
    before.rules = original_rules;
    before.calendars = BTreeMap::from([("us-ny".to_owned(), original_calendar)]);

    let mut after = Request::new(utc(2027, 3, 6, 12, 0), reloaded_goal);
    after.participants = vec![ana];
    after.rules = reloaded_rules;
    after.calendars = BTreeMap::from([("us-ny".to_owned(), reloaded_calendar)]);

    let before_json = to_json(&WireRequest::from(&before)).expect("before json");
    let after_json = to_json(&WireRequest::from(&after)).expect("after json");
    assert_eq!(
        find_availability(&engine, &before_json).expect("before answer"),
        find_availability(&engine, &after_json).expect("after answer"),
    );
}
