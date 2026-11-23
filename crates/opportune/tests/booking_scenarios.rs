//! End-to-end booking-product scenarios driven directly through the public
//! Rust `Engine` and `presets` API.
//!
//! Together with the other three booking suites —
//! `crates/opportune-py/tests/test_booking_scenarios.py`,
//! `crates/opportune-wasm/js/tests/booking_scenarios.test.mjs` and
//! `crates/opportune-json/tests/booking_persistence_test.rs` — these tests prove
//! the core scheduling workflows across every target language and the wire form a
//! caller stores them in: field-service travel composing with clearance and
//! prior-job travel, split and per-weekday hours, multi-duration public links,
//! multi-resource sales calls with optional room tiering, conference pools,
//! event-type caps, fast last-second slot confirmation with correlation IDs also
//! intact, and a stored request round-tripping byte-identically through canonical
//! JSON.

#![cfg(feature = "bundled-tzdb")]

use std::collections::BTreeMap;

use opportune::Engine;
use opportune::model::{
    Block, Explain, Goal, LeadTimeMax, Part, Participant, ParticipantKind, ParticipantTravel,
    Proposal, ProposalEvent, Request, Requirement, Response, Rule, RuleKind, Span, Term, Travel,
};
use opportune::presets;
use opportune::reasons::{ReasonCode, ReasonParams};
use opportune::tags::{Role, Selector, Tag};
use opportune::time::{
    Date, DateKey, DateRange, Duration, Grid, GridZone, Instant, LocalResolution, RuleZone,
    TimeOfDay, WallEnd, WallRange, Weekday, Weekdays, WeeklyKey, WindowSpec, Zone, ZoneName,
};

const NY: &str = "America/New_York";

fn engine() -> Engine {
    Engine::builder()
        .build()
        .expect("bundled time zone database builds")
}

fn zone(name: &str) -> ZoneName {
    ZoneName::new(name).expect("valid IANA zone name")
}

fn tag(path: &str) -> Tag {
    Tag::new(path).expect("valid tag path")
}

fn time(hour: u8, minute: u8) -> TimeOfDay {
    TimeOfDay::new(hour, minute).expect("valid time of day")
}

fn range(from: (u8, u8), until: (u8, u8)) -> WallRange {
    WallRange::new(time(from.0, from.1), WallEnd::Time(time(until.0, until.1)))
        .expect("valid wall range")
}

fn utc(year: i16, month: i8, day: i8, hour: u8, minute: u8) -> Instant {
    let date = Date::new(year, month, day).expect("valid date");
    match Zone::utc()
        .resolve_local(date, time(hour, minute))
        .expect("UTC instant in range")
    {
        LocalResolution::Unique(instant) => instant,
        LocalResolution::Skipped | LocalResolution::Repeated(_, _) => {
            unreachable!("UTC has no DST transitions")
        }
    }
}

fn span(start: Instant, end: Instant) -> Span {
    Span::new(start, end).expect("non-empty span")
}

/// The first offer of a search, with the exclusions named when there is none.
fn first_offer<'a>(response: &'a Response, context: &str) -> &'a opportune::model::Offer {
    response.offers.first().unwrap_or_else(|| {
        panic!(
            "{context}: the search offered nothing; it excluded {:?}",
            response
                .excluded
                .iter()
                .map(|entry| (entry.start, entry.reasons.first().map(|reason| reason.code)))
                .collect::<Vec<_>>()
        )
    })
}

fn host(id: &str, tags: &[&str], calendar: Vec<Block>) -> Participant {
    let mut participant = Participant::new(id, zone(NY));
    participant.tags = tags.iter().map(|t| tag(t)).collect();
    participant.calendar = Some(calendar);
    participant
}

fn resource(id: &str, tags: &[&str], calendar: Vec<Block>) -> Participant {
    host(id, tags, calendar).with_kind(ParticipantKind::Resource)
}

fn attendee(id: &str, zone_name: &str) -> Participant {
    let mut participant = Participant::attendee(id, zone(zone_name));
    participant.calendar = Some(Vec::new());
    participant
}

fn weekday_hours() -> Rule {
    presets::business_hours(
        RuleZone::Participant,
        Weekdays::range(Weekday::Monday, Weekday::Friday),
        time(9, 0),
        WallEnd::Time(time(17, 0)),
    )
    .expect("weekday business hours")
}

fn days_with_offers(response: &Response) -> Vec<(String, usize)> {
    response
        .dates
        .iter()
        .filter(|entry| entry.offers > 0)
        .map(|entry| (entry.date.to_string(), entry.offers))
        .collect()
}

fn sales_team(room_b_busy: bool) -> Vec<Participant> {
    let busy_block = |key: &str| {
        let mut block = Block::new(span(utc(2027, 3, 8, 14, 0), utc(2027, 3, 8, 18, 0)));
        block.event_key = Some(key.to_owned());
        block
    };
    vec![
        host("sales-1", &["sales"], Vec::new()),
        host("se-1", &["sales-engineer"], Vec::new()),
        host("se-2", &["sales-engineer"], Vec::new()),
        resource("room-a", &["room"], vec![busy_block("room-a-busy")]),
        resource(
            "room-b",
            &["room"],
            if room_b_busy {
                vec![busy_block("room-b-busy")]
            } else {
                Vec::new()
            },
        ),
    ]
}

/// A field-service visit: an hour on site with travel either side, following a
/// prior job whose own travel runs into the evening.
fn field_service_request() -> Request {
    let mut prior_job = Block::new(span(utc(2027, 3, 8, 18, 0), utc(2027, 3, 8, 20, 0)));
    prior_job.tags = vec![tag("job/plumbing")];
    prior_job.event_key = Some("job-101".to_owned());
    prior_job.travel_after = Duration::from_minutes(20);

    let visit = Part {
        id: "visit".to_owned(),
        tags: vec![tag("job/plumbing")],
        travel: Travel {
            before: Duration::from_minutes(25),
            after: Duration::from_minutes(25),
            by_participant: BTreeMap::from([(
                "joe".to_owned(),
                ParticipantTravel {
                    before: Duration::ZERO,
                    after: Duration::ZERO,
                },
            )]),
        },
        ..Part::new(
            Duration::from_minutes(60),
            Requirement::new(vec![Term::person("bob")]),
        )
    };

    let mut goal = Goal::new(
        zone(NY),
        span(utc(2027, 3, 8, 18, 0), utc(2027, 3, 8, 23, 0)),
        vec![visit],
    );
    goal.candidates = Some(vec![utc(2027, 3, 8, 20, 54), utc(2027, 3, 8, 20, 55)]);

    let mut request = Request::new(utc(2027, 3, 6, 12, 0), goal);
    request.participants = vec![host("bob", &[], vec![prior_job]), attendee("joe", NY)];
    request.rules = vec![
        presets::business_hours(
            RuleZone::Participant,
            Weekdays::range(Weekday::Monday, Weekday::Saturday),
            time(8, 0),
            WallEnd::Time(time(18, 0)),
        )
        .expect("mon-sat hours"),
        presets::buffers(Duration::from_minutes(10), Duration::from_minutes(10)),
    ];
    request.explain = Some(Explain { limit: 10 });
    request
}

#[test]
fn field_service_travel_composes_with_clearance_and_exempts_customer() {
    let engine = engine();
    let request = field_service_request();

    let response = engine.find_availability(&request).expect("valid search");
    assert_eq!(response.offers.len(), 1);
    let offer = first_offer(&response, "field-service travel");
    assert_eq!(offer.start(), Some(utc(2027, 3, 8, 20, 55)));

    // Only Bob receives travel holds; Joe is at home (0m override).
    let holds: Vec<(&str, &str, Instant, Instant)> = offer.events[0]
        .reserve
        .iter()
        .map(|r| {
            (
                r.kind.as_str(),
                r.participant.as_str(),
                r.span.start(),
                r.span.end(),
            )
        })
        .collect();
    assert_eq!(
        holds,
        vec![
            (
                "travel",
                "bob",
                utc(2027, 3, 8, 20, 30),
                utc(2027, 3, 8, 20, 55),
            ),
            (
                "travel",
                "bob",
                utc(2027, 3, 8, 21, 55),
                utc(2027, 3, 8, 22, 20),
            ),
        ]
    );

    assert_eq!(response.excluded.len(), 1);
    assert_eq!(response.excluded[0].start, utc(2027, 3, 8, 20, 54));
    assert_eq!(response.excluded[0].reasons[0].code, ReasonCode::Clearance);
    assert_eq!(
        response.excluded[0].reasons[0].rule.as_deref(),
        Some("buffers")
    );

    // Confirming the offered slot preserves the exact travel holds; a start
    // inside the prior job's travel_after fails citing its event_key.
    let confirmed = engine
        .check_slot(&request, &offer.to_proposal())
        .expect("check_slot succeeds");
    assert!(confirmed.valid);
    assert_eq!(confirmed.reserve, offer.events[0].reserve);

    let too_soon = Proposal {
        events: vec![ProposalEvent {
            part: "visit".to_owned(),
            start: utc(2027, 3, 8, 20, 15),
            assignment: vec![vec!["bob".to_owned()]],
        }],
    };
    let refused = engine
        .check_slot(&request, &too_soon)
        .expect("check_slot runs");
    assert!(!refused.valid);
    assert_eq!(refused.reasons.len(), 1);
    assert_eq!(refused.reasons[0].code, ReasonCode::Blocked);
    assert_eq!(refused.reasons[0].participant.as_deref(), Some("bob"));
    assert_eq!(refused.reasons[0].event_key.as_deref(), Some("job-101"));
}

#[test]
fn split_and_per_weekday_working_hours_exclude_the_midday_gap() {
    let engine = engine();
    let spec = WindowSpec::new(
        RuleZone::Participant,
        vec![
            WeeklyKey::new(
                Weekdays::range(Weekday::Monday, Weekday::Thursday),
                vec![range((8, 30), (12, 0)), range((13, 0), (18, 0))],
            )
            .expect("mon-thu split"),
            WeeklyKey::new(
                Weekdays::single(Weekday::Friday),
                vec![range((9, 0), (15, 0))],
            )
            .expect("fri"),
            WeeklyKey::new(
                Weekdays::single(Weekday::Saturday),
                vec![range((10, 0), (12, 0))],
            )
            .expect("sat"),
        ],
        Vec::new(),
    )
    .expect("valid split window");

    let mut request = Request::new(
        utc(2027, 3, 6, 12, 0),
        Goal::new(
            zone(NY),
            span(utc(2027, 3, 8, 0, 0), utc(2027, 3, 15, 0, 0)),
            vec![Part::new(
                Duration::from_minutes(30),
                Requirement::new(vec![Term::person("ana")]),
            )],
        ),
    );
    request.participants = vec![host("ana", &[], Vec::new())];
    request.rules = vec![Rule::new("hours", RuleKind::Window(spec))];
    request.limit = Some(100);

    let response = engine.find_availability(&request).expect("valid search");
    assert_eq!(
        days_with_offers(&response),
        vec![
            ("2027-03-08".to_owned(), 17),
            ("2027-03-09".to_owned(), 17),
            ("2027-03-10".to_owned(), 17),
            ("2027-03-11".to_owned(), 17),
            ("2027-03-12".to_owned(), 12),
            ("2027-03-13".to_owned(), 4),
        ]
    );
    let lunch_overlap = response.offers.iter().any(|offer| {
        let start = offer.start().expect("single-part offer");
        start >= utc(2027, 3, 8, 16, 45) && start <= utc(2027, 3, 8, 17, 30)
    });
    assert!(!lunch_overlap, "no offer starts inside the 12:00-13:00 gap");
}

#[test]
fn a_split_event_window_intersects_the_personal_day() {
    // U16: the event runs a morning and an afternoon session, and a booking has to
    // fall in one of them *and* inside the host's personal hours.
    let engine = engine();
    let mut part = Part::new(
        Duration::from_minutes(30),
        Requirement::new(vec![Term::person("ana")]),
    );
    part.tags = vec![tag("session")];
    let mut request = Request::new(
        utc(2027, 3, 7, 12, 0),
        Goal::new(
            zone(NY),
            span(utc(2027, 3, 8, 5, 0), utc(2027, 3, 10, 5, 0)),
            vec![part],
        ),
    );
    request.participants = vec![host("ana", &[], Vec::new())];
    request.rules = vec![
        presets::business_hours(
            RuleZone::Participant,
            Weekdays::range(Weekday::Monday, Weekday::Friday),
            time(9, 0),
            WallEnd::Time(time(17, 0)),
        )
        .expect("personal hours"),
        presets::business_hours_ranges(
            RuleZone::Event,
            Weekdays::range(Weekday::Monday, Weekday::Friday),
            vec![range((10, 0), (12, 0)), range((14, 0), (16, 0))],
        )
        .expect("the event's two sessions")
        .with_id("event_sessions")
        .with_when(Selector::tag(tag("session"))),
    ];
    request.limit = Some(100);

    let response = engine.find_availability(&request).expect("valid search");

    assert_eq!(
        days_with_offers(&response),
        vec![("2027-03-08".to_owned(), 8), ("2027-03-09".to_owned(), 8)],
        "four half-hour starts in each session, on both days"
    );
    let starts: Vec<Instant> = response
        .offers
        .iter()
        .filter_map(opportune::Offer::start)
        .collect();
    // March in New York is UTC-5, so 10:00 local is 15:00Z.
    assert_eq!(
        starts.first(),
        Some(&utc(2027, 3, 8, 15, 0)),
        "the morning session opens"
    );
    for gap in [utc(2027, 3, 8, 17, 0), utc(2027, 3, 8, 18, 0)] {
        assert!(
            !starts.contains(&gap),
            "no start between the sessions or after the second"
        );
    }
}

#[test]
fn public_event_types_across_30_45_and_60_minute_durations() {
    let engine = engine();
    let mut day_one_counts = Vec::new();

    for minutes in [30_u32, 45, 60] {
        let mut goal = Goal::new(
            zone(NY),
            span(utc(2027, 3, 8, 0, 0), utc(2027, 3, 20, 0, 0)),
            vec![Part::new(
                Duration::from_minutes(minutes),
                Requirement::new(vec![Term::at_least(1, Selector::role(Role::Host))]),
            )],
        );
        goal.grid = Some(
            Grid::new(
                Duration::from_minutes(15),
                time(0, 0),
                GridZone::Named(zone(NY)),
            )
            .expect("15m grid"),
        );

        let mut request = Request::new(utc(2027, 3, 5, 12, 0), goal);
        request.participants = vec![
            host("ana", &[], Vec::new()),
            attendee("invitee", "Europe/Berlin"),
        ];
        request.rules = vec![
            weekday_hours(),
            presets::buffers(Duration::from_minutes(10), Duration::from_minutes(10)),
            presets::min_notice(Duration::from_minutes(240)),
            presets::horizon(LeadTimeMax::Days(30)),
            presets::rest(time(18, 0), time(10, 0)),
        ];
        request.limit = Some(20);

        let response = engine.find_availability(&request).expect("valid search");
        let offer = first_offer(&response, "evening-to-morning rest");
        assert!(offer.flags.is_empty());
        assert_eq!(offer.events[0].assignment, vec![vec!["ana".to_owned()]]);
        let day_one = response
            .dates
            .iter()
            .find(|entry| entry.date == Date::new(2027, 3, 8).expect("date"))
            .expect("first date present")
            .offers;
        day_one_counts.push(day_one);
    }

    assert_eq!(day_one_counts, vec![31, 30, 29]);
}

#[test]
fn multi_resource_sales_call_and_single_query_optional_room_tiering() {
    let engine = engine();
    let build = |room_count: u32, room_b_busy: bool, weights: BTreeMap<String, i32>| {
        let mut part = Part::new(
            Duration::from_minutes(60),
            Requirement::new(vec![
                Term::at_least(1, Selector::tag(tag("sales-engineer"))),
                Term::person("sales-1"),
                Term::at_least(room_count, Selector::tag(tag("room"))),
            ]),
        );
        part.id = "demo".to_owned();
        part.tags = vec![tag("demo")];
        let mut request = Request::new(
            utc(2027, 3, 6, 12, 0),
            Goal::new(
                zone(NY),
                span(utc(2027, 3, 8, 14, 0), utc(2027, 3, 8, 20, 0)),
                vec![part],
            ),
        );
        request.participants = sales_team(room_b_busy);
        request.rules = vec![
            weekday_hours(),
            presets::buffers(Duration::from_minutes(10), Duration::from_minutes(10)),
        ];
        request.weights = weights;
        request
    };

    let required = engine
        .find_availability(&build(
            1,
            false,
            BTreeMap::from([("se-1".to_owned(), 2), ("se-2".to_owned(), 8)]),
        ))
        .expect("required room search");
    assert_eq!(
        first_offer(&required, "required room").events[0].assignment,
        vec![
            vec!["se-2".to_owned()],
            vec!["sales-1".to_owned()],
            vec!["room-b".to_owned()],
        ]
    );

    // One query with `at_least(0)` returns both required-only slots and slots
    // where rooms are free in `eligible[2]`.
    let optional_req = build(0, true, BTreeMap::new());
    let optional = engine
        .find_availability(&optional_req)
        .expect("optional room search");
    let without_room: Vec<_> = optional
        .offers
        .iter()
        .filter(|o| o.events[0].eligible[2].is_empty())
        .collect();
    let with_room: Vec<_> = optional
        .offers
        .iter()
        .filter(|o| !o.events[0].eligible[2].is_empty())
        .collect();
    assert!(!without_room.is_empty());
    assert!(!with_room.is_empty());
    assert_eq!(
        with_room[0].events[0].eligible[2],
        vec!["room-a".to_owned(), "room-b".to_owned()]
    );
    assert_eq!(with_room[0].events[0].assignment[2], Vec::<String>::new());

    let with_room_proposal = Proposal {
        events: vec![ProposalEvent {
            part: "demo".to_owned(),
            start: with_room[0].start().expect("start"),
            assignment: vec![
                vec!["se-1".to_owned()],
                vec!["sales-1".to_owned()],
                vec!["room-b".to_owned()],
            ],
        }],
    };
    let rejected = engine
        .check_slot(&optional_req, &with_room_proposal)
        .expect_err("assigning a room into an at_least(0) term is malformed");
    assert_eq!(rejected.code(), "proposal_host_count");

    let confirmed = engine
        .check_slot(&build(1, true, BTreeMap::new()), &with_room_proposal)
        .expect("required-room check_slot succeeds");
    assert!(confirmed.valid);
}

#[test]
fn conference_pool_window_offers_only_during_conference_hours() {
    let engine = engine();
    let delegates: Vec<Participant> = (0..24)
        .map(|index| {
            let id = format!("gtm-{index:02}");
            let calendar = if index % 3 == 0 {
                let mut busy = Block::new(span(utc(2027, 3, 9, 14, 0), utc(2027, 3, 9, 20, 0)));
                busy.event_key = Some(format!("{id}-busy"));
                vec![busy]
            } else {
                Vec::new()
            };
            host(&id, &["gtm"], calendar)
        })
        .collect();

    let mut part = Part::new(
        Duration::from_minutes(20),
        Requirement::new(vec![Term::at_least(1, Selector::tag(tag("gtm")))]),
    );
    part.id = "booth".to_owned();
    part.tags = vec![tag("conference")];

    let conf_window = presets::dated_hours(
        RuleZone::Named(zone(NY)),
        vec![
            DateKey::new(
                DateRange::single(Date::new(2027, 3, 9).expect("d1")),
                vec![range((9, 0), (17, 0))],
            )
            .expect("d1"),
            DateKey::new(
                DateRange::single(Date::new(2027, 3, 10).expect("d2")),
                vec![range((9, 0), (17, 0))],
            )
            .expect("d2"),
            DateKey::new(
                DateRange::single(Date::new(2027, 3, 11).expect("d3")),
                vec![range((9, 0), (15, 0))],
            )
            .expect("d3"),
        ],
    )
    .expect("conference window")
    .with_id("conference_hours")
    .with_who(Selector::tag(tag("gtm")));

    let mut conf_req = Request::new(
        utc(2027, 3, 6, 12, 0),
        Goal::new(
            zone(NY),
            span(utc(2027, 3, 9, 0, 0), utc(2027, 3, 12, 0, 0)),
            vec![part],
        ),
    );
    conf_req.participants = delegates;
    conf_req.rules = vec![
        conf_window,
        presets::buffers(Duration::from_minutes(5), Duration::from_minutes(5)),
    ];
    conf_req.limit = Some(200);

    let conf_answer = engine.find_availability(&conf_req).expect("conf search");
    assert_eq!(conf_answer.offers.len(), 66);
    assert!(!conf_answer.truncated);
    assert_eq!(
        days_with_offers(&conf_answer),
        vec![
            ("2027-03-09".to_owned(), 24),
            ("2027-03-10".to_owned(), 24),
            ("2027-03-11".to_owned(), 18),
        ]
    );
}

#[test]
fn event_type_daily_cap_closes_day_only_for_matching_tagged_parts() {
    let engine = engine();
    let interview_block = |start: Instant, end: Instant, key: &str| {
        let mut b = Block::new(span(start, end));
        b.tags = vec![tag("interview")];
        b.event_key = Some(key.to_owned());
        b
    };
    let make_cap_req = |tags: Vec<Tag>| {
        let mut p = Part::new(
            Duration::from_minutes(45),
            Requirement::new(vec![Term::person("ana")]),
        );
        p.tags = tags;
        let mut req = Request::new(
            utc(2027, 3, 6, 12, 0),
            Goal::new(
                zone(NY),
                span(utc(2027, 3, 8, 0, 0), utc(2027, 3, 13, 0, 0)),
                vec![p],
            ),
        );
        req.participants = vec![host(
            "ana",
            &[],
            vec![
                interview_block(utc(2027, 3, 8, 14, 0), utc(2027, 3, 8, 14, 45), "int-1"),
                interview_block(utc(2027, 3, 8, 15, 0), utc(2027, 3, 8, 15, 45), "int-2"),
            ],
        )];
        req.rules = vec![
            weekday_hours(),
            presets::cap_per_day(Selector::tag(tag("interview")), 2),
        ];
        req.limit = Some(60);
        req.explain = Some(Explain { limit: 500 });
        req
    };

    let capped = engine
        .find_availability(&make_cap_req(vec![tag("interview")]))
        .expect("capped search");
    assert_eq!(
        days_with_offers(&capped),
        vec![
            ("2027-03-09".to_owned(), 15),
            ("2027-03-10".to_owned(), 15),
            ("2027-03-11".to_owned(), 15),
            ("2027-03-12".to_owned(), 15),
        ]
    );
    let cap_reason = capped
        .excluded
        .iter()
        .flat_map(|e| &e.reasons)
        .find(|r| r.code == ReasonCode::CapReached)
        .expect("cap_reached reported");
    assert_eq!(
        cap_reason.params,
        Some(ReasonParams::CapReached {
            used: 2,
            limit: 2,
            bucket: "2027-03-08".to_owned(),
        })
    );

    let untagged = engine
        .find_availability(&make_cap_req(Vec::new()))
        .expect("untagged search");
    let day_one = untagged
        .dates
        .iter()
        .find(|e| e.date == Date::new(2027, 3, 8).expect("d"))
        .expect("day one");
    assert_eq!(day_one.offers, 11);
}

#[test]
fn last_second_check_slot_catches_race_with_event_key_and_allows_off_grid_override() {
    let engine = engine();
    let mut part = Part::new(
        Duration::from_minutes(30),
        Requirement::new(vec![Term::person("ana")]),
    );
    part.id = "consult".to_owned();
    let mut goal = Goal::new(
        zone(NY),
        span(utc(2027, 3, 8, 14, 0), utc(2027, 3, 8, 18, 0)),
        vec![part],
    );
    goal.grid = Some(
        Grid::new(
            Duration::from_minutes(30),
            time(0, 0),
            GridZone::Named(zone(NY)),
        )
        .expect("30m grid"),
    );

    let mut request = Request::new(utc(2027, 3, 8, 12, 0), goal);
    request.participants = vec![host("ana", &[], Vec::new())];
    request.rules = vec![
        weekday_hours(),
        presets::buffers(Duration::from_minutes(10), Duration::from_minutes(10)),
        presets::min_notice(Duration::from_minutes(120)),
    ];

    let listed = engine.find_availability(&request).expect("list slots");
    let offer = first_offer(&listed, "off-grid override");
    let proposal = offer.to_proposal();
    assert!(engine.check_slot(&request, &proposal).expect("check").valid);

    // Concurrent booking arrives right before commit.
    let mut raced = request.clone();
    let mut race_block = Block::new(span(offer.events[0].start, offer.events[0].end));
    race_block.event_key = Some("booking-race-42".to_owned());
    race_block.tags = vec![tag("consult")];
    raced.participants[0].calendar = Some(vec![race_block]);

    let race_check = engine.check_slot(&raced, &proposal).expect("race check");
    assert!(!race_check.valid);
    assert_eq!(race_check.reasons.len(), 1);
    assert_eq!(race_check.reasons[0].code, ReasonCode::Blocked);
    assert_eq!(race_check.reasons[0].participant.as_deref(), Some("ana"));
    assert_eq!(
        race_check.reasons[0].event_key.as_deref(),
        Some("booking-race-42")
    );
    assert_eq!(race_check.reasons[0].tags, vec![tag("consult")]);

    // Internal scheduling override: off-grid start outside the original search window.
    let off_grid = Proposal {
        events: vec![ProposalEvent {
            part: "consult".to_owned(),
            start: utc(2027, 3, 9, 14, 5),
            assignment: vec![vec!["ana".to_owned()]],
        }],
    };
    assert!(
        engine
            .check_slot(&request, &off_grid)
            .expect("override")
            .valid
    );
}
