//! End-to-end tests for the single-participant engine, over the bundled
//! database: what `validate`,
//! `required_ranges` and `find_availability` answer, driven through the public
//! model types and the public `Engine`.
//!
//! Every test builds a real request, runs it on a real engine and asserts the
//! answer a caller acts on, so deleting the behaviour it covers turns it red.
//!
//! Everything here needs the bundled database, so the file is compiled only when
//! the feature is on; the tests that use a database directory live in
//! `engine_directory.rs` and run without it.

#![cfg(feature = "bundled-tzdb")]

use std::collections::BTreeMap;

use opportune::model::{
    Block, BlockStatus, CalendarSpec, Goal, LeadTime, LeadTimeMax, Occurrence, Part, Participant,
    ParticipantTravel, Proposal, ProposalEvent, RangesQuery, Request, Requirement, Rule, RuleKind,
    Span, StatusEffect, StatusPolicy, StatusPolicyEntry, Term, Travel, ZonePeriod,
};
use opportune::tags::{Role, Selector, Tag};
use opportune::time::{
    BlackoutSpan, BlackoutSpec, Date, DateKey, DateRange, Duration, Grid, GridZone, Instant,
    LocalDateTime, LocalResolution, RuleZone, TimeOfDay, TimeZoneDatabase, WallEnd, WallRange,
    Weekday, Weekdays, WeeklyKey, WindowSpec, WorkingDays, Zone, ZoneName,
};
use opportune::{Engine, Limits, presets};

/// The instant of a UTC civil date-time, which always exists.
fn utc(year: i16, month: i8, day: i8, hour: u8, minute: u8) -> Instant {
    let date = Date::new(year, month, day).unwrap();
    let time = TimeOfDay::new(hour, minute).unwrap();
    let LocalResolution::Unique(instant) = Zone::utc().resolve_local(date, time).unwrap() else {
        panic!("UTC has no transitions");
    };
    instant
}

fn name(zone: &str) -> ZoneName {
    ZoneName::new(zone).unwrap()
}

fn tag(path: &str) -> Tag {
    Tag::new(path).unwrap()
}

/// `count` minutes after the earliest supported instant, `1975-01-01T00:00Z`.
///
/// This is not the frame `Instant::minutes` returns, which counts from the Unix
/// epoch: to shift an instant by `n` minutes use `Instant::from_minutes`, not
/// this helper.
fn minutes(count: i64) -> Instant {
    Instant::from_minutes(Instant::MIN.minutes() + count).unwrap()
}

fn span(start: Instant, end: Instant) -> Span {
    Span::new(start, end).unwrap()
}

/// A window open on every day between `start` and `end` minutes of the day.
fn daily_window(start: u8, end: u8) -> WindowSpec {
    daily_window_in(RuleZone::Named(name("UTC")), start, end)
}

fn daily_window_in(zone: RuleZone, start: u8, end: u8) -> WindowSpec {
    let hours = WallRange::new(
        TimeOfDay::new(start, 0).unwrap(),
        WallEnd::Time(TimeOfDay::new(end, 0).unwrap()),
    )
    .unwrap();
    WindowSpec::new(
        zone,
        vec![
            WeeklyKey::new(
                Weekdays::range(Weekday::Monday, Weekday::Sunday),
                vec![hours],
            )
            .unwrap(),
        ],
        Vec::new(),
    )
    .unwrap()
}

fn window_rule(id: &str, spec: WindowSpec) -> Rule {
    Rule::new(id, RuleKind::Window(spec))
}

fn blackout_rule(id: &str, spec: BlackoutSpec) -> Rule {
    Rule::new(id, RuleKind::Blackout(spec))
}

fn block(start: Instant, end: Instant) -> Block {
    Block::new(span(start, end))
}

/// An engine over the bundled database with the default limits.
fn engine() -> Engine {
    Engine::builder().build().unwrap()
}

fn bundled_database() -> TimeZoneDatabase {
    TimeZoneDatabase::bundled().unwrap()
}

/// A minimal request: one 30-minute part for `rich`, over `search`, with the
/// participant carrying the calendar and tags given.
fn single_host_request(
    now: Instant,
    search: Span,
    participant: Participant,
    rules: Vec<Rule>,
) -> Request {
    let part = Part::new(
        Duration::from_minutes(30),
        Requirement::new(vec![Term::person(participant.id.clone())]),
    );
    let mut request = Request::new(now, Goal::new(name("UTC"), search, vec![part]));
    request.participants.push(participant);
    request.rules = rules;
    request
}

/// The start instants of the response's offers.
fn offer_starts(response: &opportune::model::Response) -> Vec<Instant> {
    response
        .offers
        .iter()
        .map(|offer| offer.start().unwrap())
        .collect()
}

/// A participant with an empty calendar: known to be free.
fn free_participant(id: &str) -> Participant {
    let mut participant = Participant::new(id, name("UTC"));
    participant.calendar = Some(Vec::new());
    participant
}

// ---------------------------------------------------------------- windows ---

#[test]
fn windows_intersect_so_only_the_overlap_is_offered() {
    let request = single_host_request(
        minutes(0),
        span(minutes(9 * 60), minutes(18 * 60)),
        free_participant("rich"),
        vec![
            window_rule("broad", daily_window(9, 17)),
            window_rule("narrow", daily_window(10, 11)),
        ],
    );
    let response = engine().find_availability(&request).unwrap();
    assert!(!response.offers.is_empty());
    for start in offer_starts(&response) {
        let local = start.zoned(&Zone::utc());
        assert!(
            (10..11).contains(&i64::from(local.time().hour())),
            "{start} is inside the narrowed window"
        );
    }
    // 10:00 to 10:30 inclusive: the 30-minute booking must finish by 11:00.
    assert_eq!(response.offers.len(), 2);
    assert_eq!(response.offers[0].start(), Some(minutes(10 * 60)));
    assert_eq!(response.offers[1].start(), Some(minutes(10 * 60 + 30)));
}

#[test]
fn a_participant_with_no_window_rule_has_no_window_restriction() {
    let request = single_host_request(
        minutes(0),
        span(minutes(9 * 60), minutes(12 * 60)),
        free_participant("rich"),
        Vec::new(),
    );
    let response = engine().find_availability(&request).unwrap();
    // A 30-minute default grid over three hours: six starts, at any hour.
    assert_eq!(response.offers.len(), 6);
    assert_eq!(response.offers[0].start(), Some(minutes(9 * 60)));
}

#[test]
fn travel_must_fall_inside_the_window_like_the_booking() {
    let mut participant = free_participant("rich");
    participant.tags = vec![tag("team/sales")];
    let mut request = single_host_request(
        minutes(0),
        span(minutes(9 * 60), minutes(12 * 60)),
        participant,
        vec![window_rule("hours", daily_window(9, 12))],
    );
    request.goal.parts[0].travel = Travel {
        before: Duration::from_minutes(30),
        after: Duration::from_minutes(0),
        by_participant: BTreeMap::new(),
    };
    let response = engine().find_availability(&request).unwrap();
    // The booking's occupied start is 30 minutes before its start, so 08:30 is
    // the first instant that fits: the 09:00 candidate fails, 09:30 passes.
    assert_eq!(response.offers[0].start(), Some(minutes(9 * 60 + 30)));
    let reserve = &response.offers[0].events[0].reserve;
    assert_eq!(reserve.len(), 1);
    assert_eq!(reserve[0].kind.as_str(), "travel");
    assert_eq!(reserve[0].participant, "rich");
    assert_eq!(
        (reserve[0].span.start(), reserve[0].span.end()),
        (minutes(9 * 60), minutes(9 * 60 + 30))
    );
}

// --------------------------------------------------------------- blackouts ---

#[test]
fn blackouts_union_so_neither_rule_offers_a_time() {
    let blackout = |start: LocalDateTime, end: LocalDateTime| {
        BlackoutSpec::new(
            RuleZone::Named(name("UTC")),
            vec![BlackoutSpan::new(start, end).unwrap()],
        )
    };
    let at = |hour: u8, minute: u8| {
        LocalDateTime::new(
            Date::new(1975, 1, 1).unwrap(),
            TimeOfDay::new(hour, minute).unwrap(),
        )
    };
    let request = single_host_request(
        minutes(0),
        span(minutes(9 * 60), minutes(13 * 60)),
        free_participant("rich"),
        vec![
            blackout_rule("first", blackout(at(10, 0), at(11, 0))),
            blackout_rule("second", blackout(at(11, 30), at(12, 0))),
        ],
    );
    let response = engine().find_availability(&request).unwrap();
    let starts = offer_starts(&response);
    assert!(starts.contains(&minutes(9 * 60 + 30)));
    // A booking overlaps a blackout when its own span does, so a start exactly
    // where a blackout ends is fine and one where the next begins is not.
    for blocked in [10 * 60, 10 * 60 + 30, 11 * 60 + 30] {
        assert!(
            !starts.contains(&minutes(blocked)),
            "{blocked} is blacked out"
        );
    }
    assert!(starts.contains(&minutes(11 * 60)));
    assert!(starts.contains(&minutes(12 * 60)));
}

// --------------------------------------------------------------- clearance ---

#[test]
fn clearance_widens_blocking_blocks_and_never_windows() {
    let mut participant = free_participant("rich");
    participant.calendar = Some(vec![block(minutes(10 * 60), minutes(11 * 60))]);
    let request = single_host_request(
        minutes(0),
        span(minutes(11 * 60), minutes(13 * 60)),
        participant,
        vec![
            window_rule("hours", daily_window(11, 13)),
            Rule::new(
                "buffers",
                RuleKind::Clearance(opportune::model::Clearance {
                    before: Duration::from_minutes(15),
                    after: Duration::from_minutes(5),
                    min_gap: Duration::from_minutes(0),
                }),
            ),
        ],
    );
    let response = engine().find_availability(&request).unwrap();
    // The block ends at 11:00 and the booking needs 15 minutes before it, so the
    // first bookable start is 11:15 even though the window opens at 11:00.
    let starts = offer_starts(&response);
    assert!(
        !starts.contains(&minutes(11 * 60)),
        "the window edge does not move"
    );
    assert!(starts.contains(&minutes(11 * 60 + 30)));
}

#[test]
fn the_larger_of_a_booking_gap_and_a_min_gap_applies_and_they_never_sum() {
    let mut participant = free_participant("rich");
    // A single block with travel after it: its occupied span ends at 11:20.
    let mut busy = block(minutes(10 * 60), minutes(11 * 60));
    busy.travel_after = Duration::from_minutes(20);
    participant.calendar = Some(vec![busy]);
    let mut request = single_host_request(
        minutes(0),
        span(minutes(11 * 60), minutes(13 * 60)),
        participant,
        vec![Rule::new(
            "buffers",
            RuleKind::Clearance(opportune::model::Clearance {
                before: Duration::from_minutes(10),
                after: Duration::from_minutes(0),
                min_gap: Duration::from_minutes(5),
            }),
        )],
    );
    request.goal.parts[0].travel = Travel {
        before: Duration::from_minutes(10),
        after: Duration::from_minutes(10),
        by_participant: BTreeMap::new(),
    };
    request.goal.candidates = Some(vec![minutes(11 * 60 + 30), minutes(11 * 60 + 40)]);
    let response = engine().find_availability(&request).unwrap();
    let starts = offer_starts(&response);
    // The occupied block ends 11:20, and the gap required is max(10, 5) = 10, so
    // the booking's occupied start is 11:30 and its start 11:40. Summing the two
    // would push it to 11:45 and lose the 11:40 start.
    assert!(!starts.contains(&minutes(11 * 60 + 30)));
    assert_eq!(starts.first(), Some(&minutes(11 * 60 + 40)));
}

#[test]
fn clearance_never_shortens_against_a_blackout() {
    let blackout = BlackoutSpec::new(
        RuleZone::Named(name("UTC")),
        vec![
            BlackoutSpan::new(
                LocalDateTime::new(
                    Date::new(1975, 1, 1).unwrap(),
                    TimeOfDay::new(10, 0).unwrap(),
                ),
                LocalDateTime::new(
                    Date::new(1975, 1, 1).unwrap(),
                    TimeOfDay::new(11, 0).unwrap(),
                ),
            )
            .unwrap(),
        ],
    );
    let request = single_host_request(
        minutes(0),
        span(minutes(9 * 60), minutes(12 * 60)),
        free_participant("rich"),
        vec![
            blackout_rule("closed", blackout),
            Rule::new(
                "buffers",
                RuleKind::Clearance(opportune::model::Clearance {
                    before: Duration::from_minutes(30),
                    after: Duration::from_minutes(30),
                    min_gap: Duration::from_minutes(0),
                }),
            ),
        ],
    );
    let response = engine().find_availability(&request).unwrap();
    let starts = offer_starts(&response);
    // The blackout is judged on the booking's own occupied span, unwidened: a
    // booking ending where the blackout starts is fine, and one starting where
    // it ends is fine too.
    assert!(starts.contains(&minutes(9 * 60 + 30)));
    assert!(starts.contains(&minutes(11 * 60)));
}

// --------------------------------------------------------------- lead time ---

#[test]
fn minimum_notice_counts_travel_and_the_largest_minimum_wins() {
    let mut request = single_host_request(
        minutes(0),
        span(minutes(0), minutes(5 * 60)),
        free_participant("rich"),
        vec![
            Rule::new(
                "notice",
                RuleKind::LeadTime(LeadTime {
                    min: Duration::from_minutes(60),
                    max: None,
                }),
            ),
            Rule::new(
                "longer-notice",
                RuleKind::LeadTime(LeadTime {
                    min: Duration::from_minutes(120),
                    max: None,
                }),
            ),
        ],
    );
    request.goal.parts[0].travel = Travel {
        before: Duration::from_minutes(30),
        after: Duration::from_minutes(0),
        by_participant: BTreeMap::new(),
    };
    let response = engine().find_availability(&request).unwrap();
    // The occupied start is 30 minutes before the booking, and the notice is 120
    // minutes from `now`: the first start is 150 minutes in.
    assert_eq!(
        response
            .offers
            .first()
            .and_then(opportune::model::Offer::start),
        Some(minutes(150))
    );
}

#[test]
fn the_earliest_maximum_wins_across_rule_kinds() {
    let request = single_host_request(
        minutes(0),
        span(minutes(0), minutes(30 * 60)),
        free_participant("rich"),
        vec![
            Rule::new(
                "days",
                RuleKind::LeadTime(LeadTime {
                    min: Duration::ZERO,
                    max: Some(LeadTimeMax::Days(1)),
                }),
            ),
            Rule::new(
                "hours",
                RuleKind::LeadTime(LeadTime {
                    min: Duration::ZERO,
                    max: Some(LeadTimeMax::Duration(Duration::from_minutes(120))),
                }),
            ),
        ],
    );
    let response = engine().find_availability(&request).unwrap();
    let starts = offer_starts(&response);
    // A 120-minute maximum ends before the one-day maximum does.
    assert!(starts.contains(&minutes(120)));
    assert!(!starts.contains(&minutes(150)));
}

#[test]
fn a_working_day_maximum_reads_its_named_calendar() {
    // `now` is Friday 1975-01-03 00:00Z.
    let now = utc(1975, 1, 3, 0, 0);
    let calendar = CalendarSpec {
        zone: name("UTC"),
        working_days: WorkingDays::weekdays(),
        holidays: Vec::new(),
    };
    let mut request = single_host_request(
        now,
        span(now, utc(1975, 1, 10, 0, 0)),
        free_participant("rich"),
        vec![Rule::new(
            "horizon",
            RuleKind::LeadTime(LeadTime {
                min: Duration::ZERO,
                max: Some(LeadTimeMax::WorkingDays {
                    count: 1,
                    calendar: "org".to_owned(),
                }),
            }),
        )],
    );
    request.calendars.insert("org".to_owned(), calendar);
    let response = engine().find_availability(&request).unwrap();
    // Saturday and Sunday are not working days, so the first working day after
    // Friday is Monday the 6th: a start on the 6th is inside, one on the 7th is
    // not.
    let starts = offer_starts(&response);
    let on_monday = utc(1975, 1, 6, 12, 0);
    let on_tuesday = utc(1975, 1, 7, 0, 0);
    assert!(starts.contains(&on_monday));
    assert!(starts.iter().all(|start| *start < on_tuesday));
}

#[test]
fn each_lead_time_maximum_bounds_only_the_participants_it_selects() {
    // `now` is Friday 1975-01-03 00:00Z. Two working-day horizon rules, each
    // selected for one host: every host reads the bound of the rules that apply to
    // it, and one rule's bound never stands in for another's.
    let now = utc(1975, 1, 3, 0, 0);
    let calendar = CalendarSpec {
        zone: name("UTC"),
        working_days: WorkingDays::weekdays(),
        holidays: Vec::new(),
    };
    let horizon = |id: &str, count: u32, who: &str| {
        Rule::new(
            id,
            RuleKind::LeadTime(LeadTime {
                min: Duration::ZERO,
                max: Some(LeadTimeMax::WorkingDays {
                    count,
                    calendar: who.to_owned(),
                }),
            }),
        )
        .with_who(Selector::ids([who]))
    };
    let last_start = |id: &str| {
        let part = Part::new(
            Duration::from_minutes(30),
            Requirement::new(vec![Term::person(id)]),
        );
        let mut request = Request::new(
            now,
            Goal::new(name("UTC"), span(now, utc(1975, 1, 20, 0, 0)), vec![part]),
        );
        request.participants = vec![free_participant("quick"), free_participant("slowpoke")];
        request
            .calendars
            .insert("quick".to_owned(), calendar.clone());
        request
            .calendars
            .insert("slowpoke".to_owned(), calendar.clone());
        request.rules = vec![horizon("fast", 1, "quick"), horizon("slow", 5, "slowpoke")];
        *offer_starts(&engine().find_availability(&request).unwrap())
            .last()
            .expect("a free host offers starts inside its horizon")
    };
    // One working day after Friday is Monday the 6th; five is Friday the 10th.
    let quick = last_start("quick");
    assert!(
        quick < utc(1975, 1, 7, 0, 0),
        "{quick} is past one working day"
    );
    assert!(
        quick >= utc(1975, 1, 6, 0, 0),
        "{quick} is inside one working day"
    );
    let slow = last_start("slowpoke");
    assert!(
        slow < utc(1975, 1, 11, 0, 0),
        "{slow} is past five working days"
    );
    assert!(
        slow >= utc(1975, 1, 10, 0, 0),
        "{slow} must reach the five-working-day horizon the other rule does not bound"
    );
}

#[test]
fn the_horizon_warning_reads_each_participants_own_maximum() {
    // `now` is Friday 1975-01-03 00:00Z. Two working-day horizons over one
    // calendar, each selected for one participant: `slow` may start at `now`, so
    // five working days does not bound it, while `quick`'s five-day notice lands
    // past the one working day its own maximum allows. A bound resolved for one
    // rule must not stand in for the other.
    let now = utc(1975, 1, 3, 0, 0);
    let calendar = CalendarSpec {
        zone: name("UTC"),
        working_days: WorkingDays::weekdays(),
        holidays: Vec::new(),
    };
    let horizon = |id: &str, count: u32, notice: Duration, who: &str| {
        Rule::new(
            id,
            RuleKind::LeadTime(LeadTime {
                min: notice,
                max: Some(LeadTimeMax::WorkingDays {
                    count,
                    calendar: "org".to_owned(),
                }),
            }),
        )
        .with_who(Selector::ids([who]))
    };
    let mut request = single_host_request(
        now,
        span(now, utc(1975, 2, 1, 0, 0)),
        free_participant("slow"),
        vec![
            horizon("five_days", 5, Duration::ZERO, "slow"),
            horizon("one_day", 1, Duration::from_minutes(5 * 24 * 60), "quick"),
        ],
    );
    request.participants.push(free_participant("quick"));
    request.calendars.insert("org".to_owned(), calendar);
    let diagnostics = engine().validate(&request);
    let horizons: Vec<(Option<&str>, &str)> = diagnostics
        .warnings
        .iter()
        .filter(|warning| warning.code() == "horizon_before_earliest_start")
        .map(|warning| (warning.path(), warning.message()))
        .collect();
    assert_eq!(horizons.len(), 1, "{horizons:?}");
    assert_eq!(horizons[0].0, Some("/rules/1/params/max"));
    assert!(
        horizons[0].1.contains("`quick`"),
        "the warning names the participant its own maximum excludes: {}",
        horizons[0].1
    );
}

#[test]
fn a_named_zone_window_is_read_once_while_a_participant_zone_window_follows_each_clock() {
    // Two hosts sixteen hours apart, judged in one call: the named window covers
    // the same minutes for both, and each host's own window is read in their own
    // zone.
    let mut ana = free_participant("ana");
    ana.zone = name("Asia/Tokyo");
    let mut bo = free_participant("bo");
    bo.zone = name("America/Los_Angeles");
    let mut request = Request::new(
        utc(2027, 6, 14, 0, 0),
        Goal::new(
            name("UTC"),
            span(utc(2027, 6, 14, 0, 0), utc(2027, 6, 15, 0, 0)),
            vec![Part::new(
                Duration::from_minutes(30),
                Requirement::new(vec![Term::person("ana"), Term::person("bo")]),
            )],
        ),
    );
    request.participants = vec![ana, bo];
    request.rules = vec![
        // Open all day in UTC, so only the per-participant window decides.
        window_rule("utc", daily_window(0, 23)),
        window_rule("morning", daily_window_in(RuleZone::Participant, 9, 10)),
    ];
    // 00:30Z is 09:30 in Tokyo and 17:30 the previous day in Los Angeles.
    let check = engine()
        .check_slot(
            &request,
            &Proposal {
                events: vec![ProposalEvent {
                    part: "main".to_owned(),
                    start: utc(2027, 6, 14, 0, 30),
                    assignment: vec![vec!["ana".to_owned()], vec!["bo".to_owned()]],
                }],
            },
        )
        .unwrap();
    assert!(!check.valid, "the Los Angeles host is not in their morning");
    let cited: Vec<(Option<&str>, opportune::reasons::ReasonCode)> = check
        .reasons
        .iter()
        .map(|reason| (reason.participant.as_deref(), reason.code))
        .collect();
    assert_eq!(
        cited,
        [(Some("bo"), opportune::reasons::ReasonCode::OutsideWindow)],
        "the Tokyo host is inside their own morning, the Los Angeles host is not"
    );
}

// ----------------------------------------------------------- status policy ---
#[test]
fn the_first_matching_status_entry_across_rules_decides_a_block() {
    let mut participant = free_participant("rich");
    let mut focus = block(minutes(10 * 60), minutes(12 * 60));
    focus.tags = vec![tag("status/focus")];
    focus.travel_before = Duration::from_minutes(30);
    participant.calendar = Some(vec![focus]);
    let ignore_focus = Rule::new(
        "user-policy",
        RuleKind::StatusPolicy(StatusPolicy {
            entries: vec![StatusPolicyEntry::tags(
                Selector::Tag(tag("status/focus")),
                StatusEffect::Ignore,
            )],
        }),
    );
    let block_focus = Rule::new(
        "org-policy",
        RuleKind::StatusPolicy(StatusPolicy {
            entries: vec![StatusPolicyEntry::tags(
                Selector::Tag(tag("status/focus")),
                StatusEffect::Block,
            )],
        }),
    );
    let search = span(minutes(9 * 60), minutes(13 * 60));

    // Whichever policy comes first in request order decides, and an ignored
    // block's travel is ignored with it.
    let ignored = engine()
        .find_availability(&single_host_request(
            minutes(0),
            search,
            participant.clone(),
            vec![ignore_focus.clone(), block_focus.clone()],
        ))
        .unwrap();
    assert!(
        offer_starts(&ignored).contains(&minutes(9 * 60)),
        "an ignored block and its travel block nothing"
    );

    let blocked = engine()
        .find_availability(&single_host_request(
            minutes(0),
            search,
            participant,
            vec![block_focus, ignore_focus],
        ))
        .unwrap();
    let starts = offer_starts(&blocked);
    // The block's occupied span is 09:30-12:00, so a booking ending where it
    // starts and one starting where it ends are both clear of it.
    assert!(starts.contains(&minutes(9 * 60)));
    assert!(!starts.contains(&minutes(9 * 60 + 30)));
    assert!(!starts.contains(&minutes(11 * 60)));
    assert!(starts.contains(&minutes(12 * 60)));
}

/// The preset builds the rule the engine applies: a tentative block it names as
/// ignored blocks nothing, while the busy block beside it still does.
#[test]
fn the_status_policy_preset_ignores_the_status_it_names() {
    let mut participant = free_participant("rich");
    let mut tentative = block(minutes(10 * 60), minutes(11 * 60));
    tentative.status = BlockStatus::Tentative;
    let busy = block(minutes(14 * 60), minutes(15 * 60));
    participant.calendar = Some(vec![tentative, busy]);
    let search = span(minutes(9 * 60), minutes(16 * 60));
    let policy = presets::status_policy(vec![StatusPolicyEntry::status(
        BlockStatus::Tentative,
        StatusEffect::Ignore,
    )]);
    assert_eq!(policy.id, "status_policy");

    let without = engine()
        .find_availability(&single_host_request(
            minutes(0),
            search,
            participant.clone(),
            Vec::new(),
        ))
        .unwrap();
    assert!(
        !offer_starts(&without).contains(&minutes(10 * 60)),
        "without a policy a tentative block blocks its own hour"
    );

    let with = engine()
        .find_availability(&single_host_request(
            minutes(0),
            search,
            participant,
            vec![policy],
        ))
        .unwrap();
    let starts = offer_starts(&with);
    assert!(
        starts.contains(&minutes(10 * 60)) && starts.contains(&minutes(10 * 60 + 30)),
        "the ignored status stops blocking its own hour"
    );
    assert!(
        !starts.contains(&minutes(14 * 60)) && !starts.contains(&minutes(14 * 60 + 30)),
        "a status the policy does not name keeps its own reading"
    );
}

// ------------------------------------------------------------- applicants ---

#[test]
fn who_and_when_decide_which_rules_apply() {
    let mut participant = free_participant("rich");
    participant.tags = vec![tag("team/sales")];
    let narrow = window_rule("narrow", daily_window(10, 11));
    let someone_else = narrow
        .clone()
        .with_id("someone-else")
        .with_who(Selector::Ids(vec!["ana".to_owned()]));
    let interviews_only = narrow
        .clone()
        .with_id("interviews")
        .with_when(Selector::Tag(tag("interview")));

    let search = span(minutes(9 * 60), minutes(13 * 60));
    let mut with_ana = single_host_request(
        minutes(0),
        search,
        participant.clone(),
        vec![someone_else, interviews_only.clone()],
    );
    with_ana.participants.push(free_participant("ana"));
    let scoped = engine().find_availability(&with_ana).unwrap();
    // Neither rule reaches this participant and booking, so there is no window
    // restriction at all.
    assert_eq!(scoped.offers.len(), 8);

    // `when` is matched against the part's tags, so tagging the booking brings
    // the rule in.
    let mut matched = single_host_request(
        minutes(0),
        search,
        participant,
        vec![interviews_only.with_when(Selector::Tag(tag("team")))],
    );
    matched.goal.parts[0].tags = vec![tag("team/sales")];
    let matched = engine().find_availability(&matched).unwrap();
    assert_eq!(matched.offers.len(), 2);
}

// ------------------------------------------------------------------ output ---

#[test]
fn the_default_grid_is_the_duration_capped_at_thirty_minutes() {
    let part = Part::new(
        Duration::from_minutes(45),
        Requirement::new(vec![Term::person("rich")]),
    );
    let mut request = Request::new(
        minutes(0),
        Goal::new(name("UTC"), span(minutes(0), minutes(2 * 60)), vec![part]),
    );
    request.participants.push(free_participant("rich"));
    let response = engine().find_availability(&request).unwrap();
    assert_eq!(response.offers.len(), 4);
    assert_eq!(response.offers[0].start(), Some(minutes(0)));
    assert_eq!(
        response.offers[0].events[0].end,
        minutes(45),
        "the offer uses the part's duration, not the grid step"
    );
    assert_eq!(response.offers[1].start(), Some(minutes(30)));
}

/// The default grid's anchor is local midnight in the *event* zone, which the
/// documentation states and which no test distinguishes: the display zone is UTC
/// and the host's own zone is a whole-hour offset, so a grid misread in either of
/// those would still be anchored on UTC's half hours.
#[test]
fn the_default_grid_is_anchored_at_midnight_in_the_event_zone() {
    // Asia/Kathmandu is +05:45, so its local midnight is 18:15Z: a half-hour grid
    // anchored there offers starts at :15 and :45 past the UTC hour, and one
    // anchored on UTC's midnight offers :00 and :30 instead.
    let part = Part::new(
        Duration::from_minutes(30),
        Requirement::new(vec![Term::person("rich")]),
    );
    let mut request = Request::new(
        utc(2027, 6, 1, 0, 0),
        Goal::new(
            name("Asia/Kathmandu"),
            span(utc(2027, 7, 1, 0, 0), utc(2027, 7, 1, 2, 0)),
            vec![part],
        ),
    );
    request.goal.display_zone = name("UTC");
    request
        .participants
        .push(Participant::new("rich", name("Europe/London")).with_calendar(Vec::new()));
    let response = engine().find_availability(&request).unwrap();
    assert_eq!(
        offer_starts(&response),
        [
            utc(2027, 7, 1, 0, 15),
            utc(2027, 7, 1, 0, 45),
            utc(2027, 7, 1, 1, 15),
            utc(2027, 7, 1, 1, 45),
        ],
        "a half-hour grid counted from local midnight in the event zone, not from the \
         display zone's or the host's midnight"
    );
}

#[test]
fn supplied_candidates_are_the_start_list_in_order() {
    let mut part = Part::new(
        Duration::from_minutes(45),
        Requirement::new(vec![Term::person("rich")]),
    );
    part.id = "session".to_owned();
    let mut goal = Goal::new(name("UTC"), span(minutes(0), minutes(24 * 60)), vec![part]);
    goal.candidates = Some(vec![minutes(120), minutes(60), minutes(120), minutes(600)]);
    let mut request = Request::new(minutes(0), goal);
    request.participants.push(free_participant("rich"));
    let response = engine().find_availability(&request).unwrap();
    assert_eq!(
        offer_starts(&response),
        [minutes(60), minutes(120), minutes(600)],
        "candidates are sorted and deduplicated"
    );
    assert_eq!(response.offers[0].events[0].part, "session");
}

/// A search cannot report a booking that ends past the last supported instant: an
/// offer's `end` has no instant to name. Such a start is left out of the candidate
/// list rather than failing the search or reporting a reason no rule produced, and
/// the starts whose booking does fit are still offered.
#[test]
fn a_search_leaves_out_a_start_whose_booking_would_end_past_the_range() {
    let start = |back: i64| Instant::from_minutes(Instant::MAX.minutes() - back).unwrap();
    let mut request = single_host_request(
        minutes(0),
        span(start(60), Instant::MAX),
        free_participant("rich"),
        Vec::new(),
    );
    request.goal.candidates = Some(vec![start(60), start(30), start(29), start(1)]);

    let response = engine().find_availability(&request).unwrap();
    assert_eq!(offer_starts(&response), [start(60), start(30)]);

    // The same rule applies to grid-generated starts: on a five-minute grid the
    // starts from thirty minutes before the end are the last a thirty-minute
    // booking can keep inside the range.
    request.goal.candidates = None;
    request.goal.grid = Some(
        Grid::new(
            Duration::from_minutes(5),
            TimeOfDay::MIDNIGHT,
            GridZone::Named(name("UTC")),
        )
        .unwrap(),
    );
    let response = engine().find_availability(&request).unwrap();
    assert_eq!(
        offer_starts(&response),
        (6..=12)
            .rev()
            .map(|step| start(step * 5))
            .collect::<Vec<_>>()
    );
}

#[test]
fn date_counts_cover_the_whole_window_before_the_limit_truncates() {
    let mut part = Part::new(
        Duration::from_minutes(30),
        Requirement::new(vec![Term::person("rich")]),
    );
    part.id = "main".to_owned();
    let mut request = Request::new(
        minutes(0),
        Goal::new(
            name("UTC"),
            span(minutes(0), minutes(3 * 24 * 60)),
            vec![part],
        ),
    );
    request.participants.push(free_participant("rich"));
    request.limit = Some(2);
    let response = engine().find_availability(&request).unwrap();
    assert_eq!(response.offers.len(), 2);
    assert!(response.truncated);
    assert_eq!(response.dates.len(), 3, "every date the window touches");
    assert_eq!(response.dates[0].date, Date::new(1975, 1, 1).unwrap());
    assert_eq!(response.dates[2].date, Date::new(1975, 1, 3).unwrap());
    // 48 offers a day, counted before truncation.
    assert_eq!(response.dates[0].offers, 48);
    assert_eq!(response.dates[2].offers, 48);
}

#[test]
fn a_window_ending_at_midnight_does_not_add_the_next_date() {
    let request = single_host_request(
        minutes(0),
        span(minutes(0), minutes(24 * 60)),
        free_participant("rich"),
        Vec::new(),
    );
    let response = engine().find_availability(&request).unwrap();
    assert_eq!(response.dates.len(), 1);
    assert_eq!(response.dates[0].date, Date::new(1975, 1, 1).unwrap());
}

#[test]
fn an_assigned_host_without_a_calendar_flags_the_offer() {
    let unknown = Participant::new("rich", name("UTC"));
    let request = single_host_request(
        minutes(0),
        span(minutes(0), minutes(60)),
        unknown,
        Vec::new(),
    );
    let response = engine().find_availability(&request).unwrap();
    assert!(
        response
            .offers
            .iter()
            .all(opportune::model::Offer::calendar_unknown)
    );
    assert_eq!(response.offers[0].flags[0].as_str(), "calendar_unknown");

    let known = single_host_request(
        minutes(0),
        span(minutes(0), minutes(60)),
        free_participant("rich"),
        Vec::new(),
    );
    let response = engine().find_availability(&known).unwrap();
    assert!(
        response
            .offers
            .iter()
            .all(|offer| !offer.calendar_unknown())
    );
}

#[test]
fn the_response_reports_the_versions_it_was_produced_from() {
    let request = single_host_request(
        minutes(0),
        span(minutes(0), minutes(60)),
        free_participant("rich"),
        Vec::new(),
    );
    let engine = engine();
    let response = engine.find_availability(&request).unwrap();
    assert_eq!(response.meta.engine, opportune::VERSION);
    assert_eq!(response.meta.schema, 1);
    assert_eq!(response.meta.tzdb, engine.time_zone_database().version());
    assert!(!response.meta.tzdb.is_empty());
}

#[test]
fn a_pool_term_lists_every_eligible_host_and_assigns_the_first_by_id() {
    let mut rich = free_participant("rich");
    rich.tags = vec![tag("pool/frontend")];
    let mut ana = free_participant("ana");
    ana.tags = vec![tag("pool/frontend")];
    let part = Part::new(
        Duration::from_minutes(30),
        Requirement::new(vec![Term::at_least(1, Selector::Tag(tag("pool/frontend")))]),
    );
    let mut request = Request::new(
        minutes(0),
        Goal::new(name("UTC"), span(minutes(0), minutes(60)), vec![part]),
    );
    request.participants = vec![rich, ana];
    let response = engine().find_availability(&request).unwrap();
    let event = &response.offers[0].events[0];
    assert_eq!(event.eligible, [vec!["ana".to_owned(), "rich".to_owned()]]);
    assert_eq!(event.assignment, [vec!["ana".to_owned()]]);
}

#[test]
fn every_term_is_reported_and_assigned_in_its_own_list() {
    let part = Part::new(
        Duration::from_minutes(30),
        Requirement::new(vec![Term::person("ana"), Term::person("rich")]),
    );
    let mut request = Request::new(
        minutes(0),
        Goal::new(name("UTC"), span(minutes(0), minutes(60)), vec![part]),
    );
    request.participants = vec![free_participant("ana"), free_participant("rich")];
    assert!(engine().validate(&request).is_valid());
    let response = engine().find_availability(&request).unwrap();
    let event = &response.offers[0].events[0];
    assert_eq!(
        event.eligible,
        [vec!["ana".to_owned()], vec!["rich".to_owned()]]
    );
    assert_eq!(
        event.assignment,
        [vec!["ana".to_owned()], vec!["rich".to_owned()]]
    );

    // An attendee is judged for every offer but never appears in either list,
    // and a calendarless attendee raises no flag: a missing calendar is normal
    // for someone outside the organisation.
    let mut attendee = Participant::new("zoe", name("UTC"));
    attendee.role = Role::Attendee;
    request.participants.push(attendee);
    request.goal.parts[0].requirement = Requirement::new(vec![Term::person("rich")]);
    assert!(engine().validate(&request).is_valid());
    let response = engine().find_availability(&request).unwrap();
    let offer = &response.offers[0];
    let event = &offer.events[0];
    assert_eq!(event.eligible, [vec!["rich".to_owned()]]);
    assert_eq!(event.assignment, [vec!["rich".to_owned()]]);
    assert!(offer.flags.is_empty());

    let request = Request::new(
        minutes(0),
        Goal::new(name("UTC"), span(minutes(0), minutes(60)), Vec::new()),
    );
    let error = engine().find_availability(&request).unwrap_err();
    assert_eq!(error.code(), "invalid_request");
}

// --------------------------------------------------------------- validate ---

/// The error codes `validate` reports for a request, sorted.
fn error_codes(request: &Request) -> Vec<&'static str> {
    engine_codes(&engine(), request)
}

/// The codes `engine` reports for `request`, sorted.
fn engine_codes(engine: &Engine, request: &Request) -> Vec<&'static str> {
    let mut codes: Vec<&'static str> = engine
        .validate(request)
        .errors
        .iter()
        .map(opportune::Diagnostic::code)
        .collect();
    codes.sort_unstable();
    codes
}

fn warning_codes(request: &Request) -> Vec<&'static str> {
    let mut codes: Vec<&'static str> = engine()
        .validate(request)
        .warnings
        .iter()
        .map(opportune::Diagnostic::code)
        .collect();
    codes.sort_unstable();
    codes
}

#[test]
fn an_unknown_zone_is_a_validation_error() {
    let mut participant = free_participant("rich");
    participant.zone = name("Mars/Olympus_Mons");
    let request = single_host_request(
        minutes(0),
        span(minutes(0), minutes(60)),
        participant,
        Vec::new(),
    );
    assert_eq!(error_codes(&request), ["zone_unknown"]);
    let error = engine().find_availability(&request).unwrap_err();
    assert_eq!(error.code(), "invalid_request");
}

#[test]
fn a_request_beyond_the_configured_limits_is_rejected() {
    let request = single_host_request(
        minutes(0),
        span(minutes(0), minutes(60)),
        free_participant("rich"),
        Vec::new(),
    );
    let engine = Engine::builder()
        .limits(
            Limits::default()
                .with_max_participants(0)
                .with_max_search_window_days(0),
        )
        .build()
        .unwrap();
    // The size cap answers alone: the window bound belongs to the search the cap
    // refused to run.
    assert_eq!(engine_codes(&engine, &request), ["participants_limit"]);

    // With the size cap lifted, the window bound is what rejects the request.
    let engine = Engine::builder()
        .limits(Limits::default().with_max_search_window_days(0))
        .build()
        .unwrap();
    assert_eq!(engine_codes(&engine, &request), ["search_window_limit"]);
}

#[test]
fn the_offer_limit_is_not_a_validation_error() {
    let mut request = single_host_request(
        minutes(0),
        span(minutes(0), minutes(60)),
        free_participant("rich"),
        Vec::new(),
    );
    request.limit = Some(1);
    assert!(engine().validate(&request).is_valid());
    let response = engine().find_availability(&request).unwrap();
    assert_eq!(response.offers.len(), 1);
    assert!(response.truncated);
}

#[test]
fn limits_bound_blocks_and_candidate_starts() {
    let mut participant = free_participant("rich");
    participant.calendar = Some(vec![block(minutes(0), minutes(30))]);
    let mut request = single_host_request(
        minutes(0),
        span(minutes(0), minutes(60)),
        participant,
        Vec::new(),
    );

    // A block cap rejects the request before any search work runs.
    let blocks = Engine::builder()
        .limits(Limits::default().with_max_blocks_per_participant(0))
        .build()
        .unwrap();
    assert_eq!(engine_codes(&blocks, &request), ["blocks_limit"]);

    // A candidate cap is a search bound, and it bounds a grid and a caller's own
    // list the same way.
    let candidates = Engine::builder()
        .limits(Limits::default().with_max_candidate_starts(1))
        .build()
        .unwrap();
    assert_eq!(engine_codes(&candidates, &request), ["candidates_limit"]);

    request.goal.candidates = Some(vec![minutes(0), minutes(600)]);
    request.goal.grid = None;
    assert_eq!(engine_codes(&candidates, &request), ["candidates_limit"]);
}

#[test]
fn the_structural_errors_are_all_reported() {
    let mut goal_part = Part::new(
        Duration::from_minutes(30),
        Requirement::new(vec![Term::person("rich")]),
    );
    goal_part.id = "main".to_owned();
    let mut goal = Goal::new(
        name("UTC"),
        span(minutes(0), minutes(60)),
        vec![goal_part.clone(), goal_part.clone()],
    );
    goal.grid = Some(
        Grid::new(
            Duration::from_minutes(15),
            TimeOfDay::MIDNIGHT,
            GridZone::Named(name("UTC")),
        )
        .unwrap(),
    );
    goal.candidates = Some(vec![minutes(0)]);
    let mut request = Request::new(minutes(0), goal);
    request.participants = vec![free_participant("rich"), free_participant("rich")];
    request.rules = vec![
        window_rule("twice", daily_window(9, 17)),
        window_rule(
            "twice",
            WindowSpec::new(RuleZone::Named(name("UTC")), Vec::new(), Vec::new()).unwrap(),
        ),
    ];
    let codes = error_codes(&request);
    for expected in [
        "part_count",
        "grid_and_candidates",
        "participant_duplicate",
        "rule_id_duplicate",
    ] {
        assert!(
            codes.contains(&expected),
            "{expected} is reported: {codes:?}"
        );
    }
}

#[test]
fn unknown_participants_in_every_place_are_reported() {
    let mut part = Part::new(
        Duration::from_minutes(30),
        Requirement::new(vec![Term::person("nobody")]),
    );
    part.travel.by_participant.insert(
        "ghost".to_owned(),
        ParticipantTravel {
            before: Duration::ZERO,
            after: Duration::ZERO,
        },
    );
    let mut request = Request::new(
        minutes(0),
        Goal::new(name("UTC"), span(minutes(0), minutes(60)), vec![part]),
    );
    request.participants.push(free_participant("rich"));
    request.rules.push(
        window_rule("scoped", daily_window(9, 17))
            .with_who(Selector::Ids(vec!["absent".to_owned()])),
    );
    let codes = error_codes(&request);
    assert!(codes.contains(&"participant_unknown"), "{codes:?}");
    let messages: Vec<String> = engine()
        .validate(&request)
        .errors
        .iter()
        .map(|diagnostic| diagnostic.message().to_owned())
        .collect();
    for name in ["nobody", "ghost", "absent"] {
        assert!(
            messages.iter().any(|message| message.contains(name)),
            "the error names `{name}`: {messages:?}"
        );
    }
}

#[test]
fn an_unknown_calendar_and_an_empty_status_entry_are_errors() {
    let request = single_host_request(
        minutes(0),
        span(minutes(0), minutes(60)),
        free_participant("rich"),
        vec![
            Rule::new(
                "horizon",
                RuleKind::LeadTime(LeadTime {
                    min: Duration::ZERO,
                    max: Some(LeadTimeMax::WorkingDays {
                        count: 2,
                        calendar: "org".to_owned(),
                    }),
                }),
            ),
            Rule::new(
                "policy",
                RuleKind::StatusPolicy(StatusPolicy {
                    entries: vec![StatusPolicyEntry {
                        status: none(),
                        tags: None,
                        effect: StatusEffect::Ignore,
                    }],
                }),
            ),
        ],
    );
    let codes = error_codes(&request);
    assert!(codes.contains(&"calendar_unknown"), "{codes:?}");
    assert!(codes.contains(&"status_policy_entry_empty"), "{codes:?}");
}

/// A status that matches nothing, for the empty-entry case.
fn none() -> Option<BlockStatus> {
    None
}

#[test]
fn a_when_clause_that_cannot_match_is_a_warning_not_an_error() {
    let request = single_host_request(
        minutes(0),
        span(minutes(0), minutes(60)),
        free_participant("rich"),
        vec![
            window_rule("interviews", daily_window(9, 17))
                .with_when(Selector::Tag(tag("interview"))),
            window_rule("nobody", daily_window(9, 17))
                .with_who(Selector::Tag(tag("team/nonexistent"))),
        ],
    );
    let diagnostics = engine().validate(&request);
    assert!(diagnostics.is_valid(), "reachability is a warning");
    let codes: Vec<&str> = diagnostics
        .warnings
        .iter()
        .map(opportune::Diagnostic::code)
        .collect();
    assert!(codes.contains(&"when_matches_no_part"));
    assert!(codes.contains(&"selector_matches_no_participant"));
}

#[test]
fn a_window_no_candidate_falls_inside_is_a_warning() {
    let request = single_host_request(
        minutes(0),
        span(minutes(9 * 60), minutes(17 * 60)),
        free_participant("rich"),
        vec![window_rule("nights", daily_window(0, 1))],
    );
    let diagnostics = engine().validate(&request);
    assert!(diagnostics.is_valid());
    let codes: Vec<&str> = diagnostics
        .warnings
        .iter()
        .map(opportune::Diagnostic::code)
        .collect();
    assert!(codes.contains(&"no_candidate_in_window"));
    // And the search is empty rather than an error.
    let response = engine().find_availability(&request).unwrap();
    assert!(response.offers.is_empty());
}

#[test]
fn a_horizon_that_ends_before_the_first_possible_start_is_a_warning() {
    let request = single_host_request(
        minutes(0),
        span(minutes(0), minutes(24 * 60)),
        free_participant("rich"),
        vec![
            Rule::new(
                "notice",
                RuleKind::LeadTime(LeadTime {
                    min: Duration::from_minutes(120),
                    max: None,
                }),
            ),
            Rule::new(
                "horizon",
                RuleKind::LeadTime(LeadTime {
                    min: Duration::ZERO,
                    max: Some(LeadTimeMax::Duration(Duration::from_minutes(60))),
                }),
            ),
        ],
    );
    assert!(warning_codes(&request).contains(&"horizon_before_earliest_start"));
}

/// A notice so long that the earliest start is past the last supported instant
/// still leaves a horizon to compare against: a finite maximum that ends earlier
/// explains why the search can only return nothing.
#[test]
fn a_horizon_is_compared_when_the_notice_pushes_the_earliest_start_past_the_range() {
    let mut participant = free_participant("rich");
    participant.zone = name("UTC");
    let request = single_host_request(
        Instant::from_minutes(Instant::MAX.minutes() - 30).unwrap(),
        span(
            Instant::from_minutes(Instant::MAX.minutes() - 24 * 60).unwrap(),
            Instant::MAX,
        ),
        participant,
        vec![
            Rule::new(
                "notice",
                RuleKind::LeadTime(LeadTime {
                    min: Duration::from_minutes(60),
                    max: None,
                }),
            ),
            Rule::new(
                "horizon",
                RuleKind::LeadTime(LeadTime {
                    min: Duration::ZERO,
                    max: Some(LeadTimeMax::Duration(Duration::from_minutes(10))),
                }),
            ),
        ],
    );
    assert!(warning_codes(&request).contains(&"horizon_before_earliest_start"));
    let response = engine().find_availability(&request).unwrap();
    assert!(response.offers.is_empty());
}

#[test]
fn an_item_clause_naming_participants_is_an_error() {
    let request = single_host_request(
        minutes(0),
        span(minutes(0), minutes(60)),
        free_participant("rich"),
        vec![
            window_rule("scoped", daily_window(9, 17))
                .with_when(Selector::Ids(vec!["rich".to_owned()])),
        ],
    );
    assert_eq!(error_codes(&request), ["selector_not_for_items"]);
}

#[test]
fn a_requirement_no_host_can_fill_is_an_error() {
    let part = Part::new(
        Duration::from_minutes(30),
        Requirement::new(vec![Term::at_least(2, Selector::Tag(tag("pool/frontend")))]),
    );
    let mut participant = free_participant("rich");
    participant.tags = vec![tag("pool/frontend")];
    let mut request = Request::new(
        minutes(0),
        Goal::new(name("UTC"), span(minutes(0), minutes(60)), vec![part]),
    );
    request.participants.push(participant);
    assert_eq!(error_codes(&request), ["requirement_unfillable"]);
}

// --------------------------------------------------------- required ranges ---

#[test]
fn required_ranges_cover_a_trip_across_the_date_line() {
    // A participant whose home is Kiritimati (UTC+14) steps into UTC-12 for a
    // day mid-trip, so the same UTC window reads two different local dates and
    // the hull has to span both.
    let search = span(utc(2027, 3, 8, 0, 0), utc(2027, 3, 9, 0, 0));
    let mut participant = Participant::new("rich", name("Pacific/Kiritimati"));
    participant.zone_periods = vec![
        ZonePeriod::new(
            name("Etc/GMT+12"),
            utc(2027, 3, 8, 6, 0),
            utc(2027, 3, 8, 18, 0),
        )
        .unwrap(),
    ];
    let part = Part::new(
        Duration::from_minutes(30),
        Requirement::new(vec![Term::person("rich")]),
    );
    let mut query = RangesQuery::new(minutes(0), Goal::new(name("UTC"), search, vec![part]));
    query.participants.push(participant);
    let ranges = engine().required_ranges(&query).unwrap();
    assert!(ranges.occurrences.is_none(), "no cap applies");

    // In Kiritimati the padded window reads 8 March 14:00 to 9 March 14:29, so
    // its local dates run 8 to 9 March and its hull begins at midnight on 7
    // March, one day before the first. In UTC-12 the same window reads 7 March
    // 12:00 to 8 March 12:29, so its hull begins at midnight on 6 March there.
    // The hull is the earlier of the two: midnight on 7 March in Kiritimati,
    // which is 10:00Z on 6 March.
    let db = bundled_database();
    let kiritimati = db.zone("Pacific/Kiritimati").unwrap();
    let west = db.zone("Etc/GMT+12").unwrap();
    let start = ranges.calendar.start();
    assert_eq!(start, utc(2027, 3, 6, 10, 0));
    assert_eq!(
        start.zoned(&kiritimati).date(),
        Date::new(2027, 3, 7).unwrap()
    );
    assert_eq!(start.zoned(&kiritimati).time(), TimeOfDay::MIDNIGHT);

    // The hull ends after midnight on 11 March in Kiritimati and after midnight
    // on 10 March in UTC-12: the later of the two is 12:00Z on 10 March.
    let end = ranges.calendar.end();
    assert_eq!(end, utc(2027, 3, 10, 12, 0));
    assert_eq!(end.zoned(&west).date(), Date::new(2027, 3, 10).unwrap());
    assert!(end > start);
}

#[test]
fn required_ranges_clamp_at_the_supported_edges_instead_of_failing() {
    let part = Part::new(
        Duration::from_hours_minutes(1, 0),
        Requirement::new(vec![Term::person("rich")]),
    );
    let mut query = RangesQuery::new(
        Instant::MIN,
        Goal::new(
            name("UTC"),
            span(Instant::MIN, minutes(24 * 60)),
            vec![part],
        ),
    );
    query
        .participants
        .push(Participant::new("rich", name("Etc/GMT+12")));
    let ranges = engine().required_ranges(&query).unwrap();
    assert_eq!(
        ranges.calendar.start(),
        Instant::MIN,
        "a day before the first local date clamps to the first instant"
    );

    let part = Part::new(
        Duration::from_hours_minutes(1, 0),
        Requirement::new(vec![Term::person("rich")]),
    );
    let mut query = RangesQuery::new(
        Instant::MIN,
        Goal::new(name("UTC"), span(Instant::MIN, Instant::MAX), vec![part]),
    );
    query
        .participants
        .push(Participant::new("rich", name("Pacific/Kiritimati")));
    let ranges = engine().required_ranges(&query).unwrap();
    assert_eq!(ranges.calendar.end(), Instant::MAX);
}

/// A local date that begins twice — Newfoundland moved from 00:01 back to 23:01
/// the day before until 2010 — leaves the day before it running after the first
/// midnight. The hull runs to the day after the latest local date the search
/// touches, so it has to end where that day ends, which is the *last* time the
/// following date begins: ending at the first would leave a caller's busy evening
/// unfetched and read as free.
#[test]
fn the_calendar_hull_reaches_the_end_of_a_date_that_begins_twice() {
    let db = bundled_database();
    let zone = db.zone("America/St_Johns").unwrap();
    let LocalResolution::Repeated(first_midnight, second_midnight) = zone
        .resolve_local(Date::new(2010, 11, 7).unwrap(), TimeOfDay::MIDNIGHT)
        .unwrap()
    else {
        panic!("2010-11-07 begins twice in Newfoundland");
    };
    assert!(first_midnight < second_midnight);

    let part = Part::new(
        Duration::from_minutes(30),
        Requirement::new(vec![Term::person("rich")]),
    );
    // The search ends on 2010-11-05 locally, so the hull reaches one day past that
    // date: 2010-11-07, the day that begins twice.
    let mut query = RangesQuery::new(
        utc(2010, 11, 4, 0, 0),
        Goal::new(
            name("UTC"),
            span(utc(2010, 11, 4, 0, 0), utc(2010, 11, 6, 0, 0)),
            vec![part],
        ),
    );
    query
        .participants
        .push(Participant::new("rich", name("America/St_Johns")));

    let ranges = engine().required_ranges(&query).unwrap();
    assert_eq!(
        ranges.calendar.end(),
        second_midnight,
        "2010-11-06 ends where 2010-11-07 begins for the last time"
    );
}

#[test]
fn required_ranges_report_the_parts_that_do_not_have_one() {
    let query = RangesQuery::new(
        Instant::MIN,
        Goal::new(name("UTC"), span(Instant::MIN, Instant::MAX), Vec::new()),
    );
    let error = engine().required_ranges(&query).unwrap_err();
    assert_eq!(error.code(), "part_count");
}

#[test]
fn a_goal_can_hold_a_second_part_and_only_this_version_rejects_it() {
    // The shapes are ready for sequences: a goal is a list of parts, an offer a
    // list of events, and an event a list of term lists. A later version that
    // books several parts adds parts rather than reshaping any of them; version 1
    // rejects a second part with `part_count`, which is a rule of this version
    // rather than a limit of the types.
    let zone = name("UTC");
    let window = span(
        Instant::MIN,
        Instant::from_minutes(Instant::MIN.minutes() + 60).unwrap(),
    );
    let part = || {
        Part::new(
            Duration::from_minutes(30),
            Requirement::new(vec![Term::person("rich")]),
        )
    };
    let goal = Goal::new(zone.clone(), window, vec![part(), part()]);
    assert_eq!(goal.parts.len(), 2, "the goal holds both parts");

    let mut request = Request::new(Instant::MIN, goal);
    request.participants.push(free_participant("rich"));
    let codes: Vec<&str> = engine()
        .validate(&request)
        .errors
        .iter()
        .map(opportune::Diagnostic::code)
        .collect();
    assert_eq!(codes, ["part_count"]);
    assert_eq!(
        engine().find_availability(&request).unwrap_err().code(),
        "invalid_request"
    );
}

// ------------------------------------------------------------------ engine ---

#[test]
fn the_builder_uses_the_bundled_database_by_default() {
    let engine = Engine::builder().build().unwrap();
    assert!(!engine.time_zone_database().version().is_empty());
    let explicit = Engine::builder().tzdb_bundled().build().unwrap();
    assert_eq!(
        engine.time_zone_database().version(),
        explicit.time_zone_database().version()
    );
}

#[test]
fn occurrences_are_carried_but_do_not_change_a_search_without_caps() {
    let request = single_host_request(
        minutes(0),
        span(minutes(0), minutes(120)),
        free_participant("rich"),
        Vec::new(),
    );
    let mut with_occurrences = request.clone();
    with_occurrences.occurrences.push(
        Occurrence::new(
            "evt_91",
            span(minutes(0), minutes(60)),
            vec!["rich".to_owned()],
        )
        .unwrap(),
    );
    assert_eq!(
        engine()
            .find_availability(&with_occurrences)
            .unwrap()
            .offers,
        engine().find_availability(&request).unwrap().offers
    );
    // An occurrence nobody attends is reported, though.
    let mut invalid = request;
    let mut occurrence = Occurrence::new(
        "evt_92",
        span(minutes(0), minutes(60)),
        vec!["rich".to_owned()],
    )
    .unwrap();
    occurrence.participants.clear();
    invalid.occurrences.push(occurrence);
    assert!(
        error_codes(&invalid).contains(&"occurrence_without_participants"),
        "{:?}",
        error_codes(&invalid)
    );
}

#[test]
fn a_date_override_replaces_the_weekly_hours_for_that_date() {
    let override_key = DateKey::new(
        DateRange::single(Date::new(1975, 1, 1).unwrap()),
        vec![
            WallRange::new(
                TimeOfDay::new(14, 0).unwrap(),
                WallEnd::Time(TimeOfDay::new(15, 0).unwrap()),
            )
            .unwrap(),
        ],
    )
    .unwrap();
    let spec = WindowSpec::new(
        RuleZone::Named(name("UTC")),
        vec![
            WeeklyKey::new(
                Weekdays::range(Weekday::Monday, Weekday::Sunday),
                vec![
                    WallRange::new(
                        TimeOfDay::new(9, 0).unwrap(),
                        WallEnd::Time(TimeOfDay::new(17, 0).unwrap()),
                    )
                    .unwrap(),
                ],
            )
            .unwrap(),
        ],
        vec![override_key],
    )
    .unwrap();
    let request = single_host_request(
        minutes(0),
        span(minutes(0), minutes(24 * 60)),
        free_participant("rich"),
        vec![window_rule("hours", spec)],
    );
    let response = engine().find_availability(&request).unwrap();
    let starts = offer_starts(&response);
    assert_eq!(starts, [minutes(14 * 60), minutes(14 * 60 + 30)]);
}

#[test]
fn a_zone_period_switch_mid_window_reads_each_zones_own_clock() {
    let mut participant = free_participant("rich");
    participant.zone = name("Asia/Tokyo");
    participant.zone_periods = vec![
        ZonePeriod::new(
            name("America/Los_Angeles"),
            utc(2027, 3, 9, 16, 0),
            utc(2027, 3, 10, 0, 0),
        )
        .unwrap(),
    ];
    let request = single_host_request(
        minutes(0),
        span(utc(2027, 3, 9, 0, 0), utc(2027, 3, 9, 19, 0)),
        participant,
        vec![window_rule(
            "hours",
            daily_window_in(RuleZone::Participant, 9, 10),
        )],
    );
    let response = engine().find_availability(&request).unwrap();
    let starts = offer_starts(&response);
    // Tokyo's 09:00-10:00 is 00:00Z-01:00Z, and Los Angeles' is 17:00Z-18:00Z.
    assert!(starts.contains(&utc(2027, 3, 9, 0, 0)));
    assert!(starts.contains(&utc(2027, 3, 9, 17, 0)));
    assert!(!starts.contains(&utc(2027, 3, 9, 12, 0)));
}

#[test]
fn a_status_policy_can_make_a_free_block_block() {
    let mut participant = free_participant("rich");
    let mut held = block(minutes(10 * 60), minutes(11 * 60));
    held.status = BlockStatus::Free;
    held.tags = vec![tag("hold/checkout")];
    participant.calendar = Some(vec![held]);
    let request = single_host_request(
        minutes(0),
        span(minutes(9 * 60), minutes(12 * 60)),
        participant,
        vec![Rule::new(
            "holds-block",
            RuleKind::StatusPolicy(StatusPolicy {
                entries: vec![StatusPolicyEntry::tags(
                    Selector::Tag(tag("hold")),
                    StatusEffect::Block,
                )],
            }),
        )],
    );
    let response = engine().find_availability(&request).unwrap();
    let starts = offer_starts(&response);
    assert!(!starts.contains(&minutes(10 * 60)));
    assert!(!starts.contains(&minutes(10 * 60 + 30)));
    assert!(starts.contains(&minutes(9 * 60)));
}

#[test]
fn allowed_time_is_not_clipped_to_the_search_window() {
    // The booking starts inside the search window and ends past it, inside the
    // window rule. A builder that clipped allowed time to the search window would
    // find nowhere for the booking to go.
    let mut part = Part::new(
        Duration::from_minutes(60),
        Requirement::new(vec![Term::person("rich")]),
    );
    part.id = "main".to_owned();
    let mut request = Request::new(
        minutes(0),
        Goal::new(
            name("UTC"),
            span(minutes(12 * 60), minutes(12 * 60 + 30)),
            vec![part],
        ),
    );
    request.participants.push(free_participant("rich"));
    request
        .rules
        .push(window_rule("hours", daily_window(9, 13)));
    let response = engine().find_availability(&request).unwrap();
    assert_eq!(response.offers.len(), 1);
    assert_eq!(response.offers[0].start(), Some(minutes(12 * 60)));
    assert_eq!(response.offers[0].events[0].end, minutes(13 * 60));
}

#[test]
fn a_per_participant_travel_override_replaces_the_parts_travel() {
    let mut rich = free_participant("rich");
    rich.tags = vec![tag("pool/frontend")];
    let mut ana = free_participant("ana");
    ana.tags = vec![tag("pool/frontend")];
    let mut part = Part::new(
        Duration::from_minutes(30),
        Requirement::new(vec![Term::at_least(1, Selector::Tag(tag("pool/frontend")))]),
    );
    part.travel = Travel {
        before: Duration::from_minutes(10),
        after: Duration::from_minutes(0),
        by_participant: [(
            "ana".to_owned(),
            ParticipantTravel {
                before: Duration::from_minutes(40),
                after: Duration::from_minutes(20),
            },
        )]
        .into_iter()
        .collect(),
    };
    let mut request = Request::new(
        minutes(0),
        Goal::new(
            name("UTC"),
            span(minutes(9 * 60), minutes(11 * 60)),
            vec![part],
        ),
    );
    request.participants = vec![rich, ana];
    request
        .rules
        .push(window_rule("hours", daily_window(9, 11)));

    // At 09:30 Ana's own travel of 40 minutes puts her occupied start at 08:50,
    // before the window, so only Rich can take it.
    request.goal.candidates = Some(vec![minutes(9 * 60 + 30)]);
    let response = engine().find_availability(&request).unwrap();
    let event = &response.offers[0].events[0];
    assert_eq!(event.eligible, [vec!["rich".to_owned()]]);
    assert_eq!(event.assignment, [vec!["rich".to_owned()]]);
    assert_eq!(
        event.reserve.len(),
        1,
        "Rich's part travel of 10 minutes is reserved"
    );

    // At 09:40 Ana's occupied span fits, and she is assigned: the first host by
    // ID, with both of her travel spans reserved.
    request.goal.candidates = Some(vec![minutes(9 * 60 + 40)]);
    let response = engine().find_availability(&request).unwrap();
    let event = &response.offers[0].events[0];
    assert_eq!(event.eligible, [vec!["ana".to_owned(), "rich".to_owned()]]);
    assert_eq!(event.assignment, [vec!["ana".to_owned()]]);
    assert_eq!(
        event
            .reserve
            .iter()
            .map(|span| (
                span.participant.as_str(),
                span.span.start(),
                span.span.end()
            ))
            .collect::<Vec<_>>(),
        [
            ("ana", minutes(9 * 60), minutes(9 * 60 + 40)),
            ("ana", minutes(10 * 60 + 10), minutes(10 * 60 + 30)),
        ],
        "both travel spans are reserved, sorted by start"
    );
}

#[test]
fn the_default_offer_limit_is_one_thousand() {
    let mut goal = Goal::new(
        name("UTC"),
        span(minutes(0), minutes(24 * 60)),
        vec![Part::new(
            Duration::from_minutes(1),
            Requirement::new(vec![Term::person("rich")]),
        )],
    );
    goal.grid = Some(
        Grid::new(
            Duration::from_minutes(1),
            TimeOfDay::MIDNIGHT,
            GridZone::Named(name("UTC")),
        )
        .unwrap(),
    );
    let mut request = Request::new(minutes(0), goal);
    request.participants.push(free_participant("rich"));
    let response = engine().find_availability(&request).unwrap();
    assert_eq!(response.offers.len(), 1_000);
    assert!(response.truncated);
    assert_eq!(response.dates[0].offers, 1_440);
}

#[test]
fn a_date_maximum_ends_at_the_end_of_that_local_date() {
    let now = utc(1975, 1, 1, 0, 0);
    let request = single_host_request(
        now,
        span(now, utc(1975, 1, 5, 0, 0)),
        free_participant("rich"),
        vec![Rule::new(
            "until-friday",
            RuleKind::LeadTime(LeadTime {
                min: Duration::ZERO,
                max: Some(LeadTimeMax::Date(Date::new(1975, 1, 3).unwrap())),
            }),
        )],
    );
    let response = engine().find_availability(&request).unwrap();
    let starts = offer_starts(&response);
    assert!(starts.contains(&utc(1975, 1, 3, 23, 30)));
    assert!(!starts.contains(&utc(1975, 1, 4, 0, 0)));
}

#[test]
fn an_occurrence_naming_participants_outside_the_request_is_accepted() {
    let mut request = single_host_request(
        minutes(0),
        span(minutes(0), minutes(60)),
        free_participant("rich"),
        Vec::new(),
    );
    request.occurrences.push(
        Occurrence::new(
            "evt_91",
            span(minutes(0), minutes(60)),
            vec!["rich".to_owned(), "someone-else".to_owned()],
        )
        .unwrap(),
    );
    assert!(
        engine().validate(&request).is_valid(),
        "real meetings include people outside the request"
    );
}

#[test]
fn dates_are_the_display_zones_civil_dates_that_the_window_touches() {
    let mut part = Part::new(
        Duration::from_minutes(30),
        Requirement::new(vec![Term::person("rich")]),
    );
    part.id = "main".to_owned();
    let mut goal = Goal::new(
        name("America/New_York"),
        span(utc(2027, 3, 8, 0, 0), utc(2027, 3, 9, 0, 0)),
        vec![part],
    );
    goal.display_zone = name("America/New_York");
    let mut request = Request::new(utc(2027, 3, 8, 0, 0), goal);
    request
        .participants
        .push(Participant::new("rich", name("America/New_York")));
    let response = engine().find_availability(&request).unwrap();
    // 00:00Z on the 8th is 19:00 on the 7th in New York, so the window touches
    // two civil dates there.
    let dates: Vec<Date> = response.dates.iter().map(|count| count.date).collect();
    assert_eq!(
        dates,
        [
            Date::new(2027, 3, 7).unwrap(),
            Date::new(2027, 3, 8).unwrap()
        ]
    );
    // The default grid is read in the event zone too: local midnight to local
    // midnight is 23 hours on a spring-forward day, which 8 March is not, so 46
    // half-hour starts from the first start inside the window.
    assert_eq!(
        response
            .dates
            .iter()
            .map(|count| count.offers)
            .sum::<usize>(),
        response.offers.len()
    );
}

#[test]
fn the_response_carries_the_validation_warnings() {
    let request = single_host_request(
        minutes(0),
        span(minutes(9 * 60), minutes(17 * 60)),
        free_participant("rich"),
        vec![window_rule("nights", daily_window(0, 1))],
    );
    let response = engine().find_availability(&request).unwrap();
    assert!(
        response
            .warnings
            .iter()
            .any(|warning| warning.code() == "no_candidate_in_window"),
        "{:?}",
        response.warnings
    );
}

#[test]
fn overlapping_zone_periods_and_an_empty_working_calendar_are_errors() {
    let mut participant = free_participant("rich");
    participant.zone_periods = vec![
        ZonePeriod::new(name("America/Anchorage"), minutes(0), minutes(600)).unwrap(),
        ZonePeriod::new(name("Europe/London"), minutes(300), minutes(900)).unwrap(),
    ];
    let mut request = single_host_request(
        minutes(0),
        span(minutes(0), minutes(60)),
        participant,
        Vec::new(),
    );
    request.calendars.insert(
        "never".to_owned(),
        CalendarSpec {
            zone: name("UTC"),
            working_days: WorkingDays::NONE,
            holidays: Vec::new(),
        },
    );
    let codes = error_codes(&request);
    assert!(codes.contains(&"zone_periods_overlap"), "{codes:?}");
    assert!(codes.contains(&"working_days_empty"), "{codes:?}");
}

/// The free functions delegate to a default engine with the bundled database and
/// the default limits, so a caller who does not need to choose either has the same
/// answers as one who builds an engine explicitly.
#[test]
fn the_free_functions_agree_with_an_explicit_default_engine() {
    let request = single_host_request(
        minutes(0),
        span(minutes(0), minutes(120)),
        free_participant("rich"),
        vec![window_rule("hours", daily_window(9, 11))],
    );
    let engine = engine();
    assert_eq!(
        opportune::find_availability(&request).unwrap(),
        engine.find_availability(&request).unwrap()
    );
    assert_eq!(opportune::validate(&request), engine.validate(&request));

    let mut query = RangesQuery::new(minutes(0), request.goal.clone());
    query.participants = request.participants.clone();
    assert_eq!(
        opportune::required_ranges(&query).unwrap(),
        engine.required_ranges(&query).unwrap()
    );
}

#[test]
fn a_term_matching_only_an_attendee_is_unfillable() {
    let mut attendee = free_participant("tom");
    attendee.role = Role::Attendee;
    attendee.tags = vec![tag("pool/frontend")];
    let part = Part::new(
        Duration::from_minutes(30),
        Requirement::new(vec![Term::at_least(1, Selector::Tag(tag("pool/frontend")))]),
    );
    let mut request = Request::new(
        minutes(0),
        Goal::new(name("UTC"), span(minutes(0), minutes(60)), vec![part]),
    );
    request.participants = vec![free_participant("rich"), attendee];
    // An attendee is never assigned, so the term cannot be filled even though
    // the tags match.
    assert_eq!(error_codes(&request), ["requirement_unfillable"]);
}

#[test]
fn a_requirement_with_no_host_slot_is_a_validation_error() {
    // `Requirement::default()` is a public constructor, so a part whose terms
    // the caller has not filled in yet can reach the engine.
    let part = Part::new(Duration::from_minutes(30), Requirement::default());
    let mut request = Request::new(
        minutes(0),
        Goal::new(name("UTC"), span(minutes(0), minutes(60)), vec![part]),
    );
    request.participants.push(free_participant("rich"));
    // Nobody can be booked, so `validate` reports it rather than leaving the
    // refusal to the search.
    assert_eq!(error_codes(&request), ["requirement_unfillable"]);
    assert_eq!(
        engine().find_availability(&request).unwrap_err().code(),
        "invalid_request"
    );
}

#[test]
fn a_term_asking_for_no_host_still_gets_its_own_empty_list() {
    // `ana` sorts first by ID, so an evaluator that assigned from the first term
    // alone would book her for a term that asks for nobody. The response keeps
    // one list per term, and the term asking for no host assigns nobody.
    let part = Part::new(
        Duration::from_minutes(30),
        Requirement::new(vec![Term::at_least(0, Selector::True), Term::person("zoe")]),
    );
    let mut request = Request::new(
        minutes(0),
        Goal::new(
            name("UTC"),
            span(minutes(9 * 60), minutes(10 * 60)),
            vec![part],
        ),
    );
    request.participants = vec![free_participant("ana"), free_participant("zoe")];
    let response = engine().find_availability(&request).unwrap();
    let event = &response.offers[0].events[0];
    assert_eq!(event.assignment, [Vec::new(), vec!["zoe".to_owned()]]);
}

#[test]
fn a_requirement_selector_naming_an_absent_tag_is_warned_about() {
    // The requirement is the only participant selector in the request, and it
    // names a tag nobody carries, so the term silently matches every host
    // under `not` unless the caller is told.
    let part = Part::new(
        Duration::from_minutes(30),
        Requirement::new(vec![Term::at_least(
            1,
            Selector::Not(Box::new(Selector::Tag(tag("pool/oncall")))),
        )]),
    );
    let mut request = Request::new(
        minutes(0),
        Goal::new(name("UTC"), span(minutes(0), minutes(60)), vec![part]),
    );
    request.participants.push(free_participant("rich"));
    assert!(engine().validate(&request).is_valid());
    assert!(
        warning_codes(&request).contains(&"selector_matches_no_participant"),
        "{:?}",
        warning_codes(&request)
    );
}

#[test]
fn a_part_with_no_length_is_rejected() {
    let part = Part::new(
        Duration::from_minutes(0),
        Requirement::new(vec![Term::person("rich")]),
    );
    let mut request = Request::new(
        minutes(0),
        Goal::new(name("UTC"), span(minutes(0), minutes(60)), vec![part]),
    );
    request.participants.push(free_participant("rich"));
    assert_eq!(error_codes(&request), ["part_duration_empty"]);
    assert_eq!(
        engine().find_availability(&request).unwrap_err().code(),
        "invalid_request"
    );
}

#[test]
fn required_ranges_pad_by_clearance_as_well_as_travel() {
    let now = utc(1975, 1, 1, 0, 0);
    let part = Part::new(
        Duration::from_minutes(30),
        Requirement::new(vec![Term::person("rich")]),
    );
    let mut goal = Goal::new(name("UTC"), span(now, utc(1975, 1, 1, 22, 0)), vec![part]);
    goal.display_zone = name("UTC");
    let mut query = RangesQuery::new(now, goal);
    query
        .participants
        .push(Participant::new("rich", name("UTC")));

    // The 30-minute booking keeps the padded window inside 1 January, so the
    // hull ends after the day that follows it.
    let plain = engine().required_ranges(&query).unwrap();
    assert_eq!(plain.calendar.end(), utc(1975, 1, 3, 0, 0));

    // A two-hour clearance after the booking reaches into 2 January, so the hull
    // has to cover that date and the day after it.
    query.rules.push(Rule::new(
        "buffers",
        RuleKind::Clearance(opportune::model::Clearance {
            before: Duration::from_minutes(0),
            after: Duration::from_hours_minutes(2, 0),
            min_gap: Duration::from_minutes(0),
        }),
    ));
    let padded = engine().required_ranges(&query).unwrap();
    assert_eq!(padded.calendar.end(), utc(1975, 1, 4, 0, 0));
    assert!(padded.calendar.minutes() > plain.calendar.minutes());
}

#[test]
fn with_no_lead_time_rule_nothing_is_offered_before_now() {
    // `now` is 10:00 and the search window opens an hour earlier: the implicit
    // minimum notice of zero still keeps starts before `now` out, and the
    // booking's travel counts against it.
    let mut request = single_host_request(
        minutes(10 * 60),
        span(minutes(9 * 60), minutes(12 * 60)),
        free_participant("rich"),
        Vec::new(),
    );
    request.goal.parts[0].travel = Travel {
        before: Duration::from_minutes(30),
        after: Duration::from_minutes(0),
        by_participant: BTreeMap::new(),
    };
    let response = engine().find_availability(&request).unwrap();
    assert_eq!(
        response
            .offers
            .first()
            .and_then(opportune::model::Offer::start),
        Some(minutes(10 * 60 + 30))
    );
    assert!(
        response
            .offers
            .iter()
            .all(|offer| offer.start().unwrap() >= minutes(10 * 60 + 30))
    );
}

#[test]
fn travel_is_occupied_time_so_it_cannot_overlap_a_blackout_or_a_block() {
    let blackout = BlackoutSpec::new(
        RuleZone::Named(name("UTC")),
        vec![
            BlackoutSpan::new(
                LocalDateTime::new(
                    Date::new(1975, 1, 1).unwrap(),
                    TimeOfDay::new(9, 0).unwrap(),
                ),
                LocalDateTime::new(
                    Date::new(1975, 1, 1).unwrap(),
                    TimeOfDay::new(9, 30).unwrap(),
                ),
            )
            .unwrap(),
        ],
    );
    let mut participant = free_participant("rich");
    participant.calendar = Some(vec![block(minutes(10 * 60), minutes(11 * 60))]);
    let mut request = single_host_request(
        minutes(0),
        span(minutes(9 * 60), minutes(12 * 60)),
        participant,
        vec![
            window_rule("hours", daily_window(9, 12)),
            blackout_rule("closed", blackout),
        ],
    );
    request.goal.parts[0].travel = Travel {
        before: Duration::from_minutes(15),
        after: Duration::from_minutes(0),
        by_participant: BTreeMap::new(),
    };
    request.goal.candidates = Some(vec![
        minutes(9 * 60 + 20),
        minutes(10 * 60 + 10),
        minutes(11 * 60 + 15),
    ]);
    let response = engine().find_availability(&request).unwrap();
    let starts = offer_starts(&response);
    // 09:20's travel reaches into the blackout and 10:10's reaches into the
    // block; both booking spans are clear of each on their own, so only the
    // travel span's occupancy excludes them.
    assert_eq!(starts, [minutes(11 * 60 + 15)]);
}
