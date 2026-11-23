//! Proptest strategies for bounded scheduling inputs.
//!
//! Generators stay inside a small minute domain so the oracle's point-by-point
//! models stay fast, and so widening cannot leave the representable range.

use opportune::model::{BlockStatus, CapPeriod, StatusEffect};
use opportune::tags::Role;
use opportune::time::{Date, Instant, LocalResolution, TimeOfDay, Zone};
use proptest::prelude::*;

use crate::a1::{
    A1Case, BlackoutPlan, BlockPlan, CapPlan, ClearancePlan, GridPlan, GridZonePlan, LeadMaxPlan,
    LeadTimePlan, OccurrencePlan, ParticipantPlan, RestPlan, RulePlan, RuleZonePlan,
    StatusEntryPlan, StatusPolicyPlan, TravelPlan, WindowPlan,
};
use crate::a2::{A2Case, A2Participant, A2Rule, TermPlan};
use crate::specs::{BlackoutCase, DateRangePlan, PlanZone, WindowCase, ZonePlan};

/// The half-open upper bound of the minute domain the generators produce:
/// every generated minute is `0 <= minute < MINUTE_DOMAIN_END`.
pub const MINUTE_DOMAIN_END: i64 = 400;

/// A non-empty half-open span `[start, end)` within the minute domain.
///
/// ```
/// use opportune_testkit::generators::{span, MINUTE_DOMAIN_END};
/// use proptest::prelude::*;
/// use proptest::test_runner::{Config, TestRunner};
///
/// // Run the strategy directly: a `proptest!` block inside a doctest defines a
/// // nested function that a doctest's `main` never calls, so it would pass
/// // without executing a single case.
/// TestRunner::new(Config::with_cases(20))
///     .run(&span(), |(start, end)| {
///         prop_assert!(start < end);
///         prop_assert!(end <= MINUTE_DOMAIN_END);
///         Ok(())
///     })
///     .unwrap();
/// ```
pub fn span() -> impl Strategy<Value = (i64, i64)> {
    (0..MINUTE_DOMAIN_END).prop_flat_map(|start| {
        let end = start + 1..=MINUTE_DOMAIN_END;
        (Just(start), end)
    })
}

/// A list of up to eight spans within the minute domain.
pub fn span_list() -> impl Strategy<Value = Vec<(i64, i64)>> {
    proptest::collection::vec(span(), 0..=8)
}

/// A pair of non-negative widening amounts, bounded so a widened span starts
/// no earlier than `-WIDEN_MAX` and ends no later than
/// `MINUTE_DOMAIN_END + WIDEN_MAX`.
pub fn widen_amounts() -> impl Strategy<Value = (i64, i64)> {
    (0..=WIDEN_MAX, 0..=WIDEN_MAX)
}

/// The upper bound for each widening amount produced by [`widen_amounts`].
pub const WIDEN_MAX: i64 = 60;

/// A valid tag path: one to four segments joined by `/`.
///
/// ```
/// use opportune_testkit::generators::tag_path;
/// use proptest::prelude::*;
/// use proptest::test_runner::{Config, TestRunner};
///
/// TestRunner::new(Config::with_cases(20))
///     .run(&tag_path(), |path| {
///         prop_assert!(!path.starts_with('/'));
///         prop_assert!(!path.ends_with('/'));
///         prop_assert!(!path.contains("//"));
///         Ok(())
///     })
///     .unwrap();
/// ```
pub fn tag_path() -> impl Strategy<Value = String> {
    proptest::collection::vec("[A-Za-z0-9._-]{1,8}", 1..=4).prop_map(|segments| segments.join("/"))
}

/// A weekly key in plan form: a Monday-first day mask and the ranges those days
/// share, each `(start, end)` in minutes from the start of the day.
type WeeklyKeyPlan = (u8, Vec<(u16, u16)>);

/// A date override in plan form: start and end day offsets from the anchor, and
/// the ranges those dates have.
type DateOverridePlan = (i16, i16, Vec<(u16, u16)>);

/// The zone matrix the generated time cases draw zone names from: both
/// hemispheres, a half-hour daylight saving shift, 5:30 and 5:45 offsets, and
/// UTC.
pub const ZONE_MATRIX: [&str; 9] = [
    "America/Los_Angeles",
    "Europe/London",
    "Australia/Sydney",
    "Australia/Lord_Howe",
    "Asia/Kolkata",
    "Asia/Kathmandu",
    "America/Anchorage",
    "Asia/Tokyo",
    "UTC",
];

/// The civil dates generation anchors on: a daylight saving change in each
/// hemisphere (including Lord Howe's half hour), a leap day, and an ordinary
/// day.
const BASE_DATES: [(i16, i8, i8); 8] = [
    (2027, 3, 14),
    (2027, 3, 28),
    (2027, 4, 4),
    (2027, 10, 3),
    (2027, 10, 31),
    (2027, 11, 7),
    (2028, 2, 28),
    (2027, 6, 15),
];

/// A generated window case: a specification and the UTC range it is read over.
///
/// ```
/// use opportune_testkit::generators::window_case;
/// use proptest::prelude::*;
/// use proptest::test_runner::{Config, TestRunner};
///
/// TestRunner::new(Config::with_cases(8))
///     .run(&window_case(), |case| {
///         let length = case.until.minutes() - case.from.minutes();
///         prop_assert!(length > 1_440);
///         prop_assert!(length <= 4_320);
///         Ok(())
///     })
///     .unwrap();
/// ```
pub fn window_case() -> impl Strategy<Value = WindowCase> {
    (
        0..BASE_DATES.len(),
        plan_zone(),
        zone_name(),
        zone_name(),
        zone_name(),
        weekly_keys(),
        date_overrides(),
        period_plan(),
        range_offsets(),
    )
        .prop_map(
            |(base, zone, home, event, named, weekly, dates, periods, (start_offset, length))| {
                let (base_date, from, until) = domain(base, start_offset, length);
                WindowCase {
                    zone,
                    zones: ZonePlan {
                        home,
                        periods: resolve_periods(periods, from),
                    },
                    event,
                    named,
                    weekly,
                    dates: date_keys(&dates, base_date),
                    from,
                    until,
                }
            },
        )
}

/// A generated blackout case: local date-time spans and the UTC range they are
/// read over.
///
/// ```
/// use opportune_testkit::generators::blackout_case;
/// use proptest::prelude::*;
/// use proptest::test_runner::{Config, TestRunner};
///
/// TestRunner::new(Config::with_cases(8))
///     .run(&blackout_case(), |case| {
///         for (start, end) in &case.spans {
///             prop_assert!(start < end);
///         }
///         Ok(())
///     })
///     .unwrap();
/// ```
pub fn blackout_case() -> impl Strategy<Value = BlackoutCase> {
    (
        0..BASE_DATES.len(),
        plan_zone(),
        zone_name(),
        zone_name(),
        zone_name(),
        proptest::collection::vec(span_plan(), 0..=3),
        period_plan(),
        range_offsets(),
    )
        .prop_map(
            |(base, zone, home, event, named, spans, periods, (start_offset, length))| {
                let (base_date, from, until) = domain(base, start_offset, length);
                BlackoutCase {
                    zone,
                    zones: ZonePlan {
                        home,
                        periods: resolve_periods(periods, from),
                    },
                    event,
                    named,
                    spans: spans
                        .into_iter()
                        .map(|(start_day, start_minute, end_day, end_minute)| {
                            (
                                (day_after(base_date, start_day), minute_of_day(start_minute)),
                                (day_after(base_date, end_day), minute_of_day(end_minute)),
                            )
                        })
                        .collect(),
                    from,
                    until,
                }
            },
        )
}

fn plan_zone() -> impl Strategy<Value = PlanZone> {
    prop_oneof![
        Just(PlanZone::Participant),
        Just(PlanZone::Event),
        Just(PlanZone::Named),
    ]
}

fn zone_name() -> impl Strategy<Value = String> {
    proptest::sample::select(&ZONE_MATRIX).prop_map(str::to_owned)
}

/// A start offset within the day before the anchor, and a range length, both in
/// minutes. The range therefore begins at an arbitrary minute and crosses days,
/// so the builder is exercised away from midnight-aligned bounds.
fn range_offsets() -> impl Strategy<Value = (u32, u32)> {
    (0u32..1_440, 1_441u32..4_320)
}

/// The anchor date and the UTC range a case is read over: one day before the
/// anchor, offset by `start_offset` minutes, for `length` minutes.
fn domain(base: usize, start_offset: u32, length: u32) -> (Date, Instant, Instant) {
    let (year, month, day) = BASE_DATES[base];
    let base_date = Date::new(year, month, day).expect("the base dates are real dates");
    let LocalResolution::Unique(midnight) = Zone::utc()
        .resolve_local(base_date, TimeOfDay::MIDNIGHT)
        .expect("UTC has no transitions")
    else {
        panic!("UTC has no transitions");
    };
    let from = Instant::from_minutes(midnight.minutes() - 1_440 + i64::from(start_offset))
        .expect("the range is supported");
    let until =
        Instant::from_minutes(from.minutes() + i64::from(length)).expect("the range is supported");
    (base_date, from, until)
}

fn day_after(date: Date, days: i16) -> Date {
    date.plus_days(i64::from(days))
        .expect("a date near 2027 can move a few days")
}

fn minute_of_day(minute: u16) -> TimeOfDay {
    TimeOfDay::from_minute_of_day(minute).expect("a minute below 1440 is a time of day")
}

/// A Monday-first day mask: a single day, a short list, or a wrapping range.
fn day_mask() -> impl Strategy<Value = u8> {
    prop_oneof![
        (0usize..7).prop_map(|day| 1u8 << day),
        proptest::collection::vec(0usize..7, 2..=3)
            .prop_map(|days| days.into_iter().fold(0u8, |mask, day| mask | (1 << day))),
        (0usize..7, 0usize..7).prop_map(|(first, last)| {
            let mut mask = 0u8;
            let mut day = first;
            loop {
                mask |= 1 << day;
                if day == last {
                    break;
                }
                day = (day + 1) % 7;
            }
            mask
        }),
    ]
}

/// Up to three non-overlapping ranges within a day, each `(start, end)` in
/// minutes from the day's start. A crossing range has `end < start`; `1440` is
/// the end of the day.
fn minute_ranges() -> impl Strategy<Value = Vec<(u16, u16)>> {
    proptest::collection::vec((0u16..1_440, 0u16..=1_440), 0..=3).prop_map(|raw| {
        let mut kept: Vec<(u16, u16)> = Vec::new();
        for (start, end) in raw {
            if start == end {
                continue;
            }
            let end_of_range = linear_end(start, end);
            let overlaps = kept.iter().any(|&(kept_start, kept_end)| {
                start < linear_end(kept_start, kept_end) && kept_start < end_of_range
            });
            if !overlaps {
                kept.push((start, end));
            }
        }
        kept
    })
}

fn linear_end(start: u16, end: u16) -> u16 {
    if end < start { end + 1_440 } else { end }
}

/// Up to three weekly keys whose day masks do not overlap.
fn weekly_keys() -> impl Strategy<Value = Vec<WeeklyKeyPlan>> {
    proptest::collection::vec((day_mask(), minute_ranges()), 0..=3).prop_map(|raw| {
        let mut kept: Vec<WeeklyKeyPlan> = Vec::new();
        let mut claimed = 0u8;
        for (mask, hours) in raw {
            if mask & claimed != 0 {
                continue;
            }
            claimed |= mask;
            kept.push((mask, hours));
        }
        kept
    })
}

/// Up to three disjoint inclusive date ranges as `(start offset, length, hours)`.
fn date_overrides() -> impl Strategy<Value = Vec<DateOverridePlan>> {
    proptest::collection::vec((-1i16..4, 0i16..3, minute_ranges()), 0..=3).prop_map(|raw| {
        let mut ranges: Vec<DateOverridePlan> = raw
            .into_iter()
            .map(|(start, length, hours)| (start, start + length, hours))
            .collect();
        ranges.sort_by_key(|(start, _, _)| *start);
        let mut kept: Vec<DateOverridePlan> = Vec::new();
        for (start, end, hours) in ranges {
            if kept
                .last()
                .is_some_and(|(_, kept_end, _)| start <= *kept_end)
            {
                continue;
            }
            kept.push((start, end, hours));
        }
        kept
    })
}

fn date_keys(overrides: &[DateOverridePlan], base: Date) -> Vec<DateRangePlan> {
    overrides
        .iter()
        .map(|(start, end, hours)| {
            (
                day_after(base, *start),
                day_after(base, *end),
                hours.clone(),
            )
        })
        .collect()
}

/// Up to two zone legs as `(zone name, start offset, end offset)` minutes from
/// the range's start.
fn period_plan() -> impl Strategy<Value = Vec<(String, u32, u32)>> {
    proptest::collection::vec((zone_name(), 0u32..4_320, 0u32..4_320), 0..=2)
}

fn resolve_periods(raw: Vec<(String, u32, u32)>, from: Instant) -> Vec<(String, Instant, Instant)> {
    let mut periods: Vec<(String, u32, u32)> = raw
        .into_iter()
        .map(|(name, a, b)| (name, a.min(b), a.max(b)))
        .filter(|(_, start, end)| start < end)
        .collect();
    periods.sort_by_key(|(_, start, _)| *start);
    let mut kept: Vec<(String, Instant, Instant)> = Vec::new();
    for (name, start, end) in periods {
        let start = Instant::from_minutes(from.minutes() + i64::from(start))
            .expect("the range is supported");
        let end =
            Instant::from_minutes(from.minutes() + i64::from(end)).expect("the range is supported");
        if kept
            .last()
            .is_some_and(|(_, _, kept_end)| start < *kept_end)
        {
            continue;
        }
        kept.push((name, start, end));
    }
    kept
}

/// A span as `(start day offset, start minute, end day offset, end minute)`,
/// with the start strictly before the end.
fn span_plan() -> impl Strategy<Value = (i16, u16, i16, u16)> {
    (-1i16..4, 0u16..1_440, -1i16..4, 0u16..1_440).prop_map(
        |(first_day, first_minute, second_day, second_minute)| {
            if (first_day, first_minute) == (second_day, second_minute) {
                if first_minute < 1_439 {
                    return (first_day, first_minute, first_day, first_minute + 1);
                }
                return (first_day, first_minute, first_day + 1, 0);
            }
            if (first_day, first_minute) < (second_day, second_minute) {
                (first_day, first_minute, second_day, second_minute)
            } else {
                (second_day, second_minute, first_day, first_minute)
            }
        },
    )
}

// ---------------------------------------------------------------------------
// Whole-request (single-participant) cases
//
// A generated case keeps the single-participant shape: one host, one term, a
// five-day ceiling and every rule kind the engine evaluates. Anchors run across
// 1975-2099 and include the dates daylight saving changes fall on in both
// hemispheres, so the oracle meets both transitions and ordinary days.
// ---------------------------------------------------------------------------

/// Month and day pairs generation anchors on. All are ordinary real dates, and
/// the offsets a case adds stay inside the supported range for 1975-2099.
const A1_BASE_DATES: [(i8, i8); 8] = [
    (3, 14),
    (3, 28),
    (4, 4),
    (10, 3),
    (10, 31),
    (11, 7),
    (6, 15),
    (2, 28),
];

/// Selects excluded event keys deterministically from the keys present in a
/// case: empty half the time, a non-empty subset of the case's own keys a
/// quarter of the time, and a mix with unknown (and optionally duplicated) keys
/// the rest of the time.
fn select_excluded_keys(available: &[String], mode: u8, mask: u64, duplicate: bool) -> Vec<String> {
    match mode % 4 {
        0 | 1 => Vec::new(),
        2 => {
            if available.is_empty() {
                return Vec::new();
            }
            let mut picked: Vec<String> = available
                .iter()
                .enumerate()
                .filter(|(index, _)| (mask >> (index % 64)) & 1 == 1)
                .map(|(_, key)| key.clone())
                .collect();
            if picked.is_empty() {
                let fallback = usize::try_from(mask % u64::try_from(available.len()).unwrap_or(1))
                    .unwrap_or(0);
                picked.push(available[fallback].clone());
            }
            picked
        }
        _ => {
            let mut picked: Vec<String> = available
                .iter()
                .enumerate()
                .filter(|(index, _)| (mask >> (index % 64)) & 1 == 1)
                .take(2)
                .map(|(_, key)| key.clone())
                .collect();
            picked.push("unmatched-event-key".to_owned());
            if duplicate && let Some(first) = picked.first().cloned() {
                picked.push(first);
            }
            picked
        }
    }
}

/// Collects the sorted, unique event keys carried by a set of calendars and
/// occurrences, without using hash-based collections.
fn collect_event_keys<'a>(
    calendars: impl IntoIterator<Item = Option<&'a [BlockPlan]>>,
    occurrences: &[OccurrencePlan],
) -> Vec<String> {
    let mut keys = std::collections::BTreeSet::new();
    for blocks in calendars.into_iter().flatten() {
        for block in blocks {
            if let Some(key) = &block.event_key {
                keys.insert(key.clone());
            }
        }
    }
    for occurrence in occurrences {
        keys.insert(occurrence.event_key.clone());
    }
    keys.into_iter().collect()
}

/// A generated whole request: one host, a goal with a grid or candidates, travel,
/// and a list of rules.
///
/// ```
/// use opportune_testkit::a1::A1Oracle;
/// use opportune_testkit::generators::a1_case;
/// use opportune::time::TimeZoneDatabase;
/// use proptest::prelude::*;
/// use proptest::test_runner::{Config, TestRunner};
///
/// let db = TimeZoneDatabase::bundled().unwrap();
/// TestRunner::new(Config::with_cases(4))
///     .run(&a1_case(), |case| {
///         let length = case.search.1.minutes() - case.search.0.minutes();
///         prop_assert!(length >= 2_880 && length <= 7_200);
///         let oracle = A1Oracle::new(&case, &db);
///         prop_assert!(!oracle.candidate_starts().is_empty());
///         Ok(())
///     })
///     .unwrap();
/// ```
pub fn a1_case() -> impl Strategy<Value = A1Case> {
    a1_domain().prop_flat_map(|(base, from, until)| {
        let span = until.minutes() - from.minutes();
        (
            zone_name(),
            zone_name(),
            proptest::collection::vec(a1_part_tag(), 0..=1),
            a1_duration(),
            a1_grid_or_candidates(from, span),
            a1_travel(),
            period_plan(),
            a1_calendar(from, span),
            a1_rules(base),
            a1_now(from),
            (0u8..4, any::<u64>(), any::<bool>()),
        )
            .prop_map(
                move |(
                    home,
                    event,
                    part_tags,
                    duration,
                    (grid, candidates),
                    travel,
                    periods,
                    calendar,
                    rules,
                    now,
                    (exclude_mode, exclude_mask, exclude_dup),
                )| {
                    let available = collect_event_keys([calendar.as_deref()], &[]);
                    let exclude_event_keys =
                        select_excluded_keys(&available, exclude_mode, exclude_mask, exclude_dup);
                    A1Case {
                        now,
                        search: (from, until),
                        display: event.clone(),
                        event,
                        duration,
                        grid,
                        candidates,
                        travel,
                        part_tags,
                        participant: ParticipantPlan {
                            id: "host".to_owned(),
                            zones: ZonePlan {
                                home,
                                periods: resolve_periods(periods, from),
                            },
                            calendar,
                        },
                        rules,
                        exclude_event_keys,
                    }
                },
            )
    })
}

/// The anchor date and the UTC range a generated case is read over: one day
/// before the anchor, offset by an arbitrary minute, for two to five days.
fn a1_domain() -> impl Strategy<Value = (Date, Instant, Instant)> {
    (
        1975i16..=2099,
        0..A1_BASE_DATES.len(),
        0u32..1_440,
        2u32..=5,
    )
        .prop_map(|(year, base, offset, days)| {
            let (month, day) = A1_BASE_DATES[base];
            let base_date = Date::new(year, month, day).expect("an anchor is a real date");
            let LocalResolution::Unique(midnight) = Zone::utc()
                .resolve_local(base_date, TimeOfDay::MIDNIGHT)
                .expect("UTC has no transitions")
            else {
                panic!("UTC has no transitions");
            };
            let from = Instant::from_minutes(midnight.minutes() - 1_440 + i64::from(offset))
                .expect("the range is supported");
            let until = Instant::from_minutes(from.minutes() + i64::from(days) * 1_440)
                .expect("the range is supported");
            (base_date, from, until)
        })
}

fn a1_now(from: Instant) -> impl Strategy<Value = Instant> {
    (0u32..=1_440).prop_map(move |back| {
        Instant::from_minutes(
            from.minutes()
                .saturating_sub(i64::from(back))
                .max(Instant::MIN.minutes()),
        )
        .expect("the range is supported")
    })
}

fn a1_part_tag() -> impl Strategy<Value = String> {
    proptest::sample::select(&[
        "interview",
        "interview/front-end",
        "sales/demo",
        "team/sales",
    ])
    .prop_map(str::to_owned)
}

fn a1_duration() -> impl Strategy<Value = u32> {
    proptest::sample::select(&[15u32, 25, 30, 45, 60, 90, 120])
}

fn a1_travel() -> impl Strategy<Value = TravelPlan> {
    (
        proptest::sample::select(&[0u32, 10, 20, 25, 35, 45, 60]),
        proptest::sample::select(&[0u32, 10, 20, 25, 35, 45, 60]),
    )
        .prop_map(|(before, after)| TravelPlan { before, after })
}

/// A grid, supplied candidates, or the engine's default grid. Candidate offsets
/// are minute-aligned instants inside the search window.
fn a1_grid_or_candidates(
    from: Instant,
    span: i64,
) -> impl Strategy<Value = (Option<GridPlan>, Option<Vec<Instant>>)> {
    let last = u32::try_from(span - 1).expect("a generated window is shorter than u32::MAX");
    prop_oneof![
        2 => Just((None, None)),
        4 => (
            proptest::sample::select(&[15u32, 25, 30, 45, 60]),
            proptest::sample::select(&[0u16, 7, 15, 30, 60, 480]),
            a1_grid_zone(),
        )
            .prop_map(|(step, anchor, zone)| {
                (Some(GridPlan { step, anchor, zone }), None)
            }),
        3 => proptest::collection::vec(0u32..=last, 1..=20).prop_map(move |offsets| {
            let mut candidates: Vec<Instant> = offsets
                .into_iter()
                .map(|offset| {
                    Instant::from_minutes(from.minutes() + i64::from(offset))
                        .expect("a candidate is inside the supported range")
                })
                .collect();
            candidates.sort_unstable();
            candidates.dedup();
            (None, Some(candidates))
        }),
    ]
}

fn a1_grid_zone() -> impl Strategy<Value = GridZonePlan> {
    prop_oneof![
        Just(GridZonePlan::Event),
        zone_name().prop_map(GridZonePlan::Named),
    ]
}

fn a1_calendar(from: Instant, span: i64) -> impl Strategy<Value = Option<Vec<BlockPlan>>> {
    prop_oneof![
        1 => Just(None),
        4 => proptest::collection::vec(a1_block(from, span), 1..=4).prop_map(Some),
    ]
}

fn a1_block(from: Instant, span: i64) -> impl Strategy<Value = BlockPlan> {
    // A block may start up to a day either side of the search window. A cap
    // buckets an existing event by its own start in the zone in force there, so
    // an event just outside the window can still share the window's bucket — and
    // reading it in another zone makes that the case that tells a hull computed
    // over the window's zones apart from one computed over the bucket's.
    let last =
        i64::from(u32::try_from(span - 1).expect("a generated window is shorter than u32::MAX"))
            + 1_440;
    (
        -1_440i64..=last,
        5u32..=180,
        a1_status(),
        any::<bool>(),
        proptest::collection::vec(a1_block_tag(), 0..=2),
        proptest::option::of("[a-z]{1,8}"),
        0u32..=60,
        0u32..=60,
    )
        .prop_map(
            move |(offset, length, status, all_day, tags, event_key, before, after)| {
                let start = Instant::from_minutes(from.minutes() + offset)
                    .expect("a block is inside the supported range");
                let end = Instant::from_minutes(start.minutes() + i64::from(length))
                    .expect("a block is inside the supported range");
                BlockPlan {
                    span: (start, end),
                    status,
                    all_day,
                    tags,
                    event_key,
                    travel_before: before,
                    travel_after: after,
                }
            },
        )
}

fn a1_status() -> impl Strategy<Value = BlockStatus> {
    proptest::sample::select(&[BlockStatus::Free, BlockStatus::Tentative, BlockStatus::Busy])
}

fn a1_block_tag() -> impl Strategy<Value = String> {
    proptest::sample::select(&["status/focus", "status/oof", "veto/crm-freeze", "interview"])
        .prop_map(str::to_owned)
}

/// Up to four ordinary rules and at most one status policy, inserted at a random
/// position. One policy keeps a full reversal of the list a difference in
/// nothing but rule order, which the reordering property relies on.
fn a1_rules(base: Date) -> impl Strategy<Value = Vec<RulePlan>> {
    (
        proptest::collection::vec(a1_ordinary_rule(base), 0..=4),
        proptest::option::of(a1_status_policy()),
        0usize..=4,
    )
        .prop_map(move |(mut rules, policy, at)| {
            if let Some(policy) = policy {
                let at = at.min(rules.len());
                rules.insert(at, policy);
            }
            for (index, rule) in rules.iter_mut().enumerate() {
                rule.set_id(format!("r{index}"));
            }
            rules
        })
}

fn a1_ordinary_rule(base: Date) -> impl Strategy<Value = RulePlan> {
    prop_oneof![
        3 => (
            a1_rule_zone(),
            weekly_keys(),
            date_overrides(),
            a1_applies(),
            a1_applies(),
        )
            .prop_map(move |(zone, weekly, dates, applies_to_host, applies_to_part)| {
                RulePlan::Window(WindowPlan {
                    id: String::new(),
                    zone,
                    weekly,
                    dates: date_keys(&dates, base),
                    applies_to_host,
                    applies_to_part,
                })
            }),
        2 => (
            a1_rule_zone(),
            proptest::collection::vec(span_plan(), 0..=3),
            a1_applies(),
            a1_applies(),
        )
            .prop_map(move |(zone, spans, applies_to_host, applies_to_part)| {
                RulePlan::Blackout(BlackoutPlan {
                    id: String::new(),
                    zone,
                    spans: spans
                        .into_iter()
                        .map(|(start_day, start, end_day, end)| {
                            ((day_after(base, start_day), start), (day_after(base, end_day), end))
                        })
                        .collect(),
                    applies_to_host,
                    applies_to_part,
                })
            }),
        2 => (
            0u32..=60,
            0u32..=60,
            0u32..=15,
            a1_applies(),
            a1_applies(),
        )
            .prop_map(
                |(before, after, min_gap, applies_to_host, applies_to_part)| RulePlan::Clearance {
                    id: String::new(),
                    clearance: ClearancePlan {
                        before,
                        after,
                        min_gap,
                        applies_to_host,
                        applies_to_part,
                    },
                },
            ),
        3 => (
            0u32..=240,
            a1_lead_max(base),
            a1_applies(),
            a1_applies(),
        )
            .prop_map(|(min, max, applies_to_host, applies_to_part)| {
                RulePlan::LeadTime(LeadTimePlan {
                    id: String::new(),
                    min,
                    max,
                    applies_to_host,
                    applies_to_part,
                })
            }),
        1 => (
            // A late evening and the few hours it protects: narrow enough that a
            // generated rule excludes something without swamping the corpus.
            20u16 * 60..=23 * 60,
            4u16 * 60..=7 * 60,
            a1_applies(),
            a1_applies(),
        )
            .prop_map(|(late_after, not_before, applies_to_host, applies_to_part)| {
                RulePlan::Rest(RestPlan {
                    id: String::new(),
                    late_after,
                    not_before,
                    applies_to_host,
                    applies_to_part,
                })
            }),
        2 => a1_cap_rule(),
    ]
}

/// A generated `cap` rule.
///
/// The limit is small, and a measured cap's tighter: a workload cap that never
/// bites would leave its evaluation unexercised. A pooled cap draws its zone from
/// `event` or a named zone, because a pool has no single participant zone and
/// `validate` rejects that combination.
fn a1_cap_rule() -> impl Strategy<Value = RulePlan> {
    let tail = || {
        (
            proptest::sample::select(&[CapPeriod::Day, CapPeriod::Week, CapPeriod::Month]),
            any::<bool>(),
            a1_rule_zone(),
            proptest::sample::select(&[
                opportune::time::Weekday::Monday,
                opportune::time::Weekday::Tuesday,
                opportune::time::Weekday::Wednesday,
                opportune::time::Weekday::Thursday,
                opportune::time::Weekday::Friday,
                opportune::time::Weekday::Saturday,
                opportune::time::Weekday::Sunday,
            ]),
            a1_applies(),
            a1_applies(),
        )
    };
    (
        proptest::collection::vec(a1_rule_tag(), 0..=1),
        proptest::sample::select(&[
            opportune::model::CapMeasure::Count,
            opportune::model::CapMeasure::Minutes,
        ]),
        tail(),
    )
        .prop_flat_map(
            |(
                count,
                measure,
                (period, pooled, zone, week_starts, applies_to_host, applies_to_part),
            )| {
                let limit = match measure {
                    opportune::model::CapMeasure::Count => 0u32..=3,
                    opportune::model::CapMeasure::Minutes => 0u32..=180,
                };
                limit.prop_map(move |limit| {
                    let per = if pooled {
                        opportune::model::CapPer::Pooled
                    } else {
                        opportune::model::CapPer::Each
                    };
                    let zone = if pooled && zone == RuleZonePlan::Participant {
                        RuleZonePlan::Event
                    } else {
                        zone.clone()
                    };
                    RulePlan::Cap(CapPlan {
                        id: String::new(),
                        count: count.clone(),
                        measure,
                        limit,
                        period,
                        per,
                        zone,
                        week_starts,
                        applies_to_host,
                        applies_to_part,
                    })
                })
            },
        )
}

fn a1_lead_max(base: Date) -> impl Strategy<Value = Option<LeadMaxPlan>> {
    prop_oneof![
        Just(None),
        (30u32..=1_440).prop_map(|minutes| Some(LeadMaxPlan::Duration(minutes))),
        (0u32..=3).prop_map(|days| Some(LeadMaxPlan::Days(days))),
        (0u32..=30).prop_map(move |days| {
            Some(LeadMaxPlan::Date(
                base.plus_days(i64::from(days))
                    .expect("a nearby date exists"),
            ))
        }),
    ]
}

fn a1_status_policy() -> impl Strategy<Value = RulePlan> {
    (
        proptest::collection::vec(a1_status_entry(), 1..=3),
        a1_applies(),
        a1_applies(),
    )
        .prop_map(|(entries, applies_to_host, applies_to_part)| {
            RulePlan::StatusPolicy(StatusPolicyPlan {
                id: String::new(),
                entries,
                applies_to_host,
                applies_to_part,
            })
        })
}

fn a1_status_entry() -> impl Strategy<Value = StatusEntryPlan> {
    (
        proptest::option::of(a1_status()),
        proptest::option::of(a1_rule_tag()),
        proptest::sample::select(&[StatusEffect::Ignore, StatusEffect::Block]),
    )
        .prop_map(|(status, tag, effect)| {
            // An entry matching neither could never apply, which the engine
            // rejects, so the generator defaults it to matching every busy
            // block instead.
            let status = if status.is_none() && tag.is_none() {
                Some(BlockStatus::Busy)
            } else {
                status
            };
            StatusEntryPlan {
                status,
                tag,
                effect,
            }
        })
}

fn a1_rule_tag() -> impl Strategy<Value = String> {
    proptest::sample::select(&["status/focus", "status/oof", "interview"]).prop_map(str::to_owned)
}

fn a1_rule_zone() -> impl Strategy<Value = RuleZonePlan> {
    prop_oneof![
        Just(RuleZonePlan::Participant),
        Just(RuleZonePlan::Event),
        zone_name().prop_map(RuleZonePlan::Named),
    ]
}

fn a1_applies() -> impl Strategy<Value = bool> {
    prop_oneof![4 => Just(true), 1 => Just(false)]
}

/// One generated matching problem: per term, how many distinct hosts it needs
/// and the ID and weight of each candidate.
pub type MatchingCase = Vec<(usize, Vec<(String, i32)>)>;

/// A weight from the whole `i32` range: mostly small values and ties, with both
/// bounds included so weight sums are checked at the extremes.
pub fn matching_weight() -> impl Strategy<Value = i32> {
    prop_oneof![
        6 => -2_i32..=2,
        1 => Just(0),
        1 => Just(i32::MIN),
        1 => Just(i32::MAX),
    ]
}

/// A generated matching problem: one to four terms, each needing one or two
/// distinct hosts from a pool of up to six.
///
/// The masks leave many instances infeasible — individually satisfiable terms
/// whose pools cannot supply enough distinct hosts together — and the weights tie
/// heavily so the term-ordered lexicographic tie-break is exercised, not only the
/// total weight.
pub fn matching_case() -> impl Strategy<Value = MatchingCase> {
    (1..=6_usize)
        .prop_flat_map(|host_count| {
            let weights = proptest::collection::vec(matching_weight(), host_count);
            let terms = proptest::collection::vec(
                (1..=2_usize).prop_flat_map(move |count| {
                    proptest::collection::vec(any::<bool>(), host_count)
                        .prop_map(move |mask| (count, mask))
                }),
                1..=4,
            );
            (weights, terms)
        })
        .prop_map(|(weights, terms)| matching_plan(&weights, &terms, 1))
}

/// A generated matching problem whose candidate lists run past the total number
/// of slots: at most two terms needing one host each, drawn from three to eight
/// candidates.
///
/// Top-`S` pruning drops lower-weighted and higher-ID candidates here, so agreeing
/// with the brute-force oracle proves pruning keeps both the best total weight and
/// the exact tie-break.
pub fn matching_pruning_case() -> impl Strategy<Value = MatchingCase> {
    (1..=2_usize)
        .prop_flat_map(|term_count| {
            (3..=8_usize).prop_flat_map(move |host_count| {
                let weights = proptest::collection::vec(matching_weight(), host_count);
                let terms = proptest::collection::vec(
                    proptest::collection::vec(any::<bool>(), host_count).prop_map(|mask| (1, mask)),
                    term_count,
                );
                (weights, terms)
            })
        })
        .prop_map(|(weights, terms)| matching_plan(&weights, &terms, 3))
}

/// Turns generated weights and per-term `(count, mask)` choices into a case,
/// giving every term at least `minimum` candidates whatever the mask says.
fn matching_plan(weights: &[i32], terms: &[(usize, Vec<bool>)], minimum: usize) -> MatchingCase {
    terms
        .iter()
        .map(|(count, mask)| {
            let candidates = mask
                .iter()
                .enumerate()
                .filter(|(index, included)| **included || *index < minimum)
                .map(|(index, _)| (format!("h{index}"), weights[index]))
                .collect();
            (*count, candidates)
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Whole-request (A2) cases
//
// A generated multi-participant case: one to three hosts and up to one attendee,
// a one-to-three-term requirement, per-participant weights, and rules whose
// `who` selects an arbitrary subset. Attendees in the mask exercise rules that
// only reach an attendee, and overlapping pools exercise the joint-fillability
// and term-ordered tie-breaking paths.
// ---------------------------------------------------------------------------

/// The participant IDs generated cases draw from. They sort differently from the
/// order they are emitted in, so eligible and assigned lists are exercised
/// against request order.
const A2_IDS: [&str; 4] = ["ana", "bo", "cy", "zed"];

/// The participant tags a generated requirement selector matches. Hierarchical,
/// so `team/sales` also matches a participant tagged `team/sales/emea`.
const A2_TAGS: [&str; 4] = ["team/sales", "pool/backend", "kind/room", "level/senior"];

/// A permutation of `0..A2_IDS.len()`, so participants are emitted in an order
/// unrelated to their IDs.
fn permutation() -> impl Strategy<Value = Vec<usize>> {
    proptest::collection::vec(any::<u8>(), A2_IDS.len()).prop_map(|keys| {
        let mut order: Vec<usize> = (0..A2_IDS.len()).collect();
        order.sort_by_key(|index| (keys[*index], *index));
        order
    })
}

/// A participant tag from the generated vocabulary.
fn a2_tag() -> impl Strategy<Value = String> {
    proptest::sample::select(&A2_TAGS).prop_map(str::to_owned)
}

/// One generated multi-participant request.
///
/// ```
/// use opportune_testkit::a2::A2Oracle;
/// use opportune_testkit::generators::a2_case;
/// use opportune::time::TimeZoneDatabase;
/// use proptest::prelude::*;
/// use proptest::test_runner::{Config, TestRunner};
///
/// let db = TimeZoneDatabase::bundled().unwrap();
/// TestRunner::new(Config::with_cases(4))
///     .run(&a2_case(), |case| {
///         prop_assert!(!case.participants.is_empty());
///         prop_assert!(!case.terms.is_empty());
///         let oracle = A2Oracle::new(&case, &db);
///         prop_assert!(!oracle.candidate_starts().is_empty());
///         Ok(())
///     })
///     .unwrap();
/// ```
pub fn a2_case() -> impl Strategy<Value = A2Case> {
    a1_domain().prop_flat_map(|(base, from, until)| {
        let span = until.minutes() - from.minutes();
        a2_participants(from, span).prop_flat_map(move |participants| {
            let ids: Vec<String> = participants.iter().map(|item| item.id.clone()).collect();
            let count = participants.len();
            (
                a2_terms(ids.clone()),
                a2_occurrences(from, span, ids),
                a2_rules(base, count),
                proptest::collection::vec(a1_part_tag(), 0..=1),
                a1_duration(),
                a1_grid_or_candidates(from, span),
                a1_travel(),
                a1_now(from),
                zone_name(),
                (0u8..4, any::<u64>(), any::<bool>()),
            )
                .prop_map(
                    move |(
                        terms,
                        occurrences,
                        rules,
                        part_tags,
                        duration,
                        (grid, candidates),
                        travel,
                        now,
                        event,
                        (exclude_mode, exclude_mask, exclude_dup),
                    )| {
                        let available = collect_event_keys(
                            participants
                                .iter()
                                .map(|participant| participant.calendar.as_deref()),
                            &occurrences,
                        );
                        let exclude_event_keys = select_excluded_keys(
                            &available,
                            exclude_mode,
                            exclude_mask,
                            exclude_dup,
                        );
                        A2Case {
                            now,
                            search: (from, until),
                            display: event.clone(),
                            event,
                            duration,
                            grid,
                            candidates,
                            travel,
                            part_tags,
                            participants: participants.clone(),
                            terms,
                            rules,
                            occurrences,
                            exclude_event_keys,
                        }
                    },
                )
        })
    })
}

/// Up to three identifiable events inside the search window, each naming a
/// non-empty subset of the generated participants.
fn a2_occurrences(
    from: Instant,
    span: i64,
    ids: Vec<String>,
) -> impl Strategy<Value = Vec<OccurrencePlan>> {
    // Like a block, an occurrence may start up to a day either side of the
    // window: a cap counts it by its own start's bucket, which the window's own
    // instants need not cover.
    let last =
        i64::from(u32::try_from(span - 1).expect("a generated window is shorter than u32::MAX"))
            + 1_440;
    let participants = ids.len();
    proptest::collection::vec(
        (
            -1_440i64..=last,
            5u32..=180,
            proptest::collection::vec(a1_block_tag(), 0..=2),
            proptest::collection::vec(any::<bool>(), participants..=participants),
        ),
        0..=3,
    )
    .prop_map(move |raw| {
        raw.into_iter()
            .enumerate()
            .map(|(index, (offset, length, tags, mask))| {
                let start = Instant::from_minutes(from.minutes() + offset)
                    .expect("an occurrence is inside the supported range");
                let end = Instant::from_minutes(start.minutes() + i64::from(length))
                    .expect("an occurrence is inside the supported range");
                let mut named: Vec<String> = mask
                    .iter()
                    .zip(&ids)
                    .filter(|(named, _)| **named)
                    .map(|(_, id)| id.clone())
                    .collect();
                if named.is_empty()
                    && let Some(first) = ids.first()
                {
                    named.push(first.clone());
                }
                OccurrencePlan {
                    event_key: format!("occ{index}"),
                    span: (start, end),
                    tags,
                    participants: named,
                }
            })
            .collect()
    })
}

/// One to three hosts and up to one attendee, with zones, calendars, tags and
/// weights.
fn a2_participants(from: Instant, span: i64) -> impl Strategy<Value = Vec<A2Participant>> {
    (1usize..=3, 0usize..=1, permutation()).prop_flat_map(move |(hosts, attendees, order)| {
        let total = hosts + attendees;
        let attributes = proptest::collection::vec(
            (
                zone_name(),
                period_plan(),
                a1_calendar(from, span),
                proptest::collection::vec(a2_tag(), 0..=2),
                matching_weight(),
            ),
            total..=total,
        );
        attributes.prop_map(move |attributes| {
            attributes
                .into_iter()
                .enumerate()
                .map(
                    |(position, (home, periods, calendar, tags, weight))| A2Participant {
                        id: A2_IDS[order[position % A2_IDS.len()]].to_owned(),
                        role: if position < hosts {
                            Role::Host
                        } else {
                            Role::Attendee
                        },
                        zones: ZonePlan {
                            home,
                            periods: resolve_periods(periods, from),
                        },
                        calendar,
                        tags,
                        weight,
                    },
                )
                .collect()
        })
    })
}

/// One to three requirement terms over the generated participants.
fn a2_terms(ids: Vec<String>) -> impl Strategy<Value = Vec<TermPlan>> {
    proptest::collection::vec(
        prop_oneof![
            3 => proptest::sample::select(ids).prop_map(TermPlan::Person),
            4 => (
                1u32..=2,
                proptest::collection::vec(a2_tag(), 1..=2),
            )
                .prop_map(|(count, tags)| TermPlan::AtLeast { count, tags }),
        ],
        1..=3,
    )
}

/// Up to three ordinary rules plus at most one status policy, each naming an
/// arbitrary subset of the participants in `who`.
fn a2_rules(base: Date, participants: usize) -> impl Strategy<Value = Vec<A2Rule>> {
    let mask = move || proptest::collection::vec(a1_applies(), participants..=participants);
    (
        proptest::collection::vec((a1_ordinary_rule(base), mask(), a1_applies()), 0..=3),
        proptest::option::of((a1_status_policy(), mask(), a1_applies())),
    )
        .prop_map(|(rules, policy)| {
            let mut built: Vec<A2Rule> = rules
                .into_iter()
                .map(|(plan, who, applies_to_part)| A2Rule {
                    plan,
                    who,
                    applies_to_part,
                })
                .collect();
            if let Some((plan, who, applies_to_part)) = policy {
                built.push(A2Rule {
                    plan,
                    who,
                    applies_to_part,
                });
            }
            for (index, rule) in built.iter_mut().enumerate() {
                rule.plan.set_id(format!("r{index}"));
            }
            built
        })
}
