//! The JSON envelopes and the codegen schema over them.

// Every conversion here resolves zone names against a real database, so this
// target runs where the crate compiles one in: `make test-matrix`'s `graph` build
// keeps `bundled-tzdb` on, and checks the bundle-free build separately.
#![cfg(all(feature = "json", feature = "graph", feature = "bundled-tzdb"))]

use opportune::time::TimeZoneDatabase;
use opportune_adapters::json::{
    WireCalendarOutcome, WireWorkingHoursOutcome, calendar_view_to_blocks,
    calendar_view_to_blocks_with_db, get_schedule_to_blocks, get_schedule_to_blocks_with_db,
    working_hours_to_rule, working_hours_to_rule_with_db,
};
use serde_json::{Value, json};

/// A minimal `TZif` database directory carried with this crate's tests: a real
/// `TZif` tree holding `America/Sao_Paulo` and no other zone.
const DIRECTORY: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/tzdb-2018");

/// The database the `_with_db` variants must read.
fn directory_db() -> TimeZoneDatabase {
    TimeZoneDatabase::from_dir(DIRECTORY, None).expect("the fixture database directory loads")
}

/// One event whose conversion exercises the block wire shape.
const CALENDAR: &str = r#"{
  "value": [{
    "id": "AAMkAD",
    "type": "singleInstance",
    "isAllDay": false,
    "isCancelled": false,
    "isDraft": false,
    "showAs": "OOF",
    "iCalUId": "ical-1",
    "start": { "dateTime": "2027-03-09T14:00:00.0000000", "timeZone": "UTC" },
    "end": { "dateTime": "2027-03-09T15:00:00.0000000", "timeZone": "UTC" },
    "categories": ["Interview"]
  }, {
    "id": "AAMkAE",
    "isAllDay": false,
    "showAs": "busy",
    "start": { "dateTime": "2027-03-09T10:00:30.0000000", "timeZone": "UTC" },
    "end": { "dateTime": "2027-03-09T10:00:30.0000000", "timeZone": "UTC" }
  }]
}"#;

#[test]
fn a_calendar_envelope_carries_blocks_and_diagnostics() {
    let outcome = calendar_view_to_blocks(CALENDAR, "America/New_York", None).unwrap();
    let value: Value = serde_json::from_str(&outcome).unwrap();
    assert_eq!(value["status"], "available");
    assert_eq!(value["blocks"].as_array().unwrap().len(), 1);
    assert_eq!(value["blocks"][0]["event_id"], "AAMkAD");
    let block = &value["blocks"][0]["block"];
    assert_eq!(
        block["span"],
        json!(["2027-03-09T14:00Z", "2027-03-09T15:00Z"])
    );
    assert_eq!(block["status"], "busy");
    assert_eq!(block["all_day"], false);
    assert_eq!(block["tags"], json!(["status/oof", "category/interview"]));
    assert_eq!(block["event_key"], "ical-1");
    assert_eq!(value["diagnostics"][0]["code"], "zero_length_event");
    assert_eq!(value["diagnostics"][0]["event_id"], "AAMkAE");
    assert!(value["diagnostics"][0]["message"].is_string());

    // The envelope is closed and symmetric: what the adapter wrote parses back
    // to the same bytes, so a binding can read it with the same models.
    let parsed: WireCalendarOutcome = serde_json::from_str(&outcome).unwrap();
    assert_eq!(serde_json::to_string(&parsed).unwrap(), outcome);
}

#[test]
fn a_calendar_that_cannot_convert_is_an_unavailable_envelope() {
    let outcome = calendar_view_to_blocks("not json", "UTC", None).unwrap();
    let value: Value = serde_json::from_str(&outcome).unwrap();
    assert_eq!(value["status"], "unavailable");
    assert_eq!(value["code"], "payload_invalid");
    assert!(value["message"].as_str().unwrap().contains("not readable"));
    // The union is discriminated, so an unavailable outcome carries no blocks.
    assert!(value.get("blocks").is_none());
}

#[test]
fn an_envelope_rejects_a_key_it_does_not_name() {
    let outcome = calendar_view_to_blocks(CALENDAR, "UTC", None).unwrap();
    let mut value: Value = serde_json::from_str(&outcome).unwrap();
    value["extra"] = json!(1);
    let error = serde_json::from_value::<WireCalendarOutcome>(value).unwrap_err();
    assert!(error.to_string().contains("unknown field"), "{error}");
}

#[test]
fn a_get_schedule_envelope_is_keyed_by_schedule_id() {
    let payload = include_str!("fixtures/graph/get_schedule.json");
    let outcome = get_schedule_to_blocks(payload).unwrap();
    let value: Value = serde_json::from_str(&outcome).unwrap();
    assert_eq!(value["ana@example.com"]["status"], "available");
    assert_eq!(
        value["ana@example.com"]["blocks"][0]["block"]["span"],
        json!(["2027-03-09T15:00Z", "2027-03-09T16:00Z"])
    );
    // Free/busy carries no event identity.
    assert_eq!(
        value["ana@example.com"]["blocks"][0]["event_id"],
        Value::Null
    );
    assert_eq!(value["room-vienna@example.com"]["status"], "unavailable");
    assert_eq!(value["room-vienna@example.com"]["code"], "provider_error");
    assert_eq!(value["bo@example.com"]["blocks"], json!([]));
}

#[test]
fn a_working_hours_envelope_carries_the_rule_or_the_reason() {
    let outcome =
        working_hours_to_rule(include_str!("fixtures/graph/working_hours.json"), "wh").unwrap();
    let value: Value = serde_json::from_str(&outcome).unwrap();
    assert_eq!(value["status"], "available");
    assert_eq!(value["rule"]["id"], "wh");
    assert_eq!(value["rule"]["kind"], "window");
    assert_eq!(value["rule"]["params"]["zone"], "America/New_York");
    let _: WireWorkingHoursOutcome = serde_json::from_str(&outcome).unwrap();

    let custom = json!({
        "daysOfWeek": ["monday"],
        "startTime": "09:00:00.0000000",
        "endTime": "17:00:00.0000000",
        "timeZone": { "@odata.type": "#microsoft.graph.customTimeZone", "bias": 60 }
    })
    .to_string();
    let outcome = working_hours_to_rule(&custom, "wh").unwrap();
    let value: Value = serde_json::from_str(&outcome).unwrap();
    assert_eq!(value["status"], "unavailable");
    assert_eq!(value["code"], "custom_time_zone");
    let _: WireWorkingHoursOutcome = serde_json::from_str(&outcome).unwrap();
}

#[test]
fn a_category_map_is_validated_with_the_pointer_of_the_entry_at_fault() {
    let error = calendar_view_to_blocks(CALENDAR, "UTC", Some("not json")).unwrap_err();
    assert_eq!(error.code(), "invalid_json");

    let error = calendar_view_to_blocks(CALENDAR, "UTC", Some(r#"{ "Interview": "not a tag" }"#))
        .unwrap_err();
    assert_eq!(error.code(), "tag_invalid_character");
    assert_eq!(error.path(), "/Interview");
}

#[test]
fn a_blank_participant_zone_is_rejected_before_the_payload_is_read() {
    let error = calendar_view_to_blocks(CALENDAR, "  ", None).unwrap_err();
    assert_eq!(error.code(), "zone_name_invalid");
    assert_eq!(error.path(), "/participant_zone");
}

/// One event stamped in `zone`, which is all a `_with_db` conversion needs to show
/// which database it read.
fn stamped(zone: &str) -> String {
    json!({
        "value": [{
            "id": "AAMkAD",
            "isAllDay": false,
            "showAs": "busy",
            "start": { "dateTime": "2018-07-02T10:00:00.0000000", "timeZone": zone },
            "end": { "dateTime": "2018-07-02T11:00:00.0000000", "timeZone": zone }
        }]
    })
    .to_string()
}

/// The `_with_db` variants read zone names from the database they are given, which
/// is what lets an engine built from a `tzdb_dir` resolve adapters and evaluation
/// against the same data.
#[test]
fn a_with_db_conversion_resolves_zones_in_its_own_database() {
    let db = directory_db();

    // A zone the directory holds converts, and its own data decides the instant:
    // 10:00 in Sao Paulo in July 2018 was 13:00Z.
    let outcome =
        calendar_view_to_blocks_with_db(&db, &stamped("America/Sao_Paulo"), "UTC", None).unwrap();
    let value: Value = serde_json::from_str(&outcome).unwrap();
    assert_eq!(value["status"], "available", "{outcome}");
    assert_eq!(
        value["blocks"][0]["block"]["span"],
        json!(["2018-07-02T13:00Z", "2018-07-02T14:00Z"]),
    );

    // A zone the directory lacks is unknown, although the bundled database has it:
    // the conversion consulted the database it was given, not the bundled one.
    let outcome =
        calendar_view_to_blocks_with_db(&db, &stamped("America/New_York"), "UTC", None).unwrap();
    let value: Value = serde_json::from_str(&outcome).unwrap();
    assert_eq!(value["status"], "unavailable", "{outcome}");
    assert_eq!(value["code"], "zone_unknown");
    assert_eq!(
        serde_json::from_str::<Value>(
            &calendar_view_to_blocks(&stamped("America/New_York"), "UTC", None).unwrap()
        )
        .unwrap()["status"],
        "available",
        "the bundled database does resolve the zone, so the difference is the database",
    );
}

/// The other Graph envelopes read the database they are given too, so a caller
/// passes one database to every conversion it runs.
#[test]
fn every_graph_with_db_variant_reads_its_own_database() {
    let db = directory_db();

    let schedule = json!({
        "value": [{
            "scheduleId": "room@example.com",
            "scheduleItems": [{
                "status": "busy",
                "start": { "dateTime": "2018-07-02T10:00:00.0000000", "timeZone": "America/New_York" },
                "end": { "dateTime": "2018-07-02T11:00:00.0000000", "timeZone": "America/New_York" }
            }]
        }]
    })
    .to_string();
    let outcomes: Value =
        serde_json::from_str(&get_schedule_to_blocks_with_db(&db, &schedule).unwrap()).unwrap();
    assert_eq!(outcomes["room@example.com"]["code"], "zone_unknown");
    let bundled: Value = serde_json::from_str(&get_schedule_to_blocks(&schedule).unwrap()).unwrap();
    assert_eq!(bundled["room@example.com"]["status"], "available");

    let working_hours = json!({
        "daysOfWeek": ["monday"],
        "startTime": "09:00:00.0000000",
        "endTime": "17:00:00.0000000",
        "timeZone": { "name": "America/New_York" }
    })
    .to_string();
    let outcome: Value =
        serde_json::from_str(&working_hours_to_rule_with_db(&db, &working_hours, "wh").unwrap())
            .unwrap();
    assert_eq!(outcome["status"], "unavailable");
    assert_eq!(outcome["code"], "zone_unknown");
    let bundled: Value =
        serde_json::from_str(&working_hours_to_rule(&working_hours, "wh").unwrap()).unwrap();
    assert_eq!(bundled["status"], "available");
}

#[cfg(feature = "schema")]
mod schema {
    use std::collections::BTreeSet;

    use opportune_adapters::json::schema;
    use serde_json::Value;

    /// Every `$ref` in `value`.
    fn references(value: &Value, found: &mut Vec<String>) {
        match value {
            Value::Object(object) => {
                for (key, inner) in object {
                    if key == "$ref" {
                        found.push(inner.as_str().unwrap().to_owned());
                    } else {
                        references(inner, found);
                    }
                }
            }
            Value::Array(items) => items.iter().for_each(|item| references(item, found)),
            Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
        }
    }

    /// The paths of every schema object that names properties.
    fn objects_with_properties<'a>(
        value: &'a Value,
        path: &str,
        found: &mut Vec<(String, &'a Value)>,
    ) {
        match value {
            Value::Object(object) => {
                if object.get("properties").is_some_and(Value::is_object) {
                    found.push((path.to_owned(), value));
                }
                for (key, inner) in object {
                    objects_with_properties(inner, &format!("{path}/{key}"), found);
                }
            }
            Value::Array(items) => {
                for (index, inner) in items.iter().enumerate() {
                    objects_with_properties(inner, &format!("{path}/{index}"), found);
                }
            }
            Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
        }
    }

    /// The response and output definitions the wire layer leaves open, inherited
    /// by this bundle unchanged. They match `opportune_json`'s list, so one split
    /// governs both bundles.
    const OPEN_DEFINITIONS: [&str; 13] = [
        "Response",
        "Offer",
        "OfferEvent",
        "ReserveSpan",
        "DateCount",
        "Excluded",
        "Reason",
        "ReasonParams",
        "Diagnostic",
        "Diagnostics",
        "Meta",
        "SlotCheck",
        "RequiredRanges",
    ];

    /// The top-level definition a `#/$defs` path sits in.
    fn top_definition(path: &str) -> &str {
        path.strip_prefix("#/$defs/")
            .and_then(|rest| rest.split('/').next())
            .unwrap_or_else(|| panic!("`{path}` is not inside a definition"))
    }

    #[test]
    fn the_bundle_carries_the_adapter_envelopes_and_the_shared_wire_types() {
        let bundle = schema();
        assert_eq!(
            bundle["$schema"],
            "https://json-schema.org/draft/2020-12/schema"
        );
        for name in [
            "CalendarOutcome",
            "AnnotatedBlock",
            "AdapterDiagnostic",
            "GraphWorkingHoursOutcome",
            "Block",
            "Span",
            "Rule",
            "WindowRule",
            "WindowParams",
        ] {
            assert!(bundle["$defs"][name].is_object(), "`{name}` is missing");
        }
        // The rule variants are titled where they are written, and the `window`
        // variant is the one lifted to a definition the union points at.
        let rule_titles: Vec<&str> = bundle["$defs"]["Rule"]["oneOf"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|variant| variant["title"].as_str())
            .collect();
        assert!(rule_titles.contains(&"BlackoutRule"), "{rule_titles:?}");
        assert!(rule_titles.iter().all(|title| *title != "WindowRule"));
        assert!(
            bundle["$defs"]["Rule"]["oneOf"]
                .as_array()
                .unwrap()
                .contains(&serde_json::json!({ "$ref": "#/$defs/WindowRule" })),
        );
        assert_eq!(
            bundle["$defs"]["MatchAll"],
            serde_json::json!({ "const": true })
        );
    }

    #[test]
    fn the_working_hours_outcome_names_the_window_rule_a_conversion_produces() {
        // A conversion only ever answers with a `window` rule, so the generated
        // model holds that shape rather than a seven-way union its reader would
        // have to narrow. The reference is to the shared definition, not a second
        // spelling of it.
        let bundle = schema();
        let available = bundle["$defs"]["GraphWorkingHoursOutcome"]["oneOf"]
            .as_array()
            .unwrap()
            .iter()
            .find(|variant| variant["title"] == "AvailableWorkingHoursOutcome")
            .expect("the available outcome is a variant of the working-hours union");
        assert_eq!(
            available["properties"]["rule"]["$ref"],
            "#/$defs/WindowRule",
        );
        assert_eq!(
            bundle["$defs"]["WindowRule"]["properties"]["kind"]["const"],
            "window"
        );
    }

    #[test]
    fn the_shared_definitions_are_the_wire_layers_own() {
        // A binding generates one model set, so the adapter must not define a
        // second spelling of a type both layers carry: the published bundle starts
        // from the wire layer's and only adds its own envelopes, so every wire
        // definition reaches the exported document unchanged. That is what lets
        // the wire layer's schema tests police the bundle a consumer reads.
        let bundle = schema();
        let core = opportune_json::schema();
        let published = bundle["$defs"]
            .as_object()
            .expect("the bundle has definitions");
        let missing: Vec<&String> = core["$defs"]
            .as_object()
            .expect("the wire layer's bundle has definitions")
            .iter()
            .filter(|(name, definition)| published.get(*name) != Some(*definition))
            .map(|(name, _)| name)
            .collect();
        assert!(
            missing.is_empty(),
            "these wire definitions do not reach the published bundle unchanged: {missing:?}",
        );
    }

    #[test]
    fn every_reference_resolves_and_inputs_are_closed_while_responses_are_open() {
        let bundle = schema();
        let definitions = bundle["$defs"].as_object().unwrap();
        let mut refs = Vec::new();
        references(&bundle["$defs"], &mut refs);
        let names: BTreeSet<&str> = definitions.keys().map(String::as_str).collect();
        for reference in refs {
            let name = reference
                .strip_prefix("#/$defs/")
                .unwrap_or_else(|| panic!("`{reference}` is not a local definition reference"));
            assert!(names.contains(name), "`{reference}` names no definition");
        }

        // The bundle inherits the wire layer's split: the core response and output
        // definitions stay open, because serde reads those types without
        // `deny_unknown_fields`, so a generated model must tolerate the same
        // additive field the reader tolerates.
        for name in OPEN_DEFINITIONS {
            assert!(
                definitions.contains_key(name),
                "response definition `{name}` is missing from the bundle",
            );
            assert_ne!(
                definitions[name].get("additionalProperties"),
                Some(&serde_json::json!(false)),
                "response definition `{name}` is closed, so a generated model would reject \
                 a field a newer engine added",
            );
        }

        // Every adapter envelope is closed, and every input object with it: serde
        // rejects a key those types do not name.
        let mut objects = Vec::new();
        objects_with_properties(&bundle["$defs"], "#/$defs", &mut objects);
        assert!(objects.len() > 10);
        let mut open = 0;
        for (path, schema) in objects {
            if OPEN_DEFINITIONS.contains(&top_definition(&path)) {
                assert_ne!(
                    schema["additionalProperties"],
                    serde_json::json!(false),
                    "`{path}` closes a response object, so a generated model would \
                     reject a field a newer engine added",
                );
                open += 1;
            } else {
                assert_eq!(
                    schema["additionalProperties"],
                    serde_json::json!(false),
                    "`{path}` accepts an unknown key"
                );
            }
        }
        assert_eq!(
            open,
            OPEN_DEFINITIONS.len(),
            "each response definition must contribute exactly one open object",
        );
    }

    #[test]
    fn each_union_titles_the_variants_a_generated_model_names() {
        let bundle = schema();
        let titles = |name: &str| -> Vec<String> {
            bundle["$defs"][name]["oneOf"]
                .as_array()
                .unwrap_or_else(|| panic!("`{name}` is not a union"))
                .iter()
                .filter_map(|variant| variant["title"].as_str().map(str::to_owned))
                .collect()
        };
        assert_eq!(
            titles("CalendarOutcome"),
            ["AvailableCalendarOutcome", "UnavailableCalendarOutcome"]
        );
        assert_eq!(
            titles("GraphWorkingHoursOutcome"),
            [
                "AvailableWorkingHoursOutcome",
                "UnavailableWorkingHoursOutcome"
            ]
        );
        // Each variant is discriminated on `status`, which is what makes the
        // generated union a tagged one rather than an optional-everything class.
        for union in ["CalendarOutcome", "GraphWorkingHoursOutcome"] {
            let statuses: Vec<&str> = bundle["$defs"][union]["oneOf"]
                .as_array()
                .unwrap()
                .iter()
                .filter_map(|variant| variant["properties"]["status"]["const"].as_str())
                .collect();
            assert_eq!(statuses, ["available", "unavailable"], "`{union}`");
        }
    }

    #[test]
    fn each_union_names_the_property_that_discriminates_it() {
        let bundle = schema();
        for union in ["CalendarOutcome", "GraphWorkingHoursOutcome"] {
            assert_eq!(
                bundle["$defs"][union]["discriminator"],
                serde_json::json!({ "propertyName": "status" }),
                "`{union}`",
            );
            // The property it names is the one every variant is tagged with.
            for variant in bundle["$defs"][union]["oneOf"].as_array().unwrap() {
                assert!(
                    variant["properties"]["status"]["const"].is_string(),
                    "a variant without `status` cannot be told apart: {variant}",
                );
            }
        }
    }

    #[test]
    fn two_calls_produce_the_same_document() {
        assert_eq!(
            serde_json::to_string(&schema()).unwrap(),
            serde_json::to_string(&schema()).unwrap()
        );
    }
}

/// The bundled entry points now delegate to the `_with_db` ones, so handing the
/// `_with_db` variant the bundled database has to produce the same document the
/// bundled entry point does — otherwise a binding that switches to the seam it needs
/// for a `tzdb_dir` engine would silently convert different calendars than the calls
/// it replaced.
///
/// Each conversion is probed with the recorded provider payload it is built for, so
/// the comparison covers the zone-bearing paths (an all-day span, a Windows name and
/// a per-timestamp zone) rather than only the shape of the envelope.
#[test]
fn the_bundled_entry_point_is_the_with_db_one_on_the_bundled_database() {
    let db = TimeZoneDatabase::bundled().expect("the bundled database loads");
    let payload = |provider: &str, name: &str| {
        std::fs::read_to_string(format!(
            "{}/tests/fixtures/{provider}/{name}",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap_or_else(|err| panic!("{provider}/{name}: {err}"))
    };

    for name in [
        "calendar_view_ana.json",
        "calendar_view_bo.json",
        "calendar_view_tenant.json",
        "all_day_dst.json",
    ] {
        let calendar = payload("graph", name);
        assert_eq!(
            calendar_view_to_blocks(&calendar, "America/Los_Angeles", None).unwrap(),
            calendar_view_to_blocks_with_db(&db, &calendar, "America/Los_Angeles", None).unwrap(),
            "{name}",
        );
    }

    let schedule = payload("graph", "get_schedule.json");
    assert_eq!(
        get_schedule_to_blocks(&schedule).unwrap(),
        get_schedule_to_blocks_with_db(&db, &schedule).unwrap(),
    );

    let hours = payload("graph", "working_hours.json");
    assert_eq!(
        working_hours_to_rule(&hours, "wh").unwrap(),
        working_hours_to_rule_with_db(&db, &hours, "wh").unwrap(),
    );
}
