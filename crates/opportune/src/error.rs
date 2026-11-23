//! Typed failures and the diagnostics `validate` reports.
//!
//! Every failure carries a stable [`Error::code`], so a caller can match on it
//! across releases; the display text is for humans and may change.

use thiserror::Error;

use crate::intervals::IntervalError;
use crate::time::{Instant, TimeError};

/// What `validate` found in a request.
///
/// Errors are reserved for requests that are structurally impossible: more than
/// one part, an unresolvable zone, a limit exceeded. Anything that depends on
/// `now`, on date overrides or on calendars produces empty results and a
/// warning instead, because it may well be answerable tomorrow.
///
/// ```
/// use opportune::error::Diagnostics;
///
/// let diagnostics = Diagnostics::default();
/// assert!(diagnostics.is_valid());
/// assert!(diagnostics.errors.is_empty());
/// ```
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Diagnostics {
    /// Everything that makes the request impossible.
    pub errors: Vec<Diagnostic>,
    /// Everything the caller should know that does not make it impossible.
    pub warnings: Vec<Diagnostic>,
}

impl Diagnostics {
    /// Whether the request holds no errors.
    #[must_use]
    pub fn is_valid(&self) -> bool {
        self.errors.is_empty()
    }
}

/// One validation finding: a stable code and a sentence naming this occurrence.
///
/// A finding that points at one place in the request carries its RFC 6901 JSON
/// pointer too, so a wire layer or an editor can open the exact field:
///
/// ```
/// use opportune::Diagnostic;
///
/// let diagnostic = Diagnostic::new("zone_unknown", "unknown time zone `Mars/Olympus_Mons`")
///     .with_path("/participants/0/zone");
/// assert_eq!(diagnostic.path(), Some("/participants/0/zone"));
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diagnostic {
    code: &'static str,
    message: String,
    path: Option<String>,
}

impl Diagnostic {
    /// A finding with a stable code and a human-readable message.
    #[must_use]
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            path: None,
        }
    }

    /// The stable, machine-readable code.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        self.code
    }

    /// The human-readable message, naming the inputs that identify this
    /// occurrence.
    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }

    /// The RFC 6901 JSON pointer of the request field this finding is about,
    /// when it is about one field.
    #[must_use]
    pub fn path(&self) -> Option<&str> {
        self.path.as_deref()
    }

    /// The finding pointing at `path`, an RFC 6901 JSON pointer into the
    /// request.
    #[must_use]
    pub fn with_path(mut self, path: impl Into<String>) -> Self {
        self.path = Some(path.into());
        self
    }
}

impl core::fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

/// Why a request or a model value was rejected.
///
/// The [`Error::code`] values are stable: callers match on them across releases.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[non_exhaustive]
pub enum Error {
    /// A civil-time value or operation failed.
    #[error(transparent)]
    Time(TimeError),
    /// A half-open minute span could not be built.
    #[error(transparent)]
    Intervals(IntervalError),
    /// A time zone window was rejected as structurally ambiguous.
    #[error(transparent)]
    Window(crate::time::WindowError),
    /// A time zone blackout was rejected as empty.
    #[error(transparent)]
    Blackout(crate::time::BlackoutError),
    /// A tag path was rejected.
    #[error(transparent)]
    Tag(crate::tags::TagError),
    /// A span that covers no minute: its start equals its end.
    #[error("a span must cover at least one minute, but {start} to {end} covers none")]
    SpanEmpty {
        /// The rejected start.
        start: Instant,
        /// The rejected end.
        end: Instant,
    },
    /// A span whose start is after its end.
    #[error("a span must end after it starts, but {start} is after {end}")]
    SpanReversed {
        /// The rejected start.
        start: Instant,
        /// The rejected end.
        end: Instant,
    },
    /// A participant zone period that covers no instant.
    #[error("zone period is empty: {from} is not before {until}")]
    ZonePeriodEmpty {
        /// The rejected period's start.
        from: Instant,
        /// The rejected period's end.
        until: Instant,
    },
    /// An occurrence nobody attends, which cannot be counted for anybody.
    #[error("occurrence `{event_key}` names no participants")]
    OccurrenceWithoutParticipants {
        /// The rejected occurrence's key.
        event_key: String,
    },
    /// An event key that names no event.
    ///
    /// Two events sharing a key are one event, so an empty or whitespace-only key
    /// would merge every event written with it into one cap count.
    #[error("event key `{event_key}` is empty or whitespace, so it names no event")]
    EventKeyEmpty {
        /// The rejected key, as written.
        event_key: String,
    },
    /// A status-policy entry that matches neither a status nor a tag selector,
    /// so it could never apply.
    #[error("a status policy entry must match a status, a tag selector, or both")]
    StatusPolicyEntryEmpty,
    /// A request `validate` rejected.
    #[error("the request is invalid: {}", DisplayFirst(.errors))]
    InvalidRequest {
        /// The invalid request's errors, in the order validation found them.
        errors: Vec<Diagnostic>,
    },
    /// A goal that does not have exactly one part, which version 1 requires.
    #[error(
        "version 1 books exactly one part per request, but the goal has {parts}: model each \
         stage as its own request and pass the gap between them as the next request's \
         `search_window` or `candidates`"
    )]
    PartCount {
        /// The number of parts the goal holds.
        parts: usize,
    },
    /// A proposal that does not fit the request it was checked against.
    #[error("malformed proposal: {message}")]
    MalformedProposal {
        /// A stable code naming the shape failure. The set is
        /// `proposal_part_unknown`, `proposal_part_duplicated`, `proposal_part_missing`,
        /// `proposal_term_count`, `proposal_host_count`, `proposal_host_unknown`,
        /// `proposal_host_attendee`, `proposal_host_mismatch`, `proposal_host_duplicated`
        /// and `proposal_out_of_range`, so a caller can match on it without reading
        /// the validator.
        code: &'static str,
        /// What is wrong, naming the part, term or participant.
        message: String,
        /// The RFC 6901 JSON pointer into the proposal of the field that does
        /// not fit, when one field is at fault.
        path: Option<String>,
    },
}

impl Error {
    /// The request `validate` rejected, built from its errors.
    ///
    /// When `errors` is empty, `Display` falls back to a generic sentence
    /// rather than panicking.
    #[must_use]
    pub const fn invalid_request(errors: Vec<Diagnostic>) -> Self {
        Self::InvalidRequest { errors }
    }

    /// A proposal that does not fit the request it was checked against, with
    /// the JSON pointer of the field that does not fit.
    #[must_use]
    pub fn malformed_proposal(
        code: &'static str,
        message: impl Into<String>,
        path: impl Into<String>,
    ) -> Self {
        Self::MalformedProposal {
            code,
            message: message.into(),
            path: Some(path.into()),
        }
    }

    /// The RFC 6901 JSON pointer of the field a failure is about, when one field
    /// is at fault.
    ///
    /// A malformed proposal's pointer is into the proposal. A rejected request
    /// keeps its [`Diagnostic`] findings in [`Error::InvalidRequest`], and each of
    /// those carries its own pointer, so this method answers `None` for it.
    #[must_use]
    pub fn path(&self) -> Option<&str> {
        match self {
            Self::MalformedProposal { path, .. } => path.as_deref(),
            _ => None,
        }
    }

    /// A stable, machine-readable code for this failure.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::Time(err) => err.code(),
            Self::Intervals(err) => err.code(),
            Self::Window(err) => err.code(),
            Self::Blackout(err) => err.code(),
            Self::Tag(err) => err.code(),
            Self::SpanEmpty { .. } => "span_empty",
            Self::SpanReversed { .. } => "span_reversed",
            Self::ZonePeriodEmpty { .. } => "zone_period_empty",
            Self::OccurrenceWithoutParticipants { .. } => "occurrence_without_participants",
            Self::EventKeyEmpty { .. } => "event_key_empty",
            Self::StatusPolicyEntryEmpty => "status_policy_entry_empty",
            Self::InvalidRequest { .. } => "invalid_request",
            Self::PartCount { .. } => "part_count",
            Self::MalformedProposal { code, .. } => code,
        }
    }
}

impl From<TimeError> for Error {
    fn from(err: TimeError) -> Self {
        Self::Time(err)
    }
}

impl From<IntervalError> for Error {
    fn from(err: IntervalError) -> Self {
        Self::Intervals(err)
    }
}

impl From<crate::tags::TagError> for Error {
    fn from(err: crate::tags::TagError) -> Self {
        Self::Tag(err)
    }
}

impl From<crate::time::WindowError> for Error {
    fn from(err: crate::time::WindowError) -> Self {
        Self::Window(err)
    }
}

impl From<crate::time::BlackoutError> for Error {
    fn from(err: crate::time::BlackoutError) -> Self {
        Self::Blackout(err)
    }
}

/// Formats the first diagnostic of an [`Error::InvalidRequest`] without
/// duplicating its message as a second stored field.
struct DisplayFirst<'a>(&'a [Diagnostic]);

impl std::fmt::Display for DisplayFirst<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.0.first() {
            Some(first) => first.fmt(f),
            None => f.write_str("the request failed validation"),
        }
    }
}

/// Escapes one reference token of an RFC 6901 JSON pointer.
///
/// A pointer is `/`-separated, so a token that is itself a key or ID containing
/// `/` or `~` has to escape them: `~` becomes `~0` and `/` becomes `~1`.
/// Participant IDs and calendar names are opaque strings, so both spellings can
/// occur in a path.
pub(crate) fn escape_pointer_token(token: &str) -> String {
    let mut escaped = String::with_capacity(token.len());
    for character in token.chars() {
        match character {
            '~' => escaped.push_str("~0"),
            '/' => escaped.push_str("~1"),
            other => escaped.push(other),
        }
    }
    escaped
}

#[cfg(test)]
mod tests {
    use super::Error;

    #[test]
    fn invalid_request_with_no_diagnostics_does_not_panic() {
        let err = Error::invalid_request(Vec::new());
        assert_eq!(err.code(), "invalid_request");
        assert!(!err.to_string().is_empty());
    }
}
