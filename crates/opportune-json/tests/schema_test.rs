//! The JSON Schema bundle: the definitions it names, what each one states about
//! the wire format, and that the advertised shapes are the ones the parser
//! accepts.

#![cfg(feature = "schema")]

use std::collections::BTreeSet;

use serde_json::{Map, Value};

use opportune::model::{BlockStatus, ParticipantKind};
use opportune::tags::Selector;
use opportune_json::{WireRequest, parse_request, schema, to_json};

/// The definition names a binding generates its models from.
const ENTRY_POINTS: [&str; 12] = [
    "Request",
    "Response",
    "Proposal",
    "SlotCheck",
    "RangesQuery",
    "RequiredRanges",
    "Diagnostics",
    "Rule",
    "CalendarSpec",
    "Goal",
    "Limits",
    "Block",
];

/// The title each `Rule` variant carries, keyed by the `kind` it is tagged with.
const RULE_TITLES: [(&str, &str); 7] = [
    ("window", "WindowRule"),
    ("blackout", "BlackoutRule"),
    ("clearance", "ClearanceRule"),
    ("lead_time", "LeadTimeRule"),
    ("status_policy", "StatusPolicyRule"),
    ("rest", "RestRule"),
    ("cap", "CapRule"),
];

/// The response and output definitions, which are left open so a generated model
/// tolerates the additive fields serde reads.
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

/// The bundle's definitions.
fn definitions() -> Map<String, Value> {
    schema()["$defs"]
        .as_object()
        .cloned()
        .expect("the bundle has no `$defs` object")
}

/// The shape of a property's value: a field that reads an explicit `null` as its
/// default states its value in the first arm of an `anyOf`, with the annotations
/// that describe the field beside it.
fn shape(property: &Value) -> &Value {
    property
        .get("anyOf")
        .and_then(Value::as_array)
        .and_then(|arms| arms.first())
        .unwrap_or(property)
}

/// Whether `schema` admits `null` as a value: a type that names it, the constant,
/// or an arm of a union that does.
fn admits_null(schema: &Value) -> bool {
    let names_null = match schema.get("type") {
        Some(Value::String(name)) => name == "null",
        Some(Value::Array(names)) => names.iter().any(|name| name == "null"),
        Some(Value::Bool(_) | Value::Number(_) | Value::Object(_) | Value::Null) | None => false,
    };
    names_null
        || schema.get("const") == Some(&Value::Null)
        || ["anyOf", "oneOf"].iter().any(|keyword| {
            schema
                .get(*keyword)
                .and_then(Value::as_array)
                .is_some_and(|arms| arms.iter().any(admits_null))
        })
}

/// Calls `visit` with the JSON pointer of every property schema in `value`, and
/// whether the object carrying it names that property in `required`.
fn properties(value: &Value, pointer: &str, visit: &mut impl FnMut(&str, &Value, bool)) {
    match value {
        Value::Object(object) => {
            for (key, inner) in object {
                let pointer = format!("{pointer}/{key}");
                if key == "properties" {
                    let required = object.get("required").and_then(Value::as_array);
                    for (name, property) in inner.as_object().into_iter().flatten() {
                        let named = required.is_some_and(|names| {
                            names
                                .iter()
                                .any(|item| item.as_str() == Some(name.as_str()))
                        });
                        visit(&format!("{pointer}/{name}"), property, named);
                    }
                }
                properties(inner, &pointer, visit);
            }
        }
        Value::Array(items) => {
            for (index, inner) in items.iter().enumerate() {
                properties(inner, &format!("{pointer}/{index}"), visit);
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
    }
}

/// Calls `visit` with every JSON object and array in `value`, in document order.
fn walk(value: &Value, visit: &mut impl FnMut(&Value)) {
    visit(value);
    match value {
        Value::Object(object) => object.values().for_each(|inner| walk(inner, visit)),
        Value::Array(items) => items.iter().for_each(|inner| walk(inner, visit)),
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
    }
}

#[test]
fn the_bundle_is_a_deterministic_draft_2020_12_document() {
    let bundle = schema();
    assert_eq!(
        bundle["$schema"], "https://json-schema.org/draft/2020-12/schema",
        "the bundle declares the dialect a generated model is read in",
    );
    assert_eq!(
        serde_json::to_string(&bundle).unwrap(),
        serde_json::to_string(&schema()).unwrap(),
        "two calls must return the same bytes, or a codegen drift check cannot pass",
    );
}

#[test]
fn every_entry_point_is_a_definition_named_for_its_type() {
    let definitions = definitions();
    for name in ENTRY_POINTS {
        assert!(
            definitions.contains_key(name),
            "`{name}` is missing from the bundle's definitions",
        );
    }
    let prefixed: Vec<&String> = definitions
        .keys()
        .filter(|name| name.starts_with("Wire"))
        .collect();
    assert!(
        prefixed.is_empty(),
        "these definitions kept the wire prefix: {prefixed:?}",
    );
    for (name, definition) in &definitions {
        if name == "MatchAll" {
            continue;
        }
        assert_eq!(
            definition["title"], *name,
            "a generated model takes its name from the title, so `{name}` needs it",
        );
    }
}

#[test]
fn every_reference_names_a_definition_that_exists() {
    let definitions = definitions();
    let mut references = 0;
    walk(&Value::Object(definitions.clone()), &mut |value| {
        if let Some(reference) = value.get("$ref").and_then(Value::as_str) {
            let name = reference
                .strip_prefix("#/$defs/")
                .unwrap_or_else(|| panic!("`{reference}` is not an internal reference"));
            assert!(
                definitions.contains_key(name),
                "`{reference}` names no definition",
            );
            references += 1;
        }
    });
    assert!(
        references > 50,
        "only {references} references were found, which cannot be the whole bundle",
    );
}

/// The bundle states the exact split serde enforces: a request or input object
/// rejects a key it does not name, because the reader has `deny_unknown_fields`;
/// a response or output object is open, because the reader accepts a field a
/// newer engine added. Closing the responses would turn an additive engine release
/// into a breaking one, and opening an input would read a typo as a default.
#[test]
fn inputs_are_closed_and_responses_are_open() {
    let definitions = definitions();

    for name in OPEN_DEFINITIONS {
        assert!(
            definitions.contains_key(name),
            "response definition `{name}` is missing from the bundle",
        );
        assert_ne!(
            definitions[name].get("additionalProperties"),
            Some(&Value::Bool(false)),
            "response definition `{name}` is closed, so a generated model would reject \
             a field a newer engine added",
        );
    }

    let mut closed = 0;
    let mut open = 0;
    for (name, definition) in &definitions {
        let response = OPEN_DEFINITIONS.contains(&name.as_str());
        walk(definition, &mut |value| {
            let Some(object) = value.as_object() else {
                return;
            };
            if object.get("type").and_then(Value::as_str) != Some("object")
                || object
                    .get("properties")
                    .and_then(Value::as_object)
                    .is_none()
            {
                return;
            }
            if response {
                assert_ne!(
                    object.get("additionalProperties"),
                    Some(&Value::Bool(false)),
                    "response definition `{name}` closes an object, so a generated \
                     model would reject a field a newer engine added: {value}",
                );
                open += 1;
            } else {
                assert_eq!(
                    object.get("additionalProperties"),
                    Some(&Value::Bool(false)),
                    "input definition `{name}` leaves an object open: {value}",
                );
                closed += 1;
            }
        });
    }
    assert!(
        closed > 30,
        "only {closed} input object types were found, which cannot be the whole wire",
    );
    assert_eq!(
        open,
        OPEN_DEFINITIONS.len(),
        "each response definition must contribute exactly one open object",
    );
}

#[test]
fn the_match_all_selector_is_the_constant_true() {
    let definitions = definitions();
    assert_eq!(
        definitions["MatchAll"],
        serde_json::json!({ "const": true })
    );

    let forms = definitions["Selector"]["anyOf"]
        .as_array()
        .expect("a selector is a union of forms");
    assert!(
        forms.contains(&serde_json::json!({ "$ref": "#/$defs/MatchAll" })),
        "the selector must offer `true`",
    );
}

#[test]
fn every_selector_form_carries_the_title_its_model_is_named_after() {
    // The forms are inlined in the selector union, so a generated model takes its
    // class name from the form's own title; without one it gets a numbered name.
    let definitions = definitions();
    let titles: Vec<&str> = definitions["Selector"]["anyOf"]
        .as_array()
        .expect("a selector is a union of forms")
        .iter()
        .filter_map(|form| form["title"].as_str())
        .collect();
    for title in [
        "TagSelector",
        "AnySelector",
        "AllSelector",
        "NotSelector",
        "IdsSelector",
        "RoleSelector",
        "KindSelector",
    ] {
        assert!(titles.contains(&title), "no `{title}` among {titles:?}");
    }
}

#[test]
fn every_selector_form_the_schema_advertises_parses() {
    // Each form is one key, apart from the bare `true`; a selector carries exactly
    // one.
    let forms = [
        ("true", None),
        (r#"{ "tag": "interview" }"#, Some("tag")),
        (r#"{ "any": [] }"#, Some("any")),
        (r#"{ "all": [] }"#, Some("all")),
        (r#"{ "not": true }"#, Some("not")),
        (r#"{ "ids": ["ana"] }"#, Some("ids")),
        (r#"{ "role": "host" }"#, Some("role")),
        (r#"{ "kind": "person" }"#, Some("kind")),
    ];
    let advertised: Vec<Value> = definitions()["Selector"]["anyOf"]
        .as_array()
        .expect("a selector is a union of forms")
        .clone();
    assert_eq!(
        advertised.len(),
        forms.len(),
        "the schema advertises {} forms and {} parse",
        advertised.len(),
        forms.len(),
    );

    for (form, key) in forms {
        let rule = format!(
            r#"{{ "id": "r", "kind": "lead_time", "who": {form}, "params": {{ "min": "1h" }} }}"#
        );
        assert!(
            opportune_json::parse_rule(&rule).is_ok(),
            "the schema advertises `{form}` but it does not parse",
        );
        let advertised = match key {
            None => advertised.contains(&serde_json::json!({ "$ref": "#/$defs/MatchAll" })),
            Some(key) => advertised
                .iter()
                .any(|form| form["properties"].get(key).is_some()),
        };
        assert!(
            advertised,
            "`{form}` parses but the schema does not offer it"
        );
    }

    // A kind selector's value is a participant kind, so both kinds the wire
    // spells are selectable and each reads back as itself.
    for (wire, expected) in [
        ("person", ParticipantKind::Person),
        ("resource", ParticipantKind::Resource),
    ] {
        let document = format!(
            r#"{{ "id": "r", "kind": "lead_time", "who": {{ "kind": "{wire}" }}, "params": {{ "min": "1h" }} }}"#
        );
        let rule = opportune_json::parse_rule(&document)
            .unwrap_or_else(|err| panic!("`kind: {wire}` does not parse: {err}"));
        assert_eq!(rule.who, Selector::kind(expected));
    }
}

#[test]
fn each_rule_variant_is_titled_after_its_kind() {
    let definitions = definitions();
    let variants = definitions["Rule"]["oneOf"]
        .as_array()
        .expect("a tagged rule is a union of variants");
    assert_eq!(variants.len(), RULE_TITLES.len());

    // A variant is either inlined and titled in the union, or lifted to a
    // definition the union references, which carries the same title and the kind
    // it is tagged with. Either way a generated model takes its class name from
    // the title and one kind has one shape.
    let variants: BTreeSet<(&str, &str)> = variants
        .iter()
        .map(|variant| {
            let definition = variant["$ref"]
                .as_str()
                .and_then(|reference| reference.strip_prefix("#/$defs/"))
                .map(|name| &definitions[name]);
            let shape = definition.unwrap_or(variant);
            let kind = shape["properties"]["kind"]["const"]
                .as_str()
                .expect("a variant is tagged with its kind");
            (kind, shape["title"].as_str().expect("a variant is titled"))
        })
        .collect();
    assert_eq!(variants, RULE_TITLES.into_iter().collect());
}

/// The `window` variant is a definition of its own, so the working-hours outcome
/// can name one shape rather than the seven-way union, and the union points at
/// that same definition rather than restating it.
#[test]
fn the_window_rule_is_one_definition_the_union_points_at() {
    let definitions = definitions();
    let window = &definitions["WindowRule"];
    assert_eq!(window["title"], "WindowRule");
    assert_eq!(window["properties"]["kind"]["const"], "window");
    assert_eq!(window["additionalProperties"], Value::Bool(false));
    assert!(
        definitions["Rule"]["oneOf"]
            .as_array()
            .expect("a tagged rule is a union of variants")
            .contains(&serde_json::json!({ "$ref": "#/$defs/WindowRule" })),
        "the union restates the window variant instead of referencing its definition",
    );
}

/// Every field the specification defaults states that a document may write `null`
/// for it, because that is what the deserialisers read: `wire.rs`'s `# Nulls` says
/// an explicit `null` is the same document as an omission, and every field that
/// defaults carries the helper implementing that beside the field's own shape. The
/// same holds for the fields that are `Option`s rather than defaulted, whose serde
/// visitor answers a `null` before the inner type is read, so the set this test
/// walks is the wider one: every property a producer may leave out.
///
/// The field set is derived from the bundle rather than listed here, and from
/// `required` rather than from `default`: a property its parent does not name is
/// one schemars found a `#[serde(default)]` or an `Option` behind, which is what
/// makes a field one a producer may omit. The `default` keyword cannot carry that
/// meaning, because schemars leaves it out when a `skip_serializing_if` predicate
/// matches the default value — so a field that gained `skip_serializing_if =
/// "Vec::is_empty"` beside its `deserialize_with` would leave a `default`-derived
/// set, and take this gate with it, while the parser went on reading its `null`.
/// A property added to that set without a null arm fails this test, which is the
/// drift the two generated surfaces used to resolve in opposite directions.
#[test]
fn every_field_the_specification_defaults_admits_an_explicit_null() {
    let mut omittable = Vec::new();
    properties(
        &Value::Object(definitions()),
        "#/$defs",
        &mut |pointer, property, required| {
            if !required {
                omittable.push((pointer.to_owned(), property.clone()));
            }
        },
    );
    assert!(
        omittable.len() > 50,
        "only {} omittable properties were found, which cannot be the whole wire",
        omittable.len(),
    );

    let silent: Vec<&str> = omittable
        .iter()
        .filter(|(_, property)| !admits_null(property))
        .map(|(pointer, _)| pointer.as_str())
        .collect();
    assert!(
        silent.is_empty(),
        "these fields may be omitted but state no `null` arm, so a document the \
         parser answers is one the published bundle rejects: {silent:#?}",
    );
}

#[test]
fn the_rule_union_names_its_discriminator() {
    // A model generator reads `discriminator` and builds one tagged union, which is
    // what keeps a bad value inside one rule kind a single field error rather than
    // one error per kind. A validator ignores the keyword, so it never constrains
    // what a document may hold.
    let definitions = definitions();
    assert_eq!(
        definitions["Rule"]["discriminator"],
        serde_json::json!({ "propertyName": "kind" }),
    );
    // The property it names is the one every variant is tagged with, whether the
    // variant is inlined or lifted to a definition the union references.
    for variant in definitions["Rule"]["oneOf"].as_array().unwrap() {
        let shape = variant["$ref"]
            .as_str()
            .and_then(|reference| reference.strip_prefix("#/$defs/"))
            .map_or(variant, |name| &definitions[name]);
        assert!(
            shape["properties"]["kind"]["const"].is_string(),
            "a variant without `kind` cannot be told apart: {variant}",
        );
    }
}

/// A block's status is whatever the provider wrote, so the bundle advertises an
/// open string: a model generated from it must accept the vocabulary a real
/// calendar sends instead of rejecting the document at the model boundary. A
/// status-policy entry is the caller's own configuration and stays closed, because
/// reading a typo there as `busy` would turn an entry meant to ignore one kind of
/// event into one that ignores every busy block.
#[test]
fn a_block_status_is_the_open_string_a_provider_sends() {
    let definitions = definitions();
    let advertised = shape(&definitions["Block"]["properties"]["status"]);
    assert_eq!(advertised["type"], "string");
    assert!(
        advertised.get("enum").is_none(),
        "the bundle still closes a provider's status: {advertised}",
    );

    assert_eq!(
        shape(&definitions["StatusPolicyEntry"]["properties"]["status"])["$ref"],
        "#/$defs/Status",
    );
    let statuses: Vec<&str> = definitions["Status"]["oneOf"]
        .as_array()
        .expect("a closed status set is a union of constants")
        .iter()
        .filter_map(|form| form["const"].as_str())
        .collect();
    assert_eq!(statuses, ["free", "tentative", "busy"]);

    // A provider status the bundle does not enumerate still parses, fail-closed
    // as `busy`.
    let request = parse_request(
        r#"{
          "now": "2027-03-09T15:00Z",
          "goal": {
            "parts": [{ "duration": "30m", "requirement": { "all": [{ "person": "ana" }] } }],
            "search_window": ["2027-03-09T15:00Z", "2027-03-10T15:00Z"],
            "zone": "UTC"
          },
          "participants": [{
            "id": "ana",
            "zone": "UTC",
            "calendar": [{ "span": ["2027-03-09T15:00Z", "2027-03-09T16:00Z"], "status": "out_of_office" }]
          }]
        }"#,
    )
    .expect("a provider status still parses");
    assert_eq!(
        request.participants[0].calendar.as_ref().unwrap()[0].status,
        BlockStatus::Busy,
    );
}

/// The parser fails safe, so what the crate writes is the coerced status whatever
/// the provider sent. The open schema admits it, and the written document is a
/// fixed point of the round trip.
#[test]
fn a_written_document_carries_the_status_the_engine_coerced() {
    let request = parse_request(
        r#"{
          "now": "2027-03-09T15:00Z",
          "goal": {
            "parts": [{ "duration": "30m", "requirement": { "all": [{ "person": "ana" }] } }],
            "search_window": ["2027-03-09T15:00Z", "2027-03-09T16:00Z"],
            "zone": "UTC"
          },
          "participants": [{
            "id": "ana",
            "zone": "UTC",
            "calendar": [{
              "span": ["2027-03-09T15:00Z", "2027-03-09T16:00Z"],
              "status": "custom_provider_status"
            }]
          }]
        }"#,
    )
    .expect("a provider status still parses");
    let written = to_json(&WireRequest::from(&request)).expect("the request writes");
    let value: Value = serde_json::from_str(&written).expect("the wire writes JSON");
    let status = &value["participants"][0]["calendar"][0]["status"];
    assert_eq!(
        status, "busy",
        "the coerced status is what the crate writes back",
    );
    assert_eq!(
        to_json(&WireRequest::from(&parse_request(&written).unwrap())).unwrap(),
        written,
        "what the crate wrote is a fixed point of the round trip",
    );
}

#[test]
fn request_weights_publish_signed_32_bit_integer_bounds() {
    let definitions = definitions();
    let weights = shape(&definitions["Request"]["properties"]["weights"]);
    let value_schema = &weights["additionalProperties"];
    assert_eq!(value_schema["type"], "integer");
    assert_eq!(value_schema["format"], "int32");
    assert_eq!(value_schema["minimum"], i64::from(i32::MIN));
    assert_eq!(value_schema["maximum"], i64::from(i32::MAX));
}

/// The inclusive bounds each wire integer `format` implies, as the literals a
/// generated model must enforce.
fn expected_bounds(format: &str) -> Option<(Value, Value)> {
    match format {
        "int32" => Some((
            Value::from(-2_147_483_648_i64),
            Value::from(2_147_483_647_u64),
        )),
        "uint32" => Some((Value::from(0_i64), Value::from(4_294_967_295_u64))),
        "uint64" => Some((
            Value::from(0_i64),
            Value::from(18_446_744_073_709_551_615_u64),
        )),
        _ => None,
    }
}

/// Every integer the wire carries states the full range its `format` names, not
/// only the `minimum` schemars derives for an unsigned field.
///
/// The gate walks the bundle rather than a list of fields, so an `int32`,
/// `uint32` or `uint64` added tomorrow cannot arrive with an implied bound: a
/// generated model that states only `minimum: 0` accepts a count the parser
/// refuses, which is what made `uint64` fields a boundary failure rather than a
/// document failure.
#[test]
fn every_wire_integer_publishes_the_range_its_format_names() {
    let mut seen = 0;
    walk(&Value::Object(definitions()), &mut |value| {
        let Some(object) = value.as_object() else {
            return;
        };
        let integer = match object.get("type") {
            Some(Value::String(kind)) => kind == "integer",
            // An optional integer states `"type": ["integer", "null"]`.
            Some(Value::Array(kinds)) => kinds.iter().any(|kind| kind.as_str() == Some("integer")),
            _ => false,
        };
        if !integer {
            return;
        }
        let Some(format) = object.get("format").and_then(Value::as_str) else {
            return;
        };
        let Some((minimum, maximum)) = expected_bounds(format) else {
            return;
        };
        assert_eq!(
            object.get("minimum"),
            Some(&minimum),
            "the {format} minimum is not the one the format implies: {object:?}",
        );
        assert_eq!(
            object.get("maximum"),
            Some(&maximum),
            "the {format} maximum is not the one the format implies: {object:?}",
        );
        seen += 1;
    });
    assert!(
        seen >= 20,
        "only {seen} bounded integers were found, which cannot be the whole wire",
    );
}

/// The three widths the wire actually uses, pinned at the fields a caller writes
/// them into, so a regression names the field rather than a count.
#[test]
fn the_three_integer_widths_state_their_bounds_where_the_fields_are() {
    let definitions = definitions();
    for (name, field) in [
        ("Limits", "max_search_window_days"),
        ("AtLeastTerm", "at_least"),
        ("CapParams", "limit"),
        ("Meta", "schema"),
    ] {
        let property = shape(&definitions[name]["properties"][field]);
        assert_eq!(property["format"], "uint32", "{name}.{field}");
        assert_eq!(property["minimum"], 0, "{name}.{field}");
        assert_eq!(property["maximum"], 4_294_967_295_u64, "{name}.{field}");
    }
    for (name, field) in [
        ("Limits", "max_participants"),
        ("Request", "limit"),
        ("Explain", "limit"),
        ("DateCount", "offers"),
    ] {
        let property = shape(&definitions[name]["properties"][field]);
        assert_eq!(property["format"], "uint64", "{name}.{field}");
        assert_eq!(property["minimum"], 0, "{name}.{field}");
        assert_eq!(
            property["maximum"], 18_446_744_073_709_551_615_u64,
            "{name}.{field}",
        );
    }
    let weights = shape(&definitions["Request"]["properties"]["weights"]);
    assert_eq!(weights["additionalProperties"]["format"], "int32");
    assert_eq!(
        weights["additionalProperties"]["minimum"],
        i64::from(i32::MIN)
    );
    assert_eq!(
        weights["additionalProperties"]["maximum"],
        i64::from(i32::MAX)
    );
}
