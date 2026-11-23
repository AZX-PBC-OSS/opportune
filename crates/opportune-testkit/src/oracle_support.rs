//! The reference models' shared pieces: minute-by-minute membership, the
//! lead-time bounds, and the block and status-policy decisions the A1 and A2
//! oracles both read.
//!
//! Everything here works in whole minutes and booleans, straight from the
//! specification text, and shares nothing with the engine's interval sets,
//! breakpoints or matching.

use opportune::model::{BlockStatus, CapMeasure, CapPeriod, StatusEffect};
use opportune::time::{Date, Instant, TimeOfDay, Zone};

use crate::a1::{
    BlockPlan, CapPlan, LeadMaxPlan, OccurrencePlan, RestPlan, StatusEntryPlan, StatusPolicyPlan,
};
use crate::specs::ResolvedZones;

/// Minute-by-minute membership over a judged range.
pub(crate) struct MinuteFlags {
    base: i64,
    flags: Vec<bool>,
}

impl MinuteFlags {
    /// Builds the flags for every minute of `from..until`.
    pub(crate) fn over(from: i64, until: i64, mut member: impl FnMut(i64) -> bool) -> Self {
        let mut flags = Vec::new();
        let mut minute = from;
        while minute < until {
            flags.push(member(minute));
            minute += 1;
        }
        Self { base: from, flags }
    }

    /// Whether `minute` is a member. Minutes outside the judged range are not.
    pub(crate) fn contains(&self, minute: i64) -> bool {
        let index = minute - self.base;
        usize::try_from(index)
            .ok()
            .and_then(|index| self.flags.get(index))
            .copied()
            .unwrap_or(false)
    }

    /// Whether every minute of the half-open span is a member.
    pub(crate) fn contains_span(&self, span: (i64, i64)) -> bool {
        (span.0..span.1).all(|minute| self.contains(minute))
    }

    /// Whether any minute of the half-open span is a member.
    pub(crate) fn overlaps(&self, span: (i64, i64)) -> bool {
        (span.0..span.1).any(|minute| self.contains(minute))
    }
}

/// A `lead_time` maximum, resolved into the answer it gives about a start.
#[derive(Clone, Copy, Debug)]
pub(crate) enum LeadBound {
    /// The latest start instant; `None` is unbounded.
    Instant(Option<i64>),
    /// The latest local date in the event zone; `None` is unbounded.
    Date(Option<Date>),
}

impl LeadBound {
    /// Whether the bound allows `start`.
    pub(crate) fn allows(self, start: Instant, event: &Zone) -> bool {
        match self {
            Self::Instant(Some(latest)) => start.minutes() <= latest,
            Self::Instant(None) | Self::Date(None) => true,
            Self::Date(Some(latest)) => start.zoned(event).date() <= latest,
        }
    }
}

/// Resolves one `lead_time` maximum into the bound it places on starts.
pub(crate) fn resolve_bound(max: LeadMaxPlan, now: Instant, event: &Zone) -> LeadBound {
    match max {
        LeadMaxPlan::Duration(minutes) => {
            LeadBound::Instant(Some(now.minutes() + i64::from(minutes)))
        }
        LeadMaxPlan::Days(days) => {
            LeadBound::Date(event.zoned(now).date().plus_days(i64::from(days)).ok())
        }
        LeadMaxPlan::Date(date) => LeadBound::Date(Some(date)),
    }
}

/// Whether a block is in the blocking set: the first matching status entry across
/// the applicable policies decides, and with none a `free` block is ignored while
/// every other status blocks.
pub(crate) fn blocks(block: &BlockPlan, policies: &[&StatusPolicyPlan]) -> bool {
    for policy in policies {
        for entry in &policy.entries {
            if entry_matches(entry, block) {
                return entry.effect == StatusEffect::Block;
            }
        }
    }
    block.status != BlockStatus::Free
}

/// Whether a status-policy entry matches a block.
fn entry_matches(entry: &StatusEntryPlan, block: &BlockPlan) -> bool {
    if entry.status.is_some_and(|status| status != block.status) {
        return false;
    }
    match &entry.tag {
        Some(path) => {
            let tag = opportune::tags::Tag::new(path).expect("a generated tag is valid");
            block.tags.iter().any(|block_tag| {
                tag.is_ancestor_or_self_of(
                    &opportune::tags::Tag::new(block_tag).expect("a generated tag is valid"),
                )
            })
        }
        None => true,
    }
}

/// Whether two half-open minute spans share a minute.
pub(crate) fn overlaps(left: (i64, i64), right: (i64, i64)) -> bool {
    left.0 < right.1 && right.0 < left.1
}

/// The time of day `minute` minutes after midnight.
pub(crate) fn minute_of_day(minute: u16) -> TimeOfDay {
    TimeOfDay::from_minute_of_day(minute).expect("a minute below 1440 is a time of day")
}

/// The local dates a participant's existing activity reaches the late and early
/// parts of, computed once per rule rather than once per candidate start.
pub(crate) struct RestDays {
    /// Dates on which a blocking, non-all-day block has a minute at or after
    /// `late_after`.
    late: Vec<Date>,
    /// Dates on which it has a minute before `not_before`.
    early: Vec<Date>,
}

/// Classifies the participant's existing activity into the dates it reaches
/// late and early.
pub(crate) fn rest_activity_days(
    rest: &RestPlan,
    zones: &ResolvedZones,
    activity: &[(i64, i64)],
) -> RestDays {
    let mut days = RestDays {
        late: Vec::new(),
        early: Vec::new(),
    };
    for &(start, end) in activity {
        for minute in start..end {
            let Some(local) = local_at(minute, zones) else {
                continue;
            };
            let date = local.date();
            let time = local.time().minute_of_day();
            if time >= rest.late_after && !days.late.contains(&date) {
                days.late.push(date);
            }
            if time < rest.not_before && !days.early.contains(&date) {
                days.early.push(date);
            }
        }
    }
    days
}

/// Whether a `rest` rule excludes the booking at `booking`, judged from
/// instant-to-local membership alone.
///
/// The model reads the specification text directly. For every local date `D` the
/// booking can be part of — its own, and the one before — it asks whether the
/// participant's other activity reaches `late(D)` and `early(D + 1)`, and whether
/// the booking reaches one of those two. The booking itself is classified here,
/// because activity includes it.
pub(crate) fn rest_excludes(
    rest: &RestPlan,
    zones: &ResolvedZones,
    activity: &RestDays,
    booking: (i64, i64),
) -> bool {
    let mut booking_late: Vec<Date> = Vec::new();
    let mut booking_early: Vec<Date> = Vec::new();
    for minute in booking.0..booking.1 {
        let Some(local) = local_at(minute, zones) else {
            continue;
        };
        let date = local.date();
        let time = local.time().minute_of_day();
        if time >= rest.late_after && !booking_late.contains(&date) {
            booking_late.push(date);
        }
        if time < rest.not_before && !booking_early.contains(&date) {
            booking_early.push(date);
        }
    }
    // `D` must have activity in `late(D)`, so only the activity's late dates and
    // the booking's own can be the day the rule pairs `early(D + 1)` against.
    activity.late.iter().chain(booking_late.iter()).any(|day| {
        let Some(next) = day.plus_days(1).ok() else {
            return false;
        };
        let activity_early = activity.early.contains(&next) || booking_early.contains(&next);
        let booking_involved = booking_late.contains(day) || booking_early.contains(&next);
        activity_early && booking_involved
    })
}

/// The local reading of `minute`, or `None` outside the supported instants.
fn local_at(minute: i64, zones: &ResolvedZones) -> Option<opportune::time::ZonedInstant> {
    let instant = Instant::from_minutes(minute).ok()?;
    Some(instant.zoned(zones.zone_at(instant)))
}

/// One event the caps count, after event-key merging: the oracle's own reading
/// of the specification's deduplication rules.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CountedPlanEvent {
    /// The event's own span, without travel, in Unix minutes.
    pub(crate) span: (i64, i64),
    /// Every tag the event is known by.
    pub(crate) tags: Vec<String>,
    /// The participants the event involves, sorted and without repeats. Names
    /// the request does not carry are dropped.
    pub(crate) participants: Vec<String>,
}

/// Every event a request can identify, merged by event key.
///
/// The rules are read straight from the specification: an occurrence is
/// authoritative for its span and participants and its tags union with any block
/// sharing the key; blocks sharing a key without an occurrence become one event
/// from the earliest start to the latest end, with their participants and tags
/// unioned; a block with no key stands alone. An all-day block contributes no
/// count and no minutes, and takes no part in forming a block-only event, but its
/// tags still join the event an occurrence identifies.
///
/// Any event whose key appears in `exclude_event_keys` — whether an occurrence
/// or a block (including an all-day block sharing that key) — is dropped before
/// any merging takes place, so the excluded booking contributes neither count,
/// minutes, nor tags.
pub(crate) fn counted_events(
    occurrences: &[OccurrencePlan],
    calendars: &[(&str, &[BlockPlan])],
    exclude_event_keys: &[String],
) -> Vec<CountedPlanEvent> {
    let excluded: std::collections::BTreeSet<&str> =
        exclude_event_keys.iter().map(String::as_str).collect();
    let known: Vec<&str> = calendars.iter().map(|(id, _)| *id).collect();
    let mut events: Vec<CountedPlanEvent> = Vec::new();
    let mut keyed: std::collections::BTreeMap<String, (usize, bool)> =
        std::collections::BTreeMap::new();

    for occurrence in occurrences {
        if excluded.contains(occurrence.event_key.as_str())
            || keyed.contains_key(&occurrence.event_key)
        {
            continue;
        }
        let mut participants: Vec<String> = occurrence
            .participants
            .iter()
            .filter(|id| known.contains(&id.as_str()))
            .cloned()
            .collect();
        participants.sort();
        participants.dedup();
        keyed.insert(occurrence.event_key.clone(), (events.len(), true));
        events.push(CountedPlanEvent {
            span: (occurrence.span.0.minutes(), occurrence.span.1.minutes()),
            tags: occurrence.tags.clone(),
            participants,
        });
    }

    for (id, blocks) in calendars {
        for block in *blocks {
            // Drop excluded blocks before any merging — including before the
            // all-day tag-union branch — so an excluded booking leaves no trace
            // in cap workload or tag unions.
            if block
                .event_key
                .as_deref()
                .is_some_and(|key| excluded.contains(key))
            {
                continue;
            }
            // An all-day block is time away from work, not meeting workload: it
            // never counts toward a cap, however many minutes it spans. It is
            // still part of the event an occurrence identifies, so its tags join
            // that event's — the occurrence keeps its own span and participants,
            // so the block cannot contribute minutes that way either.
            if block.all_day {
                if let Some(key) = &block.event_key
                    && let Some((index, true)) = keyed.get(key.as_str()).copied()
                {
                    union_tags(&mut events[index].tags, &block.tags);
                }
                continue;
            }
            let span = (block.span.0.minutes(), block.span.1.minutes());
            let Some(key) = &block.event_key else {
                events.push(CountedPlanEvent {
                    span,
                    tags: block.tags.clone(),
                    participants: vec![(*id).to_owned()],
                });
                continue;
            };
            match keyed.get(key.as_str()).copied() {
                Some((index, true)) => union_tags(&mut events[index].tags, &block.tags),
                Some((index, false)) => {
                    let event = &mut events[index];
                    event.span = (event.span.0.min(span.0), event.span.1.max(span.1));
                    if !event.participants.iter().any(|named| named == id) {
                        event.participants.push((*id).to_owned());
                    }
                    union_tags(&mut event.tags, &block.tags);
                }
                None => {
                    keyed.insert(key.clone(), (events.len(), false));
                    events.push(CountedPlanEvent {
                        span,
                        tags: block.tags.clone(),
                        participants: vec![(*id).to_owned()],
                    });
                }
            }
        }
    }

    for event in &mut events {
        event.participants.sort();
        event.participants.dedup();
    }
    events
}

fn union_tags(target: &mut Vec<String>, extra: &[String]) {
    for tag in extra {
        if !target.contains(tag) {
            target.push(tag.clone());
        }
    }
}

/// One cap resolved for the oracle: its plan, the zone its buckets are read in
/// (`None` for the participant's own zone), and the usage it already holds.
#[derive(Clone, Debug)]
pub(crate) struct CapModel {
    pub(crate) plan: CapPlan,
    zone: Option<Zone>,
    usage: Vec<(Date, u64)>,
}

impl CapModel {
    pub(crate) fn new(plan: CapPlan, zone: Option<Zone>, usage: Vec<(Date, u64)>) -> Self {
        Self { plan, zone, usage }
    }

    /// What the booking adds to a bucket.
    pub(crate) fn contribution(&self, duration: u32) -> u64 {
        match self.plan.measure {
            CapMeasure::Count => 1,
            CapMeasure::Minutes => u64::from(duration),
        }
    }

    /// The `cap_reached` parameters when the booking's start would take its
    /// bucket past the limit.
    pub(crate) fn excluded(
        &self,
        start: Instant,
        zones: Option<&ResolvedZones>,
        event: &Zone,
        duration: u32,
    ) -> Option<(u64, u32, String)> {
        let zone = match &self.zone {
            Some(zone) => zone,
            None => zones.map_or(event, |zones| zones.zone_at(start)),
        };
        let label = cap_label(start.zoned(zone).date(), &self.plan);
        let used = self
            .usage
            .iter()
            .find(|(date, _)| *date == label)
            .map_or(0, |(_, used)| *used);
        (used + self.contribution(duration) > u64::from(self.plan.limit))
            .then(|| (used, self.plan.limit, label.to_string()))
    }
}

/// The usage a cap already holds for one participant, or for a pool.
///
/// `subject` names the participant an `each` cap is measured for; `None` measures
/// a pooled cap, which counts an event once when it names any covered
/// participant. `zone` is the cap's fixed bucket zone, or `None` for the
/// participant's own piecewise zone.
pub(crate) fn cap_usage(
    plan: &CapPlan,
    events: &[CountedPlanEvent],
    subject: Option<&str>,
    covered: &[&str],
    zone: Option<&Zone>,
    zones: Option<&ResolvedZones>,
    event: &Zone,
) -> Vec<(Date, u64)> {
    let mut usage: std::collections::BTreeMap<Date, u64> = std::collections::BTreeMap::new();
    for counted in events {
        if !plan.counts_any(&counted.tags) {
            continue;
        }
        let involved = match subject {
            Some(id) => counted.participants.iter().any(|named| named == id),
            None => counted
                .participants
                .iter()
                .any(|named| covered.contains(&named.as_str())),
        };
        if !involved {
            continue;
        }
        let Ok(start) = Instant::from_minutes(counted.span.0) else {
            continue;
        };
        let bucket_zone = match zone {
            Some(zone) => zone,
            None => zones.map_or(event, |zones| zones.zone_at(start)),
        };
        let label = cap_label(start.zoned(bucket_zone).date(), plan);
        let amount = match plan.measure {
            CapMeasure::Count => 1,
            CapMeasure::Minutes => u64::try_from(counted.span.1 - counted.span.0).unwrap_or(0),
        };
        *usage.entry(label).or_default() += amount;
    }
    usage.into_iter().collect()
}

/// The label a cap bucket takes: the date its civil period begins on, computed
/// from the calendar rather than with the engine's helpers.
fn cap_label(date: Date, plan: &CapPlan) -> Date {
    match plan.period {
        CapPeriod::Day => date,
        CapPeriod::Week => {
            let today = i64::from(plan_weekday_index(date.weekday()));
            let start = i64::from(plan_weekday_index(plan.week_starts));
            date.plus_days(-(today - start).rem_euclid(7))
                .expect("a supported date's week starts inside the calendar")
        }
        CapPeriod::Month => {
            Date::new(date.year(), date.month(), 1).expect("the first day of a month always exists")
        }
        // `CapPeriod` is `#[non_exhaustive]`; the oracle only builds the periods
        // 0.1 evaluates.
        _ => unreachable!("the oracle only builds the cap periods 0.1 evaluates"),
    }
}

/// Monday-first index of a weekday, written out rather than shared with the
/// engine so the two derivations stay independent.
fn plan_weekday_index(day: opportune::time::Weekday) -> u8 {
    match day {
        opportune::time::Weekday::Monday => 0,
        opportune::time::Weekday::Tuesday => 1,
        opportune::time::Weekday::Wednesday => 2,
        opportune::time::Weekday::Thursday => 3,
        opportune::time::Weekday::Friday => 4,
        opportune::time::Weekday::Saturday => 5,
        opportune::time::Weekday::Sunday => 6,
    }
}
