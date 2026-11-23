//! Versioned upgrades: what version 1 rewrites a stored document into, and how an
//! unknown version or a malformed document is reported.

use opportune_json::{upgrade_calendar, upgrade_goal, upgrade_rule};

/// A rule written the way a caller would have stored it.
const RULE: &str = r#"{ "id": "r_min", "kind": "lead_time", "params": { "min": "4h" } }"#;

/// A working calendar written the way a caller would have stored it.
const CALENDAR: &str = r#"{ "zone": "Europe/London", "working_days": "mon-fri" }"#;

/// A goal written the way a caller would have stored it.
const GOAL: &str = r#"{
  "parts": [{ "duration": "45m", "requirement": { "all": [{ "person": "ana" }] } }],
  "search_window": ["2027-03-09T15:00Z", "2027-03-10T15:00Z"],
  "zone": "UTC"
}"#;

#[test]
fn a_stored_rule_upgrades_to_its_canonical_version_one_json() {
    let canonical = upgrade_rule(RULE, 1).expect("a version 1 rule upgrades");
    assert_eq!(
        canonical,
        concat!(
            r#"{"kind":"lead_time","id":"r_min","who":{"role":"host"},"when":true,"#,
            r#""params":{"min":"4h"}}"#,
        ),
        "the upgrade resolves the defaults the stored document left to the wire",
    );
    assert_eq!(
        upgrade_rule(&canonical, 1).unwrap(),
        canonical,
        "the canonical form is a fixed point",
    );
}

#[test]
fn a_stored_calendar_upgrades_to_its_canonical_version_one_json() {
    let canonical = upgrade_calendar(CALENDAR, 1).expect("a version 1 calendar upgrades");
    assert_eq!(
        canonical, r#"{"zone":"Europe/London","working_days":"mon,tue,wed,thu,fri","holidays":[]}"#,
        "a day range is written as the list of days it means, and the holidays are stated",
    );
    assert_eq!(upgrade_calendar(&canonical, 1).unwrap(), canonical);
}

#[test]
fn a_stored_goal_upgrades_to_its_canonical_version_one_json() {
    let canonical = upgrade_goal(GOAL, 1).expect("a version 1 goal upgrades");
    assert_eq!(
        canonical,
        concat!(
            r#"{"parts":[{"id":"main","tags":[],"duration":"45m","#,
            r#""requirement":{"all":[{"person":"ana"}]},"#,
            r#""travel":{"before":"0h","after":"0h","by_participant":{}}}],"#,
            r#""search_window":["2027-03-09T15:00Z","2027-03-10T15:00Z"],"zone":"UTC","#,
            r#""display_zone":"UTC"}"#,
        ),
        "the upgrade resolves every default the stored document left to the wire",
    );
    assert_eq!(upgrade_goal(&canonical, 1).unwrap(), canonical);
}

#[test]
fn an_unsupported_version_is_rejected_before_the_document_is_read() {
    for version in [0, 2, 99] {
        for upgraded in [
            upgrade_rule(RULE, version),
            upgrade_calendar(CALENDAR, version),
            upgrade_goal(GOAL, version),
        ] {
            let error = upgraded.expect_err("a version this build does not read is rejected");
            assert_eq!(error.code(), "unsupported_schema_version");
            assert_eq!(
                error.path(),
                "",
                "no field is at fault: the document as a whole was written in another version",
            );
            assert!(
                error.message().contains(&version.to_string()),
                "the message names the version that was written: {}",
                error.message(),
            );
        }
    }

    // A document from another version is not necessarily a malformed document of
    // this one, so nothing about it is read: even a body that is not a rule is
    // reported as the wrong version rather than as a broken rule.
    let error = upgrade_rule("not a rule", 2).expect_err("the version is checked first");
    assert_eq!(error.code(), "unsupported_schema_version");
}

#[test]
fn a_malformed_version_one_rule_names_the_field_at_fault() {
    let error = upgrade_rule(
        r#"{ "id": "r_min", "kind": "lead_time", "params": { "min": "1d" } }"#,
        1,
    )
    .expect_err("a duration cannot name days");
    assert_eq!(error.code(), "duration_contains_days");
    assert_eq!(error.path(), "/params/min");
}

#[test]
fn a_malformed_version_one_calendar_names_the_date_at_fault() {
    let error = upgrade_calendar(
        r#"{ "zone": "UTC", "holidays": ["2027-01-01", "2027-02-29"] }"#,
        1,
    )
    .expect_err("2027 is not a leap year");
    assert_eq!(error.code(), "date_invalid");
    assert_eq!(error.path(), "/holidays/1");
}

#[test]
fn a_malformed_version_one_goal_names_the_start_at_fault() {
    let error = upgrade_goal(
        r#"{
          "parts": [{ "duration": "45m", "requirement": { "all": [{ "person": "ana" }] } }],
          "candidates": ["2027-03-09T15:00:30Z"],
          "search_window": ["2027-03-09T15:00Z", "2027-03-10T15:00Z"],
          "zone": "UTC"
        }"#,
        1,
    )
    .expect_err("a candidate start must be minute-aligned");
    assert_eq!(error.code(), "instant_not_aligned");
    assert_eq!(error.path(), "/candidates/0");
}

/// The canonical form every stored rule kind upgrades to is a fixed point, not
/// only the `lead_time` one: running the migration again rewrites nothing.
#[test]
fn every_stored_rule_kind_upgrades_to_a_fixed_point() {
    let stored = [
        r#"{ "id": "r_window", "kind": "window",
             "params": { "zone": "event", "weekly": { "mon-fri": ["09:00-17:00"] } } }"#,
        r#"{ "id": "r_blackout", "kind": "blackout",
             "params": { "zone": "event",
                         "spans": [["2027-01-01T00:00", "2027-01-02T00:00"]] } }"#,
        r#"{ "id": "r_clearance", "kind": "clearance",
             "params": { "before": "10m", "after": "10m", "min_gap": "5m" } }"#,
        RULE,
        r#"{ "id": "r_status", "kind": "status_policy",
             "params": { "entries": [{ "status": "busy", "effect": "ignore" }] } }"#,
        r#"{ "id": "r_rest", "kind": "rest",
             "params": { "late_after": "18:00", "not_before": "10:00" } }"#,
        r#"{ "id": "r_cap", "kind": "cap",
             "params": { "count": { "tag": "interview" }, "measure": "count", "limit": 2,
                         "period": "day", "per": "each", "zone": "UTC" } }"#,
    ];
    for rule in stored {
        let canonical = upgrade_rule(rule, 1).expect("a version 1 rule upgrades");
        assert_eq!(
            upgrade_rule(&canonical, 1).expect("the canonical form re-reads"),
            canonical,
            "the canonical form of {rule} is not a fixed point",
        );
    }
}

/// A calendar that works no weekday has no day-list spelling, and a stored goal
/// may carry candidates instead of a grid: both still upgrade to a fixed point.
#[test]
fn the_stored_documents_with_no_spelling_of_their_own_are_fixed_points() {
    for stored in [
        r#"{ "zone": "UTC" }"#,
        r#"{ "zone": "UTC", "working_days": "sun-sat", "holidays": ["2027-01-01"] }"#,
    ] {
        let canonical = upgrade_calendar(stored, 1).expect("a version 1 calendar upgrades");
        assert_eq!(
            upgrade_calendar(&canonical, 1).expect("the canonical form re-reads"),
            canonical,
            "the canonical form of {stored} is not a fixed point",
        );
    }

    let goal = r#"{
      "parts": [{ "duration": "45m", "requirement": { "all": [{ "person": "ana" }] } }],
      "candidates": ["2027-03-09T15:00Z"],
      "search_window": ["2027-03-09T15:00Z", "2027-03-10T15:00Z"],
      "zone": "UTC"
    }"#;
    let canonical = upgrade_goal(goal, 1).expect("a version 1 goal upgrades");
    assert_eq!(
        upgrade_goal(&canonical, 1).expect("the canonical form re-reads"),
        canonical,
        "a goal stored with candidates rather than a grid is not a fixed point",
    );
}

/// Two stored documents that mean the same working week canonicalise to the same
/// bytes, which is what makes the upgrade usable as a database comparison.
#[test]
fn two_spellings_of_one_working_week_upgrade_identically() {
    assert_eq!(
        upgrade_calendar(r#"{ "zone": "UTC", "working_days": "mon-fri" }"#, 1)
            .expect("a version 1 calendar upgrades"),
        upgrade_calendar(
            r#"{ "zone": "UTC", "working_days": "mon,tue,wed,thu,fri" }"#,
            1,
        )
        .expect("a version 1 calendar upgrades"),
    );
}

/// The version a stored row carries comes from this crate, so a persistence layer
/// that depends only on `opportune-json` does not hardcode the number in its
/// migration and its `upgrade_*` call.
#[test]
fn the_schema_version_is_part_of_the_wire_layer() {
    let version: u32 = opportune_json::SCHEMA_VERSION;
    assert_eq!(version, 1, "version 1 is what this build reads and writes");
    assert!(
        upgrade_rule(RULE, version).is_ok(),
        "the version the crate publishes is the one its upgrades accept",
    );
}
