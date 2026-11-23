//! The one payload a failure crosses into JavaScript as.
//!
//! A JavaScript caller cannot receive a Rust type across the boundary, and every
//! failure this crate reports is a [`JsonError`]: a stable code, a message, the
//! RFC 6901 pointer of the field at fault, and the findings a rejected request
//! carries. Written as one JSON document and thrown as a string, the TypeScript
//! layer rebuilds an `OpportuneError` holding the same four fields, so an
//! application answers its own caller with the pointer into the document it sent
//! rather than with a restatement of a Rust type name.
//!
//! ```json
//! {
//!   "code": "invalid_request",
//!   "message": "the request is invalid: zone_unknown: unknown time zone `Mars/Olympus_Mons`",
//!   "path": null,
//!   "errors": [
//!     { "code": "zone_unknown", "message": "unknown time zone `Mars/Olympus_Mons`",
//!       "path": "/participants/0/zone" }
//!   ]
//! }
//! ```
//!
//! `path` is `null` — not `""` — when the failure is about the document as a
//! whole, because a pointer a caller cannot dereference is better spelled the way
//! its own language spells absence. Each finding carries its own pointer, or
//! `null` when it is not about one field.

use opportune::Diagnostic;
use opportune_json::JsonError;
use wasm_bindgen::JsValue;

/// The structured JSON document a failed call throws.
///
/// Every field is always present, so a caller reads `code`, `message`, `path` and
/// `errors` without checking whether the payload carried them.
///
/// The object is written directly with [`serde_json::to_string`] for string
/// escaping rather than through `serde_json::Value` / `json!`, which keeps
/// `serde_json::Value` out of the size-budgeted WASM binary while emitting the
/// exact same sorted-key JSON bytes.
///
/// ```
/// use opportune_json::JsonError;
/// use opportune_wasm::error::payload;
///
/// let error = JsonError::new("invalid_json", "expected value", "");
/// assert_eq!(
///     payload(&error),
///     r#"{"code":"invalid_json","errors":[],"message":"expected value","path":null}"#,
/// );
/// ```
#[must_use]
pub fn payload(error: &JsonError) -> String {
    let mut out = String::with_capacity(96);
    out.push_str(r#"{"code":"#);
    push_json_str(&mut out, error.code());
    out.push_str(r#","errors":["#);
    for (index, diagnostic) in error.errors().iter().enumerate() {
        if index > 0 {
            out.push(',');
        }
        push_finding(&mut out, diagnostic);
    }
    out.push_str(r#"],"message":"#);
    push_json_str(&mut out, error.message());
    out.push_str(r#","path":"#);
    push_json_opt_str(&mut out, pointer(error.path()));
    out.push('}');
    out
}

/// `error` as the value a WASM export throws.
pub(crate) fn rejection(error: &JsonError) -> JsValue {
    JsValue::from_str(&payload(error))
}

/// The pointer `path`, or [`None`] when it names no field.
///
/// [`JsonError`] spells "the document as a whole" as `""`, and a caller cannot
/// dereference an empty pointer, so it crosses as `null`.
fn pointer(path: &str) -> Option<&str> {
    if path.is_empty() { None } else { Some(path) }
}

/// Appends one request finding as a sorted-key JSON object.
fn push_finding(out: &mut String, diagnostic: &Diagnostic) {
    out.push_str(r#"{"code":"#);
    push_json_str(out, diagnostic.code());
    out.push_str(r#","message":"#);
    push_json_str(out, diagnostic.message());
    out.push_str(r#","path":"#);
    push_json_opt_str(out, diagnostic.path());
    out.push('}');
}

/// Appends `value` as a JSON string literal.
fn push_json_str(out: &mut String, value: &str) {
    let encoded = serde_json::to_string(value).expect("a UTF-8 string always serializes as JSON");
    out.push_str(&encoded);
}

/// Appends `value` as a JSON string literal, or `null` when absent.
fn push_json_opt_str(out: &mut String, value: Option<&str>) {
    match value {
        Some(text) => push_json_str(out, text),
        None => out.push_str("null"),
    }
}
