//! Builders and presets over the domain types: sugar that constructs the general
//! shapes and adds no semantics of their own.
//!
//! Each preset generates its rule ID from the rule it builds, so a rule always has
//! an ID to cite — and the two presets that add an argument to a shape rather than
//! a shape of their own keep the shape's ID: `business_hours_ranges` writes
//! `business_hours`, `buffers_with_gap` writes `buffers`. Rule IDs are unique in a
//! request, so a caller with two presets that build the same kind renames one with
//! [`Rule::with_id`].
//!
//! The one-person, one-meeting case is about this much code:
//!
//! ```
//! use opportune::model::{Goal, Participant, Request, Requirement, Span, Term};
//! use opportune::presets;
//! use opportune::time::{Date, Duration, Instant, RuleZone, TimeOfDay, Weekday, Weekdays, ZoneName};
//!
//! # #[cfg(feature = "bundled-tzdb")] {
//! let zone = ZoneName::new("Europe/London").unwrap();
//! let now = Instant::from_utc(Date::new(2027, 6, 14).unwrap(), TimeOfDay::new(7, 0).unwrap()).unwrap();
//! let until = Instant::from_utc(Date::new(2027, 6, 19).unwrap(), TimeOfDay::new(0, 0).unwrap()).unwrap();
//! let mut request = Request::new(
//!     now,
//!     Goal::single(zone.clone(), Span::new(now, until).unwrap(), Duration::from_minutes(30), Requirement::new(vec![Term::person("rich")])),
//! );
//! request
//!     .participants
//!     .push(Participant::new("rich", zone).with_calendar(Vec::new()));
//! request.rules.push(presets::business_hours(
//!     RuleZone::Participant, Weekdays::range(Weekday::Monday, Weekday::Friday),
//!     TimeOfDay::new(9, 0).unwrap(), TimeOfDay::new(17, 0).unwrap(),
//! ).unwrap());
//! request.rules.push(presets::buffers(Duration::from_minutes(10), Duration::from_minutes(10)));
//! request.rules.push(presets::min_notice(Duration::from_hours_minutes(4, 0)));
//!
//! let response = opportune::Engine::builder().build().unwrap().find_availability(&request).unwrap();
//! assert!(!response.offers.is_empty());
//! # }
//! ```

use std::collections::BTreeMap;

use crate::intervals::{IntervalSet, Span as MinuteSpan};
use crate::model::{
    Cap, CapMeasure, CapPer, CapPeriod, Clearance, LeadTime, LeadTimeMax, Participant, Rest, Rule,
    RuleKind, Span, StatusPolicy, StatusPolicyEntry,
};
use crate::tags::Selector;
use crate::time::{
    BlackoutSpan, BlackoutSpec, Date, DateKey, DateRange, Duration, Instant, RuleZone, TimeOfDay,
    WallEnd, WallRange, Weekday, Weekdays, WeeklyKey, WindowError, WindowSpec, Zone, ZoneName,
};

/// How many minutes one civil day holds in UTC, which has no transitions.
const UTC_DAY_MINUTES: i64 = 24 * 60;

/// The most dated entries [`available_only`] accepts: ten years of UTC days.
///
/// A stated availability is a handful of hours an external attendee is reachable.
/// The ceiling is a guard, not a limit anyone should meet: the preset writes one
/// dated entry per UTC day of every stated span, so an availability without a
/// ceiling turns one bad argument into millions of entries.
pub const MAX_AVAILABLE_ONLY_DAYS: u32 = 3660;

/// A `window` rule open `from`–`until` on `days`, read in `zone`.
///
/// The rule's ID is `business_hours`.
///
/// ```
/// use opportune::presets;
/// use opportune::time::{RuleZone, TimeOfDay, WallEnd, Weekday, Weekdays};
///
/// let rule = presets::business_hours(
///     RuleZone::Participant,
///     Weekdays::range(Weekday::Monday, Weekday::Friday),
///     TimeOfDay::new(9, 0).unwrap(),
///     WallEnd::Time(TimeOfDay::new(17, 0).unwrap()),
/// )
/// .unwrap();
/// assert_eq!(rule.id, "business_hours");
/// assert!(rule.window().is_some());
/// ```
///
/// # Errors
/// Returns a [`WindowError`] when the range covers no minute, as `00:00-00:00`
/// does; write a full day as `00:00-24:00`.
pub fn business_hours(
    zone: RuleZone,
    days: Weekdays,
    from: TimeOfDay,
    until: impl Into<WallEnd>,
) -> Result<Rule, WindowError> {
    business_hours_ranges(zone, days, vec![WallRange::new(from, until)?])
}

/// A `window` rule open on each of `hours` every day in `days`, read in `zone`.
///
/// This is [`business_hours`] for a day split into several ranges, as a lunch
/// break or an event window with a morning and an afternoon session is. The
/// rule's ID is `business_hours`.
///
/// ```
/// use opportune::presets;
/// use opportune::time::{RuleZone, TimeOfDay, WallEnd, WallRange, Weekday, Weekdays};
///
/// let daytime = WallRange::new(
///     TimeOfDay::new(9, 0).unwrap(),
///     WallEnd::Time(TimeOfDay::new(17, 0).unwrap()),
/// )
/// .unwrap();
/// let rule = presets::business_hours_ranges(
///     RuleZone::Participant,
///     Weekdays::single(Weekday::Tuesday),
///     vec![daytime],
/// )
/// .unwrap();
/// let hours = &rule.window().unwrap().weekly()[1]; // Tuesday, Monday first
/// assert_eq!(hours.len(), 1);
/// ```
///
/// # Errors
/// Returns a [`WindowError`] when two ranges overlap or a range covers no
/// minute. `days` names the days the ranges are open on, so an empty set writes a
/// window closed on every day rather than failing.
pub fn business_hours_ranges(
    zone: RuleZone,
    days: Weekdays,
    hours: Vec<WallRange>,
) -> Result<Rule, WindowError> {
    window_rule(
        "business_hours",
        zone,
        vec![WeeklyKey::new(days, hours)?],
        Vec::new(),
    )
}

/// A `window` rule open on `dates` only, read in `zone`.
///
/// The `dates` keys are the whole window: a date no key covers is closed, so this
/// is the conference-booth shape — open on the days the event runs, and nowhere
/// else — rather than an override on top of weekly hours. The rule's ID is
/// `dated_hours`.
///
/// ```
/// use opportune::presets;
/// use opportune::time::{
///     Date, DateKey, DateRange, RuleZone, TimeOfDay, WallEnd, WallRange,
/// };
///
/// let booth = DateKey::new(
///     DateRange::single(Date::new(2027, 6, 14).unwrap()),
///     vec![WallRange::new(
///         TimeOfDay::new(10, 0).unwrap(),
///         WallEnd::Time(TimeOfDay::new(16, 0).unwrap()),
///     )
///     .unwrap()],
/// )
/// .unwrap();
/// let rule = presets::dated_hours(RuleZone::Event, vec![booth]).unwrap();
/// assert_eq!(rule.id, "dated_hours");
/// assert_eq!(rule.window().unwrap().dates().len(), 1);
/// assert!(rule.window().unwrap().weekly().iter().all(Vec::is_empty));
/// ```
///
/// # Errors
/// Returns a [`WindowError`] when two keys claim a date and when the ranges
/// within one key overlap or cover no minute.
pub fn dated_hours(zone: RuleZone, dates: Vec<DateKey>) -> Result<Rule, WindowError> {
    window_rule("dated_hours", zone, Vec::new(), dates)
}

/// A `window` rule open on each weekday's own hours, with optional `dates`
/// overrides, read in `zone`.
///
/// This is [`business_hours`] for a week whose days do not share one schedule: a
/// day split into a morning and an afternoon session, a late start on Thursday, a
/// different set of hours on Monday and Wednesday. Each [`WeeklyKey`] names the
/// days that share its hours; a day no key covers is closed. The `dates` keys
/// override the weekly hours for the dates they cover, and a key with no hours
/// closes them — a holiday, say. The rule's ID is `weekly_hours`.
///
/// ```
/// use opportune::presets;
/// use opportune::time::{
///     Date, DateKey, DateRange, RuleZone, TimeOfDay, WallEnd, WallRange, Weekday, Weekdays,
///     WeeklyKey,
/// };
///
/// let daytime = WallRange::new(
///     TimeOfDay::new(9, 0).unwrap(),
///     WallEnd::Time(TimeOfDay::new(17, 0).unwrap()),
/// )
/// .unwrap();
/// let with_lunch = [
///     WallRange::new(
///         TimeOfDay::new(9, 0).unwrap(),
///         WallEnd::Time(TimeOfDay::new(12, 0).unwrap()),
///     )
///     .unwrap(),
///     WallRange::new(
///         TimeOfDay::new(14, 0).unwrap(),
///         WallEnd::Time(TimeOfDay::new(18, 0).unwrap()),
///     )
///     .unwrap(),
/// ];
/// let holiday = DateKey::new(
///     DateRange::single(Date::new(2027, 12, 26).unwrap()),
///     Vec::new(),
/// )
/// .unwrap();
///
/// let rule = presets::weekly_hours(
///     RuleZone::Event,
///     vec![
///         WeeklyKey::new(
///             Weekdays::list([Weekday::Monday, Weekday::Wednesday]),
///             vec![daytime],
///         )
///         .unwrap(),
///         WeeklyKey::new(Weekdays::single(Weekday::Tuesday), with_lunch.to_vec()).unwrap(),
///     ],
///     vec![holiday],
/// )
/// .unwrap();
/// assert_eq!(rule.id, "weekly_hours");
/// let window = rule.window().unwrap();
/// assert_eq!(window.weekly()[0].len(), 1); // Monday shares Wednesday's hours.
/// assert_eq!(window.weekly()[1].len(), 2); // Tuesday is split around lunch.
/// assert!(window.dates()[0].hours().is_empty()); // The holiday is closed.
/// ```
///
/// # Errors
/// Returns a [`WindowError`] when two `weekly` keys claim the same weekday, when
/// two `dates` keys claim the same date, or when the hours within one key overlap
/// or cover no minute.
pub fn weekly_hours(
    zone: RuleZone,
    weekly: Vec<WeeklyKey>,
    dates: Vec<DateKey>,
) -> Result<Rule, WindowError> {
    window_rule("weekly_hours", zone, weekly, dates)
}

/// A `window` rule named `id` over the validated specification of `weekly` and
/// `dates` in `zone`.
///
/// The three window presets differ only in how they spell their keys, so the
/// specification and the rule are built here once.
fn window_rule(
    id: &'static str,
    zone: RuleZone,
    weekly: Vec<WeeklyKey>,
    dates: Vec<DateKey>,
) -> Result<Rule, WindowError> {
    let spec = WindowSpec::new(zone, weekly, dates)?;
    Ok(Rule::new(id, RuleKind::Window(spec)))
}

/// A `blackout` rule: no booking may happen inside any of `spans`, read in
/// `zone`.
///
/// A blackout is closed time — a holiday, a maintenance window, a company
/// shutdown — so its spans **union**: a booking that overlaps any of them is
/// excluded. The spans may arrive in any order and may overlap or touch; the
/// specification sorts and coalesces them, because the union is all that decides
/// membership. The rule's ID is `blackout`.
///
/// ```
/// use opportune::presets;
/// use opportune::time::{
///     BlackoutSpan, Date, LocalDateTime, RuleZone, TimeOfDay, ZoneName,
/// };
///
/// let shutdown = BlackoutSpan::new(
///     LocalDateTime::new(Date::new(2026, 12, 24).unwrap(), TimeOfDay::MIDNIGHT),
///     LocalDateTime::new(Date::new(2027, 1, 2).unwrap(), TimeOfDay::MIDNIGHT),
/// )
/// .unwrap();
/// let rule = presets::blackout(RuleZone::Named(ZoneName::utc()), vec![shutdown]);
/// assert_eq!(rule.id, "blackout");
/// let spans = rule.blackout().expect("a blackout rule").spans();
/// assert_eq!(spans.len(), 1);
/// assert_eq!(spans[0], shutdown);
/// ```
#[must_use]
pub fn blackout(zone: RuleZone, spans: Vec<BlackoutSpan>) -> Rule {
    Rule::new(
        "blackout",
        RuleKind::Blackout(BlackoutSpec::new(zone, spans)),
    )
}

/// A `clearance` rule: the gaps a booking needs before and after it, and the
/// least gap between any two events.
///
/// The rule's ID is `buffers`.
///
/// ```
/// use opportune::presets;
/// use opportune::time::Duration;
///
/// let rule = presets::buffers(Duration::from_minutes(10), Duration::from_minutes(15));
/// assert_eq!(rule.clearance().unwrap().after.minutes(), 15);
/// ```
#[must_use]
pub fn buffers(before: Duration, after: Duration) -> Rule {
    buffers_with_gap(before, after, Duration::ZERO)
}

/// A `clearance` rule with the gaps a booking needs, and the least gap between
/// the booking and any existing event.
///
/// `before` and `after` are the gaps the new booking needs before and after it;
/// `min_gap` is the least gap required between an existing event and the
/// booking on either side (the largest applicable gap wins, never a sum).
/// The rule's ID is `buffers`.
///
/// ```
/// use opportune::presets;
/// use opportune::time::Duration;
///
/// let rule = presets::buffers_with_gap(
///     Duration::from_minutes(10),
///     Duration::from_minutes(10),
///     Duration::from_minutes(15),
/// );
/// assert_eq!(rule.clearance().unwrap().min_gap.minutes(), 15);
/// ```
#[must_use]
pub fn buffers_with_gap(before: Duration, after: Duration, min_gap: Duration) -> Rule {
    Rule::new(
        "buffers",
        RuleKind::Clearance(Clearance {
            before,
            after,
            min_gap,
        }),
    )
}

/// A `lead_time` rule carrying only a minimum notice.
///
/// The rule's ID is `min_notice`.
///
/// ```
/// use opportune::presets;
/// use opportune::time::Duration;
///
/// let rule = presets::min_notice(Duration::from_hours_minutes(4, 0));
/// assert_eq!(rule.lead_time().unwrap().min.minutes(), 240);
/// assert!(rule.lead_time().unwrap().max.is_none());
/// ```
#[must_use]
pub fn min_notice(notice: Duration) -> Rule {
    Rule::new(
        "min_notice",
        RuleKind::LeadTime(LeadTime {
            min: notice,
            max: None,
        }),
    )
}

/// A `lead_time` rule carrying only a latest start.
///
/// The rule's ID is `horizon`.
///
/// ```
/// use opportune::model::LeadTimeMax;
/// use opportune::presets;
///
/// let rule = presets::horizon(LeadTimeMax::Days(3));
/// assert_eq!(rule.lead_time().unwrap().min.minutes(), 0);
/// assert!(rule.lead_time().unwrap().max.is_some());
/// ```
#[must_use]
pub fn horizon(max: LeadTimeMax) -> Rule {
    Rule::new(
        "horizon",
        RuleKind::LeadTime(LeadTime {
            min: Duration::ZERO,
            max: Some(max),
        }),
    )
}

/// A `rest` rule: no booking before `not_before` when anything runs at or after
/// `late_after` the day before.
///
/// The rule's ID is `rest`.
///
/// ```
/// use opportune::presets;
/// use opportune::time::TimeOfDay;
///
/// let rule = presets::rest(TimeOfDay::new(18, 0).unwrap(), TimeOfDay::new(10, 0).unwrap());
/// assert_eq!(rule.id, "rest");
/// assert_eq!(rule.rest().unwrap().not_before.minute_of_day(), 600);
/// ```
#[must_use]
pub fn rest(late_after: TimeOfDay, not_before: TimeOfDay) -> Rule {
    Rule::new("rest", RuleKind::Rest(Rest::new(late_after, not_before)))
}

/// A `cap` rule: at most `limit` events matching `count` per participant per
/// civil day.
///
/// The bucket is the participant's own zone, so a trip does not move a day
/// around under them. The rule's ID is `cap_per_day`.
///
/// ```
/// use opportune::presets;
/// use opportune::tags::{Selector, Tag};
///
/// let rule = presets::cap_per_day(Selector::Tag(Tag::new("interview").unwrap()), 2);
/// assert_eq!(rule.id, "cap_per_day");
/// let cap = rule.cap().expect("a cap rule");
/// assert_eq!(cap.limit, 2);
/// assert_eq!(cap.measure, opportune::model::CapMeasure::Count);
/// assert_eq!(cap.period, opportune::model::CapPeriod::Day);
/// assert_eq!(cap.per, opportune::model::CapPer::Each);
/// ```
#[must_use]
pub fn cap_per_day(count: Selector, limit: u32) -> Rule {
    cap(
        "cap_per_day",
        count,
        limit,
        CapMeasure::Count,
        CapPeriod::Day,
        CapPer::Each,
        RuleZone::Participant,
    )
}

/// A `cap` rule: at most `limit` events matching `count` per civil day, counted
/// in `zone`, with `per` deciding whether each participant in the rule's `who` is
/// limited on their own or the pool together.
///
/// This is [`cap_per_day`] with its two fixed arguments exposed: `cap_per_day`
/// always limits each participant in their own zone, so a pooled daily cap or one
/// bucketed in the event zone is written with this constructor rather than by
/// building a [`Cap`] from seven positional arguments. The
/// rule's ID is `cap_per_day`, as `cap_per_day`'s is.
///
/// ```
/// use opportune::model::{CapPer, CapPeriod};
/// use opportune::presets;
/// use opportune::tags::{Selector, Tag};
/// use opportune::time::RuleZone;
///
/// let rule = presets::cap_per_day_with(
///     Selector::Tag(Tag::new("interview").unwrap()),
///     4,
///     CapPer::Pooled,
///     RuleZone::Event,
/// );
/// let cap = rule.cap().unwrap();
/// assert_eq!((cap.limit, cap.period, cap.per), (4, CapPeriod::Day, CapPer::Pooled));
/// ```
#[must_use]
pub fn cap_per_day_with(count: Selector, limit: u32, per: CapPer, zone: RuleZone) -> Rule {
    cap(
        "cap_per_day",
        count,
        limit,
        CapMeasure::Count,
        CapPeriod::Day,
        per,
        zone,
    )
}

/// A `cap` rule: at most `limit` events matching `count` per participant per
/// civil week, counted in `zone`.
///
/// A week starts on Monday and runs seven civil days. `per` decides whether the
/// limit applies to each participant in the rule's `who` or to them together,
/// and a pooled cap therefore needs `zone` to be `event` or a named zone. The
/// rule's ID is `cap_per_week`.
///
/// ```
/// use opportune::model::{CapPer, CapPeriod};
/// use opportune::presets;
/// use opportune::tags::{Selector, Tag};
/// use opportune::time::RuleZone;
///
/// let rule = presets::cap_per_week(
///     Selector::Tag(Tag::new("interview").unwrap()),
///     8,
///     CapPer::Pooled,
///     RuleZone::Event,
/// );
/// let cap = rule.cap().unwrap();
/// assert_eq!((cap.limit, cap.period, cap.per), (8, CapPeriod::Week, CapPer::Pooled));
/// ```
#[must_use]
pub fn cap_per_week(count: Selector, limit: u32, per: CapPer, zone: RuleZone) -> Rule {
    cap(
        "cap_per_week",
        count,
        limit,
        CapMeasure::Count,
        CapPeriod::Week,
        per,
        zone,
    )
}

/// A `cap` rule: at most `limit` events matching `count` per participant per
/// civil month, counted in `zone`.
///
/// `per` and `zone` mean what they mean in [`cap_per_week`]. The rule's ID is
/// `cap_per_month`.
///
/// ```
/// use opportune::model::CapPer;
/// use opportune::presets;
/// use opportune::tags::{Selector, Tag};
/// use opportune::time::RuleZone;
///
/// let rule = presets::cap_per_month(
///     Selector::Tag(Tag::new("demo").unwrap()),
///     40,
///     CapPer::Each,
///     RuleZone::Participant,
/// );
/// assert_eq!(rule.cap().unwrap().period, opportune::model::CapPeriod::Month);
/// ```
#[must_use]
pub fn cap_per_month(count: Selector, limit: u32, per: CapPer, zone: RuleZone) -> Rule {
    cap(
        "cap_per_month",
        count,
        limit,
        CapMeasure::Count,
        CapPeriod::Month,
        per,
        zone,
    )
}

/// A `cap` rule: at most `limit_minutes` of events matching `count` per
/// participant per civil day, counted in `zone`.
///
/// `per` and `zone` mean what they mean in [`cap_per_week`]. The rule's ID is
/// `cap_minutes_per_day`.
///
/// This is the "no more than three hours of demos a day" shape, and it is
/// [`cap_minutes_per_week`] for a day: each event contributes its own duration
/// without travel.
///
/// ```
/// use opportune::model::{CapMeasure, CapPer, CapPeriod};
/// use opportune::presets;
/// use opportune::tags::{Selector, Tag};
/// use opportune::time::RuleZone;
///
/// let rule = presets::cap_minutes_per_day(
///     Selector::Tag(Tag::new("sales/demo").unwrap()),
///     3 * 60,
///     CapPer::Each,
///     RuleZone::Participant,
/// );
/// let cap = rule.cap().unwrap();
/// assert_eq!((cap.measure, cap.period, cap.limit), (CapMeasure::Minutes, CapPeriod::Day, 180));
/// ```
#[must_use]
pub fn cap_minutes_per_day(
    count: Selector,
    limit_minutes: u32,
    per: CapPer,
    zone: RuleZone,
) -> Rule {
    cap(
        "cap_minutes_per_day",
        count,
        limit_minutes,
        CapMeasure::Minutes,
        CapPeriod::Day,
        per,
        zone,
    )
}

/// A `cap` rule: at most `limit_minutes` of events matching `count` per
/// participant per civil week, counted in `zone`.
///
/// Each event contributes its own duration without travel, so this is the
/// "no more than six hours of demos a week" shape. A week starts on Monday and
/// runs seven civil days; `per` and `zone` mean what they mean in
/// [`cap_per_week`]. The rule's ID is `cap_minutes_per_week`.
///
/// ```
/// use opportune::model::{CapMeasure, CapPer};
/// use opportune::presets;
/// use opportune::tags::{Selector, Tag};
/// use opportune::time::RuleZone;
///
/// // Six hours of demo time a week.
/// let rule = presets::cap_minutes_per_week(
///     Selector::Tag(Tag::new("sales/demo").unwrap()),
///     6 * 60,
///     CapPer::Each,
///     RuleZone::Participant,
/// );
/// let cap = rule.cap().unwrap();
/// assert_eq!((cap.measure, cap.limit), (CapMeasure::Minutes, 360));
/// ```
#[must_use]
pub fn cap_minutes_per_week(
    count: Selector,
    limit_minutes: u32,
    per: CapPer,
    zone: RuleZone,
) -> Rule {
    cap(
        "cap_minutes_per_week",
        count,
        limit_minutes,
        CapMeasure::Minutes,
        CapPeriod::Week,
        per,
        zone,
    )
}

/// A `cap` rule: at most `limit_minutes` of events matching `count` per
/// participant per civil month, counted in `zone`.
///
/// `per` and `zone` mean what they mean in [`cap_per_week`]. The rule's ID is
/// `cap_minutes_per_month`.
///
/// ```
/// use opportune::model::{CapMeasure, CapPer, CapPeriod};
/// use opportune::presets;
/// use opportune::tags::{Selector, Tag};
/// use opportune::time::RuleZone;
///
/// let rule = presets::cap_minutes_per_month(
///     Selector::Tag(Tag::new("sales/demo").unwrap()),
///     40 * 60,
///     CapPer::Each,
///     RuleZone::Participant,
/// );
/// let cap = rule.cap().unwrap();
/// assert_eq!((cap.measure, cap.period), (CapMeasure::Minutes, CapPeriod::Month));
/// ```
#[must_use]
pub fn cap_minutes_per_month(
    count: Selector,
    limit_minutes: u32,
    per: CapPer,
    zone: RuleZone,
) -> Rule {
    cap(
        "cap_minutes_per_month",
        count,
        limit_minutes,
        CapMeasure::Minutes,
        CapPeriod::Month,
        per,
        zone,
    )
}

/// A `status_policy` rule: the entries, in the order the caller wrote them, that
/// decide which blocks block.
///
/// The rule's ID is `status_policy`. Entries are tried in that order across every
/// applicable policy, so a narrower policy belongs before a broader one. Rename
/// it with [`Rule::with_id`] when a reason or a finding should cite another ID,
/// and scope it with [`Rule::with_who`]/[`Rule::unless`] as any other rule is.
///
/// ```
/// use opportune::model::{BlockStatus, StatusEffect, StatusPolicyEntry};
/// use opportune::presets;
///
/// // A tentative hold does not block; everything else keeps its own reading.
/// let rule = presets::status_policy(vec![StatusPolicyEntry::status(
///     BlockStatus::Tentative,
///     StatusEffect::Ignore,
/// )]);
/// assert_eq!(rule.id, "status_policy");
/// assert_eq!(rule.status_policy().unwrap().entries.len(), 1);
/// ```
#[must_use]
pub fn status_policy(entries: Vec<StatusPolicyEntry>) -> Rule {
    Rule::new(
        "status_policy",
        RuleKind::StatusPolicy(StatusPolicy { entries }),
    )
}

/// A `cap` rule over `measure`, `period`, `per` and `zone`, with the rule named
/// `id`.
fn cap(
    id: &'static str,
    count: Selector,
    limit: u32,
    measure: CapMeasure,
    period: CapPeriod,
    per: CapPer,
    zone: RuleZone,
) -> Rule {
    Rule::new(
        id,
        RuleKind::Cap(Cap::new(
            count,
            measure,
            limit,
            period,
            per,
            zone,
            Weekday::Monday,
        )),
    )
}

/// Stated availability for an external attendee: the attendee, and the `window`
/// rule that makes exactly those instants available to them.
///
/// The spans are UTC instants, coalesced first and then written as dated ranges
/// in the UTC zone, split at UTC midnight with a span ending at midnight written
/// as ending `24:00`. A window rather than blocks is deliberate: windows get no
/// clearance and never trigger rest, so stated availability cannot be mistaken
/// for a meeting, and an excluded start explains itself as `outside_window`
/// citing this rule's ID.
///
/// The rule's ID is `available_only:{id}`, derived from the attendee so that two
/// attendees' stated availability compose in one request without colliding, and
/// its `who` selector names only that attendee, so it constrains nobody else.
/// Rename it with [`Rule::with_id`] when a reason should cite another ID.
///
/// ```
/// use opportune::model::Span;
/// use opportune::presets;
/// use opportune::tags::Role;
/// use opportune::time::Instant;
///
/// let spans = [
///     Span::new(Instant::MIN, Instant::from_minutes(Instant::MIN.minutes() + 60).unwrap()).unwrap(),
///     // Adjacent to the first, so the two coalesce into one hour.
///     Span::new(Instant::from_minutes(Instant::MIN.minutes() + 60).unwrap(),
///               Instant::from_minutes(Instant::MIN.minutes() + 90).unwrap()).unwrap(),
/// ];
/// let (attendee, rule) = presets::available_only("candidate", &spans).unwrap();
/// assert_eq!(attendee.id, "candidate");
/// assert_eq!(attendee.role, Role::Attendee);
/// assert_eq!(attendee.zone.as_str(), "UTC");
/// assert_eq!(rule.id, "available_only:candidate");
/// let window = rule.window().expect("the preset writes a window");
/// assert_eq!(window.dates().len(), 1);
/// assert_eq!(window.dates()[0].hours()[0].end().to_string(), "01:30");
/// ```
///
/// # Panics
/// Never for the [`Span`]s a request can hold: every guard here states an
/// invariant coalescing establishes — a span covering at least one minute
/// inside the supported instants, and one disjoint range list per UTC date. The
/// constructors are fallible by signature, not by input.
///
/// # Errors
/// Returns a [`WindowError::AvailabilityTooLong`] when the stated spans need
/// more than [`MAX_AVAILABLE_ONLY_DAYS`] dated entries. The window writes one
/// entry per UTC day of every stated span, so an availability without a ceiling
/// turns one wrong argument into millions of entries written before the engine is
/// called.
pub fn available_only(
    id: impl Into<String>,
    spans: &[Span],
) -> Result<(Participant, Rule), WindowError> {
    let id = id.into();
    let attendee = Participant::attendee(id.clone(), ZoneName::utc());

    // Coalescing first means two adjacent spans become one dated range rather
    // than two ranges that touch, which a window key would reject as ambiguous.
    let union = IntervalSet::from_spans(spans.iter().map(|span| span.to_minute_span()).collect());
    // The ceiling bounds what the preset writes, so it counts the whole stated
    // availability: spans that each fit the ceiling can still add up to millions
    // of entries.
    let entries = dated_entries(union.spans());
    if entries > u64::from(MAX_AVAILABLE_ONLY_DAYS) {
        return Err(WindowError::AvailabilityTooLong {
            entries,
            max: MAX_AVAILABLE_ONLY_DAYS,
        });
    }
    let mut by_date: BTreeMap<Date, Vec<WallRange>> = BTreeMap::new();
    let utc = Zone::utc();
    for span in union.spans() {
        let mut from = span.start();
        while from < span.end() {
            // UTC days are exactly 1440 minutes, so a day's bounds are minute
            // arithmetic rather than a zone lookup that cannot fail here.
            let day_start = from.div_euclid(UTC_DAY_MINUTES) * UTC_DAY_MINUTES;
            let day_end = day_start + UTC_DAY_MINUTES;
            let to = span.end().min(day_end);
            let date = Instant::from_minutes(from)
                .expect("a coalesced span lies inside the supported instants")
                .zoned(&utc)
                .date();
            let start_time = TimeOfDay::from_minute_of_day(
                u16::try_from(from - day_start).expect("a time within one UTC day"),
            )
            .expect("a minute below 1440 is a time of day");
            let end = if to == day_end {
                WallEnd::Midnight
            } else {
                WallEnd::Time(
                    TimeOfDay::from_minute_of_day(
                        u16::try_from(to - day_start).expect("a time within one UTC day"),
                    )
                    .expect("a minute below 1440 is a time of day"),
                )
            };
            by_date.entry(date).or_default().push(
                WallRange::new(start_time, end)
                    .expect("a coalesced span covers at least one minute of the day"),
            );
            from = day_end;
        }
    }

    let dates = by_date
        .into_iter()
        .map(|(date, hours)| {
            DateKey::new(DateRange::single(date), hours)
                .expect("coalescing leaves one disjoint range list per date")
        })
        .collect();
    let spec = WindowSpec::new(RuleZone::Named(ZoneName::utc()), Vec::new(), dates)
        .expect("each date appears once and its ranges are disjoint");
    let rule = Rule::new(format!("available_only:{id}"), RuleKind::Window(spec))
        .with_who(Selector::ids([id]));
    Ok((attendee, rule))
}

/// The dated entries `spans` need: one per UTC day each of them covers.
///
/// UTC days are exactly `UTC_DAY_MINUTES` long, so a span's first and last date
/// are minute arithmetic rather than a zone lookup that cannot fail here. Two
/// disjoint spans on one date write two entries under one key, so this is the
/// number of entries written, not the number of keys.
fn dated_entries(spans: &[MinuteSpan]) -> u64 {
    spans
        .iter()
        .map(|span| {
            let first = span.start().div_euclid(UTC_DAY_MINUTES);
            let last = (span.end() - 1).div_euclid(UTC_DAY_MINUTES);
            u64::try_from(last - first + 1).expect("a span covers at least one day")
        })
        .sum()
}

#[cfg(test)]
mod tests {
    use crate::presets;
    use crate::time::{
        Date, DateKey, DateRange, RuleZone, TimeOfDay, WallEnd, WallRange, Weekday, Weekdays,
        WeeklyKey, WindowError, Zone, ZoneName, ZoneSchedule,
    };

    fn range(from: (u8, u8), until: (u8, u8)) -> WallRange {
        WallRange::new(
            TimeOfDay::new(from.0, from.1).unwrap(),
            WallEnd::Time(TimeOfDay::new(until.0, until.1).unwrap()),
        )
        .unwrap()
    }

    /// A per-weekday schedule is one rule, not one rule per day: Monday and
    /// Wednesday share their hours, Tuesday is split around lunch, a date override
    /// replaces the weekday's hours for the date it names, and the holiday key
    /// closes its date.
    #[test]
    fn weekly_hours_reads_each_weekday_and_overrides_the_dates_it_names() {
        let holiday = Date::new(2027, 6, 14).unwrap(); // Monday
        let short_wednesday = Date::new(2027, 6, 16).unwrap();
        let rule = presets::weekly_hours(
            RuleZone::Named(ZoneName::new("UTC").unwrap()),
            vec![
                WeeklyKey::new(
                    Weekdays::list([Weekday::Monday, Weekday::Wednesday]),
                    vec![range((9, 0), (17, 0))],
                )
                .unwrap(),
                WeeklyKey::new(
                    Weekdays::single(Weekday::Tuesday),
                    vec![range((9, 0), (12, 0)), range((14, 0), (18, 0))],
                )
                .unwrap(),
            ],
            vec![
                DateKey::new(DateRange::single(holiday), Vec::new()).unwrap(),
                DateKey::new(
                    DateRange::single(short_wednesday),
                    vec![range((13, 0), (15, 0))],
                )
                .unwrap(),
            ],
        )
        .unwrap();

        assert_eq!(rule.id, "weekly_hours");
        let spec = rule.window().expect("the preset writes a window");
        assert_eq!(
            spec.weekly()[0].len(),
            1,
            "the shared key's hours are written for each day it names"
        );
        assert_eq!(spec.weekly()[2].len(), 1, "Wednesday shares the key");
        assert_eq!(spec.weekly()[1].len(), 2, "Tuesday is split around lunch");
        assert!(spec.weekly()[3].is_empty(), "Thursday has no hours");

        let zones = ZoneSchedule::fixed(Zone::utc());
        let at = |day: i8, (hour, minute): (u8, u8)| {
            let local = Zone::utc()
                .resolve_local(
                    Date::new(2027, 6, day).unwrap(),
                    TimeOfDay::new(hour, minute).unwrap(),
                )
                .unwrap();
            let crate::time::LocalResolution::Unique(instant) = local else {
                panic!("UTC has no daylight saving transitions");
            };
            instant
        };
        assert!(
            !spec.contains(at(14, (9, 30)), &zones),
            "the holiday is closed"
        );
        assert!(
            !spec.contains(at(15, (13, 0)), &zones),
            "Tuesday's lunch is closed"
        );
        assert!(
            spec.contains(at(15, (14, 30)), &zones),
            "Tuesday's afternoon session"
        );
        assert!(
            spec.contains(at(15, (10, 30)), &zones),
            "Tuesday's morning session"
        );
        assert!(
            !spec.contains(at(16, (9, 0)), &zones),
            "the override replaces Wednesday's shared hours"
        );
        assert!(
            spec.contains(at(16, (13, 30)), &zones),
            "the override's own hours"
        );
        assert!(
            !spec.contains(at(17, (10, 0)), &zones),
            "Thursday has no hours of its own"
        );
    }

    /// Two weekly keys that claim the same day are rejected rather than silently
    /// intersected, which is the mistake a caller makes by stacking two presets.
    #[test]
    fn weekly_hours_rejects_two_keys_that_claim_one_day() {
        let monday = || {
            WeeklyKey::new(
                Weekdays::single(Weekday::Monday),
                vec![range((9, 0), (17, 0))],
            )
            .unwrap()
        };
        assert_eq!(
            presets::weekly_hours(RuleZone::Event, vec![monday(), monday()], Vec::new()),
            Err(WindowError::WeekdayRepeated {
                day: Weekday::Monday
            })
        );
    }
}
