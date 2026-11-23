//! Regression tests for performance, memory-amplification, and DoS-bound fixes.
//!
//! Each test exercises a shape that previously triggered quadratic/multiplicative
//! work or memory amplification and verifies both bounded execution and exact
//! specification parity across `validate`, `find_availability`, `check_slot`,
//! and `required_ranges`.

#![cfg(feature = "bundled-tzdb")]

use opportune::Engine;
use opportune::model::{
    Block, CalendarSpec, Cap, CapMeasure, CapPer, CapPeriod, Clearance, Explain, Goal, LeadTime,
    LeadTimeMax, Occurrence, Offer, Participant, Proposal, RangesQuery, Request, Requirement, Rest,
    Rule, RuleKind, Span, Term, Travel, ZonePeriod,
};
use opportune::presets;
use opportune::reasons::ReasonCode;
use opportune::tags::{Role, Selector, Tag};
use opportune::time::{
    BlackoutSpan, BlackoutSpec, Date, Duration, Grid, GridZone, Horizon, Instant, LocalDateTime,
    LocalResolution, RuleZone, TimeOfDay, TimeZoneDatabase, WallEnd, WallRange, Weekday, Weekdays,
    WeeklyKey, WindowSpec, WorkingCalendar, Zone, ZoneName, ZonePeriod as TimeZonePeriod,
    ZoneSchedule,
};

fn engine() -> Engine {
    Engine::builder()
        .build()
        .expect("the bundled database builds")
}

fn zone_name(name: &str) -> ZoneName {
    ZoneName::new(name).expect("fixture zone name is valid")
}

fn date(year: i16, month: i8, day: i8) -> Date {
    Date::new(year, month, day).expect("fixture date is valid")
}

fn time(hour: u8, minute: u8) -> TimeOfDay {
    TimeOfDay::new(hour, minute).expect("fixture time is valid")
}

fn utc(year: i16, month: i8, day: i8, hour: u8, minute: u8) -> Instant {
    let LocalResolution::Unique(instant) = Zone::utc()
        .resolve_local(date(year, month, day), time(hour, minute))
        .expect("fixture instant is in range")
    else {
        panic!("UTC has no transitions");
    };
    instant
}

fn span(start: Instant, end: Instant) -> Span {
    Span::new(start, end).expect("fixture span is non-empty")
}

/// An over-limit 125-year search window driven by a daily grid stays below
/// `max_candidate_starts` (45,656 <= 100,000). `validate()` must report
/// `search_window_limit` and skip `warn_if_no_candidate_fits_window` instead of
/// building windows across 125 years × participants × rules.
#[test]
fn over_limit_grid_search_window_skips_window_warning_and_stays_bounded() {
    let engine = engine();
    let mut goal = Goal::single(
        zone_name("UTC"),
        span(Instant::MIN, Instant::MAX),
        Duration::from_minutes(30),
        Requirement::new(vec![Term::at_least(1, Selector::role(Role::Host))]),
    );
    goal.grid = Some(
        Grid::new(
            Duration::from_minutes(1_440),
            TimeOfDay::MIDNIGHT,
            GridZone::Event,
        )
        .expect("daily grid is valid"),
    );

    let mut request = Request::new(Instant::MIN, goal);
    request.participants = (0..100)
        .map(|i| Participant::new(format!("h{i}"), zone_name("UTC")).with_calendar(Vec::new()))
        .collect();
    let sunday_only = presets::business_hours(
        RuleZone::Event,
        Weekdays::single(Weekday::Sunday),
        time(9, 0),
        WallEnd::Time(time(17, 0)),
    )
    .expect("valid window")
    .window()
    .expect("window rule")
    .clone();
    request.rules = (0..100)
        .map(|i| Rule::new(format!("w{i}"), RuleKind::Window(sunday_only.clone())))
        .collect();

    let diagnostics = engine.validate(&request);
    assert!(
        diagnostics
            .errors
            .iter()
            .any(|d| d.code() == "search_window_limit"),
        "over-cap search window must still report search_window_limit: {:?}",
        diagnostics.errors
    );
    assert!(
        !diagnostics
            .warnings
            .iter()
            .any(|d| d.code() == "no_candidate_in_window"),
        "over-cap grid search must skip window building: {:?}",
        diagnostics.warnings
    );
}

/// An over-cap search window with many explicit candidates spread across
/// decades whose grouped envelopes exceed `envelope_limit` skips window
/// building rather than running hundreds of envelopes × participants × rules.
#[test]
fn over_limit_wide_explicit_candidates_bound_window_warning_work() {
    let engine = engine();
    let mut goal = Goal::single(
        zone_name("UTC"),
        span(Instant::MIN, Instant::MAX),
        Duration::from_minutes(30),
        Requirement::new(vec![Term::person("ana")]),
    );
    // 200 candidates spaced 200 days apart: 200 distinct groups > 113-day
    // envelope budget.
    let step = 200 * 1_440_i64;
    goal.candidates = Some(
        (0..200_i64)
            .map(|i| Instant::from_minutes(Instant::MIN.minutes() + i * step).unwrap())
            .collect(),
    );
    let mut request = Request::new(Instant::MIN, goal);
    request.participants =
        vec![Participant::new("ana", zone_name("UTC")).with_calendar(Vec::new())];
    request.rules = vec![
        presets::business_hours(
            RuleZone::Event,
            Weekdays::single(Weekday::Sunday),
            time(9, 0),
            WallEnd::Time(time(17, 0)),
        )
        .expect("valid window"),
    ];

    let diagnostics = engine.validate(&request);
    assert!(
        diagnostics
            .errors
            .iter()
            .any(|d| d.code() == "search_window_limit")
    );
    assert!(
        !diagnostics
            .warnings
            .iter()
            .any(|d| d.code() == "no_candidate_in_window")
    );
}

/// Precomputed `rest_self_blocked` must agree exactly with `failing_reasons` /
/// `check_slot` when `late_after <= not_before` (same-day overlap) and across
/// DST transitions (spring-forward and fall-back).
#[test]
fn rest_self_conflict_precomputation_matches_exact_semantics_including_overlap_day_and_dst() {
    let engine = engine();

    // 1. Overlapping-day rest rule (`late_after = 08:00 <= not_before = 18:00`):
    //    - A daytime 30m booking at 10:00 touches early(D) and late(D) on the
    //      SAME day D, so it must be OFFERED (not self-conflicting).
    //    - A midnight-crossing booking at 23:45 touches late(D) and early(D+1),
    //      so it must be EXCLUDED citing the earliest rest rule.
    let window = span(utc(2027, 3, 10, 0, 0), utc(2027, 3, 12, 0, 0));
    let daytime = utc(2027, 3, 10, 10, 0);
    let touches_midnight_end = utc(2027, 3, 10, 23, 30); // ends at 00:00 (not early on D+1)
    let crosses_midnight = utc(2027, 3, 10, 23, 45); // [23:45, 00:15) -> late(10) + early(11)

    let mut goal = Goal::single(
        zone_name("UTC"),
        window,
        Duration::from_minutes(30),
        Requirement::new(vec![Term::person("ana")]),
    );
    goal.candidates = Some(vec![daytime, touches_midnight_end, crosses_midnight]);
    let mut request = Request::new(utc(2027, 3, 9, 0, 0), goal);
    request.participants =
        vec![Participant::new("ana", zone_name("UTC")).with_calendar(Vec::new())];
    request.rules = vec![
        Rule::new(
            "overlap_first",
            RuleKind::Rest(Rest::new(time(8, 0), time(18, 0))),
        ),
        Rule::new(
            "overlap_second",
            RuleKind::Rest(Rest::new(time(9, 0), time(17, 0))),
        ),
    ];
    request.explain = Some(Explain::new(100));

    let response = engine.find_availability(&request).expect("search succeeds");
    let offered: Vec<Instant> = response.offers.iter().map(|o| o.start().unwrap()).collect();
    assert_eq!(offered, vec![daytime, touches_midnight_end]);
    assert_eq!(response.excluded.len(), 1);
    assert_eq!(response.excluded[0].start, crosses_midnight);
    assert_eq!(response.excluded[0].reasons.len(), 1);
    assert_eq!(response.excluded[0].reasons[0].code, ReasonCode::Rest);
    assert_eq!(
        response.excluded[0].reasons[0].rule.as_deref(),
        Some("overlap_first")
    );

    // `check_slot` must agree on every candidate.
    for &(start, expected_valid) in &[
        (daytime, true),
        (touches_midnight_end, true),
        (crosses_midnight, false),
    ] {
        let checked = engine
            .check_slot(
                &request,
                &Proposal::single("main", start, vec![vec!["ana".to_owned()]]),
            )
            .expect("check_slot succeeds");
        assert_eq!(checked.valid, expected_valid, "disagreement at {start}");
    }

    // 2. Across America/Santiago midnight spring-forward (2027-09-05 00:00 -> 01:00):
    //    2027-09-04 is UTC-4; transition at 2027-09-05T04:00Z jumps from
    //    2027-09-04 23:59 to 2027-09-05 01:00. With `late_after = 22:00`,
    //    `not_before = 08:00`, a 30m booking at 03:45Z spans 23:45 (Sep 4) to
    //    01:15 (Sep 5) and must self-conflict, while 03:30Z ends at 04:00Z
    //    (entirely on Sep 4) and must be offered.
    let santiago_window = span(utc(2027, 9, 5, 0, 0), utc(2027, 9, 5, 12, 0));
    let before_jump = utc(2027, 9, 5, 3, 30);
    let across_jump = utc(2027, 9, 5, 3, 45);
    let after_jump = utc(2027, 9, 5, 4, 0);
    let mut dst_goal = Goal::single(
        zone_name("America/Santiago"),
        santiago_window,
        Duration::from_minutes(30),
        Requirement::new(vec![Term::person("ana")]),
    );
    dst_goal.candidates = Some(vec![before_jump, across_jump, after_jump]);
    let mut dst_req = Request::new(utc(2027, 9, 1, 0, 0), dst_goal);
    dst_req.participants =
        vec![Participant::new("ana", zone_name("America/Santiago")).with_calendar(Vec::new())];
    dst_req.rules = vec![Rule::new(
        "night_rest",
        RuleKind::Rest(Rest::new(time(22, 0), time(8, 0))),
    )];
    dst_req.explain = Some(Explain::new(100));

    let dst_resp = engine
        .find_availability(&dst_req)
        .expect("DST search succeeds");
    let dst_offered: Vec<Instant> = dst_resp.offers.iter().map(|o| o.start().unwrap()).collect();
    assert_eq!(dst_offered, vec![before_jump, after_jump]);
    assert_eq!(dst_resp.excluded.len(), 1);
    assert_eq!(dst_resp.excluded[0].start, across_jump);
    assert_eq!(dst_resp.excluded[0].reasons[0].code, ReasonCode::Rest);
    for &(start, expected_valid) in &[
        (before_jump, true),
        (across_jump, false),
        (after_jump, true),
    ] {
        let checked = engine
            .check_slot(
                &dst_req,
                &Proposal::single("main", start, vec![vec!["ana".to_owned()]]),
            )
            .expect("check_slot succeeds");
        assert_eq!(checked.valid, expected_valid, "DST disagreement at {start}");
    }
}

/// `WindowSpec::intervals` slice narrowing and `BlackoutSpec::intervals`
/// `partition_point` must agree minute-by-minute with `contains` on dense
/// multi-range schedules, midnight-crossing spills, many short zone periods in
/// a single day, and DST transitions.
#[test]
fn window_and_blackout_breakpoint_optimisations_agree_minute_by_minute() {
    let db = TimeZoneDatabase::bundled().expect("bundled tzdb loads");
    let ny = db.zone("America/New_York").expect("New York exists");
    let tokyo = db.zone("Asia/Tokyo").expect("Tokyo exists");

    // Dense 1-minute-on / 1-minute-off ranges plus a midnight-crossing spill on
    // Tuesday (`23:30-01:15`).
    let mut monday_ranges = Vec::new();
    for m in (0..1_380_u16).step_by(4) {
        let s = TimeOfDay::from_minute_of_day(m).unwrap();
        let e = TimeOfDay::from_minute_of_day(m + 2).unwrap();
        monday_ranges.push(WallRange::new(s, WallEnd::Time(e)).unwrap());
    }
    monday_ranges.push(WallRange::new(time(23, 30), WallEnd::Time(time(1, 15))).unwrap());

    let window = WindowSpec::new(
        RuleZone::Participant,
        vec![
            WeeklyKey::new(
                Weekdays::list([Weekday::Sunday, Weekday::Monday]),
                monday_ranges,
            )
            .unwrap(),
            WeeklyKey::new(
                Weekdays::list([Weekday::Tuesday, Weekday::Saturday]),
                vec![WallRange::new(time(9, 0), WallEnd::Time(time(17, 0))).unwrap()],
            )
            .unwrap(),
        ],
        Vec::new(),
    )
    .unwrap();

    // Schedule crossing America/New_York spring-forward (2027-03-14) with many
    // short Tokyo zone periods carved into the same local day.
    let from = utc(2027, 3, 13, 0, 0);
    let until = utc(2027, 3, 16, 0, 0);
    let mut periods = Vec::new();
    for k in 0..40_i64 {
        let p_from = Instant::from_minutes(from.minutes() + k * 90 + 15).unwrap();
        let p_until = Instant::from_minutes(p_from.minutes() + 35).unwrap();
        periods.push(TimeZonePeriod::new(tokyo.clone(), p_from, p_until).unwrap());
    }
    let schedule = ZoneSchedule::new(ny, periods).unwrap();

    let built_window = window.intervals(&schedule, from, until).unwrap();
    for minute in from.minutes()..until.minutes() {
        let instant = Instant::from_minutes(minute).unwrap();
        assert_eq!(
            built_window.contains_minute(minute),
            window.contains(instant, &schedule),
            "window mismatch at {instant}"
        );
    }

    // BlackoutSpec with thousands of historical spans before `from`, spans
    // inside `from..until`, and spans after `until`.
    let mut blackout_spans = Vec::new();
    let mut cur = date(2020, 1, 1);
    for _ in 0..2_500 {
        let start = LocalDateTime::new(cur, time(10, 0));
        let end = LocalDateTime::new(cur, time(11, 30));
        blackout_spans.push(BlackoutSpan::new(start, end).unwrap());
        cur = cur.plus_days(1).unwrap();
    }
    blackout_spans.push(
        BlackoutSpan::new(
            LocalDateTime::new(date(2027, 3, 13), time(22, 0)),
            LocalDateTime::new(date(2027, 3, 14), time(3, 30)),
        )
        .unwrap(),
    );
    blackout_spans.push(
        BlackoutSpan::new(
            LocalDateTime::new(date(2027, 3, 15), time(12, 0)),
            LocalDateTime::new(date(2027, 3, 15), time(14, 0)),
        )
        .unwrap(),
    );
    blackout_spans.push(
        BlackoutSpan::new(
            LocalDateTime::new(date(2030, 1, 1), time(0, 0)),
            LocalDateTime::new(date(2030, 1, 2), time(0, 0)),
        )
        .unwrap(),
    );
    let blackout = BlackoutSpec::new(RuleZone::Participant, blackout_spans);
    let built_blackout = blackout.intervals(&schedule, from, until).unwrap();
    for minute in from.minutes()..until.minutes() {
        let instant = Instant::from_minutes(minute).unwrap();
        assert_eq!(
            built_blackout.contains_minute(minute),
            blackout.contains(instant, &schedule),
            "blackout mismatch at {instant}"
        );
    }
}

/// Prefix-max binary search in `BlockedIndex` must still find an earlier long
/// enclosing block even when hundreds of shorter blocks sit between the long
/// block's start and the candidate start, and must still cite the earliest
/// clearance rule when two blocks' clearance gaps overlap a candidate.
#[test]
fn blocked_and_clearance_prefix_max_index_handles_nested_and_overlapping_blocks() {
    let engine = engine();
    let day_start = utc(2027, 3, 10, 0, 0);
    let day_end = utc(2027, 3, 11, 0, 0);

    // One long block covering 01:00..20:00 (with key "long_enclosing"), plus
    // 100 short 10-minute blocks nested inside 02:00..15:00, plus a late block
    // at 21:00..21:30 for clearance citation.
    let mut blocks = vec![
        Block::new(span(utc(2027, 3, 10, 1, 0), utc(2027, 3, 10, 20, 0)))
            .with_event_key("long_enclosing")
            .unwrap(),
    ];
    for i in 0..100_i64 {
        let s = Instant::from_minutes(utc(2027, 3, 10, 2, 0).minutes() + i * 7).unwrap();
        let e = Instant::from_minutes(s.minutes() + 5).unwrap();
        blocks.push(
            Block::new(span(s, e))
                .with_event_key(format!("short_{i:03}"))
                .unwrap(),
        );
    }
    blocks.push(
        Block::new(span(utc(2027, 3, 10, 21, 0), utc(2027, 3, 10, 21, 30)))
            .with_event_key("late_meeting")
            .unwrap(),
    );

    // Candidate 1 at 18:00..18:30: past every short block's end, still inside
    // `long_enclosing` (01:00..20:00) -> must cite `long_enclosing` as Blocked.
    let inside_long = utc(2027, 3, 10, 18, 0);
    // Candidate 2 at 20:10..20:40: clear of `long_enclosing` (ends 20:00) and
    // `late_meeting` (starts 21:00), but within `before=20m` of `long_enclosing`
    // (supplied by rule `c_after_then_before` at pos 1) AND within `after=30m`
    // of `late_meeting` (supplied by rule `c_earliest` at pos 0) -> must cite
    // `c_earliest` and `late_meeting`!
    let clearance_both = utc(2027, 3, 10, 20, 10);
    // Candidate 3 at 22:00..22:30: completely clear.
    let clear = utc(2027, 3, 10, 22, 0);

    let mut goal = Goal::single(
        zone_name("UTC"),
        span(day_start, day_end),
        Duration::from_minutes(30),
        Requirement::new(vec![Term::person("ana")]),
    );
    goal.candidates = Some(vec![inside_long, clearance_both, clear]);
    let mut request = Request::new(utc(2027, 3, 9, 0, 0), goal);
    request.participants = vec![Participant::new("ana", zone_name("UTC")).with_calendar(blocks)];
    request.rules = vec![
        Rule::new(
            "c_earliest",
            RuleKind::Clearance(Clearance {
                before: Duration::ZERO,
                after: Duration::from_minutes(30),
                min_gap: Duration::ZERO,
            }),
        ),
        Rule::new(
            "c_later",
            RuleKind::Clearance(Clearance {
                before: Duration::from_minutes(20),
                after: Duration::ZERO,
                min_gap: Duration::ZERO,
            }),
        ),
    ];
    request.explain = Some(Explain::new(100));

    let response = engine.find_availability(&request).expect("search succeeds");
    assert_eq!(response.offers.len(), 1);
    assert_eq!(response.offers[0].start(), Some(clear));
    assert_eq!(response.excluded.len(), 2);

    assert_eq!(response.excluded[0].start, inside_long);
    assert_eq!(response.excluded[0].reasons[0].code, ReasonCode::Blocked);
    assert_eq!(
        response.excluded[0].reasons[0].event_key.as_deref(),
        Some("long_enclosing")
    );

    assert_eq!(response.excluded[1].start, clearance_both);
    assert_eq!(response.excluded[1].reasons[0].code, ReasonCode::Clearance);
    assert_eq!(
        response.excluded[1].reasons[0].rule.as_deref(),
        Some("c_earliest")
    );
    assert_eq!(
        response.excluded[1].reasons[0].event_key.as_deref(),
        Some("late_meeting")
    );
}

/// Pooled caps with many historical occurrences across decades filter buckets
/// to the judged window/start and share usage cleanly across participants,
/// agreeing between `find_availability`, `check_slot`, and `required_ranges`.
#[test]
fn pooled_caps_filter_distant_buckets_and_agree_with_check_slot() {
    let engine = engine();
    let window = span(utc(2027, 3, 10, 9, 0), utc(2027, 3, 11, 17, 0));
    let full_day = utc(2027, 3, 10, 10, 0);
    let open_day = utc(2027, 3, 11, 10, 0);

    let mut goal = Goal::single(
        zone_name("UTC"),
        window,
        Duration::from_minutes(30),
        Requirement::new(vec![Term::at_least(1, Selector::role(Role::Host))]),
    );
    goal.candidates = Some(vec![full_day, open_day]);
    let mut request = Request::new(utc(2027, 3, 9, 0, 0), goal);
    request.participants = (0..50)
        .map(|i| Participant::new(format!("h{i}"), zone_name("UTC")).with_calendar(Vec::new()))
        .collect();
    request.rules = vec![Rule::new(
        "pool_daily",
        RuleKind::Cap(Cap::new(
            Selector::True,
            CapMeasure::Count,
            1,
            CapPeriod::Day,
            CapPer::Pooled,
            RuleZone::Event,
            Weekday::Monday,
        )),
    )];
    // 2,000 distant occurrences spanning years 2030..2035 plus 1 on 2027-03-10.
    let mut occurrences: Vec<Occurrence> = (0..2_000_i64)
        .map(|i| {
            let s = Instant::from_minutes(utc(2030, 1, 1, 9, 0).minutes() + i * 1_440).unwrap();
            let e = Instant::from_minutes(s.minutes() + 30).unwrap();
            Occurrence::new(format!("future_{i}"), span(s, e), vec!["h0".to_owned()]).unwrap()
        })
        .collect();
    occurrences.push(
        Occurrence::new(
            "today_evt",
            span(utc(2027, 3, 10, 14, 0), utc(2027, 3, 10, 14, 30)),
            vec!["h0".to_owned()],
        )
        .unwrap(),
    );
    request.occurrences = occurrences;
    request.explain = Some(Explain::new(100));

    let response = engine.find_availability(&request).expect("search succeeds");
    assert_eq!(response.offers.len(), 1);
    assert_eq!(response.offers[0].start(), Some(open_day));
    assert_eq!(response.excluded.len(), 1);
    assert_eq!(response.excluded[0].start, full_day);
    assert!(
        response.excluded[0]
            .reasons
            .iter()
            .all(|r| r.code == ReasonCode::CapReached)
    );

    // `check_slot` on a distant date in year 2030 (which has 1 occurrence) must
    // still see that occurrence because `check_slot` filters buckets around
    // `proposal.start`, not `goal.search_window`.
    let year_2030_full = utc(2030, 1, 1, 12, 0);
    let checked = engine
        .check_slot(
            &request,
            &Proposal::single("main", year_2030_full, vec![vec!["h1".to_owned()]]),
        )
        .expect("check_slot succeeds");
    assert!(!checked.valid);
    assert_eq!(checked.reasons.len(), 1);
    assert_eq!(checked.reasons[0].code, ReasonCode::CapReached);
}

/// A request for one named host at each of `starts` inside `window`, needing
/// `before` and `after` travel, with a night `rest` rule that protects 22:00-08:00.
fn night_rest_request(window: Span, starts: &[Instant], before: u32, after: u32) -> Request {
    let mut goal = Goal::single(
        zone_name("UTC"),
        window,
        Duration::from_minutes(30),
        Requirement::new(vec![Term::person("ana")]),
    );
    goal.candidates = Some(starts.to_vec());
    goal.parts[0].travel = Travel::new(
        Duration::from_minutes(before),
        Duration::from_minutes(after),
    );
    let mut request = Request::new(Instant::MIN, goal);
    request.participants =
        vec![Participant::new("ana", zone_name("UTC")).with_calendar(Vec::new())];
    request.rules = vec![Rule::new(
        "night_rest",
        RuleKind::Rest(Rest::new(time(22, 0), time(8, 0))),
    )];
    request.explain = Some(Explain::new(100));
    request
}

/// `rest_self_blocked` is keyed by `occupied_len`, so it is only exact while the
/// booking's occupied span is not clipped by the supported instant range.
/// `candidate_starts_limited` withholds every start whose travel leaves that range,
/// which keeps the key exact for every start the search judges.
///
/// This pins both sides of that boundary at the end of the range: the last start
/// whose travel fits is judged by the search exactly as `check_slot` judges it, and
/// the start one minute past it is refused by *both* entry points, so neither the
/// memo nor the exact walk ever has to judge a clipped span.
#[test]
fn rest_self_travel_at_the_last_instant_keeps_both_entry_points_consistent() {
    let engine = engine();
    let window = span(
        Instant::from_minutes(Instant::MAX.minutes() - 2 * 1_440).expect("in range"),
        Instant::MAX,
    );
    // The last admissible start: the booking ends 30 minutes before `Instant::MAX`
    // and its after-travel lands exactly on it, so the occupied span is still the
    // full `occupied_len` and the memo is exact. It stops at local midnight rather
    // than crossing it, so the rest rule leaves it alone.
    let admissible = Instant::from_minutes(Instant::MAX.minutes() - 60).expect("in range");
    // One minute later the same travel leaves the range.
    let clipped = Instant::from_minutes(Instant::MAX.minutes() - 59).expect("in range");
    let request = night_rest_request(window, &[admissible, clipped], 0, 30);

    let response = engine.find_availability(&request).expect("search succeeds");
    let offered: Vec<Instant> = response.offers.iter().filter_map(Offer::start).collect();
    assert_eq!(
        offered,
        vec![admissible],
        "the last start whose travel fits is offered, and the clipped one is withheld"
    );
    assert_eq!(
        response.excluded,
        Vec::new(),
        "a withheld start is not excluded"
    );

    let checked = engine
        .check_slot(
            &request,
            &Proposal::single("main", admissible, vec![vec!["ana".to_owned()]]),
        )
        .expect("check_slot succeeds");
    assert!(
        checked.valid,
        "the confirmation agrees the admissible start fits: {:?}",
        checked.reasons
    );

    let clipped_check = engine
        .check_slot(
            &request,
            &Proposal::single("main", clipped, vec![vec!["ana".to_owned()]]),
        )
        .expect_err("a proposal whose travel leaves the range cannot be judged");
    assert_eq!(
        clipped_check.code(),
        "proposal_out_of_range",
        "both entry points refuse to judge a clipped occupied span"
    );
}

/// The same boundary at the start of the range: before-travel below `Instant::MIN`
/// is withheld from the search and refused by the confirmation, and the first start
/// that fits is offered by both. Its occupied span begins at local midnight, inside
/// the early window, so the rest rule does not fire on it either.
#[test]
fn rest_self_travel_at_the_first_instant_keeps_both_entry_points_consistent() {
    let engine = engine();
    let window = span(
        Instant::MIN,
        Instant::from_minutes(Instant::MIN.minutes() + 2 * 1_440).expect("in range"),
    );
    let early = Instant::from_minutes(Instant::MIN.minutes() + 30).expect("in range");
    let request = night_rest_request(window, &[Instant::MIN, early], 30, 0);

    let response = engine.find_availability(&request).expect("search succeeds");
    let offered: Vec<Instant> = response.offers.iter().filter_map(Offer::start).collect();
    assert_eq!(offered, vec![early]);
    assert_eq!(response.excluded, Vec::new());

    let checked = engine
        .check_slot(
            &request,
            &Proposal::single("main", early, vec![vec!["ana".to_owned()]]),
        )
        .expect("check_slot succeeds");
    assert!(
        checked.valid,
        "the search and the confirmation agree at the first fitting start: {:?}",
        checked.reasons
    );

    let floor_check = engine
        .check_slot(
            &request,
            &Proposal::single("main", Instant::MIN, vec![vec!["ana".to_owned()]]),
        )
        .expect_err("a proposal whose before-travel leaves the range cannot be judged");
    assert_eq!(floor_check.code(), "proposal_out_of_range");
}

/// `BucketFilter`'s lower bound is its earliest *bucket label* minus the timezone
/// pad, never the search window minus the pad: a monthly pooled cap must count an
/// occurrence 20 days before the window start that shares its month, and a
/// Wednesday-starting weekly cap must bucket Tuesday's occurrence with the previous
/// Wednesday rather than the candidate's own week.
#[test]
fn bucket_filter_follows_the_label_not_the_window_start() {
    let engine = engine();

    // Monthly: the window starts on 2027-03-22 and the occurrence sits on
    // 2027-03-02 — 20 days earlier, still in March, so both fall in the bucket
    // labelled 2027-03-01.
    let monthly_start = utc(2027, 3, 22, 10, 0);
    let mut month_goal = Goal::single(
        zone_name("UTC"),
        span(utc(2027, 3, 22, 9, 0), utc(2027, 3, 22, 17, 0)),
        Duration::from_minutes(30),
        Requirement::new(vec![Term::at_least(1, Selector::role(Role::Host))]),
    );
    month_goal.candidates = Some(vec![monthly_start]);
    let mut monthly = Request::new(utc(2027, 3, 21, 0, 0), month_goal);
    monthly.participants = vec![Participant::new("h0", zone_name("UTC")).with_calendar(Vec::new())];
    monthly.rules = vec![Rule::new(
        "month_pool",
        RuleKind::Cap(Cap::new(
            Selector::True,
            CapMeasure::Count,
            1,
            CapPeriod::Month,
            CapPer::Pooled,
            RuleZone::Event,
            Weekday::Monday,
        )),
    )];
    monthly.occurrences = vec![
        Occurrence::new(
            "early_in_the_month",
            span(utc(2027, 3, 2, 9, 0), utc(2027, 3, 2, 9, 30)),
            vec!["h0".to_owned()],
        )
        .expect("the occurrence is non-empty"),
    ];
    monthly.explain = Some(Explain::new(100));

    let month_response = engine
        .find_availability(&monthly)
        .expect("the search succeeds");
    assert!(month_response.offers.is_empty());
    assert_eq!(month_response.excluded.len(), 1);
    assert_eq!(month_response.excluded[0].start, monthly_start);
    assert_eq!(
        month_response.excluded[0].reasons[0].code,
        ReasonCode::CapReached,
        "an occurrence in the same month is counted however far before the window it is"
    );

    // Weekly, starting on Wednesday: Tuesday 2027-03-23 belongs to the bucket
    // labelled 2027-03-17, so it excludes that candidate and leaves the Wednesday
    // 2027-03-24 candidate — a bucket of its own — open.
    let tuesday = utc(2027, 3, 23, 12, 0);
    let wednesday = utc(2027, 3, 24, 10, 0);
    let mut week_goal = Goal::single(
        zone_name("UTC"),
        span(utc(2027, 3, 23, 9, 0), utc(2027, 3, 24, 17, 0)),
        Duration::from_minutes(30),
        Requirement::new(vec![Term::at_least(1, Selector::role(Role::Host))]),
    );
    week_goal.candidates = Some(vec![tuesday, wednesday]);
    let mut weekly = Request::new(utc(2027, 3, 22, 0, 0), week_goal);
    weekly.participants = vec![Participant::new("h0", zone_name("UTC")).with_calendar(Vec::new())];
    weekly.rules = vec![Rule::new(
        "week_pool",
        RuleKind::Cap(Cap::new(
            Selector::True,
            CapMeasure::Count,
            1,
            CapPeriod::Week,
            CapPer::Pooled,
            RuleZone::Event,
            Weekday::Wednesday,
        )),
    )];
    weekly.occurrences = vec![
        Occurrence::new(
            "tuesday_meeting",
            span(utc(2027, 3, 23, 10, 0), utc(2027, 3, 23, 10, 30)),
            vec!["h0".to_owned()],
        )
        .expect("the occurrence is non-empty"),
    ];
    weekly.explain = Some(Explain::new(100));

    let week_response = engine
        .find_availability(&weekly)
        .expect("the search succeeds");
    let offered: Vec<Instant> = week_response
        .offers
        .iter()
        .filter_map(Offer::start)
        .collect();
    assert_eq!(offered, vec![wednesday]);
    assert_eq!(week_response.excluded.len(), 1);
    assert_eq!(week_response.excluded[0].start, tuesday);
    assert_eq!(
        week_response.excluded[0].reasons[0].code,
        ReasonCode::CapReached
    );
}

/// `BucketFilter` may pad a bucket label by two days on each side only because no
/// zone's offset ever leaves ±24h of UTC, so a local date is at most one date
/// label away from the UTC date at the same instant.
///
/// This sweeps the bundled database — every name it carries, at a sample of
/// instants across the whole supported range — so a database that broke that bound
/// fails here rather than silently dropping an occurrence.
#[test]
fn every_bundled_zone_offset_stays_within_a_day_of_utc() {
    const MINUTES_PER_DAY: i32 = 24 * 60;
    let db = TimeZoneDatabase::bundled().expect("the bundled database loads");
    let names: Vec<&'static str> = jiff_tzdb::available().collect();
    assert!(
        names.len() > 400,
        "the bundled database holds the IANA zones, but only {} names were listed",
        names.len()
    );

    // A month apart is fine-grained enough to see every offset regime a zone has
    // and coarse enough to keep the sweep to a few hundred thousand reads.
    let step = 30 * i64::from(MINUTES_PER_DAY);
    for name in names {
        let zone = db
            .zone(name)
            .unwrap_or_else(|err| panic!("`{name}` is listed but does not resolve: {err}"));
        let mut minute = Instant::MIN.minutes();
        while minute <= Instant::MAX.minutes() {
            let instant = Instant::from_minutes(minute).expect("a sampled instant is in range");
            let offset = zone.offset_at(instant).minutes();
            assert!(
                offset.abs() < MINUTES_PER_DAY,
                "`{name}` reads {offset} minutes from UTC at {instant}, beyond the one-day pad \
                 `BucketFilter` assumes"
            );
            minute += step;
        }
    }
}

/// `required_ranges` deduplicates repeated zone names across participants and
/// zone periods while producing the exact same UTC hull.
#[test]
fn required_ranges_deduplicates_repeated_zones_across_periods() {
    let engine = engine();
    let window = span(utc(2027, 3, 10, 9, 0), utc(2027, 3, 15, 17, 0));
    let goal = Goal::single(
        zone_name("UTC"),
        window,
        Duration::from_minutes(30),
        Requirement::new(vec![Term::at_least(1, Selector::role(Role::Host))]),
    );
    let mut query = RangesQuery::new(utc(2027, 3, 9, 0, 0), goal);

    let mut periods = Vec::new();
    for i in 0..200_i64 {
        let s = Instant::from_minutes(utc(2027, 3, 10, 0, 0).minutes() + i * 30).unwrap();
        let e = Instant::from_minutes(s.minutes() + 20).unwrap();
        let z = if i % 2 == 0 {
            "America/New_York"
        } else {
            "Asia/Tokyo"
        };
        periods.push(ZonePeriod::new(zone_name(z), s, e).unwrap());
    }
    query.participants = (0..50)
        .map(|i| {
            Participant::new(format!("h{i}"), zone_name("Europe/London"))
                .with_zone_periods(periods.clone())
        })
        .collect();
    query.rules = vec![Rule::new(
        "rest",
        RuleKind::Rest(Rest::new(time(20, 0), time(8, 0))),
    )];

    let deduplicated = engine.required_ranges(&query).expect("ranges succeed");

    // Compare against a 1-participant query carrying just one period per distinct
    // zone inside the window.
    let mut minimal = query.clone();
    minimal.participants = vec![
        Participant::new("h0", zone_name("Europe/London")).with_zone_periods(vec![
            ZonePeriod::new(
                zone_name("America/New_York"),
                utc(2027, 3, 10, 0, 0),
                utc(2027, 3, 10, 0, 20),
            )
            .unwrap(),
            ZonePeriod::new(
                zone_name("Asia/Tokyo"),
                utc(2027, 3, 10, 0, 30),
                utc(2027, 3, 10, 0, 50),
            )
            .unwrap(),
        ]),
    ];
    let expected = engine.required_ranges(&minimal).expect("minimal succeeds");
    assert_eq!(deduplicated, expected);
}

/// Shared fixed-zone windows with many disjoint ranges across many participants
/// (without `explain`) evaluate cleanly with exact results and no per-participant
/// interval-set cloning blowup.
#[test]
fn many_participants_sharing_dense_fixed_zone_windows_evaluate_cleanly() {
    let engine = engine();
    let window = span(utc(2027, 3, 10, 0, 0), utc(2027, 3, 20, 0, 0));
    let mut goal = Goal::single(
        zone_name("UTC"),
        window,
        Duration::from_minutes(30),
        Requirement::new(vec![Term::at_least(1, Selector::role(Role::Host))]),
    );
    goal.candidates = Some(vec![utc(2027, 3, 10, 9, 0), utc(2027, 3, 10, 12, 0)]);
    let mut request = Request::new(utc(2027, 3, 9, 0, 0), goal);
    request.participants = (0..100)
        .map(|i| Participant::new(format!("h{i}"), zone_name("UTC")).with_calendar(Vec::new()))
        .collect();

    // 200 disjoint 2-minute ranges in the morning plus 09:00-10:00 open.
    let mut ranges = Vec::new();
    for m in (0..540_u16).step_by(3) {
        ranges.push(
            WallRange::new(
                TimeOfDay::from_minute_of_day(m).unwrap(),
                WallEnd::Time(TimeOfDay::from_minute_of_day(m + 2).unwrap()),
            )
            .unwrap(),
        );
    }
    ranges.push(WallRange::new(time(9, 0), WallEnd::Time(time(10, 0))).unwrap());
    let spec = WindowSpec::new(
        RuleZone::Event,
        vec![WeeklyKey::new(Weekdays::range(Weekday::Monday, Weekday::Sunday), ranges).unwrap()],
        Vec::new(),
    )
    .unwrap();
    request.rules = (0..50)
        .map(|i| Rule::new(format!("w{i}"), RuleKind::Window(spec.clone())))
        .collect();

    let response = engine.find_availability(&request).expect("search succeeds");
    assert_eq!(response.offers.len(), 1);
    assert_eq!(response.offers[0].start(), Some(utc(2027, 3, 10, 9, 0)));
    assert_eq!(response.warnings.len(), 0);
}

/// Ensure `Travel` per-participant overrides work cleanly with `validate` and
/// `find_availability` when `default_spans` caching is active.
#[test]
fn validate_and_find_availability_agree_on_window_warnings_with_travel_overrides() {
    let engine = engine();
    let mut goal = Goal::single(
        zone_name("UTC"),
        span(utc(2027, 3, 10, 9, 0), utc(2027, 3, 10, 10, 0)),
        Duration::from_minutes(30),
        Requirement::new(vec![Term::at_least(1, Selector::role(Role::Host))]),
    );
    goal.candidates = Some(vec![utc(2027, 3, 10, 9, 0)]);
    goal.parts[0].travel = Travel::new(Duration::ZERO, Duration::ZERO).with_participant(
        "far",
        Duration::from_minutes(30),
        Duration::ZERO,
    );

    let mut request = Request::new(utc(2027, 3, 9, 0, 0), goal);
    request.participants = vec![
        Participant::new("near", zone_name("UTC")).with_calendar(Vec::new()),
        Participant::new("far", zone_name("UTC")).with_calendar(Vec::new()),
    ];
    request.rules = vec![
        presets::business_hours(
            RuleZone::Event,
            Weekdays::range(Weekday::Monday, Weekday::Sunday),
            time(9, 0),
            WallEnd::Time(time(17, 0)),
        )
        .expect("valid window"),
    ];

    let diag = engine.validate(&request);
    let resp = engine.find_availability(&request).expect("search succeeds");
    assert_eq!(
        diag.warnings, resp.warnings,
        "validate() and find_availability() must emit identical warnings"
    );
    assert_eq!(resp.warnings.len(), 1);
    assert_eq!(resp.warnings[0].code(), "no_candidate_in_window");
    assert!(resp.warnings[0].message().contains("`far`"));
}

/// A `working_days` maximum is resolved exactly for every start a search judges:
/// the offers are the candidates [`Horizon::bound`] allows, even though the
/// search resolves the count against the window's end rather than the whole
/// supported calendar.
#[test]
fn a_working_days_maximum_stays_exact_inside_the_search_window() {
    let engine = engine();
    let now = utc(2027, 3, 12, 9, 0); // Friday
    let holiday = date(2027, 3, 17);
    let calendar = WorkingCalendar::new(Zone::utc(), Weekdays::weekdays(), vec![holiday])
        .expect("the calendar works at least one day");
    let exact = Horizon::WorkingDays {
        count: 3,
        calendar: calendar.clone(),
    }
    .bound(now, &Zone::utc())
    .expect("the horizon resolves");

    let window_end = Instant::from_minutes(now.minutes() + 7 * 24 * 60).expect("in range");
    let starts: Vec<Instant> = (0..7 * 24 * 4)
        .map(|index| Instant::from_minutes(now.minutes() + index * 15).expect("in range"))
        .collect();
    let mut goal = Goal::single(
        zone_name("UTC"),
        span(now, window_end),
        Duration::from_minutes(30),
        Requirement::new(vec![Term::at_least(1, Selector::role(Role::Host))]),
    );
    goal.candidates = Some(starts.clone());

    let mut request = Request::new(now, goal);
    request.participants =
        vec![Participant::new("ana", zone_name("UTC")).with_calendar(Vec::new())];
    request.calendars.insert(
        "org".to_owned(),
        CalendarSpec {
            zone: zone_name("UTC"),
            working_days: Weekdays::weekdays(),
            holidays: vec![holiday],
        },
    );
    request.rules = vec![Rule::new(
        "horizon",
        RuleKind::LeadTime(LeadTime {
            min: Duration::ZERO,
            max: Some(LeadTimeMax::WorkingDays {
                count: 3,
                calendar: "org".to_owned(),
            }),
        }),
    )];

    let response = engine
        .find_availability(&request)
        .expect("the search succeeds");
    let offered: Vec<Instant> = response.offers.iter().filter_map(Offer::start).collect();
    let expected: Vec<Instant> = starts
        .iter()
        .copied()
        .filter(|start| exact.allows(*start))
        .collect();
    assert_eq!(
        offered, expected,
        "the search must apply exactly the horizon's bound"
    );
}

/// A `working_days` count no search can reach costs no calendar walk: the walk is
/// bounded by the search horizon, so a rule set at the configured maximum stays
/// fast and offers exactly what the same request without a maximum offers.
#[test]
fn an_unreachable_working_days_maximum_costs_no_walk() {
    let engine = engine();
    let now = utc(2027, 3, 12, 9, 0);
    let window_end = utc(2027, 3, 14, 9, 0);
    let candidates: Vec<Instant> = (0..2 * 24 * 4)
        .map(|index| Instant::from_minutes(now.minutes() + index * 15).expect("in range"))
        .collect();

    let build = |max: Option<LeadTimeMax>| {
        let mut goal = Goal::single(
            zone_name("UTC"),
            span(now, window_end),
            Duration::from_minutes(30),
            Requirement::new(vec![Term::at_least(1, Selector::role(Role::Host))]),
        );
        goal.candidates = Some(candidates.clone());
        let mut request = Request::new(now, goal);
        request.participants =
            vec![Participant::new("ana", zone_name("UTC")).with_calendar(Vec::new())];
        request.calendars.insert(
            "org".to_owned(),
            CalendarSpec {
                zone: zone_name("UTC"),
                working_days: Weekdays::weekdays(),
                holidays: Vec::new(),
            },
        );
        request.rules = (0..engine.limits().max_rules)
            .map(|index| {
                Rule::new(
                    format!("horizon-{index}"),
                    RuleKind::LeadTime(LeadTime {
                        min: Duration::ZERO,
                        max: max.clone(),
                    }),
                )
            })
            .collect();
        request
    };

    let unlimited = build(None);
    let unreachable = build(Some(LeadTimeMax::WorkingDays {
        count: u32::MAX,
        calendar: "org".to_owned(),
    }));

    let expected = engine
        .find_availability(&unlimited)
        .expect("the search succeeds");
    let started = std::time::Instant::now();
    let wide = engine
        .find_availability(&unreachable)
        .expect("the search succeeds");
    let elapsed = started.elapsed();

    let starts = |response: &opportune::model::Response| -> Vec<Instant> {
        response.offers.iter().filter_map(Offer::start).collect()
    };
    assert_eq!(starts(&wide), starts(&expected));
    assert!(
        elapsed < std::time::Duration::from_secs(2),
        "an unreachable working-day maximum must not walk the calendar once per rule: {elapsed:?}"
    );
}

/// A request whose blocks share one event key across several participants, each
/// block carrying its own tag and one per participant carrying the cap's tag.
fn shared_key_request(reverse: bool) -> Request {
    // The default cap on blocks per participant, so the request is inside its
    // limits while the tag union is at its widest.
    const BLOCKS: u32 = 10_000;
    const PARTICIPANTS: usize = 4;

    let held = span(utc(2027, 3, 10, 9, 0), utc(2027, 3, 10, 10, 0));
    let next_day = utc(2027, 3, 11, 9, 0);
    let blocks = |participant: usize, reverse: bool| -> Vec<Block> {
        let mut blocks: Vec<Block> = (0..BLOCKS)
            .map(|index| {
                let mut tags = vec![Tag::new(&format!("bulk/{participant}/{index}")).expect("tag")];
                if index == 0 {
                    // One block per participant carries the tag the cap counts.
                    tags.push(Tag::new("tally").expect("tag"));
                }
                Block::new(held)
                    .with_event_key("shared")
                    .expect("key")
                    .with_tags(tags)
            })
            .collect();
        if reverse {
            blocks.reverse();
        }
        blocks
    };

    let mut goal = Goal::single(
        zone_name("UTC"),
        span(utc(2027, 3, 10, 0, 0), utc(2027, 3, 12, 0, 0)),
        Duration::from_minutes(30),
        Requirement::new(vec![Term::at_least(1, Selector::role(Role::Host))]),
    );
    goal.candidates = Some(vec![utc(2027, 3, 10, 11, 0), next_day]);
    // The cap applies to a booking only when the booking itself carries a tag its
    // `count` matches.
    goal.parts[0].tags = vec![Tag::new("tally").expect("tag")];
    let mut request = Request::new(utc(2027, 3, 9, 0, 0), goal);
    request.explain = Some(Explain::new(64));
    request.participants = (0..PARTICIPANTS)
        .map(|index| {
            Participant::new(format!("h{index}"), zone_name("UTC"))
                .with_calendar(blocks(index, reverse))
        })
        .collect();
    request.rules = vec![Rule::new(
        "pooled",
        RuleKind::Cap(Cap::new(
            Selector::tag(Tag::new("tally").expect("tag")),
            CapMeasure::Count,
            1,
            CapPeriod::Day,
            CapPer::Pooled,
            RuleZone::Event,
            Weekday::Monday,
        )),
    )];
    request
}

/// What a shared-key response says: offer starts, exclusion starts, whether a cap
/// excluded one, and whether a keyless warning was raised.
fn shared_key_summary(
    response: &opportune::model::Response,
) -> (Vec<Instant>, Vec<Instant>, bool, bool) {
    (
        response
            .offers
            .iter()
            .filter_map(Offer::start)
            .collect::<Vec<_>>(),
        response
            .excluded
            .iter()
            .map(|entry| entry.start)
            .collect::<Vec<_>>(),
        response
            .excluded
            .iter()
            .flat_map(|entry| entry.reasons.iter())
            .any(|reason| reason.code == ReasonCode::CapReached),
        response
            .warnings
            .iter()
            .any(|warning| warning.code() == "block_missing_event_key"),
    )
}

/// A key shared by thousands of blocks on each of several participants unions its
/// tags in `O(n log n)`, and the merged event still counts once toward a pooled
/// cap. The answer does not depend on the order the blocks arrive in.
#[test]
fn a_shared_key_with_many_tags_counts_its_blocks_once() {
    let engine = engine();
    let forward_request = shared_key_request(false);
    let started = std::time::Instant::now();
    let forward = engine
        .find_availability(&forward_request)
        .expect("the search succeeds");
    let elapsed = started.elapsed();

    let (offers, excluded, cap_reached, keyless_warning) = shared_key_summary(&forward);
    assert_eq!(
        offers,
        vec![utc(2027, 3, 11, 9, 0)],
        "the merged event fills the day's bucket"
    );
    assert_eq!(excluded, vec![utc(2027, 3, 10, 11, 0)]);
    assert!(cap_reached, "the pooled cap is what excludes the first day");
    assert!(
        !keyless_warning,
        "every block carries the key, so no keyless warning is due"
    );
    assert!(
        elapsed < std::time::Duration::from_secs(2),
        "a shared key with 10,000 tags per participant must union in O(n log n): {elapsed:?}"
    );

    let reversed = engine
        .find_availability(&shared_key_request(true))
        .expect("the search succeeds");
    assert_eq!(
        shared_key_summary(&forward).0,
        shared_key_summary(&reversed).0,
        "the offer list must not depend on block order"
    );
    assert_eq!(
        shared_key_summary(&forward).1,
        shared_key_summary(&reversed).1,
        "the exclusion list must not depend on block order"
    );
}

/// A `who` selector naming the same participant many times selects exactly the
/// participants the deduplicated list selects, and a `when` that matches no part
/// never reaches any participant.
#[test]
fn a_repeated_ids_selector_and_an_unmatched_when_agree_across_participants() {
    let engine = engine();
    let now = utc(2027, 3, 10, 0, 0);
    let ranges = vec![WallRange::new(time(9, 0), WallEnd::Time(time(17, 0))).expect("range")];
    let spec = WindowSpec::new(
        RuleZone::Event,
        vec![
            WeeklyKey::new(Weekdays::range(Weekday::Monday, Weekday::Sunday), ranges).expect("key"),
        ],
        Vec::new(),
    )
    .expect("spec");

    let build = |ids: Vec<String>| {
        let mut goal = Goal::single(
            zone_name("UTC"),
            span(utc(2027, 3, 10, 0, 0), utc(2027, 3, 10, 23, 0)),
            Duration::from_minutes(30),
            Requirement::new(vec![Term::at_least(1, Selector::role(Role::Host))]),
        );
        goal.candidates = Some(vec![utc(2027, 3, 10, 8, 0), utc(2027, 3, 10, 9, 30)]);
        let mut request = Request::new(now, goal);
        request.participants = (0..4)
            .map(|index| {
                Participant::new(format!("h{index}"), zone_name("UTC")).with_calendar(Vec::new())
            })
            .collect();
        request.rules = vec![
            Rule::new("repeated", RuleKind::Window(spec.clone()))
                .with_who(Selector::ids(ids.clone())),
            // Its `when` names a tag no part carries, so it can never apply.
            Rule::new("never", RuleKind::Clearance(Clearance::default()))
                .with_when(Selector::tag(Tag::new("no/such").unwrap())),
        ];
        request
    };

    let repeated = build(vec!["h0".to_owned(); 10_000]);
    let deduplicated = build(vec!["h0".to_owned()]);

    let diagnostics = engine.validate(&repeated);
    assert!(
        diagnostics
            .warnings
            .iter()
            .any(|warning| warning.code() == "when_matches_no_part"),
        "the unmatched `when` is reported once: {:?}",
        diagnostics.warnings
    );

    let response = engine
        .find_availability(&repeated)
        .expect("the search succeeds");
    assert_eq!(response.offers.len(), 2);
    let eligible: Vec<Vec<String>> = response
        .offers
        .iter()
        .map(|offer| offer.events[0].eligible[0].clone())
        .collect();
    assert_eq!(
        eligible,
        vec![
            // 08:00: only `h0` has a window, and `h0` is outside it.
            vec!["h1".to_owned(), "h2".to_owned(), "h3".to_owned()],
            // 09:30: every host is inside.
            vec![
                "h0".to_owned(),
                "h1".to_owned(),
                "h2".to_owned(),
                "h3".to_owned()
            ],
        ],
        "a repeated ID selects the same participant a deduplicated list does"
    );

    let single = engine
        .find_availability(&deduplicated)
        .expect("the search succeeds");
    let starts = |response: &opportune::model::Response| -> Vec<Vec<String>> {
        response
            .offers
            .iter()
            .map(|offer| offer.events[0].eligible[0].clone())
            .collect()
    };
    assert_eq!(starts(&response), starts(&single));
}

/// Every host tied on weight with a term that has to dodge a singleton term for each
/// of them: the tie-break probes the flow once per rejected candidate, so the first
/// term alone costs 63 solves, and each solve reuses the pooled graph instead of
/// rebuilding one. The answer must still be the lexicographic optimum, and stable.
#[test]
fn a_wide_tied_assignment_probes_the_flow_in_reused_scratch_and_stays_exact() {
    const HOSTS: usize = 64;
    let engine = engine();
    let hosts: Vec<String> = (0..HOSTS).map(|index| format!("h{index:02}")).collect();

    // One flexible term over every host, then a singleton term for all but the
    // last, so the flexible term can only take `h63`.
    let mut requirement = vec![Term::at_least(1, Selector::role(Role::Host))];
    requirement.extend(hosts[..HOSTS - 1].iter().map(Term::person));

    let mut goal = Goal::single(
        zone_name("UTC"),
        span(utc(2027, 3, 10, 9, 0), utc(2027, 3, 10, 17, 0)),
        Duration::from_minutes(30),
        Requirement::new(requirement),
    );
    goal.candidates = Some(vec![utc(2027, 3, 10, 9, 0)]);
    let mut request = Request::new(utc(2027, 3, 9, 0, 0), goal);
    request.participants = hosts
        .iter()
        .map(|id| Participant::new(id, zone_name("UTC")).with_calendar(Vec::new()))
        .collect();
    request.limit = Some(1);

    let started = std::time::Instant::now();
    let response = engine
        .find_availability(&request)
        .expect("the search succeeds");
    let elapsed = started.elapsed();

    let mut expected: Vec<Vec<String>> = vec![vec![hosts[HOSTS - 1].clone()]];
    expected.extend(hosts[..HOSTS - 1].iter().map(|id| vec![id.clone()]));
    assert_eq!(
        response.offers[0].events[0].assignment, expected,
        "the flexible term takes the only host no singleton needs"
    );
    assert!(
        elapsed < std::time::Duration::from_secs(2),
        "a 64-host tied assignment must not rebuild its graph per probe: {elapsed:?}"
    );

    let again = engine
        .find_availability(&request)
        .expect("the search succeeds");
    assert_eq!(
        again.offers[0].events[0].assignment, response.offers[0].events[0].assignment,
        "the answer must not depend on anything but the request"
    );
}
