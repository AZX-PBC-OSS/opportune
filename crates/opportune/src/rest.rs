//! The `rest` rule: a late finish blocks an early start.
//!
//! *Activity* is the occupied spans of a participant's blocking, non-all-day
//! blocks plus the booking being offered. For a local date `D`, `late(D)` is the
//! part of `D` at or after `late_after` and `early(D)` the part before
//! `not_before`, both read in the participant's zone at each instant. A booking is
//! excluded when activity overlaps both `late(D)` and `early(D + 1)` for some `D`,
//! and the booking itself is part of one of those overlaps. All-day blocks,
//! blackouts and the events the request excludes never count as activity, and each
//! rest rule is evaluated on its own: two rules never combine one rule's
//! `late_after` with another's `not_before`.
//!
//! The minute sets are built over a range wider than the judged one, because
//! activity a local day either side of a booking can exclude it.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use crate::error::Error;
use crate::eval::BlockedSpan;
use crate::intervals::{IntervalSet, Span as MinuteSpan};
use crate::rules::{Applicable, REST_REACH_MINUTES};
use crate::time::{
    Date, Instant, RuleZone, TimeOfDay, WallEnd, WallRange, Weekday, Weekdays, WeeklyKey,
    WindowSpec, ZoneSchedule,
};

/// Cached `(late, early)` minute sets for one `(rule_position, home_zone_name)`.
type RestPair = (Arc<IntervalSet>, Arc<IntervalSet>);

/// Request-scoped memo for `rest` rules on participants without zone periods.
///
/// Every participant staying in the same home zone shares identical `late` and
/// `early` minute sets, identical date-jump lists, and (for equal occupied
/// lengths) identical self-conflict occupied-start spans. Keying by
/// `(rule_position, home_zone_name)` keeps peak memory and build time linear in
/// unique zones rather than multiplying by participants.
#[derive(Default)]
pub(crate) struct RestMemo<'a> {
    /// Cached `(late, early)` sets for fixed-home-zone schedules.
    sets: BTreeMap<(usize, &'a str), RestPair>,
    /// Cached date-jump lists for fixed-home-zone schedules over the request's
    /// rest reach.
    jumps: BTreeMap<&'a str, Vec<i64>>,
    /// Cached self-conflicting occupied-start spans for `(rule_position,
    /// home_zone_name, occupied_len)`.
    self_blocked: BTreeMap<(usize, &'a str, i64), Arc<IntervalSet>>,
}

impl<'a> RestMemo<'a> {
    fn date_jumps(
        &mut self,
        schedule: &ZoneSchedule,
        home_zone_name: &'a str,
        from: Instant,
        until: Instant,
    ) -> Result<Vec<i64>, Error> {
        if !schedule.periods().is_empty() {
            return Ok(schedule.date_jumps(from, until)?);
        }
        if let Some(cached) = self.jumps.get(home_zone_name) {
            return Ok(cached.clone());
        }
        let computed = schedule.date_jumps(from, until)?;
        self.jumps.insert(home_zone_name, computed.clone());
        Ok(computed)
    }

    fn rule_sets(
        &mut self,
        position: usize,
        rest: crate::model::Rest,
        schedule: &ZoneSchedule,
        home_zone_name: &'a str,
        reach: MinuteSpan,
    ) -> Result<RestPair, Error> {
        if !schedule.periods().is_empty() {
            let (late, early) = build_rest_sets(rest, schedule, reach)?;
            return Ok((Arc::new(late), Arc::new(early)));
        }
        let key = (position, home_zone_name);
        if let Some((late, early)) = self.sets.get(&key) {
            return Ok((Arc::clone(late), Arc::clone(early)));
        }
        let (late, early) = build_rest_sets(rest, schedule, reach)?;
        let pair = (Arc::new(late), Arc::new(early));
        self.sets
            .insert(key, (Arc::clone(&pair.0), Arc::clone(&pair.1)));
        Ok(pair)
    }

    #[allow(clippy::too_many_arguments)]
    fn push_self_blocked(
        &mut self,
        position: usize,
        late: &IntervalSet,
        early: &IntervalSet,
        schedule: &ZoneSchedule,
        home_zone_name: &'a str,
        jumps: &[i64],
        occupied_len: i64,
        out: &mut Vec<MinuteSpan>,
    ) {
        if occupied_len <= 1 {
            return;
        }
        if !schedule.periods().is_empty() {
            out.extend(self_conflict_spans(
                late,
                early,
                schedule,
                jumps,
                occupied_len,
            ));
            return;
        }
        let key = (position, home_zone_name, occupied_len);
        let cached = if let Some(set) = self.self_blocked.get(&key) {
            Arc::clone(set)
        } else {
            let set = Arc::new(IntervalSet::from_spans(self_conflict_spans(
                late,
                early,
                schedule,
                jumps,
                occupied_len,
            )));
            self.self_blocked.insert(key, Arc::clone(&set));
            set
        };
        out.extend_from_slice(cached.spans());
    }
}

/// One applicable `rest` rule, with its late and early minutes and the local
/// dates the participant's existing activity reaches them on.
pub(crate) struct RestActivity<'a> {
    /// The rule that supplied the exclusion, for the reason's citation.
    pub(crate) rule: &'a str,
    /// The minutes whose local time is at or after `late_after`.
    pub(crate) late: Arc<IntervalSet>,
    /// The minutes whose local time is before `not_before`.
    pub(crate) early: Arc<IntervalSet>,
    /// Local dates on which the participant's existing activity has a minute in
    /// `late`.
    pub(crate) late_dates: BTreeSet<Date>,
    /// Local dates on which it has a minute in `early`.
    pub(crate) early_dates: BTreeSet<Date>,
}

/// Each applicable `rest` rule, with its late and early minutes and the local
/// dates the participant's existing activity reaches them on, plus the instants
/// the date walk has to stop at besides local midnight and the precomputed set
/// of occupied-start minutes where a booking of length `occupied_len` conflicts
/// with itself across `(D, D + 1)`.
///
/// Activity is the occupied spans of blocking, non-all-day blocks: `blocked`
/// already drops blocks a `status_policy` ignores and the events the request
/// excludes. All-day blocks and blackouts are not activity, and existing conflicts
/// between other events only exclude a booking when the booking itself is part of
/// the overlap.
///
/// # Errors
/// Returns an [`Error::Time`] failure only for a zone whose offsets are not whole
/// minutes, which the database never produces inside the supported range.
pub(crate) fn rest_time<'a>(
    rules: &Applicable<'a>,
    blocked: &[BlockedSpan<'_>],
    schedule: &ZoneSchedule,
    home_zone_name: &'a str,
    judged: MinuteSpan,
    occupied_len: i64,
    memo: &mut RestMemo<'a>,
) -> Result<(Vec<RestActivity<'a>>, Vec<i64>, IntervalSet), Error> {
    if rules.rests.is_empty() {
        return Ok((Vec::new(), Vec::new(), IntervalSet::new()));
    }
    // The walk reaches `REST_REACH_MINUTES` either side of the judged range, which
    // can put its end past the last representable instant. `build_minutes` includes
    // that last minute, and nothing beyond it exists to walk.
    let reach = MinuteSpan::new(
        judged
            .start()
            .saturating_sub(REST_REACH_MINUTES)
            .max(Instant::MIN.minutes()),
        judged.end().saturating_add(REST_REACH_MINUTES),
    )?;
    let (from_instant, until_instant) = span_instants(reach);
    let jumps = memo.date_jumps(schedule, home_zone_name, from_instant, until_instant)?;
    let activity = IntervalSet::from_spans(
        blocked
            .iter()
            .filter(|blocked| !blocked.block.all_day)
            .filter_map(|blocked| {
                // Activity is clipped to the minutes that exist, including the last
                // one, which `build_minutes` judges too.
                clipped(
                    blocked.occupied,
                    reach.start(),
                    reach.end().min(Instant::MAX.minutes() + 1),
                )
            })
            .collect(),
    );
    let mut rests = Vec::with_capacity(rules.rests.len());
    let mut self_blocked_spans: Vec<MinuteSpan> = Vec::new();
    for &(rule, position, rest) in &rules.rests {
        let (late, early) = memo.rule_sets(position, *rest, schedule, home_zone_name, reach)?;
        let mut late_dates = BTreeSet::new();
        let mut early_dates = BTreeSet::new();
        if !activity.is_empty() {
            set_dates(
                &activity.intersection(&late),
                schedule,
                &jumps,
                &mut late_dates,
            );
            set_dates(
                &activity.intersection(&early),
                schedule,
                &jumps,
                &mut early_dates,
            );
        }
        memo.push_self_blocked(
            position,
            &late,
            &early,
            schedule,
            home_zone_name,
            &jumps,
            occupied_len,
            &mut self_blocked_spans,
        );
        rests.push(RestActivity {
            rule,
            late,
            early,
            late_dates,
            early_dates,
        });
    }
    let self_blocked = IntervalSet::from_spans(self_blocked_spans);
    Ok((rests, jumps, self_blocked))
}

/// Builds the `(late, early)` minute sets for one `rest` rule over `reach`.
fn build_rest_sets(
    rest: crate::model::Rest,
    schedule: &ZoneSchedule,
    reach: MinuteSpan,
) -> Result<(IntervalSet, IntervalSet), Error> {
    let late = crate::time::build_minutes(
        &daily_span(rest.late_after, WallEnd::Midnight),
        schedule,
        reach,
    )?;
    let early = if rest.not_before == TimeOfDay::MIDNIGHT {
        // A day has no protected part before midnight, so nothing is early.
        IntervalSet::new()
    } else {
        crate::time::build_minutes(
            &daily_span(TimeOfDay::MIDNIGHT, WallEnd::Time(rest.not_before)),
            schedule,
            reach,
        )?
    };
    Ok((late, early))
}

/// The occupied-start minutes `s` where `[s, s + occupied_len)` overlaps both
/// `late(D)` and `early(D + 1)` for some local date `D`.
fn self_conflict_spans(
    late: &IntervalSet,
    early: &IntervalSet,
    schedule: &ZoneSchedule,
    jumps: &[i64],
    occupied_len: i64,
) -> Vec<MinuteSpan> {
    if occupied_len <= 1 || late.is_empty() || early.is_empty() {
        return Vec::new();
    }
    let mut late_by_date: BTreeMap<Date, Vec<(i64, i64)>> = BTreeMap::new();
    for span in late.spans() {
        schedule.for_each_date_segment(jumps, span.start(), span.end(), |from, until, date| {
            late_by_date.entry(date).or_default().push((from, until));
        });
    }
    let mut early_by_date: BTreeMap<Date, Vec<(i64, i64)>> = BTreeMap::new();
    for span in early.spans() {
        schedule.for_each_date_segment(jumps, span.start(), span.end(), |from, until, date| {
            early_by_date.entry(date).or_default().push((from, until));
        });
    }
    let mut blocked = Vec::new();
    for (day, late_segments) in &late_by_date {
        let Ok(next) = day.plus_days(1) else {
            continue;
        };
        let Some(early_segments) = early_by_date.get(&next) else {
            continue;
        };
        intersect_day_segments(late_segments, early_segments, occupied_len, &mut blocked);
    }
    blocked
}

/// Appends the occupied-start minutes whose booking overlaps both a segment of
/// `late` and a segment of `early`, where the two slices are the raw segments one
/// local date carries.
///
/// A booking of length `occupied_len` starting at `s` overlaps `[start, end)` iff
/// `s` is in `[start - occupied_len + 1, end)`. So each side is widened once, the
/// two widened sides are canonicalised with [`IntervalSet::from_spans`], and the
/// canonical sets are intersected: over sets of minutes,
/// `union(intersect(widen(l), widen(e)))` is `intersect(union(widen(l)),
/// union(widen(e)))`, however many segments each date carries.
///
/// Widening comes before the canonicalisation, so the widened segments are what
/// [`IntervalSet::from_spans`] merges: widening can make neighbouring segments
/// overlap, and the merged set is the one a booking start has to fall in. The
/// widening is the saturating `x.saturating_sub(occupied_len).saturating_add(1)`
/// rather than `x.saturating_sub(occupied_len - 1)`, which differs from it at the
/// bottom of the representable range.
fn intersect_day_segments(
    late: &[(i64, i64)],
    early: &[(i64, i64)],
    occupied_len: i64,
    out: &mut Vec<MinuteSpan>,
) {
    let late = widened_segments(late, occupied_len);
    let early = widened_segments(early, occupied_len);
    out.extend_from_slice(late.intersection(&early).spans());
}

/// One date's raw segments with each start moved back by `occupied_len - 1`
/// minutes in the saturating form, as the canonical set of minutes a booking of
/// that length may start at to reach any of them.
///
/// A segment shorter than its own widening covers no start and is left out, which
/// is what the pairwise form of the same arithmetic produced for it. Whenever
/// `self_conflict_spans` reaches this helper (`occupied_len` above one), the
/// widened start stays before the segment's own end, so nothing is dropped.
fn widened_segments(segments: &[(i64, i64)], occupied_len: i64) -> IntervalSet {
    IntervalSet::from_spans(
        segments
            .iter()
            .filter_map(|&(start, end)| {
                let widened = start.saturating_sub(occupied_len).saturating_add(1);
                (widened < end).then(|| MinuteSpan::from_valid(widened, end))
            })
            .collect(),
    )
}

/// The earliest applicable `rest` rule that excludes the booking occupying
/// `occupied`, or `None` when every rule passes.
///
/// Every rule is judged on its own: two rules never combine one rule's
/// `late_after` with another rule's `not_before`. The booking's late and early
/// dates are read from the same minute sets the existing activity was classified
/// with, so a booking that crosses local midnight from a late minute into the
/// protected morning excludes itself.
pub(crate) fn rest_conflict<'a>(
    rests: &[RestActivity<'a>],
    schedule: &ZoneSchedule,
    jumps: &[i64],
    occupied: &MinuteSpan,
) -> Option<&'a str> {
    for rest in rests {
        let mut booking_late = BTreeSet::new();
        let mut booking_early = BTreeSet::new();
        span_dates(&rest.late, occupied, schedule, jumps, &mut booking_late);
        span_dates(&rest.early, occupied, schedule, jumps, &mut booking_early);
        // A pair `(D, D + 1)` involves the booking iff `D` is in `booking_late`
        // (and `D + 1` is early in existing activity or the booking itself) or
        // `D + 1` is in `booking_early` (and `D` is late in existing activity).
        // Driving the check from the booking's own dates skips scanning the
        // calendar's dates on candidate starts that touch neither side.
        let excluded = booking_late.iter().any(|day| {
            day.plus_days(1)
                .is_ok_and(|next| rest.early_dates.contains(&next) || booking_early.contains(&next))
        }) || booking_early.iter().any(|next| {
            next.plus_days(-1)
                .is_ok_and(|day| rest.late_dates.contains(&day))
        });
        if excluded {
            return Some(rest.rule);
        }
    }
    None
}

/// Whether the booking itself crosses from a late minute of some local date `D`
/// into a protected early minute of `D + 1` under `rest`.
pub(crate) fn rest_self_conflict(
    rest: &RestActivity<'_>,
    schedule: &ZoneSchedule,
    jumps: &[i64],
    occupied: &MinuteSpan,
) -> bool {
    let mut booking_late = BTreeSet::new();
    let mut booking_early = BTreeSet::new();
    span_dates(&rest.late, occupied, schedule, jumps, &mut booking_late);
    span_dates(&rest.early, occupied, schedule, jumps, &mut booking_early);
    booking_late.iter().any(|day| {
        day.plus_days(1)
            .is_ok_and(|next| booking_early.contains(&next))
    })
}

/// A specification open on every day from `from` until `until`, read in the
/// participant's zone at each instant.
///
/// Rest reads its late and early minutes this way, so instant membership is
/// decided by the same wall-clock rule windows use, transitions included.
fn daily_span(from: TimeOfDay, until: WallEnd) -> WindowSpec {
    let range = WallRange::new(from, until)
        .expect("a range from one time of day to a later part of the day is non-empty");
    let week = WeeklyKey::new(
        Weekdays::range(Weekday::Monday, Weekday::Sunday),
        vec![range],
    )
    .expect("one key covering every day is valid");
    WindowSpec::new(RuleZone::Participant, vec![week], Vec::new())
        .expect("one weekly key with no date overrides is a valid window")
}

/// `span` narrowed to `from..until`, or `None` when it falls outside.
fn clipped(span: MinuteSpan, from: i64, until: i64) -> Option<MinuteSpan> {
    MinuteSpan::new(span.start().max(from), span.end().min(until)).ok()
}

/// Adds every local date the minutes of `set` fall on.
fn set_dates(
    set: &IntervalSet,
    schedule: &ZoneSchedule,
    jumps: &[i64],
    dates: &mut BTreeSet<Date>,
) {
    for span in set.spans() {
        schedule.dates_in(jumps, span.start(), span.end(), dates);
    }
}

/// Adds every local date the minutes of `set` inside `span` fall on.
fn span_dates(
    set: &IntervalSet,
    span: &MinuteSpan,
    schedule: &ZoneSchedule,
    jumps: &[i64],
    dates: &mut BTreeSet<Date>,
) {
    let spans = set.spans();
    let first = spans.partition_point(|candidate| candidate.end() <= span.start());
    for candidate in &spans[first..] {
        if candidate.start() >= span.end() {
            break;
        }
        schedule.dates_in(
            jumps,
            candidate.start().max(span.start()),
            candidate.end().min(span.end()),
            dates,
        );
    }
}

/// Collects every constant-date segment of `set` whose local date satisfies
/// `predicate`.
pub(crate) fn collect_matching_date_spans(
    set: &IntervalSet,
    schedule: &ZoneSchedule,
    jumps: &[i64],
    predicate: impl Fn(Date) -> bool,
    out: &mut Vec<MinuteSpan>,
) {
    for span in set.spans() {
        schedule.for_each_date_segment(jumps, span.start(), span.end(), |from, until, date| {
            if predicate(date)
                && let Ok(segment) = MinuteSpan::new(from, until)
            {
                out.push(segment);
            }
        });
    }
}

/// The instants a minute range is read as: its start, and the last supported
/// instant when its end lies past it.
///
/// Nothing exists past `Instant::MAX`, so a range reaching beyond it — `rest`
/// reads further than the range it judges — is read as ending there, which is the
/// instant interval construction takes as its exclusive end.
fn span_instants(judged: MinuteSpan) -> (Instant, Instant) {
    (
        Instant::from_minutes(judged.start()).unwrap_or(Instant::MIN),
        Instant::from_minutes(judged.end()).unwrap_or(Instant::MAX),
    )
}

#[cfg(test)]
mod tests {
    use super::{
        MinuteSpan, ZoneSchedule, intersect_day_segments, self_conflict_spans, widened_segments,
    };
    use crate::intervals::IntervalSet;
    use crate::time::{Date, LocalResolution, TimeOfDay, Zone};

    /// The pre-optimisation construction: every late segment against every early
    /// segment, one product at a time.
    ///
    /// It reproduces the arithmetic this module used before widening each side
    /// once, so a difference in either the overlaps or the boundary arithmetic
    /// shows up as a disagreement here rather than as a silently different answer.
    fn pairwise_reference(
        late: &[(i64, i64)],
        early: &[(i64, i64)],
        occupied_len: i64,
    ) -> Vec<MinuteSpan> {
        let mut spans = Vec::new();
        for &(l0, l1) in late {
            for &(e0, e1) in early {
                let from = l0.max(e0).saturating_sub(occupied_len).saturating_add(1);
                let until = l1.min(e1);
                if let Ok(span) = MinuteSpan::new(from, until) {
                    spans.push(span);
                }
            }
        }
        spans
    }

    /// The UTC minute `hour:minute` of a June 2027 day falls on.
    fn minute(day: i8, hour: u8, minute: u8) -> i64 {
        let date = Date::new(2027, 6, day).expect("a June 2027 date");
        let time = TimeOfDay::new(hour, minute).expect("a time of day");
        match Zone::utc().resolve_local(date, time) {
            Ok(LocalResolution::Unique(instant)) => instant.minutes(),
            Ok(_) => panic!("UTC has no skipped or repeated local times"),
            Err(err) => panic!("a fixture instant is inside the supported range: {err}"),
        }
    }

    /// One day pair's raw segments: `late(D)` and `early(D + 1)`.
    type DayPair<'a> = (&'a [(i64, i64)], &'a [(i64, i64)]);

    /// The canonical minutes a construction covers.
    fn covered(spans: &[MinuteSpan]) -> IntervalSet {
        IntervalSet::from_spans(spans.to_vec())
    }

    #[test]
    fn a_day_level_construction_covers_the_same_minutes_as_the_pairwise_product() {
        let late = [(600, 660), (700, 780), (1_000, 1_020)];
        let early = [(500, 640), (750, 800), (1_010, 1_500)];
        for occupied_len in [2, 3, 7, 30, 61, 120, 400] {
            let mut widened = Vec::new();
            intersect_day_segments(&late, &early, occupied_len, &mut widened);
            assert_eq!(
                covered(&widened),
                covered(&pairwise_reference(&late, &early, occupied_len)),
                "occupied_len {occupied_len}"
            );
        }
    }

    /// Where a sweep of segment layouts starts: inside the range, at its bottom,
    /// and close enough to its top that the widening saturates.
    const SWEEP_BASES: [i64; 6] = [0, 500, 1_000, i64::MIN, i64::MIN + 40, i64::MAX - 300];

    #[test]
    fn every_day_level_pairing_in_the_sweep_agrees_with_the_pairwise_product() {
        for base in SWEEP_BASES {
            for count in 1..=3_usize {
                for length in [1_i64, 2, 5, 30] {
                    for gap in [0_i64, 1, 2, 7, 40] {
                        for shift in [-45_i64, -30, -5, 0, 15, 60] {
                            for occupied_len in [2_i64, 3, 7, 30, 90, 400] {
                                let segments_at = |offset: i64| -> Vec<(i64, i64)> {
                                    (0..count)
                                        .filter_map(|index| {
                                            let step = i64::try_from(index).ok()?;
                                            let start = base
                                                .saturating_add(offset)
                                                .saturating_add(step * (length + gap));
                                            let end = start.saturating_add(length);
                                            (end > start).then_some((start, end))
                                        })
                                        .collect()
                                };
                                let late = segments_at(0);
                                let early = segments_at(shift);
                                if late.is_empty() || early.is_empty() {
                                    continue;
                                }
                                let mut widened = Vec::new();
                                intersect_day_segments(&late, &early, occupied_len, &mut widened);
                                assert_eq!(
                                    covered(&widened),
                                    covered(&pairwise_reference(&late, &early, occupied_len)),
                                    "late {late:?}, early {early:?}, occupied_len {occupied_len}"
                                );
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn segments_that_only_overlap_once_widened_coalesce_before_the_intersection() {
        // Three ten-minute segments two minutes apart, and an early segment over
        // the middle of them. Widened by a long booking they touch and coalesce
        // into one range; the minutes covered are the same either way.
        let late = [(600, 610), (612, 622), (624, 634)];
        let early = [(605, 630)];
        assert_eq!(widened_segments(&late, 2).spans().len(), 3);
        assert_eq!(
            widened_segments(&late, 30).spans(),
            [MinuteSpan::new(571, 634).unwrap()],
            "a thirty-minute booking's widening joins all three segments"
        );
        for occupied_len in [2, 30] {
            let mut blocks = Vec::new();
            intersect_day_segments(&late, &early, occupied_len, &mut blocks);
            assert_eq!(
                covered(&blocks),
                covered(&pairwise_reference(&late, &early, occupied_len)),
                "occupied_len {occupied_len}"
            );
        }
    }

    #[test]
    fn a_booking_too_short_to_conflict_agrees_with_the_pairwise_product() {
        // `self_conflict_spans` covers no start for a booking of one minute or
        // less, so this is the arithmetic alone: the day-level form has to agree
        // with the product at, below and above the widening it applies.
        let late = [(600, 601), (610, 640), (700, 703)];
        let early = [(602, 611), (612, 700), (701, 702)];
        for occupied_len in [0, 1, -1, -30] {
            let mut widened = Vec::new();
            intersect_day_segments(&late, &early, occupied_len, &mut widened);
            assert_eq!(
                covered(&widened),
                covered(&pairwise_reference(&late, &early, occupied_len)),
                "occupied_len {occupied_len}"
            );
        }
    }

    #[test]
    fn widening_saturates_at_the_bottom_of_the_representable_range() {
        // `i64::MIN` itself cannot come out of schedule segmentation — a local
        // date's segment starts inside the date — so the boundary is exercised
        // through the helper the segmentation feeds.
        let late = [(i64::MIN, i64::MIN + 10)];
        let early = [(i64::MIN + 5, i64::MIN + 200)];
        let mut widened = Vec::new();
        intersect_day_segments(&late, &early, 30, &mut widened);
        // `(i64::MIN + 5) - 30 + 1` saturates to the minute after the floor.
        // `saturating_sub(occupied_len - 1)` would have kept `i64::MIN` itself,
        // which no booking can start at.
        assert_eq!(
            widened,
            [MinuteSpan::new(i64::MIN + 1, i64::MIN + 10).unwrap()]
        );
        assert_eq!(
            covered(&widened),
            covered(&pairwise_reference(&late, &early, 30))
        );
    }

    #[test]
    fn self_conflicts_are_found_across_local_dates_for_every_day_pair() {
        // Two evenings and two mornings, each evening crossing local midnight, so
        // `for_each_date_segment` cuts every evening into one segment per date and
        // both day pairs contribute. A thirty-minute booking from 23:01 reaches
        // the evening and the morning after it.
        let late = IntervalSet::from_spans(vec![
            MinuteSpan::new(minute(14, 22, 0), minute(15, 1, 0)).unwrap(),
            MinuteSpan::new(minute(15, 22, 0), minute(16, 1, 0)).unwrap(),
        ]);
        let early = IntervalSet::from_spans(vec![
            MinuteSpan::new(minute(15, 0, 30), minute(15, 2, 0)).unwrap(),
            MinuteSpan::new(minute(16, 0, 30), minute(16, 2, 0)).unwrap(),
        ]);
        let occupied_len = 90;
        let blocked = self_conflict_spans(
            &late,
            &early,
            &ZoneSchedule::fixed(Zone::utc()),
            &[],
            occupied_len,
        );

        // The segments each date carries, in the order the date walk appends
        // them: the evening of the date before a morning, cut at local midnight.
        let day_pairs: [DayPair<'_>; 2] = [
            (
                &[(minute(14, 22, 0), minute(15, 0, 0))],
                &[(minute(15, 0, 30), minute(15, 2, 0))],
            ),
            (
                &[
                    (minute(15, 0, 0), minute(15, 1, 0)),
                    (minute(15, 22, 0), minute(16, 0, 0)),
                ],
                &[(minute(16, 0, 30), minute(16, 2, 0))],
            ),
        ];
        let expected: Vec<MinuteSpan> = day_pairs
            .iter()
            .flat_map(|&(late, early)| pairwise_reference(late, early, occupied_len))
            .collect();
        assert_eq!(
            blocked,
            [
                MinuteSpan::new(minute(14, 23, 1), minute(15, 0, 0)).unwrap(),
                MinuteSpan::new(minute(15, 23, 1), minute(16, 0, 0)).unwrap(),
            ],
            "one span per day pair, appended in this fixture's date order"
        );
        assert_eq!(covered(&blocked), covered(&expected));

        // A booking that only reaches the morning of a date covers no start: the
        // morning alone is one half of the pair, never both.
        let morning_only = self_conflict_spans(
            &IntervalSet::new(),
            &early,
            &ZoneSchedule::fixed(Zone::utc()),
            &[],
            occupied_len,
        );
        assert!(morning_only.is_empty());
    }
}
