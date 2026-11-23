//! The `limits` and `event_key` wire contracts: a limits document may name only
//! the caps it changes, and every cap it leaves out — or writes as `null` — is the
//! engine's own default; an event key that names no event is rejected where it was
//! written.

use opportune::Limits;
use opportune_json::{WireLimits, parse_limits, parse_request, to_json};

const FIVE_LIMITS: &str = r#"{
  "max_participants": 500,
  "max_blocks_per_participant": 10000,
  "max_search_window_days": 92,
  "max_candidate_starts": 100000,
  "max_explain_limit": 10000
}"#;

#[test]
fn a_five_field_limits_document_parses_with_the_engine_defaults() {
    let limits = parse_limits(FIVE_LIMITS).expect("a five-field document parses");
    assert_eq!(limits, Limits::default());
    assert_eq!(limits.max_rules, 500);
    assert_eq!(limits.max_occurrences, 50_000);
    assert_eq!(limits.max_requirement_slots, 64);
    assert_eq!(limits.max_duration_minutes, 7 * 24 * 60);
    assert_eq!(limits.max_zone_periods_per_participant, 1_000);
}

/// A value each of the ten caps is distinct from the engine's default, so a field
/// the wire drops or reads from the wrong place changes the answer.
const DISTINCT: [u64; 10] = [11, 22, 33, 44, 55, 66, 77, 88, 99, 111];

/// The distinct value of each cap, in the order the fields are declared.
fn distinct_limits() -> Limits {
    let [
        participants,
        blocks,
        days,
        starts,
        explain,
        rules,
        occurrences,
        slots,
        duration,
        periods,
    ] = DISTINCT;
    let count = |value: u64| usize::try_from(value).expect("a small count fits a usize");
    Limits::default()
        .with_max_participants(count(participants))
        .with_max_blocks_per_participant(count(blocks))
        .with_max_search_window_days(u32::try_from(days).expect("a small day count"))
        .with_max_candidate_starts(count(starts))
        .with_max_explain_limit(count(explain))
        .with_max_rules(count(rules))
        .with_max_occurrences(count(occurrences))
        .with_max_requirement_slots(count(slots))
        .with_max_duration_minutes(u32::try_from(duration).expect("a small minute count"))
        .with_max_zone_periods_per_participant(count(periods))
}

/// Every one of the ten caps survives a write and a read, each pinned to its own
/// field: a wire field wired to the wrong domain field cannot pass, whatever the
/// values are.
#[test]
fn every_limit_field_round_trips_to_its_own_value() {
    let limits = distinct_limits();
    let written = to_json(&WireLimits::from(&limits)).expect("the limits write");
    let read = parse_limits(&written).expect("the limits read");
    assert_eq!(read, limits);

    let value: serde_json::Value = serde_json::from_str(&written).expect("a limits document");
    let [
        participants,
        blocks,
        days,
        starts,
        explain,
        rules,
        occurrences,
        slots,
        duration,
        periods,
    ] = DISTINCT;
    for (field, expected) in [
        ("max_participants", participants),
        ("max_blocks_per_participant", blocks),
        ("max_candidate_starts", starts),
        ("max_explain_limit", explain),
        ("max_rules", rules),
        ("max_occurrences", occurrences),
        ("max_requirement_slots", slots),
        ("max_zone_periods_per_participant", periods),
    ] {
        assert_eq!(
            value[field], expected,
            "{field} is written where it belongs"
        );
    }
    assert_eq!(value["max_search_window_days"], days);
    assert_eq!(value["max_duration_minutes"], duration);

    // The domain's `u32` caps come back as `u32`, not as a saturated `usize`.
    let _: u32 = read.max_search_window_days;
    let _: u32 = read.max_duration_minutes;
}

/// An empty limits document is the engine's default configuration, and a
/// document may name only the caps it changes — every field is optional.
#[test]
fn an_absent_or_partial_limits_document_takes_the_engine_defaults() {
    assert_eq!(
        parse_limits("{}").expect("an empty document is the default"),
        Limits::default(),
    );
    let partial = parse_limits(r#"{ "max_search_window_days": 30 }"#)
        .expect("a partial document takes the defaults for the rest");
    assert_eq!(partial.max_search_window_days, 30);
    assert_eq!(partial.max_participants, Limits::default().max_participants);
    assert_eq!(
        partial.max_explain_limit,
        Limits::default().max_explain_limit
    );
    assert_eq!(partial.max_rules, Limits::default().max_rules);
    assert_eq!(
        parse_limits(r#"{ "max_participants": 7 }"#)
            .expect("one cap is enough")
            .max_participants,
        7,
    );
}

#[test]
fn a_nine_field_limits_document_takes_the_zone_period_default() {
    // The nine fields a document written before the zone-period cap existed
    // carries: the new limit is absent, not null, and reads as the engine default.
    let document = r#"{
      "max_participants": 500,
      "max_blocks_per_participant": 10000,
      "max_search_window_days": 92,
      "max_candidate_starts": 100000,
      "max_explain_limit": 10000,
      "max_rules": 500,
      "max_occurrences": 50000,
      "max_requirement_slots": 64,
      "max_duration_minutes": 10080
    }"#;
    let limits = parse_limits(document).expect("an existing nine-field document still parses");
    assert_eq!(limits.max_zone_periods_per_participant, 1_000);
    assert_eq!(limits, Limits::default());
}

#[test]
fn an_explicit_null_for_an_added_limit_represents_the_engine_default() {
    // A binding that writes every key, or a document read back from a JSONB column,
    // puts an explicit `null` where a human would leave the field out; every limit
    // reads it as the default, as every other defaulted wire field does.
    let document = r#"{
      "max_participants": null,
      "max_blocks_per_participant": null,
      "max_search_window_days": null,
      "max_candidate_starts": null,
      "max_explain_limit": null,
      "max_rules": null,
      "max_occurrences": null,
      "max_requirement_slots": null,
      "max_duration_minutes": null,
      "max_zone_periods_per_participant": null
    }"#;
    assert_eq!(
        parse_limits(document).expect("an explicit null takes the default"),
        Limits::default()
    );
}

#[test]
fn a_limits_field_of_the_wrong_width_names_the_field_at_fault() {
    let error = parse_limits(r#"{ "max_search_window_days": 4294967296 }"#)
        .expect_err("a day count is a u32 in the domain");
    assert_eq!(error.path(), "/max_search_window_days");
    assert!(
        matches!(error.code(), "invalid_type" | "invalid_value"),
        "a value the field cannot hold is a shape failure, not a domain one: {error}",
    );

    let error =
        parse_limits(r#"{ "max_participants": -1 }"#).expect_err("a count cannot be negative");
    assert_eq!(error.path(), "/max_participants");
    assert!(
        matches!(error.code(), "invalid_type" | "invalid_value"),
        "{error}",
    );
}

#[test]
fn the_added_limits_are_read_from_the_document() {
    let document = r#"{
      "max_participants": 500,
      "max_blocks_per_participant": 10000,
      "max_search_window_days": 92,
      "max_candidate_starts": 100000,
      "max_explain_limit": 10000,
      "max_rules": 5,
      "max_occurrences": 6,
      "max_requirement_slots": 7,
      "max_duration_minutes": 8,
      "max_zone_periods_per_participant": 9
    }"#;
    let limits = parse_limits(document).expect("the document is a limits document");
    assert_eq!(limits.max_rules, 5);
    assert_eq!(limits.max_occurrences, 6);
    assert_eq!(limits.max_requirement_slots, 7);
    assert_eq!(limits.max_duration_minutes, 8);
    assert_eq!(limits.max_zone_periods_per_participant, 9);
}

#[test]
fn the_written_limits_document_reads_back_as_the_same_limits() {
    let limits = Limits::default()
        .with_max_rules(12)
        .with_max_occurrences(34)
        .with_max_requirement_slots(56)
        .with_max_duration_minutes(78)
        .with_max_zone_periods_per_participant(90);
    let written = to_json(&WireLimits::from(&limits)).expect("the limits write");
    assert_eq!(parse_limits(&written).expect("the limits read"), limits);
    // The document states every limit rather than leaning on the defaults, so a
    // caller can read the engine's own caps off one written document.
    for field in [
        "max_rules",
        "max_occurrences",
        "max_requirement_slots",
        "max_duration_minutes",
        "max_zone_periods_per_participant",
    ] {
        assert!(written.contains(field), "{field} is written: {written}");
    }
}

/// A request document with a minimal goal, ready for the test to add what it
/// breaks.
fn request_document(extra: &str) -> String {
    format!(
        r#"{{
          "now": "2027-06-14T08:00Z",
          "goal": {{
            "parts": [{{ "duration": "30m",
                          "requirement": {{ "all": [{{ "person": "ana" }}] }} }}],
            "search_window": ["2027-06-14T08:00Z", "2027-06-14T10:00Z"],
            "zone": "UTC"
          }},
          "participants": [{{ "id": "ana", "zone": "UTC", "calendar": [] }}]{extra}
        }}"#
    )
}

#[test]
fn an_empty_occurrence_event_key_is_rejected_where_it_was_written() {
    for key in ["", "   "] {
        let document = request_document(&format!(
            r#", "occurrences": [{{ "event_key": "{key}",
                                     "span": ["2027-06-14T08:00Z", "2027-06-14T09:00Z"],
                                     "participants": ["ana"] }}]"#
        ));
        let error = parse_request(&document).expect_err("an empty key names no event");
        assert_eq!(error.code(), "event_key_empty", "{key:?}");
        assert_eq!(error.path(), "/occurrences/0/event_key", "{key:?}");
    }
}

#[test]
fn an_empty_block_event_key_is_rejected_where_it_was_written() {
    for key in ["", "  "] {
        let document = format!(
            r#"{{
              "now": "2027-06-14T08:00Z",
              "goal": {{
                "parts": [{{ "duration": "30m",
                              "requirement": {{ "all": [{{ "person": "ana" }}] }} }}],
                "search_window": ["2027-06-14T08:00Z", "2027-06-14T10:00Z"],
                "zone": "UTC"
              }},
              "participants": [{{ "id": "ana", "zone": "UTC",
                                  "calendar": [{{ "span": ["2027-06-14T08:00Z", "2027-06-14T09:00Z"],
                                                  "event_key": "{key}" }}] }}]
            }}"#
        );
        let error = parse_request(&document).expect_err("an empty key names no event");
        assert_eq!(error.code(), "event_key_empty", "{key:?}");
        assert_eq!(
            error.path(),
            "/participants/0/calendar/0/event_key",
            "{key:?}"
        );
    }
}
