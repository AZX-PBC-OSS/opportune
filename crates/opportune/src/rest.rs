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
    for (day, late_spans) in &late_by_date {
        let Ok(next) = day.plus_days(1) else {
            continue;
        };
        let Some(early_spans) = early_by_date.get(&next) else {
            continue;
        };
        for &(l0, l1) in late_spans {
            for &(e0, e1) in early_spans {
                // `[s, s + occupied_len)` overlaps `[l0, l1)` iff
                // `l0 - occupied_len + 1 <= s < l1`, and `[e0, e1)` iff
                // `e0 - occupied_len + 1 <= s < e1`.
                let from = l0.max(e0).saturating_sub(occupied_len).saturating_add(1);
                let until = l1.min(e1);
                if let Ok(span) = MinuteSpan::new(from, until) {
                    blocked.push(span);
                }
            }
        }
    }
    blocked
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
