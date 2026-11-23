//! Allowed time and blackouts: the minutes a participant's windows are open for,
//! and the minutes their blackouts close.
//!
//! Both are built over the judged range, once per participant, with the
//! segment-start breakpoint builder in [`crate::time`], and the allowed set is
//! what `usable` starts from. A rule that reads a fixed zone covers the same
//! minutes for every participant, so those are built once for the request.

use std::collections::BTreeMap;
use std::sync::Arc;

use crate::error::Error;
use crate::intervals::{IntervalSet, Span as MinuteSpan};
use crate::rest::RestActivity;
use crate::rules::Applicable;
use crate::time::{Instant, RuleZone, TimeZoneDatabase, WindowSpec, Zone, ZoneSchedule};

use super::blocked::BlockedSpan;

/// The minutes one applicable rule covers over the judged range, with the rule's
/// ID so a reason can cite it.
pub(crate) type RuleMinutes<'a> = Vec<(&'a str, Arc<IntervalSet>)>;

/// Window and blackout minutes shared by participants in one request.
///
/// A rule read in a fixed zone — the event's, or one it names — covers the same
/// minutes for everyone: both the judged range and that zone belong to the
/// request, not to the participant. A rule read in [`RuleZone::Participant`]
/// covers the same minutes for every participant staying in the same home zone
/// with no zone periods. Entries are stored behind [`Arc`] so every participant
/// shares one allocation instead of cloning the full interval set.
///
/// A memo belongs to the one judged range it was built over: those minutes are a
/// property of that range, so it must not outlive the resolution that created it.
#[derive(Default)]
pub(crate) struct FixedMinutes<'a> {
    windows: BTreeMap<usize, Arc<IntervalSet>>,
    blackouts: BTreeMap<usize, Arc<IntervalSet>>,
    home_windows: BTreeMap<(usize, &'a str), Arc<IntervalSet>>,
    home_blackouts: BTreeMap<(usize, &'a str), Arc<IntervalSet>>,
}

/// A rule's resolution context for [`FixedMinutes::window`]: the database, the
/// participant's zone schedule and home zone name, the event zone, and the
/// judged range.
pub(crate) struct WindowContext<'a, 's> {
    pub(crate) db: &'s TimeZoneDatabase,
    pub(crate) schedule: &'s ZoneSchedule,
    pub(crate) home_zone_name: &'a str,
    pub(crate) event: &'s Zone,
    pub(crate) judged: MinuteSpan,
}

impl<'a> FixedMinutes<'a> {
    /// Builds or reuses the minutes of `spec` at `position` over `ctx.judged`.
    pub(crate) fn window(
        &mut self,
        position: usize,
        spec: &WindowSpec,
        ctx: &WindowContext<'a, '_>,
    ) -> Result<Arc<IntervalSet>, Error> {
        memoized_rule(
            &mut self.windows,
            &mut self.home_windows,
            position,
            spec.zone(),
            ctx.schedule.periods().is_empty(),
            ctx.home_zone_name,
            || {
                let zones = spec.zone().resolve(ctx.db, ctx.schedule, ctx.event)?;
                Ok(crate::time::build_minutes(spec, &zones, ctx.judged)?)
            },
        )
    }
}

/// Whether `zone` resolves the same for every participant, which is what makes
/// its minutes shareable across the request.
///
/// Matched variant by variant rather than by excluding the one that follows the
/// schedule: sharing one participant's minutes with another is a wrong answer,
/// not a slow one, so a zone this has never seen has to stop the build here
/// rather than fall through to shareable.
fn is_fixed(zone: &RuleZone) -> bool {
    match zone {
        RuleZone::Event | RuleZone::Named(_) => true,
        RuleZone::Participant => false,
    }
}

/// Returns a shared [`Arc<IntervalSet>`] for `position`, caching fixed-zone
/// rules by `position` and participant-zone rules on period-free schedules by
/// `(position, home_zone_name)`.
fn memoized_rule<'a>(
    fixed_cache: &mut BTreeMap<usize, Arc<IntervalSet>>,
    home_cache: &mut BTreeMap<(usize, &'a str), Arc<IntervalSet>>,
    position: usize,
    zone: &RuleZone,
    fixed_home: bool,
    home_zone_name: &'a str,
    build: impl FnOnce() -> Result<IntervalSet, Error>,
) -> Result<Arc<IntervalSet>, Error> {
    if is_fixed(zone) {
        if let Some(minutes) = fixed_cache.get(&position) {
            return Ok(Arc::clone(minutes));
        }
        let minutes = Arc::new(build()?);
        fixed_cache.insert(position, Arc::clone(&minutes));
        return Ok(minutes);
    }
    if fixed_home {
        let key = (position, home_zone_name);
        if let Some(minutes) = home_cache.get(&key) {
            return Ok(Arc::clone(minutes));
        }
        let minutes = Arc::new(build()?);
        home_cache.insert(key, Arc::clone(&minutes));
        return Ok(minutes);
    }
    Ok(Arc::new(build()?))
}

/// The minutes of the judged range inside every applicable window, together with
/// each window's own minutes when `keep_per_rule` is set (for explanations and
/// `check_slot`).
///
/// With no window applying the answer is `None`: a participant with no applicable
/// window has no window restriction, which is a different fact from an interval
/// set covering the judged range, because an occupied span can reach past it.
#[allow(clippy::too_many_arguments)]
pub(super) fn allowed_time<'a>(
    db: &TimeZoneDatabase,
    rules: &Applicable<'a>,
    schedule: &ZoneSchedule,
    home_zone_name: &'a str,
    event: &Zone,
    judged: MinuteSpan,
    keep_per_rule: bool,
    fixed: &mut FixedMinutes<'a>,
) -> Result<(Option<Arc<IntervalSet>>, RuleMinutes<'a>), Error> {
    let mut allowed: Option<Arc<IntervalSet>> = None;
    let mut windows = if keep_per_rule {
        Vec::with_capacity(rules.windows.len())
    } else {
        Vec::new()
    };
    let ctx = WindowContext {
        db,
        schedule,
        home_zone_name,
        event,
        judged,
    };
    for (id, position, spec) in &rules.windows {
        // When no per-rule citation is needed and the intersection is already
        // empty, no later window can re-open any minute.
        if !keep_per_rule && allowed.as_ref().is_some_and(|current| current.is_empty()) {
            break;
        }
        let set = fixed.window(*position, spec, &ctx)?;
        allowed = Some(match &allowed {
            Some(current) => Arc::new(current.intersection(&set)),
            None => Arc::clone(&set),
        });
        if keep_per_rule {
            windows.push((*id, set));
        }
    }
    Ok((allowed, windows))
}

/// The minutes of the judged range that each applicable blackout blacks out, in
/// request order (when `keep_per_rule` is set), together with all blackout spans
/// to subtract when building `usable`.
#[allow(clippy::too_many_arguments)]
pub(super) fn blackout_time<'a>(
    db: &TimeZoneDatabase,
    rules: &Applicable<'a>,
    schedule: &ZoneSchedule,
    home_zone_name: &'a str,
    event: &Zone,
    judged: MinuteSpan,
    keep_per_rule: bool,
    fixed: &mut FixedMinutes<'a>,
) -> Result<(RuleMinutes<'a>, Vec<MinuteSpan>), Error> {
    let mut blackouts = if keep_per_rule {
        Vec::with_capacity(rules.blackouts.len())
    } else {
        Vec::new()
    };
    let mut spans = Vec::new();
    let fixed_home = schedule.periods().is_empty();
    for (id, position, spec) in &rules.blackouts {
        let set = memoized_rule(
            &mut fixed.blackouts,
            &mut fixed.home_blackouts,
            *position,
            spec.zone(),
            fixed_home,
            home_zone_name,
            || {
                let zones = spec.zone().resolve(db, schedule, event)?;
                Ok(crate::time::build_minutes(*spec, &zones, judged)?)
            },
        )?;
        spans.extend_from_slice(set.spans());
        if keep_per_rule {
            blackouts.push((*id, set));
        }
    }
    Ok((blackouts, spans))
}

/// Precomputes the minutes a booking may occupy: `allowed` (or every representable
/// minute when no window applies) that survives every blackout,
/// clearance-widened blocking block, and existing-activity `rest` exclusion.
pub(super) fn build_usable(
    allowed: Option<&Arc<IntervalSet>>,
    mut forbidden: Vec<MinuteSpan>,
    blocked: &[BlockedSpan<'_>],
    rests: &[RestActivity<'_>],
    schedule: &ZoneSchedule,
    jumps: &[i64],
) -> Arc<IntervalSet> {
    for block in blocked {
        forbidden.push(block.widened);
    }
    for rest in rests {
        if !rest.late_dates.is_empty() {
            crate::rest::collect_matching_date_spans(
                &rest.early,
                schedule,
                jumps,
                |day| {
                    day.plus_days(-1)
                        .is_ok_and(|prev| rest.late_dates.contains(&prev))
                },
                &mut forbidden,
            );
        }
        if !rest.early_dates.is_empty() {
            crate::rest::collect_matching_date_spans(
                &rest.late,
                schedule,
                jumps,
                |day| {
                    day.plus_days(1)
                        .is_ok_and(|next| rest.early_dates.contains(&next))
                },
                &mut forbidden,
            );
        }
    }
    if forbidden.is_empty() {
        return match allowed {
            Some(allowed) => Arc::clone(allowed),
            None => Arc::new(representable_minutes()),
        };
    }
    let blocked_set = IntervalSet::from_spans(forbidden);
    match allowed {
        Some(allowed) => Arc::new(allowed.difference(&blocked_set)),
        None => Arc::new(representable_minutes().difference(&blocked_set)),
    }
}

/// Every minute the model can represent, which is the space a booking occupies
/// when no window restricts it.
fn representable_minutes() -> IntervalSet {
    IntervalSet::from_spans(vec![
        MinuteSpan::new(Instant::MIN.minutes(), Instant::MAX.minutes() + 1)
            .expect("the supported minute range is non-empty"),
    ])
}

/// `span` narrowed to the minutes the model can represent, or `None` when it holds
/// none of them.
///
/// A booking or a block at either edge of the supported range can need travel that
/// falls outside it. Those minutes exist in neither the interval arithmetic nor the
/// rules a caller wrote, so the representable part is what a rule can be judged
/// against: judging it neither invents a reason for a minute no rule can describe
/// nor drops a blackout that does apply.
pub(super) fn representable(span: MinuteSpan) -> Option<MinuteSpan> {
    MinuteSpan::new(
        span.start().max(Instant::MIN.minutes()),
        span.end().min(Instant::MAX.minutes() + 1),
    )
    .ok()
}
