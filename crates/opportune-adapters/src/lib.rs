//! Calendar payload conversions from Graph and Google sources into
//! [`opportune`] domain types.
//!
//! Adapters are a translation layer only: every payload becomes domain types, so an
//! adapter cannot change what the engine means. They perform no network and no
//! filesystem I/O and read no system clock, so a recorded payload converts to the
//! same blocks on every platform, `wasm32` included.
//!
//! # The contract
//!
//! Every provider returns the same shapes: [`CalendarOutcome`] per calendar, with
//! [`AnnotatedBlock`]s and [`AdapterDiagnostic`]s when it converts and a stable
//! code when it does not. An errored calendar is never an empty block list,
//! because an empty list reads as "completely free". Free/busy sources convert
//! into [`CalendarOutcome`] too, but they carry no event IDs, categories or
//! all-day flag, so their blocks have no event key and are never `all_day`; rest
//! rules and category-based caps need an event source.
//!
//! [`zones::resolve_zone`] resolves the zone names payloads carry — IANA first,
//! then Windows names through the CLDR mapping (feature `windows-zones`), then a
//! small alias table — and returns the IANA name behind them, so a rule built from
//! a payload is resolvable by the engine. [`zones::category_to_tag`] turns a
//! provider category into a tag.
//!
//! # Providers
//!
//! - `graph` (feature `graph`): Microsoft Graph / Entra Outlook `calendarView`,
//!   `getSchedule` and `workingHours`.
//! - `google` (feature `google`): Google Calendar `events.list` and
//!   `freeBusy.query`.
//!
//! Each module's rustdoc carries the caller requirements its endpoint imposes —
//! the query parameters, the pagination, the chunking limits.
//!
//! # Markers
//!
//! Callers write their own data onto provider events under [`MARKER_TAGS`]
//! (space-separated tags) and [`MARKER_EVENT_KEY`]. With the `graph` feature on,
//! `graph::EXTENDED_PROPERTY_EXPAND` is the `$expand` clause that fetches Graph's
//! opportune-property-set markers.
//!
//! [`opportune`]: https://docs.rs/opportune

#[cfg(any(feature = "google", feature = "graph"))]
mod blocks;
#[cfg(feature = "google")]
pub mod google;
#[cfg(feature = "graph")]
pub mod graph;
#[cfg(feature = "json")]
pub mod json;
pub mod types;
pub mod zones;

pub use types::{
    AdapterDiagnostic, AdapterPayloadError, AdapterZoneError, AnnotatedBlock, CalendarOutcome,
    MARKER_EVENT_KEY, MARKER_TAGS, WorkingHoursError,
};
