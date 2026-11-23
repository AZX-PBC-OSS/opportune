//! The shared adapter contract: what a provider payload becomes, or why it could
//! not be converted.
//!
//! Every provider adapter returns the same two shapes, so a caller that fetches
//! from Graph today and Google tomorrow reads one contract. A calendar either
//! converts into annotated blocks plus per-event diagnostics, or comes back
//! [`CalendarOutcome::Unavailable`] with a stable code. An errored calendar is
//! never an empty block list: an empty list would read as "completely free".
//!
//! A block carries the provider's own event ID beside it rather than in it, so a
//! caller can apply its own conventions — its booking IDs, its CRM's fields — to
//! the blocks of events it recognises without the adapter knowing anything about
//! them.
//!
//! # Markers
//!
//! Callers write their own data onto provider events under two names, on both
//! providers: [`MARKER_TAGS`] (space-separated tags) and [`MARKER_EVENT_KEY`].
//! Google reads them from the event's shared extended properties, Graph from
//! single-value extended properties in opportune's own property set — the `graph`
//! module's `EXTENDED_PROPERTY_EXPAND` is the `$expand` clause that fetches them.
//!
//! # Codes
//!
//! The codes are stable: callers match on them across releases.
//!
//! [`CalendarOutcome::Unavailable`] and [`WorkingHoursError`]:
//!
//! | Code | Meaning |
//! |---|---|
//! | `payload_invalid` | The payload is not the document the adapter asked for, or an event in it cannot become a block. |
//! | `provider_error` | The provider reported an error for one calendar inside a successful response. |
//! | `unexpanded_series` | A recurring series arrived unexpanded, so the caller queried the wrong endpoint. |
//! | `zone_unknown` | A zone name the payload carries is neither IANA, a Windows name this build maps, nor an alias. |
//! | `custom_time_zone` | The working hours use Graph's custom-zone form (a bias and daylight offset), which is not a name. |
//! | `invalid_working_hours` | The working hours' days, times or range cannot describe a window. |
//!
//! [`AdapterDiagnostic`]:
//!
//! | Code | Meaning |
//! |---|---|
//! | `zero_length_event` | The event starts and ends at the same instant, so it was dropped. |
//! | `end_time_unspecified` | Google reported `endTimeUnspecified`, so the end it still sent was kept as given. |
//! | `attendees_omitted` | Google omitted the attendee list, so the user's own invitation response is unknown and the event reads as busy unless it is transparent. |
//! | `marker_tag_invalid` | A token in a marker tag list is not a tag path, so it was left out. |
//! | `category_tag_unmappable` | A category name has no ASCII slug, so it made no tag and a `category/...` policy never matches it. |

use opportune::model::Block;
use thiserror::Error;

/// The extended property, category or shared-property name a caller writes tags
/// into: a space-separated list of tag paths.
pub const MARKER_TAGS: &str = "opportune.tags";

/// The marker name a caller writes its own event key into. It takes precedence
/// over the provider's own identifier, which is what lets a caller recognise
/// events it created on a calendar it does not own.
pub const MARKER_EVENT_KEY: &str = "opportune.event_key";

/// One converted block, with the provider's ID for the event beside it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AnnotatedBlock {
    /// The provider's ID for the event, when the source carries one. Free/busy
    /// sources do not: they report spans, not events.
    pub event_id: Option<String>,
    /// The block itself.
    pub block: Block,
}

/// Something a caller should know about one event that did not stop it
/// converting: a zero-length event that was dropped, or a marker tag that is not
/// a tag.
///
/// A diagnostic never fails a calendar. A payload that cannot be converted at all
/// is [`CalendarOutcome::Unavailable`] instead, because a caller's recovery —
/// retry, fall back, ask for another permission — depends on the reason.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AdapterDiagnostic {
    /// The stable, machine-readable code.
    pub code: &'static str,
    /// The provider's ID for the event the finding is about, when there is one.
    pub event_id: Option<String>,
    /// A sentence naming the value and why it was left out.
    pub message: String,
}

impl AdapterDiagnostic {
    /// A finding about the event `event_id`.
    #[must_use]
    pub fn for_event(
        code: &'static str,
        event_id: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            code,
            event_id: Some(event_id.into()),
            message: message.into(),
        }
    }

    /// A finding about a span whose source reports no event identity, such as a
    /// free/busy item.
    #[must_use]
    pub fn for_span(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            event_id: None,
            message: message.into(),
        }
    }
}

/// What one participant's calendar converted into.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CalendarOutcome {
    /// The calendar converted. Blocks are in provider order; the engine sorts what
    /// it needs.
    Available {
        /// The converted blocks.
        blocks: Vec<AnnotatedBlock>,
        /// Per-event findings that did not stop the conversion.
        diagnostics: Vec<AdapterDiagnostic>,
    },
    /// The calendar could not be converted, so whether the participant is free is
    /// unknown rather than answered with an empty list.
    Unavailable {
        /// A stable, machine-readable code.
        code: &'static str,
        /// A sentence naming what could not be converted and why.
        message: String,
    },
}

/// Why a zone name a payload carries could not be resolved.
///
/// The name is tried as an IANA name, then — with the `windows-zones` feature —
/// as a Windows name through the CLDR mapping, then against the adapter's small
/// alias table.
#[derive(Clone, Debug, PartialEq, Eq, Error)]
#[non_exhaustive]
pub enum AdapterZoneError {
    /// No database entry, Windows mapping or alias covers the name.
    #[error("the time zone `{name}` cannot be resolved")]
    Unresolved {
        /// The name as the payload wrote it.
        name: String,
    },
}

impl AdapterZoneError {
    /// The stable, machine-readable code: [`opportune`]'s own `zone_unknown`, so a
    /// caller sees one vocabulary across the wire layer and the adapters.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::Unresolved { .. } => "zone_unknown",
        }
    }
}

/// Why a mailbox's `workingHours` could not become a `window` rule.
///
/// The failure is confined to the working hours: blocks converted from the same
/// `getSchedule` response are unaffected, so a caller whose mailbox reports a
/// custom time zone falls back to its own window rather than losing availability.
#[derive(Clone, Debug, PartialEq, Eq, Error)]
#[non_exhaustive]
pub enum WorkingHoursError {
    /// Graph reported a `#microsoft.graph.customTimeZone` rather than a named
    /// zone, which this adapter does not resolve.
    #[error(
        "the working hours use a custom Graph time zone, which this adapter does not resolve: \
         fall back to the caller's own window"
    )]
    CustomTimeZone,
    /// The named zone is neither an IANA name, a Windows name this build maps, nor
    /// an alias.
    #[error("the working hours name the time zone `{name}`, which cannot be resolved")]
    UnresolvedZone {
        /// The name as the payload wrote it.
        name: String,
    },
    /// The days, times or range the payload reports cannot describe a window. The
    /// message carries the underlying reason.
    #[error("the working hours cannot describe a window: {message}")]
    InvalidWorkingHours {
        /// What about the working hours could not be represented.
        message: String,
    },
}

impl WorkingHoursError {
    /// The stable, machine-readable code. [`Self::CustomTimeZone`] and
    /// [`Self::UnresolvedZone`] are the codes a caller switches on.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::CustomTimeZone => "custom_time_zone",
            Self::UnresolvedZone { .. } => "zone_unknown",
            Self::InvalidWorkingHours { .. } => "invalid_working_hours",
        }
    }
}

/// Why a provider response could not be read at all.
///
/// A payload that is not the document the adapter asked for has no per-calendar
/// answer to give, so it fails the whole call. Per-calendar failures inside a
/// readable response — Graph's `scheduleInformation.error` — are
/// [`CalendarOutcome::Unavailable`] instead.
#[derive(Clone, Debug, PartialEq, Eq, Error)]
#[non_exhaustive]
pub enum AdapterPayloadError {
    /// The document is not readable as the named provider response.
    #[error("the {shape} payload is not readable: {message}")]
    InvalidPayload {
        /// The provider response the adapter expected.
        shape: &'static str,
        /// What was wrong with the document.
        message: String,
    },
}

impl AdapterPayloadError {
    /// The stable, machine-readable code.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::InvalidPayload { .. } => "payload_invalid",
        }
    }

    /// The failure for a document that is not readable as `shape`.
    ///
    /// Only a compiled-in provider reads documents, so it is absent from a build
    /// with every provider feature off.
    #[cfg(any(feature = "google", feature = "graph"))]
    pub(crate) fn invalid(shape: &'static str, message: impl Into<String>) -> Self {
        Self::InvalidPayload {
            shape,
            message: message.into(),
        }
    }
}
