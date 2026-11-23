//! Writes the combined JSON Schema bundle the binding models are generated from.
//!
//! Run it from the workspace root with
//! `cargo run -p opportune-testkit --bin export_schema > schema.json`. The bundle
//! is the wire layer's schema with the adapter envelopes added: one document, so a
//! binding generates one model set, and a shape both layers share — a block, a
//! rule, their spans and selectors — has exactly one definition.
//!
//! The document is written with sorted keys, so an unchanged tree produces the
//! same bytes and a schema change shows up as a diff. `make codegen` feeds the
//! output to `datamodel-codegen`; `make check-codegen` regenerates the models and
//! fails on any difference from the committed ones.
//!
//! `--without-descriptions` writes the same document with the prose dropped. No
//! recipe in the repository passes it: a field's prose is part of the `OpenAPI`
//! document an application serves, so `make codegen` keeps it and `_models.py` is
//! exempted from the project's column gate instead of being generated without the
//! descriptions that breach it.

use std::io::{self, Write};
use std::process::ExitCode;

use serde_json::Value;

/// The command-line flag that drops the schema's prose descriptions.
const WITHOUT_DESCRIPTIONS: &str = "--without-descriptions";

/// The combined schema bundle: the wire layer's definitions plus the adapter
/// envelopes.
///
/// The adapters own the merge (`opportune_adapters::json::schema`), because they
/// are the layer that knows which shapes it shares with the wire layer and must
/// not restate.
fn combined() -> Value {
    opportune_adapters::json::schema()
}

/// The keys whose value maps a *name* to a schema rather than being a schema, so a
/// `description` among their entries is a field or definition called `description`
/// and not prose to drop.
const NAME_MAPS: [&str; 4] = [
    "$defs",
    "properties",
    "patternProperties",
    "dependentSchemas",
];

/// `value` without the `description` every schema object may carry.
fn without_descriptions(value: &mut Value) {
    match value {
        Value::Object(object) => {
            object.remove("description");
            for (key, inner) in object.iter_mut() {
                match inner {
                    Value::Object(schemas) if NAME_MAPS.contains(&key.as_str()) => {
                        for schema in schemas.values_mut() {
                            without_descriptions(schema);
                        }
                    }
                    other => without_descriptions(other),
                }
            }
        }
        Value::Array(items) => items.iter_mut().for_each(without_descriptions),
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
    }
}

/// The bundle as the text the model generator reads.
fn render(descriptions: bool) -> String {
    let mut bundle = combined();
    if !descriptions {
        without_descriptions(&mut bundle);
    }
    let mut text = serde_json::to_string_pretty(&bundle)
        .expect("a schema bundle is a JSON value, and a JSON value always serialises");
    text.push('\n');
    text
}

fn main() -> ExitCode {
    let mut arguments = std::env::args().skip(1);
    let descriptions = match arguments.next() {
        None => true,
        Some(argument) if argument == WITHOUT_DESCRIPTIONS => false,
        Some(argument) => {
            eprintln!(
                "export_schema: `{argument}` is not an option; the options are none, or \
                 `{WITHOUT_DESCRIPTIONS}`"
            );
            return ExitCode::from(2);
        }
    };
    if let Some(argument) = arguments.next() {
        eprintln!("export_schema: `{argument}` is one argument too many");
        return ExitCode::from(2);
    }

    let text = render(descriptions);
    match io::stdout().write_all(text.as_bytes()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("export_schema: cannot write the bundle: {error}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use serde_json::Value;

    use super::{combined, render};

    /// The names `$ref` targets, so a rename that misses a reference fails here
    /// rather than in the generator.
    fn references(value: &Value, found: &mut BTreeSet<String>) {
        match value {
            Value::Object(object) => {
                for (key, inner) in object {
                    if let Some(reference) = inner.as_str().filter(|_| key == "$ref") {
                        found.insert(reference.to_owned());
                    }
                    references(inner, found);
                }
            }
            Value::Array(items) => items.iter().for_each(|item| references(item, found)),
            Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
        }
    }

    /// Every `description` in the document, wherever it sits.
    fn descriptions(value: &Value, found: &mut Vec<String>) {
        match value {
            Value::Object(object) => {
                if let Some(Value::String(description)) = object.get("description") {
                    found.push(description.clone());
                }
                for inner in object.values() {
                    descriptions(inner, found);
                }
            }
            Value::Array(items) => items.iter().for_each(|item| descriptions(item, found)),
            Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
        }
    }

    #[test]
    fn two_calls_render_the_same_bytes() {
        assert_eq!(render(true), render(true));
        assert_eq!(render(false), render(false));
    }

    #[test]
    fn the_bundle_is_a_draft_2020_12_document() {
        let bundle = combined();
        assert_eq!(
            bundle["$schema"],
            "https://json-schema.org/draft/2020-12/schema"
        );
    }

    #[test]
    fn the_bundle_carries_the_engine_and_adapter_shapes() {
        let bundle = combined();
        let definitions = bundle["$defs"].as_object().expect("the bundle has $defs");
        for name in [
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
            "AnnotatedBlock",
            "CalendarOutcome",
            "AdapterDiagnostic",
            "GraphWorkingHoursOutcome",
        ] {
            assert!(definitions.contains_key(name), "the bundle has no `{name}`");
        }
    }

    #[test]
    fn the_outcome_unions_title_their_variants() {
        // A generated model takes a class per outcome from the variant titles: the
        // variants are inlined in the tagged union and have no definition to be
        // named after.
        let bundle = combined();
        for (union, titles) in [
            (
                "CalendarOutcome",
                ["AvailableCalendarOutcome", "UnavailableCalendarOutcome"],
            ),
            (
                "GraphWorkingHoursOutcome",
                [
                    "AvailableWorkingHoursOutcome",
                    "UnavailableWorkingHoursOutcome",
                ],
            ),
        ] {
            let found: Vec<&str> = bundle["$defs"][union]["oneOf"]
                .as_array()
                .unwrap_or_else(|| panic!("`{union}` has no variants"))
                .iter()
                .filter_map(|variant| variant["title"].as_str())
                .collect();
            for title in titles {
                assert!(
                    found.contains(&title),
                    "no `{title}` in `{union}`: {found:?}"
                );
            }
        }
    }

    #[test]
    fn a_shared_shape_keeps_exactly_one_name() {
        // The adapter bundle strips its own `Wire` prefix and reuses the wire
        // layer's definition for anything it shares, so a definition still
        // carrying the prefix is one the merge restated rather than reused.
        let bundle = combined();
        let stray: Vec<&String> = bundle["$defs"]
            .as_object()
            .expect("the bundle has $defs")
            .keys()
            .filter(|name| name.starts_with("Wire"))
            .collect();
        assert!(stray.is_empty(), "definitions not renamed: {stray:?}");
    }

    #[test]
    fn every_reference_names_a_definition() {
        let bundle = combined();
        let definitions = bundle["$defs"].as_object().expect("the bundle has $defs");
        let mut found = BTreeSet::new();
        references(&bundle, &mut found);
        for reference in found {
            let name = reference.strip_prefix("#/$defs/").unwrap_or_else(|| {
                panic!("`{reference}` is not a reference into the bundle's definitions")
            });
            assert!(
                definitions.contains_key(name),
                "`{reference}` names no definition"
            );
        }
    }

    #[test]
    fn the_match_all_selector_is_the_constant_true() {
        let bundle = combined();
        assert_eq!(
            bundle["$defs"]["MatchAll"],
            serde_json::json!({ "const": true })
        );
    }

    #[test]
    fn every_rule_variant_is_titled_after_its_kind() {
        let bundle = combined();
        let definitions = bundle["$defs"].as_object().expect("the bundle has $defs");
        // A variant is inlined and titled in the union, or lifted to a definition
        // the union references; either way one kind has one titled shape.
        let shapes: Vec<&serde_json::Value> = bundle["$defs"]["Rule"]["oneOf"]
            .as_array()
            .expect("the rule union has variants")
            .iter()
            .map(|variant| {
                variant["$ref"]
                    .as_str()
                    .and_then(|reference| reference.strip_prefix("#/$defs/"))
                    .map_or(variant, |name| &definitions[name])
            })
            .collect();
        let titles: Vec<&str> = shapes
            .iter()
            .filter_map(|shape| shape["title"].as_str())
            .collect();
        for title in [
            "WindowRule",
            "BlackoutRule",
            "ClearanceRule",
            "LeadTimeRule",
            "StatusPolicyRule",
            "RestRule",
            "CapRule",
        ] {
            assert!(titles.contains(&title), "no `{title}` among {titles:?}");
        }
    }

    #[test]
    fn the_prose_free_render_drops_every_description_the_published_one_keeps_them() {
        let mut prose = Vec::new();
        descriptions(&combined(), &mut prose);
        assert!(
            prose.iter().any(|text| text.len() > 100),
            "the bundle has no long description, so the drop is untested"
        );

        let mut dropped = Vec::new();
        descriptions(&serde_json::from_str(&render(false)).unwrap(), &mut dropped);
        assert!(dropped.is_empty(), "descriptions survived: {dropped:?}");
    }

    #[test]
    fn a_name_a_schema_map_keys_is_not_mistaken_for_prose() {
        // `properties` and `$defs` map a *name* to a schema, so a `description` key
        // among them is a field or definition called `description`, not the prose
        // keyword. Dropping it would remove that field from every generated model
        // while the bundle the wire layer exports still carries it — silently, since
        // the models are generated from the stripped form.
        let schema = serde_json::json!({
            "type": "object",
            "$defs": {
                "description": { "type": "string", "description": "what it is" },
            },
            "properties": {
                "description": { "type": "string", "description": "what it is" },
            },
            "required": ["description"],
        });
        let mut value = schema.clone();
        super::without_descriptions(&mut value);
        assert_eq!(
            value,
            serde_json::json!({
                "type": "object",
                "$defs": { "description": { "type": "string" } },
                "properties": { "description": { "type": "string" } },
                "required": ["description"],
            }),
        );
    }

    #[test]
    fn dropping_descriptions_changes_nothing_else() {
        let full: Value = serde_json::from_str(&render(true)).unwrap();
        let lean: Value = serde_json::from_str(&render(false)).unwrap();
        let mut stripped = full;
        super::without_descriptions(&mut stripped);
        assert_eq!(stripped, lean);
    }
}
