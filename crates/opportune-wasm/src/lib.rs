//! WASM binding for the [`opportune`] engine: the wire entry points, the Graph
//! and Google conversions, the schema upgrades and the marker constants, for a
//! JavaScript or TypeScript caller in Node or Bun.
//!
//! The boundary is JSON in both directions: a caller hands in a document and gets
//! one back, so the format lives in one place — [`opportune_json`]'s wire types —
//! rather than in a hand-written conversion per language. Failures are thrown as
//! the structured payload [`error::payload`] writes, which the TypeScript layer
//! turns back into an `OpportuneError` carrying the stable `code`, the RFC 6901
//! `path` of the field at fault and the findings a rejected request produced.
//! Nothing panics across the boundary.
//!
//! This module is the export surface only. The behaviour lives in [`engine`] and
//! [`adapters`], which are plain Rust and therefore drive the same code on the
//! host target's tests; a binding that re-implemented the work here could only be
//! tested in a wasm runtime.
//!
//! [`opportune`]: https://docs.rs/opportune

pub mod adapters;
pub mod engine;
pub mod error;

use engine::Engine;
use wasm_bindgen::prelude::*;

/// An engine: the bundled time zone database and the request limits, built once
/// and shared by every call.
///
/// `limits_json` holds the request-size limits as JSON, or is `null` or `undefined`
/// for the engine's defaults — the document `default_limits_json` writes, with any
/// of its fields changed.
///
/// The query methods answer from this engine, so `limits_json` applies to them.
/// The Graph and Google conversions resolve the zone names a payload carries
/// against this engine's database, so a payload and the evaluation it feeds are
/// read in one database. The one exception is Google `freeBusy`, whose ranges
/// carry their own offsets and read no zone name.
#[wasm_bindgen]
pub struct WasmEngine {
    engine: Engine,
}

#[wasm_bindgen]
impl WasmEngine {
    /// Builds an engine over the bundled database and the given limits.
    ///
    /// # Errors
    /// Returns [`Err`] holding the structured payload with the database's own
    /// code when it cannot be built, and with the parser's code and pointer when
    /// `limits_json` is not a limits document.
    #[wasm_bindgen(constructor)]
    #[expect(
        clippy::needless_pass_by_value,
        reason = "an optional JavaScript string arrives as `Option<String>`: `Option<&str>` is not a type `wasm-bindgen` converts, and the string is the export's only copy"
    )]
    pub fn new(limits_json: Option<String>) -> Result<WasmEngine, JsValue> {
        Engine::new(limits_json.as_deref())
            .map(|engine| Self { engine })
            .map_err(|error| error::rejection(&error))
    }

    /// The version of the time zone database this engine resolves zones against,
    /// as a response's `meta.tzdb` reports it.
    #[must_use]
    pub fn tzdb_version(&self) -> String {
        self.engine.tzdb_version().to_owned()
    }

    /// Offers: every candidate start the request allows, as a JSON `Response`.
    ///
    /// # Errors
    /// Returns [`Err`] holding the structured payload when the document is not a
    /// request, or when the request is invalid: a rejected request carries every
    /// finding its validation produced in `errors`, each with its own pointer.
    pub fn find_availability_json(&self, request_json: &str) -> Result<String, JsValue> {
        self.engine
            .find_availability_json(request_json)
            .map_err(|error| error::rejection(&error))
    }

    /// Judges one proposed booking against the same rules and calendars, as a
    /// JSON `SlotCheck`.
    ///
    /// # Errors
    /// Returns [`Err`] holding the structured payload when either document is
    /// rejected, naming the proposal field at fault in `path` when the proposal
    /// does not fit the request.
    pub fn check_slot_json(
        &self,
        request_json: &str,
        proposal_json: &str,
    ) -> Result<String, JsValue> {
        self.engine
            .check_slot_json(request_json, proposal_json)
            .map_err(|error| error::rejection(&error))
    }

    /// The request's static errors and warnings, as a JSON `Diagnostics`.
    ///
    /// # Errors
    /// Returns [`Err`] holding the structured payload when the document is not a
    /// request.
    pub fn validate_json(&self, request_json: &str) -> Result<String, JsValue> {
        self.engine
            .validate_json(request_json)
            .map_err(|error| error::rejection(&error))
    }

    /// The calendar and occurrence data a ranges query needs, as a JSON
    /// `RequiredRanges`.
    ///
    /// The hulls begin a two-phase fetch: with `occurrences` present the caller
    /// also needs every record carrying an event key observed on a participant's
    /// own-span occurrence-range records, on every participant, or all records
    /// across the full supported time domain; otherwise it must fail closed. A
    /// one-pass fetch is key-complete only under the identical-own-span invariant.
    ///
    /// # Errors
    /// Returns [`Err`] holding the structured payload when the query document is
    /// rejected.
    pub fn required_ranges_json(&self, query_json: &str) -> Result<String, JsValue> {
        self.engine
            .required_ranges_json(query_json)
            .map_err(|error| error::rejection(&error))
    }

    /// Converts a Graph `calendarView` response, as a JSON `CalendarOutcome`.
    ///
    /// `participant_zone` is the mailbox owner's IANA zone, read for all-day
    /// events; `category_map_json` is an optional JSON object mapping a category
    /// name to a tag path.
    ///
    /// # Errors
    /// Returns [`Err`] holding the structured payload when `participant_zone` is
    /// blank, when `category_map_json` is not a JSON object of tag paths, or when
    /// a mapped tag is not a tag path. A payload this adapter cannot read is an
    /// `unavailable` outcome in the result, not an error.
    #[expect(
        clippy::needless_pass_by_value,
        reason = "an optional JavaScript string arrives as `Option<String>`: `Option<&str>` is not a type `wasm-bindgen` converts, and the string is the export's only copy"
    )]
    pub fn graph_calendar_view_to_blocks_json(
        &self,
        payload_json: &str,
        participant_zone: &str,
        category_map_json: Option<String>,
    ) -> Result<String, JsValue> {
        adapters::calendar_view_to_blocks(
            &self.engine,
            payload_json,
            participant_zone,
            category_map_json.as_deref(),
        )
        .map_err(|error| error::rejection(&error))
    }

    /// Converts a Graph `getSchedule` response, as JSON outcomes keyed by
    /// `scheduleId`.
    ///
    /// # Errors
    /// Returns [`Err`] holding the structured payload with code `payload_invalid`
    /// when the document is not a `getSchedule` response this adapter can read. A
    /// schedule the provider reported an error for is an `unavailable` outcome
    /// inside the object, not an error.
    pub fn graph_get_schedule_to_blocks_json(&self, payload_json: &str) -> Result<String, JsValue> {
        adapters::get_schedule_to_blocks(&self.engine, payload_json)
            .map_err(|error| error::rejection(&error))
    }

    /// Converts one mailbox's Graph `workingHours`, as a JSON
    /// `GraphWorkingHoursOutcome`.
    ///
    /// `rule_id` is the ID the resulting `window` rule takes.
    ///
    /// # Errors
    /// Returns [`Err`] holding the structured payload only for a document that
    /// cannot be written as JSON. A custom time zone is an `unavailable` outcome,
    /// so a caller falls back to its own window without catching anything.
    pub fn graph_working_hours_to_rule_json(
        &self,
        payload_json: &str,
        rule_id: &str,
    ) -> Result<String, JsValue> {
        adapters::working_hours_to_rule(&self.engine, payload_json, rule_id)
            .map_err(|error| error::rejection(&error))
    }

    /// Converts a Google `events.list` response, as a JSON `CalendarOutcome`.
    ///
    /// `participant_zone` is the calendar owner's IANA zone, read for all-day
    /// events when the response names no `timeZone` of its own.
    ///
    /// # Errors
    /// Returns [`Err`] holding the structured payload when `participant_zone` is
    /// blank. A payload this adapter cannot read is an `unavailable` outcome in
    /// the result, not an error.
    pub fn google_events_list_to_blocks_json(
        &self,
        payload_json: &str,
        participant_zone: &str,
    ) -> Result<String, JsValue> {
        adapters::events_list_to_blocks(&self.engine, payload_json, participant_zone)
            .map_err(|error| error::rejection(&error))
    }

    /// Converts a Google `freeBusy.query` response, as JSON outcomes keyed by
    /// calendar ID.
    ///
    /// These ranges carry their own UTC offsets and name no zone, so the
    /// conversion reads no database.
    ///
    /// # Errors
    /// Returns [`Err`] holding the structured payload with code `payload_invalid`
    /// when the document is not a `freeBusy.query` response this adapter can
    /// read. A calendar the provider reported an error for is an `unavailable`
    /// outcome inside the object, not an error.
    pub fn google_free_busy_to_blocks_json(&self, payload_json: &str) -> Result<String, JsValue> {
        adapters::free_busy_to_blocks(payload_json).map_err(|error| error::rejection(&error))
    }
}

/// The version of the `opportune` crate this module was built from.
#[wasm_bindgen]
#[must_use]
pub fn version() -> String {
    opportune::VERSION.to_owned()
}

/// The engine's own default limits as the JSON document a caller starts from.
///
/// The document comes from [`opportune::Limits::default`], so the defaults a
/// caller reads cannot drift from the ones the engine applies to a request that
/// names no limits at all.
///
/// # Errors
/// Returns [`Err`] holding the structured payload only if the document cannot be
/// written, which the engine's own limits never fail to be.
#[wasm_bindgen]
pub fn default_limits_json() -> Result<String, JsValue> {
    engine::default_limits_json().map_err(|error| error::rejection(&error))
}

/// Rewrites a stored rule as the current schema version's canonical JSON.
///
/// `from_version` is the schema version the rule was written with.
///
/// # Errors
/// Returns [`Err`] holding the structured payload with code
/// `unsupported_schema_version` when this build does not read that version, and
/// with the parser's own code and pointer when the document is not a valid rule
/// of it.
#[wasm_bindgen]
pub fn upgrade_rule_json(rule_json: &str, from_version: u32) -> Result<String, JsValue> {
    opportune_json::upgrade_rule(rule_json, from_version).map_err(|error| error::rejection(&error))
}

/// Rewrites a stored working calendar as the current schema version's canonical
/// JSON.
///
/// # Errors
/// Returns [`Err`] holding the structured payload for the same reasons
/// [`upgrade_rule_json`] does.
#[wasm_bindgen]
pub fn upgrade_calendar_json(calendar_json: &str, from_version: u32) -> Result<String, JsValue> {
    opportune_json::upgrade_calendar(calendar_json, from_version)
        .map_err(|error| error::rejection(&error))
}

/// Rewrites a stored goal as the current schema version's canonical JSON.
///
/// # Errors
/// Returns [`Err`] holding the structured payload for the same reasons
/// [`upgrade_rule_json`] does.
#[wasm_bindgen]
pub fn upgrade_goal_json(goal_json: &str, from_version: u32) -> Result<String, JsValue> {
    opportune_json::upgrade_goal(goal_json, from_version).map_err(|error| error::rejection(&error))
}

/// The extended-property name a caller writes its space-separated tags under.
///
/// The value is the adapter's own constant, so a caller and the adapter cannot
/// disagree about where the tags are stored.
#[wasm_bindgen]
#[must_use]
pub fn marker_tags() -> String {
    adapters::MARKER_TAGS.to_owned()
}

/// The extended-property name a caller writes its event key under.
#[wasm_bindgen]
#[must_use]
pub fn marker_event_key() -> String {
    adapters::MARKER_EVENT_KEY.to_owned()
}

/// The GUID of the Graph property set the marker properties live in.
#[wasm_bindgen]
#[must_use]
pub fn graph_extended_property_namespace_guid() -> String {
    adapters::EXTENDED_PROPERTY_NAMESPACE_GUID.to_owned()
}

/// The `singleValueExtendedProperty` ID the tags marker is stored under.
#[wasm_bindgen]
#[must_use]
pub fn graph_extended_property_tags_id() -> String {
    adapters::EXTENDED_PROPERTY_TAGS_ID.to_owned()
}

/// The `singleValueExtendedProperty` ID the event-key marker is stored under.
#[wasm_bindgen]
#[must_use]
pub fn graph_extended_property_event_key_id() -> String {
    adapters::EXTENDED_PROPERTY_EVENT_KEY_ID.to_owned()
}

/// The Graph `$expand` clause that fetches the marker properties.
#[wasm_bindgen]
#[must_use]
pub fn graph_extended_property_expand() -> String {
    adapters::EXTENDED_PROPERTY_EXPAND.to_owned()
}
