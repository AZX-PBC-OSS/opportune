//! End-to-end tests for the `cap` rule kind and the occurrences it counts: what
//! one event is after event-key merging, how `each` and `pooled` limits combine,
//! how count and minute measures differ, where the reason sits in the precedence
//! order, and that `find_availability`, `check_slot`, `validate` and
//! `required_ranges` agree.
//!
//! Every test builds a real request and runs it on a real engine, so deleting the
//! behaviour it covers turns it red.

#![cfg(feature = "bundled-tzdb")]

use opportune::Engine;
use opportune::model::{
    Block, BlockStatus, Cap, CapMeasure, CapPer, CapPeriod, Explain, Goal, Occurrence, Participant,
    Proposal, ProposalEvent, RangesQuery, Request, Requirement, Rule, RuleKind, Span, StatusEffect,
    StatusPolicy, StatusPolicyEntry, Term, ZonePeriod,
};
use opportune::presets;
use opportune::reasons::{ReasonCode, ReasonParams};
use opportune::tags::{Selector, Tag};
use opportune::time::{
    Date, Duration, Instant, LocalResolution, RuleZone, TimeOfDay, Weekday, Zone, ZoneName,
};

/// Monday 14 June 2027, the fixture's first day.
const MONDAY: (i16, i8, i8) = (2027, 6, 14);
/// Tuesday 15 June 2027, the fixture's second day.
const TUESDAY: (i16, i8, i8) = (2027, 6, 15);
/// Wednesday 16 June 2027, the day after the fixture's second day.
const WEDNESDAY: (i16, i8, i8) = (2027, 6, 16);

fn engine() -> Engine {
    Engine::builder()
        .build()
        .expect("the bundled database builds")
}

fn name(zone: &str) -> ZoneName {
    ZoneName::new(zone).expect("a fixture zone is a zone name")
}

fn tag(path: &str) -> Tag {
    Tag::new(path).expect("a fixture tag is a valid tag")
}

fn time(hour: u8, minute: u8) -> TimeOfDay {
    TimeOfDay::new(hour, minute).expect("a fixture time is a time of day")
}

/// The instant a local date-time falls on in `zone`, when it exists once.
fn local(zone: &str, day: (i16, i8, i8), hour: u8, minute: u8) -> Instant {
    let resolved = engine()
        .time_zone_database()
        .zone(zone)
        .expect("a fixture zone is in the bundled database");
    let date = Date::new(day.0, day.1, day.2).expect("a fixture date is a real date");
    match resolved
        .resolve_local(date, time(hour, minute))
        .expect("the fixture instant is inside the supported range")
    {
        LocalResolution::Unique(instant) => instant,
        LocalResolution::Skipped | LocalResolution::Repeated(_, _) => {
            panic!(
                "{date} {} in {zone} must exist exactly once",
                time(hour, minute)
            );
        }
    }
}

/// The UTC instant of a local date-time.
fn utc(day: (i16, i8, i8), hour: u8, minute: u8) -> Instant {
    let LocalResolution::Unique(instant) = Zone::utc()
        .resolve_local(
            Date::new(day.0, day.1, day.2).expect("a fixture date is a real date"),
            time(hour, minute),
        )
        .expect("the fixture instant is inside the supported range")
    else {
        panic!("UTC has no transitions");
    };
    instant
}

fn span(start: Instant, end: Instant) -> Span {
    Span::new(start, end).expect("the fixture span is non-empty")
}

fn block(start: Instant, end: Instant) -> Block {
    Block::new(span(start, end))
}

/// A busy block over `[start, end)` carrying `tags`.
fn tagged_block(start: Instant, end: Instant, tags: &[&str]) -> Block {
    let mut block = block(start, end);
    block.tags = tags.iter().map(|path| tag(path)).collect();
    block
}

/// A `cap` rule counting `counted` events.
fn cap_rule(
    id: &str,
    counted: &str,
    measure: CapMeasure,
    limit: u32,
    period: CapPeriod,
    per: CapPer,
    zone: RuleZone,
) -> Rule {
    Rule::new(
        id,
        RuleKind::Cap(Cap::new(
            Selector::Tag(tag(counted)),
            measure,
            limit,
            period,
            per,
            zone,
            Weekday::Monday,
        )),
    )
}

/// A host in UTC with the given blocks.
fn host(id: &str, blocks: Vec<Block>) -> Participant {
    let mut participant = Participant::new(id, name("UTC"));
    participant.calendar = Some(blocks);
    participant
}

/// A request with the given search, candidates, participants, rules and part
/// tags, booking a 30-minute event and explaining every exclusion.
fn request(
    search: Span,
    candidates: Vec<Instant>,
    participants: Vec<Participant>,
    rules: Vec<Rule>,
    part_tags: &[&str],
) -> Request {
    let mut goal = Goal::single(
        name("UTC"),
        search,
        Duration::from_minutes(30),
        Requirement::new(vec![Term::person("ana")]),
    );
    goal.candidates = Some(candidates);
    goal.parts[0].tags = part_tags.iter().map(|path| tag(path)).collect();
    let mut request = Request::new(utc(MONDAY, 0, 0), goal);
    request.participants = participants;
    request.rules = rules;
    request.explain = Some(Explain::new(10_000));
    request
}

/// The search window covering both fixture days' afternoons.
fn two_day_search() -> Span {
    span(utc(MONDAY, 14, 0), utc(TUESDAY, 15, 0))
}

fn proposal(start: Instant, assigned: &[&str]) -> Proposal {
    Proposal {
        events: vec![ProposalEvent {
            part: "main".to_owned(),
            start,
            assignment: vec![assigned.iter().map(|id| (*id).to_owned()).collect()],
        }],
    }
}

fn offered(response: &opportune::model::Response) -> Vec<Instant> {
    response
        .offers
        .iter()
        .map(|offer| offer.start().expect("an offer has an event"))
        .collect()
}

/// Every `cap_reached` reason an exclusion reports at `start`: the rule that set
/// it, the participant it names, the usage, the limit and the bucket's label.
fn cap_reasons(
    response: &opportune::model::Response,
    start: Instant,
) -> Vec<(&str, &str, u64, u32, &str)> {
    let Some(excluded) = response.excluded.iter().find(|entry| entry.start == start) else {
        return Vec::new();
    };
    excluded
        .reasons
        .iter()
        .filter(|reason| reason.code == ReasonCode::CapReached)
        .filter_map(|reason| {
            let ReasonParams::CapReached {
                used,
                limit,
                bucket,
            } = reason.params.as_ref()?
            else {
                return None;
            };
            Some((
                reason.rule.as_deref()?,
                reason.participant.as_deref()?,
                *used,
                *limit,
                bucket.as_str(),
            ))
        })
        .collect()
}

/// The parameters of the first `cap_reached` reason an exclusion reports for
/// `participant`.
fn cap_params(
    response: &opportune::model::Response,
    start: Instant,
    participant: &str,
) -> Option<(u64, u32, String)> {
    cap_reasons(response, start)
        .into_iter()
        .find(|reason| reason.1 == participant)
        .map(|(_, _, used, limit, bucket)| (used, limit, bucket.to_owned()))
}

#[test]
fn the_cap_per_day_preset_builds_a_daily_each_cap() {
    let rule = presets::cap_per_day(Selector::Tag(tag("interview")), 2);
    assert_eq!(rule.id, "cap_per_day");
    let cap = rule.cap().expect("the preset writes a cap rule");
    assert_eq!(cap.limit, 2);
    assert_eq!(cap.measure, CapMeasure::Count);
    assert_eq!(cap.period, CapPeriod::Day);
    assert_eq!(cap.per, CapPer::Each);
    assert_eq!(cap.zone, RuleZone::Participant);
    assert_eq!(cap.week_starts, Weekday::Monday);
}

/// The minute-cap presets name their own period, and the engine bounds the bucket
/// each one names: the same spent hour fills a day, a week or a month.
#[test]
fn the_minute_cap_presets_bound_the_period_each_names() {
    let demo = || Selector::Tag(tag("sales/demo"));
    let spent = tagged_block(utc(MONDAY, 9, 0), utc(MONDAY, 10, 0), &["sales/demo"]);
    let engine = engine();

    let cases = [
        (
            "cap_minutes_per_day",
            CapPeriod::Day,
            presets::cap_minutes_per_day(demo(), 60, CapPer::Each, RuleZone::Participant),
            utc(MONDAY, 14, 0),
            "2027-06-14",
        ),
        (
            "cap_minutes_per_week",
            CapPeriod::Week,
            presets::cap_minutes_per_week(demo(), 60, CapPer::Each, RuleZone::Participant),
            utc(TUESDAY, 14, 0),
            "2027-06-14",
        ),
        (
            "cap_minutes_per_month",
            CapPeriod::Month,
            presets::cap_minutes_per_month(demo(), 60, CapPer::Each, RuleZone::Participant),
            utc(TUESDAY, 14, 0),
            "2027-06-01",
        ),
    ];
    for (id, period, rule, candidate, bucket) in cases {
        assert_eq!(rule.id, id);
        let cap = rule.cap().expect("the preset writes a cap rule");
        assert_eq!(
            (
                cap.measure,
                cap.period,
                cap.limit,
                cap.per,
                cap.zone.clone()
            ),
            (
                CapMeasure::Minutes,
                period,
                60,
                CapPer::Each,
                RuleZone::Participant
            ),
            "{id}"
        );

        let request = request(
            span(utc(MONDAY, 9, 0), utc(WEDNESDAY, 15, 0)),
            vec![candidate],
            vec![host("ana", vec![spent.clone()])],
            vec![rule],
            &["sales/demo"],
        );
        assert!(engine.validate(&request).is_valid(), "{id}");
        let response = engine.find_availability(&request).unwrap();
        assert!(
            offered(&response).is_empty(),
            "{id}: the spent hour fills this bucket"
        );
        assert_eq!(
            cap_reasons(&response, candidate),
            [(id, "ana", 60, 60, bucket)],
            "{id}"
        );
    }
}

#[test]
fn an_each_cap_excludes_only_the_day_the_participant_has_filled() {
    // Two interviews on Monday fill a two-a-day cap, so Monday's candidate is
    // excluded and Tuesday's — a fresh bucket — is offered. The reason carries
    // the bucket's usage, limit and label.
    let request = request(
        two_day_search(),
        vec![utc(MONDAY, 14, 0), utc(TUESDAY, 14, 0)],
        vec![host(
            "ana",
            vec![
                tagged_block(utc(MONDAY, 9, 0), utc(MONDAY, 10, 0), &["interview"]),
                tagged_block(utc(MONDAY, 10, 0), utc(MONDAY, 11, 0), &["interview"]),
            ],
        )],
        vec![cap_rule(
            "each_day",
            "interview",
            CapMeasure::Count,
            2,
            CapPeriod::Day,
            CapPer::Each,
            RuleZone::Participant,
        )],
        &["interview"],
    );
    let engine = engine();
    assert!(engine.validate(&request).is_valid());
    let response = engine.find_availability(&request).unwrap();
    assert_eq!(offered(&response), [utc(TUESDAY, 14, 0)]);
    assert_eq!(
        cap_params(&response, utc(MONDAY, 14, 0), "ana"),
        Some((2, 2, "2027-06-14".to_owned()))
    );

    // `check_slot` reports the same cap for the excluded start.
    let check = engine
        .check_slot(&request, &proposal(utc(MONDAY, 14, 0), &["ana"]))
        .unwrap();
    assert!(!check.valid);
    let reason = check
        .reasons
        .iter()
        .find(|reason| reason.code == ReasonCode::CapReached)
        .expect("the cap excludes the slot");
    assert_eq!(reason.rule.as_deref(), Some("each_day"));
    assert_eq!(
        reason.params.as_ref().and_then(|params| match params {
            ReasonParams::CapReached { used, .. } => Some(*used),
            _ => None,
        }),
        Some(2)
    );
}

#[test]
fn a_pooled_cap_counts_an_event_once_however_many_calendars_hold_it() {
    // The same event, marked with one key, sits on both interviewers' calendars.
    // A pooled cap counts it once, so the single event it already holds is what
    // the reason reports, not two.
    let mut on_ana = tagged_block(utc(MONDAY, 9, 0), utc(MONDAY, 10, 0), &["interview"]);
    on_ana.event_key = Some("evt_91".to_owned());
    let mut on_bo = on_ana.clone();
    on_bo.span = span(utc(MONDAY, 9, 0), utc(MONDAY, 10, 0));
    let mut request = request(
        two_day_search(),
        vec![utc(MONDAY, 14, 0)],
        vec![host("ana", vec![on_ana]), host("bo", vec![on_bo])],
        vec![cap_rule(
            "pool_day",
            "interview",
            CapMeasure::Count,
            1,
            CapPeriod::Day,
            CapPer::Pooled,
            RuleZone::Event,
        )],
        &["interview"],
    );
    request.goal.parts[0].requirement =
        Requirement::new(vec![Term::person("ana"), Term::person("bo")]);
    let engine = engine();
    assert!(engine.validate(&request).is_valid());
    let response = engine.find_availability(&request).unwrap();
    assert!(offered(&response).is_empty());
    assert_eq!(
        cap_params(&response, utc(MONDAY, 14, 0), "ana"),
        Some((1, 1, "2027-06-14".to_owned()))
    );
    assert_eq!(
        cap_params(&response, utc(MONDAY, 14, 0), "bo"),
        Some((1, 1, "2027-06-14".to_owned()))
    );
}

#[test]
fn an_occurrence_is_authoritative_for_span_and_participants_and_unions_tags() {
    // The occurrence names only ana and fixes the span, while bo carries a longer
    // block with the same key tagged `interview`. The event therefore counts for
    // ana alone, over the occurrence's span, and its tags are the union — which is
    // what makes it count toward the `interview` cap at all.
    let mut on_ana = block(utc(MONDAY, 9, 0), utc(MONDAY, 10, 0));
    on_ana.event_key = Some("evt_91".to_owned());
    let mut on_bo = tagged_block(utc(MONDAY, 8, 0), utc(MONDAY, 11, 0), &["interview"]);
    on_bo.event_key = Some("evt_91".to_owned());
    let mut request = request(
        two_day_search(),
        vec![utc(MONDAY, 14, 0)],
        vec![host("ana", vec![on_ana]), host("bo", vec![on_bo])],
        vec![cap_rule(
            "each_day",
            "interview",
            CapMeasure::Count,
            1,
            CapPeriod::Day,
            CapPer::Each,
            RuleZone::Participant,
        )],
        &["interview"],
    );
    request.goal.parts[0].requirement =
        Requirement::new(vec![Term::person("ana"), Term::person("bo")]);
    request.occurrences = vec![
        Occurrence::new(
            "evt_91",
            span(utc(MONDAY, 9, 0), utc(MONDAY, 10, 0)),
            vec!["ana".to_owned()],
        )
        .expect("the occurrence names a participant"),
    ];

    let engine = engine();
    assert!(engine.validate(&request).is_valid());
    let response = engine.find_availability(&request).unwrap();
    // ana is over her cap; bo, whom the occurrence does not name, is not.
    assert!(offered(&response).is_empty());
    assert_eq!(
        cap_params(&response, utc(MONDAY, 14, 0), "ana"),
        Some((1, 1, "2027-06-14".to_owned()))
    );
    assert_eq!(cap_params(&response, utc(MONDAY, 14, 0), "bo"), None);
    assert!(
        response.excluded[0]
            .reasons
            .iter()
            .all(|reason| reason.participant.as_deref() != Some("bo")),
        "bo is free, so no reason is reported about them"
    );
    assert_eq!(
        response.excluded[0].reasons.len(),
        1,
        "ana's cap is the whole explanation"
    );
}

#[test]
fn blocks_sharing_a_key_merge_from_the_earliest_start_to_the_latest_end() {
    // Two blocks of one keyless-in-occurrence series merge into one event whose
    // minutes run from the earliest start to the latest end, so a 60-minute cap is
    // already seven hours over.
    let mut first = tagged_block(utc(MONDAY, 9, 0), utc(MONDAY, 10, 0), &["interview"]);
    first.event_key = Some("series_a".to_owned());
    let mut second = tagged_block(utc(MONDAY, 14, 0), utc(MONDAY, 16, 0), &["interview"]);
    second.event_key = Some("series_a".to_owned());
    let request = request(
        two_day_search(),
        vec![utc(MONDAY, 16, 30)],
        vec![host("ana", vec![first, second])],
        vec![cap_rule(
            "minutes_day",
            "interview",
            CapMeasure::Minutes,
            60,
            CapPeriod::Day,
            CapPer::Each,
            RuleZone::Participant,
        )],
        &["interview"],
    );
    let response = engine().find_availability(&request).unwrap();
    assert!(offered(&response).is_empty());
    // 09:00 to 16:00 is seven hours.
    assert_eq!(
        cap_params(&response, utc(MONDAY, 16, 30), "ana"),
        Some((420, 60, "2027-06-14".to_owned()))
    );
}

#[test]
fn a_keyless_block_counts_once_per_block() {
    // Unlike a keyed event, two keyless blocks count separately, so a cap of two
    // is exactly full.
    let request = request(
        two_day_search(),
        vec![utc(MONDAY, 14, 0)],
        vec![host(
            "ana",
            vec![
                tagged_block(utc(MONDAY, 9, 0), utc(MONDAY, 10, 0), &["interview"]),
                tagged_block(utc(MONDAY, 10, 0), utc(MONDAY, 11, 0), &["interview"]),
            ],
        )],
        vec![cap_rule(
            "each_day",
            "interview",
            CapMeasure::Count,
            2,
            CapPeriod::Day,
            CapPer::Each,
            RuleZone::Participant,
        )],
        &["interview"],
    );
    let response = engine().find_availability(&request).unwrap();
    assert!(offered(&response).is_empty());
    assert_eq!(
        cap_params(&response, utc(MONDAY, 14, 0), "ana"),
        Some((2, 2, "2027-06-14".to_owned()))
    );
}

#[test]
fn each_pooled_cap_warns_about_only_the_keyless_blocks_it_counts() {
    // Two pooled caps over two differently tagged series and two participants. A
    // participant's keyless blocks are collected once but counted per cap, so the
    // second cap reports its own tally rather than the first cap's, a participant
    // no keyless block stands against is skipped, and the cap that a block is not
    // tagged for is not warned about at all.
    let request = request(
        two_day_search(),
        vec![utc(MONDAY, 14, 0)],
        vec![
            host(
                "ana",
                vec![
                    tagged_block(utc(MONDAY, 9, 0), utc(MONDAY, 10, 0), &["interview"]),
                    tagged_block(utc(MONDAY, 10, 0), utc(MONDAY, 11, 0), &["interview"]),
                    tagged_block(utc(MONDAY, 11, 0), utc(MONDAY, 12, 0), &["review"]),
                ],
            ),
            host("bo", Vec::new()),
        ],
        vec![
            cap_rule(
                "by_interview",
                "interview",
                CapMeasure::Count,
                5,
                CapPeriod::Day,
                CapPer::Pooled,
                RuleZone::Event,
            ),
            cap_rule(
                "by_review",
                "review",
                CapMeasure::Count,
                5,
                CapPeriod::Day,
                CapPer::Pooled,
                RuleZone::Event,
            ),
        ],
        &["interview", "review"],
    );
    let diagnostics = engine().validate(&request);
    let reported: Vec<(&str, &str)> = diagnostics
        .warnings
        .iter()
        .filter(|warning| warning.code() == "block_missing_event_key")
        .map(|warning| (warning.path().unwrap_or(""), warning.message()))
        .collect();
    assert_eq!(reported.len(), 2, "{reported:?}");
    assert!(
        reported[0].1.contains("`ana` holds 2 block(s)")
            && reported[0].1.contains("`by_interview`"),
        "the first cap counts only the blocks it selects: {}",
        reported[0].1
    );
    assert!(
        reported[1].1.contains("`ana` holds 1 block(s)") && reported[1].1.contains("`by_review`"),
        "the second cap counts its own series, not the first cap's: {}",
        reported[1].1
    );
    assert!(
        reported
            .iter()
            .all(|(path, _)| *path == "/participants/0/calendar"),
        "only the participant holding keyless blocks is named: {reported:?}"
    );
}

#[test]
fn minute_caps_exclude_travel() {
    // The block's own hour is what counts; its travel widens occupied time for
    // availability but adds nothing to workload.
    let mut travelled = tagged_block(utc(MONDAY, 9, 0), utc(MONDAY, 10, 0), &["sales/demo"]);
    travelled.travel_before = Duration::from_minutes(30);
    travelled.travel_after = Duration::from_minutes(30);
    let request = request(
        two_day_search(),
        vec![utc(MONDAY, 14, 0)],
        vec![host("ana", vec![travelled])],
        vec![cap_rule(
            "minutes_day",
            "sales/demo",
            CapMeasure::Minutes,
            60,
            CapPeriod::Day,
            CapPer::Each,
            RuleZone::Participant,
        )],
        &["sales/demo"],
    );
    let response = engine().find_availability(&request).unwrap();
    assert!(offered(&response).is_empty());
    assert_eq!(
        cap_params(&response, utc(MONDAY, 14, 0), "ana"),
        Some((60, 60, "2027-06-14".to_owned()))
    );
}

#[test]
fn minute_week_and_count_month_caps_compose_and_roll_their_buckets() {
    // Six hours of demos a week and four demos a month, built by the presets: the
    // week of 14 June is spent, while July is a fresh week and a fresh month.
    let demo = |start: Instant, end: Instant| tagged_block(start, end, &["sales/demo"]);
    let capped = |week_minutes: u32, month_count: u32| {
        request(
            span(utc(MONDAY, 14, 0), utc((2027, 7, 1), 15, 0)),
            vec![utc((2027, 6, 16), 14, 0), utc((2027, 7, 1), 14, 0)],
            vec![host(
                "ana",
                vec![
                    demo(utc(MONDAY, 9, 0), utc(MONDAY, 10, 30)),
                    demo(utc(MONDAY, 10, 30), utc(MONDAY, 12, 0)),
                    demo(utc(TUESDAY, 9, 0), utc(TUESDAY, 10, 30)),
                    demo(utc(TUESDAY, 10, 30), utc(TUESDAY, 12, 0)),
                ],
            )],
            vec![
                presets::cap_minutes_per_week(
                    Selector::Tag(tag("sales/demo")),
                    week_minutes,
                    CapPer::Each,
                    RuleZone::Participant,
                ),
                presets::cap_per_month(
                    Selector::Tag(tag("sales/demo")),
                    month_count,
                    CapPer::Each,
                    RuleZone::Participant,
                ),
            ],
            &["sales/demo"],
        )
    };
    let engine = engine();

    // The week cap alone: 360 of 360 minutes spent, so Wednesday is excluded while
    // the month's 4-of-40 leaves July free.
    let week_bound = capped(6 * 60, 40);
    assert!(engine.validate(&week_bound).is_valid());
    let response = engine.find_availability(&week_bound).unwrap();
    assert_eq!(
        offered(&response),
        [utc((2027, 7, 1), 14, 0)],
        "July is a fresh bucket for the minutes cap"
    );
    assert_eq!(
        cap_reasons(&response, utc((2027, 6, 16), 14, 0)),
        [("cap_minutes_per_week", "ana", 360, 360, "2027-06-14")],
        "the minutes cap measures the four demos' six hours in the week's bucket"
    );

    // The month cap alone: 4 of 4 demos spent, so the exclusion is the count cap
    // and July is a fresh month.
    let month_bound = capped(10 * 60, 4);
    assert!(engine.validate(&month_bound).is_valid());
    let response = engine.find_availability(&month_bound).unwrap();
    assert_eq!(
        offered(&response),
        [utc((2027, 7, 1), 14, 0)],
        "July is a fresh bucket for the monthly count cap"
    );
    assert_eq!(
        cap_reasons(&response, utc((2027, 6, 16), 14, 0)),
        [("cap_per_month", "ana", 4, 4, "2027-06-01")],
        "the monthly cap counts the four demos in the month's bucket"
    );
}

#[test]
fn a_pooled_minute_cap_counts_an_event_once_not_once_per_participant() {
    // One 60-minute event names both interviewers. The pool's workload is the
    // event's hour, not two hours, so the reason reports the same usage for each
    // of them.
    let mut request = request(
        two_day_search(),
        vec![utc(MONDAY, 14, 0)],
        vec![host("ana", Vec::new()), host("bo", Vec::new())],
        vec![cap_rule(
            "pool_minutes",
            "interview",
            CapMeasure::Minutes,
            60,
            CapPeriod::Day,
            CapPer::Pooled,
            RuleZone::Event,
        )],
        &["interview"],
    );
    request.goal.parts[0].requirement =
        Requirement::new(vec![Term::person("ana"), Term::person("bo")]);
    let mut occurrence = Occurrence::new(
        "evt_91",
        span(utc(MONDAY, 9, 0), utc(MONDAY, 10, 0)),
        vec!["ana".to_owned(), "bo".to_owned()],
    )
    .expect("the occurrence names participants");
    occurrence.tags = vec![tag("interview")];
    request.occurrences = vec![occurrence];

    let response = engine().find_availability(&request).unwrap();
    assert!(offered(&response).is_empty());
    for participant in ["ana", "bo"] {
        assert_eq!(
            cap_params(&response, utc(MONDAY, 14, 0), participant),
            Some((60, 60, "2027-06-14".to_owned())),
            "the pool counts the event's sixty minutes once, not once per person"
        );
    }
}

#[test]
fn a_cap_counts_blocks_whatever_their_status_or_policy() {
    // Both blocks count even though neither blocks availability: one is `free`,
    // the other tentative and ignored by the policy. The reason is therefore the
    // cap, not `blocked`.
    let free = tagged_block(utc(MONDAY, 9, 0), utc(MONDAY, 10, 0), &["interview"]);
    let mut tentative = tagged_block(utc(MONDAY, 10, 0), utc(MONDAY, 11, 0), &["interview"]);
    tentative.status = BlockStatus::Tentative;
    let policy = Rule::new(
        "ignore-tentative",
        RuleKind::StatusPolicy(StatusPolicy {
            entries: vec![StatusPolicyEntry::status(
                BlockStatus::Tentative,
                StatusEffect::Ignore,
            )],
        }),
    );
    let request = request(
        two_day_search(),
        vec![utc(MONDAY, 14, 0)],
        vec![host("ana", vec![free, tentative])],
        vec![
            policy,
            cap_rule(
                "each_day",
                "interview",
                CapMeasure::Count,
                2,
                CapPeriod::Day,
                CapPer::Each,
                RuleZone::Participant,
            ),
        ],
        &["interview"],
    );
    let response = engine().find_availability(&request).unwrap();
    assert!(offered(&response).is_empty());
    let reason = &response.excluded[0].reasons[0];
    assert_eq!(reason.code, ReasonCode::CapReached);
    assert_eq!(
        reason.params.as_ref().and_then(|params| match params {
            ReasonParams::CapReached { used, .. } => Some(*used),
            _ => None,
        }),
        Some(2),
        "the free block and the policy-ignored block both count"
    );
}

#[test]
fn an_all_day_block_blocks_its_day_but_never_counts_toward_a_cap() {
    // An all-day OOO block tagged `interview` would contribute its full 1,440
    // minutes and wipe out a 1,200-minute weekly cap on its own. It counts
    // nothing instead: Monday (its own day) stays blocked as busy time, while
    // the rest of the week stays bookable.
    let mut ooo = tagged_block(utc(MONDAY, 0, 0), utc(TUESDAY, 0, 0), &["interview"]);
    ooo.all_day = true;
    let request = request(
        two_day_search(),
        vec![utc(MONDAY, 14, 0), utc(TUESDAY, 14, 0)],
        vec![host("ana", vec![ooo])],
        vec![cap_rule(
            "each_week",
            "interview",
            CapMeasure::Minutes,
            1_200,
            CapPeriod::Week,
            CapPer::Each,
            RuleZone::Participant,
        )],
        &["interview"],
    );
    let engine = engine();
    assert!(engine.validate(&request).is_valid());
    let response = engine.find_availability(&request).unwrap();

    // Monday is blocked by the all-day block itself, not by the cap.
    assert_eq!(offered(&response), [utc(TUESDAY, 14, 0)]);
    let monday = &response
        .excluded
        .iter()
        .find(|entry| entry.start == utc(MONDAY, 14, 0))
        .expect("the OOO day's candidate is excluded");
    assert!(
        monday
            .reasons
            .iter()
            .any(|reason| reason.code == ReasonCode::Blocked),
        "the all-day block blocks its own day as busy time"
    );
    assert!(
        monday
            .reasons
            .iter()
            .all(|reason| reason.code != ReasonCode::CapReached),
        "the all-day block contributes no minutes to the cap"
    );
    assert_eq!(cap_params(&response, utc(MONDAY, 14, 0), "ana"), None);

    // Tuesday, a different day of the same week, is bookable.
    let check = engine
        .check_slot(&request, &proposal(utc(TUESDAY, 14, 0), &["ana"]))
        .unwrap();
    assert!(check.valid, "the rest of the week stays bookable");
}

#[test]
fn a_pooled_cap_ignores_an_all_day_block_under_either_measure() {
    // A pool has one bucket for every covered member, so an all-day block cannot
    // hide behind another participant's calendar either. The all-day block sits
    // on Tuesday and the candidate on Monday of the same week, so only the week
    // bucket can see it: the timed hour leaves exactly the room the booking needs
    // under both measures.
    let a_week = || {
        let mut ooo = tagged_block(utc(TUESDAY, 0, 0), utc(WEDNESDAY, 0, 0), &["interview"]);
        ooo.all_day = true;
        vec![
            host(
                "ana",
                vec![
                    tagged_block(utc(MONDAY, 9, 0), utc(MONDAY, 10, 0), &["interview"]),
                    ooo,
                ],
            ),
            host("bo", Vec::new()),
        ]
    };
    let both = || Requirement::new(vec![Term::person("ana"), Term::person("bo")]);
    let engine = engine();

    for (measure, limit) in [(CapMeasure::Count, 2), (CapMeasure::Minutes, 90)] {
        let mut request = request(
            two_day_search(),
            vec![utc(MONDAY, 14, 0)],
            a_week(),
            vec![cap_rule(
                "pool_week",
                "interview",
                measure,
                limit,
                CapPeriod::Week,
                CapPer::Pooled,
                RuleZone::Event,
            )],
            &["interview"],
        );
        request.goal.parts[0].requirement = both();
        assert!(engine.validate(&request).is_valid());
        let response = engine.find_availability(&request).unwrap();
        assert_eq!(
            offered(&response),
            [utc(MONDAY, 14, 0)],
            "{measure:?}: only the timed hour fills the pool"
        );
        assert_eq!(cap_params(&response, utc(MONDAY, 14, 0), "ana"), None);
    }
}

#[test]
fn an_all_day_block_sharing_an_event_key_contributes_no_minutes() {
    // One event, carried all-day on bo's calendar and as a timed hour on ana's.
    // Merged, the two spans would run midnight to midnight and 1,440 minutes
    // would land in a 60-minute cap; only ana's own hour is workload.
    let mut on_bo = tagged_block(utc(MONDAY, 0, 0), utc(TUESDAY, 0, 0), &["interview"]);
    on_bo.all_day = true;
    on_bo.event_key = Some("evt_shared".to_owned());
    let mut on_ana = tagged_block(utc(MONDAY, 9, 0), utc(MONDAY, 10, 0), &["interview"]);
    on_ana.event_key = Some("evt_shared".to_owned());
    let mut request = request(
        two_day_search(),
        vec![utc(MONDAY, 14, 0)],
        vec![host("ana", vec![on_ana]), host("bo", vec![on_bo])],
        vec![cap_rule(
            "minutes_day",
            "interview",
            CapMeasure::Minutes,
            60,
            CapPeriod::Day,
            CapPer::Each,
            RuleZone::Participant,
        )],
        &["interview"],
    );
    request.goal.parts[0].requirement =
        Requirement::new(vec![Term::person("ana"), Term::person("bo")]);
    let response = engine().find_availability(&request).unwrap();
    assert_eq!(
        cap_params(&response, utc(MONDAY, 14, 0), "ana"),
        Some((60, 60, "2027-06-14".to_owned())),
        "the all-day copy contributes no minutes to the merged event"
    );
}

#[test]
fn an_all_day_copy_adds_no_participant_to_a_keyed_event() {
    // The cap covers bo alone, and bo's calendar holds the event only as an
    // all-day block, so it is not bo's workload: bo's week stays empty and
    // Tuesday is offered. Were the all-day copy enough to make bo a participant
    // of the merged event, ana's hour would land in bo's week bucket and the
    // booking would trip bo's cap. The candidate sits on Tuesday because bo's
    // all-day block still blocks its own Monday as busy time.
    let mut on_bo = tagged_block(utc(MONDAY, 0, 0), utc(TUESDAY, 0, 0), &["interview"]);
    on_bo.all_day = true;
    on_bo.event_key = Some("evt_shared".to_owned());
    let mut on_ana = tagged_block(utc(MONDAY, 9, 0), utc(MONDAY, 10, 0), &["interview"]);
    on_ana.event_key = Some("evt_shared".to_owned());
    let mut request = request(
        two_day_search(),
        vec![utc(TUESDAY, 14, 0)],
        vec![host("ana", vec![on_ana]), host("bo", vec![on_bo])],
        vec![
            cap_rule(
                "bo_week",
                "interview",
                CapMeasure::Minutes,
                60,
                CapPeriod::Week,
                CapPer::Each,
                RuleZone::Participant,
            )
            .with_who(Selector::ids(["bo"])),
        ],
        &["interview"],
    );
    request.goal.parts[0].requirement =
        Requirement::new(vec![Term::person("ana"), Term::person("bo")]);
    assert!(engine().validate(&request).is_valid());
    let response = engine().find_availability(&request).unwrap();
    assert_eq!(
        offered(&response),
        [utc(TUESDAY, 14, 0)],
        "the event's participants are the calendars holding it as a timed block"
    );
}

#[test]
fn an_all_day_block_sharing_an_occurrence_key_still_unions_its_tags() {
    // A block and an occurrence sharing an event key are one event, and the tags
    // are the union of both. bo's calendar carries the event all-day, tagged
    // `interview`; the caller's occurrence for the same key is ana's booked hour,
    // which carries no tag of its own. The union is what makes ana's event count,
    // so the cap has to see it — and flipping only the all-day flag must not
    // change that, because the block contributes tags, never workload. The
    // occurrence stays authoritative for the span, so the minutes counted are its
    // hour and not the all-day block's midnight-to-midnight day.
    let response = |all_day: bool, measure: CapMeasure, limit: u32| {
        let mut on_bo = tagged_block(utc(MONDAY, 0, 0), utc(TUESDAY, 0, 0), &["interview"]);
        on_bo.all_day = all_day;
        on_bo.event_key = Some("evt_91".to_owned());
        let mut request = request(
            two_day_search(),
            vec![utc(MONDAY, 14, 0)],
            vec![host("ana", Vec::new()), host("bo", vec![on_bo])],
            vec![cap_rule(
                "each_day",
                "interview",
                measure,
                limit,
                CapPeriod::Day,
                CapPer::Each,
                RuleZone::Participant,
            )],
            &["interview"],
        );
        request.goal.parts[0].requirement =
            Requirement::new(vec![Term::person("ana"), Term::person("bo")]);
        request.occurrences = vec![
            Occurrence::new(
                "evt_91",
                span(utc(MONDAY, 9, 0), utc(MONDAY, 10, 0)),
                vec!["ana".to_owned()],
            )
            .expect("the occurrence names a participant"),
        ];
        assert!(engine().validate(&request).is_valid());
        engine().find_availability(&request).unwrap()
    };

    assert_eq!(
        cap_params(
            &response(false, CapMeasure::Count, 1),
            utc(MONDAY, 14, 0),
            "ana"
        ),
        Some((1, 1, "2027-06-14".to_owned())),
        "the timed copy's tag makes the occurrence's event count"
    );
    assert_eq!(
        cap_params(
            &response(true, CapMeasure::Count, 1),
            utc(MONDAY, 14, 0),
            "ana"
        ),
        Some((1, 1, "2027-06-14".to_owned())),
        "an all-day copy's tags are still the event's tags"
    );
    assert_eq!(
        cap_params(
            &response(true, CapMeasure::Minutes, 60),
            utc(MONDAY, 14, 0),
            "ana"
        ),
        Some((60, 60, "2027-06-14".to_owned())),
        "the occurrence's hour is the event's span, not the all-day day"
    );
}

#[test]
fn rest_keeps_precedence_over_a_cap_the_same_start_trips() {
    // The start is both inside the protected morning and past the day's cap. The
    // search reports the canonical first reason, rest, while `check_slot` reports
    // both in precedence order.
    let start = utc(TUESDAY, 9, 0);
    let request = request(
        two_day_search(),
        vec![start],
        vec![host(
            "ana",
            vec![
                block(utc(MONDAY, 19, 0), utc(MONDAY, 20, 0)),
                tagged_block(utc(TUESDAY, 6, 0), utc(TUESDAY, 7, 0), &["interview"]),
            ],
        )],
        vec![
            cap_rule(
                "each_day",
                "interview",
                CapMeasure::Count,
                1,
                CapPeriod::Day,
                CapPer::Each,
                RuleZone::Participant,
            ),
            presets::rest(time(18, 0), time(10, 0)),
        ],
        &["interview"],
    );
    let engine = engine();
    let response = engine.find_availability(&request).unwrap();
    assert!(offered(&response).is_empty());
    assert_eq!(
        response.excluded[0].reasons[0].code,
        ReasonCode::Rest,
        "rest sits before cap in the canonical precedence"
    );

    let check = engine
        .check_slot(&request, &proposal(start, &["ana"]))
        .unwrap();
    assert!(!check.valid);
    let codes: Vec<ReasonCode> = check.reasons.iter().map(|reason| reason.code).collect();
    assert_eq!(codes, [ReasonCode::Rest, ReasonCode::CapReached]);
}

#[test]
fn validate_bounds_candidate_generation_for_a_window_past_every_limit() {
    // A search window spanning the whole supported range on a one-minute grid
    // would generate tens of millions of starts. Validation only needs to know
    // that the request is over the limit, so it must report both errors without
    // materialising that search space.
    let mut request = request(
        span(Instant::MIN, Instant::MAX),
        Vec::new(),
        vec![host("ana", Vec::new())],
        Vec::new(),
        &["interview"],
    );
    request.goal.candidates = None;
    request.goal.grid = Some(
        opportune::time::Grid::new(
            Duration::from_minutes(1),
            time(0, 0),
            opportune::time::GridZone::Event,
        )
        .expect("a one-minute grid is valid"),
    );
    let codes: Vec<&str> = engine()
        .validate(&request)
        .errors
        .iter()
        .map(opportune::Diagnostic::code)
        .collect();
    assert!(codes.contains(&"search_window_limit"), "{codes:?}");
    assert!(codes.contains(&"candidates_limit"), "{codes:?}");
}

#[test]
fn a_cap_applies_only_when_the_part_tags_match_its_count_and_when() {
    let setup = |count: &str, when: Selector, part_tags: &[&str]| {
        let rule = cap_rule(
            "each_day",
            count,
            CapMeasure::Count,
            1,
            CapPeriod::Day,
            CapPer::Each,
            RuleZone::Participant,
        )
        .with_when(when);
        request(
            two_day_search(),
            vec![utc(MONDAY, 14, 0)],
            vec![host(
                "ana",
                vec![tagged_block(
                    utc(MONDAY, 9, 0),
                    utc(MONDAY, 10, 0),
                    &["interview"],
                )],
            )],
            vec![rule],
            part_tags,
        )
    };
    // The part is not an interview, so the cap does not measure this booking:
    // both with `count` alone and with a `when` that matches nothing.
    for request in [
        setup("interview", Selector::True, &[]),
        setup("interview", Selector::Tag(tag("interview")), &[]),
    ] {
        let response = engine().find_availability(&request).unwrap();
        assert_eq!(
            offered(&response),
            [utc(MONDAY, 14, 0)],
            "a cap for another kind of booking must not apply"
        );
    }
    // Both match: the existing interview fills the cap.
    let response = engine()
        .find_availability(&setup(
            "interview",
            Selector::Tag(tag("interview")),
            &["interview"],
        ))
        .unwrap();
    assert!(offered(&response).is_empty());
}

#[test]
fn validation_reports_a_pooled_participant_zone_and_warns_about_keyless_blocks() {
    // The all-day block carries no key either, but it never counts toward a cap,
    // so it is not what the warning is about; the timed block is.
    let mut ooo = tagged_block(utc(MONDAY, 0, 0), utc(TUESDAY, 0, 0), &["interview"]);
    ooo.all_day = true;
    let request = request(
        two_day_search(),
        vec![utc(MONDAY, 14, 0)],
        vec![host(
            "ana",
            vec![
                ooo,
                tagged_block(utc(MONDAY, 9, 0), utc(MONDAY, 10, 0), &["interview"]),
            ],
        )],
        vec![cap_rule(
            "pool_day",
            "interview",
            CapMeasure::Count,
            1,
            CapPeriod::Day,
            CapPer::Pooled,
            RuleZone::Participant,
        )],
        &["interview"],
    );
    let diagnostics = engine().validate(&request);
    assert_eq!(
        diagnostics.errors[0].code(),
        "cap_pooled_participant_zone",
        "a pool has no single participant zone to bucket in"
    );
    let warnings: Vec<_> = diagnostics
        .warnings
        .iter()
        .filter(|warning| warning.code() == "block_missing_event_key")
        .collect();
    let warning = warnings
        .first()
        .copied()
        .expect("a keyless block a pooled cap counts is warned about");
    assert_eq!(warnings.len(), 1);
    assert!(
        warning.message().contains("holds 1 block(s)"),
        "only the timed block is counted one by one: {:?}",
        warning.message()
    );
}

#[test]
fn validation_rejects_duplicate_occurrence_keys_and_ignores_unknown_participants() {
    let mut request = request(
        two_day_search(),
        vec![utc(MONDAY, 14, 0)],
        vec![host("ana", Vec::new())],
        vec![cap_rule(
            "each_day",
            "interview",
            CapMeasure::Count,
            1,
            CapPeriod::Day,
            CapPer::Each,
            RuleZone::Participant,
        )],
        &["interview"],
    );
    let occurrence = |key: &str, participants: Vec<String>| {
        Occurrence::new(
            key,
            span(utc(MONDAY, 9, 0), utc(MONDAY, 10, 0)),
            participants,
        )
        .expect("the occurrence names a participant")
    };
    request.occurrences = vec![
        occurrence("evt_91", vec!["ana".to_owned()]),
        // A real meeting includes people outside the request; their names are
        // ignored rather than errors.
        occurrence("evt_92", vec!["ghost".to_owned()]),
        occurrence("evt_91", vec!["ana".to_owned()]),
    ];
    let diagnostics = engine().validate(&request);
    assert_eq!(
        diagnostics
            .errors
            .iter()
            .map(opportune::Diagnostic::code)
            .collect::<Vec<_>>(),
        ["occurrence_key_duplicate"]
    );

    // The unknown name is dropped, so the event counts for nobody.
    request.occurrences.truncate(2);
    assert!(engine().validate(&request).is_valid());
    let response = engine().find_availability(&request).unwrap();
    assert_eq!(
        offered(&response),
        [utc(MONDAY, 14, 0)],
        "an event naming only unknown participants counts for nobody"
    );
}

#[test]
fn required_ranges_returns_no_occurrence_range_without_a_cap() {
    let request = request(
        two_day_search(),
        vec![utc(MONDAY, 14, 0)],
        vec![host("ana", Vec::new())],
        Vec::new(),
        &["interview"],
    );
    let ranges = query_ranges(&request);
    assert_eq!(ranges.occurrences, None);
}

#[test]
fn required_ranges_hulls_the_buckets_the_search_touches() {
    let query = |period: CapPeriod, week_starts: Weekday| {
        let rule = Rule::new(
            "each",
            RuleKind::Cap(Cap::new(
                Selector::Tag(tag("interview")),
                CapMeasure::Count,
                1,
                period,
                CapPer::Each,
                RuleZone::Event,
                week_starts,
            )),
        );
        let request = request(
            span(utc(MONDAY, 6, 0), utc(MONDAY, 18, 0)),
            vec![utc(MONDAY, 14, 0)],
            vec![host("ana", Vec::new())],
            vec![rule],
            &["interview"],
        );
        query_ranges(&request)
    };
    // A day bucket is exactly that day; a month bucket is the whole civil month;
    // and a week bucket starts on the configured weekday, not ISO's Monday.
    assert_eq!(
        query(CapPeriod::Day, Weekday::Monday).occurrences,
        Some(span(utc(MONDAY, 0, 0), utc(TUESDAY, 0, 0)))
    );
    assert_eq!(
        query(CapPeriod::Month, Weekday::Monday).occurrences,
        Some(span(utc((2027, 6, 1), 0, 0), utc((2027, 7, 1), 0, 0)))
    );
    // Monday 14 June falls in the week that started on Wednesday 9 June.
    assert_eq!(
        query(CapPeriod::Week, Weekday::Wednesday).occurrences,
        Some(span(utc((2027, 6, 9), 0, 0), utc((2027, 6, 16), 0, 0)))
    );
}

/// The ranges a request needs, computed from its own goal, rules and
/// participants.
fn query_ranges(request: &Request) -> opportune::model::RequiredRanges {
    let mut query = RangesQuery::new(request.now, request.goal.clone());
    query.rules.clone_from(&request.rules);
    query.participants.clone_from(&request.participants);
    engine()
        .required_ranges(&query)
        .expect("the fixture query resolves")
}

#[test]
fn required_ranges_covers_a_counted_block_the_participant_reads_in_another_zone() {
    // ana is in Tokyo for the whole Wednesday window and home in London by
    // Sunday. A week cap starting Monday buckets both readings as the week of 14
    // June: the booking's start is read in Tokyo, and the existing Sunday-evening
    // interview is read in London once she is home again. A caller that fetched
    // only what `required_ranges` asks for would miss that interview and offer a
    // week whose cap is already full.
    let window = span(utc((2027, 6, 16), 0, 0), utc((2027, 6, 16), 12, 0));
    let sunday = tagged_block(
        local("Europe/London", (2027, 6, 20), 23, 0),
        local("Europe/London", (2027, 6, 20), 23, 30),
        &["interview"],
    );
    let mut ana = host("ana", vec![sunday]);
    ana.zone = name("Europe/London");
    ana.zone_periods = vec![
        ZonePeriod::new(name("Asia/Tokyo"), window.start(), window.end())
            .expect("the leg is non-empty"),
    ];
    let start = utc((2027, 6, 16), 6, 0);
    let request = request(
        window,
        vec![start],
        vec![ana],
        vec![cap_rule(
            "each_week",
            "interview",
            CapMeasure::Count,
            1,
            CapPeriod::Week,
            CapPer::Each,
            RuleZone::Participant,
        )],
        &["interview"],
    );
    let engine = engine();
    let diag = engine.validate(&request);
    assert!(diag.is_valid());
    let response = engine.find_availability(&request).unwrap();
    assert!(
        offered(&response).is_empty(),
        "the week already holds the Sunday interview"
    );
    assert_eq!(
        cap_params(&response, start, "ana"),
        Some((1, 1, "2027-06-14".to_owned()))
    );

    // A caller fetches calendar data over the calendar range, and cap-counted
    // events over the occurrence range. Dropping everything outside both must
    // leave the answer alone, or the returned ranges under-state what the request
    // needs.
    let ranges = query_ranges(&request);
    let occurrences = ranges
        .occurrences
        .expect("a cap applies, so occurrences are needed");
    let mut fetched = request.clone();
    fetched.participants[0]
        .calendar
        .as_mut()
        .expect("the fixture carries a calendar")
        .retain(|block| {
            let from = block.span.start().minutes();
            let until = block.span.end().minutes() + block.travel_after.minutes();
            let calendar = ranges.calendar;
            let in_calendar = from < calendar.end().minutes() && calendar.start().minutes() < until;
            let in_occurrences =
                from < occurrences.end().minutes() && occurrences.start().minutes() < until;
            in_calendar || in_occurrences
        });
    let stripped = engine.find_availability(&fetched).unwrap();
    assert_eq!(
        offered(&stripped),
        offered(&response),
        "the caller removed only data the returned ranges said it need not fetch"
    );
}

#[test]
fn required_ranges_reaches_a_month_bucket_a_leg_weeks_after_the_window() {
    // The bucket a month cap reads is a whole civil month, so an interview at the
    // end of June counts against a booking in the middle of June even though it
    // lies three weeks past the window and is read in a zone ana only reaches
    // later. The occurrence hull has to cover that month in every zone she is in
    // during it; a hull built from the window's own zones stops at the window, the
    // caller drops the interview, and the engine offers a month whose cap is full.
    let window = span(utc((2027, 6, 10), 9, 0), utc((2027, 6, 10), 11, 0));
    let interview = local("Pacific/Midway", (2027, 6, 30), 23, 0);
    let interview_end =
        Instant::from_minutes(interview.minutes() + 30).expect("a supported instant");
    let mut ana = host(
        "ana",
        vec![tagged_block(interview, interview_end, &["interview"])],
    );
    ana.zone = name("Europe/London");
    ana.zone_periods = vec![
        ZonePeriod::new(
            name("Pacific/Midway"),
            utc((2027, 6, 28), 0, 0),
            utc((2027, 7, 2), 0, 0),
        )
        .expect("the trip is non-empty"),
    ];
    let start = utc((2027, 6, 10), 10, 0);
    let mut request = request(
        window,
        vec![start],
        vec![ana],
        vec![cap_rule(
            "each_month",
            "interview",
            CapMeasure::Count,
            1,
            CapPeriod::Month,
            CapPer::Each,
            RuleZone::Participant,
        )],
        &["interview"],
    );
    request.now = utc((2027, 6, 9), 0, 0);
    let engine = engine();
    assert!(engine.validate(&request).is_valid());
    let response = engine.find_availability(&request).unwrap();
    assert!(
        offered(&response).is_empty(),
        "the month already holds the interview"
    );
    assert_eq!(
        cap_params(&response, start, "ana"),
        Some((1, 1, "2027-06-01".to_owned()))
    );

    let ranges = query_ranges(&request);
    let occurrences = ranges
        .occurrences
        .expect("a cap applies, so occurrences are needed");
    assert!(
        interview >= ranges.calendar.end(),
        "the interview lies outside the calendar hull, so only the occurrence hull can cover it"
    );
    assert!(
        occurrences.start() <= interview && interview < occurrences.end(),
        "the occurrence hull {occurrences:?} excludes the counted interview at {interview}"
    );

    let mut fetched = request.clone();
    fetched.participants[0]
        .calendar
        .as_mut()
        .expect("the fixture carries a calendar")
        .retain(|block| {
            let from = block.span.start().minutes();
            let until = block.span.end().minutes() + block.travel_after.minutes();
            let calendar = ranges.calendar;
            let in_calendar = from < calendar.end().minutes() && calendar.start().minutes() < until;
            let in_occurrences =
                from < occurrences.end().minutes() && occurrences.start().minutes() < until;
            in_calendar || in_occurrences
        });
    let stripped = engine.find_availability(&fetched).unwrap();
    assert_eq!(
        offered(&stripped),
        offered(&response),
        "the caller removed only data the returned ranges said it need not fetch"
    );
}

#[test]
fn required_ranges_clamps_the_occurrence_hull_at_the_supported_edges() {
    // A month bucket at either edge of the supported range has a bound outside
    // it: 1975-01-01 in Kiritimati begins before `Instant::MIN`, and 2100-01-01
    // in Midway begins after `Instant::MAX`. Both clamps widen, so the hull stays
    // a superset and `required_ranges` still answers instead of failing on an
    // empty span.
    let query = |zone: &str, window: Span| {
        let mut goal = Goal::single(
            name("UTC"),
            window,
            Duration::from_minutes(30),
            Requirement::new(vec![Term::person("ana")]),
        );
        goal.parts[0].tags = vec![tag("interview")];
        let mut query = RangesQuery::new(window.start(), goal);
        query.participants = vec![Participant::new("ana", name(zone))];
        query.rules = vec![cap_rule(
            "each_month",
            "interview",
            CapMeasure::Count,
            1,
            CapPeriod::Month,
            CapPer::Each,
            RuleZone::Participant,
        )];
        engine()
            .required_ranges(&query)
            .expect("a range at the edge of the supported range resolves")
    };

    let first = Instant::MIN;
    let low_end = Instant::from_minutes(Instant::MIN.minutes() + 30).expect("a supported instant");
    let low = query("Pacific/Kiritimati", span(first, low_end)).occurrences;
    let low = low.expect("a cap applies");
    assert_eq!(
        low.start(),
        Instant::MIN,
        "January 1975 begins before the edge"
    );
    assert!(
        low.start() <= first && first < low.end(),
        "the hull {low:?} must hold the window's own bucket"
    );

    let high_start =
        Instant::from_minutes(Instant::MAX.minutes() - 30).expect("a supported instant");
    let high = query("Pacific/Midway", span(high_start, Instant::MAX)).occurrences;
    let high = high.expect("a cap applies");
    assert_eq!(
        high.end(),
        Instant::MAX,
        "the bucket runs past the last instant"
    );
    assert!(
        high.start() <= high_start && high_start < high.end(),
        "the hull {high:?} must hold the window's own bucket"
    );
}
