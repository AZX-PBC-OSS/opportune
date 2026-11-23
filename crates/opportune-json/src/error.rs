//! Wire failures: a stable code, a message, an RFC 6901 pointer, and the request
//! diagnostics a failure carries.
//!
//! Three kinds of failure are reported through [`JsonError`]. A document that
//! does not match the wire shape fails with the pointer serde was reading when it
//! failed; a document that matches the shape but not the domain rules fails with
//! the pointer the converter was building; an upgrade asked for a schema version
//! this build does not read fails before the document is read at all, because the
//! document as a whole is at fault. Each carries a stable [`code`] to match on
//! across releases and a [`path`] that is an RFC 6901 JSON pointer into the
//! document, with `""` naming the whole document.
//!
//! [`code`]: JsonError::code
//! [`path`]: JsonError::path
//!
//! # Codes
//!
//! Shape failures — reported by serde, or by the custom deserialisers for the
//! selector, term and `lead_time` maximum forms:
//!
//! | Code | Meaning |
//! |---|---|
//! | `invalid_json` | The input is not JSON, or ends early. |
//! | `unknown_field` | An object carried a key the wire shape does not name; the pointer names the object and the message names the key. |
//! | `missing_field` | An object left out a key the wire shape requires. |
//! | `duplicate_field` | An object named the same key twice; the pointer names the object and the message names the key. |
//! | `unknown_variant` | A string did not name one of a closed set of values. |
//! | `invalid_type` | A value had the wrong JSON type. |
//! | `invalid_value` | A value had the right type and an impossible value. |
//! | `invalid_length` | An array had the wrong number of elements. |
//! | `invalid_shape` | A value matched none of the forms a field accepts. |
//!
//! Domain failures carry the code of the `opportune` failure that rejected the
//! value (`tag_invalid_character`, `window_weekday_repeated`, `zone_name_invalid`
//! and so on), so the wire layer never invents a second vocabulary for a rule the
//! core already names. The wire layer's own additions are:
//!
//! | Code | Meaning |
//! |---|---|
//! | `instant_invalid` | A timestamp is not RFC 3339 with an explicit offset. |
//! | `duration_invalid` | A duration is not written in hours and minutes. |
//! | `duration_contains_days` | A duration named a day, which is not an exact length. |
//! | `date_invalid` | A date is not a `YYYY-MM-DD` civil date that exists. |
//! | `date_range_invalid` | A date range is not `date` or `date..date`. |
//! | `local_date_time_invalid` | A date-time is not written `YYYY-MM-DDTHH:MM`. |
//! | `time_of_day_invalid` | A time of day is not written `HH:MM`, or is outside the clock. |
//! | `wall_range_invalid` | A wall-clock range is not written `HH:MM-HH:MM`. |
//! | `weekday_invalid` | A weekday is not one of `mon` to `sun`. |
//! | `weekdays_invalid` | A day-key is not a list or range of weekdays. |
//! | `serialization_failed` | A value could not be written as JSON. |
//! | `unsupported_schema_version` | An upgrade was asked for a schema version this build does not read. |

use core::fmt;

use opportune::Diagnostic;
use serde_path_to_error::Segment;

/// A rejected JSON document.
///
/// It carries the stable [`code`](Self::code), a [`message`](Self::message)
/// naming the offending value, the RFC 6901 [`path`](Self::path) of the field at
/// fault, and any request [`errors`](Self::errors) the failure wraps.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct JsonError {
    code: &'static str,
    message: String,
    path: String,
    errors: Vec<Diagnostic>,
}

impl JsonError {
    /// A failure with a stable code, a message and the RFC 6901 pointer of the
    /// field at fault.
    ///
    /// The wire layer and the provider adapters report their findings through one
    /// error type, so a binding raises one exception with one set of codes
    /// whichever layer rejected the document. An adapter's code belongs in its own
    /// documented registry, and its `path` points into the document it was reading.
    ///
    /// ```
    /// use opportune_json::JsonError;
    ///
    /// let error = JsonError::new("payload_invalid", "the payload is not readable", "");
    /// assert_eq!(error.code(), "payload_invalid");
    /// assert_eq!(error.path(), "");
    /// ```
    #[must_use]
    pub fn new(code: &'static str, message: impl Into<String>, path: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            path: path.into(),
            errors: Vec::new(),
        }
    }

    /// The failure a domain value's constructor or the engine reported for
    /// `path`.
    ///
    /// The code and message are the domain failure's own, so a caller sees one
    /// vocabulary across the wire and the engine. A rejected request keeps its
    /// diagnostic findings, each with the pointer of the request field it is
    /// about.
    pub(crate) fn from_domain(err: opportune::Error, path: &str) -> Self {
        let code = err.code();
        let message = err.to_string();
        let errors = match err {
            opportune::Error::InvalidRequest { errors, .. } => errors,
            _ => Vec::new(),
        };
        Self {
            code,
            message,
            path: path.to_owned(),
            errors,
        }
    }

    /// The failure serde reported, with its tracked path converted to an RFC 6901
    /// pointer.
    pub(crate) fn from_serde(err: serde_path_to_error::Error<serde_json::Error>) -> Self {
        let track = err.path().clone();
        let inner = err.into_inner();
        let reported = inner.to_string();
        let code = classify(&inner, &reported);
        let path = pointer(&track, rejected_key(&reported));
        let message = union_message(&reported).unwrap_or(reported);
        Self {
            code,
            message,
            path,
            errors: Vec::new(),
        }
    }

    /// The stable, machine-readable code.
    ///
    /// The codes are listed in the [module documentation](self) and do not change
    /// across releases.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        self.code
    }

    /// The message naming the offending value, for a person.
    ///
    /// The text may be reworded; match on [`code`](Self::code) instead.
    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }

    /// The RFC 6901 JSON pointer of the field at fault, or `""` for the document
    /// as a whole.
    #[must_use]
    pub fn path(&self) -> &str {
        &self.path
    }

    /// The request diagnostics the failure wraps.
    ///
    /// A failure that wraps a request validation error carries every finding
    /// validation produced, so a caller sees all of its problems at once; every
    /// other failure carries none.
    #[must_use]
    pub fn errors(&self) -> &[Diagnostic] {
        &self.errors
    }
}

impl fmt::Display for JsonError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.path.is_empty() {
            write!(f, "{}: {}", self.code, self.message)
        } else {
            write!(f, "{} at `{}`: {}", self.code, self.path, self.message)
        }
    }
}

impl std::error::Error for JsonError {}

impl From<opportune::Error> for JsonError {
    /// The failure a domain value's constructor or the engine reported.
    ///
    /// One engine failure names a field of a document the caller wrote: a
    /// malformed proposal points into the proposal, and that pointer means the
    /// same thing here as it does in the engine, so it is kept. Every other
    /// failure is about no single field — the wire layer supplies the pointer for
    /// the field it was reading, and it is the one that knows.
    fn from(err: opportune::Error) -> Self {
        let path = err.path().unwrap_or_default().to_owned();
        Self::from_domain(err, &path)
    }
}

impl From<serde_json::Error> for JsonError {
    /// A failure writing a value as JSON. It has no pointer: the document does
    /// not exist yet.
    fn from(err: serde_json::Error) -> Self {
        Self::new("serialization_failed", err.to_string(), "")
    }
}

impl From<opportune::time::TimeError> for JsonError {
    /// The failure a civil-time constructor or operation reported, with the
    /// failure's own stable code and message and no pointer.
    ///
    /// It is the conversion that lets a handler returning `Result<_, JsonError>`
    /// — an Axum or Tonic route, say — use `?` on `Instant::new`, `Date::new`,
    /// `ZoneName::new` and the rest instead of restating each rejection.
    ///
    /// ```
    /// use opportune::time::{Instant, TimeError};
    /// use opportune_json::JsonError;
    ///
    /// let err: TimeError = Instant::from_unix_seconds(0).unwrap_err();
    /// let rejected = JsonError::from(err);
    /// assert_eq!(rejected.code(), "instant_out_of_range");
    /// assert_eq!(rejected.path(), "");
    /// ```
    fn from(err: opportune::time::TimeError) -> Self {
        Self::from_domain(err.into(), "")
    }
}

/// Appends `segment` to the RFC 6901 pointer `base`, escaping `~` as `~0` and
/// `/` as `~1`.
///
/// Participant IDs, calendar names and rule IDs are opaque, so a token can carry
/// either character.
///
/// ```
/// use opportune_json::{join_index, join_pointer};
///
/// assert_eq!(join_pointer("", "participants"), "/participants");
/// assert_eq!(join_pointer("/participants", "a/b~c"), "/participants/a~1b~0c");
/// assert_eq!(join_index("/participants/0/calendar", 2), "/participants/0/calendar/2");
/// ```
#[must_use]
pub fn join_pointer(base: &str, segment: &str) -> String {
    let mut pointer = String::with_capacity(base.len() + segment.len() + 1);
    pointer.push_str(base);
    pointer.push('/');
    for character in segment.chars() {
        match character {
            '~' => pointer.push_str("~0"),
            '/' => pointer.push_str("~1"),
            other => pointer.push(other),
        }
    }
    pointer
}

/// Appends the array index `index` to the RFC 6901 pointer `base`.
#[must_use]
pub fn join_index(base: &str, index: usize) -> String {
    format!("{base}/{index}")
}

/// The RFC 6901 pointer naming `path`.
///
/// `rejected` is the key a shape failure says was wrong — `unknown field` names
/// the key it does not accept, and `duplicate field` names the key it saw twice.
/// The track can include that key, because serde was reading it when it rejected
/// it; the pointer names the object whose shape the key broke, which is what a
/// caller has to fix, and the message names the key, so a trailing segment equal
/// to `rejected` is dropped.
///
/// A track holding a segment the tracker could not name is not a path this layer
/// can turn into a pointer — dropping the segment would name an ancestor field the
/// failure is not about — so the whole document is answered instead.
fn pointer(path: &serde_path_to_error::Path, rejected: Option<&str>) -> String {
    let segments: Vec<&Segment> = path.iter().collect();
    let mut pointer = String::new();
    for (index, segment) in segments.iter().enumerate() {
        let last = index + 1 == segments.len();
        match segment {
            Segment::Map { key } | Segment::Enum { variant: key } => {
                if last && rejected == Some(key.as_str()) {
                    continue;
                }
                pointer = join_pointer(&pointer, key);
            }
            Segment::Seq { index } => pointer = join_index(&pointer, *index),
            // JSON object keys are always strings, so serde_json never produces
            // this; a non-string key from another document format would.
            Segment::Unknown => return String::new(),
        }
    }
    pointer
}

/// The sentence an untagged union's "no variant matched" message becomes.
///
/// serde names the Rust type it was deserializing, which says nothing to a
/// caller of the wire layer, so each untagged union spells out the forms it
/// accepts here. A union without an entry falls back to a sentence that still
/// names no internals.
fn union_message(message: &str) -> Option<String> {
    let rest = message.strip_prefix("data did not match any variant of untagged enum ")?;
    let name = rest.split(' ').next()?;
    let forms = match name {
        "WireTerm" => {
            "a requirement term is `{ \"person\": id }` or `{ \"at_least\": n, \"of\": selector }`"
        }
        "WireLeadTimeMax" => {
            "a `lead_time` maximum is `{ \"duration\": d }`, `{ \"days\": n }`, \
             `{ \"working_days\": n, \"calendar\": name }` or `{ \"date\": d }`"
        }
        _ => "the value does not match any of the forms this field accepts",
    };
    Some(forms.to_owned())
}

/// The key a shape failure names as the one at fault, when it names one.
///
/// serde reports a key it does not accept as `unknown field `key``, and a key it
/// saw twice as `duplicate field `key``. Both sentences are the only place the key
/// appears, so the pointer can name the object rather than repeat it.
fn rejected_key(message: &str) -> Option<&str> {
    ["unknown field `", "duplicate field `"]
        .iter()
        .find_map(|prefix| message.strip_prefix(prefix))
        .and_then(|rest| rest.split_once('`'))
        .map(|(name, _)| name)
}

/// The stable code for a serde data error.
///
/// serde reports every data error as a message, and exposes no code of its own,
/// so the categories serde's message begins with are the taxonomy this layer can
/// name. A message outside them — one of this crate's own custom deserialisers,
/// say — is an `invalid_value`.
fn classify(err: &serde_json::Error, message: &str) -> &'static str {
    if err.is_syntax() || err.is_eof() || err.is_io() {
        return "invalid_json";
    }
    // serde appends the line and column; the category is what precedes it.
    let head = message.split(" at line ").next().unwrap_or(message);
    if head.starts_with("unknown field") {
        "unknown_field"
    } else if head.starts_with("missing field") {
        "missing_field"
    } else if head.starts_with("duplicate field") {
        "duplicate_field"
    } else if head.starts_with("unknown variant") {
        "unknown_variant"
    } else if head.starts_with("invalid type") {
        "invalid_type"
    } else if head.starts_with("invalid length") {
        "invalid_length"
    } else if head.starts_with("data did not match any variant") {
        "invalid_shape"
    } else {
        "invalid_value"
    }
}

#[cfg(test)]
mod tests {
    use opportune::Diagnostic;

    use super::{JsonError, join_index, join_pointer};

    #[test]
    fn a_pointer_escapes_the_two_reserved_characters() {
        assert_eq!(join_pointer("", "tag"), "/tag");
        assert_eq!(join_pointer("/a", "b~c"), "/a/b~0c");
        assert_eq!(join_pointer("/a", "b/c"), "/a/b~1c");
        assert_eq!(join_index("/rules", 3), "/rules/3");
        assert_eq!(join_index("", 0), "/0");
    }

    #[test]
    fn a_displayed_error_names_where_it_failed() {
        let error = JsonError::new(
            "duration_contains_days",
            "a duration cannot contain days",
            "/rules/2/params/min",
        );
        assert_eq!(
            error.to_string(),
            "duration_contains_days at `/rules/2/params/min`: a duration cannot contain days"
        );
        assert_eq!(
            JsonError::new("invalid_json", "expected value", "").to_string(),
            "invalid_json: expected value"
        );
    }

    #[test]
    fn a_malformed_proposal_keeps_the_pointer_it_names() {
        let error = JsonError::from(opportune::Error::malformed_proposal(
            "proposal_part_unknown",
            "the goal has no part `nope`",
            "/events/0/part",
        ));
        assert_eq!(error.code(), "proposal_part_unknown");
        assert_eq!(
            error.path(),
            "/events/0/part",
            "the pointer into the proposal the caller wrote is what names the field"
        );
        assert_eq!(error.errors(), &[]);
    }

    #[test]
    fn a_rejected_request_keeps_its_findings() {
        let findings = vec![
            Diagnostic::new("zone_unknown", "unknown time zone `Mars/Olympus_Mons`")
                .with_path("/participants/0/zone"),
        ];
        let error = JsonError::from(opportune::Error::invalid_request(findings.clone()));
        assert_eq!(error.code(), "invalid_request");
        assert_eq!(error.errors(), findings.as_slice());
        assert_eq!(error.path(), "", "the pointers live on the findings");
    }

    #[test]
    fn a_time_failure_keeps_its_code_and_message() {
        let rejected: opportune::time::TimeError =
            opportune::time::Instant::from_unix_seconds(0).expect_err("the epoch is out of range");
        let code = rejected.code();
        let message = rejected.to_string();
        let error = JsonError::from(rejected);
        assert_eq!(error.code(), code);
        assert_eq!(
            error.message(),
            message,
            "the message is the failure's own sentence, not a restatement of a Rust type",
        );
        assert_eq!(
            error.path(),
            "",
            "no field is at fault: the value was built, not read",
        );
        assert_eq!(error.errors(), &[]);
    }

    #[test]
    fn a_duplicate_field_names_the_object_that_repeated_it() {
        // A nested duplicate names the object, exactly as an unknown key there
        // does, so a handler points a client at the object it has to fix rather
        // than at the value inside it.
        let error = crate::parse_request(
            r#"{ "now": "2027-03-09T15:00Z",
                 "goal": { "parts": [{ "duration": "30m",
                                       "requirement": { "all": [{ "person": "ana" }] } }],
                           "search_window": ["2027-03-09T15:00Z", "2027-03-09T16:00Z"],
                           "zone": "UTC" },
                 "participants": [{ "id": "ana", "id": "bea", "zone": "UTC" }] }"#,
        )
        .unwrap_err();
        assert_eq!(error.code(), "duplicate_field");
        assert!(
            error.message().contains("`id`"),
            "the message names the key: {error}",
        );
        assert_eq!(error.path(), "/participants/0");

        // A duplicate at the document root names the document.
        let error =
            crate::parse_calendar(r#"{ "zone": "UTC", "zone": "Europe/London" }"#).unwrap_err();
        assert_eq!(error.code(), "duplicate_field");
        assert_eq!(error.path(), "");

        // And an unknown key beside it names the same object, so the two shape
        // failures agree on where the caller has to look.
        let unknown = crate::parse_request(
            r#"{ "now": "2027-03-09T15:00Z",
                 "goal": { "parts": [{ "duration": "30m",
                                       "requirement": { "all": [{ "person": "ana" }] } }],
                           "search_window": ["2027-03-09T15:00Z", "2027-03-09T16:00Z"],
                           "zone": "UTC" },
                 "participants": [{ "id": "ana", "zone": "UTC", "nickname": "a" }] }"#,
        )
        .unwrap_err();
        assert_eq!(unknown.code(), "unknown_field");
        assert_eq!(unknown.path(), "/participants/0");
    }

    #[test]
    fn an_empty_reference_token_is_a_pointer_segment() {
        // RFC 6901 allows the empty reference token, and a calendar name or a
        // participant ID may be empty in a map key, so `/calendars//zone` has to
        // be spelled rather than collapsed.
        assert_eq!(join_pointer("", ""), "/");
        assert_eq!(join_pointer("/calendars", ""), "/calendars/");
        assert_eq!(
            join_pointer(&join_pointer("/calendars", ""), "zone"),
            "/calendars//zone",
        );
        assert_eq!(
            join_pointer(&join_pointer("/weights", "a/b~c"), ""),
            "/weights/a~1b~0c/",
        );
    }
}
