//! Time zones, the time zone database, and the conversion between instants and
//! local times.
//!
//! A [`TimeZoneDatabase`] is always an explicit choice: the bundled database
//! compiled into the binary, or a directory the caller names. It never consults
//! the host's database, because results that depend on the machine are not
//! reproducible.
//!
//! ```
//! use opportune::time::TimeZoneDatabase;
//!
//! # #[cfg(feature = "bundled-tzdb")] {
//! let db = TimeZoneDatabase::bundled().unwrap();
//! assert!(!db.version().is_empty());
//! // Legacy aliases resolve to the zone they name.
//! let old_name = db.zone("US/Eastern").unwrap();
//! let current = db.zone("America/New_York").unwrap();
//! assert_eq!(old_name.name(), "US/Eastern");
//! assert_eq!(
//!     old_name.offset_at(opportune::time::Instant::MAX),
//!     current.offset_at(opportune::time::Instant::MAX),
//! );
//! # }
//! ```

use core::fmt;

use jiff::tz::{AmbiguousOffset, TimeZone};

use crate::time::breakpoints;
use crate::time::error::TimeError;
use crate::time::{Date, Instant, TimeOfDay};

/// The name of a time zone, as a caller writes it.
///
/// A request names zones and the engine resolves them, so a name has to be
/// storable before any database has answered for it: whether `America/New_York`
/// exists is the engine's question, and a name it cannot resolve is a request
/// error rather than a construction failure. What a name cannot be is nothing
/// at all, which is why the constructor rejects a blank one.
///
/// ```
/// use opportune::time::ZoneName;
///
/// let name = ZoneName::new("America/New_York").unwrap();
/// assert_eq!(name.as_str(), "America/New_York");
/// assert_eq!(ZoneName::new("  ").unwrap_err().code(), "zone_name_invalid");
/// ```
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ZoneName(String);

impl ZoneName {
    /// Creates a zone name from the text a caller wrote.
    ///
    /// # Errors
    /// Returns [`TimeError::ZoneNameInvalid`] for an empty or whitespace-only
    /// name.
    pub fn new(name: &str) -> Result<Self, TimeError> {
        if name.trim().is_empty() {
            return Err(TimeError::ZoneNameInvalid {
                name: name.to_owned(),
            });
        }
        Ok(Self(name.to_owned()))
    }

    /// The name, as written.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The `UTC` zone name, which is always valid.
    ///
    /// It is the name a caller reaches for when a request needs a fixed zone and
    /// nothing is being validated from a document, so it needs neither a
    /// `Result` nor an `unwrap`:
    ///
    /// ```
    /// use opportune::time::ZoneName;
    ///
    /// assert_eq!(ZoneName::utc().as_str(), "UTC");
    /// assert_eq!(ZoneName::new("UTC").unwrap(), ZoneName::utc());
    /// ```
    #[must_use]
    pub fn utc() -> Self {
        Self("UTC".to_owned())
    }
}

impl fmt::Debug for ZoneName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl AsRef<str> for ZoneName {
    /// The name, as written.
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl TryFrom<&str> for ZoneName {
    type Error = TimeError;

    fn try_from(name: &str) -> Result<Self, TimeError> {
        Self::new(name)
    }
}

impl TryFrom<String> for ZoneName {
    type Error = TimeError;

    fn try_from(name: String) -> Result<Self, TimeError> {
        if name.trim().is_empty() {
            return Err(TimeError::ZoneNameInvalid { name });
        }
        Ok(Self(name))
    }
}

impl fmt::Display for ZoneName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// A set of IANA time zones, with the version label every response reports.
///
/// Build one from the database compiled into the binary (`bundled`, with the
/// `bundled-tzdb` feature) or from a directory the caller names (`from_dir`, on
/// targets with a filesystem). Those two names are deliberately not doc links:
/// each constructor exists on only some builds, and a link to an item a `cfg`
/// removed breaks `cargo doc` with `-D warnings` on every build without it.
/// There is no constructor that falls back to the host's database: a caller
/// without the bundled data must say which database it wants.
#[derive(Clone)]
pub struct TimeZoneDatabase {
    db: jiff::tz::TimeZoneDatabase,
    version: String,
}

impl TimeZoneDatabase {
    /// The time zone database bundled into the binary.
    ///
    /// ```
    /// # #[cfg(feature = "bundled-tzdb")] {
    /// use opportune::time::TimeZoneDatabase;
    ///
    /// let db = TimeZoneDatabase::bundled().unwrap();
    /// assert!(!db.version().is_empty());
    /// assert!(db.zone("America/New_York").is_ok());
    /// # }
    /// ```
    ///
    /// # Errors
    /// Returns [`TimeError::TzdbBundledUnavailable`] when the binary was built
    /// with `bundled-tzdb` but carries no data, which means every zone lookup
    /// would fail for a reason the caller cannot see.
    #[cfg(feature = "bundled-tzdb")]
    pub fn bundled() -> Result<Self, TimeError> {
        let db = jiff::tz::TimeZoneDatabase::bundled();
        if db.is_definitively_empty() {
            return Err(TimeError::TzdbBundledUnavailable);
        }
        Ok(Self {
            db,
            version: jiff_tzdb::VERSION.to_owned(),
        })
    }

    /// A time zone database read from a directory of `TZif` files.
    ///
    /// The version label is the contents of the directory's `+VERSION` file
    /// when it holds one, and otherwise the caller's `label`. Responses report
    /// it as `meta.tzdb`, so a database with neither is rejected at
    /// construction rather than reporting an undefined version.
    ///
    /// ```no_run
    /// use opportune::time::TimeZoneDatabase;
    ///
    /// let db = TimeZoneDatabase::from_dir("/usr/share/zoneinfo", Some("2026f"))?;
    /// assert!(!db.version().is_empty());
    /// # Ok::<(), opportune::time::TimeError>(())
    /// ```
    ///
    /// # Errors
    /// Returns [`TimeError::TzdbDirectoryUnusable`] when the directory holds no
    /// `TZif` data or cannot be read, and [`TimeError::TzdbVersionMissing`] when
    /// it has no `+VERSION` and the caller supplied no label.
    #[cfg(not(target_family = "wasm"))]
    pub fn from_dir(
        path: impl AsRef<std::path::Path>,
        label: Option<&str>,
    ) -> Result<Self, TimeError> {
        let path = path.as_ref();
        let shown = path.display().to_string();
        let unusable = |reason: String| TimeError::TzdbDirectoryUnusable {
            path: shown.clone(),
            reason,
        };
        let db =
            jiff::tz::TimeZoneDatabase::from_dir(path).map_err(|err| unusable(err.to_string()))?;
        // `is_definitively_empty` answers true only for a database that already
        // knows it holds nothing, and a directory database reads lazily. Asking
        // it for one name forces the read, so a directory of unrelated files is
        // rejected here instead of turning every later zone lookup into
        // `zone_unknown`.
        if db.available().next().is_none() {
            return Err(unusable(
                "the directory holds no TZif time zone files".to_owned(),
            ));
        }
        let version = read_version(path)
            .map_err(unusable)?
            .or_else(|| normalised_label(label))
            .ok_or_else(|| TimeError::TzdbVersionMissing {
                path: shown.clone(),
            })?;
        Ok(Self { db, version })
    }

    /// The database's version label, as reported in `meta.tzdb`.
    #[must_use]
    pub fn version(&self) -> &str {
        &self.version
    }

    /// The zone with the given IANA name, including legacy aliases.
    ///
    /// ```
    /// # #[cfg(feature = "bundled-tzdb")] {
    /// # use opportune::time::TimeZoneDatabase;
    /// let db = TimeZoneDatabase::bundled().unwrap();
    /// assert_eq!(db.zone("US/Eastern").unwrap().name(), "US/Eastern");
    /// assert!(db.zone("Mars/Olympus_Mons").is_err());
    /// # }
    /// ```
    ///
    /// # Errors
    /// Returns [`TimeError::ZoneUnknown`] when the name is not in this
    /// database.
    pub fn zone(&self, name: &str) -> Result<Zone, TimeError> {
        self.db
            .get(name)
            .map(|inner| Zone {
                name: name.to_owned(),
                inner,
            })
            .map_err(|_| TimeError::ZoneUnknown {
                name: name.to_owned(),
            })
    }

    /// Every zone name this database holds. Test-only: the whole-minute offset
    /// test sweeps the bundled database with it, and no production path needs
    /// the whole list.
    #[cfg(all(test, feature = "bundled-tzdb"))]
    pub(crate) fn available_names(&self) -> Vec<String> {
        self.db
            .available()
            .map(|name| name.as_str().to_owned())
            .collect()
    }
}

impl fmt::Debug for TimeZoneDatabase {
    /// Formats the version, not the database handle: the handle's own debug
    /// output is the entire zone list.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TimeZoneDatabase")
            .field("version", &self.version)
            .finish_non_exhaustive()
    }
}

/// Reads a directory's `+VERSION` file, treating a missing or blank file as no
/// version at all so the caller's label can supply one.
#[cfg(not(target_family = "wasm"))]
fn read_version(path: &std::path::Path) -> Result<Option<String>, String> {
    match std::fs::read_to_string(path.join("+VERSION")) {
        Ok(contents) => Ok(normalised_label(Some(&contents))),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(format!("+VERSION cannot be read: {err}")),
    }
}

/// Trims a version label and treats a blank one as no label, so a caller's
/// label and `+VERSION` content are normalised by the same rule and a blank
/// label cannot report an undefined version in `meta.tzdb`.
#[cfg(not(target_family = "wasm"))]
fn normalised_label(label: Option<&str>) -> Option<String> {
    label
        .map(str::trim)
        .filter(|label| !label.is_empty())
        .map(str::to_owned)
}

/// One time zone from a [`TimeZoneDatabase`].
///
/// A zone converts instants to local dates and times, and local times back to
/// the instants they exist at — none, one, or two of them.
#[derive(Clone, PartialEq, Eq)]
pub struct Zone {
    name: String,
    inner: TimeZone,
}

impl Zone {
    /// Coordinated Universal Time, which needs no database.
    ///
    /// ```
    /// # use opportune::time::{Date, LocalResolution, TimeOfDay, Zone};
    /// let utc = Zone::utc();
    /// let local = utc.resolve_local(
    ///     Date::new(2027, 3, 9).unwrap(),
    ///     TimeOfDay::new(15, 0).unwrap(),
    /// ).unwrap();
    /// let LocalResolution::Unique(instant) = local else {
    ///     panic!("UTC has no daylight saving transitions")
    /// };
    /// assert_eq!(instant.to_string(), "2027-03-09T15:00Z");
    /// ```
    #[must_use]
    pub fn utc() -> Self {
        Self {
            name: "UTC".to_owned(),
            inner: TimeZone::UTC,
        }
    }

    /// The name this zone was requested by.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The zone's offset from UTC at an instant.
    #[must_use]
    pub fn offset_at(&self, instant: Instant) -> UtcOffset {
        UtcOffset {
            seconds: self.inner.to_offset(instant.as_jiff()).seconds(),
        }
    }

    /// The local date and time an instant falls on in this zone.
    ///
    /// ```
    /// # use opportune::time::{Instant, Zone};
    /// let tokyo_instant = Instant::from_unix_seconds(1_804_604_400).unwrap();
    /// let local = tokyo_instant.zoned(&Zone::utc());
    /// assert_eq!(local.to_string(), "2027-03-09 15:00 UTC+00:00");
    /// ```
    ///
    /// [`Instant::zoned`]: crate::time::Instant::zoned
    #[must_use]
    pub fn zoned(&self, instant: Instant) -> ZonedInstant {
        instant.zoned(self)
    }

    /// The instants a local date and time exist at in this zone.
    ///
    /// A local time inside a spring-forward gap does not exist
    /// ([`LocalResolution::Skipped`]); a local time inside a fall-back overlap
    /// exists twice ([`LocalResolution::Repeated`], earlier instant first).
    ///
    /// ```
    /// # #[cfg(feature = "bundled-tzdb")] {
    /// use opportune::time::{Date, LocalResolution, TimeOfDay, TimeZoneDatabase};
    ///
    /// let db = TimeZoneDatabase::bundled().unwrap();
    /// let new_york = db.zone("America/New_York").unwrap();
    /// let spring_forward = new_york.resolve_local(
    ///     Date::new(2027, 3, 14).unwrap(),
    ///     TimeOfDay::new(2, 30).unwrap(),
    /// ).unwrap();
    /// assert_eq!(spring_forward, LocalResolution::Skipped);
    /// # }
    /// ```
    ///
    /// # Errors
    /// Returns [`TimeError::CivilToInstant`] when the local time lies outside
    /// the representable instant range.
    pub fn resolve_local(&self, date: Date, time: TimeOfDay) -> Result<LocalResolution, TimeError> {
        let to_instant = |seconds: i64| {
            Instant::from_unix_seconds(seconds).map_err(|err| match err {
                // The civil time exists; the instant it names is outside the
                // supported range, so the mapping is what failed.
                TimeError::InstantOutOfRange { .. } => TimeError::CivilToInstant { date, time },
                other => other,
            })
        };
        match self.local_seconds(date, time)?.as_slice() {
            [] => Ok(LocalResolution::Skipped),
            [only] => Ok(LocalResolution::Unique(to_instant(*only)?)),
            [earlier, later] => Ok(LocalResolution::Repeated(
                to_instant(*earlier)?,
                to_instant(*later)?,
            )),
            _ => unreachable!("a local date-time occurs at no more than two instants"),
        }
    }

    /// The Unix seconds a local date-time exists at in this zone, ascending:
    /// none inside a spring-forward gap and two inside a fall-back overlap.
    fn local_seconds(&self, date: Date, time: TimeOfDay) -> Result<LocalValues<i64>, TimeError> {
        let datetime = time.on(date);
        let to_seconds = |offset: jiff::tz::Offset| {
            offset
                .to_timestamp(datetime)
                .map(jiff::Timestamp::as_second)
                .map_err(|_| TimeError::CivilToInstant { date, time })
        };
        match self.inner.to_ambiguous_zoned_datetime(datetime).offset() {
            AmbiguousOffset::Gap { .. } => Ok(LocalValues::empty(0)),
            AmbiguousOffset::Unambiguous { offset } => Ok(LocalValues::one(to_seconds(offset)?)),
            AmbiguousOffset::Fold { before, after } => {
                Ok(LocalValues::two(to_seconds(before)?, to_seconds(after)?))
            }
        }
    }

    /// Every instant the local date-time occurs at, ascending, leaving out any
    /// that falls outside the supported instant range.
    ///
    /// A local date-time near an edge of the supported range can exist twice
    /// with only one of the two inside it. The omitted instant cannot be a
    /// breakpoint of a range the caller asked about, because that range is
    /// inside the supported range; a sub-minute offset, on the other hand, is a
    /// database defect rather than an edge, so it is an error here too.
    pub(crate) fn local_instants(
        &self,
        date: Date,
        time: TimeOfDay,
    ) -> Result<LocalValues<Instant>, TimeError> {
        let mut instants = LocalValues::empty(Instant::MIN);
        for seconds in self.local_seconds(date, time)? {
            match Instant::from_unix_seconds(seconds) {
                Ok(instant) => instants.push(instant),
                Err(TimeError::InstantOutOfRange { .. }) => {}
                Err(other) => return Err(other),
            }
        }
        Ok(instants)
    }

    /// The instants strictly between `after` and `until` at which this zone's
    /// offset changes, ascending. A change exactly at `after` is not returned:
    /// the offset in force there is already the one `after` reads.
    pub(crate) fn transition_instants(
        &self,
        after: Instant,
        until: Instant,
    ) -> Result<Vec<Instant>, TimeError> {
        let mut instants = Vec::new();
        for transition in self.inner.following(after.as_jiff()) {
            let seconds = transition.timestamp().as_second();
            if seconds >= until.unix_seconds() {
                break;
            }
            instants.push(Instant::from_unix_seconds(seconds)?);
        }
        Ok(instants)
    }

    /// The first instant whose local date in this zone is `date` or later.
    ///
    /// This is the instant a civil date begins at, which a local midnight does
    /// not always name: a forward transition can skip midnight altogether, and
    /// then the date begins at the transition; a backward transition can repeat
    /// midnight, and the earlier instant is the one the date begins at. It is
    /// what an all-day event becomes when its civil dates are resolved in the
    /// participant's zone.
    ///
    /// ```
    /// # #[cfg(feature = "bundled-tzdb")] {
    /// use opportune::time::{Date, TimeOfDay, TimeZoneDatabase};
    ///
    /// let db = TimeZoneDatabase::bundled().unwrap();
    /// let santiago = db.zone("America/Santiago").unwrap();
    /// // Santiago springs forward from midnight to 01:00 that day, so the date
    /// // begins at the transition rather than at a midnight that never happens.
    /// let begins = santiago.day_start(Date::new(2027, 9, 5).unwrap()).unwrap();
    /// assert_eq!(santiago.zoned(begins).time(), TimeOfDay::new(1, 0).unwrap());
    /// # }
    /// ```
    ///
    /// # Errors
    /// Returns [`TimeError::CivilToInstant`] when the date begins outside the
    /// representable instant range.
    pub fn day_start(&self, date: Date) -> Result<Instant, TimeError> {
        match self.resolve_local(date, TimeOfDay::MIDNIGHT)? {
            LocalResolution::Unique(instant) | LocalResolution::Repeated(instant, _) => Ok(instant),
            LocalResolution::Skipped => {
                // UTC is transition-free, so it fixes a civil date's instant and
                // every zone's offset is within a day of it; the transition that
                // swallowed this midnight lies inside the window scanned below.
                let approximately = Instant::from_utc(date, TimeOfDay::MIDNIGHT)?;
                let lo = Instant::from_minutes(
                    (approximately.minutes() - 26 * 60).max(Instant::MIN.minutes()),
                )?;
                let hi = Instant::from_minutes(
                    (approximately.minutes() + 26 * 60).min(Instant::MAX.minutes()),
                )?;
                for transition in self.transition_instants(lo, hi)? {
                    if self.zoned(transition).date() >= date {
                        return Ok(transition);
                    }
                }
                // No transition describes this date, so the only safe answer is
                // one no later than the truth: a wider range, never a narrower
                // one.
                Ok(lo)
            }
        }
    }

    /// The Unix second a local date and time names, resolving a skipped or
    /// repeated local reading the way provider timestamps are read: the earlier
    /// instant in a repeated hour, and the instant after the gap in a skipped
    /// one (jiff's *compatible* disambiguation).
    ///
    /// The seconds component is part of the local reading, because a skipped hour
    /// shifts it: New York's local `02:30:15` on the spring-forward day is
    /// `03:30:15` after the gap, so resolving the minute alone and adding the
    /// seconds would land fifteen seconds early. The nanosecond component does not
    /// change which instant a reading names — every offset in the supported range
    /// is a whole number of minutes — so it is applied when the result is rounded
    /// to the minute with [`Instant::floor_to_minute_nanos`] or
    /// [`Instant::ceil_to_minute_nanos`].
    ///
    /// ```
    /// # #[cfg(feature = "bundled-tzdb")] {
    /// use opportune::time::{Date, Instant, TimeOfDay, TimeZoneDatabase};
    ///
    /// let db = TimeZoneDatabase::bundled().unwrap();
    /// let new_york = db.zone("America/New_York").unwrap();
    /// // 02:30 does not exist on the spring-forward day: compatible resolution
    /// // moves it past the gap, to 03:30 local.
    /// let seconds = new_york
    ///     .resolve_local_compatible(
    ///         Date::new(2027, 3, 14).unwrap(),
    ///         TimeOfDay::new(2, 30).unwrap(),
    ///         15,
    ///     )
    ///     .unwrap();
    /// let start = Instant::floor_to_minute_nanos(seconds, 0).unwrap();
    /// assert_eq!(start.to_string(), "2027-03-14T07:30Z");
    /// // The stray second still carries the end into the next minute.
    /// assert_eq!(
    ///     Instant::ceil_to_minute_nanos(seconds, 0).unwrap().to_string(),
    ///     "2027-03-14T07:31Z"
    /// );
    /// # }
    /// ```
    ///
    /// [`Instant::floor_to_minute_nanos`]: crate::time::Instant::floor_to_minute_nanos
    /// [`Instant::ceil_to_minute_nanos`]: crate::time::Instant::ceil_to_minute_nanos
    ///
    /// # Errors
    /// Returns [`TimeError::SecondOutOfRange`] when `second` is not inside a
    /// minute, and [`TimeError::CivilToInstant`] when the reading has no
    /// representable civil date-time. The seconds returned are not clipped to the
    /// supported instant range: round them with
    /// [`Instant::floor_to_minute_nanos`] or [`Instant::ceil_to_minute_nanos`],
    /// which reject a value outside it.
    pub fn resolve_local_compatible(
        &self,
        date: Date,
        time: TimeOfDay,
        second: u8,
    ) -> Result<i64, TimeError> {
        let unrepresentable = || TimeError::CivilToInstant { date, time };
        if second >= 60 {
            return Err(TimeError::SecondOutOfRange { second });
        }
        let datetime = jiff::civil::DateTime::new(
            date.year(),
            date.month(),
            date.day(),
            i8::try_from(time.hour()).map_err(|_| unrepresentable())?,
            i8::try_from(time.minute()).map_err(|_| unrepresentable())?,
            i8::try_from(second).map_err(|_| unrepresentable())?,
            0,
        )
        .map_err(|_| unrepresentable())?;
        let zoned = self
            .inner
            .to_zoned_datetime(datetime)
            .map_err(|_| unrepresentable())?;
        Ok(zoned.timestamp().as_second())
    }

    /// The last instant whose local date is `date`, if any instant inside the
    /// supported range carries it.
    ///
    /// This is the instant a date-based bound has to end at: a start reads the
    /// limit's date up to and including this instant, and no start after it does.
    /// It is read back from the boundary rather than assumed to be the last
    /// midnight of the next date, because a repeated midnight can fall on either
    /// side of that boundary:
    ///
    /// * Newfoundland ended daylight saving at 00:01 local until 2010, so the
    ///   previous date's last hour comes *after* the first midnight of the next
    ///   date; there the next date's last midnight ends the previous date.
    /// * Cuba ends daylight saving at 01:00 wall time (`0:00s` standard), so local
    ///   midnight itself is repeated; there the next date's *first* midnight ends
    ///   the previous date, and the hour between the two midnights already reads
    ///   the next date.
    ///
    /// So the answer is the last midnight whose own preceding minute still reads
    /// `date`, whether or not it is also the last midnight. A midnight a forward
    /// transition skips does not occur at all, and the date then ends one minute
    /// before the transition that swallowed it.
    ///
    /// Returns `None` when no supported instant reads a date at or before `date`,
    /// and [`Instant::MAX`] when every supported instant does.
    ///
    /// # Errors
    /// Returns a [`TimeError`] only for a zone whose offsets are not whole minutes,
    /// which the database never produces inside the supported range.
    pub(crate) fn last_instant_on(&self, date: Date) -> Result<Option<Instant>, TimeError> {
        let next = date.plus_days(1)?;
        let midnights = self.local_instants(next, TimeOfDay::MIDNIGHT)?;
        if !midnights.as_slice().is_empty() {
            // The boundary is the last midnight whose preceding minute still reads
            // `date`; a fold can make the later one read the next date instead.
            let mut last = None;
            for midnight in midnights {
                let Ok(before) = Instant::from_minutes(midnight.minutes() - 1) else {
                    // `midnight` is the first supported instant, so no instant reads
                    // an earlier date and there is nothing before it to return.
                    continue;
                };
                if self.zoned(before).date() <= date {
                    last = Some(last.map_or(before, |seen: Instant| seen.max(before)));
                }
            }
            if last.is_some() {
                return Ok(last);
            }
        }
        // Either a forward transition skipped this midnight, so `next` begins at
        // the transition, or the midnights are outside the supported range.
        match self.day_start(next) {
            Ok(start) => Ok(Instant::from_minutes(start.minutes() - 1)
                .ok()
                .filter(|before| self.zoned(*before).date() <= date)),
            // `next` begins outside the supported instants. If `date` is already at
            // or past the last local date, every start reads a date at or before it;
            // otherwise no start does.
            Err(TimeError::CivilToInstant { .. } | TimeError::InstantOutOfRange { .. }) => {
                Ok((date >= self.zoned(Instant::MAX).date()).then_some(Instant::MAX))
            }
            Err(err) => Err(err),
        }
    }

    /// Whether any instant falls on `date` in this zone.
    ///
    /// Every local date between the range's own first and last date is reachable by
    /// some instant, except one a forward transition across the date line skipped
    /// outright; a date whose midnight lies below the range floor is still reached
    /// by the instants after it that day.
    pub(crate) fn date_exists(&self, date: Date) -> bool {
        let first = self.zoned(Instant::MIN).date();
        let last = self.zoned(Instant::MAX).date();
        if date < first || date > last {
            return false;
        }
        match self.day_start(date) {
            // A skipped date's first instant is the transition that swallowed it,
            // which already carries the following date.
            Ok(start) => self.zoned(start).date() <= date,
            // The date's midnight lies outside the supported range, so only its own
            // bounds decide, and they did above: the date begins at the range edge.
            Err(_) => true,
        }
    }

    /// The earliest and latest local dates the instants of `from..until` fall
    /// on in this zone, exactly.
    ///
    /// Exact rather than estimated: the local reading rises one minute per
    /// minute between transitions, so its extremes over the range sit at the
    /// range's edges or on either side of a transition, however far an offset
    /// jumps.
    pub(crate) fn local_date_bounds(
        &self,
        from: Instant,
        until: Instant,
    ) -> Result<(Date, Date), TimeError> {
        let transitions = self.transition_instants(from, until)?;
        Ok(breakpoints::dates_covering(self, from, until, &transitions))
    }
}

impl fmt::Debug for Zone {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.name)
    }
}

/// An offset from UTC.
///
/// Offsets are whole minutes everywhere in the supported instant range, which
/// is what lets interval arithmetic stay in minutes; [`UtcOffset::seconds`]
/// exposes the underlying value so a database that ever broke that promise
/// would fail loudly instead of silently truncating.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct UtcOffset {
    seconds: i32,
}

impl UtcOffset {
    /// The offset in seconds, which is `minutes * 60` for every offset in the
    /// supported range.
    #[must_use]
    pub const fn seconds(&self) -> i32 {
        self.seconds
    }

    /// The offset in whole minutes, truncating any sub-minute component.
    #[must_use]
    pub const fn minutes(&self) -> i32 {
        self.seconds / 60
    }
}

impl fmt::Debug for UtcOffset {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

impl fmt::Display for UtcOffset {
    /// Formats the offset as `UTC+HH:MM` or `UTC-HH:MM`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let sign = if self.seconds < 0 { '-' } else { '+' };
        let total = self.seconds.unsigned_abs() / 60;
        write!(f, "UTC{sign}{:02}:{:02}", total / 60, total % 60)
    }
}

/// An instant together with the local date, time and offset it falls on in one
/// zone.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct ZonedInstant {
    instant: Instant,
    date: Date,
    time: TimeOfDay,
    offset: UtcOffset,
}

impl ZonedInstant {
    /// The instant.
    #[must_use]
    pub const fn instant(&self) -> Instant {
        self.instant
    }

    /// The local date.
    #[must_use]
    pub const fn date(&self) -> Date {
        self.date
    }

    /// The local time of day.
    #[must_use]
    pub const fn time(&self) -> TimeOfDay {
        self.time
    }

    /// The zone's offset from UTC at this instant.
    #[must_use]
    pub const fn offset(&self) -> UtcOffset {
        self.offset
    }
}

impl fmt::Debug for ZonedInstant {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

impl fmt::Display for ZonedInstant {
    /// Formats the local time as `YYYY-MM-DD HH:MM UTC+HH:MM`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {} {}", self.date, self.time, self.offset)
    }
}

impl Instant {
    /// The local date and time this instant falls on in `zone`.
    ///
    /// ```
    /// # use opportune::time::{Instant, Zone};
    /// let instant = Instant::ceil_to_minute(1_804_604_400).unwrap();
    /// assert_eq!(instant.zoned(&Zone::utc()).to_string(), "2027-03-09 15:00 UTC+00:00");
    /// ```
    #[must_use]
    pub fn zoned(&self, zone: &Zone) -> ZonedInstant {
        let timestamp = self.as_jiff();
        let offset = zone.inner.to_offset(timestamp);
        let datetime = offset.to_datetime(timestamp);
        ZonedInstant {
            instant: *self,
            date: Date::from_jiff(datetime.date()),
            time: TimeOfDay::from_jiff(datetime.time()),
            offset: UtcOffset {
                seconds: offset.seconds(),
            },
        }
    }
}

/// The instants a local date and time exist at in a zone.
///
/// Daylight saving transitions make the count zero or two: a local time inside
/// a spring-forward gap does not exist, and one inside a fall-back overlap
/// exists twice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalResolution {
    /// The local time was skipped by a forward transition.
    Skipped,
    /// The local time occurs once.
    Unique(Instant),
    /// The local time occurs twice; the earlier instant comes first.
    Repeated(Instant, Instant),
}

/// Zero, one, or two values produced by resolving a local date-time, stored on
/// the stack so grid generation and breakpoint collection allocate nothing per
/// lookup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct LocalValues<T: Copy> {
    items: [T; 2],
    len: usize,
}

impl<T: Copy> LocalValues<T> {
    const fn empty(fill: T) -> Self {
        Self {
            items: [fill, fill],
            len: 0,
        }
    }

    const fn one(first: T) -> Self {
        Self {
            items: [first, first],
            len: 1,
        }
    }

    const fn two(first: T, second: T) -> Self {
        Self {
            items: [first, second],
            len: 2,
        }
    }

    fn push(&mut self, value: T) {
        debug_assert!(self.len < 2, "a local date-time occurs at most twice");
        self.items[self.len] = value;
        self.len += 1;
    }

    pub(crate) fn as_slice(&self) -> &[T] {
        &self.items[..self.len]
    }
}

impl<T: Copy> IntoIterator for LocalValues<T> {
    type Item = T;
    type IntoIter = std::iter::Take<std::array::IntoIter<T, 2>>;

    fn into_iter(self) -> Self::IntoIter {
        self.items.into_iter().take(self.len)
    }
}

/// The fixture directory the directory tests load.
#[cfg(all(test, not(target_family = "wasm")))]
fn fixture(directory: &str) -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(directory)
}

#[cfg(all(test, feature = "bundled-tzdb"))]
mod bundled_tests {
    use super::{TimeZoneDatabase, UtcOffset};
    use crate::time::{Instant, TimeOfDay, Zone};

    #[test]
    fn the_database_reports_the_bundled_version() {
        let db = TimeZoneDatabase::bundled().unwrap();
        assert_eq!(db.version(), jiff_tzdb::VERSION);
        assert!(!db.version().is_empty());
    }

    #[test]
    fn legacy_aliases_resolve_to_the_zone_they_name() {
        let db = TimeZoneDatabase::bundled().unwrap();
        let alias = db.zone("US/Eastern").unwrap();
        let primary = db.zone("America/New_York").unwrap();
        assert_eq!(alias.name(), "US/Eastern");
        for instant in [Instant::MIN, Instant::MAX] {
            assert_eq!(alias.offset_at(instant), primary.offset_at(instant));
        }
        // A spring-forward day behaves the same under either name.
        let noon = Zone::utc()
            .resolve_local(
                crate::time::Date::new(2027, 3, 14).unwrap(),
                TimeOfDay::new(12, 0).unwrap(),
            )
            .unwrap();
        let crate::time::LocalResolution::Unique(noon) = noon else {
            panic!("UTC has no transitions")
        };
        assert_eq!(alias.zoned(noon).offset(), UtcOffset { seconds: -4 * 3600 });
    }

    #[test]
    fn a_local_time_that_no_instant_can_represent_is_reported() {
        // 9999-12-31 23:59 at UTC-12 falls into the year 10000, past the last
        // instant any timestamp can hold. It is an error, not a panic.
        let db = TimeZoneDatabase::bundled().unwrap();
        let west = db.zone("Etc/GMT+12").unwrap();
        let error = west
            .resolve_local(
                crate::time::Date::new(9999, 12, 31).unwrap(),
                TimeOfDay::new(23, 59).unwrap(),
            )
            .unwrap_err();
        assert_eq!(error.code(), "civil_to_instant");
    }

    #[test]
    fn a_civil_date_begins_at_its_midnight_or_at_the_transition_that_skips_it() {
        let db = TimeZoneDatabase::bundled().unwrap();
        let date = |y, m, d| crate::time::Date::new(y, m, d).unwrap();

        // An ordinary midnight is the instant the date begins.
        let new_york = db.zone("America/New_York").unwrap();
        let midnight = new_york.day_start(date(2027, 3, 9)).unwrap();
        assert_eq!(new_york.zoned(midnight).time(), TimeOfDay::MIDNIGHT);
        assert_eq!(new_york.zoned(midnight).date(), date(2027, 3, 9));
        assert!(new_york.date_exists(date(2027, 3, 9)));

        // Santiago springs forward from midnight to 01:00 on 2027-09-05, so the
        // date begins at the transition rather than at a midnight that never
        // happens.
        let santiago = db.zone("America/Santiago").unwrap();
        let begins = santiago.day_start(date(2027, 9, 5)).unwrap();
        assert_eq!(santiago.zoned(begins).date(), date(2027, 9, 5));
        assert_eq!(santiago.zoned(begins).time(), TimeOfDay::new(1, 0).unwrap());
        assert_eq!(begins.to_string(), "2027-09-05T04:00Z");
        // The minute before it is still the previous date.
        let before = Instant::from_minutes(begins.minutes() - 1).unwrap();
        assert_eq!(santiago.zoned(before).date(), date(2027, 9, 4));

        // Samoa skipped 2011-12-30 altogether by crossing the date line, so
        // that date has no instants at all and 31 December begins where 30
        // December would have.
        let apia = db.zone("Pacific/Apia").unwrap();
        assert!(!apia.date_exists(date(2011, 12, 30)));
        assert!(apia.date_exists(date(2011, 12, 31)));
        let begins = apia.day_start(date(2011, 12, 31)).unwrap();
        assert_eq!(apia.zoned(begins).date(), date(2011, 12, 31));
    }

    #[test]
    fn a_local_date_range_is_the_exact_span_of_dates_its_instants_fall_on() {
        let db = TimeZoneDatabase::bundled().unwrap();
        let new_york = db.zone("America/New_York").unwrap();
        let utc = |y, m, d, hour| {
            let crate::time::LocalResolution::Unique(instant) = Zone::utc()
                .resolve_local(
                    crate::time::Date::new(y, m, d).unwrap(),
                    TimeOfDay::new(hour, 0).unwrap(),
                )
                .unwrap()
            else {
                panic!("UTC has no transitions")
            };
            instant
        };

        // 09:00Z to 09:00Z the next day is 04:00 on 9 March to 04:00 on 10 March
        // in New York: two local date labels, and the bounds are exactly them.
        let from = utc(2027, 3, 9, 9);
        let until = Instant::from_minutes(from.minutes() + 24 * 60).unwrap();
        let (first, last) = new_york.local_date_bounds(from, until).unwrap();
        assert_eq!(first, crate::time::Date::new(2027, 3, 9).unwrap());
        assert_eq!(last, crate::time::Date::new(2027, 3, 10).unwrap());
    }

    /// `Grid::starts` walks one local day either side of its window, which is
    /// enough exactly when no zone's offset moves *backward* by more than a day
    /// anywhere in the supported range: a candidate's local date then lies
    /// within one day of the local date at either window edge. A forward jump
    /// is harmless, because it only pushes local dates later. A database that
    /// broke this bound would make a grid silently drop candidates, so the
    /// bound is measured here rather than assumed.
    #[test]
    fn no_zone_moves_its_offset_backward_by_more_than_a_day() {
        const SECONDS_PER_DAY: i64 = 86_400;
        let db = TimeZoneDatabase::bundled().unwrap();
        let names = db.available_names();
        assert!(
            names.len() > 400,
            "the bundled database holds the IANA zones, but only {} names were listed",
            names.len()
        );

        for name in names {
            let zone = db
                .zone(&name)
                .unwrap_or_else(|err| panic!("`{name}` is listed but does not resolve: {err}"));
            // The highest offset seen so far, so the bound covers a cumulative
            // backward drift across several transitions and not only the step
            // between two adjacent ones.
            let mut highest_offset = i64::from(zone.offset_at(Instant::MIN).seconds());
            for transition in zone.inner.following(Instant::MIN.as_jiff()) {
                let seconds = transition.timestamp().as_second();
                if seconds > Instant::MAX.unix_seconds() {
                    break;
                }
                let instant = Instant::from_unix_seconds(seconds).unwrap_or_else(|err| {
                    panic!(
                        "`{name}` changes offset at {seconds}, which is not a whole minute: {err}"
                    )
                });
                let offset = i64::from(zone.offset_at(instant).seconds());
                assert!(
                    highest_offset - offset <= SECONDS_PER_DAY,
                    "`{name}` has moved backward from {highest_offset}s to {offset}s by {instant}: \
                     more than a day, so a grid window near it would miss candidates"
                );
                highest_offset = highest_offset.max(offset);
            }
        }
    }

    #[test]
    fn every_zone_has_whole_minute_offsets_across_the_supported_range() {
        let db = TimeZoneDatabase::bundled().unwrap();
        let names = db.available_names();
        assert!(
            names.len() > 400,
            "the bundled database holds the IANA zones, but only {} names were listed",
            names.len()
        );

        let mut transitions = 0usize;
        for name in names {
            let zone = db
                .zone(&name)
                .unwrap_or_else(|err| panic!("`{name}` is listed but does not resolve: {err}"));
            let check = |offset: UtcOffset| {
                assert_eq!(
                    offset.seconds() % 60,
                    0,
                    "`{name}` has a sub-minute offset of {} seconds",
                    offset.seconds()
                );
            };
            check(zone.offset_at(Instant::MIN));
            for transition in zone.inner.following(Instant::MIN.as_jiff()) {
                let seconds = transition.timestamp().as_second();
                if seconds > Instant::MAX.unix_seconds() {
                    break;
                }
                let instant = Instant::from_unix_seconds(seconds).unwrap_or_else(|err| {
                    panic!(
                        "`{name}` changes offset at {seconds}, which is not a whole minute: {err}"
                    )
                });
                check(zone.offset_at(instant));
                transitions += 1;
            }
        }
        assert!(
            transitions > 10_000,
            "the sweep should cross the database's transitions, but saw {transitions}"
        );
    }
}

#[cfg(all(test, not(target_family = "wasm")))]
mod directory_tests {
    use super::{TimeZoneDatabase, fixture};
    use crate::time::{Date, LocalResolution, TimeOfDay};

    #[test]
    fn the_version_comes_from_plus_version_then_the_caller_label() {
        let versioned = TimeZoneDatabase::from_dir(fixture("tzdb-2018"), None).unwrap();
        assert_eq!(versioned.version(), "2018f");
        // `+VERSION` wins over a caller label, so a stored label cannot
        // misreport the data being used.
        let labelled = TimeZoneDatabase::from_dir(fixture("tzdb-2018"), Some("2026f")).unwrap();
        assert_eq!(labelled.version(), "2018f");

        let unversioned = TimeZoneDatabase::from_dir(fixture("tzdb-unversioned"), None);
        assert_eq!(
            unversioned.err(),
            Some(crate::time::TimeError::TzdbVersionMissing {
                path: fixture("tzdb-unversioned").display().to_string(),
            })
        );
        let labelled =
            TimeZoneDatabase::from_dir(fixture("tzdb-unversioned"), Some("custom-2018")).unwrap();
        assert_eq!(labelled.version(), "custom-2018");
    }

    #[test]
    fn an_unusable_directory_is_a_construction_error() {
        let missing = fixture("no-such-database");
        let error = TimeZoneDatabase::from_dir(&missing, Some("2026f")).unwrap_err();
        assert_eq!(error.code(), "tzdb_directory_unusable");
        assert!(error.to_string().contains("no-such-database"));
    }

    #[test]
    fn a_directory_database_never_falls_back_to_the_system_database() {
        // The fixture holds only America/Sao_Paulo. The host database certainly
        // holds London, so a fallback would resolve it.
        let db = TimeZoneDatabase::from_dir(fixture("tzdb-2018"), None).unwrap();
        assert!(db.zone("America/Sao_Paulo").is_ok());
        assert_eq!(db.zone("Europe/London").unwrap_err().code(), "zone_unknown");
    }

    #[test]
    fn two_database_versions_resolve_the_same_local_time_differently() {
        let before = TimeZoneDatabase::from_dir(fixture("tzdb-2018"), None).unwrap();
        let after = TimeZoneDatabase::from_dir(fixture("tzdb-2019"), None).unwrap();
        assert_ne!(before.version(), after.version());

        // In December 2019 the older database still observes Brazilian
        // daylight saving; the newer one does not, because Brazil abolished it.
        let date = Date::new(2019, 12, 15).unwrap();
        let noon = TimeOfDay::new(12, 0).unwrap();
        let older = before
            .zone("America/Sao_Paulo")
            .unwrap()
            .resolve_local(date, noon)
            .unwrap();
        let newer = after
            .zone("America/Sao_Paulo")
            .unwrap()
            .resolve_local(date, noon)
            .unwrap();
        let LocalResolution::Unique(older) = older else {
            panic!("noon exists")
        };
        let LocalResolution::Unique(newer) = newer else {
            panic!("noon exists")
        };
        assert_eq!(older.to_string(), "2019-12-15T14:00Z"); // UTC-2
        assert_eq!(newer.to_string(), "2019-12-15T15:00Z"); // UTC-3
        assert_ne!(older, newer);
    }
}

#[cfg(test)]
mod tests {
    use super::{UtcOffset, Zone};
    use crate::time::{Date, Instant, LocalResolution, TimeOfDay};

    #[test]
    fn utc_needs_no_database() {
        let utc = Zone::utc();
        let noon = utc
            .resolve_local(
                Date::new(2027, 3, 9).unwrap(),
                TimeOfDay::new(12, 0).unwrap(),
            )
            .unwrap();
        let LocalResolution::Unique(instant) = noon else {
            panic!("UTC has no transitions")
        };
        assert_eq!(instant.to_string(), "2027-03-09T12:00Z");
        assert_eq!(utc.offset_at(instant), UtcOffset { seconds: 0 });
        assert_eq!(utc.offset_at(instant).minutes(), 0);
        assert_eq!(utc.offset_at(instant).to_string(), "UTC+00:00");
    }

    #[test]
    fn offsets_display_with_sign_and_minutes() {
        assert_eq!(
            UtcOffset {
                seconds: -12 * 3600
            }
            .to_string(),
            "UTC-12:00"
        );
        assert_eq!(UtcOffset { seconds: -720 }.minutes(), -12);
        assert_eq!(
            UtcOffset {
                seconds: 5 * 3600 + 45 * 60
            }
            .to_string(),
            "UTC+05:45"
        );
        assert_eq!(
            UtcOffset {
                seconds: 5 * 3600 + 45 * 60
            }
            .minutes(),
            345,
            "Kathmandu is 5 hours 45 minutes ahead"
        );
    }

    #[test]
    fn instants_round_trip_through_utc() {
        let instant = Instant::from_minutes(Instant::MIN.minutes() + 123).unwrap();
        let local = instant.zoned(&Zone::utc());
        assert_eq!(local.instant(), instant);
        assert_eq!(local.to_string(), "1975-01-01 02:03 UTC+00:00");
        let LocalResolution::Unique(back) = Zone::utc()
            .resolve_local(local.date(), local.time())
            .unwrap()
        else {
            panic!("UTC has no transitions")
        };
        assert_eq!(back, instant);
    }
}
