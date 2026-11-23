//! Wire types, validating JSON conversions and RFC 6901 diagnostics for the
//! [`opportune`] engine.
//!
//! Wire types are deliberately separate from the domain types: parsing converts
//! into [`opportune`] types and can never change what the engine means. A
//! duration is a string in a document and a [`Duration`] once parsed; a block
//! status is any string in a document — an unrecognised one reading as `busy` —
//! and a [`BlockStatus`] once parsed. A rejected document comes back as a
//! [`JsonError`] whose `path` is an RFC 6901 JSON pointer naming the field at
//! fault, so a caller can open the exact place an API consumer has to fix.
//!
//! # Entry points
//!
//! [`parse_request`] reads a search request, and [`parse_proposal`],
//! [`parse_ranges_query`], [`parse_rule`], [`parse_goal`], [`parse_calendar`] and
//! [`parse_limits`] read the pieces a caller stores on its own. Each is a public
//! [`TryFrom`] conversion too — `Request::try_from(&WireRequest)`
//! and its siblings — so a Rust service that already holds a wire value, because it
//! deserialized one as a field of its own request body, converts it without writing
//! it back to JSON text. The records a service caches on their own convert the same
//! way — `Participant::try_from(&WireParticipant)`, `Block::try_from(&WireBlock)` and
//! `Occurrence::try_from(&WireOccurrence)` — so a calendar row read back from JSONB
//! becomes the domain value without a second parse. [`to_json`] writes any wire value
//! back out, deterministically:
//! maps are ordered, and every required field is always present.
//! Parsing never evaluates a request, so a name the engine cannot resolve — an
//! unknown zone or calendar — is [`Engine::validate`]'s finding rather than a parse
//! failure.
//!
//! The answer types — [`WireResponse`], [`WireSlotCheck`], [`WireDiagnostics`],
//! [`WireRequiredRanges`] — read back from JSON as well, so a Rust HTTP client
//! deserializes the engine's answer instead of handling JSON text, and an additive
//! field a newer engine sends does not fail the read.
//!
//! [`find_availability`], [`check_slot`], [`validate`] and [`required_ranges`] run
//! one of those requests through an [`opportune::Engine`] and write the answer as
//! the canonical JSON document. They are the whole request-to-response pipeline in
//! one call, so a binding crosses its language boundary once instead of
//! reassembling the three steps in its own dialect. Their examples build an engine
//! from the bundled database, which the `bundled-tzdb` feature provides; the wire
//! layer itself never reads a database, so that feature is off by default and an
//! adopter supplies the engine — and the database behind it — itself.
//!
//! # Schemas and upgrades
//!
//! With the `schema` feature on, `schema()` returns the whole format as one
//! Draft 2020-12 JSON Schema document whose `$defs` name and title every wire
//! type a binding generates a model from, so those models are generated from the
//! same shapes serde reads. The `Rule` union inlines its variants and titles each
//! after its kind; the `window` variant is also a `WindowRule` definition the
//! union references, so a shape that only ever holds one names it directly.
//! [`upgrade_rule`], [`upgrade_calendar`] and [`upgrade_goal`] take a document a
//! caller stored together with the schema version it was written with and return
//! the current version's canonical JSON, rejecting a version this build does not
//! read instead of misreading it. [`SCHEMA_VERSION`] is the version they read, so
//! a persistence layer that stores the number beside the document takes it from
//! here rather than hardcoding it.
//!
//! # Defaults
//!
//! Wire format defaults are applied while parsing: a participant
//! is a `person` and a `host`, a part's ID is `main`, a goal's display zone is its
//! event zone, a block is `busy` and not all-day, a rule constrains every host
//! (`who`) and every booking (`when`), and a cap's week starts on Monday. Optional
//! lists default to empty, and an absent field that means "nothing" — a
//! participant's calendar, a goal's grid — stays absent rather than becoming
//! something plausible.
//!
//! # Rounding
//!
//! `now` rounds up to the next minute, a block's endpoints round outward, and
//! every other instant — a candidate start, a search window, a proposal's start —
//! must already be minute-aligned, because the model has no sub-minute instants.
//!
//! ```
//! use opportune_json::{WireRequest, parse_request, to_json};
//!
//! let document = r#"{
//!   "now": "2027-03-09T15:00:01Z",
//!   "goal": {
//!     "parts": [{
//!       "duration": "45m",
//!       "requirement": { "all": [{ "person": "ana" }] }
//!     }],
//!     "search_window": ["2027-03-09T15:00Z", "2027-03-10T15:00Z"],
//!     "zone": "UTC"
//!   },
//!   "participants": [{ "id": "ana", "zone": "UTC" }]
//! }"#;
//! let request = parse_request(document).unwrap();
//! // `now` rounded up, the part is `main`, and the participant is a host person.
//! assert_eq!(request.now.to_string(), "2027-03-09T15:01Z");
//! assert_eq!(request.goal.parts[0].id, "main");
//! assert_eq!(request.goal.display_zone, request.goal.zone);
//! assert_eq!(request.participants[0].role, opportune::tags::Role::Host);
//!
//! // The wire value writes back out deterministically.
//! let wire = WireRequest::from(&request);
//! assert_eq!(to_json(&wire).unwrap(), to_json(&wire).unwrap());
//! ```
//!
//! [`Duration`]: opportune::time::Duration
//! [`BlockStatus`]: opportune::model::BlockStatus
//! [`Engine::validate`]: opportune::Engine::validate

mod convert;
pub mod error;
pub mod primitives;
#[cfg(feature = "schema")]
mod schema;
#[cfg(feature = "schema")]
#[doc(hidden)]
pub mod schema_support {
    //! Schema-bundle normalisation helpers shared with `opportune-adapters`.
    pub use crate::schema::{close_object, discriminate, retarget};
}
mod upgrade;
pub mod wire;

pub use error::{JsonError, join_index, join_pointer};
pub use opportune::model::SCHEMA_VERSION;
pub use primitives::{InstantRounding, MatchAll};
#[cfg(feature = "schema")]
pub use schema::schema;
pub use upgrade::{upgrade_calendar, upgrade_goal, upgrade_rule};
pub use wire::*;

use opportune::Engine;
use opportune::Limits;
use opportune::model::{CalendarSpec, Goal, Proposal, RangesQuery, Request, Rule};
use serde::Serialize;
use serde::de::DeserializeOwned;

/// Parses a search request.
///
/// Wire format defaults are applied, `now` rounds up, blocks
/// round outward, and every other instant must be minute-aligned.
///
/// ```
/// use opportune_json::parse_request;
///
/// let request = parse_request(r#"{
///   "now": "2027-03-09T15:00Z",
///   "goal": {
///     "parts": [{ "duration": "30m", "requirement": { "all": [{ "person": "ana" }] } }],
///     "search_window": ["2027-03-09T15:00Z", "2027-03-10T15:00Z"],
///     "zone": "UTC"
///   }
/// }"#).unwrap();
/// assert_eq!(request.limit(), 1_000);
/// ```
///
/// # Errors
/// Returns [`JsonError`] when the document is not JSON, does not match the wire
/// shape, or holds a value the domain types cannot: a duration naming days, an
/// unaligned instant, a tag with a disallowed character, a status-policy entry
/// that matches nothing. The error's `path` names the field at fault.
pub fn parse_request(json: &str) -> Result<Request, JsonError> {
    convert::request(&decode::<WireRequest>(json)?)
}

/// Parses one rule.
///
/// ```
/// use opportune_json::parse_rule;
///
/// let rule = parse_rule(r#"{
///   "id": "r_min",
///   "kind": "lead_time",
///   "params": { "min": "4h" }
/// }"#).unwrap();
/// assert_eq!(rule.id, "r_min");
/// ```
///
/// # Errors
/// Returns [`JsonError`] when the document is not JSON, does not match the wire
/// shape, or holds a value the rule cannot: a window range that ends where it
/// starts, a repeated weekday, a duration naming days.
pub fn parse_rule(json: &str) -> Result<Rule, JsonError> {
    convert::rule(&decode::<WireRule>(json)?, "")
}

/// Parses one working calendar.
///
/// ```
/// use opportune_json::parse_calendar;
///
/// let calendar = parse_calendar(r#"{
///   "zone": "America/New_York",
///   "working_days": "mon-fri",
///   "holidays": ["2027-01-01"]
/// }"#).unwrap();
/// assert_eq!(calendar.zone.as_str(), "America/New_York");
/// ```
///
/// # Errors
/// Returns [`JsonError`] when the document is not JSON, does not match the wire
/// shape, or names a date that does not exist or a weekday that is not one.
pub fn parse_calendar(json: &str) -> Result<CalendarSpec, JsonError> {
    convert::calendar_of(&decode::<WireCalendarSpec>(json)?, "")
}

/// Parses one goal.
///
/// ```
/// use opportune_json::parse_goal;
///
/// let goal = parse_goal(r#"{
///   "parts": [{ "duration": "45m", "requirement": { "all": [{ "person": "ana" }] } }],
///   "search_window": ["2027-03-09T15:00Z", "2027-03-10T15:00Z"],
///   "zone": "Europe/London",
///   "display_zone": "America/New_York"
/// }"#).unwrap();
/// assert_eq!(goal.parts[0].duration.minutes(), 45);
/// assert_eq!(goal.display_zone.as_str(), "America/New_York");
/// ```
///
/// # Errors
/// Returns [`JsonError`] when the document is not JSON, does not match the wire
/// shape, or holds a value the goal cannot: a grid step outside one minute to one
/// day, unaligned candidate starts, a span that covers no minute.
pub fn parse_goal(json: &str) -> Result<Goal, JsonError> {
    convert::goal(&decode::<WireGoal>(json)?, "")
}

/// Parses a proposed booking.
///
/// ```
/// use opportune_json::parse_proposal;
///
/// let proposal = parse_proposal(r#"{
///   "events": [{ "part": "main", "start": "2027-03-09T15:00Z", "assignment": [["ana"]] }]
/// }"#).unwrap();
/// assert_eq!(proposal.events[0].assignment[0][0], "ana");
/// ```
///
/// # Errors
/// Returns [`JsonError`] when the document is not JSON, does not match the wire
/// shape, or names a start that is not minute-aligned.
pub fn parse_proposal(json: &str) -> Result<Proposal, JsonError> {
    convert::proposal(&decode::<WireProposal>(json)?)
}

/// Parses a ranges query.
///
/// ```
/// use opportune_json::parse_ranges_query;
///
/// let query = parse_ranges_query(r#"{
///   "now": "2027-03-09T15:00Z",
///   "goal": {
///     "parts": [{ "duration": "30m", "requirement": { "all": [{ "person": "ana" }] } }],
///     "search_window": ["2027-03-09T15:00Z", "2027-03-10T15:00Z"],
///     "zone": "UTC"
///   },
///   "participants": [{ "id": "ana", "zone": "UTC" }]
/// }"#).unwrap();
/// assert_eq!(query.participants.len(), 1);
/// ```
///
/// # Errors
/// Returns [`JsonError`] for the same reasons [`parse_request`] does.
pub fn parse_ranges_query(json: &str) -> Result<RangesQuery, JsonError> {
    convert::ranges_query(&decode::<WireRangesQuery>(json)?)
}

/// Parses the engine's request-size limits.
///
/// ```
/// use opportune_json::parse_limits;
///
/// let limits = parse_limits(r#"{
///   "max_participants": 50,
///   "max_blocks_per_participant": 500,
///   "max_search_window_days": 31,
///   "max_candidate_starts": 20000,
///   "max_explain_limit": 500
/// }"#).unwrap();
/// assert_eq!(limits.max_participants, 50);
/// ```
///
/// Every field is optional, and one the document omits — or writes as `null` — is
/// [`Limits::default`]'s value for that cap; `{}` is therefore the engine's
/// default configuration. To override one cap, name only that one:
///
/// ```
/// use opportune::Limits;
/// use opportune_json::parse_limits;
///
/// let limits = parse_limits(r#"{ "max_search_window_days": 30 }"#).unwrap();
/// assert_eq!(limits.max_search_window_days, 30);
/// assert_eq!(limits.max_participants, Limits::default().max_participants);
/// ```
///
/// # Errors
/// Returns [`JsonError`] when the document is not JSON or does not match the wire
/// shape: a count the field cannot hold, or a key the shape does not name.
pub fn parse_limits(json: &str) -> Result<Limits, JsonError> {
    Ok(convert::limits(&decode::<WireLimits>(json)?))
}

/// Writes a wire value as JSON, deterministically.
///
/// ```
/// use opportune_json::{WireSpan, to_json};
///
/// assert_eq!(to_json(&WireSpan("2027-03-09T15:00Z".to_owned(), "2027-03-09T15:45Z".to_owned())).unwrap(),
///     r#"["2027-03-09T15:00Z","2027-03-09T15:45Z"]"#);
/// ```
///
/// # Errors
/// Returns [`JsonError`] with code `serialization_failed` when the value cannot be
/// written, which a wire value never does: every map key is a string and no
/// serialiser refuses a value.
pub fn to_json<T: Serialize>(value: &T) -> Result<String, JsonError> {
    serde_json::to_string(value).map_err(JsonError::from)
}

/// The offers a request document produces, as the canonical JSON `Response`.
///
/// The three steps a caller otherwise repeats — parse, evaluate, write — are one
/// call, so a binding hands a document over and passes the answer on unchanged.
///
/// ```
/// # #[cfg(feature = "bundled-tzdb")] {
/// use opportune::Engine;
/// use opportune_json::find_availability;
///
/// let engine = Engine::builder().build().unwrap();
/// let response = find_availability(&engine, r#"{
///   "now": "2027-03-09T15:00Z",
///   "goal": {
///     "parts": [{ "duration": "30m", "requirement": { "all": [{ "person": "ana" }] } }],
///     "search_window": ["2027-03-09T15:00Z", "2027-03-09T16:00Z"],
///     "zone": "UTC"
///   },
///   "participants": [{ "id": "ana", "zone": "UTC", "calendar": [] }]
/// }"#).unwrap();
/// assert!(response.starts_with(r#"{"offers":"#), "{response}");
/// # }
/// ```
///
/// # Errors
/// Returns [`JsonError`] when the document is rejected, as [`parse_request`]
/// reports it, and the engine's own failure — `invalid_request` carrying every
/// validation finding in [`errors`](JsonError::errors) — when the request is
/// invalid.
pub fn find_availability(engine: &Engine, request_json: &str) -> Result<String, JsonError> {
    let request = parse_request(request_json)?;
    let response = engine.find_availability(&request)?;
    to_json(&WireResponse::from(&response))
}

/// The judgement of one proposed booking, as the canonical JSON `SlotCheck`.
///
/// ```
/// # #[cfg(feature = "bundled-tzdb")] {
/// use opportune::Engine;
/// use opportune_json::check_slot;
///
/// let engine = Engine::builder().build().unwrap();
/// let request = r#"{
///   "now": "2027-03-09T15:00Z",
///   "goal": {
///     "parts": [{ "duration": "30m", "requirement": { "all": [{ "person": "ana" }] } }],
///     "search_window": ["2027-03-09T15:00Z", "2027-03-09T16:00Z"],
///     "zone": "UTC"
///   },
///   "participants": [{ "id": "ana", "zone": "UTC", "calendar": [] }]
/// }"#;
/// let proposal = r#"{
///   "events": [{ "part": "main", "start": "2027-03-09T15:00Z", "assignment": [["ana"]] }]
/// }"#;
/// let checked = check_slot(&engine, request, proposal).unwrap();
/// assert!(checked.contains(r#"{"valid":true"#), "{checked}");
/// # }
/// ```
///
/// # Errors
/// Returns [`JsonError`] when either document is rejected, and the engine's own
/// failure — a `proposal_*` code naming the proposal field at fault in
/// [`path`](JsonError::path) — when the proposal does not fit the request.
pub fn check_slot(
    engine: &Engine,
    request_json: &str,
    proposal_json: &str,
) -> Result<String, JsonError> {
    let request = parse_request(request_json)?;
    let proposal = parse_proposal(proposal_json)?;
    let checked = engine.check_slot(&request, &proposal)?;
    to_json(&WireSlotCheck::from(&checked))
}

/// A request document's static errors and warnings, as canonical JSON
/// `Diagnostics`.
///
/// ```
/// # #[cfg(feature = "bundled-tzdb")] {
/// use opportune::Engine;
/// use opportune_json::validate;
///
/// let engine = Engine::builder().build().unwrap();
/// let diagnostics = validate(&engine, r#"{
///   "now": "2027-03-09T15:00Z",
///   "goal": {
///     "parts": [{ "duration": "30m", "requirement": { "all": [{ "person": "ana" }] } }],
///     "search_window": ["2027-03-09T15:00Z", "2027-03-09T16:00Z"],
///     "zone": "Mars/Olympus_Mons"
///   }
/// }"#).unwrap();
/// assert!(diagnostics.contains("zone_unknown"), "{diagnostics}");
/// # }
/// ```
///
/// # Errors
/// Returns [`JsonError`] when the document is rejected, as [`parse_request`]
/// reports it.
///
/// A request that validation finds impossible is not an error here: the findings
/// are the answer, and a caller that wants them as one hands it to
/// [`find_availability`], which reports them as `invalid_request`.
pub fn validate(engine: &Engine, request_json: &str) -> Result<String, JsonError> {
    let request = parse_request(request_json)?;
    to_json(&WireDiagnostics::from(&engine.validate(&request)))
}

/// The calendar and occurrence data a ranges query needs, as canonical JSON
/// `RequiredRanges`.
///
/// ```
/// # #[cfg(feature = "bundled-tzdb")] {
/// use opportune::Engine;
/// use opportune_json::required_ranges;
///
/// let engine = Engine::builder().build().unwrap();
/// let ranges = required_ranges(&engine, r#"{
///   "now": "2027-03-09T15:00Z",
///   "goal": {
///     "parts": [{ "duration": "30m", "requirement": { "all": [{ "person": "ana" }] } }],
///     "search_window": ["2027-03-09T15:00Z", "2027-03-09T16:00Z"],
///     "zone": "UTC"
///   },
///   "participants": [{ "id": "ana", "zone": "UTC" }]
/// }"#).unwrap();
/// assert!(ranges.starts_with(r#"{"calendar":"#), "{ranges}");
/// # }
/// ```
///
/// # Errors
/// Returns [`JsonError`] when the document is rejected, as
/// [`parse_ranges_query`] reports it, and the engine's own failure when the query
/// is invalid.
pub fn required_ranges(engine: &Engine, query_json: &str) -> Result<String, JsonError> {
    let query = parse_ranges_query(query_json)?;
    let ranges = engine.required_ranges(&query)?;
    to_json(&WireRequiredRanges::from(&ranges))
}

/// Decodes a document, tracking the pointer of the field serde is reading.
///
/// `serde_path_to_error` reports the path but does not insist the document ends
/// where the value did, so trailing characters are rejected explicitly rather
/// than silently ignored.
fn decode<T: DeserializeOwned>(json: &str) -> Result<T, JsonError> {
    let mut deserializer = serde_json::Deserializer::from_str(json);
    let value =
        serde_path_to_error::deserialize(&mut deserializer).map_err(JsonError::from_serde)?;
    deserializer
        .end()
        .map_err(|err| JsonError::new("invalid_json", err.to_string(), ""))?;
    Ok(value)
}
