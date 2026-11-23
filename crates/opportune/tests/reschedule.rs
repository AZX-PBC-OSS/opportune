//! End-to-end tests for rescheduling an existing booking with
//! `Request::exclude_event_keys`: the event being moved takes no part in busy
//! blocking, `rest` activity or cap workload, so it can be booked elsewhere even
//! when its old copy would have blocked the slot, triggered rest from its old
//! time or used up the host's cap.
//!
//! Every case builds a real request and runs it on a real engine, and each one
//! states both answers — with the exclusion and without it — so the behaviour that
//! changes is the exclusion itself rather than the fixture.

#![cfg(feature = "bundled-tzdb")]

use opportune::Engine;
use opportune::Error;
use opportune::Limits;
use opportune::model::{
    Block, Cap, CapMeasure, CapPer, CapPeriod, Explain, Goal, Occurrence, Participant, Proposal,
    ProposalEvent, Request, Requirement, Rest, Rule, RuleKind, Span, Term,
};
use opportune::tags::{Selector, Tag};
use opportune::time::{Duration, Instant, RuleZone, TimeOfDay, Weekday, ZoneName};

/// Monday 14 June 2027, the fixture's first day.
const MONDAY: (i16, i8, i8) = (2027, 6, 14);
/// Tuesday 15 June 2027, the fixture's second day.
const TUESDAY: (i16, i8, i8) = (2027, 6, 15);

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

/// The UTC instant of a local date-time.
fn utc(day: (i16, i8, i8), hour: u8, minute: u8) -> Instant {
    use opportune::time::{Date, LocalResolution, Zone};

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

/// A busy block over `[start, end)` tagged `tags` and identified by `key`.
fn keyed_block(start: Instant, end: Instant, tags: &[&str], key: &str) -> Block {
    let mut block = Block::new(span(start, end));
    block.tags = tags.iter().map(|path| tag(path)).collect();
    block.event_key = Some(key.to_owned());
    block
}

/// A host in UTC with the given blocks.
fn host(id: &str, blocks: Vec<Block>) -> Participant {
    let mut participant = Participant::new(id, name("UTC"));
    participant.calendar = Some(blocks);
    participant
}

/// A host in UTC tagged `interview`, with the given blocks.
fn tagged_host(id: &str, blocks: Vec<Block>) -> Participant {
    let mut participant = host(id, blocks);
    participant.tags = vec![tag("interview")];
    participant
}

/// A request booking a one-hour event, over `candidates`, for `terms`, with the
/// given participants, rules and part tags.
fn request(
    search: Span,
    candidates: Vec<Instant>,
    participants: Vec<Participant>,
    rules: Vec<Rule>,
    terms: Vec<Term>,
    part_tags: &[&str],
) -> Request {
    let mut goal = Goal::single(
        name("UTC"),
        search,
        Duration::from_minutes(60),
        Requirement::new(terms),
    );
    goal.candidates = Some(candidates);
    goal.parts[0].tags = part_tags.iter().map(|path| tag(path)).collect();
    let mut request = Request::new(utc(MONDAY, 0, 0), goal);
    request.participants = participants;
    request.rules = rules;
    request.explain = Some(Explain::new(10_000));
    request
}

/// A `cap` rule counting `counted` events, `who` defaulting to every host.
fn cap_rule(
    id: &str,
    counted: &str,
    measure: CapMeasure,
    limit: u32,
    period: CapPeriod,
    per: CapPer,
) -> Rule {
    Rule::new(
        id,
        RuleKind::Cap(Cap::new(
            Selector::Tag(tag(counted)),
            measure,
            limit,
            period,
            per,
            RuleZone::Event,
            Weekday::Monday,
        )),
    )
}

/// An occurrence of `key` over `[start, end)`, tagged `tags` and naming `ana`.
fn occurrence(key: &str, start: Instant, end: Instant, tags: &[&str]) -> Occurrence {
    let mut occurrence = Occurrence::new(key, span(start, end), vec!["ana".to_owned()])
        .expect("the fixture occurrence names a key and a participant");
    occurrence.tags = tags.iter().map(|path| tag(path)).collect();
    occurrence
}

fn offered(response: &opportune::model::Response) -> Vec<Instant> {
    response
        .offers
        .iter()
        .map(|offer| offer.start().expect("an offer has an event"))
        .collect()
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

/// The day's window, from one afternoon to the next.
fn day_and_after() -> Span {
    span(utc(MONDAY, 8, 0), utc(TUESDAY, 18, 0))
}

/// One host already at a two-a-day cap, with the event being moved among the two.
fn host_at_daily_cap() -> Participant {
    host(
        "ana",
        vec![
            keyed_block(
                utc(MONDAY, 9, 0),
                utc(MONDAY, 10, 0),
                &["interview"],
                "evt_a",
            ),
            keyed_block(
                utc(MONDAY, 11, 0),
                utc(MONDAY, 12, 0),
                &["interview"],
                "evt_b",
            ),
        ],
    )
}

#[test]
fn a_daily_count_cap_at_capacity_still_books_the_event_being_moved() {
    let engine = engine();
    let slot = utc(MONDAY, 14, 0);
    let build = |exclude: &[&str]| {
        request(
            day_and_after(),
            vec![slot],
            vec![host_at_daily_cap()],
            vec![cap_rule(
                "each_day",
                "interview",
                CapMeasure::Count,
                2,
                CapPeriod::Day,
                CapPer::Each,
            )],
            vec![Term::person("ana")],
            &["interview"],
        )
        .with_exclude_event_keys(exclude.iter().map(|key| (*key).to_owned()).collect())
    };

    // Two of two counted, so a third reading of the day refuses the slot: that is
    // exactly the state a caller reschedules from.
    let without = engine.find_availability(&build(&[])).unwrap();
    assert_eq!(offered(&without), [], "the day is at its cap");
    assert!(
        without.excluded.iter().any(|entry| entry.start == slot),
        "the slot is judged and excluded"
    );

    // Naming the event being moved frees its own count: the day holds one event
    // and the booking is the second, which the cap allows.
    let with = engine.find_availability(&build(&["evt_a"])).unwrap();
    assert_eq!(offered(&with), [slot], "the moved event's count is gone");

    let accepted = engine
        .check_slot(&build(&["evt_a"]), &proposal(slot, &["ana"]))
        .unwrap();
    assert!(accepted.valid, "the same slot confirms at capacity");
    assert!(accepted.reasons.is_empty());
    let refused = engine
        .check_slot(&build(&[]), &proposal(slot, &["ana"]))
        .unwrap();
    assert!(
        !refused.valid,
        "without the exclusion the cap still refuses it"
    );
}

#[test]
fn a_weekly_minutes_cap_at_capacity_still_books_the_event_being_moved() {
    let engine = engine();
    let slot = utc(MONDAY, 14, 0);
    // Two 60-minute meetings a 150-minute week may not take: 120 used, and the
    // one-hour booking is the 180th minute.
    let build = |exclude: &[&str]| {
        request(
            day_and_after(),
            vec![slot],
            vec![host_at_daily_cap()],
            vec![cap_rule(
                "each_week",
                "interview",
                CapMeasure::Minutes,
                150,
                CapPeriod::Week,
                CapPer::Each,
            )],
            vec![Term::person("ana")],
            &["interview"],
        )
        .with_exclude_event_keys(exclude.iter().map(|key| (*key).to_owned()).collect())
    };

    assert_eq!(offered(&engine.find_availability(&build(&[])).unwrap()), []);
    assert_eq!(
        offered(&engine.find_availability(&build(&["evt_a"])).unwrap()),
        [slot],
        "the moved event's 60 minutes no longer hold the week's budget"
    );
    assert!(
        engine
            .check_slot(&build(&["evt_a"]), &proposal(slot, &["ana"]))
            .unwrap()
            .valid
    );
}

#[test]
fn a_pooled_cap_at_capacity_still_books_the_event_being_moved() {
    let engine = engine();
    let slot = utc(MONDAY, 14, 0);
    // The pool's two events are not ana's two: she holds only `evt_a`, while bo holds
    // both. Ana's own day has room for the booking throughout, so only a cap that
    // counts the covered hosts together refuses it — which is what makes this the
    // pooled shape rather than an `each` one.
    let pool = || {
        vec![
            host(
                "ana",
                vec![keyed_block(
                    utc(MONDAY, 9, 0),
                    utc(MONDAY, 10, 0),
                    &["interview"],
                    "evt_a",
                )],
            ),
            host(
                "bo",
                vec![
                    keyed_block(
                        utc(MONDAY, 9, 0),
                        utc(MONDAY, 10, 0),
                        &["interview"],
                        "evt_a",
                    ),
                    keyed_block(
                        utc(MONDAY, 11, 0),
                        utc(MONDAY, 12, 0),
                        &["interview"],
                        "evt_b",
                    ),
                ],
            ),
        ]
    };
    let build = |exclude: &[&str]| {
        request(
            day_and_after(),
            vec![slot],
            pool(),
            vec![cap_rule(
                "pool_week",
                "interview",
                CapMeasure::Count,
                2,
                CapPeriod::Week,
                CapPer::Pooled,
            )],
            vec![Term::person("ana")],
            &["interview"],
        )
        .with_exclude_event_keys(exclude.iter().map(|key| (*key).to_owned()).collect())
    };

    assert_eq!(offered(&engine.find_availability(&build(&[])).unwrap()), []);
    assert_eq!(
        offered(&engine.find_availability(&build(&["evt_a"])).unwrap()),
        [slot],
        "the pool holds one event, and the booking is the second"
    );
    assert!(
        engine
            .check_slot(&build(&["evt_a"]), &proposal(slot, &["ana"]))
            .unwrap()
            .valid
    );
}

#[test]
fn the_event_being_moved_is_excluded_when_only_its_occurrence_identifies_it() {
    let engine = engine();
    let slot = utc(MONDAY, 14, 0);
    // The moved booking lives on another calendar: the caller has its occurrence and
    // no block of its own. The cap still counts it, and naming its key removes it —
    // the block skip alone could never reach it.
    let build = |exclude: &[&str]| {
        let mut request = request(
            day_and_after(),
            vec![slot],
            vec![host(
                "ana",
                vec![keyed_block(
                    utc(MONDAY, 11, 0),
                    utc(MONDAY, 12, 0),
                    &["interview"],
                    "evt_b",
                )],
            )],
            vec![cap_rule(
                "each_day",
                "interview",
                CapMeasure::Count,
                2,
                CapPeriod::Day,
                CapPer::Each,
            )],
            vec![Term::person("ana")],
            &["interview"],
        );
        request.occurrences = vec![
            occurrence(
                "evt_a",
                utc(MONDAY, 9, 0),
                utc(MONDAY, 10, 0),
                &["interview"],
            ),
            occurrence(
                "evt_b",
                utc(MONDAY, 11, 0),
                utc(MONDAY, 12, 0),
                &["interview"],
            ),
        ];
        request.with_exclude_event_keys(exclude.iter().map(|key| (*key).to_owned()).collect())
    };

    let diagnostics = engine.validate(&build(&[]));
    assert!(diagnostics.is_valid(), "{:?}", diagnostics.errors);
    assert_eq!(
        offered(&engine.find_availability(&build(&[])).unwrap()),
        [],
        "the occurrence alone is one of the day's two counted events"
    );
    assert_eq!(
        offered(&engine.find_availability(&build(&["evt_a"])).unwrap()),
        [slot],
        "naming the occurrence's key removes it from the day"
    );
}

#[test]
fn a_multi_host_panel_at_capacity_books_the_event_being_moved_for_every_host() {
    let engine = engine();
    let slot = utc(MONDAY, 14, 0);
    let panel = || {
        vec![
            host_at_daily_cap(),
            host(
                "bo",
                vec![
                    keyed_block(
                        utc(MONDAY, 9, 0),
                        utc(MONDAY, 10, 0),
                        &["interview"],
                        "evt_a",
                    ),
                    keyed_block(
                        utc(MONDAY, 11, 0),
                        utc(MONDAY, 12, 0),
                        &["interview"],
                        "evt_b",
                    ),
                ],
            ),
        ]
    };
    let build = |exclude: &[&str]| {
        request(
            day_and_after(),
            vec![slot],
            panel(),
            vec![cap_rule(
                "each_day",
                "interview",
                CapMeasure::Count,
                2,
                CapPeriod::Day,
                CapPer::Each,
            )],
            vec![Term::person("ana"), Term::person("bo")],
            &["interview"],
        )
        .with_exclude_event_keys(exclude.iter().map(|key| (*key).to_owned()).collect())
    };

    // Both interviewers are at their own daily limit, so the panel cannot meet.
    assert_eq!(offered(&engine.find_availability(&build(&[])).unwrap()), []);
    let with = engine.find_availability(&build(&["evt_a"])).unwrap();
    assert_eq!(offered(&with), [slot]);
    assert_eq!(
        with.offers[0].assigned_hosts(),
        ["ana", "bo"],
        "the panel is the same two hosts after the move"
    );
    let panel_proposal = Proposal::single(
        "main",
        slot,
        vec![vec!["ana".to_owned()], vec!["bo".to_owned()]],
    );
    assert!(
        engine
            .check_slot(&build(&["evt_a"]), &panel_proposal)
            .unwrap()
            .valid
    );
}

#[test]
fn an_earlier_start_after_a_late_event_is_bookable_when_that_event_moves() {
    let engine = engine();
    let slot = utc(TUESDAY, 9, 0);
    // The meeting being moved ran late on Monday, which protects Tuesday before
    // ten under the rest rule. Moving it means that late finish is no longer
    // activity, so the new morning slot does not trip rest.
    let late = || {
        host(
            "ana",
            vec![keyed_block(
                utc(MONDAY, 18, 30),
                utc(MONDAY, 20, 0),
                &[],
                "evt_late",
            )],
        )
    };
    let build = |exclude: &[&str]| {
        request(
            day_and_after(),
            vec![slot],
            vec![late()],
            vec![Rule::new(
                "rest",
                RuleKind::Rest(Rest::new(time(18, 0), time(10, 0))),
            )],
            vec![Term::person("ana")],
            &[],
        )
        .with_exclude_event_keys(exclude.iter().map(|key| (*key).to_owned()).collect())
    };

    assert_eq!(
        offered(&engine.find_availability(&build(&[])).unwrap()),
        [],
        "Monday's late finish protects Tuesday morning"
    );
    assert_eq!(
        offered(&engine.find_availability(&build(&["evt_late"])).unwrap()),
        [slot],
        "the event being moved is no longer rest activity"
    );
    assert!(
        engine
            .check_slot(&build(&["evt_late"]), &proposal(slot, &["ana"]))
            .unwrap()
            .valid
    );
}

#[test]
fn an_excluded_event_with_no_cap_or_rest_rule_still_stops_blocking_its_old_slot() {
    let engine = engine();
    let slot = utc(MONDAY, 14, 0);
    // Moving a meeting onto the hour its own old copy ends: the two spans touch,
    // which only a `clearance` gap would otherwise refuse. Without the exclusion
    // the old block is in the way; with it, the slot is free.
    let blocking = || {
        host(
            "ana",
            vec![keyed_block(
                utc(MONDAY, 13, 30),
                utc(MONDAY, 14, 0),
                &[],
                "evt_old",
            )],
        )
    };
    let rule = opportune::presets::buffers(Duration::from_minutes(10), Duration::from_minutes(10));
    let build = |exclude: &[&str]| {
        request(
            day_and_after(),
            vec![slot],
            vec![blocking()],
            vec![rule.clone()],
            vec![Term::person("ana")],
            &[],
        )
        .with_exclude_event_keys(exclude.iter().map(|key| (*key).to_owned()).collect())
    };

    assert_eq!(
        offered(&engine.find_availability(&build(&[])).unwrap()),
        [],
        "the old copy's clearance gap still reaches the new start"
    );
    assert_eq!(
        offered(&engine.find_availability(&build(&["evt_old"])).unwrap()),
        [slot],
        "the event being moved no longer stands in the way"
    );
}

#[test]
fn an_exclusion_list_longer_than_the_occurrence_cap_fails_closed() {
    let engine = Engine::builder()
        .limits(Limits::default().with_max_occurrences(2))
        .build()
        .expect("the bundled database builds");
    let request = request(
        day_and_after(),
        vec![utc(MONDAY, 14, 0)],
        vec![host_at_daily_cap()],
        Vec::new(),
        vec![Term::person("ana")],
        &[],
    )
    // The blank entry would be reported by an entry scan; the length is checked
    // first, so a document validation refuses is never read key by key.
    .with_exclude_event_keys(vec!["evt_a".to_owned(), String::new(), "evt_b".to_owned()]);

    let diagnostics = engine.validate(&request);
    assert_eq!(
        diagnostics
            .errors
            .iter()
            .map(|finding| (finding.code(), finding.path()))
            .collect::<Vec<_>>(),
        [("exclude_event_keys_limit", Some("/exclude_event_keys"))],
        "one finding for the length, and no per-entry scan"
    );
}

#[test]
fn an_empty_or_whitespace_excluded_key_names_its_own_index() {
    let engine = engine();
    let request = request(
        day_and_after(),
        vec![utc(MONDAY, 14, 0)],
        vec![host_at_daily_cap()],
        Vec::new(),
        vec![Term::person("ana")],
        &[],
    )
    .with_exclude_event_keys(vec!["evt_a".to_owned(), " ".to_owned(), String::new()]);

    let diagnostics = engine.validate(&request);
    assert_eq!(
        diagnostics
            .errors
            .iter()
            .map(|finding| (finding.code(), finding.path()))
            .collect::<Vec<_>>(),
        [
            ("event_key_empty", Some("/exclude_event_keys/1")),
            ("event_key_empty", Some("/exclude_event_keys/2")),
        ],
        "each empty entry is named at its own index, and the real key is not"
    );
}

/// The length bound is a request validation rule rather than a rule about
/// searching: a caller that never calls `validate` still cannot make a search or a
/// confirmation build an exclusion set past the occurrence cap.
#[test]
fn an_over_long_exclusion_list_is_refused_by_search_and_by_confirmation() {
    fn findings(outcome: Result<(), Error>, what: &str) -> Vec<(String, Option<String>)> {
        match outcome.err() {
            None => panic!("{what} refuses the request"),
            Some(Error::InvalidRequest { errors, .. }) => errors
                .iter()
                .map(|finding| (finding.code().to_owned(), finding.path().map(str::to_owned)))
                .collect(),
            Some(other) => panic!("{what} reports an invalid request, not `{other}`"),
        }
    }

    let engine = Engine::builder()
        .limits(Limits::default().with_max_occurrences(2))
        .build()
        .expect("the bundled database builds");
    let slot = utc(MONDAY, 14, 0);
    let build = || {
        request(
            day_and_after(),
            vec![slot],
            vec![host_at_daily_cap()],
            Vec::new(),
            vec![Term::person("ana")],
            &[],
        )
        .with_exclude_event_keys(vec![
            "evt_a".to_owned(),
            "evt_b".to_owned(),
            "evt_c".to_owned(),
        ])
    };

    let expected = [(
        "exclude_event_keys_limit".to_owned(),
        Some("/exclude_event_keys".to_owned()),
    )];
    assert_eq!(
        findings(engine.find_availability(&build()).map(|_| ()), "a search"),
        expected,
        "a search names the list and reports nothing else"
    );
    assert_eq!(
        findings(
            engine
                .check_slot(&build(), &proposal(slot, &["ana"]))
                .map(|_| ()),
            "a confirmation",
        ),
        expected,
        "a confirmation refuses the same document"
    );
}

/// The bound is *past* the occurrence cap, so a list of exactly that many keys is
/// read and every key in it is honoured: a caller moving the events a raised cap
/// allows is neither refused nor half-read.
#[test]
fn an_exclusion_list_at_the_occurrence_cap_excludes_every_key_it_names() {
    let engine = Engine::builder()
        .limits(Limits::default().with_max_occurrences(2))
        .build()
        .expect("the bundled database builds");
    let slot = utc(MONDAY, 14, 0);
    // A cap of one over a day of two events: dropping only one of the two keys
    // still leaves the day full, so a list read to its first key alone cannot
    // offer this slot.
    let request = request(
        day_and_after(),
        vec![slot],
        vec![host_at_daily_cap()],
        vec![cap_rule(
            "each_day",
            "interview",
            CapMeasure::Count,
            1,
            CapPeriod::Day,
            CapPer::Each,
        )],
        vec![Term::person("ana")],
        &["interview"],
    )
    .with_exclude_event_keys(vec!["evt_a".to_owned(), "evt_b".to_owned()]);

    let diagnostics = engine.validate(&request);
    assert!(diagnostics.is_valid(), "{:?}", diagnostics.errors);
    assert_eq!(
        offered(&engine.find_availability(&request).unwrap()),
        [slot],
        "both of the day's two counted events are moving, so the day holds none"
    );
}

#[test]
fn many_keys_and_many_events_answer_cleanly_under_the_event_limits() {
    let engine = engine();
    // Five hundred keyed meetings across two hosts, and an exclusion list of a
    // thousand keys: one real, the rest duplicates and keys no event carries. The
    // lookup is a set, so neither list is scanned per event.
    let blocks = |offset: usize| {
        (0..250)
            .map(|index| {
                let start = utc(MONDAY, 0, 0).minutes() + i64::try_from(index * 2).unwrap();
                let start = Instant::from_minutes(start).unwrap();
                let end = Instant::from_minutes(start.minutes() + 1).unwrap();
                keyed_block(
                    start,
                    end,
                    &["interview"],
                    &format!("evt_{}", offset + index),
                )
            })
            .collect()
    };
    let mut keys: Vec<String> = vec!["evt_0".to_owned()];
    keys.extend((0..999).map(|index| format!("unrelated_{}", index % 7)));
    let request = request(
        day_and_after(),
        vec![utc(MONDAY, 14, 0)],
        vec![
            tagged_host("ana", blocks(0)),
            tagged_host("bo", blocks(250)),
        ],
        Vec::new(),
        vec![Term::at_least(1, Selector::Tag(tag("interview")))],
        &["interview"],
    )
    .with_exclude_event_keys(keys);

    let diagnostics = engine.validate(&request);
    assert!(diagnostics.is_valid(), "{:?}", diagnostics.errors);
    let response = engine.find_availability(&request).unwrap();
    assert_eq!(
        offered(&response),
        [utc(MONDAY, 14, 0)],
        "the slot is free whatever the exclusion list holds"
    );
}
