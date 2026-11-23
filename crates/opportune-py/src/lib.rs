//! Python bindings for the [`opportune`] crate.
//!
//! The compiled extension module is `opportune._opportune`. The Python package in
//! `python/opportune/` re-exports its public surface and ships its type stub.
//!
//! The boundary is JSON in both directions: a Python caller hands in a document
//! and gets one back, so the format lives in one place — `opportune-json`'s wire
//! types — rather than in a hand-written conversion per language. Failures raise
//! `OpportuneError`, which carries a stable `code`, the RFC 6901 `path` of the
//! field at fault and the findings a rejected request produced; nothing panics
//! across the boundary.
//!
//! A call that does more than translate arguments releases the GIL around its
//! computation, so other Python threads keep running while the Rust side works.

mod engine;
mod error;

use pyo3::prelude::*;
use pyo3::types::PyString;

use engine::Engine;
use error::OpportuneError;
use error::detach;

/// Rewrites a stored rule as the current schema version's canonical JSON.
///
/// `rule_json` is the stored rule and `from_version` the schema version it was
/// written with.
///
/// Raises `OpportuneError` with code `unsupported_schema_version` when this build
/// does not read `from_version`, and the parser's own failure — a code and the
/// RFC 6901 pointer of the field at fault — when the document is not a valid rule
/// of that version.
#[pyfunction]
fn upgrade_rule_json(
    py: Python<'_>,
    rule_json: &Bound<'_, PyString>,
    from_version: u32,
) -> PyResult<String> {
    let rule_json = error::document(rule_json)?;
    detach(py, || opportune_json::upgrade_rule(rule_json, from_version))
}

/// Rewrites a stored working calendar as the current schema version's canonical
/// JSON.
///
/// Raises `OpportuneError` for the same reasons `upgrade_rule_json` raises.
#[pyfunction]
fn upgrade_calendar_json(
    py: Python<'_>,
    calendar_json: &Bound<'_, PyString>,
    from_version: u32,
) -> PyResult<String> {
    let calendar_json = error::document(calendar_json)?;
    detach(py, || {
        opportune_json::upgrade_calendar(calendar_json, from_version)
    })
}

/// Rewrites a stored goal as the current schema version's canonical JSON.
///
/// Raises `OpportuneError` for the same reasons `upgrade_rule_json` raises.
#[pyfunction]
fn upgrade_goal_json(
    py: Python<'_>,
    goal_json: &Bound<'_, PyString>,
    from_version: u32,
) -> PyResult<String> {
    let goal_json = error::document(goal_json)?;
    detach(py, || opportune_json::upgrade_goal(goal_json, from_version))
}

/// Adds the extension module's public surface to `module`.
fn register(module: &Bound<'_, PyModule>) -> PyResult<()> {
    module.add("__version__", opportune::VERSION)?;
    module.add("OpportuneError", module.py().get_type::<OpportuneError>())?;
    module.add_class::<Engine>()?;
    // The defaults a Python caller starts from come from `opportune::Limits::default`,
    // so `default_limits()` cannot drift from what the engine applies to a request
    // that names no limits at all.
    module.add(
        "DEFAULT_LIMITS_JSON",
        engine::default_limits_json(module.py())?,
    )?;
    module.add_function(wrap_pyfunction!(upgrade_rule_json, module)?)?;
    module.add_function(wrap_pyfunction!(upgrade_calendar_json, module)?)?;
    module.add_function(wrap_pyfunction!(upgrade_goal_json, module)?)?;
    module.add("MARKER_TAGS", opportune_adapters::MARKER_TAGS)?;
    module.add("MARKER_EVENT_KEY", opportune_adapters::MARKER_EVENT_KEY)?;
    module.add(
        "GRAPH_EXTENDED_PROPERTY_NAMESPACE_GUID",
        opportune_adapters::graph::EXTENDED_PROPERTY_NAMESPACE_GUID,
    )?;
    module.add(
        "GRAPH_EXTENDED_PROPERTY_TAGS_ID",
        opportune_adapters::graph::EXTENDED_PROPERTY_TAGS_ID,
    )?;
    module.add(
        "GRAPH_EXTENDED_PROPERTY_EVENT_KEY_ID",
        opportune_adapters::graph::EXTENDED_PROPERTY_EVENT_KEY_ID,
    )?;
    module.add(
        "GRAPH_EXTENDED_PROPERTY_EXPAND",
        opportune_adapters::graph::EXTENDED_PROPERTY_EXPAND,
    )?;
    Ok(())
}

/// Calendar availability and scheduling calculations across constraints.
#[pymodule(name = "_opportune")]
mod extension {
    use pyo3::prelude::*;

    #[pymodule_init]
    fn init(module: &Bound<'_, PyModule>) -> PyResult<()> {
        crate::register(module)
    }
}

#[cfg(test)]
mod tests {
    use pyo3::prelude::*;
    use pyo3::types::{PyDict, PyModule};
    use serde_json::Value;

    use super::register;

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

    /// A database directory carried with the core crate's own tests: a real `TZif`
    /// tree whose `+VERSION` file says `2018f`.
    const VERSIONED_DIRECTORY: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../opportune/tests/fixtures/tzdb-2018"
    );

    /// The same tree with no `+VERSION` file, so its version is the caller's label.
    const UNVERSIONED_DIRECTORY: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../opportune/tests/fixtures/tzdb-unversioned"
    );

    /// A `calendarView` response with one timed event.
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

    /// The registered module, exactly as the extension initialises it.
    fn module(py: Python<'_>) -> Bound<'_, PyModule> {
        let module = PyModule::new(py, "_opportune").expect("module allocates");
        register(&module).expect("module initialises");
        module
    }

    /// The `_Engine` type the module registers.
    fn engine_type(py: Python<'_>) -> Bound<'_, PyAny> {
        module(py)
            .getattr("_Engine")
            .expect("_Engine is registered")
    }

    /// An `_Engine` built the way a Python caller builds one, from `arguments`.
    fn build_engine<'py>(
        py: Python<'py>,
        arguments: &[(&str, &str)],
    ) -> PyResult<Bound<'py, PyAny>> {
        let kwargs = PyDict::new(py);
        for (name, value) in arguments {
            kwargs.set_item(name, value)?;
        }
        engine_type(py).call((), Some(&kwargs))
    }

    /// An `_Engine` over the bundled database and the default limits.
    fn engine(py: Python<'_>) -> Bound<'_, PyAny> {
        build_engine(py, &[]).expect("the bundled database builds an engine")
    }

    /// `document` parsed as JSON.
    fn json(document: &str) -> Value {
        serde_json::from_str(document).unwrap_or_else(|err| panic!("{document} is not JSON: {err}"))
    }

    /// The string `object` holds.
    fn string(object: &Bound<'_, PyAny>) -> String {
        object.extract().unwrap_or_else(|_| {
            panic!(
                "{} is a string",
                object
                    .repr()
                    .map_or_else(|_| "the value".to_owned(), |repr| repr.to_string())
            )
        })
    }

    /// The string `object.attribute` holds.
    fn attribute(object: &Bound<'_, PyAny>, name: &str) -> String {
        string(
            &object
                .getattr(name)
                .unwrap_or_else(|_| panic!("{name} is present")),
        )
    }

    /// `object.attribute` as an optional string, for the pointer fields.
    fn optional_attribute(object: &Bound<'_, PyAny>, name: &str) -> Option<String> {
        object
            .getattr(name)
            .unwrap_or_else(|_| panic!("{name} is present"))
            .extract()
            .unwrap_or_else(|_| panic!("{name} is a string or None"))
    }

    /// The `key` a finding dict holds, as an optional string.
    fn field(entry: &Bound<'_, PyDict>, key: &str) -> Option<String> {
        entry
            .get_item(key)
            .unwrap_or_else(|_| panic!("{key} is present"))
            .expect("the finding holds a value")
            .extract()
            .unwrap_or_else(|_| panic!("{key} is a string or None"))
    }

    /// The `code` of the `OpportuneError` `raised`.
    fn code(py: Python<'_>, raised: &PyErr) -> String {
        attribute(raised.value(py).as_any(), "code")
    }

    /// The `errors` of the `OpportuneError` `raised`, as Python dicts.
    fn findings<'py>(py: Python<'py>, raised: &PyErr) -> Vec<Bound<'py, PyDict>> {
        raised
            .value(py)
            .getattr("errors")
            .expect("errors is set")
            .extract()
            .expect("errors is a list of dicts")
    }

    #[test]
    fn module_registers_the_crate_version() {
        Python::initialize();
        Python::attach(|py| {
            let version = attribute(module(py).as_any(), "__version__");
            assert_eq!(version, opportune::VERSION);
        });
    }

    #[test]
    fn module_registers_every_marker_constant() {
        Python::initialize();
        Python::attach(|py| {
            let module = module(py);
            let constants: [(&str, &str); 6] = [
                ("MARKER_TAGS", opportune_adapters::MARKER_TAGS),
                ("MARKER_EVENT_KEY", opportune_adapters::MARKER_EVENT_KEY),
                (
                    "GRAPH_EXTENDED_PROPERTY_NAMESPACE_GUID",
                    opportune_adapters::graph::EXTENDED_PROPERTY_NAMESPACE_GUID,
                ),
                (
                    "GRAPH_EXTENDED_PROPERTY_TAGS_ID",
                    opportune_adapters::graph::EXTENDED_PROPERTY_TAGS_ID,
                ),
                (
                    "GRAPH_EXTENDED_PROPERTY_EVENT_KEY_ID",
                    opportune_adapters::graph::EXTENDED_PROPERTY_EVENT_KEY_ID,
                ),
                (
                    "GRAPH_EXTENDED_PROPERTY_EXPAND",
                    opportune_adapters::graph::EXTENDED_PROPERTY_EXPAND,
                ),
            ];
            for (name, expected) in constants {
                assert_eq!(
                    attribute(module.as_any(), name),
                    expected,
                    "{name} carries the adapter's own value"
                );
            }
        });
    }

    #[test]
    fn the_engine_offers_checks_validates_and_reports_its_ranges() {
        Python::initialize();
        Python::attach(|py| {
            let engine = engine(py);
            let version = attribute(&engine, "tzdb_version");
            assert!(!version.is_empty(), "the bundled database has a version");

            let offers = json(&string(
                &engine
                    .call_method1("find_availability_json", (REQUEST,))
                    .expect("the request evaluates"),
            ));
            assert_eq!(
                offers["offers"][0]["events"][0]["start"], "2027-03-09T15:00Z",
                "the first candidate start is offered in an empty calendar"
            );
            assert_eq!(
                offers["meta"]["tzdb"], version,
                "the answer reports the version the engine holds"
            );

            let checked = json(&string(
                &engine
                    .call_method1("check_slot_json", (REQUEST, PROPOSAL))
                    .expect("the proposal is checked"),
            ));
            assert_eq!(checked["valid"], true, "{checked}");

            let diagnostics = json(&string(
                &engine
                    .call_method1("validate_json", (REQUEST,))
                    .expect("the request validates"),
            ));
            assert_eq!(diagnostics["errors"], json("[]"), "{diagnostics}");

            let ranges = json(&string(
                &engine
                    .call_method1("required_ranges_json", (REQUEST,))
                    .expect("the ranges are reported"),
            ));
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
        });
    }

    #[test]
    fn an_engine_built_with_limits_enforces_them() {
        Python::initialize();
        Python::attach(|py| {
            let limits = r#"{
              "max_participants": 500,
              "max_blocks_per_participant": 10000,
              "max_search_window_days": 92,
              "max_candidate_starts": 2,
              "max_explain_limit": 10000
            }"#;
            let engine = build_engine(py, &[("limits_json", limits)]).expect("the limits parse");

            let raised = engine
                .call_method1("find_availability_json", (REQUEST,))
                .expect_err("the candidate count is above the configured maximum");
            assert_eq!(code(py, &raised), "invalid_request");
            let findings = findings(py, &raised);
            assert_eq!(
                field(&findings[0], "code"),
                Some("candidates_limit".to_owned()),
                "the finding names the limit that was exceeded"
            );
            assert_eq!(
                field(&findings[0], "path"),
                Some("/goal/grid".to_owned()),
                "the finding points at the field that produced the candidates"
            );
        });
    }

    #[test]
    fn a_directory_database_reports_its_version_file() {
        Python::initialize();
        Python::attach(|py| {
            let engine = build_engine(py, &[("tzdb_dir", VERSIONED_DIRECTORY)])
                .expect("the fixture database loads");
            assert_eq!(attribute(&engine, "tzdb_version"), "2018f");
        });
    }

    /// The engine formats with the database version it answers with, so a debugger
    /// or a log line naming an engine says which database produced its answers.
    #[test]
    fn the_engine_reports_its_database_version_when_formatted() {
        Python::initialize();
        Python::attach(|py| {
            let engine = engine(py);
            let version = attribute(&engine, "tzdb_version");
            let repr = engine.repr().expect("the engine formats").to_string();
            assert!(repr.contains("_Engine"), "{repr}");
            assert!(repr.contains("tzdb_version"), "{repr}");
            assert!(repr.contains(&version), "{repr}");
        });
    }

    #[test]
    fn a_directory_without_a_version_file_takes_the_caller_label() {
        Python::initialize();
        Python::attach(|py| {
            let engine = build_engine(
                py,
                &[
                    ("tzdb_dir", UNVERSIONED_DIRECTORY),
                    ("tzdb_version", "custom-2018"),
                ],
            )
            .expect("the fixture database loads");
            assert_eq!(attribute(&engine, "tzdb_version"), "custom-2018");
        });
    }

    #[test]
    fn a_version_label_with_no_directory_to_label_raises() {
        Python::initialize();
        Python::attach(|py| {
            let raised = build_engine(py, &[("tzdb_version", "2026f")])
                .expect_err("the bundled database reports its own version");
            assert_eq!(code(py, &raised), "tzdb_version_without_directory");
        });
    }

    #[test]
    fn a_directory_that_cannot_be_read_raises_with_its_cause() {
        Python::initialize();
        Python::attach(|py| {
            let raised = build_engine(py, &[("tzdb_dir", "/nonexistent/opportune-tzdb")])
                .expect_err("no database directory exists there");
            assert_eq!(code(py, &raised), "tzdb_directory_unusable");
        });
    }

    #[test]
    fn a_document_that_is_not_json_raises_its_code_and_cause() {
        Python::initialize();
        Python::attach(|py| {
            let raised = engine(py)
                .call_method1("find_availability_json", ("{ \"now\": ",))
                .expect_err("the document is truncated");
            assert_eq!(code(py, &raised), "invalid_json");
            assert!(
                attribute(raised.value(py).as_any(), "message").contains("EOF"),
                "the message keeps the parser's cause"
            );
        });
    }

    #[test]
    fn a_field_the_wire_shape_does_not_name_raises_its_pointer() {
        Python::initialize();
        Python::attach(|py| {
            let document = REQUEST.replace(
                r#""participants": [{ "id": "ana""#,
                r#""participants": [{ "idd": "ana""#,
            );
            let raised = engine(py)
                .call_method1("find_availability_json", (document,))
                .expect_err("`idd` is not a participant field");
            assert_eq!(code(py, &raised), "unknown_field");
            assert_eq!(
                optional_attribute(raised.value(py).as_any(), "path").as_deref(),
                Some("/participants/0")
            );
        });
    }

    #[test]
    fn a_rejected_request_raises_every_finding_with_its_pointer() {
        Python::initialize();
        Python::attach(|py| {
            let document = REQUEST.replace(r#""zone": "UTC""#, r#""zone": "Mars/Olympus_Mons""#);
            let raised = engine(py)
                .call_method1("find_availability_json", (document,))
                .expect_err("the participant's zone does not exist");
            assert_eq!(code(py, &raised), "invalid_request");
            assert_eq!(
                optional_attribute(raised.value(py).as_any(), "path"),
                None,
                "the pointers live on the findings, not on the failure"
            );
            let findings = findings(py, &raised);
            assert!(!findings.is_empty(), "the failure carries its findings");
            assert!(
                findings.iter().any(
                    |finding| field(finding, "path").as_deref() == Some("/participants/0/zone")
                ),
                "a finding points at the participant's zone: {findings:?}"
            );
        });
    }

    #[test]
    fn a_malformed_proposal_raises_the_pointer_into_it() {
        Python::initialize();
        Python::attach(|py| {
            let proposal = r#"{
              "events": [{
                "part": "main",
                "start": "2027-03-09T15:00Z",
                "assignment": [["ana"], ["ana"]]
              }]
            }"#;
            let raised = engine(py)
                .call_method1("check_slot_json", (REQUEST, proposal))
                .expect_err("the goal has one term and the proposal lists two");
            assert_eq!(code(py, &raised), "proposal_term_count");
            assert_eq!(
                optional_attribute(raised.value(py).as_any(), "path").as_deref(),
                Some("/events/0/assignment"),
                "the pointer names the field of the proposal that does not fit"
            );
        });
    }

    #[test]
    fn an_unsupported_schema_version_raises_before_the_document_is_read() {
        Python::initialize();
        Python::attach(|py| {
            let module = module(py);
            let upgrade = module
                .getattr("upgrade_rule_json")
                .expect("upgrade_rule_json is registered");
            let rule = r#"{"id": "r_min", "kind": "lead_time", "params": { "min": "4h" }}"#;
            let canonical = string(&upgrade.call1((rule, 1)).expect("version 1 is read"));
            assert!(
                canonical.contains(r#""id":"r_min""#),
                "the canonical form keeps the rule: {canonical}"
            );

            let raised = upgrade
                .call1((rule, 99))
                .expect_err("no build reads version 99");
            assert_eq!(code(py, &raised), "unsupported_schema_version");
        });
    }

    #[test]
    fn every_stored_document_upgrades_through_its_own_function() {
        Python::initialize();
        Python::attach(|py| {
            let module = module(py);
            let calendar = string(
                &module
                    .getattr("upgrade_calendar_json")
                    .expect("upgrade_calendar_json is registered")
                    .call1((
                        r#"{ "zone": "Europe/London", "working_days": "mon-fri" }"#,
                        1,
                    ))
                    .expect("the calendar upgrades"),
            );
            assert!(
                calendar.contains(r#""working_days":"mon,tue,wed,thu,fri""#),
                "the day key is spelled out: {calendar}"
            );

            let goal = string(
                &module
                    .getattr("upgrade_goal_json")
                    .expect("upgrade_goal_json is registered")
                    .call1((
                        r#"{
                          "parts": [{ "duration": "45m", "requirement": { "all": [{ "person": "ana" }] } }],
                          "search_window": ["2027-03-09T15:00Z", "2027-03-10T15:00Z"],
                          "zone": "UTC"
                        }"#,
                        1,
                    ))
                    .expect("the goal upgrades"),
            );
            assert!(
                goal.contains(r#""display_zone":"UTC""#),
                "the display zone defaults to the event zone: {goal}"
            );
        });
    }

    #[test]
    fn the_graph_adapter_methods_convert_their_payloads() {
        Python::initialize();
        Python::attach(|py| {
            let engine = engine(py);

            let calendar_view = json(&string(
                &engine
                    .call_method1(
                        "graph_calendar_view_to_blocks_json",
                        (GRAPH_CALENDAR_VIEW, "America/New_York"),
                    )
                    .expect("the calendar view converts"),
            ));
            assert_eq!(calendar_view["status"], "available");
            assert_eq!(calendar_view["blocks"][0]["event_id"], "AAMkAD");
            assert_eq!(
                calendar_view["blocks"][0]["block"]["tags"][0], "category/interview",
                "an unmapped category becomes a slugged tag"
            );

            let schedule = json(&string(
                &engine
                    .call_method1("graph_get_schedule_to_blocks_json", (GRAPH_GET_SCHEDULE,))
                    .expect("the schedule converts"),
            ));
            assert_eq!(schedule["ana@example.com"]["status"], "available");
            assert_eq!(
                schedule["ana@example.com"]["blocks"][0]["event_id"],
                Value::Null,
                "a free/busy source carries no event ID"
            );

            let working_hours = json(&string(
                &engine
                    .call_method1("graph_working_hours_to_rule_json", (GRAPH_WORKING_HOURS,))
                    .expect("the working hours convert"),
            ));
            assert_eq!(working_hours["status"], "available");
            assert_eq!(working_hours["rule"]["kind"], "window");
            assert_eq!(working_hours["rule"]["id"], "working_hours");

            let events = json(&string(
                &engine
                    .call_method1(
                        "google_events_list_to_blocks_json",
                        (GOOGLE_EVENTS_LIST, "UTC"),
                    )
                    .expect("the events convert"),
            ));
            assert_eq!(events["blocks"][0]["event_id"], "evt-1");

            let free_busy = json(&string(
                &engine
                    .call_method1("google_free_busy_to_blocks_json", (GOOGLE_FREE_BUSY,))
                    .expect("the free/busy ranges convert"),
            ));
            assert_eq!(
                free_busy["ana@example.com"]["blocks"][0]["block"]["status"],
                "busy"
            );
        });
    }

    /// A wheel built without the default `windows-zones` feature would answer
    /// `zone_unknown` here — `unavailable` for the whole mailbox rather than a
    /// rule — which is the failure the feature exists to prevent.
    #[test]
    #[cfg(feature = "windows-zones")]
    fn the_default_feature_resolves_a_windows_zone_name() {
        const WINDOWS_ZONE: &str = r#"{
          "daysOfWeek": ["monday"],
          "startTime": "09:00:00",
          "endTime": "17:00:00",
          "timeZone": { "name": "GMT Standard Time" }
        }"#;
        Python::initialize();
        Python::attach(|py| {
            let working_hours = json(&string(
                &engine(py)
                    .call_method1("graph_working_hours_to_rule_json", (WINDOWS_ZONE,))
                    .expect("the working hours convert"),
            ));
            assert_eq!(
                working_hours["status"], "available",
                "a wheel built with the default feature resolves `GMT Standard Time`: {working_hours}"
            );
        });
    }

    #[test]
    fn an_unreadable_adapter_payload_is_an_outcome_rather_than_an_exception() {
        Python::initialize();
        Python::attach(|py| {
            let outcome = json(&string(
                &engine(py)
                    .call_method1(
                        "graph_calendar_view_to_blocks_json",
                        ("{ \"value\": 7 }", "UTC"),
                    )
                    .expect("an unreadable payload is about the calendar, not the call"),
            ));
            assert_eq!(outcome["status"], "unavailable", "{outcome}");
        });
    }

    #[test]
    fn one_engine_answers_concurrent_calls_identically() {
        Python::initialize();
        let engine = Python::attach(|py| engine(py).unbind());
        let expected = Python::attach(|py| {
            string(
                &engine
                    .bind(py)
                    .call_method1("find_availability_json", (REQUEST,))
                    .expect("the request evaluates"),
            )
        });

        let threads: Vec<_> = Python::attach(|py| {
            (0..8)
                .map(|_| {
                    let engine = engine.clone_ref(py);
                    std::thread::spawn(move || {
                        Python::attach(|py| {
                            string(
                                &engine
                                    .bind(py)
                                    .call_method1("find_availability_json", (REQUEST,))
                                    .expect("a shared engine answers a second thread"),
                            )
                        })
                    })
                })
                .collect()
        });
        for thread in threads {
            let answer = thread.join().expect("the thread does not panic");
            assert_eq!(answer, expected, "every thread gets the same answer");
        }
    }

    /// A client document can hold an unpaired surrogate: `json.loads` accepts an
    /// `"\ud800"` escape, and a caller that re-serialises it without ASCII
    /// escaping hands Python a string holding a lone surrogate, which UTF-8
    /// cannot spell. The same document written with ASCII escapes already fails
    /// as `invalid_json`, so the unreadable form has to fail the same way rather
    /// than escaping as the encoder's own `UnicodeEncodeError`, which no
    /// `OpportuneError` handler catches.
    #[test]
    fn a_document_with_an_unpaired_surrogate_is_rejected_as_invalid_json() {
        Python::initialize();
        Python::attach(|py| {
            let document = py
                .eval(
                    cr#"'{"participants": [{"id": "' + chr(0xD800) + '", "zone": "UTC", "calendar": []}]}'"#,
                    None,
                    None,
                )
                .expect("a document holding one unpaired surrogate");
            let raised = engine(py)
                .call_method1("find_availability_json", (document,))
                .expect_err("the document has no UTF-8 spelling");
            assert!(
                raised.is_instance_of::<super::OpportuneError>(py),
                "a document the boundary cannot read raises the module's own exception"
            );
            assert_eq!(code(py, &raised), "invalid_json");
        });
    }
}
