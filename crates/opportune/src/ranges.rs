//! What calendar and occurrence data a request needs, before fetching any.
//!
//! The calendar range is the UTC hull of every instant whose local date, under
//! each participant's zone at that instant, falls between the day before the
//! earliest local date the padded search window touches and the day after the
//! latest — computed in the home zone and in every zone a period puts the
//! participant in near the window, so a trip across the date line cannot hide
//! earlier activity.
//!
//! The day either side is what a `rest` rule reads: a booking is excluded by a
//! late finish on the day before its own local date, or by an early start on the
//! day after it, so both neighbouring days' events must be fetched. For a
//! participant a rest rule applies to, the range is widened by the same reach the
//! engine gives rest, so a neighbouring date read in a zone the participant has
//! since left is still covered.
//!
//! The occurrence range is the hull of every bucket of every cap that applies to
//! the request and that the search window touches, read in the cap's own zone —
//! the event zone, a fixed zone, or each participant's piecewise zone. Calendar
//! data a cap counts has to be fetched over that range, not just the calendar
//! range, or a cap would see a quiet day where the caller has bookings.
//!
//! The returned hulls begin a two-phase fetch, not a one-pass query: a cap merges
//! every record sharing an event key across all supplied time ranges. After the
//! initial calendar and occurrence-range fetch, the caller collects the event keys
//! observed on every participant's own-span occurrence-range records and fetches
//! every record carrying those keys — all-day and otherwise nonmatching records
//! included, on every participant. One round settles because every record carries
//! at most one event key, and the closed request must be evaluated against a
//! consistent provider snapshot. A one-pass fetch is key-complete only when every
//! copy of an event has the identical own span; otherwise it can miss a same-key
//! record outside the hull and answer with a false offer or exclusion. When exact
//! closure is impossible, the caller fetches all occurrences and blocks across the
//! full supported time domain (`Span::EVERYTHING`) or fails closed.

use std::collections::btree_map::Entry;
use std::collections::{BTreeMap, BTreeSet};

use crate::Engine;
use crate::error::{Diagnostic, Error};
use crate::model::Span;
use crate::model::{Cap, CapPeriod, Part, RangesQuery, RequiredRanges, RuleKind};
use crate::rules::{REST_REACH_MINUTES, RuleMatcher};
use crate::time::{Date, Instant, RuleZone, TimeZoneDatabase, Zone, ZoneSchedule};

/// The calendar and occurrence data `query` needs.
///
/// The counts that multiply that computation — its participants, its rules and
/// each participant's zone periods — are bounded here rather than left to a prior
/// [`Engine::validate`](crate::Engine::validate), because this is the first engine
/// call a caller's fetch plan makes on a document. This call reads no calendar
/// block and no occurrence, matches no requirement and generates no candidate
/// start, so the collection and requirement caps on those have nothing to refuse
/// here. The window and duration caps are request-search bounds rather than fetch
/// rejection rules: padding by the booking's duration and travel is what makes
/// the fetch sufficient, and resolving each zone still reads the offset
/// transitions inside the resulting hull. A wide query can therefore produce a
/// wide fetch and its zone lookups; refusing extreme values is request
/// validation's contract.
///
/// # Errors
/// Returns [`Error::PartCount`] when the goal does not have exactly one part,
/// [`Error::InvalidRequest`] carrying the same findings the request validator
/// reports for those counts — a `participants_limit` at `/participants`, a
/// `rules_limit` at `/rules` and a `zone_periods_limit` for each participant
/// holding too many — and an [`Error::Time`] failure when a zone cannot be
/// resolved. A range that cannot be computed is never replaced by a narrower
/// one: a caller that fetched too little would read a busy day as free.
pub(crate) fn required_ranges(
    engine: &Engine,
    query: &RangesQuery,
) -> Result<RequiredRanges, Error> {
    let part = query.goal.part().ok_or(Error::PartCount {
        parts: query.goal.parts.len(),
    })?;
    // The loops below multiply participants by rules, and a cap rule's zone
    // resolution reads every period of every covered participant, so a document
    // past a cap is answered before the first participant is resolved: the work
    // the cap just refused is exactly what resolving would pay for. Every
    // offending participant is named, as the request validator names them.
    let over_cap: Vec<Diagnostic> =
        crate::validate::size_limit_diagnostics(engine.limits(), &query.participants, &query.rules)
            .collect();
    if !over_cap.is_empty() {
        return Err(Error::invalid_request(over_cap));
    }
    let db = engine.time_zone_database();

    // The padded window: the search window widened by the request-wide travel
    // envelope — the largest effective travel any request participant needs —
    // and by the clearance at each edge and the booking's duration at the end.
    // A default no participant uses widens nothing; every participant still
    // counts, including attendees and unassigned backup hosts. Fetching too
    // much costs a little; fetching too little reads a busy day as free, so the
    // two widenings add rather than compete.
    let mut clearance_before = 0;
    let mut clearance_after = 0;
    let mut has_rest = Vec::with_capacity(query.participants.len());
    // Every participant is judged for the same booking, so the half of a rule's
    // applicability that depends on the part's tags — its `when`, and a cap's
    // `count` — is decided once for the query rather than once per participant.
    let matcher = RuleMatcher::new(&query.rules, &part.tags);
    for participant in &query.participants {
        let context = participant.context();
        let applicable = matcher.select(context);
        let clearance = applicable.clearance;
        clearance_before = clearance_before.max(clearance.before.max(clearance.min_gap).minutes());
        clearance_after = clearance_after.max(clearance.after.max(clearance.min_gap).minutes());
        has_rest.push(!applicable.rests.is_empty());
    }
    let travel_before = part
        .travel
        .largest_before_for_participants(&query.participants)
        .minutes();
    let travel_after = part
        .travel
        .largest_after_for_participants(&query.participants)
        .minutes();
    let before = travel_before + clearance_before;
    let after = part.duration.minutes() + travel_after + clearance_after;
    let (padded_start, padded_end) = widened_window(query.goal.search_window, before, after)?;
    let (rest_start, rest_end) = widened_window(
        query.goal.search_window,
        before.saturating_add(REST_REACH_MINUTES),
        after.saturating_add(REST_REACH_MINUTES),
    )?;

    let mut earliest = Instant::MAX;
    let mut latest = Instant::MIN;
    // Deduplicate `(zone_name, has_rest)` in participant order so repeated zones
    // across participants or zone periods resolve and hull at most once per
    // reach width while preserving the first unknown-zone error in request order.
    let mut resolved_zones: BTreeMap<&str, Zone> = BTreeMap::new();
    let mut hulled: BTreeSet<(&str, bool)> = BTreeSet::new();
    for (participant, &rest) in query.participants.iter().zip(&has_rest) {
        let (reach_start, reach_end) = if rest {
            (rest_start, rest_end)
        } else {
            (padded_start, padded_end)
        };
        let home = participant.zone.as_str();
        let overlapping = participant
            .zone_periods
            .iter()
            // A period that ended before the window can still hold rest activity
            // on a neighbouring local date, so selection reaches as far as the
            // hull does.
            .filter(|period| reach_start < period.until() && period.from() < reach_end)
            .map(|period| period.zone().as_str());
        for zone_name in std::iter::once(home).chain(overlapping) {
            if !hulled.insert((zone_name, rest)) {
                continue;
            }
            let zone = match resolved_zones.entry(zone_name) {
                Entry::Occupied(entry) => entry.into_mut(),
                Entry::Vacant(entry) => entry.insert(db.zone(zone_name)?),
            };
            let Some((from, until)) = hull_for(zone, reach_start, reach_end)? else {
                continue;
            };
            earliest = earliest.min(from);
            latest = latest.max(until);
        }
    }
    let calendar = if earliest < latest {
        crate::model::Span::new(earliest, latest)?
    } else {
        // No participant names a zone, so the padded window is the widest range
        // the request can need.
        crate::model::Span::new(padded_start, padded_end)?
    };

    Ok(RequiredRanges {
        calendar,
        occurrences: occurrences_hull(query, db, part)?,
    })
}

/// The window widened by `before` minutes at its start and `after` at its end,
/// clamped to the supported range.
fn widened_window(window: Span, before: i64, after: i64) -> Result<(Instant, Instant), Error> {
    let start = Instant::from_minutes(
        window
            .start()
            .minutes()
            .saturating_sub(before)
            .max(Instant::MIN.minutes()),
    )?;
    let end = Instant::from_minutes(
        window
            .end()
            .minutes()
            .saturating_add(after)
            .min(Instant::MAX.minutes()),
    )?;
    Ok((start, end))
}

/// The UTC hull of every bucket, of every cap that applies to this request, that
/// the search window touches.
///
/// `None` when no cap applies, because nothing counts occurrences then. A cap
/// applies when its `when` and its `count` both match the part's tags and its
/// `who` selects at least one participant: with nobody to count for, no bucket
/// can gate the booking.
fn occurrences_hull(
    query: &RangesQuery,
    db: &TimeZoneDatabase,
    part: &Part,
) -> Result<Option<crate::model::Span>, Error> {
    let window = query.goal.search_window;
    let mut earliest = Instant::MAX;
    let mut latest = Instant::MIN;
    let mut applies = false;
    // A participant's hull reads their whole schedule, and every cap covering them
    // reads the same periods: resolving and walking it per cap would multiply the
    // request's periods by its rules. Two things are therefore computed once: the
    // schedule with the local dates its window touches, which depend on no cap, and
    // the hull, which depends on those dates and on the cap's bucket shape — its
    // period and week start — but not on the cap's limit, measure, zone or selector,
    // so it is shared by every cap with that shape. A participant no
    // participant-zone cap covers is never resolved, so a schedule `validate` would
    // report still surfaces only where it is read.
    let mut reads: BTreeMap<usize, ParticipantRead> = BTreeMap::new();
    let mut hulls: BTreeMap<HullKey, Hull> = BTreeMap::new();
    for rule in &query.rules {
        let RuleKind::Cap(cap) = &rule.kind else {
            continue;
        };
        if !rule.when.matches_item(&part.tags) || !cap.count.matches_item(&part.tags) {
            continue;
        }
        let covered: Vec<usize> = query
            .participants
            .iter()
            .enumerate()
            .filter(|(_, participant)| rule.who.matches_participant(participant.context()))
            .map(|(index, _)| index)
            .collect();
        if covered.is_empty() {
            continue;
        }
        applies = true;
        match &cap.zone {
            RuleZone::Participant => {
                for index in covered {
                    let key = (index, cap.period.as_str(), cap.week_starts.index());
                    let hull = if let Some(hull) = hulls.get(&key) {
                        *hull
                    } else {
                        let read = match reads.entry(index) {
                            Entry::Occupied(entry) => entry.into_mut(),
                            Entry::Vacant(entry) => entry.insert({
                                let schedule =
                                    crate::eval::resolve_schedule(db, &query.participants[index])?;
                                let dates = window_dates(&schedule, window)?;
                                ParticipantRead {
                                    runs: zone_runs(&schedule),
                                    dates,
                                }
                            }),
                        };
                        let hull = schedule_hull(read, cap)?;
                        hulls.insert(key, hull);
                        hull
                    };
                    if let Some((from, until)) = hull {
                        earliest = earliest.min(from);
                        latest = latest.max(until);
                    }
                }
            }
            RuleZone::Event | RuleZone::Named(_) => {
                let zone = fixed_zone(cap, db, query)?;
                if let Some((from, until)) = zone_hull(&zone, cap, window.start(), window.end())? {
                    earliest = earliest.min(from);
                    latest = latest.max(until);
                }
            }
        }
    }
    if !applies {
        return Ok(None);
    }
    Ok(Some(crate::model::Span::new(earliest, latest)?))
}

/// One participant's bucket-independent reading: the local dates its window
/// touches, and the runs its schedule is made of. Both depend on the participant
/// and the query only, so every cap covering them reads the one copy.
struct ParticipantRead {
    /// Every zone the participant is ever in, with the runs it is in force for.
    runs: Vec<ZoneRuns>,
    /// The first and last local dates the search window touches.
    dates: Option<(Date, Date)>,
}

/// One zone of a participant's schedule, with every run it is in force for.
struct ZoneRuns {
    zone: Zone,
    /// The runs the zone is in force for, ascending and disjoint.
    runs: Vec<(Instant, Instant)>,
}

/// The runs one zone is in force for, by zone name, while they are collected.
type GroupedRuns<'a> = BTreeMap<&'a str, (Zone, Vec<(Instant, Instant)>)>;

/// The stretches the schedule spends in each zone, grouped by zone and in time
/// order, over the whole supported range.
///
/// A bucket hull needs only the first and last run its bucket range touches, so
/// grouping the runs by zone lets one binary search each answer that without
/// walking the schedule once per bucket shape. The whole range is used rather than
/// the reach one shape happens to need, so the answer depends on the schedule
/// alone: every run a bucket can touch is in the list.
fn zone_runs(schedule: &ZoneSchedule) -> Vec<ZoneRuns> {
    let mut grouped: GroupedRuns<'_> = BTreeMap::new();
    let mut cursor = Instant::MIN;
    for period in schedule.periods() {
        group(&mut grouped, schedule.home(), cursor, period.from());
        group(&mut grouped, period.zone(), period.from(), period.until());
        cursor = cursor.max(period.until());
    }
    if cursor < Instant::MAX {
        group(&mut grouped, schedule.home(), cursor, Instant::MAX);
    }
    grouped
        .into_values()
        .map(|(zone, runs)| ZoneRuns { zone, runs })
        .collect()
}

/// Adds `from..until` to `zone`'s run list, dropping an empty stretch.
fn group<'a>(grouped: &mut GroupedRuns<'a>, zone: &'a Zone, from: Instant, until: Instant) {
    if from < until {
        grouped
            .entry(zone.name())
            .or_insert_with(|| (zone.clone(), Vec::new()))
            .1
            .push((from, until));
    }
}

/// One participant's bucket hull, keyed by the participant and the cap's bucket
/// shape: the period and the weekday a week starts on.
type HullKey = (usize, &'static str, u8);

/// The UTC hull of the buckets a cap touches for one participant, or `None` when
/// the window touches none of them.
type Hull = Option<(Instant, Instant)>;

/// The fixed zone a cap's buckets are read in.
fn fixed_zone(cap: &Cap, db: &TimeZoneDatabase, query: &RangesQuery) -> Result<Zone, Error> {
    Ok(match &cap.zone {
        RuleZone::Named(name) => db.zone(name.as_str())?,
        // A participant-zone cap is handled leg by leg; a pooled cap cannot name
        // the participant zone, which validation reports.
        RuleZone::Event | RuleZone::Participant => db.zone(query.goal.zone.as_str())?,
    })
}

/// The bucket hull of a cap read in a participant's piecewise zone.
///
/// Each leg of the schedule is judged in its own zone, so a trip across a date
/// line contributes the buckets of both zones and a label that occurs twice is
/// fetched twice over. Gaps between legs belong to the home zone.
fn schedule_hull(read: &ParticipantRead, cap: &Cap) -> Result<Hull, Error> {
    // A bucket label belongs to the date an event's own start reads in the zone
    // in force at that instant, not to the zone in force during the window. Two
    // instants in different zones can share one label — a booking during a Tokyo
    // leg and an existing London meeting later the same week are both in that
    // week's bucket — so the hull has to cover every instant whose own-zone date
    // falls on a label the window touches, in every leg and home stretch that
    // overlaps those dates.
    let Some((first, last)) = read.dates else {
        return Ok(None);
    };
    let label_min = crate::caps::bucket_label(first, cap.period, cap.week_starts);
    let label_after = bucket_after(
        crate::caps::bucket_label(last, cap.period, cap.week_starts),
        cap.period,
    )?;

    let mut hull: Option<(Instant, Instant)> = None;
    // Each zone reads the labels at its own offset, so its bucket is one fixed
    // span. The earliest minute a run of that zone contributes is the first run's
    // start and the latest is the last run's end: every run between them lies
    // inside the span. Two binary searches therefore answer this zone's hull
    // without walking its runs, and a zone whose runs sit either side of the span
    // without touching it contributes nothing.
    for ZoneRuns { zone, runs } in &read.runs {
        let bucket_from = day_start_clamped(zone, label_min, Instant::MIN);
        let bucket_until = bucket_end(zone, label_after)?;
        if bucket_from >= bucket_until {
            continue;
        }
        let Some(&(from, _)) = runs.get(runs.partition_point(|(_, until)| *until <= bucket_from))
        else {
            continue;
        };
        let Some(index) = runs
            .partition_point(|(start, _)| *start < bucket_until)
            .checked_sub(1)
        else {
            continue;
        };
        let start = from.max(bucket_from);
        let end = runs[index].1.min(bucket_until);
        if start >= end {
            continue;
        }
        hull = Some(match hull {
            Some((earliest, latest)) => (earliest.min(start), latest.max(end)),
            None => (start, end),
        });
    }
    Ok(hull)
}

/// The first and last local dates the search window touches, each read in the zone
/// in force at that instant.
///
/// Each window instant is read in the zone in force there, so a trip that moves
/// the local date gives the dates the person actually experiences. Only the
/// extremes come back, not a label: the caller buckets them, and bucketing is
/// monotone in the date, so the bucket of the earliest date is the earliest bucket
/// and the bucket after the latest date ends last. That keeps this walk — the one
/// per-instant zone reading in the hull — independent of the cap reading it, which
/// is why it is paid once per participant rather than once per cap shape.
fn window_dates(
    schedule: &ZoneSchedule,
    window: crate::model::Span,
) -> Result<Option<(Date, Date)>, Error> {
    let mut first: Option<Date> = None;
    let mut last: Option<Date> = None;
    for (zone, from, until) in schedule.legs(window.start(), window.end()) {
        if until <= from {
            continue;
        }
        let (earliest, latest) = zone.local_date_bounds(from, until)?;
        first = Some(first.map_or(earliest, |current| current.min(earliest)));
        last = Some(last.map_or(latest, |current| current.max(latest)));
    }
    Ok(first.zip(last))
}

/// The hull of the buckets a cap touches over `from..until` in one zone: from
/// the start of the bucket holding the earliest local date the range touches to
/// the end of the bucket holding the latest.
fn zone_hull(
    zone: &Zone,
    cap: &Cap,
    from: Instant,
    until: Instant,
) -> Result<Option<(Instant, Instant)>, Error> {
    if until <= from {
        return Ok(None);
    }
    let (first, last) = zone.local_date_bounds(from, until)?;
    let first_label = crate::caps::bucket_label(first, cap.period, cap.week_starts);
    let last_label = crate::caps::bucket_label(last, cap.period, cap.week_starts);
    let start = day_start_clamped(zone, first_label, Instant::MIN);
    let end = bucket_end(zone, bucket_after(last_label, cap.period)?)?;
    Ok((start < end).then_some((start, end)))
}

/// The instant the bucket ending at `date` is over: one minute after the last
/// instant that still reads the date before `date`.
///
/// `date` is the first date of the next bucket, so the bucket ends where the
/// previous date's last instant ends. Reading that back through
/// [`Zone::last_instant_on`] is exact for every repeated-midnight shape: a
/// backward transition that starts at midnight (Cuba's 01:00 wall back to 00:00)
/// means the previous date already ended at the *first* midnight, while one inside
/// the hour before it (Newfoundland's 00:01 back to 23:01) leaves the previous
/// date's tail after it. A midnight skipped by a forward transition has none, so
/// the clock's first instant of the date stands in.
fn bucket_end(zone: &Zone, date: Date) -> Result<Instant, Error> {
    let Some(previous) = date.plus_days(-1).ok() else {
        return Ok(day_start_clamped(zone, date, Instant::MAX));
    };
    match zone.last_instant_on(previous)? {
        Some(last) => Ok(Instant::from_minutes(last.minutes() + 1).unwrap_or(Instant::MAX)),
        // No supported instant reads the previous date, so this bucket has no
        // instants and its end is the edge of the supported range.
        None => Ok(day_start_clamped(zone, date, Instant::MAX)),
    }
}

/// The date the bucket labelled `label` ends at: the start of the next bucket.
fn bucket_after(label: Date, period: CapPeriod) -> Result<Date, Error> {
    Ok(match period {
        CapPeriod::Day => label.plus_days(1)?,
        CapPeriod::Week => label.plus_days(7)?,
        CapPeriod::Month => label.next_month_start()?,
    })
}

/// The UTC hull, in `zone`, of the local days from one day before the earliest
/// local date `padded_start..padded_end` touches to one day after the latest.
///
/// The hull starts when the first of those days begins and ends when the last
/// begins for the last time, so a backward offset transition that repeats a
/// midnight keeps the tail of the day before it rather than cutting the fetch
/// short.
///
/// `None` when the padded range is empty, which a search window that covers no
/// instant cannot be in a well-formed request.
fn hull_for(
    zone: &Zone,
    padded_start: Instant,
    padded_end: Instant,
) -> Result<Option<(Instant, Instant)>, Error> {
    if padded_end <= padded_start {
        return Ok(None);
    }
    let (first, last) = zone.local_date_bounds(padded_start, padded_end)?;
    let Some(from_date) = first.plus_days(-1).ok() else {
        return Ok(Some((Instant::MIN, Instant::MAX)));
    };
    let Some(until_date) = last.plus_days(2).ok() else {
        return Ok(Some((Instant::MIN, Instant::MAX)));
    };
    // A date below the supported range starts at the first supported instant,
    // and one above it starts past the last: clamping keeps the hull exact where
    // it can be and as wide as possible where the calendar runs out.
    let from = day_start_clamped(zone, from_date, Instant::MIN);
    let until = bucket_end(zone, until_date)?;
    Ok(Some((from, until)))
}

/// The instant `date` begins at in `zone`, clamped to the supported range.
///
/// `at_edge` is returned when the date lies outside the range the instants can
/// represent, which is the direction that widens the hull rather than narrowing
/// it.
fn day_start_clamped(zone: &Zone, date: Date, at_edge: Instant) -> Instant {
    if date <= Instant::MIN.zoned(zone).date() {
        return Instant::MIN;
    }
    if date > Instant::MAX.zoned(zone).date() {
        return Instant::MAX;
    }
    zone.day_start(date).unwrap_or(at_edge)
}

/// A hull's end must be where the date before its bucket actually ends, which is
/// not always the last midnight of the next date: Cuba ended daylight saving at
/// 01:00 wall time, so local midnight of 2018-11-04 happened twice and the hour
/// between the two midnights already read the 4th. Taking the last midnight would
/// hand an adapter an hour whose own-zone date belongs to the next bucket.
#[cfg(all(test, feature = "bundled-tzdb"))]
mod tests {
    use super::bucket_end;
    use crate::time::{Date, LocalResolution, TimeOfDay, TimeZoneDatabase};

    #[test]
    fn a_bucket_ending_on_a_midnight_starting_fold_stops_at_the_first_midnight() {
        let db = TimeZoneDatabase::bundled().unwrap();
        let havana = db.zone("America/Havana").unwrap();
        let next = Date::new(2018, 11, 4).unwrap();
        let LocalResolution::Repeated(first, second) =
            havana.resolve_local(next, TimeOfDay::MIDNIGHT).unwrap()
        else {
            panic!("2018-11-04 begins twice in Havana");
        };
        assert_eq!(second.minutes() - first.minutes(), 60);
        assert_eq!(
            havana.zoned(second).date(),
            next,
            "the second midnight already reads the next date"
        );

        // The bucket for 2018-11-03 ends where the 4th begins, at the first
        // midnight — not an hour later at the repeated one.
        assert_eq!(bucket_end(&havana, next).unwrap(), first);
    }
}
