//! Workload caps: the events a cap measures, merged by event key, and the civil
//! bucket each one falls in.
//!
//! Caps count workload rather than availability, so every block counts whatever
//! its status or a `status_policy` decides — except an all-day block, which is
//! time away from work rather than meeting workload and never counts. The events
//! they measure are the caller's occurrences and calendar blocks, deduplicated by
//! event key: an occurrence is authoritative for the span and participants of the
//! event it shares a key with and the tags are the union of both, blocks sharing
//! a key with no occurrence merge into one event from the earliest start to the
//! latest end, and a block with no key counts on its own. The events a request
//! excludes — the ones the caller is moving — take no part in any of it.
//!
//! Buckets are civil days, weeks or months in the cap's zone, labelled with the
//! date the period begins on. An event belongs to the bucket carrying the label
//! of its start, so a date label that occurs twice, as crossing the date line
//! eastward repeats one, is a single bucket.

use std::collections::btree_map::Entry;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use crate::error::Error;
use crate::eval::ParticipantSettings;
use crate::exclude::ExcludedKeys;
use crate::intervals::{IntervalSet, Span as MinuteSpan};
use crate::model::{Cap, CapMeasure, CapPer, CapPeriod, Part, Request, RuleKind, Span};
use crate::tags::Tag;
use crate::time::{
    Date, Instant, RuleZone, TimeOfDay, TimeZoneDatabase, Weekday, Zone, ZoneSchedule,
};

/// One identifiable event a cap can measure, after event-key merging.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CountedEvent {
    /// The event's own span, without travel.
    pub(crate) span: MinuteSpan,
    /// Every tag the event is known by: the occurrence's merged with any blocks'.
    pub(crate) tags: Vec<Tag>,
    /// The request's participants the event involves, sorted and without
    /// repeats. Names the request does not carry are dropped.
    pub(crate) participants: Vec<String>,
    /// The caller's key for the event, or `None` for a keyless block that counts
    /// on its own. It breaks the remaining ties in the canonical order.
    pub(crate) event_key: Option<String>,
}

/// Every event the request can identify, merged by event key, less the events
/// `excluded` names.
///
/// The order is deterministic: by start, then end, then the participant list, the
/// tags, and the caller's event key, so two requests carrying the same events in a
/// different order produce the same sequence.
pub(crate) fn counted_events(request: &Request, excluded: &ExcludedKeys<'_>) -> Vec<CountedEvent> {
    let mut events: Vec<CountedEvent> = Vec::new();
    let known: BTreeSet<&str> = request
        .participants
        .iter()
        .map(|participant| participant.id.as_str())
        .collect();
    // The index of each key's event, and whether an occurrence supplied it.
    let mut keyed: BTreeMap<&str, (usize, bool)> = BTreeMap::new();
    // Each keyed event's tags as a set, built the first time a block joins that
    // event so a union costs a binary search per tag instead of a scan of every
    // tag the event already carries. Keyless blocks never join an event, so they
    // need no set at all.
    let mut tag_sets: BTreeMap<usize, BTreeSet<Tag>> = BTreeMap::new();

    for occurrence in &request.occurrences {
        // An excluded event is one the caller is moving: it never enters `events`,
        // so it contributes no span, no participants and no tags to any bucket.
        if excluded.excludes(Some(occurrence.event_key.as_str())) {
            continue;
        }
        // A duplicate key is a validation error; keeping the first keeps this
        // function total for a caller that reaches it without validating.
        if keyed.contains_key(occurrence.event_key.as_str()) {
            continue;
        }
        let mut participants: Vec<String> = occurrence
            .participants
            .iter()
            .filter(|id| known.contains(id.as_str()))
            .cloned()
            .collect();
        participants.sort();
        participants.dedup();
        let span = occurrence.span.to_minute_span();
        keyed.insert(occurrence.event_key.as_str(), (events.len(), true));
        events.push(CountedEvent {
            span,
            tags: occurrence.tags.clone(),
            participants,
            event_key: Some(occurrence.event_key.clone()),
        });
    }

    for participant in &request.participants {
        for block in participant.blocks() {
            if excluded.excludes(block.event_key.as_deref()) {
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
                    union_event_tags(&mut events, &mut tag_sets, index, &block.tags);
                }
                continue;
            }
            let span = block.span.to_minute_span();
            let Some(key) = &block.event_key else {
                // A keyless block counts on its own, once per calendar it
                // appears on.
                events.push(CountedEvent {
                    span,
                    tags: block.tags.clone(),
                    participants: vec![participant.id.clone()],
                    event_key: None,
                });
                continue;
            };
            match keyed.get(key.as_str()).copied() {
                // The occurrence is authoritative for the span and participants;
                // only the tags are unioned.
                Some((index, true)) => {
                    union_event_tags(&mut events, &mut tag_sets, index, &block.tags);
                }
                Some((index, false)) => {
                    {
                        let event = &mut events[index];
                        event.span = event.span.hull(span);
                        // The tail check keeps the common case free: one
                        // participant's blocks are visited contiguously, so its ID
                        // is already last. It is only an optimisation — duplicate
                        // participant entries, or a key met again in a
                        // non-contiguous stretch, are removed by the `sort`/`dedup`
                        // pass below.
                        if event.participants.last() != Some(&participant.id) {
                            event.participants.push(participant.id.clone());
                        }
                    }
                    union_event_tags(&mut events, &mut tag_sets, index, &block.tags);
                }
                None => {
                    keyed.insert(key.as_str(), (events.len(), false));
                    events.push(CountedEvent {
                        span,
                        tags: block.tags.clone(),
                        participants: vec![participant.id.clone()],
                        event_key: Some(key.clone()),
                    });
                }
            }
        }
    }

    for event in &mut events {
        event.participants.sort();
        event.participants.dedup();
    }
    events.sort_by(|left, right| {
        (left.span.start(), left.span.end())
            .cmp(&(right.span.start(), right.span.end()))
            .then_with(|| left.participants.cmp(&right.participants))
            .then_with(|| left.tags.cmp(&right.tags))
            .then_with(|| left.event_key.cmp(&right.event_key))
    });
    events
}

/// Adds `extra` to the tags of the event at `index`, keeping the set that mirrors
/// them.
///
/// The set is built from the event's tags the first time a block joins it, so
/// every later union costs a binary search per tag rather than a scan of every
/// tag the event already carries.
fn union_event_tags(
    events: &mut [CountedEvent],
    tag_sets: &mut BTreeMap<usize, BTreeSet<Tag>>,
    index: usize,
    extra: &[Tag],
) {
    let seen = tag_sets
        .entry(index)
        .or_insert_with(|| events[index].tags.iter().cloned().collect());
    union_tags(&mut events[index].tags, seen, extra);
}

/// Adds every tag of `extra` the event does not already carry, keeping the order
/// the tags were first seen in.
///
/// `seen` mirrors `target` as a set, so a key shared by thousands of blocks with
/// thousands of distinct tags costs `O(log n)` per tag rather than a scan of every
/// tag already carried. The caller initialises `seen` from `target` the first time
/// it unions into that event.
fn union_tags(target: &mut Vec<Tag>, seen: &mut BTreeSet<Tag>, extra: &[Tag]) {
    for tag in extra {
        if seen.insert(tag.clone()) {
            target.push(tag.clone());
        }
    }
}

/// How a cap's bucket zone is resolved and shared: pooled caps always carry a
/// fixed zone, while per-participant caps either carry a fixed zone or follow
/// the participant's own zone schedule (`None`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum CapScope {
    /// Pooled across covered participants, always bucketed in one fixed zone.
    Pooled(Zone),
    /// Per participant, bucketed in `Some(zone)` or the participant's schedule.
    Each(Option<Zone>),
}

/// One cap that applies to a participant, with the usage it already holds in
/// each bucket.
///
/// `usage` is keyed by the bucket's label date and shared via [`Arc`] for pooled
/// caps so covered participants do not clone the bucket map.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ParticipantCap<'a> {
    /// The rule that supplied the exclusion, for the reason's citation.
    pub(crate) rule: &'a str,
    /// The most the period may hold.
    pub(crate) limit: u32,
    /// What the booking itself would add: one event, or its minutes.
    pub(crate) contribution: u64,
    /// The civil period a bucket covers.
    pub(crate) period: CapPeriod,
    /// The weekday a week bucket starts on.
    pub(crate) week_starts: Weekday,
    /// Whether this cap is pooled or per-participant, together with its bucket zone.
    pub(crate) scope: CapScope,
    /// The usage already in each reachable bucket, by label.
    pub(crate) usage: Arc<BTreeMap<Date, u64>>,
}

impl ParticipantCap<'_> {
    /// The usage in the bucket containing `start` and that bucket's label.
    fn usage_at(&self, schedule: &ZoneSchedule, start: Instant) -> (u64, Date) {
        let date = match &self.scope {
            CapScope::Pooled(zone) | CapScope::Each(Some(zone)) => zone.zoned(start).date(),
            CapScope::Each(None) => schedule.zone_at(start).zoned(start).date(),
        };
        let label = bucket_label(date, self.period, self.week_starts);
        (self.usage.get(&label).copied().unwrap_or(0), label)
    }

    /// Whether adding the booking's contribution would take the bucket past the
    /// limit, and the usage and label the exclusion reports.
    pub(crate) fn exceeded(&self, schedule: &ZoneSchedule, start: Instant) -> Option<(u64, Date)> {
        let (used, label) = self.usage_at(schedule, start);
        (used.saturating_add(self.contribution) > u64::from(self.limit)).then_some((used, label))
    }

    /// Whether even an empty bucket exceeds the limit on its own.
    pub(crate) fn always_exceeded(&self) -> bool {
        self.contribution > u64::from(self.limit)
    }

    /// The bucket labels whose existing usage already leaves no room for this
    /// booking, sorted ascending.
    pub(crate) fn exceeded_buckets(&self) -> Vec<Date> {
        let limit = u64::from(self.limit);
        self.usage
            .iter()
            .filter_map(|(&label, &used)| {
                (used.saturating_add(self.contribution) > limit).then_some(label)
            })
            .collect()
    }
}

/// Cached constant-date segments for fixed zones across a search window.
type FixedZoneSegments = BTreeMap<String, Vec<(i64, i64, Date)>>;

/// Returns the constant-date segments `(from, until, date)` of `window` in
/// `zone`, computing them at most once per zone name.
fn fixed_zone_segments<'a>(
    cache: &'a mut FixedZoneSegments,
    zone: &Zone,
    window: Span,
) -> Result<&'a [(i64, i64, Date)], Error> {
    let segments = match cache.entry(zone.name().to_owned()) {
        Entry::Occupied(entry) => entry.into_mut(),
        Entry::Vacant(entry) => {
            let fixed = ZoneSchedule::fixed(zone.clone());
            let jumps = fixed.date_jumps(window.start(), window.end())?;
            let mut segments = Vec::new();
            fixed.for_each_date_segment(
                &jumps,
                window.start().minutes(),
                window.end().minutes(),
                |from, until, date| segments.push((from, until, date)),
            );
            entry.insert(segments)
        }
    };
    Ok(segments.as_slice())
}

/// Collects start-minute spans in `segments` whose bucket label is in `exceeded`.
fn collect_exceeded_from_segments(
    segments: &[(i64, i64, Date)],
    period: CapPeriod,
    week_starts: Weekday,
    exceeded: &[Date],
    out: &mut Vec<MinuteSpan>,
) {
    for &(from, until, date) in segments {
        let label = bucket_label(date, period, week_starts);
        if exceeded.binary_search(&label).is_ok()
            && let Ok(span) = MinuteSpan::new(from, until)
        {
            out.push(span);
        }
    }
}

/// Precomputes which start minutes inside `window` fail at least one applicable
/// cap for one participant, reusing cached fixed-zone date segments and pooled
/// cap blocked spans across participants.
fn blocked_starts_with_caches<'a>(
    caps: &[ParticipantCap<'a>],
    schedule: &ZoneSchedule,
    window: Span,
    fixed_segments: &mut FixedZoneSegments,
    pooled_blocked: &mut BTreeMap<&'a str, Vec<MinuteSpan>>,
) -> Result<(bool, IntervalSet), Error> {
    if caps.is_empty() {
        return Ok((false, IntervalSet::new()));
    }
    let mut always = false;
    let mut spans: Vec<MinuteSpan> = Vec::new();
    let mut participant_jumps: Option<Vec<i64>> = None;
    for cap in caps {
        if cap.always_exceeded() {
            always = true;
            break;
        }
        match &cap.scope {
            CapScope::Pooled(zone) => {
                if let Some(cached) = pooled_blocked.get(cap.rule) {
                    spans.extend_from_slice(cached);
                    continue;
                }
                let exceeded = cap.exceeded_buckets();
                let mut rule_spans = Vec::new();
                if !exceeded.is_empty() {
                    let segments = fixed_zone_segments(fixed_segments, zone, window)?;
                    collect_exceeded_from_segments(
                        segments,
                        cap.period,
                        cap.week_starts,
                        &exceeded,
                        &mut rule_spans,
                    );
                }
                spans.extend_from_slice(&rule_spans);
                pooled_blocked.insert(cap.rule, rule_spans);
            }
            CapScope::Each(fixed_zone) => {
                let exceeded = cap.exceeded_buckets();
                if exceeded.is_empty() {
                    continue;
                }
                if let Some(zone) = fixed_zone {
                    let segments = fixed_zone_segments(fixed_segments, zone, window)?;
                    collect_exceeded_from_segments(
                        segments,
                        cap.period,
                        cap.week_starts,
                        &exceeded,
                        &mut spans,
                    );
                } else if schedule.periods().is_empty() {
                    let segments = fixed_zone_segments(fixed_segments, schedule.home(), window)?;
                    collect_exceeded_from_segments(
                        segments,
                        cap.period,
                        cap.week_starts,
                        &exceeded,
                        &mut spans,
                    );
                } else {
                    let jumps = if let Some(jumps) = &participant_jumps {
                        jumps.as_slice()
                    } else {
                        let computed = schedule.date_jumps(window.start(), window.end())?;
                        participant_jumps.insert(computed).as_slice()
                    };
                    schedule.for_each_date_segment(
                        jumps,
                        window.start().minutes(),
                        window.end().minutes(),
                        |from, until, date| {
                            let label = bucket_label(date, cap.period, cap.week_starts);
                            if exceeded.binary_search(&label).is_ok()
                                && let Ok(span) = MinuteSpan::new(from, until)
                            {
                                spans.push(span);
                            }
                        },
                    );
                }
            }
        }
    }
    if always {
        Ok((true, IntervalSet::new()))
    } else {
        Ok((false, IntervalSet::from_spans(spans)))
    }
}

/// The label a bucket takes: the date the civil period begins on.
///
/// # Panics
/// A caller passes the local date of a supported instant, which lies inside
/// 1975 to 2100; stepping back to a week's start and forward to a month's first
/// day cannot leave the calendar there.
pub(crate) fn bucket_label(date: Date, period: CapPeriod, week_starts: Weekday) -> Date {
    match period {
        CapPeriod::Day => date,
        CapPeriod::Week => date
            .week_start(week_starts)
            .expect("a supported date's week starts inside the calendar"),
        CapPeriod::Month => date.first_of_month(),
    }
}

/// The inclusive range of bucket labels (and a conservative UTC minute envelope
/// around them) that candidate starts in `[first_start, last_start_inclusive]`
/// can query in any timezone.
struct BucketFilter {
    min_label: Option<Date>,
    max_label: Option<Date>,
    min_utc_minute: Option<i64>,
    max_utc_minute: Option<i64>,
}

impl BucketFilter {
    fn for_target(
        first_start: Instant,
        last_start_inclusive: Instant,
        period: CapPeriod,
        week_starts: Weekday,
    ) -> Self {
        let utc_first = Zone::utc().zoned(first_start).date();
        let utc_last = Zone::utc().zoned(last_start_inclusive).date();
        // Every IANA timezone offset in 1975..=2100 is within ±1 day of UTC, so
        // padding by 2 days on either side covers every local date any timezone
        // can assign to a start in `[first_start, last_start_inclusive]`.
        let min_label = utc_first
            .plus_days(-2)
            .ok()
            .map(|date| bucket_label(date, period, week_starts));
        let max_label = utc_last
            .plus_days(2)
            .ok()
            .map(|date| bucket_label(date, period, week_starts));
        let min_utc_minute = min_label
            .and_then(|label| Instant::from_utc(label, TimeOfDay::MIDNIGHT).ok())
            .map(|instant| instant.minutes().saturating_sub(2 * 1_440));
        let max_utc_minute = max_label
            .and_then(|label| bucket_after(label, period).ok())
            .and_then(|after| Instant::from_utc(after, TimeOfDay::MIDNIGHT).ok())
            .map(|instant| instant.minutes().saturating_add(2 * 1_440));
        Self {
            min_label,
            max_label,
            min_utc_minute,
            max_utc_minute,
        }
    }

    fn may_include_minute(&self, start_minute: i64) -> bool {
        if self.min_utc_minute.is_some_and(|lo| start_minute < lo) {
            return false;
        }
        if self.max_utc_minute.is_some_and(|hi| start_minute >= hi) {
            return false;
        }
        true
    }

    fn includes_label(&self, label: Date) -> bool {
        if self.min_label.is_some_and(|lo| label < lo) {
            return false;
        }
        if self.max_label.is_some_and(|hi| label > hi) {
            return false;
        }
        true
    }
}

fn bucket_after(label: Date, period: CapPeriod) -> Result<Date, Error> {
    Ok(match period {
        CapPeriod::Day => label.plus_days(1)?,
        CapPeriod::Week => label.plus_days(7)?,
        CapPeriod::Month => label.next_month_start()?,
    })
}

/// Populates every participant's applicable caps (and, for searches, their
/// precomputed `cap_always_blocked` / `cap_blocked` sets) in place, borrowing
/// each participant's already-resolved schedule and selected rules directly.
///
/// Bucket accumulation is filtered to buckets reachable by `target_starts`
/// (`(first_start, last_start_inclusive)`), so events far outside the judged
/// window never bloat per-cap bucket maps.
///
/// # Errors
/// Returns an [`Error::Time`] failure for an unknown named zone, which `validate`
/// reports first.
#[allow(clippy::too_many_arguments)]
pub(crate) fn populate_caps<'a>(
    request: &'a Request,
    part: &Part,
    event: &Zone,
    db: &TimeZoneDatabase,
    participants: &mut [ParticipantSettings<'a>],
    excluded: &ExcludedKeys<'_>,
    cap_blocked_over: Option<Span>,
    target_starts: (Instant, Instant),
) -> Result<(), Error> {
    // Which participants each cap rule covers, in the order `select` selected it.
    let mut covered: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
    for (index, settings) in participants.iter().enumerate() {
        for (id, _) in &settings.rules.caps {
            covered.entry(id).or_default().push(index);
        }
    }
    if covered.is_empty() {
        return Ok(());
    }
    let events = counted_events(request, excluded);
    // The events each cap measures, borrowed from `events` and reused across cap
    // rules: building the view costs one pass over the events per rule, and no
    // event's tags, participants or key are copied for it.
    let mut measured: Vec<&CountedEvent> = Vec::new();
    for rule in &request.rules {
        let RuleKind::Cap(cap) = &rule.kind else {
            continue;
        };
        let Some(covered) = covered.get(rule.id.as_str()) else {
            continue;
        };
        let covered: &[usize] = covered;
        let filter = BucketFilter::for_target(
            target_starts.0,
            target_starts.1,
            cap.period,
            cap.week_starts,
        );
        measured.clear();
        measured.extend(events.iter().filter(|event| {
            filter.may_include_minute(event.span.start()) && cap.count.matches_item(&event.tags)
        }));
        let contribution = contribution(cap, part);
        match cap.per {
            CapPer::Pooled => {
                let fixed = fixed_cap_zone(cap, event, db)?;
                let covered_ids: BTreeSet<&str> = covered
                    .iter()
                    .map(|index| request.participants[*index].id.as_str())
                    .collect();
                let usage = Arc::new(pooled_usage(cap, &fixed, &covered_ids, &measured, &filter)?);
                for &index in covered {
                    participants[index].caps.push(ParticipantCap {
                        rule: &rule.id,
                        limit: cap.limit,
                        contribution,
                        period: cap.period,
                        week_starts: cap.week_starts,
                        scope: CapScope::Pooled(fixed.clone()),
                        usage: Arc::clone(&usage),
                    });
                }
            }
            CapPer::Each => {
                let zone = each_cap_zone(cap, event, db)?;
                // One pass over the measured events adds each event to every
                // covered participant it names, so the cost tracks the events and
                // the people they involve rather than the whole participant list.
                let mut usages = each_usages(
                    cap,
                    zone.as_ref(),
                    participants,
                    request,
                    covered,
                    &measured,
                    &filter,
                )?;
                for &index in covered {
                    participants[index].caps.push(ParticipantCap {
                        rule: &rule.id,
                        limit: cap.limit,
                        contribution,
                        period: cap.period,
                        week_starts: cap.week_starts,
                        scope: CapScope::Each(zone.clone()),
                        usage: Arc::new(std::mem::take(&mut usages[index])),
                    });
                }
            }
        }
    }

    if let Some(window) = cap_blocked_over {
        let mut fixed_segments = FixedZoneSegments::new();
        let mut pooled_blocked: BTreeMap<&'a str, Vec<MinuteSpan>> = BTreeMap::new();
        for settings in participants.iter_mut() {
            let (cap_always_blocked, cap_blocked) = blocked_starts_with_caches(
                &settings.caps,
                &settings.schedule,
                window,
                &mut fixed_segments,
                &mut pooled_blocked,
            )?;
            settings.cap_always_blocked = cap_always_blocked;
            settings.cap_blocked = cap_blocked;
        }
    }
    Ok(())
}

/// What the booking adds to a bucket: one event, or its own minutes.
fn contribution(cap: &Cap, part: &Part) -> u64 {
    match cap.measure {
        CapMeasure::Count => 1,
        CapMeasure::Minutes => u64::try_from(part.duration.minutes()).unwrap_or(0),
    }
}

/// What one event adds to a bucket under `cap`'s measure.
fn amount(cap: &Cap, event: &CountedEvent) -> u64 {
    match cap.measure {
        CapMeasure::Count => 1,
        CapMeasure::Minutes => u64::try_from(event.span.end() - event.span.start()).unwrap_or(0),
    }
}

/// The fixed zone a pooled cap's buckets are read in.
///
/// A pooled cap in the participant's zone is a validation error, because a pool
/// has no single zone to bucket in; evaluation falls back to the event zone
/// rather than failing a request `validate` already rejects.
fn fixed_cap_zone(cap: &Cap, event: &Zone, db: &TimeZoneDatabase) -> Result<Zone, Error> {
    Ok(match &cap.zone {
        RuleZone::Participant | RuleZone::Event => event.clone(),
        RuleZone::Named(name) => db.zone(name.as_str())?,
    })
}

/// The zone a per-participant cap's buckets are read in: `None` when the cap
/// follows the participant's own zone, and the fixed zone otherwise.
fn each_cap_zone(cap: &Cap, event: &Zone, db: &TimeZoneDatabase) -> Result<Option<Zone>, Error> {
    match &cap.zone {
        RuleZone::Participant => Ok(None),
        RuleZone::Event => Ok(Some(event.clone())),
        RuleZone::Named(name) => Ok(Some(db.zone(name.as_str())?)),
    }
}

/// The usage a pooled cap already holds, counting each event once however many
/// covered participants it names.
fn pooled_usage(
    cap: &Cap,
    fixed: &Zone,
    covered: &BTreeSet<&str>,
    measured: &[&CountedEvent],
    filter: &BucketFilter,
) -> Result<BTreeMap<Date, u64>, Error> {
    let mut usage: BTreeMap<Date, u64> = BTreeMap::new();
    for event in measured {
        if !event
            .participants
            .iter()
            .any(|named| covered.contains(named.as_str()))
        {
            continue;
        }
        let start = Instant::from_minutes(event.span.start())?;
        let label = bucket_label(fixed.zoned(start).date(), cap.period, cap.week_starts);
        if !filter.includes_label(label) {
            continue;
        }
        let entry = usage.entry(label).or_default();
        *entry = entry.saturating_add(amount(cap, event));
    }
    Ok(usage)
}

/// The usage each covered participant's `each` cap already holds, indexed like
/// the request's participants.
///
/// A single pass over the events reads every participant an event names, so a
/// request with many participants and many blocks costs the events times the
/// people each one involves, not their product.
fn each_usages(
    cap: &Cap,
    zone: Option<&Zone>,
    participants: &[ParticipantSettings<'_>],
    request: &Request,
    covered: &[usize],
    measured: &[&CountedEvent],
    filter: &BucketFilter,
) -> Result<Vec<BTreeMap<Date, u64>>, Error> {
    let covered_index: BTreeMap<&str, usize> = covered
        .iter()
        .map(|index| (request.participants[*index].id.as_str(), *index))
        .collect();
    let mut usages: Vec<BTreeMap<Date, u64>> = vec![BTreeMap::new(); participants.len()];
    for event in measured {
        let start = Instant::from_minutes(event.span.start())?;
        let amount = amount(cap, event);
        for id in &event.participants {
            let Some(index) = covered_index.get(id.as_str()).copied() else {
                continue;
            };
            let bucket_zone = match zone {
                Some(zone) => zone,
                None => participants[index].schedule.zone_at(start),
            };
            let label = bucket_label(bucket_zone.zoned(start).date(), cap.period, cap.week_starts);
            if !filter.includes_label(label) {
                continue;
            }
            let entry = usages[index].entry(label).or_default();
            *entry = entry.saturating_add(amount);
        }
    }
    Ok(usages)
}
