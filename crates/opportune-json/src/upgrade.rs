//! Versioned upgrades for the documents a caller stores.
//!
//! Applications persist rules, calendars and goals as JSON documents — in a
//! document store, an object store or a JSON column, say — and the wire format may
//! gain a field between releases. An upgrade takes such a document together with
//! the schema version it was written with and returns it in the current version's
//! canonical form: every default resolved, every value written the one way the
//! current version spells it, and a fixed point, so upgrading the result again
//! rewrites nothing.
//!
//! Canonical spelling covers what the domain normalises: a day key becomes the
//! list of days it means, a duration becomes the hours and minutes the domain
//! holds, a window's weekly keys become one key per weekday, and blackout spans
//! are merged and sorted. What the domain keeps as written it writes back as
//! written — a window's date keys, and a list the engine reads as a set, such as
//! a calendar's holidays or a part's tags — so two stored documents that mean the
//! same thing do not always upgrade to the same bytes.
//!
//! Version 1 is the only version so far, so an upgrade is the validating round
//! trip itself: the parser rejects what the domain types cannot hold, naming the
//! field at fault with an RFC 6901 pointer, and [`to_json`] writes the current
//! shape. A version this build does not know is rejected *before* the document is
//! read, with the stable code
//! [`unsupported_schema_version`](crate::JsonError::code), rather than read with
//! the wrong parser and possibly accepted.

use crate::error::JsonError;
use crate::wire::{WireCalendarSpec, WireGoal, WireRule};
use crate::{SCHEMA_VERSION, parse_calendar, parse_goal, parse_rule, to_json};

/// Rewrites a stored rule as the current schema version's canonical JSON.
///
/// ```
/// use opportune_json::upgrade_rule;
///
/// let stored = r#"{"id": "r_min", "kind": "lead_time", "params": { "min": "4h" }}"#;
/// let canonical = upgrade_rule(stored, 1).unwrap();
/// assert_eq!(canonical, r#"{"kind":"lead_time","id":"r_min","who":{"role":"host"},"when":true,"params":{"min":"4h"}}"#);
/// // The canonical form is stable: reading it again rewrites nothing.
/// assert_eq!(upgrade_rule(&canonical, 1).unwrap(), canonical);
/// ```
///
/// # Errors
/// Returns [`JsonError`] with code `unsupported_schema_version` when
/// `from_version` is not the version this build reads, and the parser's own
/// failure — a stable code and the RFC 6901 pointer of the field at fault — when
/// the document is not a valid rule of that version.
pub fn upgrade_rule(json: &str, from_version: u32) -> Result<String, JsonError> {
    supported(from_version)?;
    to_json(&WireRule::from(&parse_rule(json)?))
}

/// Rewrites a stored working calendar as the current schema version's canonical
/// JSON.
///
/// ```
/// use opportune_json::upgrade_calendar;
///
/// let stored = r#"{ "zone": "Europe/London", "working_days": "mon-fri" }"#;
/// let canonical = upgrade_calendar(stored, 1).unwrap();
/// assert_eq!(canonical, r#"{"zone":"Europe/London","working_days":"mon,tue,wed,thu,fri","holidays":[]}"#);
/// ```
///
/// # Errors
/// Returns [`JsonError`] for the same reasons [`upgrade_rule`] does.
pub fn upgrade_calendar(json: &str, from_version: u32) -> Result<String, JsonError> {
    supported(from_version)?;
    to_json(&WireCalendarSpec::from(&parse_calendar(json)?))
}

/// Rewrites a stored goal as the current schema version's canonical JSON.
///
/// ```
/// use opportune_json::upgrade_goal;
///
/// let stored = r#"{
///   "parts": [{ "duration": "45m", "requirement": { "all": [{ "person": "ana" }] } }],
///   "search_window": ["2027-03-09T15:00Z", "2027-03-10T15:00Z"],
///   "zone": "UTC"
/// }"#;
/// let canonical = upgrade_goal(stored, 1).unwrap();
/// assert!(canonical.contains(r#""display_zone":"UTC""#), "{canonical}");
/// ```
///
/// # Errors
/// Returns [`JsonError`] for the same reasons [`upgrade_rule`] does.
pub fn upgrade_goal(json: &str, from_version: u32) -> Result<String, JsonError> {
    supported(from_version)?;
    to_json(&WireGoal::from(&parse_goal(json)?))
}

/// Rejects a document written with a schema version this build does not read.
///
/// The check runs before the document is parsed, because a document from another
/// version is not necessarily a malformed document of this one: reading it with
/// the wrong parser could accept it and silently change its meaning.
///
/// Each readable version has one arm. Version 1 is the current and only version,
/// and its step is a no-op: the parser and [`to_json`] already are the
/// canonicalisation. A later version adds its arm here together with the step that
/// lifts its document, so an old version keeps being readable instead of becoming
/// an error the day the current one moves.
fn supported(from_version: u32) -> Result<(), JsonError> {
    match from_version {
        1 => Ok(()),
        other => Err(JsonError::new(
            "unsupported_schema_version",
            format!(
                "the document was written with schema version {other}, \
                 and this build reads version {SCHEMA_VERSION}",
            ),
            "",
        )),
    }
}
