//! The engine a Python caller builds once and shares across worker threads.
//!
//! `_Engine` is the whole wire surface: an application builds one at startup from
//! its configuration and every thread uses it. It holds an [`opportune::Engine`]
//! and nothing else, so it is `Sync`: two threads calling
//! `find_availability_json` at the same time share the time zone database's
//! immutable state and contend on no lock. Each call releases the GIL for the
//! parse, the evaluation and the serialisation, so the callers genuinely run in
//! parallel rather than taking turns.
//!
//! Everything crosses as JSON, in both directions. The return values are JSON
//! documents — `str` in Python — rather than generated Python objects, because
//! the typed Pydantic layer is generated from the same schema the Rust side
//! writes against, and a second hand-written conversion here would be a second
//! spelling of the format to keep in step.
//!
//! The Graph and Google conversions are methods here too, even though they read
//! no request state: a caller then reaches one object for the whole wire surface,
//! and passes the engine it already holds. They resolve the zone names a payload
//! carries against this engine's database, so an engine built from a `tzdb_dir`
//! converts provider payloads in the same database it later evaluates against.
//!
//! Every string argument is read through [`error::document`] or [`error::argument`]
//! rather than by `PyO3`'s own extraction, so a lone surrogate in any of them is
//! the extension's `OpportuneError` and never a bare `UnicodeEncodeError`.

use opportune::Engine as CoreEngine;
use opportune_json::{JsonError, WireLimits, parse_limits, to_json};
use pyo3::prelude::*;
use pyo3::types::PyString;

use crate::error;

/// An engine: the time zone database and the request limits, built once and
/// shared by every thread.
///
/// `tzdb_dir` names a directory of `TZif` files to resolve zones against, or is
/// `None` for the database bundled into the extension. `tzdb_version` labels that
/// directory when it carries no `+VERSION` file; passing one without `tzdb_dir`
/// raises `OpportuneError`, because the bundled database reports its own version.
/// `limits_json` holds the request-size limits as JSON, or is `None` for the
/// engine's defaults.
///
/// Raises `OpportuneError` when the database cannot be built, the version is
/// missing, the limits are not the shape the engine reads, or a version label is
/// given with no directory to label.
///
/// The four query methods answer from this engine, so `tzdb_dir` and
/// `limits_json` apply to them. The Graph and Google conversions resolve the zone
/// names a payload carries against this engine's database too, so a custom
/// directory is used for adapters and evaluation alike. The one exception is
/// Google `freeBusy`, whose ranges carry their own offsets and read no zone name.
#[pyclass(frozen, name = "_Engine", module = "opportune._opportune")]
pub(crate) struct Engine {
    engine: CoreEngine,
}

#[pymethods]
impl Engine {
    #[new]
    #[pyo3(signature = (tzdb_dir=None, tzdb_version=None, limits_json=None))]
    fn new(
        py: Python<'_>,
        tzdb_dir: Option<&Bound<'_, PyString>>,
        tzdb_version: Option<&Bound<'_, PyString>>,
        limits_json: Option<&Bound<'_, PyString>>,
    ) -> PyResult<Self> {
        let tzdb_dir = tzdb_dir.map(error::argument).transpose()?;
        let tzdb_version = tzdb_version.map(error::argument).transpose()?;
        // Building from a directory reads several hundred files, so the release
        // matters here as it does for a search: a server starting one engine must
        // not stall every thread it has already accepted.
        let limits = limits_json.map(error::document).transpose()?;
        let engine = error::detach(py, || build(tzdb_dir, tzdb_version, limits))?;
        Ok(Self { engine })
    }

    /// The version of the time zone database this engine resolves zones against,
    /// as `meta.tzdb` reports it.
    #[getter]
    fn tzdb_version(&self) -> &str {
        self.engine.time_zone_database().version()
    }

    /// The engine as a debugger or a log line shows it: its type and the version
    /// of the time zone database it answers with.
    fn __repr__(&self) -> String {
        format!(
            "_Engine(tzdb_version={:?})",
            self.engine.time_zone_database().version()
        )
    }

    /// Offers: every candidate start the request's calendars, rules, caps and
    /// rest allow, as a JSON `Response`.
    ///
    /// `request_json` is the request as JSON. Raises `OpportuneError` when the
    /// document is not a request, or the request is invalid: a rejected request
    /// carries every finding its validation produced in `.errors`, each with the
    /// RFC 6901 pointer of the request field it is about.
    fn find_availability_json(
        &self,
        py: Python<'_>,
        request_json: &Bound<'_, PyString>,
    ) -> PyResult<String> {
        let request_json = error::document(request_json)?;
        error::detach(py, || {
            opportune_json::find_availability(&self.engine, request_json)
        })
    }

    /// Judges one proposed booking against the same rules and calendars, as a
    /// JSON `SlotCheck`.
    ///
    /// `request_json` is the request the proposal was made against and
    /// `proposal_json` the proposed booking. Raises `OpportuneError` when either
    /// document is rejected. A proposal that does not fit the request — an unknown
    /// part, the wrong number of terms or hosts, a duplicate host — names the
    /// proposal field at fault in `path`, such as `/events/0/assignment`.
    fn check_slot_json(
        &self,
        py: Python<'_>,
        request_json: &Bound<'_, PyString>,
        proposal_json: &Bound<'_, PyString>,
    ) -> PyResult<String> {
        let request_json = error::document(request_json)?;
        let proposal_json = error::document(proposal_json)?;
        error::detach(py, || {
            opportune_json::check_slot(&self.engine, request_json, proposal_json)
        })
    }

    /// The request's static errors and warnings, as a JSON `Diagnostics`.
    ///
    /// `request_json` is the request as JSON. Raises `OpportuneError` when the
    /// document is not a request.
    fn validate_json(
        &self,
        py: Python<'_>,
        request_json: &Bound<'_, PyString>,
    ) -> PyResult<String> {
        let request_json = error::document(request_json)?;
        error::detach(py, || opportune_json::validate(&self.engine, request_json))
    }

    /// The calendar and occurrence data a request needs, as a JSON
    /// `RequiredRanges`.
    ///
    /// `query_json` is the ranges query as JSON. The normal path is an initial
    /// fetch — blocks whose occupied span, their own span widened by their travel,
    /// overlaps `calendar`, plus every occurrence and block whose own span overlaps
    /// `occurrences` — followed by event-key closure: observe the keys on every
    /// participant's own-span occurrence-range records, then fetch every record
    /// carrying one of those keys on every participant. Occurrences are one
    /// request-global list, never filtered by the participants they name.
    ///
    /// The hulls cover only the query's goal search window, so a confirmation
    /// outside it needs a new query whose window contains the proposed start and
    /// the same initial fetch and event-key closure. When exact closure is
    /// impossible, fetch the full supported time domain with limits raised to cover
    /// it (`max_blocks_per_participant` and `max_occurrences`) or fail closed by
    /// returning an error rather than evaluating narrowed data. See
    /// `RequiredRanges` in the Rust documentation for the whole contract.
    ///
    /// Raises `OpportuneError` when the document is rejected.
    fn required_ranges_json(
        &self,
        py: Python<'_>,
        query_json: &Bound<'_, PyString>,
    ) -> PyResult<String> {
        let query_json = error::document(query_json)?;
        error::detach(py, || {
            opportune_json::required_ranges(&self.engine, query_json)
        })
    }

    /// Converts a Graph `calendarView` response, as a JSON `CalendarOutcome`.
    ///
    /// `payload_json` is the Graph response, `participant_zone` the mailbox
    /// owner's IANA zone that all-day events are read in, and `category_map_json`
    /// an optional JSON object mapping a category name to a tag path. Zone names
    /// resolve against this engine's database, so a payload and the evaluation it
    /// feeds are read in one database.
    ///
    /// Raises `OpportuneError` when the participant zone is blank, the category
    /// map is not an object of tag paths, or the engine's time zone database is
    /// unusable. A payload this adapter cannot read is an `unavailable` outcome in
    /// the result, not an exception.
    #[pyo3(signature = (payload_json, participant_zone, category_map_json=None))]
    fn graph_calendar_view_to_blocks_json(
        &self,
        py: Python<'_>,
        payload_json: &Bound<'_, PyString>,
        participant_zone: &Bound<'_, PyString>,
        category_map_json: Option<&Bound<'_, PyString>>,
    ) -> PyResult<String> {
        let payload_json = error::document(payload_json)?;
        let participant_zone = error::argument(participant_zone)?;
        let category_map_json = category_map_json.map(error::document).transpose()?;
        error::detach(py, || {
            opportune_adapters::json::calendar_view_to_blocks_with_db(
                self.engine.time_zone_database(),
                payload_json,
                participant_zone,
                category_map_json,
            )
        })
    }

    /// Converts a Graph `getSchedule` response, as JSON outcomes keyed by
    /// `scheduleId`.
    ///
    /// Raises `OpportuneError` when the document is not a `getSchedule` response.
    /// A schedule the provider reported an error for is an `unavailable` outcome
    /// inside the object, not an exception.
    fn graph_get_schedule_to_blocks_json(
        &self,
        py: Python<'_>,
        payload_json: &Bound<'_, PyString>,
    ) -> PyResult<String> {
        let payload_json = error::document(payload_json)?;
        error::detach(py, || {
            opportune_adapters::json::get_schedule_to_blocks_with_db(
                self.engine.time_zone_database(),
                payload_json,
            )
        })
    }

    /// Converts one mailbox's Graph `workingHours`, as a JSON
    /// `GraphWorkingHoursOutcome`.
    ///
    /// `payload_json` is the `workingHours` object and `rule_id` the ID the
    /// resulting `window` rule takes, defaulting to `working_hours`. Zone names
    /// resolve against this engine's database.
    ///
    /// Raises `OpportuneError` only when the engine's time zone database is
    /// unusable. A custom time zone is an `unavailable` outcome, so a caller falls
    /// back to its own window without catching an exception.
    #[pyo3(signature = (payload_json, rule_id=None))]
    fn graph_working_hours_to_rule_json(
        &self,
        py: Python<'_>,
        payload_json: &Bound<'_, PyString>,
        rule_id: Option<&Bound<'_, PyString>>,
    ) -> PyResult<String> {
        let payload_json = error::document(payload_json)?;
        let rule_id = match rule_id {
            Some(rule_id) => error::argument(rule_id)?,
            None => "working_hours",
        };
        error::detach(py, || {
            opportune_adapters::json::working_hours_to_rule_with_db(
                self.engine.time_zone_database(),
                payload_json,
                rule_id,
            )
        })
    }

    /// Converts a Google `events.list` response, as a JSON `CalendarOutcome`.
    ///
    /// `payload_json` is the Google response and `participant_zone` the calendar
    /// owner's IANA zone, read for all-day events when the response names no
    /// `timeZone` of its own. Zone names resolve against this engine's database.
    ///
    /// Raises `OpportuneError` when the participant zone is blank. A payload this
    /// adapter cannot read is an `unavailable` outcome in the result, not an
    /// exception.
    fn google_events_list_to_blocks_json(
        &self,
        py: Python<'_>,
        payload_json: &Bound<'_, PyString>,
        participant_zone: &Bound<'_, PyString>,
    ) -> PyResult<String> {
        let payload_json = error::document(payload_json)?;
        let participant_zone = error::argument(participant_zone)?;
        error::detach(py, || {
            opportune_adapters::json::events_list_to_blocks_with_db(
                self.engine.time_zone_database(),
                payload_json,
                participant_zone,
            )
        })
    }

    /// Converts a Google `freeBusy.query` response, as JSON outcomes keyed by
    /// calendar ID.
    ///
    /// These ranges carry their own UTC offsets and name no zone, so the
    /// conversion reads no database.
    ///
    /// Raises `OpportuneError` when the document is not a `freeBusy.query`
    /// response. A calendar the provider reported an error for is an `unavailable`
    /// outcome inside the object, not an exception.
    #[expect(
        clippy::unused_self,
        reason = "the conversion reads no database, but it is a method so a Python caller reaches one object for the whole wire surface"
    )]
    fn google_free_busy_to_blocks_json(
        &self,
        py: Python<'_>,
        payload_json: &Bound<'_, PyString>,
    ) -> PyResult<String> {
        let payload_json = error::document(payload_json)?;
        error::detach(py, || {
            opportune_adapters::json::free_busy_to_blocks(payload_json)
        })
    }
}

/// The engine's own default limits as the JSON document `default_limits()` reads.
///
/// The document is built from [`opportune::Limits::default`] through the same
/// [`WireLimits`] conversion a caller's limits take, so the defaults a caller
/// starts from are the ones the engine applies when a request names no limits at
/// all.
///
/// # Errors
/// Returns [`OpportuneError`] only if the document cannot be written, which the
/// engine's own limits never fail to be.
pub(crate) fn default_limits_json(py: Python<'_>) -> PyResult<String> {
    let limits = opportune::Limits::default();
    to_json(&WireLimits::from(&limits)).map_err(|error| error::rejection(py, &error))
}

/// Builds the core engine from the arguments a Python caller gave.
///
/// Absent arguments take the engine's own defaults: the bundled time zone
/// database and the default limits.
fn build(
    tzdb_dir: Option<&str>,
    tzdb_version: Option<&str>,
    limits_json: Option<&str>,
) -> Result<CoreEngine, JsonError> {
    let mut builder = CoreEngine::builder();
    match (tzdb_dir, tzdb_version) {
        (Some(directory), label) => builder = builder.tzdb_dir(directory, label),
        (None, Some(label)) => {
            return Err(JsonError::new(
                error::VERSION_WITHOUT_DIRECTORY,
                format!(
                    "a time zone database version label `{label}` was given with no \
                     `tzdb_dir` to label; the bundled database reports its own version",
                ),
                "",
            ));
        }
        (None, None) => {}
    }
    if let Some(limits_json) = limits_json {
        builder = builder.limits(parse_limits(limits_json)?);
    }
    builder
        .build()
        .map_err(|err| JsonError::new(err.code(), err.to_string(), ""))
}
