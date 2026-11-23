//! The binding's behaviour on the host target: every entry point is driven
//! through the crate's own surface, and every failure is read as the code, the
//! pointer and the findings the caller receives.
//!
//! The exports themselves are called where they can be: their success paths need
//! no JavaScript. Their failure path throws a `JsValue`, which a host target
//! cannot construct (wasm-bindgen's own cast panics outside a wasm runtime), so a
//! failure is driven through the plain entry point the export delegates to —
//! [`opportune_wasm::engine::Engine`] or [`opportune_wasm::adapters`] — and read
//! through [`opportune_wasm::error::payload`], the one function the export wraps
//! it with.

use opportune_wasm::engine::Engine;
use opportune_wasm::error::payload;
use opportune_wasm::{WasmEngine, adapters};
use serde_json::Value;

/// A request: one 30-minute booking for one participant over three hours of an
/// empty calendar, so the only limits are the ones the engine applies.
const REQUEST: &str = r#"{
  "now": "2027-03-09T15:00Z",
  "goal": {
    "parts": [{ "duration": "30m", "requirement": { "all": [{ "person": "ana" }] } }],
    "search_window": ["2027-03-09T15:00Z", "2027-03-09T18:00Z"],
    "zone": "UTC"
  },
  "participants": [{ "id": "ana", "zone": "UTC", "calendar": [] }]
}"#;

/// The booking [`REQUEST`] offers, at the first candidate start.
const PROPOSAL: &str = r#"{
  "events": [{ "part": "main", "start": "2027-03-09T15:00Z", "assignment": [["ana"]] }]
}"#;

/// A `calendarView` response with one timed event in a category.
const GRAPH_CALENDAR_VIEW: &str = r#"{ "value": [{
  "id": "AAMkAD",
  "isAllDay": false,
  "showAs": "busy",
  "categories": ["Interview"],
  "start": { "dateTime": "2027-03-09T09:00:00.0000000", "timeZone": "UTC" },
  "end": { "dateTime": "2027-03-09T10:00:00.0000000", "timeZone": "UTC" }
}] }"#;

/// A `getSchedule` response with one healthy mailbox.
const GRAPH_GET_SCHEDULE: &str = r#"{ "value": [{
  "scheduleId": "ana@example.com",
  "scheduleItems": [{
    "status": "busy",
    "start": { "dateTime": "2027-03-09T09:00:00.0000000", "timeZone": "UTC" },
    "end": { "dateTime": "2027-03-09T10:00:00.0000000", "timeZone": "UTC" }
  }]
}] }"#;

/// A `workingHours` object in an IANA zone name.
const GRAPH_WORKING_HOURS: &str = r#"{
  "daysOfWeek": ["monday", "tuesday", "wednesday", "thursday", "friday"],
  "startTime": "09:00:00",
  "endTime": "17:00:00",
  "timeZone": { "name": "Europe/London" }
}"#;

/// A `workingHours` object naming its zone the way Outlook on Windows does.
const GRAPH_WINDOWS_WORKING_HOURS: &str = r#"{
  "daysOfWeek": ["monday"],
  "startTime": "09:00:00",
  "endTime": "17:00:00",
  "timeZone": { "name": "GMT Standard Time" }
}"#;

/// An `events.list` response with one timed event.
const GOOGLE_EVENTS_LIST: &str = r#"{ "timeZone": "UTC", "items": [{
  "id": "evt-1",
  "status": "confirmed",
  "start": { "dateTime": "2027-03-09T09:00:00Z" },
  "end": { "dateTime": "2027-03-09T10:00:00Z" }
}] }"#;

/// A `freeBusy.query` response with one busy calendar.
const GOOGLE_FREE_BUSY: &str = r#"{ "calendars": { "ana@example.com": {
  "busy": [{ "start": "2027-03-09T09:00:00Z", "end": "2027-03-09T10:00:00Z" }]
} } }"#;

/// A Graph rule a caller stored at schema version 1.
const STORED_RULE: &str = r#"{ "id": "r_min", "kind": "lead_time", "params": { "min": "4h" } }"#;

/// `document` parsed as JSON.
fn json(document: &str) -> Value {
    serde_json::from_str(document).unwrap_or_else(|err| panic!("{document} is not JSON: {err}"))
}

/// The payload of `error` parsed as JSON, as a caller reads it.
fn report(error: &opportune_json::JsonError) -> Value {
    json(&payload(error))
}

/// The findings a payload carries, as a caller reads them.
fn findings_of(report: &Value) -> Vec<Value> {
    report["errors"]
        .as_array()
        .cloned()
        .unwrap_or_else(|| panic!("the findings are a list: {report}"))
}

/// An engine over the bundled database and the default limits.
fn engine() -> Engine {
    Engine::new(None).expect("the bundled database builds an engine")
}

#[test]
fn the_engine_answers_every_query_over_the_wasm_surface() {
    let limits = opportune_wasm::default_limits_json().expect("the limits document writes");
    let engine =
        WasmEngine::new(Some(limits)).expect("the default limits document builds an engine");
    let version = engine.tzdb_version();
    assert!(!version.is_empty(), "the bundled database has a version");

    let offers = json(
        &engine
            .find_availability_json(REQUEST)
            .expect("the request evaluates"),
    );
    assert_eq!(
        offers["offers"][0]["events"][0]["start"], "2027-03-09T15:00Z",
        "the first candidate start is offered in an empty calendar: {offers}"
    );
    assert_eq!(
        offers["meta"]["tzdb"], version,
        "the answer reports the version the export reports"
    );

    let checked = json(
        &engine
            .check_slot_json(REQUEST, PROPOSAL)
            .expect("the proposal is checked"),
    );
    assert_eq!(checked["valid"], true, "{checked}");

    let diagnostics = json(
        &engine
            .validate_json(REQUEST)
            .expect("the request validates"),
    );
    assert_eq!(diagnostics["errors"], json("[]"), "{diagnostics}");

    let ranges = json(
        &engine
            .required_ranges_json(REQUEST)
            .expect("the ranges are reported"),
    );
    assert_eq!(
        ranges["calendar"],
        json(r#"["2027-03-08T00:00Z", "2027-03-11T00:00Z"]"#),
        "the day before the search window to the day after it, in UTC: {ranges}"
    );
    assert_eq!(
        ranges["occurrences"],
        Value::Null,
        "no cap applies, so no occurrence data is needed: {ranges}"
    );
}

#[test]
fn the_version_export_names_the_core_crate_this_module_was_built_from() {
    assert_eq!(opportune_wasm::version(), opportune::VERSION);
}

#[test]
fn the_default_limits_document_is_the_one_the_engine_reads() {
    let document = opportune_wasm::default_limits_json().expect("the limits document writes");
    let limits = json(&document);
    assert_eq!(limits["max_participants"], 500);
    assert_eq!(limits["max_blocks_per_participant"], 10_000);
    assert_eq!(limits["max_search_window_days"], 92);
    assert_eq!(limits["max_candidate_starts"], 100_000);
    assert_eq!(limits["max_explain_limit"], 10_000);
    assert_eq!(limits["max_rules"], 500);
    assert_eq!(limits["max_occurrences"], 50_000);
    assert_eq!(limits["max_requirement_slots"], 64);
    assert_eq!(limits["max_duration_minutes"], 10_080);
    assert_eq!(limits["max_zone_periods_per_participant"], 1_000);

    Engine::new(Some(&document)).expect("the document it writes is the document it reads");
}

#[test]
fn a_limits_document_that_is_not_one_is_the_constructions_failure() {
    // Partial limits documents default every omitted field.
    Engine::new(Some(r#"{ "max_participants": 500 }"#))
        .expect("every limits field defaults when omitted");

    let Err(error) = Engine::new(Some(r#"{ "unknown_cap": 1 }"#)) else {
        panic!("an unknown limits key must fail construction");
    };
    let unknown_report = report(&error);
    assert_eq!(unknown_report["code"], "unknown_field", "{unknown_report}");

    let Err(error) = Engine::new(Some(r#"{ "max_participants": "many" }"#)) else {
        panic!("a non-integer limits value must fail construction");
    };
    let type_report = report(&error);
    assert_eq!(type_report["code"], "invalid_type", "{type_report}");
}

#[test]
fn an_engine_built_with_limits_enforces_them() {
    let limits = r#"{
      "max_participants": 500,
      "max_blocks_per_participant": 10000,
      "max_search_window_days": 92,
      "max_candidate_starts": 2,
      "max_explain_limit": 10000
    }"#;
    let engine = Engine::new(Some(limits)).expect("the limits parse");

    let error = engine
        .find_availability_json(REQUEST)
        .expect_err("the candidate count is above the configured maximum");
    let report = report(&error);
    assert_eq!(report["code"], "invalid_request");
    let findings = findings_of(&report);
    let finding = findings
        .iter()
        .find(|finding| finding["code"] == "candidates_limit")
        .unwrap_or_else(|| panic!("no `candidates_limit` finding: {report}"));
    assert_eq!(
        finding["path"], "/goal/grid",
        "the finding points at the field that produced the candidates: {report}"
    );
}

/// The constructor export reads the limits document it is handed, rather than
/// building an engine over the defaults and dropping the argument: a request the
/// default limits refuse is answered once the caller raises the limit in the
/// document `default_limits_json` writes.
///
/// This is the one place an export's own failure path is reached on the host, and
/// it is reached only when the test is red: a constructor that drops its document
/// refuses the request, and the refusal an export throws is a `JsValue`, so the
/// failure reads as wasm-bindgen's `function not implemented on non-wasm32
/// targets` panic rather than as an assertion.
#[test]
fn the_constructor_export_reads_the_limits_document_it_is_given() {
    let explaining = REQUEST.replace(r#""now""#, r#""explain": { "limit": 20000 }, "now""#);

    let refused = engine()
        .find_availability_json(&explaining)
        .expect_err("the default `max_explain_limit` is 10000, so 20000 is refused");
    assert_eq!(report(&refused)["code"], "invalid_request");

    let mut limits =
        json(&opportune_wasm::default_limits_json().expect("the limits document writes"));
    assert!(
        limits.get("max_explain_limit").is_some(),
        "the defaults document names the explain limit: {limits}"
    );
    limits["max_explain_limit"] = Value::from(20_000);
    let engine = WasmEngine::new(Some(limits.to_string()))
        .expect("the caller's own limits document builds an engine");

    let answered = json(
        &engine
            .find_availability_json(&explaining)
            .expect("the raised limit is the one the engine applies, so the request is answered"),
    );
    assert!(
        answered["offers"]
            .as_array()
            .is_some_and(|offers| !offers.is_empty()),
        "the same request the default limits refuse is offered: {answered}"
    );
}

/// A count the document carries is read as `u64`, so four billion offers or
/// explanations is the same document in a browser as on a 64-bit server.
///
/// A request limit past anything a search can produce is accepted and truncates
/// nothing; an `explain.limit` past the engine's configured maximum is the
/// configured-limits finding rather than a deserialisation failure.
#[test]
fn a_count_beyond_four_billion_reads_the_same_on_every_target() {
    let unbounded = REQUEST.replace(r#""now""#, r#""limit": 4294967296, "now""#);
    let offers = json(
        &engine()
            .find_availability_json(&unbounded)
            .expect("four billion offers is a count, not a shape failure"),
    );
    assert!(
        offers["offers"]
            .as_array()
            .is_some_and(|offers| !offers.is_empty()),
        "a limit no search can reach still offers every candidate: {offers}"
    );
    assert_eq!(
        offers["truncated"], false,
        "a limit no search can reach truncates nothing: {offers}"
    );

    let explaining = REQUEST.replace(r#""now""#, r#""explain": { "limit": 4294967296 }, "now""#);
    let error = engine()
        .find_availability_json(&explaining)
        .expect_err("the explanation limit is above the configured maximum");
    let report = report(&error);
    assert_eq!(
        report["code"], "invalid_request",
        "the same request fails the same way in a browser and on a server: {report}"
    );
    let findings = findings_of(&report);
    let finding = findings
        .iter()
        .find(|finding| finding["code"] == "explain_limit")
        .unwrap_or_else(|| panic!("no `explain_limit` finding: {report}"));
    assert_eq!(finding["path"], "/explain/limit");
}

#[test]
fn a_rejected_request_reports_its_findings_and_no_pointer_of_its_own() {
    let document = REQUEST.replace(r#""zone": "UTC""#, r#""zone": "Mars/Olympus_Mons""#);
    let error = engine()
        .find_availability_json(&document)
        .expect_err("the participant's zone does not exist");
    let report = report(&error);
    assert_eq!(report["code"], "invalid_request");
    assert_eq!(
        report["path"],
        Value::Null,
        "the pointers live on the findings, not on the failure: {report}"
    );
    let findings = findings_of(&report);
    assert!(
        findings
            .iter()
            .any(|finding| finding["path"] == "/participants/0/zone"),
        "a finding points at the participant's zone: {report}"
    );
}

#[test]
fn a_document_that_is_not_json_keeps_the_parsers_cause() {
    let error = engine()
        .find_availability_json("{ \"now\": ")
        .expect_err("the document is truncated");
    let report = report(&error);
    assert_eq!(report["code"], "invalid_json");
    assert_eq!(
        report["path"], "/now",
        "the pointer names the member serde was reading: {report}"
    );
    assert_eq!(report["errors"], json("[]"), "{report}");
    assert!(
        report["message"]
            .as_str()
            .is_some_and(|message| message.contains("EOF")),
        "the message keeps the parser's cause: {report}"
    );
}

#[test]
fn a_field_the_wire_shape_does_not_name_is_reported_at_its_object() {
    let document = REQUEST.replace(r#""id": "ana""#, r#""idd": "ana""#);
    let error = engine()
        .find_availability_json(&document)
        .expect_err("`idd` is not a participant field");
    let report = report(&error);
    assert_eq!(report["code"], "unknown_field");
    assert_eq!(report["path"], "/participants/0");
}

#[test]
fn a_malformed_proposal_names_the_pointer_into_it() {
    let proposal = r#"{
      "events": [{ "part": "main", "start": "2027-03-09T15:00Z", "assignment": [["ana"], ["ana"]] }]
    }"#;
    let error = engine()
        .check_slot_json(REQUEST, proposal)
        .expect_err("the goal has one term and the proposal lists two");
    let report = report(&error);
    assert_eq!(report["code"], "proposal_term_count");
    assert_eq!(
        report["path"], "/events/0/assignment",
        "the pointer names the field of the proposal that does not fit: {report}"
    );
}

#[test]
fn an_unreadable_query_is_reported_by_its_own_entry_point() {
    let error = engine()
        .required_ranges_json(r#"{ "now": "2027-03-09T15:00Z" }"#)
        .expect_err("the query carries neither a goal nor participants");
    let report = report(&error);
    assert_eq!(report["code"], "missing_field");
    assert_eq!(
        report["path"],
        Value::Null,
        "a missing top-level field is reported about the document: {report}"
    );
}

#[test]
fn every_adapter_converts_its_payload_over_the_wasm_surface() {
    let engine = WasmEngine::new(None).expect("the bundled database builds an engine");

    let calendar_view = json(
        &engine
            .graph_calendar_view_to_blocks_json(GRAPH_CALENDAR_VIEW, "America/New_York", None)
            .expect("the calendar view converts"),
    );
    assert_eq!(calendar_view["status"], "available", "{calendar_view}");
    assert_eq!(calendar_view["blocks"][0]["event_id"], "AAMkAD");
    assert_eq!(
        calendar_view["blocks"][0]["block"]["tags"][0], "category/interview",
        "an unmapped category becomes a slugged tag: {calendar_view}"
    );

    let schedule = json(
        &engine
            .graph_get_schedule_to_blocks_json(GRAPH_GET_SCHEDULE)
            .expect("the schedule converts"),
    );
    assert_eq!(schedule["ana@example.com"]["status"], "available");
    assert_eq!(
        schedule["ana@example.com"]["blocks"][0]["event_id"],
        Value::Null,
        "a free/busy source carries no event ID"
    );

    let working_hours = json(
        &engine
            .graph_working_hours_to_rule_json(GRAPH_WORKING_HOURS, "ana_hours")
            .expect("the working hours convert"),
    );
    assert_eq!(working_hours["status"], "available", "{working_hours}");
    assert_eq!(working_hours["rule"]["kind"], "window");
    assert_eq!(
        working_hours["rule"]["id"], "ana_hours",
        "the rule takes the ID the caller named"
    );

    let events = json(
        &engine
            .google_events_list_to_blocks_json(GOOGLE_EVENTS_LIST, "UTC")
            .expect("the events convert"),
    );
    assert_eq!(events["blocks"][0]["event_id"], "evt-1");

    let free_busy = json(
        &engine
            .google_free_busy_to_blocks_json(GOOGLE_FREE_BUSY)
            .expect("the free/busy ranges convert"),
    );
    assert_eq!(
        free_busy["ana@example.com"]["blocks"][0]["block"]["status"],
        "busy"
    );
}

#[test]
fn the_graph_category_map_is_the_argument_that_maps_a_category_to_a_tag() {
    let engine = WasmEngine::new(None).expect("the bundled database builds an engine");
    let outcome = json(
        &engine
            .graph_calendar_view_to_blocks_json(
                GRAPH_CALENDAR_VIEW,
                "UTC",
                Some(r#"{ "Interview": "hiring/interview" }"#.to_owned()),
            )
            .expect("the calendar view converts"),
    );
    assert_eq!(
        outcome["blocks"][0]["block"]["tags"][0], "hiring/interview",
        "the caller's mapping decides the tag: {outcome}"
    );
}

#[test]
fn a_category_map_that_is_not_a_tag_path_is_the_calls_failure() {
    let error = adapters::calendar_view_to_blocks(
        &engine(),
        GRAPH_CALENDAR_VIEW,
        "UTC",
        Some(r#"{ "Interview": "a tag with spaces" }"#),
    )
    .expect_err("a tag path carries no spaces");
    let report = report(&error);
    assert_eq!(report["code"], "tag_invalid_character", "{report}");
    assert_eq!(
        report["path"], "/Interview",
        "the pointer names the category the map got wrong: {report}"
    );
}

#[test]
fn a_blank_participant_zone_is_the_calls_failure() {
    let calendar_view =
        adapters::calendar_view_to_blocks(&engine(), GRAPH_CALENDAR_VIEW, "  ", None)
            .expect_err("a blank zone names no zone");
    let calendar_view = report(&calendar_view);
    assert_eq!(
        calendar_view["code"], "zone_name_invalid",
        "{calendar_view}"
    );
    assert_eq!(
        calendar_view["path"], "/participant_zone",
        "the pointer names the argument at fault: {calendar_view}"
    );

    let events = adapters::events_list_to_blocks(&engine(), GOOGLE_EVENTS_LIST, "")
        .expect_err("a blank zone names no zone");
    assert_eq!(report(&events)["code"], "zone_name_invalid");
}

#[test]
fn a_failure_about_the_calendar_is_an_outcome_rather_than_a_failure() {
    let outcome = json(
        &adapters::calendar_view_to_blocks(&engine(), "{ \"value\": 7 }", "UTC", None)
            .expect("an unreadable payload is about the calendar, not the call"),
    );
    assert_eq!(
        outcome["status"], "unavailable",
        "an errored mailbox must not read as a free one: {outcome}"
    );

    let unresolvable = GRAPH_WORKING_HOURS.replace("Europe/London", "Mars/Olympus_Mons");
    let working_hours = json(
        &adapters::working_hours_to_rule(&engine(), &unresolvable, "working_hours")
            .expect("a zone this build cannot resolve is a value, not a thrown failure"),
    );
    assert_eq!(
        working_hours["status"], "unavailable",
        "a caller falls back to its own window without catching anything: {working_hours}"
    );
    assert_eq!(
        working_hours["code"], "zone_unknown",
        "the outcome names what could not be converted: {working_hours}"
    );
}

#[test]
fn a_payload_of_the_wrong_envelope_shape_is_the_calls_failure() {
    let error = adapters::get_schedule_to_blocks(&engine(), "{ \"value\": 7 }")
        .expect_err("a `getSchedule` response carries schedules, not a number");
    let report = report(&error);
    assert_eq!(report["code"], "payload_invalid", "{report}");
}

/// The `windows-zones` feature is what decides whether a Windows zone name
/// resolves, and the compiled-out build fails closed rather than reading the
/// calendar as free: the value it answers is `unavailable` carrying the adapter's
/// own `zone_unknown` for a name nothing resolves.
#[test]
fn the_windows_zone_feature_decides_whether_a_windows_name_resolves() {
    let engine = WasmEngine::new(None).expect("the bundled database builds an engine");
    let working_hours = json(
        &engine
            .graph_working_hours_to_rule_json(GRAPH_WINDOWS_WORKING_HOURS, "working_hours")
            .expect("the working hours convert"),
    );

    #[cfg(feature = "windows-zones")]
    assert_eq!(
        working_hours["status"], "available",
        "a bundle built with the default feature resolves `GMT Standard Time`: {working_hours}"
    );

    #[cfg(not(feature = "windows-zones"))]
    {
        assert_eq!(
            working_hours["status"], "unavailable",
            "a bundle that cannot resolve the name does not read the calendar as free: \
             {working_hours}"
        );
        assert_eq!(
            working_hours["code"], "zone_unknown",
            "the outcome names why the mailbox could not be converted: {working_hours}"
        );
    }
}

#[test]
fn every_stored_document_upgrades_through_its_own_export() {
    let rule = opportune_wasm::upgrade_rule_json(STORED_RULE, 1).expect("version 1 is read");
    assert!(
        rule.contains(r#""id":"r_min""#),
        "the canonical form keeps the rule: {rule}"
    );

    let calendar = opportune_wasm::upgrade_calendar_json(
        r#"{ "zone": "Europe/London", "working_days": "mon-fri" }"#,
        1,
    )
    .expect("the calendar upgrades");
    assert!(
        calendar.contains(r#""working_days":"mon,tue,wed,thu,fri""#),
        "the day key is spelled out: {calendar}"
    );

    let goal = opportune_wasm::upgrade_goal_json(
        r#"{
          "parts": [{ "duration": "45m", "requirement": { "all": [{ "person": "ana" }] } }],
          "search_window": ["2027-03-09T15:00Z", "2027-03-10T15:00Z"],
          "zone": "UTC"
        }"#,
        1,
    )
    .expect("the goal upgrades");
    assert!(
        goal.contains(r#""display_zone":"UTC""#),
        "the display zone defaults to the event zone: {goal}"
    );
}

/// The version an upgrade export throws for is refused before the document is
/// read: [`opportune_wasm::upgrade_rule_json`] is this call with its failure
/// wrapped in the payload, so the code read here is the code a caller catches.
#[test]
fn an_unsupported_schema_version_is_reported_with_its_payload() {
    let error = opportune_json::upgrade_rule(STORED_RULE, 99)
        .expect_err("no build reads schema version 99");
    let report = report(&error);
    assert_eq!(report["code"], "unsupported_schema_version", "{report}");
    assert_eq!(report["path"], Value::Null);
}

/// `error::payload` is the one document every export throws, so its shape is
/// pinned here: all four fields are always present, and an absent pointer is
/// `null` rather than `""`.
#[test]
fn the_payload_always_carries_all_four_fields() {
    let error = opportune_json::JsonError::new("invalid_json", "expected value", "");
    assert_eq!(
        payload(&error),
        r#"{"code":"invalid_json","errors":[],"message":"expected value","path":null}"#,
    );

    let findings = vec![
        opportune::Diagnostic::new("zone_unknown", "unknown time zone `Mars/Olympus_Mons`")
            .with_path("/participants/0/zone"),
        opportune::Diagnostic::new("calendar_unknown", "no calendar for `bo`"),
    ];
    let rejected = opportune_json::JsonError::from(opportune::Error::invalid_request(findings));
    assert_eq!(
        payload(&rejected),
        r#"{"code":"invalid_request","errors":[{"code":"zone_unknown","message":"unknown time zone `Mars/Olympus_Mons`","path":"/participants/0/zone"},{"code":"calendar_unknown","message":"no calendar for `bo`","path":null}],"message":"the request is invalid: zone_unknown: unknown time zone `Mars/Olympus_Mons`","path":null}"#,
        "every field is present, and a finding without a pointer says so",
    );
}

#[test]
fn every_marker_export_carries_the_adapters_own_value() {
    assert_eq!(
        opportune_wasm::marker_tags(),
        opportune_adapters::MARKER_TAGS
    );
    assert_eq!(
        opportune_wasm::marker_event_key(),
        opportune_adapters::MARKER_EVENT_KEY
    );
    assert_eq!(
        opportune_wasm::graph_extended_property_namespace_guid(),
        opportune_adapters::graph::EXTENDED_PROPERTY_NAMESPACE_GUID
    );
    assert_eq!(
        opportune_wasm::graph_extended_property_tags_id(),
        opportune_adapters::graph::EXTENDED_PROPERTY_TAGS_ID
    );
    assert_eq!(
        opportune_wasm::graph_extended_property_event_key_id(),
        opportune_adapters::graph::EXTENDED_PROPERTY_EVENT_KEY_ID
    );
    assert_eq!(
        opportune_wasm::graph_extended_property_expand(),
        opportune_adapters::graph::EXTENDED_PROPERTY_EXPAND
    );
}
