//! The engine a JavaScript caller builds once, and the JSON entry points over it.
//!
//! [`Engine`] is the Rust-callable form of the `WasmEngine` exports: the same
//! delegation to the wire layer's shared facade, returning the layer's own
//! [`JsonError`] rather than a JavaScript value. The exports wrap it, so a failure is
//! readable here as a code, a pointer and its findings; the throw that carries it to
//! JavaScript is the one link only a wasm runtime can execute.
//!
//! Everything crosses as JSON, in both directions. The return values are JSON
//! documents rather than converted objects, because the TypeScript types are
//! generated from the schema the Rust side writes against, and a second
//! hand-written conversion would be a second spelling of the format to keep in
//! step.

use opportune::Engine as CoreEngine;
use opportune::Limits;
use opportune::time::TimeZoneDatabase;
use opportune_json::{JsonError, WireLimits, parse_limits, to_json};

/// An engine: the bundled time zone database and the request limits, with the
/// request-to-response entry points as JSON.
///
/// There is no `tzdb_dir` here. A browser has no filesystem to read a `TZif`
/// tree from, so the database is always the one compiled into the module; the
/// version it reports is what a response's `meta.tzdb` carries.
pub struct Engine {
    engine: CoreEngine,
}

impl Engine {
    /// Builds an engine over the bundled database.
    ///
    /// `limits_json` holds the request-size limits as JSON, or is [`None`] for
    /// the engine's own defaults — the same document `default_limits_json`
    /// writes, with any of its fields changed.
    ///
    /// # Errors
    /// Returns [`JsonError`] when the database cannot be built, or when
    /// `limits_json` is not the document [`parse_limits`] reads: a missing
    /// limit, a key the shape does not name, or a value of the wrong type.
    pub fn new(limits_json: Option<&str>) -> Result<Self, JsonError> {
        let mut builder = CoreEngine::builder();
        if let Some(limits_json) = limits_json {
            builder = builder.limits(parse_limits(limits_json)?);
        }
        // The builder answers with the domain error, so the wire layer's own
        // conversion is what carries its code and message across: one translation
        // for every engine failure, not a hand-written one per call site.
        let engine = builder
            .build()
            .map_err(|err| JsonError::from(opportune::Error::from(err)))?;
        Ok(Self { engine })
    }

    /// The version of the time zone database this engine resolves zones against,
    /// as a response's `meta.tzdb` reports it.
    #[must_use]
    pub fn tzdb_version(&self) -> &str {
        self.engine.time_zone_database().version()
    }

    /// Offers: every candidate start the request's calendars, rules, caps and
    /// rest allow, as a JSON `Response`.
    ///
    /// # Errors
    /// Returns [`JsonError`] when the document is not a request, or when the
    /// request is invalid: a rejected request carries every finding its
    /// validation produced in [`errors`](JsonError::errors), each with the RFC
    /// 6901 pointer of the request field it is about.
    pub fn find_availability_json(&self, request_json: &str) -> Result<String, JsonError> {
        opportune_json::find_availability(&self.engine, request_json)
    }

    /// Judges one proposed booking against the same rules and calendars, as a
    /// JSON `SlotCheck`.
    ///
    /// # Errors
    /// Returns [`JsonError`] when either document is rejected. A proposal that
    /// does not fit the request — an unknown part, the wrong number of terms or
    /// hosts, a duplicate host — names the proposal field at fault in
    /// [`path`](JsonError::path), such as `/events/0/assignment`.
    pub fn check_slot_json(
        &self,
        request_json: &str,
        proposal_json: &str,
    ) -> Result<String, JsonError> {
        opportune_json::check_slot(&self.engine, request_json, proposal_json)
    }

    /// The request's static errors and warnings, as a JSON `Diagnostics`.
    ///
    /// # Errors
    /// Returns [`JsonError`] when the document is not a request.
    pub fn validate_json(&self, request_json: &str) -> Result<String, JsonError> {
        opportune_json::validate(&self.engine, request_json)
    }

    /// The calendar and occurrence data a ranges query needs, as a JSON
    /// `RequiredRanges`.
    ///
    /// The normal path is an initial fetch — every block whose occupied span, its
    /// own span widened by its travel, overlaps `calendar`, plus every occurrence
    /// and block whose own span overlaps `occurrences` — followed by event-key
    /// closure: observe the keys on every participant's own-span occurrence-range
    /// records, then fetch every record carrying one of those keys on every
    /// participant. Occurrences are one request-global list, never filtered by the
    /// participants they name.
    ///
    /// The hulls cover only instants inside the query's goal search window, so a
    /// confirmation outside it needs a new query whose window contains the proposed
    /// start and the same initial fetch and event-key closure. When exact closure is
    /// impossible, fetch the full supported time domain with limits raised to cover
    /// it (`max_blocks_per_participant` and `max_occurrences`) or fail closed by
    /// returning an error rather than evaluating narrowed data. See `RequiredRanges`
    /// in `opportune` for the whole contract.
    ///
    /// # Errors
    /// Returns [`JsonError`] when the query document is rejected.
    pub fn required_ranges_json(&self, query_json: &str) -> Result<String, JsonError> {
        opportune_json::required_ranges(&self.engine, query_json)
    }

    /// The database the adapter conversions in [`crate::adapters`] read payload
    /// zone names against.
    ///
    /// It is this engine's database, so a payload and the evaluation it feeds are
    /// read in one database.
    pub(crate) fn time_zone_database(&self) -> &TimeZoneDatabase {
        self.engine.time_zone_database()
    }
}

/// The engine's own default limits as the JSON document a caller starts from.
///
/// The document is built from [`opportune::Limits::default`] rather than written
/// out here, so the defaults a caller reads are the ones the engine applies to a
/// request that names no limits at all.
///
/// # Errors
/// Returns [`JsonError`] only if the document cannot be written, which the
/// engine's own limits never fail to be.
pub fn default_limits_json() -> Result<String, JsonError> {
    let limits = Limits::default();
    to_json(&WireLimits::from(&limits))
}
