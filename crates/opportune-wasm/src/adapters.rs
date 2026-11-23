//! The Graph and Google payload conversions, read against an engine's database.
//!
//! Each function is the Rust-callable form of the matching `WasmEngine` export,
//! and delegates to the adapter's own `*_with_db` conversion: the payload is read
//! with the engine's time zone database, so the blocks a caller stores and the
//! evaluation they feed resolve zone names in one database.
//!
//! A payload this layer cannot read is an `unavailable` outcome inside the
//! returned document rather than an error, because it is about the calendar, not
//! about the call: an errored mailbox must not read as a free one. The errors are
//! about the arguments — a blank participant zone, a category map that is not an
//! object of tag paths — and about the provider envelope itself, which has to be
//! a payload of the documented shape before a calendar can be one value in it.

pub use opportune_adapters::graph::{
    EXTENDED_PROPERTY_EVENT_KEY_ID, EXTENDED_PROPERTY_EXPAND, EXTENDED_PROPERTY_NAMESPACE_GUID,
    EXTENDED_PROPERTY_TAGS_ID,
};
pub use opportune_adapters::{MARKER_EVENT_KEY, MARKER_TAGS};

use crate::engine::Engine;
use opportune_json::JsonError;

/// Converts a Graph `calendarView` response, as a JSON `CalendarOutcome`.
///
/// `participant_zone` is the mailbox owner's IANA zone, read for all-day events;
/// `category_map_json` is an optional JSON object mapping a category name to a
/// tag path.
///
/// # Errors
/// Returns [`JsonError`] when `participant_zone` is blank, when
/// `category_map_json` is not a JSON object of tag paths, or when a mapped tag is
/// not a tag path.
pub fn calendar_view_to_blocks(
    engine: &Engine,
    payload_json: &str,
    participant_zone: &str,
    category_map_json: Option<&str>,
) -> Result<String, JsonError> {
    opportune_adapters::json::calendar_view_to_blocks_with_db(
        engine.time_zone_database(),
        payload_json,
        participant_zone,
        category_map_json,
    )
}

/// Converts a Graph `getSchedule` response, as JSON outcomes keyed by
/// `scheduleId`.
///
/// # Errors
/// Returns [`JsonError`] with code `payload_invalid` when the document is not a
/// `getSchedule` response this adapter can read.
pub fn get_schedule_to_blocks(engine: &Engine, payload_json: &str) -> Result<String, JsonError> {
    opportune_adapters::json::get_schedule_to_blocks_with_db(
        engine.time_zone_database(),
        payload_json,
    )
}

/// Converts one mailbox's Graph `workingHours`, as a JSON
/// `GraphWorkingHoursOutcome`.
///
/// `rule_id` is the ID the resulting `window` rule takes.
///
/// # Errors
/// Returns [`JsonError`] only for a document that cannot be written as JSON. A
/// working-hours payload that cannot become a rule is an `unavailable` outcome,
/// so a caller falls back to its own window without catching anything.
pub fn working_hours_to_rule(
    engine: &Engine,
    payload_json: &str,
    rule_id: &str,
) -> Result<String, JsonError> {
    opportune_adapters::json::working_hours_to_rule_with_db(
        engine.time_zone_database(),
        payload_json,
        rule_id,
    )
}

/// Converts a Google `events.list` response, as a JSON `CalendarOutcome`.
///
/// `participant_zone` is the calendar owner's IANA zone, read for all-day events
/// when the response names no `timeZone` of its own.
///
/// # Errors
/// Returns [`JsonError`] when `participant_zone` is blank.
pub fn events_list_to_blocks(
    engine: &Engine,
    payload_json: &str,
    participant_zone: &str,
) -> Result<String, JsonError> {
    opportune_adapters::json::events_list_to_blocks_with_db(
        engine.time_zone_database(),
        payload_json,
        participant_zone,
    )
}

/// Converts a Google `freeBusy.query` response, as JSON outcomes keyed by
/// calendar ID.
///
/// These ranges carry their own UTC offsets and name no zone, so the conversion
/// reads no database — it takes no engine.
///
/// # Errors
/// Returns [`JsonError`] with code `payload_invalid` when the document is not a
/// `freeBusy.query` response this adapter can read.
pub fn free_busy_to_blocks(payload_json: &str) -> Result<String, JsonError> {
    opportune_adapters::json::free_busy_to_blocks(payload_json)
}
