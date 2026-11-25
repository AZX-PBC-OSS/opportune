//! The engine: the time zone database and the request limits, held once and
//! reused.

use crate::error::{Diagnostics, Error};
use crate::eval;
use crate::limits::Limits;
use crate::model::{Proposal, Request, RequiredRanges, Response, SlotCheck};
use crate::time::TimeZoneDatabase;

/// An engine: the time zone database and the limits every request is judged
/// against.
///
/// Build one once and reuse it. The database is always an explicit choice — the
/// bundled one, or a directory the caller names — and never the host's, because
/// results that depend on the machine are not reproducible.
///
/// ```
/// # #[cfg(feature = "bundled-tzdb")] {
/// use opportune::{Engine, Limits};
///
/// let engine = Engine::builder().limits(Limits::default()).build().unwrap();
/// assert!(!engine.time_zone_database().version().is_empty());
/// # }
/// ```
#[derive(Debug, Clone)]
pub struct Engine {
    db: TimeZoneDatabase,
    limits: Limits,
}

impl Engine {
    /// Starts building an engine.
    #[must_use]
    pub fn builder() -> EngineBuilder {
        EngineBuilder::default()
    }

    /// The time zone database the engine resolves names against.
    #[must_use]
    pub fn time_zone_database(&self) -> &TimeZoneDatabase {
        &self.db
    }

    /// The limits the engine enforces.
    #[must_use]
    pub fn limits(&self) -> &Limits {
        &self.limits
    }

    /// The static checks on a request, without computing availability.
    ///
    /// Errors are reserved for requests that are structurally impossible, so a
    /// calendar that has not been fetched produces an empty answer and a warning
    /// rather than an error.
    ///
    /// ```
    /// use opportune::{Engine, model::Request, model::Goal, model::Span};
    /// use opportune::time::{Instant, ZoneName};
    ///
    /// # #[cfg(feature = "bundled-tzdb")] {
    /// let engine = Engine::builder().build().unwrap();
    /// let goal = Goal::new(
    ///     ZoneName::new("UTC").unwrap(),
    ///     Span::new(Instant::MIN, Instant::MAX).unwrap(),
    ///     Vec::new(),
    /// );
    /// let request = Request::new(Instant::MIN, goal);
    /// let diagnostics = engine.validate(&request);
    /// // A goal with no part cannot be booked.
    /// assert_eq!(diagnostics.errors[0].code(), "part_count");
    /// # }
    /// ```
    #[must_use]
    pub fn validate(&self, request: &Request) -> Diagnostics {
        crate::validate::validate(self, request)
    }

    /// The times that can be booked, with who could take each one.
    ///
    /// ```
    /// # #[cfg(feature = "bundled-tzdb")] {
    /// use opportune::model::{Goal, Part, Request, Requirement, Span, Term};
    /// use opportune::{Engine, model::Participant};
    /// use opportune::time::{Duration, Instant, ZoneName};
    ///
    /// let engine = Engine::builder().build().unwrap();
    /// let zone = ZoneName::new("UTC").unwrap();
    /// let day = Span::new(
    ///     Instant::from_minutes(Instant::MIN.minutes()).unwrap(),
    ///     Instant::from_minutes(Instant::MIN.minutes() + 24 * 60).unwrap(),
    /// ).unwrap();
    /// let part = Part::new(
    ///     Duration::from_minutes(30),
    ///     Requirement::new(vec![Term::person("rich")]),
    /// );
    /// let mut request = Request::new(Instant::MIN, Goal::new(zone.clone(), day, vec![part]));
    /// request
    ///     .participants
    ///     .push(Participant::new("rich", zone).with_calendar(Vec::new()));
    ///
    /// let response = engine.find_availability(&request).unwrap();
    /// assert_eq!(response.offers.len(), 48);
    /// assert_eq!(response.dates.len(), 1);
    /// assert_eq!(response.dates[0].offers, 48);
    /// assert!(!response.truncated);
    /// # }
    /// ```
    ///
    /// # Errors
    /// Returns [`Error::InvalidRequest`] carrying the same errors [`Engine::validate`]
    /// reports, and [`Error::Time`] for a value that cannot be resolved.
    pub fn find_availability(&self, request: &Request) -> Result<Response, Error> {
        let mut validated = crate::validate::validate_for_search(self, request);
        if !validated.diagnostics.is_valid() {
            return Err(Error::invalid_request(validated.diagnostics.errors));
        }
        let settings = eval::search_settings_resolved(
            self,
            request,
            validated.candidates,
            validated.pre_resolved,
        )?;
        crate::validate::append_window_warnings(&settings, &mut validated.diagnostics.warnings);
        crate::output::response(self, request, &settings, validated.diagnostics.warnings)
    }

    /// Checks one proposed booking against the same rules, calendars and caps
    /// [`Engine::find_availability`] uses, ignoring the grid and the search
    /// window.
    ///
    /// The request is checked for what a confirmation depends on — its structure,
    /// its requirement, and the configured limits on its size and durations — and
    /// not for what only a search needs: a
    /// search window longer than the configured maximum, a grid producing more
    /// candidate starts than the configured maximum, both a grid and candidate
    /// starts, or a grid zone that cannot be resolved. None of those decide
    /// anything about the proposed start, so none of them rejects it.
    ///
    /// Every failing reason is returned, whether or not the request asked for an
    /// explanation, so a caller confirming a booking always learns what stands in
    /// the way. For a booking [`Engine::find_availability`] offered, the answer is
    /// valid with no reasons and the same flags and reserve spans the offer
    /// carries.
    ///
    /// A proposal whose booking, or whose required travel, reaches past either edge
    /// of the supported range is rejected rather than answered: the model has no
    /// `Span` to report for the minutes beyond `Instant::MIN` or `Instant::MAX`, so
    /// a caller could not persist the booking or the holds it needs. The search
    /// applies the same rule with the request-wide travel envelope, so a start it
    /// offers always confirms.
    ///
    /// ```
    /// # #[cfg(feature = "bundled-tzdb")] {
    /// use opportune::model::{Goal, Participant, Proposal, ProposalEvent, Request, Requirement, Span, Term};
    /// use opportune::Engine;
    /// use opportune::time::{Duration, Instant, ZoneName};
    ///
    /// let engine = Engine::builder().build().unwrap();
    /// let zone = ZoneName::new("UTC").unwrap();
    /// let day = Span::new(Instant::MIN, Instant::from_minutes(Instant::MIN.minutes() + 24 * 60).unwrap()).unwrap();
    /// let goal = Goal::single(
    ///     zone.clone(),
    ///     day,
    ///     Duration::from_minutes(30),
    ///     Requirement::new(vec![Term::person("rich")]),
    /// );
    /// let mut request = Request::new(Instant::MIN, goal);
    /// request
    ///     .participants
    ///     .push(Participant::new("rich", zone).with_calendar(Vec::new()));
    ///
    /// let offer = engine.find_availability(&request).unwrap().offers[0].clone();
    /// let check = engine.check_slot(&request, &Proposal {
    ///     events: vec![ProposalEvent {
    ///         part: "main".to_owned(),
    ///         start: offer.start().unwrap(),
    ///         assignment: vec![vec!["rich".to_owned()]],
    ///     }],
    /// }).unwrap();
    /// assert!(check.valid);
    /// assert!(check.reasons.is_empty());
    /// # }
    /// ```
    ///
    /// # Errors
    /// Returns [`Error::InvalidRequest`] carrying the same errors [`Engine::validate`]
    /// reports for the request's structure, requirement and size,
    /// [`Error::MalformedProposal`] for a proposal that does not fit the request
    /// (an unknown part, the wrong number of terms or hosts, a repeated host, an
    /// attendee assigned, a host that does not fill its term, or a booking whose
    /// occupied range is not representable, with code `proposal_out_of_range` at
    /// `/events/{index}/start`), and an
    /// [`Error::Time`] failure for a value that cannot be resolved.
    pub fn check_slot(&self, request: &Request, proposal: &Proposal) -> Result<SlotCheck, Error> {
        let (diagnostics, pre_resolved) = crate::validate::validate_for_slot(self, request);
        if !diagnostics.is_valid() {
            return Err(Error::invalid_request(diagnostics.errors));
        }
        let event = crate::validate::check_proposal(request, proposal)?;
        let settings = eval::slot_settings_resolved(self, request, event.start, pre_resolved)?;
        let assignment: Vec<&str> = event
            .assignment
            .iter()
            .flatten()
            .map(String::as_str)
            .collect();
        crate::output::slot_check(
            self,
            &settings,
            event.start,
            &assignment,
            diagnostics.warnings,
        )
    }

    /// The calendar and occurrence data a request needs, before fetching any.
    ///
    /// ```
    /// # #[cfg(feature = "bundled-tzdb")] {
    /// use opportune::model::{Goal, Part, RangesQuery, Requirement, Span, Term};
    /// use opportune::{Engine, model::Participant};
    /// use opportune::time::{Duration, Instant, ZoneName};
    ///
    /// let engine = Engine::builder().build().unwrap();
    /// let zone = ZoneName::new("UTC").unwrap();
    /// let part = Part::new(
    ///     Duration::from_minutes(30),
    ///     Requirement::new(vec![Term::person("rich")]),
    /// );
    /// let mut query = RangesQuery::new(
    ///     Instant::MIN,
    ///     Goal::new(
    ///         zone.clone(),
    ///         Span::new(
    ///             Instant::from_minutes(Instant::MIN.minutes() + 24 * 60).unwrap(),
    ///             Instant::from_minutes(Instant::MIN.minutes() + 48 * 60).unwrap(),
    ///         )
    ///         .unwrap(),
    ///         vec![part],
    ///     ),
    /// );
    /// query
    ///     .participants
    ///     .push(Participant::new("rich", zone).with_calendar(Vec::new()));
    ///
    /// let ranges = engine.required_ranges(&query).unwrap();
    /// // The padded window runs one day either side of the search window, and
    /// // no cap applies.
    /// assert_eq!(ranges.calendar.start(), Instant::MIN);
    /// assert!(ranges.occurrences.is_none());
    /// # }
    /// ```
    ///
    /// The returned hulls start the documented two-phase fetch, not a one-pass
    /// query: a cap merges every record sharing an event key across all supplied
    /// time ranges. Fetch the initial calendar and occurrence hulls — every block
    /// whose occupied span, its own span widened by its travel, overlaps
    /// `calendar`, and every occurrence and block whose own span overlaps
    /// `occurrences` — observe the event keys on every participant's own-span
    /// occurrence-range records, then fetch every record carrying one of those
    /// keys, on every participant, including all-day and otherwise nonmatching
    /// records. `Request::occurrences` is one request-global list: never filter it
    /// by the participants an occurrence names. Evaluate the closed request against
    /// a consistent provider snapshot.
    ///
    /// The hulls cover only instants inside the query's `goal.search_window`, but
    /// `check_slot` ignores that window. To confirm a start outside it, call
    /// `required_ranges` again with a query whose `goal.search_window` contains the
    /// proposed start and repeat the same initial fetch and event-key closure;
    /// reusing the search's hulls can read a blocked minute or a spent cap as free.
    ///
    /// A one-pass fetch is key-complete only when every copy of an event has the
    /// identical own span; otherwise a same-key record outside the hull can produce
    /// a false offer or a false exclusion. When exact closure is unavailable, fetch
    /// all occurrences and blocks across the full supported time domain
    /// (`Span::EVERYTHING`) with [`crate::Limits`] raised to cover it — raise
    /// `max_blocks_per_participant` and `max_occurrences` with
    /// [`crate::Limits::with_max_blocks_per_participant`] and
    /// [`crate::Limits::with_max_occurrences`], because the published defaults can
    /// refuse that fetch with `blocks_limit` or `occurrences_limit` — or fail
    /// closed by returning an error instead of evaluating a narrowed request; no
    /// engine call fails closed. [`RequiredRanges`] carries the whole contract.
    ///
    /// # Errors
    /// Returns [`Error::PartCount`] for a goal that does not have exactly one
    /// part, [`Error::InvalidRequest`] carrying the same findings the request
    /// validator reports for the counts this computation multiplies — a
    /// `participants_limit` at `/participants`, a `rules_limit` at `/rules` and a
    /// `zone_periods_limit` for each participant holding too many zone periods —
    /// and an [`Error::Time`] failure for a zone that cannot be resolved. A range
    /// that cannot be computed is never replaced by a narrower one.
    pub fn required_ranges(
        &self,
        query: &crate::model::RangesQuery,
    ) -> Result<RequiredRanges, Error> {
        crate::ranges::required_ranges(self, query)
    }
}

/// Builds an [`Engine`].
///
/// A database is required: with neither `tzdb_bundled` nor `tzdb_dir`, building
/// fails rather than falling back to the host's database. The bundled database is
/// the default when the `bundled-tzdb` feature is on.
#[derive(Debug, Default, Clone)]
pub struct EngineBuilder {
    /// `Some` once the caller names a database, holding the construction's
    /// outcome so a failure is reported from `build` with its own cause rather
    /// than being flattened into "none configured".
    database: Option<Result<TimeZoneDatabase, crate::time::TimeError>>,
    limits: Option<Limits>,
}

/// The database an engine uses when the caller names none: the bundled one when
/// the feature is on, and nothing at all otherwise — there is no host fallback.
#[cfg(feature = "bundled-tzdb")]
fn default_database() -> Result<TimeZoneDatabase, crate::time::TimeError> {
    TimeZoneDatabase::bundled()
}

/// The database an engine uses when the caller names none: nothing, because a
/// build without `bundled-tzdb` has no database compiled in.
#[cfg(not(feature = "bundled-tzdb"))]
fn default_database() -> Result<TimeZoneDatabase, crate::time::TimeError> {
    Err(crate::time::TimeError::TzdbNotConfigured)
}

impl EngineBuilder {
    /// Uses the bundled time zone database.
    #[cfg(feature = "bundled-tzdb")]
    #[must_use]
    pub fn tzdb_bundled(mut self) -> Self {
        self.database = Some(TimeZoneDatabase::bundled());
        self
    }

    /// Uses a time zone database read from a directory of `TZif` files.
    ///
    /// The version label comes from the directory's `+VERSION` file, and from
    /// `label` when it has none; a database with neither is a construction
    /// error, because every response reports `meta.tzdb`.
    ///
    /// A directory that cannot be used is reported by [`EngineBuilder::build`]
    /// with the directory's own failure, not as a missing configuration.
    #[cfg(not(target_family = "wasm"))]
    #[must_use]
    pub fn tzdb_dir(mut self, path: impl AsRef<std::path::Path>, label: Option<&str>) -> Self {
        self.database = Some(TimeZoneDatabase::from_dir(path, label));
        self
    }

    /// Uses the given limits instead of the defaults.
    #[must_use]
    pub fn limits(mut self, limits: Limits) -> Self {
        self.limits = Some(limits);
        self
    }

    /// Builds the engine.
    ///
    /// # Errors
    /// Returns [`crate::time::TimeError::TzdbNotConfigured`] when no database was
    /// configured, and the database's own construction failure when a named
    /// directory could not be used.
    pub fn build(self) -> Result<Engine, crate::time::TimeError> {
        let db = match self.database {
            Some(database) => database?,
            None => default_database()?,
        };
        Ok(Engine {
            db,
            limits: self.limits.unwrap_or_default(),
        })
    }
}

/// The engines the free functions use: the bundled database and the default
/// limits, built once per process.
#[cfg(feature = "bundled-tzdb")]
fn default_engine() -> Result<&'static Engine, Error> {
    use std::sync::OnceLock;

    static ENGINE: OnceLock<Engine> = OnceLock::new();
    if let Some(engine) = ENGINE.get() {
        return Ok(engine);
    }
    let engine = Engine::builder().build()?;
    Ok(ENGINE.get_or_init(|| engine))
}

/// Offers, using a default engine with the bundled database and default limits.
///
/// # Errors
/// Returns the same failures [`Engine::find_availability`] does.
#[cfg(feature = "bundled-tzdb")]
pub fn find_availability(request: &Request) -> Result<Response, Error> {
    default_engine()?.find_availability(request)
}

/// Checks one proposed booking, using a default engine with the bundled database
/// and default limits.
///
/// # Errors
/// Returns the same failures [`Engine::check_slot`] does.
#[cfg(feature = "bundled-tzdb")]
pub fn check_slot(request: &Request, proposal: &Proposal) -> Result<SlotCheck, Error> {
    default_engine()?.check_slot(request, proposal)
}

/// Static checks, using a default engine with the bundled database and default
/// limits.
///
/// A binary built without its bundled database cannot judge any zone, and that
/// failure is reported as an error diagnostic rather than as a clean request:
/// the signature answers with diagnostics, so the only honest way to say "the
/// engine could not start" is one of them.
#[cfg(feature = "bundled-tzdb")]
#[must_use]
pub fn validate(request: &Request) -> Diagnostics {
    match default_engine() {
        Ok(engine) => engine.validate(request),
        Err(err) => {
            let mut diagnostics = Diagnostics::default();
            diagnostics
                .errors
                .push(crate::error::Diagnostic::new(err.code(), err.to_string()));
            diagnostics
        }
    }
}

/// The data a request needs, using a default engine with the bundled database
/// and default limits.
///
/// The returned hulls begin the two-phase fetch [`Engine::required_ranges`]
/// documents: fetch the initial calendar and occurrence hulls, then every record
/// carrying an event key observed on an own-span occurrence-range record, or fetch
/// the full supported time domain with [`crate::Limits`] raised to cover it or fail
/// closed. They cover only instants inside the query's `goal.search_window`; a
/// confirmation outside it needs a new query whose window contains the proposed
/// start and the same initial fetch and event-key closure.
///
/// # Errors
/// Returns [`crate::time::TimeError::TzdbBundledUnavailable`] wrapped in an
/// [`Error`] when the binary carries no bundled database, and the same failures
/// [`Engine::required_ranges`] does.
#[cfg(feature = "bundled-tzdb")]
pub fn required_ranges(query: &crate::model::RangesQuery) -> Result<RequiredRanges, Error> {
    default_engine()?.required_ranges(query)
}
