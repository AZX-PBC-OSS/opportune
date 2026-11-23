//! The JSON Schema bundle generated from the wire shapes.
//!
//! One document carries the whole wire format as named definitions, so a language
//! binding can generate one model set from one file: the request a caller sends,
//! the response it reads, and the pieces it stores on their own — a rule, a
//! calendar, a goal, the limits — all reachable from `$defs` by name.
//!
//! The bundle is generated from the same derives serde reads, so it cannot drift
//! from the format. It is then normalised in five ways, each of which states
//! something the generated schema leaves implicit:
//!
//! - every definition is named for its wire type without the `Wire` prefix, and
//!   titled with that name, because a generated model takes its class name from
//!   the title;
//! - every request and input object type states that it rejects a key it does not
//!   name, and every response and output object type is left open. The input types
//!   say so through serde's `deny_unknown_fields`, so a typo in a request is a
//!   parse error; the response types have no such attribute, so a service reading a
//!   newer engine's additive field still deserializes, and a generated model built
//!   from the bundle must tolerate that field too. Closing the inputs and leaving
//!   the outputs open is what makes a rolling deploy safe: an older binding can
//!   still read a newer engine's answer without the schema rejecting it;
//! - every `int32`, `uint32` and `uint64` integer states the range its `format`
//!   implies. The `format` already names the width, but an integer without
//!   `minimum` and `maximum` generates a model that accepts a value the reader
//!   refuses, so the range is stated beside the format rather than left implicit
//!   in it;
//! - each `Rule` variant is titled after its `kind` (`WindowRule` to `CapRule`),
//!   since the variants are inlined in the tagged union and have no definition of
//!   their own to be named after, and each states the same closure: serde's
//!   internally tagged enums cannot carry `deny_unknown_fields`, so schemars reads
//!   the flag from the enum and would leave the variant unstated even though the
//!   variant struct rejects an unknown key at parse time. The `window` variant is
//!   then lifted to a definition the union references, so an outcome that is always
//!   a `window` rule can name one shape rather than seven;
//! - the `Rule` union names `kind` as its discriminator. That keyword is an
//!   `OpenAPI` annotation rather than a JSON Schema one, so a validator ignores it,
//!   but a model generator uses it to build one tagged union instead of trying
//!   every variant in turn — which is what turns a bad value inside one kind into
//!   a single field error rather than one error per kind.
//!
//! Two calls return byte-identical documents: every map is ordered, and nothing
//! in the bundle depends on the order the definitions were reached.
//!
//! What a field accepts is stated at the field: the null arm a defaulted field
//! carries comes from [`null_as_default`](crate::wire) beside the deserialiser that
//! reads it, not from a list kept here, so a field that gains or loses null
//! tolerance cannot leave the bundle behind.

use schemars::SchemaGenerator;
use schemars::generate::SchemaSettings;
use serde_json::{Map, Value};

use crate::wire::{
    WireBlock, WireCalendarSpec, WireDiagnostics, WireGoal, WireLimits, WireProposal,
    WireRangesQuery, WireRequest, WireRequiredRanges, WireResponse, WireRule, WireSlotCheck,
};

/// The JSON Schema dialect the bundle is written in.
const DIALECT: &str = "https://json-schema.org/draft/2020-12/schema";

/// The prefix every internal `$ref` in the bundle carries.
const DEFINITIONS: &str = "#/$defs/";

/// The prefix every wire type's name carries, which the bundle drops.
const WIRE: &str = "Wire";

/// The definition that is the constant `true` rather than an object, and so has
/// no title to take.
const MATCH_ALL: &str = "MatchAll";

/// The title each `Rule` variant takes, from the `kind` it is tagged with.
const RULE_TITLES: [(&str, &str); 7] = [
    ("window", "WindowRule"),
    ("blackout", "BlackoutRule"),
    ("clearance", "ClearanceRule"),
    ("lead_time", "LeadTimeRule"),
    ("status_policy", "StatusPolicyRule"),
    ("rest", "RestRule"),
    ("cap", "CapRule"),
];

/// The response and output definitions, which stay open.
///
/// Serde reads these types without `deny_unknown_fields`, so an older client
/// tolerates a field a newer engine added. Stating `additionalProperties: false`
/// on them would make a generated model reject the answer the reader accepts, and
/// would turn an additive engine release into a breaking one. Every definition
/// not named here is a request or input shape, which stays closed.
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

/// The JSON Schema of the wire format, as one Draft 2020-12 document.
///
/// Every definition is named and titled after the wire type it describes, with
/// the `Wire` prefix dropped: the request and its pieces, the response and its
/// pieces, and the values a caller stores on their own. The `Rule` union inlines
/// its variants and titles each after its kind; the `window` variant is also
/// lifted to a `WindowRule` definition the union references, because a
/// working-hours conversion always yields one and the adapters' schema names it
/// rather than the seven-way union.
/// [`MatchAll`](crate::MatchAll) is the constant `true`; every request and input
/// object type rejects a key it does not name, while every response and output
/// object type is left open so an older client accepts a field a newer engine
/// added; and a block's `status` is any string — a provider's own vocabulary,
/// where an unrecognized value reads as `busy`. Every `int32`,
/// `uint32` and `uint64` integer states the range its `format` implies, so a
/// generated model refuses the value serde refuses. Every field with a
/// wire format default carries a `null` arm beside it — and a `default` too,
/// unless the document omits the field whenever it holds that default — because
/// an explicit `null` is how a binding that writes every key leaves such a field
/// out.
///
/// ```
/// use opportune_json::schema;
///
/// let bundle = schema();
/// assert_eq!(bundle["$schema"], "https://json-schema.org/draft/2020-12/schema");
/// assert_eq!(bundle["$defs"]["MatchAll"], serde_json::json!({ "const": true }));
///
/// // A request is closed and a response is open, which is what lets an older
/// // client read a newer engine's additive field without the schema rejecting it.
/// assert_eq!(bundle["$defs"]["Request"]["additionalProperties"], false);
/// assert!(bundle["$defs"]["Response"].get("additionalProperties").is_none());
///
/// // The variants of a rule are titled after their kinds, which a generated model
/// // takes its class names from. The `window` variant is lifted to a definition of
/// // its own, so a shape that is always a `window` rule names `WindowRule` rather
/// // than the union.
/// let titles: Vec<&str> = bundle["$defs"]["Rule"]["oneOf"]
///     .as_array()
///     .unwrap()
///     .iter()
///     .filter_map(|variant| variant["title"].as_str())
///     .collect();
/// assert!(titles.iter().any(|title| *title == "BlackoutRule"), "{titles:?}");
/// assert_eq!(bundle["$defs"]["WindowRule"]["title"], "WindowRule");
/// assert_eq!(
///     bundle["$defs"]["WindowRule"]["properties"]["kind"]["const"],
///     "window",
/// );
/// ```
///
/// # Panics
/// Panics when the wire types reach a rule kind this module has no title for,
/// which means a rule kind was added without the title a generated model takes
/// its class name from, or when the tagged rule union is missing entirely or has
/// no `window` variant to lift. The bundle also panics if two wire types would
/// take the same name, because one of them would silently replace the other.
#[must_use]
pub fn schema() -> Value {
    let mut generator = SchemaSettings::draft2020_12().into_generator();
    register_definitions(&mut generator);
    let definitions = definitions(generator.take_definitions(false));
    let mut document = Map::new();
    document.insert("$schema".to_owned(), Value::from(DIALECT));
    document.insert("$defs".to_owned(), Value::Object(definitions));
    Value::Object(document)
}

/// Registers the schema of every entry point, and of everything each of them
/// reaches, in `generator`.
fn register_definitions(generator: &mut SchemaGenerator) {
    generator.subschema_for::<WireRequest>();
    generator.subschema_for::<WireResponse>();
    generator.subschema_for::<WireProposal>();
    generator.subschema_for::<WireSlotCheck>();
    generator.subschema_for::<WireRangesQuery>();
    generator.subschema_for::<WireRequiredRanges>();
    generator.subschema_for::<WireDiagnostics>();
    generator.subschema_for::<WireRule>();
    generator.subschema_for::<WireCalendarSpec>();
    generator.subschema_for::<WireGoal>();
    generator.subschema_for::<WireLimits>();
    generator.subschema_for::<WireBlock>();
}

/// The generated definitions, normalised into the names and titles a generated
/// model takes.
///
/// Request and input objects are closed, because serde rejects a key they do not
/// name; [`OPEN_DEFINITIONS`] are left open, because serde reads those types with
/// no `deny_unknown_fields`, so a generated model must tolerate the same additive
/// field the reader tolerates.
fn definitions(raw: Map<String, Value>) -> Map<String, Value> {
    let mut definitions = Map::new();
    for (wire_name, mut schema) in raw {
        let name = wire_name.strip_prefix(WIRE).unwrap_or(&wire_name);
        retarget(&mut schema, |raw_name| {
            raw_name.strip_prefix(WIRE).unwrap_or(raw_name).to_owned()
        });
        if !OPEN_DEFINITIONS.contains(&name) {
            close_object(&mut schema);
        }
        bound_integers(&mut schema);
        if name != MATCH_ALL {
            schema["title"] = Value::from(name);
        }
        let replaced = definitions.insert(name.to_owned(), schema);
        assert!(
            replaced.is_none(),
            "two wire types are named `{name}` once the `Wire` prefix is dropped",
        );
    }
    name_rule_variants(&mut definitions);
    lift_rule_variant(&mut definitions, "window", "WindowRule");
    discriminate(&mut definitions, "Rule", "kind");
    definitions
}

/// Publishes explicit `minimum`/`maximum` bounds on every integer schema whose
/// `format` names a fixed width, so a generated model enforces the same range
/// serde does.
///
/// schemars states the width in `format` and, for an unsigned field, the `0`
/// minimum; the upper bound is implied by the format and nothing else states it.
/// `int32` reads signed, `uint32` and `uint64` unsigned. Widths this format never
/// uses are left alone rather than guessed at.
fn bound_integers(value: &mut Value) {
    match value {
        Value::Object(object) => {
            let integer = match object.get("type") {
                Some(Value::String(kind)) => kind == "integer",
                // An `Option<u32>` field states `"type": ["integer", "null"]`, so
                // the arm that admits a number is still an integer schema.
                Some(Value::Array(kinds)) => {
                    kinds.iter().any(|kind| kind.as_str() == Some("integer"))
                }
                _ => false,
            };
            let bounds = match object.get("format").and_then(Value::as_str) {
                Some(format) if integer => integer_bounds(format),
                _ => None,
            };
            if let Some((minimum, maximum)) = bounds {
                object.entry("minimum".to_owned()).or_insert(minimum);
                object.entry("maximum".to_owned()).or_insert(maximum);
            }
            for inner in object.values_mut() {
                bound_integers(inner);
            }
        }
        Value::Array(items) => items.iter_mut().for_each(bound_integers),
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
    }
}

/// The inclusive bounds a wire integer `format` implies, as the two values the
/// schema states.
fn integer_bounds(format: &str) -> Option<(Value, Value)> {
    match format {
        "int32" => Some((Value::from(i32::MIN), Value::from(i32::MAX))),
        "uint32" => Some((Value::from(0), Value::from(u32::MAX))),
        "uint64" => Some((Value::from(0), Value::from(u64::MAX))),
        _ => None,
    }
}

/// States that the union `name` is discriminated by the property `property`.
///
/// # Panics
/// Panics if `definitions` has no object definition named `name`.
pub fn discriminate(definitions: &mut Map<String, Value>, name: &str, property: &str) {
    let definition = definitions
        .get_mut(name)
        .and_then(Value::as_object_mut)
        .unwrap_or_else(|| panic!("the bundle has no `{name}` union to discriminate"));
    definition.insert(
        "discriminator".to_owned(),
        serde_json::json!({ "propertyName": property }),
    );
}

/// States that an object schema rejects a key it does not name, unless it already
/// says which keys it accepts.
pub fn close_object(schema: &mut Value) {
    let Some(object) = schema.as_object_mut() else {
        return;
    };
    if object.get("properties").is_some_and(Value::is_object)
        && !object.contains_key("additionalProperties")
    {
        object.insert("additionalProperties".to_owned(), Value::Bool(false));
    }
}

/// Points every `$ref` in `value` at the definition name returned by `rename`.
pub fn retarget(value: &mut Value, rename: impl Fn(&str) -> String + Copy) {
    match value {
        Value::Object(object) => {
            for (key, inner) in object {
                match (key.as_str(), inner.as_str()) {
                    ("$ref", Some(reference)) => {
                        if let Some(raw_name) = reference.strip_prefix(DEFINITIONS) {
                            let renamed = rename(raw_name);
                            if renamed != raw_name {
                                *inner = Value::from(format!("{DEFINITIONS}{renamed}"));
                            }
                        }
                    }
                    _ => retarget(inner, rename),
                }
            }
        }
        Value::Array(items) => items.iter_mut().for_each(|item| retarget(item, rename)),
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
    }
}

/// Lifts the `Rule` variant tagged `kind` to a definition named `name`, leaving
/// the union pointing at it.
fn lift_rule_variant(definitions: &mut Map<String, Value>, kind: &str, name: &str) {
    let variants = definitions
        .get_mut("Rule")
        .and_then(|rule| rule.get_mut("oneOf"))
        .and_then(Value::as_array_mut)
        .expect("the bundle has no `Rule` union to lift a variant from");
    let index = variants
        .iter()
        .position(|variant| {
            variant
                .pointer("/properties/kind/const")
                .and_then(Value::as_str)
                == Some(kind)
        })
        .unwrap_or_else(|| panic!("the `Rule` union has no `{kind}` variant to lift"));
    let variant = variants.remove(index);
    variants.insert(
        index,
        serde_json::json!({ "$ref": format!("{DEFINITIONS}{name}") }),
    );
    let replaced = definitions.insert(name.to_owned(), variant);
    assert!(
        replaced.is_none(),
        "lifting `{kind}` to `{name}` would replace an existing definition",
    );
}

/// Titles each `Rule` variant after its kind, and states that it rejects an
/// unknown key.
fn name_rule_variants(definitions: &mut Map<String, Value>) {
    let variants = definitions
        .get_mut("Rule")
        .and_then(|rule| rule.get_mut("oneOf"))
        .and_then(Value::as_array_mut)
        .expect("the bundle has no `Rule` union to title its variants in");
    for variant in variants {
        let kind = variant
            .pointer("/properties/kind/const")
            .and_then(Value::as_str)
            .map(str::to_owned);
        let Some(kind) = kind else { continue };
        let title = RULE_TITLES
            .iter()
            .find_map(|(name, title)| (*name == kind).then_some(*title))
            .unwrap_or_else(|| panic!("the schema has no title for the rule kind `{kind}`"));
        if let Some(object) = variant.as_object_mut() {
            object.insert("title".to_owned(), Value::from(title));
        }
        close_object(variant);
    }
}
