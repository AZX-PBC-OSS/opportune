//! JSON envelopes for language bindings, and the codegen schema over them.
//!
//! A binding crosses its language boundary with JSON only, so the adapter outputs
//! need a wire shape as strict as the engine's: every struct denies an unknown key,
//! nothing flattens another value, and each outcome is a discriminated union on
//! `status`, which generates a clean tagged union in Pydantic or TypeScript rather
//! than an optional-everything class.
//!
//! The envelopes wrap the wire layer's own [`WireBlock`] and [`WireRule`], so a
//! binding generates one model set: a block is the same shape whether it came from
//! a request or from a provider payload, and a working-hours rule is the same shape
//! a request carries.
//!
//! ```
//! # #[cfg(all(feature = "graph", feature = "bundled-tzdb"))] {
//! use opportune_adapters::json::calendar_view_to_blocks;
//!
//! let payload = r#"{
//!   "value": [{
//!     "id": "AAMkAD",
//!     "isAllDay": false,
//!     "showAs": "busy",
//!     "start": { "dateTime": "2027-03-09T14:00:00.0000000", "timeZone": "UTC" },
//!     "end": { "dateTime": "2027-03-09T15:00:00.0000000", "timeZone": "UTC" }
//!   }]
//! }"#;
//!
//! let outcome = calendar_view_to_blocks(payload, "America/New_York", None).unwrap();
//! let value: serde_json::Value = serde_json::from_str(&outcome).unwrap();
//! assert_eq!(value["status"], "available");
//! assert_eq!(value["blocks"][0]["event_id"], "AAMkAD");
//! assert_eq!(value["blocks"][0]["block"]["all_day"], false);
//! # }
//! ```

use opportune::model::Rule;
use opportune_json::{WireBlock, WireRule};
use serde::{Deserialize, Serialize};

// The provider entry points are the only callers of these, so a build with the
// envelopes and no provider compiles them out rather than carrying dead code.
#[cfg(feature = "graph")]
use opportune::tags::Tag;
#[cfg(any(feature = "google", feature = "graph"))]
use opportune::time::{TimeZoneDatabase, ZoneName};
#[cfg(any(feature = "google", feature = "graph"))]
use opportune_json::JsonError;
#[cfg(feature = "graph")]
use opportune_json::join_pointer;
#[cfg(any(feature = "google", feature = "graph"))]
use std::collections::BTreeMap;

#[cfg(any(feature = "google", feature = "graph"))]
use crate::types::AdapterPayloadError;
use crate::types::{AdapterDiagnostic, AnnotatedBlock, CalendarOutcome, WorkingHoursError};

/// One converted block as JSON, with the provider's event ID beside it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct WireAnnotatedBlock {
    /// The provider's ID for the event, or `null` when the source reports spans
    /// rather than events.
    pub event_id: Option<String>,
    /// The block, in the wire layer's own shape.
    pub block: WireBlock,
}

/// One per-event finding as JSON.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct WireAdapterDiagnostic {
    /// The stable, machine-readable code.
    pub code: String,
    /// The provider's ID for the event the finding is about, when there is one.
    pub event_id: Option<String>,
    /// A sentence naming the value and why it was left out.
    pub message: String,
}

/// What one participant's calendar converted into.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub enum WireCalendarOutcome {
    /// The calendar converted.
    Available(AvailableCalendarOutcome),
    /// The calendar could not be converted, so availability is unknown rather
    /// than answered with no blocks.
    Unavailable(UnavailableCalendarOutcome),
}

/// A calendar that converted.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct AvailableCalendarOutcome {
    /// The converted blocks, in provider order.
    pub blocks: Vec<WireAnnotatedBlock>,
    /// Per-event findings that did not stop the conversion.
    pub diagnostics: Vec<WireAdapterDiagnostic>,
}

/// A calendar, or a working-hours conversion, that could not be completed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct UnavailableCalendarOutcome {
    /// A stable, machine-readable code.
    pub code: String,
    /// A sentence naming what could not be converted and why.
    pub message: String,
}

/// One mailbox's working hours, as a rule or as the reason it could not become
/// one.
///
/// Working hours are Graph-only, which is why the generated model is named
/// `GraphWorkingHoursOutcome`. The failure is a value rather than an exception so
/// a caller can fall back to its own window without catching one.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub enum WireWorkingHoursOutcome {
    /// The working hours became a `window` rule.
    Available(AvailableWorkingHoursOutcome),
    /// The working hours could not become a rule.
    Unavailable(UnavailableCalendarOutcome),
}

/// Working hours that became a `window` rule.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct AvailableWorkingHoursOutcome {
    /// The `window` rule, in the wire layer's own shape.
    #[cfg_attr(feature = "schema", schemars(schema_with = "window_rule_schema"))]
    pub rule: WireRule,
}

/// One working-hours rule: the `window` definition, never the seven-way union.
///
/// A conversion only ever produces a `window` rule, so a model generated from the
/// bundle holds that shape instead of a union its reader has to narrow. The
/// reference is to the same `WindowRule` definition the `Rule` union points at, so
/// the bundle carries one spelling of it.
#[cfg(feature = "schema")]
fn window_rule_schema(_generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
    schemars::json_schema!({ "$ref": "#/$defs/WindowRule" })
}

impl From<&AnnotatedBlock> for WireAnnotatedBlock {
    fn from(annotated: &AnnotatedBlock) -> Self {
        Self {
            event_id: annotated.event_id.clone(),
            block: WireBlock::from(&annotated.block),
        }
    }
}

impl From<&AdapterDiagnostic> for WireAdapterDiagnostic {
    fn from(diagnostic: &AdapterDiagnostic) -> Self {
        Self {
            code: diagnostic.code.to_owned(),
            event_id: diagnostic.event_id.clone(),
            message: diagnostic.message.clone(),
        }
    }
}

impl From<&CalendarOutcome> for WireCalendarOutcome {
    fn from(outcome: &CalendarOutcome) -> Self {
        match outcome {
            CalendarOutcome::Available {
                blocks,
                diagnostics,
            } => Self::Available(AvailableCalendarOutcome {
                blocks: blocks.iter().map(WireAnnotatedBlock::from).collect(),
                diagnostics: diagnostics
                    .iter()
                    .map(WireAdapterDiagnostic::from)
                    .collect(),
            }),
            CalendarOutcome::Unavailable { code, message } => {
                Self::Unavailable(UnavailableCalendarOutcome {
                    code: (*code).to_owned(),
                    message: message.clone(),
                })
            }
        }
    }
}

impl From<&Rule> for WireWorkingHoursOutcome {
    fn from(rule: &Rule) -> Self {
        Self::Available(AvailableWorkingHoursOutcome {
            rule: WireRule::from(rule),
        })
    }
}

impl From<&WorkingHoursError> for WireWorkingHoursOutcome {
    fn from(error: &WorkingHoursError) -> Self {
        Self::Unavailable(UnavailableCalendarOutcome {
            code: error.code().to_owned(),
            message: error.to_string(),
        })
    }
}

/// Converts a `calendarView` response into the JSON [`WireCalendarOutcome`],
/// resolving zone names against `db`.
///
/// This is the conversion behind `calendar_view_to_blocks`: a caller that holds
/// its own time zone database — an engine built from a `tzdb_dir`, say — passes
/// it here so the payload is read in the same database the engine later
/// evaluates against. `participant_zone` is the mailbox owner's zone, needed to
/// resolve all-day events; `category_map_json` is an optional JSON object mapping
/// a category name to a tag path.
///
/// ```
/// # #[cfg(all(feature = "graph", feature = "bundled-tzdb"))] {
/// use opportune::time::TimeZoneDatabase;
/// use opportune_adapters::json::calendar_view_to_blocks_with_db;
///
/// let payload = r#"{ "value": [{ "id": "AAMkAD", "isAllDay": false, "showAs": "free",
///   "start": { "dateTime": "2027-03-09T09:00:00.0000000", "timeZone": "UTC" },
///   "end": { "dateTime": "2027-03-09T10:00:00.0000000", "timeZone": "UTC" } }] }"#;
/// let db = TimeZoneDatabase::bundled().unwrap();
/// let outcome: serde_json::Value =
///     serde_json::from_str(&calendar_view_to_blocks_with_db(&db, payload, "UTC", None).unwrap())
///         .unwrap();
/// assert_eq!(outcome["status"], "available");
/// # }
/// ```
///
/// # Errors
/// Returns [`JsonError`] when `participant_zone` is blank, when
/// `category_map_json` is not a JSON object of strings, or when a mapped tag is
/// not a tag path. A payload that cannot be read is an `unavailable` outcome, not
/// an error: it is about the calendar, not about the call.
#[cfg(feature = "graph")]
pub fn calendar_view_to_blocks_with_db(
    db: &TimeZoneDatabase,
    payload_json: &str,
    participant_zone: &str,
    category_map_json: Option<&str>,
) -> Result<String, JsonError> {
    let zone = zone_name(participant_zone)?;
    let category_map = category_map(category_map_json)?;
    let outcome =
        crate::graph::calendar_view_to_blocks(db, payload_json, &zone, category_map.as_ref());
    opportune_json::to_json(&WireCalendarOutcome::from(&outcome))
}

/// Converts a `calendarView` response into the JSON [`WireCalendarOutcome`].
///
/// `participant_zone` is the mailbox owner's zone, needed to resolve all-day
/// events; `category_map_json` is an optional JSON object mapping a category name
/// to a tag path. Zone names resolve against the bundled database, which this
/// entry point needs (`bundled-tzdb`); a caller with its own passes it to
/// [`calendar_view_to_blocks_with_db`].
///
/// ```
/// # #[cfg(all(feature = "graph", feature = "bundled-tzdb"))] {
/// use opportune_adapters::json::calendar_view_to_blocks;
///
/// let payload = r#"{ "value": [{ "id": "AAMkAD", "isAllDay": false, "showAs": "free",
///   "start": { "dateTime": "2027-03-09T09:00:00.0000000", "timeZone": "UTC" },
///   "end": { "dateTime": "2027-03-09T10:00:00.0000000", "timeZone": "UTC" } }] }"#;
/// let outcome: serde_json::Value =
///     serde_json::from_str(&calendar_view_to_blocks(payload, "UTC", None).unwrap()).unwrap();
/// assert_eq!(outcome["status"], "available");
/// assert_eq!(outcome["blocks"][0]["block"]["status"], "free");
/// # }
/// ```
///
/// # Errors
/// Returns [`JsonError`] when `participant_zone` is blank, when
/// `category_map_json` is not a JSON object of strings, or when a mapped tag is
/// not a tag path. A payload that cannot be read is an `unavailable` outcome, not
/// an error: it is about the calendar, not about the call.
#[cfg(all(feature = "graph", feature = "bundled-tzdb"))]
pub fn calendar_view_to_blocks(
    payload_json: &str,
    participant_zone: &str,
    category_map_json: Option<&str>,
) -> Result<String, JsonError> {
    calendar_view_to_blocks_with_db(
        &bundled()?,
        payload_json,
        participant_zone,
        category_map_json,
    )
}

/// Converts a `getSchedule` response into JSON outcomes keyed by `scheduleId`,
/// resolving zone names against `db`.
///
/// This is the conversion behind `get_schedule_to_blocks`, for a caller that
/// holds its own time zone database.
///
/// # Errors
/// Returns [`JsonError`] with code `payload_invalid` when the document is not a
/// `getSchedule` response this adapter can read. A schedule the provider reported
/// an error for, or one whose omitted `scheduleItems` cannot be read, is an
/// `unavailable` outcome inside the object, not an error.
#[cfg(feature = "graph")]
pub fn get_schedule_to_blocks_with_db(
    db: &TimeZoneDatabase,
    payload_json: &str,
) -> Result<String, JsonError> {
    let outcomes = crate::graph::get_schedule_to_blocks(db, payload_json)
        .map_err(|err| payload_error(&err))?;
    outcomes_to_json(&outcomes)
}

/// Converts a `getSchedule` response into JSON outcomes keyed by `scheduleId`.
///
/// Zone names resolve against the bundled database, which this entry point needs
/// (`bundled-tzdb`); a caller with its own passes it to
/// [`get_schedule_to_blocks_with_db`].
///
/// # Errors
/// Returns [`JsonError`] with code `payload_invalid` when the document is not a
/// `getSchedule` response this adapter can read. A schedule the provider reported
/// an error for, or one whose omitted `scheduleItems` cannot be read, is an
/// `unavailable` outcome inside the object, not an error.
#[cfg(all(feature = "graph", feature = "bundled-tzdb"))]
pub fn get_schedule_to_blocks(payload_json: &str) -> Result<String, JsonError> {
    get_schedule_to_blocks_with_db(&bundled()?, payload_json)
}

/// Converts one mailbox's `workingHours` into the JSON
/// [`WireWorkingHoursOutcome`], resolving zone names against `db`.
///
/// This is the conversion behind `working_hours_to_rule`, for a caller that
/// holds its own time zone database.
///
/// # Errors
/// Returns [`JsonError`] only for a document that cannot be written as JSON. A
/// working-hours payload that cannot become a rule is an `unavailable` outcome,
/// so the caller can fall back to its own window without handling an exception.
#[cfg(feature = "graph")]
pub fn working_hours_to_rule_with_db(
    db: &TimeZoneDatabase,
    payload_json: &str,
    rule_id: &str,
) -> Result<String, JsonError> {
    let outcome = match crate::graph::working_hours_to_rule(db, payload_json, rule_id) {
        Ok(rule) => WireWorkingHoursOutcome::from(&rule),
        Err(err) => WireWorkingHoursOutcome::from(&err),
    };
    opportune_json::to_json(&outcome)
}

/// Converts one mailbox's `workingHours` into the JSON
/// [`WireWorkingHoursOutcome`].
///
/// Zone names resolve against the bundled database, which this entry point needs
/// (`bundled-tzdb`); a caller with its own passes it to
/// [`working_hours_to_rule_with_db`].
///
/// # Errors
/// Returns [`JsonError`] only when the bundled time zone database is unusable. A
/// working-hours payload that cannot become a rule is an `unavailable` outcome, so
/// the caller can fall back to its own window without handling an exception.
#[cfg(all(feature = "graph", feature = "bundled-tzdb"))]
pub fn working_hours_to_rule(payload_json: &str, rule_id: &str) -> Result<String, JsonError> {
    working_hours_to_rule_with_db(&bundled()?, payload_json, rule_id)
}

/// Converts a Google `events.list` response into the JSON
/// [`WireCalendarOutcome`], resolving zone names against `db`.
///
/// This is the conversion behind `events_list_to_blocks`, for a caller that
/// holds its own time zone database. `participant_zone` is the calendar owner's
/// zone, needed when the response names no `timeZone` of its own to read all-day
/// events in.
///
/// # Errors
/// Returns [`JsonError`] when `participant_zone` is blank. A payload that cannot
/// be read is an `unavailable` outcome, not an error: it is about the calendar,
/// not about the call.
#[cfg(feature = "google")]
pub fn events_list_to_blocks_with_db(
    db: &TimeZoneDatabase,
    payload_json: &str,
    participant_zone: &str,
) -> Result<String, JsonError> {
    let zone = zone_name(participant_zone)?;
    let outcome = crate::google::events_list_to_blocks(db, payload_json, &zone);
    opportune_json::to_json(&WireCalendarOutcome::from(&outcome))
}

/// Converts a Google `events.list` response into the JSON
/// [`WireCalendarOutcome`].
///
/// `participant_zone` is the calendar owner's zone, needed when the response names
/// no `timeZone` of its own to read all-day events in. Zone names resolve against
/// the bundled database, which this entry point needs (`bundled-tzdb`); a caller
/// with its own passes it to [`events_list_to_blocks_with_db`].
///
/// ```
/// # #[cfg(all(feature = "google", feature = "bundled-tzdb"))] {
/// use opportune_adapters::json::events_list_to_blocks;
///
/// let payload = r#"{ "timeZone": "UTC", "items": [{ "id": "evt-1", "status": "confirmed",
///   "start": { "dateTime": "2027-03-09T09:00:00Z" },
///   "end": { "dateTime": "2027-03-09T10:00:00Z" } }] }"#;
/// let outcome: serde_json::Value =
///     serde_json::from_str(&events_list_to_blocks(payload, "UTC").unwrap()).unwrap();
/// assert_eq!(outcome["status"], "available");
/// assert_eq!(outcome["blocks"][0]["event_id"], "evt-1");
/// # }
/// ```
///
/// # Errors
/// Returns [`JsonError`] when `participant_zone` is blank. A payload that cannot be
/// read is an `unavailable` outcome, not an error: it is about the calendar, not
/// about the call.
#[cfg(all(feature = "google", feature = "bundled-tzdb"))]
pub fn events_list_to_blocks(
    payload_json: &str,
    participant_zone: &str,
) -> Result<String, JsonError> {
    events_list_to_blocks_with_db(&bundled()?, payload_json, participant_zone)
}

/// Converts a Google `freeBusy.query` response into JSON outcomes keyed by
/// calendar ID.
///
/// # Errors
/// Returns [`JsonError`] with code `payload_invalid` when the document is not a
/// `freeBusy.query` response this adapter can read. A calendar the provider
/// reported an error for is an `unavailable` outcome inside the object, not an
/// error.
#[cfg(feature = "google")]
pub fn free_busy_to_blocks(payload_json: &str) -> Result<String, JsonError> {
    let outcomes =
        crate::google::free_busy_to_blocks(payload_json).map_err(|err| payload_error(&err))?;
    outcomes_to_json(&outcomes)
}

/// The adapter envelopes as one Draft 2020-12 JSON Schema bundle.
///
/// The bundle is the wire layer's own schema with the adapter definitions added,
/// so a binding generates one model set from one file and a type both layers share
/// — [`WireBlock`], [`WireRule`], their spans and selectors — has exactly one
/// definition. Each adapter definition is named and titled after its envelope with
/// the `Wire` prefix dropped, every object rejects a key it does not name, each
/// union titles the variants it inlines so a generated model has one class per
/// outcome, and each union names `status` as its discriminator so the generated
/// union is a tagged one rather than an optional-everything class.
/// [`WireWorkingHoursOutcome`] is titled `GraphWorkingHoursOutcome` because working
/// hours are Graph-only.
///
/// ```
/// use opportune_adapters::json::schema;
///
/// let bundle = schema();
/// assert_eq!(bundle["$schema"], "https://json-schema.org/draft/2020-12/schema");
/// assert!(bundle["$defs"]["CalendarOutcome"].is_object());
/// assert!(bundle["$defs"]["Block"].is_object());
/// ```
///
/// # Panics
/// Panics when the wire layer's bundle carries no `$defs`, which would mean its
/// format changed under this crate, and when a union is missing a variant title,
/// which would mean a variant was added without the name a generated model takes.
#[cfg(feature = "schema")]
#[must_use]
pub fn schema() -> serde_json::Value {
    use opportune_json::schema_support::{close_object, discriminate, retarget};
    use schemars::generate::SchemaSettings;
    use serde_json::Value;

    let mut document = opportune_json::schema();
    let mut definitions = document
        .get_mut("$defs")
        .and_then(Value::as_object_mut)
        .expect("the wire layer's schema carries its definitions in `$defs`")
        .clone();

    let mut generator = SchemaSettings::draft2020_12().into_generator();
    generator.subschema_for::<WireCalendarOutcome>();
    generator.subschema_for::<WireWorkingHoursOutcome>();
    for (wire_name, mut definition) in generator.take_definitions(false) {
        let name = envelope_name(&wire_name);
        retarget(&mut definition, envelope_name);
        close_object(&mut definition);
        close_variants(&mut definition);
        // The wire layer titles every definition after its type; the adapter's own
        // definitions state the same title, because a generated model takes its
        // class name from it.
        if let Some(object) = definition.as_object_mut() {
            object.insert("title".to_owned(), Value::from(name.as_str()));
        }
        // The wire layer owns every shape it already defines, so a shared type is
        // never redefined here with a second spelling.
        definitions.entry(name).or_insert(definition);
    }

    title_variants(&mut definitions, "CalendarOutcome", CALENDAR_VARIANTS);
    title_variants(
        &mut definitions,
        "GraphWorkingHoursOutcome",
        WORKING_HOURS_VARIANTS,
    );
    for (name, property) in DISCRIMINATED {
        discriminate(&mut definitions, name, property);
    }

    document["$defs"] = Value::Object(definitions);
    document
}

/// The definition name an envelope's `Wire` name takes.
#[cfg(feature = "schema")]
fn envelope_name(wire_name: &str) -> String {
    let name = wire_name.strip_prefix("Wire").unwrap_or(wire_name);
    if name == "WorkingHoursOutcome" {
        return "GraphWorkingHoursOutcome".to_owned();
    }
    name.to_owned()
}

/// The title each [`WireCalendarOutcome`] variant takes.
#[cfg(feature = "schema")]
const CALENDAR_VARIANTS: [(&str, &str); 2] = [
    ("available", "AvailableCalendarOutcome"),
    ("unavailable", "UnavailableCalendarOutcome"),
];

/// The title each [`WireWorkingHoursOutcome`] variant takes.
#[cfg(feature = "schema")]
const WORKING_HOURS_VARIANTS: [(&str, &str); 2] = [
    ("available", "AvailableWorkingHoursOutcome"),
    ("unavailable", "UnavailableWorkingHoursOutcome"),
];

/// Closes the variant objects a tagged union inlines.
#[cfg(feature = "schema")]
fn close_variants(schema: &mut serde_json::Value) {
    let Some(variants) = schema
        .get_mut("oneOf")
        .and_then(serde_json::Value::as_array_mut)
    else {
        return;
    };
    for variant in variants {
        opportune_json::schema_support::close_object(variant);
    }
}

/// Titles each variant of the union `name` after the outcome it holds.
#[cfg(feature = "schema")]
fn title_variants(
    definitions: &mut serde_json::Map<String, serde_json::Value>,
    name: &str,
    titles: [(&str, &str); 2],
) {
    let variants = definitions
        .get_mut(name)
        .and_then(|union| union.get_mut("oneOf"))
        .and_then(serde_json::Value::as_array_mut)
        .unwrap_or_else(|| panic!("the bundle has no `{name}` union to title its variants in"));
    for variant in variants {
        let tag = variant
            .pointer("/properties/status/const")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned);
        let Some(tag) = tag else {
            continue;
        };
        let title = titles
            .iter()
            .find_map(|(value, title)| (*value == tag).then_some(*title))
            .unwrap_or_else(|| panic!("the schema has no title for the `{name}` status `{tag}`"));
        if let Some(object) = variant.as_object_mut() {
            object.insert("title".to_owned(), serde_json::Value::from(title));
        }
    }
}

/// The unions whose value decides which variant an object is, and the property
/// that carries it.
///
/// `discriminator` is an `OpenAPI` annotation rather than a JSON Schema one, so a
/// validator ignores it; a model generator uses it to build one tagged union
/// instead of trying every variant in turn, which is what turns a bad value inside
/// one variant into a single field error rather than one error per variant.
#[cfg(feature = "schema")]
const DISCRIMINATED: [(&str, &str); 2] = [
    ("CalendarOutcome", "status"),
    ("GraphWorkingHoursOutcome", "status"),
];

/// The bundled time zone database, or the failure that says it is unusable.
#[cfg(all(feature = "bundled-tzdb", any(feature = "google", feature = "graph")))]
fn bundled() -> Result<TimeZoneDatabase, JsonError> {
    TimeZoneDatabase::bundled().map_err(|err| JsonError::new(err.code(), err.to_string(), ""))
}

/// The participant zone a caller names, as the wire layer's own name type.
#[cfg(any(feature = "google", feature = "graph"))]
fn zone_name(name: &str) -> Result<ZoneName, JsonError> {
    ZoneName::new(name).map_err(|err| {
        JsonError::new(
            err.code(),
            format!("participant zone: {err}"),
            "/participant_zone",
        )
    })
}

/// One outcome per calendar ID, as JSON, sorted by ID.
///
/// The map is a `BTreeMap`, so the object keys are in lexicographic order rather
/// than the order the provider named the calendars; a JSON object has no order a
/// caller may rely on.
#[cfg(any(feature = "google", feature = "graph"))]
fn outcomes_to_json(outcomes: &BTreeMap<String, CalendarOutcome>) -> Result<String, JsonError> {
    let wire: BTreeMap<String, WireCalendarOutcome> = outcomes
        .iter()
        .map(|(id, outcome)| (id.clone(), WireCalendarOutcome::from(outcome)))
        .collect();
    opportune_json::to_json(&wire)
}

/// A payload failure as the structured error a binding raises.
#[cfg(any(feature = "google", feature = "graph"))]
fn payload_error(err: &AdapterPayloadError) -> JsonError {
    JsonError::new(err.code(), err.to_string(), "")
}

/// The caller's category map, read from a JSON object of name to tag path.
#[cfg(feature = "graph")]
fn category_map(json: Option<&str>) -> Result<Option<BTreeMap<String, Tag>>, JsonError> {
    let Some(json) = json else {
        return Ok(None);
    };
    let raw: BTreeMap<String, String> = serde_json::from_str(json)
        .map_err(|err| JsonError::new("invalid_json", err.to_string(), ""))?;
    let mut map = BTreeMap::new();
    for (name, path) in raw {
        let tag = Tag::new(&path).map_err(|err| {
            JsonError::new(
                err.code(),
                format!("the category map's tag `{path}`: {err}"),
                join_pointer("", &name),
            )
        })?;
        map.insert(name, tag);
    }
    Ok(Some(map))
}
