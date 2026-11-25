//! Candidate starts: the instants a search judges, from the goal's grid or from
//! the caller's own list.

use crate::Engine;
use crate::error::Error;
use crate::model::{Goal, Part};
use crate::occupancy;
use crate::time::{Duration, Grid, GridZone, Instant, TimeOfDay};

/// The candidate starts for the goal, in ascending order and without repeats,
/// kept inside the search window, stopping once `limit` have been found.
///
/// A goal with neither a grid nor candidates uses the default grid: the duration
/// capped at 30 minutes, anchored at 00:00 in the goal's zone.
///
/// A start is left out when its booking, widened by the request-wide travel
/// envelope — the largest effective travel, each participant's override or the
/// part's travel where they have none, that any request participant needs —
/// reaches past the supported range: every offer reports the span its booking
/// occupies and the travel to reserve, and the model has no `Span` to report
/// for a minute past either edge. The envelope is the request-wide one, so a
/// start any participant's travel could place outside the range is withheld,
/// whatever assignment the search would choose; it is narrower than the
/// part-wide `Travel::largest_before` and `Travel::largest_after`, which fold
/// the default in even when every participant overrides it. An omitted start is
/// left out of both the offers and the exclusions: nothing about it is
/// bookable, and the range ending is not a rule the caller wrote.
///
/// Validation asks whether a request produces more starts than the configured
/// maximum, so it needs at most one more than that; bounding the walk there keeps
/// a search window written far past the limits from allocating a start per
/// minute of it. A truncated answer is not sorted, because only its length is
/// meaningful.
///
/// # Errors
/// Returns an [`Error::Time`] failure when the grid's zone is unknown, or a step
/// or local date cannot be walked.
pub(crate) fn candidate_starts_limited(
    engine: &Engine,
    request: &crate::model::Request,
    limit: usize,
) -> Result<Vec<Instant>, Error> {
    let goal = &request.goal;
    let window = goal.search_window;
    let (duration, before, after) =
        goal.part()
            .map_or((Duration::ZERO, Duration::ZERO, Duration::ZERO), |part| {
                (
                    part.duration,
                    part.travel
                        .largest_before_for_participants(&request.participants),
                    part.travel
                        .largest_after_for_participants(&request.participants),
                )
            });
    let representable =
        |start: &Instant| occupancy::overflow(*start, duration, before, after).is_none();
    if let Some(candidates) = &goal.candidates {
        let mut kept: Vec<Instant> = candidates
            .iter()
            .copied()
            .filter(|start| window.contains(*start) && representable(start))
            .collect();
        kept.sort_unstable();
        kept.dedup();
        kept.truncate(limit);
        return Ok(kept);
    }
    let Some(part) = goal.part() else {
        return Ok(Vec::new());
    };
    let db = engine.time_zone_database();
    let event = db.zone(goal.zone.as_str())?;
    let grid = match &goal.grid {
        Some(grid) => grid.clone(),
        None => default_grid(goal, part),
    };
    let zone = grid.zone(db, &event)?;
    // The walk stops once `limit` starts are found, so a withheld start must not
    // consume one: begin at the first start the before-travel envelope can
    // represent. Starts at the far edge need no such care — they are a suffix of
    // the ascending walk, so they are only reached when fewer than `limit`
    // representable starts exist.
    let first = Instant::MIN.minutes().saturating_add(before.minutes());
    let from = Instant::from_minutes(window.start().minutes().max(first)).unwrap_or(Instant::MAX);
    Ok(grid
        .starts_up_to(&zone, from, window.end(), Some(limit))?
        .into_iter()
        .filter(representable)
        .collect())
}

/// The default grid: the duration capped at 30 minutes, anchored at 00:00 in the
/// goal's zone.
fn default_grid(goal: &Goal, part: &Part) -> Grid {
    let capped = part.duration.min(Duration::from_minutes(30));
    // A booking with no length is rejected by validation; the grid still needs a
    // step it accepts, so the fallback keeps this function total.
    let step = if capped.minutes() < 1 {
        Duration::from_minutes(1)
    } else {
        capped
    };
    Grid::new(
        step,
        TimeOfDay::MIDNIGHT,
        GridZone::Named(goal.zone.clone()),
    )
    .expect("a default grid's step is between 1 and 1440 minutes")
}
