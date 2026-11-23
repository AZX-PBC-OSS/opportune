//! The pipeline's answer against an independent reference.
//!
//! `find_availability` decides eligibility from interval sets built over the
//! padded range, while the wall-clock rules themselves answer instant membership
//! directly. Those are two routes to the same answer, and this file keeps them
//! honest: every candidate the reference admits minute by minute must be
//! offered, with the same eligible pool, assignment, reserve and flags.
//!
//! The reference reads `WindowSpec::contains` and `BlackoutSpec::contains` per
//! minute and per participant, so a padded range that is too narrow, an allowed
//! set clipped to the search window, a clearance applied on the wrong side, a
//! blackout widened with clearance, or one participant's rules, calendar or
//! travel resolved against another's, shows up as a disagreement rather than as
//! a plausible answer.
//!
//! `when` applicability is left constant here — every rule applies to every
//! booking — because `who_and_when_decide_which_rules_apply` pins that wiring;
//! `who` varies, so per-participant rule selection is exercised. The deeper
//! generated oracle belongs to `opportune-testkit`.

#![cfg(feature = "bundled-tzdb")]

use std::collections::BTreeMap;

use opportune::model::{
    Block, BlockStatus, Clearance, Goal, LeadTime, Offer, Part, Participant, ParticipantTravel,
    Request, Requirement, Rule, RuleKind, Span, StatusEffect, StatusPolicy, StatusPolicyEntry,
    Term, Travel, ZonePeriod,
};
use opportune::tags::{ParticipantContext, Role, Selector, Tag};
use opportune::time::{
    BlackoutSpan, BlackoutSpec, Date, Duration, Grid, GridZone, Instant, LocalDateTime,
    LocalResolution, RuleZone, TimeOfDay, TimeZoneDatabase, WallEnd, WallRange, Weekday, Weekdays,
    WeeklyKey, WindowSpec, Zone, ZoneName, ZoneSchedule,
};
use opportune::{Engine, RangesQuery};

fn name(zone: &str) -> ZoneName {
    ZoneName::new(zone).unwrap()
}

fn utc(year: i16, month: i8, day: i8, hour: u8, minute: u8) -> Instant {
    let date = Date::new(year, month, day).unwrap();
    let time = TimeOfDay::new(hour, minute).unwrap();
    let LocalResolution::Unique(instant) = Zone::utc().resolve_local(date, time).unwrap() else {
        panic!("UTC has no transitions");
    };
    instant
}

fn span(start: Instant, end: Instant) -> Span {
    Span::new(start, end).unwrap()
}

fn tag(path: &str) -> Tag {
    Tag::new(path).unwrap()
}

fn date(year: i16, month: i8, day: i8) -> Date {
    Date::new(year, month, day).unwrap()
}

fn local(date: Date, hour: u8, minute: u8) -> LocalDateTime {
    LocalDateTime::new(date, TimeOfDay::new(hour, minute).unwrap())
}

fn block(spans: (Instant, Instant)) -> Block {
    Block::new(span(spans.0, spans.1))
}

/// A window open every day from `start` to `end`, in `zone`.
fn daily(zone: RuleZone, start: (u8, u8), end: (u8, u8)) -> WindowSpec {
    let range = WallRange::new(
        TimeOfDay::new(start.0, start.1).unwrap(),
        WallEnd::Time(TimeOfDay::new(end.0, end.1).unwrap()),
    )
    .unwrap();
    WindowSpec::new(
        zone,
        vec![
            WeeklyKey::new(
                Weekdays::range(Weekday::Monday, Weekday::Sunday),
                vec![range],
            )
            .unwrap(),
        ],
        Vec::new(),
    )
    .unwrap()
}

fn blackout_in(zone: RuleZone, spans: Vec<(LocalDateTime, LocalDateTime)>) -> BlackoutSpec {
    BlackoutSpec::new(
        zone,
        spans
            .into_iter()
            .map(|(start, end)| BlackoutSpan::new(start, end).unwrap())
            .collect(),
    )
}

/// One participant in a scenario.
struct Host {
    id: &'static str,
    zone: &'static str,
    periods: Vec<(&'static str, Instant, Instant)>,
    /// `None` means the host supplied no calendar.
    calendar: Option<Vec<Block>>,
    /// `None` means the part's travel applies to this host.
    travel: Option<(u32, u32)>,
}

impl Host {
    /// A host that reported an empty calendar: known to be free.
    fn free(id: &'static str, zone: &'static str) -> Self {
        Self {
            id,
            zone,
            periods: Vec::new(),
            calendar: Some(Vec::new()),
            travel: None,
        }
    }
}

/// One request the reference and the engine both answer.
struct Scenario {
    hosts: Vec<Host>,
    event_zone: &'static str,
    window: Option<WindowSpec>,
    blackout: Option<BlackoutSpec>,
    rules: Vec<Rule>,
    /// The part's travel, for hosts without an override.
    travel: (u32, u32),
    now: Instant,
    search: Span,
    /// Grid step in minutes, anchored at midnight in the event zone.
    step: u32,
    duration: u32,
    candidates: Option<Vec<Instant>>,
}

impl Scenario {
    /// The rules that come from the scenario's own window and blackout.
    fn with_kinds(mut self) -> Self {
        if let Some(window) = self.window.clone() {
            self.rules
                .insert(0, Rule::new("hours", RuleKind::Window(window)));
        }
        if let Some(blackout) = self.blackout.clone() {
            self.rules
                .insert(0, Rule::new("closed", RuleKind::Blackout(blackout)));
        }
        self
    }

    /// The grid's candidate starts, or the supplied ones.
    fn candidates(&self, event: &Zone) -> Vec<Instant> {
        match &self.candidates {
            Some(candidates) => candidates.clone(),
            None => Grid::new(
                Duration::from_minutes(self.step),
                TimeOfDay::MIDNIGHT,
                GridZone::Event,
            )
            .unwrap()
            .starts(event, self.search.start(), self.search.end())
            .unwrap(),
        }
    }

    /// The travel `host` needs, as `(before, after)`.
    fn travel_for(&self, host: &Host) -> (i64, i64) {
        let (before, after) = host.travel.unwrap_or(self.travel);
        (i64::from(before), i64::from(after))
    }

    fn schedule(db: &TimeZoneDatabase, host: &Host) -> ZoneSchedule {
        let mut periods = Vec::new();
        for (zone, from, until) in &host.periods {
            periods.push(
                opportune::time::ZonePeriod::new(db.zone(zone).unwrap(), *from, *until).unwrap(),
            );
        }
        ZoneSchedule::new(db.zone(host.zone).unwrap(), periods).unwrap()
    }
}

/// One host with everything the reference resolves once: its applicable rules,
/// its zone function, and the clearance the booking needs around its blocks.
struct Ready<'a> {
    host: &'a Host,
    travel: (i64, i64),
    lead_min: Duration,
    windows: Vec<(&'a WindowSpec, ZoneSchedule)>,
    blackouts: Vec<(&'a BlackoutSpec, ZoneSchedule)>,
    /// Only the blocks that block, with their occupied spans and the widening
    /// applied.
    blocked: Vec<(i64, i64)>,
}

impl<'a> Ready<'a> {
    fn new(db: &TimeZoneDatabase, scenario: &'a Scenario, host: &'a Host) -> Self {
        let event = db.zone(scenario.event_zone).unwrap();
        let schedule = Scenario::schedule(db, host);
        let rules: Vec<&Rule> = scenario
            .rules
            .iter()
            .filter(|rule| {
                rule.who.matches_participant(ParticipantContext {
                    id: host.id,
                    kind: opportune::model::ParticipantKind::Person,
                    role: Role::Host,
                    tags: &[],
                })
            })
            .collect();

        let lead_min = rules
            .iter()
            .filter_map(|rule| match &rule.kind {
                RuleKind::LeadTime(lead) => Some(lead.min),
                _ => None,
            })
            .fold(Duration::ZERO, Duration::max);
        let clearance = rules
            .iter()
            .filter_map(|rule| match &rule.kind {
                RuleKind::Clearance(clearance) => Some(*clearance),
                _ => None,
            })
            // Combined here rather than through the engine's own helper, so this
            // oracle stays independent of the code it checks.
            .fold(Clearance::default(), |combined, clearance| Clearance {
                before: combined.before.max(clearance.before),
                after: combined.after.max(clearance.after),
                min_gap: combined.min_gap.max(clearance.min_gap),
            });
        // A block is widened at the end by the booking's `before` gap and at the
        // start by its `after` gap, each against the least gap.
        let at_start = clearance.after.max(clearance.min_gap).minutes();
        let at_end = clearance.before.max(clearance.min_gap).minutes();

        let windows = rules
            .iter()
            .filter_map(|rule| match &rule.kind {
                RuleKind::Window(spec) => Some((
                    spec,
                    spec.zone()
                        .resolve(db, &schedule, &event)
                        .expect("the scenario's zones resolve"),
                )),
                _ => None,
            })
            .collect();
        let blackouts = rules
            .iter()
            .filter_map(|rule| match &rule.kind {
                RuleKind::Blackout(spec) => Some((
                    spec,
                    spec.zone()
                        .resolve(db, &schedule, &event)
                        .expect("the scenario's zones resolve"),
                )),
                _ => None,
            })
            .collect();

        let blocked = host
            .calendar
            .iter()
            .flatten()
            .filter(|block| holds(&rules, block))
            .map(|block| {
                let (start, end) = occupied_minutes(block);
                (start.saturating_sub(at_start), end.saturating_add(at_end))
            })
            .collect();

        Self {
            host,
            travel: scenario.travel_for(host),
            lead_min,
            windows,
            blackouts,
            blocked,
        }
    }

    /// Whether this host can take a booking starting at `start`.
    fn admits(&self, scenario: &Scenario, start: Instant) -> bool {
        let (travel_before, travel_after) = self.travel;
        let occupied_start = start.minutes() - travel_before;
        let occupied_end = start.minutes() + i64::from(scenario.duration) + travel_after;
        if occupied_start < scenario.now.minutes() + self.lead_min.minutes() {
            return false;
        }
        let at = |minute: i64| Instant::from_minutes(minute).expect("inside the search window");
        if !self.windows.iter().all(|(spec, zones)| {
            (occupied_start..occupied_end).all(|minute| spec.contains(at(minute), zones))
        }) {
            return false;
        }
        if self.blackouts.iter().any(|(spec, zones)| {
            (occupied_start..occupied_end).any(|minute| spec.contains(at(minute), zones))
        }) {
            return false;
        }
        !self
            .blocked
            .iter()
            .any(|(start, end)| *start < occupied_end && occupied_start < *end)
    }
}

/// The blocking decision the status policies make: first match across rules in
/// request order.
fn holds(rules: &[&Rule], block: &Block) -> bool {
    let effect = rules.iter().find_map(|rule| match &rule.kind {
        RuleKind::StatusPolicy(policy) => policy.decide(block.status, &block.tags),
        _ => None,
    });
    match effect {
        Some(StatusEffect::Ignore) => false,
        Some(StatusEffect::Block) => true,
        None => block.status != BlockStatus::Free,
    }
}

fn occupied_minutes(block: &Block) -> (i64, i64) {
    (
        block.span.start().minutes() - block.travel_before.minutes(),
        block.span.end().minutes() + block.travel_after.minutes(),
    )
}

/// What one offer should look like, whichever route produced it.
#[derive(Debug, PartialEq, Eq)]
struct OfferShape {
    start: Instant,
    eligible: Vec<String>,
    assigned: String,
    reserve: Vec<(String, Instant, Instant)>,
    calendar_unknown: bool,
}

/// Every offer the reference admits, in start order: one minute at a time and
/// one participant at a time, with instant membership rather than the interval
/// builders.
fn reference(db: &TimeZoneDatabase, scenario: &Scenario) -> Vec<OfferShape> {
    let event = db.zone(scenario.event_zone).unwrap();
    let ready: Vec<Ready<'_>> = scenario
        .hosts
        .iter()
        .map(|host| Ready::new(db, scenario, host))
        .collect();

    let mut shapes = Vec::new();
    for start in scenario.candidates(&event) {
        if !scenario.search.contains(start) {
            continue;
        }
        let mut eligible: Vec<&str> = ready
            .iter()
            .filter(|host| host.admits(scenario, start))
            .map(|host| host.host.id)
            .collect();
        if eligible.is_empty() {
            continue;
        }
        eligible.sort_unstable();
        let assigned = *eligible.first().expect("checked non-empty");
        let host = scenario
            .hosts
            .iter()
            .find(|host| host.id == assigned)
            .expect("an eligible host is a scenario host");
        let (before, after) = scenario.travel_for(host);
        let mut reserve = Vec::new();
        if before > 0 {
            reserve.push((
                assigned.to_owned(),
                Instant::from_minutes(start.minutes() - before).expect("inside the range"),
                start,
            ));
        }
        if after > 0 {
            let end = Instant::from_minutes(start.minutes() + i64::from(scenario.duration))
                .expect("inside the range");
            reserve.push((
                assigned.to_owned(),
                end,
                Instant::from_minutes(end.minutes() + after).expect("inside the range"),
            ));
        }
        shapes.push(OfferShape {
            start,
            eligible: eligible.iter().map(|id| (*id).to_owned()).collect(),
            assigned: assigned.to_owned(),
            reserve,
            calendar_unknown: host.calendar.is_none(),
        });
    }
    shapes
}

/// The response's offers in the same shape.
fn offers(engine: &Engine, scenario: &Scenario) -> Vec<OfferShape> {
    let mut participants = Vec::new();
    let mut overrides = BTreeMap::new();
    for host in &scenario.hosts {
        let mut participant = Participant::new(host.id, name(host.zone));
        participant.calendar.clone_from(&host.calendar);
        for (zone, from, until) in &host.periods {
            participant
                .zone_periods
                .push(ZonePeriod::new(name(zone), *from, *until).unwrap());
        }
        participants.push(participant);
        if let Some((before, after)) = host.travel {
            overrides.insert(
                host.id.to_owned(),
                ParticipantTravel {
                    before: Duration::from_minutes(before),
                    after: Duration::from_minutes(after),
                },
            );
        }
    }
    let mut part = Part::new(
        Duration::from_minutes(scenario.duration),
        Requirement::new(vec![Term::at_least(1, Selector::True)]),
    );
    part.travel = Travel {
        before: Duration::from_minutes(scenario.travel.0),
        after: Duration::from_minutes(scenario.travel.1),
        by_participant: overrides,
    };
    let mut goal = Goal::new(name(scenario.event_zone), scenario.search, vec![part]);
    if let Some(candidates) = &scenario.candidates {
        goal.candidates = Some(candidates.clone());
    } else {
        goal.grid = Some(
            Grid::new(
                Duration::from_minutes(scenario.step),
                TimeOfDay::MIDNIGHT,
                GridZone::Event,
            )
            .unwrap(),
        );
    }
    let mut request = Request::new(scenario.now, goal);
    request.participants = participants;
    request.rules.clone_from(&scenario.rules);

    let response = engine.find_availability(&request).unwrap();
    response
        .offers
        .iter()
        .map(|offer| {
            let event = offer.events.first().expect("one part, one event");
            OfferShape {
                start: event.start,
                eligible: event.eligible[0].clone(),
                assigned: event.assignment[0][0].clone(),
                reserve: event
                    .reserve
                    .iter()
                    .map(|reserve| {
                        (
                            reserve.participant.clone(),
                            reserve.span.start(),
                            reserve.span.end(),
                        )
                    })
                    .collect(),
                calendar_unknown: offer.calendar_unknown(),
            }
        })
        .collect()
}

fn compare(label: &str, scenario: &Scenario) {
    let engine = Engine::builder().build().unwrap();
    let expected = reference(engine.time_zone_database(), scenario);
    let actual = offers(&engine, scenario);
    assert_eq!(
        actual, expected,
        "{label}: the engine's offers disagree with minute-by-minute membership"
    );
}

#[test]
fn a_window_split_by_a_fall_back_matches_minute_membership() {
    // New York falls back at 02:00 local on 2027-11-07, so a 00:00-04:00 window
    // is two intervals and a booking's occupied span may sit in either.
    compare(
        "fall-back window with travel",
        &Scenario {
            hosts: vec![Host::free("a", "America/New_York")],
            event_zone: "America/New_York",
            window: Some(daily(RuleZone::Participant, (0, 0), (4, 0))),
            blackout: None,
            rules: Vec::new(),
            travel: (30, 30),
            now: utc(2027, 11, 6, 0, 0),
            search: span(utc(2027, 11, 7, 0, 0), utc(2027, 11, 8, 0, 0)),
            step: 15,
            duration: 45,
            candidates: None,
        }
        .with_kinds(),
    );
}

#[test]
fn blocks_blackouts_and_clearance_agree_with_minute_membership() {
    let day = date(2027, 3, 10);
    let mut first = block((utc(2027, 3, 10, 10, 0), utc(2027, 3, 10, 11, 0)));
    first.travel_before = Duration::from_minutes(15);
    let mut second = block((utc(2027, 3, 10, 14, 0), utc(2027, 3, 10, 15, 0)));
    second.travel_after = Duration::from_minutes(10);
    let mut ignored = block((utc(2027, 3, 10, 16, 0), utc(2027, 3, 10, 17, 0)));
    ignored.status = BlockStatus::Free;
    ignored.tags = vec![tag("status/focus")];
    let mut host = Host::free("a", "UTC");
    host.calendar = Some(vec![first, second, ignored]);
    compare(
        "blocks with travel and clearance",
        &Scenario {
            hosts: vec![host],
            event_zone: "UTC",
            window: Some(daily(RuleZone::Named(name("UTC")), (8, 0), (18, 0))),
            blackout: Some(blackout_in(
                RuleZone::Named(name("UTC")),
                vec![(local(day, 12, 0), local(day, 12, 45))],
            )),
            rules: vec![
                Rule::new(
                    "buffers",
                    RuleKind::Clearance(Clearance {
                        before: Duration::from_minutes(10),
                        after: Duration::from_minutes(20),
                        min_gap: Duration::from_minutes(15),
                    }),
                ),
                Rule::new(
                    "ignore-focus",
                    RuleKind::StatusPolicy(StatusPolicy {
                        entries: vec![StatusPolicyEntry::tags(
                            Selector::Tag(tag("status/focus")),
                            StatusEffect::Ignore,
                        )],
                    }),
                ),
            ],
            travel: (10, 25),
            now: utc(2027, 3, 10, 0, 0),
            search: span(utc(2027, 3, 10, 7, 0), utc(2027, 3, 10, 19, 0)),
            step: 5,
            duration: 50,
            candidates: None,
        }
        .with_kinds(),
    );
}

#[test]
fn a_zone_period_switch_inside_the_window_agrees_with_minute_membership() {
    let mut host = Host::free("a", "Asia/Tokyo");
    host.periods = vec![(
        "America/Los_Angeles",
        utc(2027, 3, 9, 16, 0),
        utc(2027, 3, 10, 0, 0),
    )];
    compare(
        "zone period switch",
        &Scenario {
            hosts: vec![host],
            event_zone: "UTC",
            window: Some(daily(RuleZone::Participant, (8, 0), (12, 0))),
            blackout: Some(blackout_in(
                RuleZone::Participant,
                vec![(
                    local(date(2027, 3, 9), 9, 0),
                    local(date(2027, 3, 10), 10, 0),
                )],
            )),
            rules: Vec::new(),
            travel: (15, 15),
            now: utc(2027, 3, 9, 0, 0),
            search: span(utc(2027, 3, 9, 0, 0), utc(2027, 3, 10, 6, 0)),
            step: 15,
            duration: 30,
            candidates: None,
        }
        .with_kinds(),
    );
}

/// A pool whose hosts differ in travel and calendar, so the eligible list, the
/// chosen assignment, the reserve span and the flag all depend on resolving
/// *each* participant's own settings.
///
/// With a 09:00-12:00 UTC window and a 30-minute booking: at 09:15 `a`'s own
/// 20-minute journey starts before the window, so only the calendar-less `c` can
/// take it and the offer is flagged; at 09:30 `a` is clear of its block and wins
/// the assignment with its own travel reserved; at 10:45 `a` is inside its block
/// and `b` is past the end of its own, so `b` is assigned with the part's travel.
#[test]
fn a_heterogeneous_pool_assigns_the_first_eligible_host_with_its_own_travel() {
    let mut blocked_early = Host::free("b", "UTC");
    blocked_early.calendar = Some(vec![block((
        utc(2027, 3, 10, 9, 0),
        utc(2027, 3, 10, 10, 30),
    ))]);
    let mut blocked_late = Host::free("a", "UTC");
    blocked_late.calendar = Some(vec![block((
        utc(2027, 3, 10, 10, 30),
        utc(2027, 3, 10, 11, 0),
    ))]);
    blocked_late.travel = Some((20, 10));
    // No calendar at all: free within the rules, but flagged when assigned.
    let mut unknown = Host::free("c", "UTC");
    unknown.calendar = None;

    let scenario = Scenario {
        hosts: vec![blocked_late, blocked_early, unknown],
        event_zone: "UTC",
        window: Some(daily(RuleZone::Named(name("UTC")), (9, 0), (12, 0))),
        blackout: None,
        rules: Vec::new(),
        travel: (5, 5),
        now: utc(2027, 3, 10, 0, 0),
        search: span(utc(2027, 3, 10, 9, 0), utc(2027, 3, 10, 12, 0)),
        step: 15,
        duration: 30,
        candidates: None,
    }
    .with_kinds();

    let engine = Engine::builder().build().unwrap();
    let expected = reference(engine.time_zone_database(), &scenario);
    let actual = offers(&engine, &scenario);
    assert_eq!(
        actual, expected,
        "the engine's offers disagree with minute-by-minute membership"
    );

    let at = |hour, minute| {
        actual
            .iter()
            .find(|offer| offer.start == utc(2027, 3, 10, hour, minute))
            .unwrap_or_else(|| panic!("no offer at {hour:02}:{minute:02}"))
    };
    // Only the host with no calendar can take 09:15, so the offer is flagged,
    // and the reserve is that host's own (the part's) travel.
    let early = at(9, 15);
    assert_eq!(early.assigned, "c");
    assert_eq!(early.eligible, ["c"]);
    assert!(early.calendar_unknown);
    assert_eq!(
        early.reserve,
        [
            (
                "c".to_owned(),
                utc(2027, 3, 10, 9, 10),
                utc(2027, 3, 10, 9, 15)
            ),
            (
                "c".to_owned(),
                utc(2027, 3, 10, 9, 45),
                utc(2027, 3, 10, 9, 50)
            ),
        ]
    );
    // `a`'s own travel, not the part's, is what the offer reserves.
    let own_travel = at(9, 30);
    assert_eq!(own_travel.assigned, "a");
    assert_eq!(own_travel.eligible, ["a", "c"]);
    assert!(!own_travel.calendar_unknown);
    assert_eq!(
        own_travel.reserve,
        [
            (
                "a".to_owned(),
                utc(2027, 3, 10, 9, 10),
                utc(2027, 3, 10, 9, 30)
            ),
            (
                "a".to_owned(),
                utc(2027, 3, 10, 10, 0),
                utc(2027, 3, 10, 10, 10)
            ),
        ]
    );
    // `a` is inside its block and `b` is past its own, so `b` is assigned.
    let next_in_line = at(10, 45);
    assert_eq!(next_in_line.assigned, "b");
    assert_eq!(next_in_line.eligible, ["b", "c"]);
    assert_eq!(
        next_in_line.reserve,
        [
            (
                "b".to_owned(),
                utc(2027, 3, 10, 10, 40),
                utc(2027, 3, 10, 10, 45)
            ),
            (
                "b".to_owned(),
                utc(2027, 3, 10, 11, 15),
                utc(2027, 3, 10, 11, 20)
            ),
        ]
    );
}

/// A deterministic pseudo-random generator, so combinations no hand fixture
/// would reach are covered and a failure names the case that found it.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        self.0 >> 33
    }

    /// A value below `bound`.
    fn below(&mut self, bound: u64) -> u64 {
        self.next() % bound
    }

    /// An index below `bound`.
    fn index(&mut self, bound: usize) -> usize {
        usize::try_from(self.below(u64::try_from(bound).expect("an index bound fits in u64")))
            .expect("a remainder below the bound fits usize")
    }

    /// A count below `bound`.
    fn count(&mut self, bound: u32) -> u32 {
        u32::try_from(self.below(u64::from(bound))).expect("a remainder fits u32")
    }

    /// A whole number of minutes below `bound`.
    fn minutes(&mut self, bound: i64) -> i64 {
        i64::try_from(self.below(u64::try_from(bound).expect("a positive bound"))).expect("fits")
    }

    /// An hour below `bound`.
    fn hour(&mut self, bound: u8) -> u8 {
        u8::try_from(self.below(u64::from(bound))).expect("a remainder fits u8")
    }

    /// Whether the coin came up heads.
    fn coin(&mut self) -> bool {
        self.below(2) == 0
    }
}

const ZONES: [&str; 8] = [
    "UTC",
    "America/New_York",
    "Europe/London",
    "Australia/Lord_Howe",
    "America/Santiago",
    "Pacific/Kiritimati",
    "Etc/GMT+12",
    "Asia/Kolkata",
];

const HOST_IDS: [&str; 3] = ["a", "b", "c"];

/// Days with a transition or a date-line crossing in at least one zone, plus
/// quiet days.
const BASES: [fn() -> Instant; 7] = [
    || utc(2027, 3, 14, 0, 0),
    || utc(2027, 11, 7, 0, 0),
    || utc(2027, 9, 5, 0, 0),
    || utc(2027, 4, 4, 0, 0),
    || utc(2027, 10, 3, 0, 0),
    || utc(2027, 3, 10, 0, 0),
    || utc(2011, 12, 29, 0, 0),
];

/// One random host: a random zone, maybe a trip, maybe a calendar, maybe travel
/// of its own.
fn random_host(rng: &mut Rng, id: &'static str, base: Instant, days: i64) -> Host {
    let mut host = Host {
        id,
        zone: ZONES[rng.index(ZONES.len())],
        periods: Vec::new(),
        calendar: rng.coin().then(|| random_blocks(rng, base, days)),
        travel: rng.coin().then(|| (rng.count(46), rng.count(46))),
    };
    if rng.coin() {
        let from = Instant::from_minutes(base.minutes() + rng.minutes(days * 12 * 60)).unwrap();
        let length = 60 + rng.minutes(6 * 60);
        let until = Instant::from_minutes(from.minutes() + length).unwrap();
        host.periods
            .push((ZONES[rng.index(ZONES.len())], from, until));
    }
    host
}

/// Blocks with random travel, length and status, spread over the search.
fn random_blocks(rng: &mut Rng, base: Instant, days: i64) -> Vec<Block> {
    (0..rng.count(3))
        .map(|_| {
            let offset = rng.minutes(days * 24 * 60);
            let length = 15 + rng.minutes(120);
            let start = Instant::from_minutes(base.minutes() + offset).unwrap();
            let end = Instant::from_minutes(start.minutes() + length).unwrap();
            let mut block = block((start, end));
            block.travel_before = Duration::from_minutes(rng.count(31));
            block.travel_after = Duration::from_minutes(rng.count(31));
            match rng.index(3) {
                0 => {
                    block.status = BlockStatus::Free;
                    block.tags = vec![tag("status/focus")];
                }
                1 => block.status = BlockStatus::Tentative,
                _ => block.status = BlockStatus::Busy,
            }
            block
        })
        .collect()
}

/// A window at a random hour span, in a random zone selection.
fn random_window(rng: &mut Rng) -> WindowSpec {
    let zone = match rng.index(3) {
        0 => RuleZone::Participant,
        1 => RuleZone::Event,
        _ => RuleZone::Named(name("UTC")),
    };
    let start_hour = rng.hour(24);
    let start_minute = 15 * rng.hour(4);
    let open_hours = 2 + rng.hour(8);
    let end_hour = (u16::from(start_hour) + u16::from(open_hours)).min(23) as u8;
    // A window range whose start equals its end is rejected at construction, so
    // nudge the edge in that case.
    let (start_hour, end_hour) = if start_hour == end_hour {
        (start_hour.saturating_sub(1), end_hour)
    } else {
        (start_hour, end_hour)
    };
    daily(zone, (start_hour, start_minute), (end_hour, start_minute))
}

/// A blackout over one local date, with a random hour range.
fn random_blackout(rng: &mut Rng, day: Date) -> Option<BlackoutSpec> {
    rng.coin().then(|| {
        let from = 8 + rng.hour(6);
        blackout_in(
            RuleZone::Participant,
            vec![(local(day, from, 0), local(day, from + 1, 30))],
        )
    })
}

/// Clearance with both gaps, a status policy that ignores focus blocks, and
/// notice.
fn random_rules(rng: &mut Rng) -> Vec<Rule> {
    vec![
        Rule::new(
            "buffers",
            RuleKind::Clearance(Clearance {
                before: Duration::from_minutes(rng.count(31)),
                after: Duration::from_minutes(rng.count(31)),
                min_gap: Duration::from_minutes(rng.count(16)),
            }),
        ),
        Rule::new(
            "ignore-focus",
            RuleKind::StatusPolicy(StatusPolicy {
                entries: vec![StatusPolicyEntry::tags(
                    Selector::Tag(tag("status/focus")),
                    StatusEffect::Ignore,
                )],
            }),
        ),
        Rule::new(
            "notice",
            RuleKind::LeadTime(LeadTime {
                min: Duration::from_minutes(rng.count(121)),
                max: None,
            }),
        ),
    ]
}

/// One random scenario: one to three hosts with their own zones, trips,
/// calendars and travel; two windows to intersect, one possibly scoped by
/// `who`; two blackouts to union; both clearance gaps; and notice.
fn scenario(rng: &mut Rng) -> Scenario {
    let event_zone = if rng.coin() { "UTC" } else { ZONES[0] };
    let base = BASES[rng.index(BASES.len())]();
    let days = 1 + rng.minutes(3);
    let search = span(
        base,
        Instant::from_minutes(base.minutes() + days * 24 * 60).unwrap(),
    );
    let now =
        Instant::from_minutes((base.minutes() - rng.minutes(6 * 60)).max(Instant::MIN.minutes()))
            .unwrap();
    let blackout_day = Zone::utc()
        .zoned(base)
        .date()
        .plus_days(rng.minutes(2))
        .unwrap();

    let host_count = 1 + rng.index(HOST_IDS.len());
    let mut hosts: Vec<Host> = HOST_IDS[..host_count]
        .iter()
        .map(|id| random_host(rng, id, base, days))
        .collect();
    hosts.sort_by_key(|host| host.id);

    // The second window and blackout sometimes apply to one host alone, so
    // per-participant rule selection is exercised.
    let scoped = |rng: &mut Rng| -> Selector {
        if rng.coin() {
            Selector::Ids(vec![HOST_IDS[rng.index(host_count)].to_owned()])
        } else {
            Selector::True
        }
    };

    let mut rules = random_rules(rng);
    if rng.coin() {
        rules.push(
            Rule::new("also-hours", RuleKind::Window(random_window(rng))).with_who(scoped(rng)),
        );
    }
    if let Some(blackout) = random_blackout(rng, blackout_day) {
        rules.push(Rule::new("also-closed", RuleKind::Blackout(blackout)).with_who(scoped(rng)));
    }

    Scenario {
        hosts,
        event_zone,
        window: Some(random_window(rng)),
        blackout: random_blackout(rng, blackout_day),
        rules,
        travel: (rng.count(46), rng.count(46)),
        now,
        search,
        step: [15, 30, 60][rng.index(3)],
        duration: 15 + rng.count(76),
        candidates: None,
    }
    .with_kinds()
}

#[test]
fn the_sweep_agrees_with_minute_membership() {
    let engine = Engine::builder().build().unwrap();
    let db = engine.time_zone_database();
    let mut rng = Rng(0x5eed_1234_abcd_9876);
    for case in 0..120u32 {
        let scenario = scenario(&mut rng);
        let expected = reference(db, &scenario);
        let actual = offers(&engine, &scenario);
        assert_eq!(
            actual,
            expected,
            "case {case} ({} hosts, event zone {}, base {}): the engine's offers disagree \
             with minute-by-minute membership",
            scenario.hosts.len(),
            scenario.event_zone,
            scenario.search.start(),
        );
    }
}

/// Every local date the half-open window touches, with the number of offers on
/// it, counted a minute at a time.
fn reference_dates(zone: &Zone, window: Span, starts: &[Instant]) -> Vec<(Date, usize)> {
    let mut counts: BTreeMap<Date, usize> = BTreeMap::new();
    let mut minute = window.start().minutes();
    while minute < window.end().minutes() {
        let instant = Instant::from_minutes(minute).unwrap();
        counts.entry(zone.zoned(instant).date()).or_default();
        minute += 1;
    }
    for start in starts {
        *counts.entry(zone.zoned(*start).date()).or_default() += 1;
    }
    counts.into_iter().collect()
}

#[test]
fn dates_are_every_civil_date_the_window_touches() {
    let engine = Engine::builder().build().unwrap();
    let db = engine.time_zone_database();
    // New York across its fall-back, Kiritimati on a quiet day, and Apia across
    // the date it skipped in 2011.
    let cases: [(&str, Span); 3] = [
        (
            "America/New_York",
            span(utc(2027, 11, 6, 3, 0), utc(2027, 11, 9, 3, 0)),
        ),
        (
            "Pacific/Kiritimati",
            span(utc(2027, 3, 8, 0, 0), utc(2027, 3, 9, 0, 0)),
        ),
        (
            "Pacific/Apia",
            span(utc(2011, 12, 29, 0, 0), utc(2011, 12, 31, 0, 0)),
        ),
    ];
    for (zone, search) in cases {
        let scenario = Scenario {
            hosts: vec![Host::free("a", zone)],
            event_zone: zone,
            window: None,
            blackout: None,
            rules: Vec::new(),
            travel: (0, 0),
            now: search.start(),
            search,
            step: 60,
            duration: 30,
            candidates: None,
        };
        let response_offers = offers(&engine, &scenario);
        // The response itself, for the dates and counts.
        let part = Part::new(
            Duration::from_minutes(30),
            Requirement::new(vec![Term::person("a")]),
        );
        let mut goal = Goal::new(name(zone), search, vec![part]);
        goal.grid = Some(
            Grid::new(
                Duration::from_minutes(60),
                TimeOfDay::MIDNIGHT,
                GridZone::Event,
            )
            .unwrap(),
        );
        let mut request = Request::new(search.start(), goal);
        request.participants.push(Participant::new("a", name(zone)));
        let response = engine.find_availability(&request).unwrap();
        let starts: Vec<Instant> = response.offers.iter().filter_map(Offer::start).collect();
        assert_eq!(
            starts.len(),
            response_offers.len(),
            "{zone}: the two helpers agree on the offers"
        );

        let expected = reference_dates(&db.zone(zone).unwrap(), search, &starts);
        let actual: Vec<(Date, usize)> = response
            .dates
            .iter()
            .map(|count| (count.date, count.offers))
            .collect();
        assert_eq!(actual, expected, "{zone}: date counts disagree");
    }
}

/// The first instant whose local date in `zone` is `date` or later, found by a
/// minute scan two days either side of the date's UTC midnight.
fn start_of_date(zone: &Zone, date: Date) -> Instant {
    let LocalResolution::Unique(anchor) = Zone::utc()
        .resolve_local(date, TimeOfDay::MIDNIGHT)
        .unwrap()
    else {
        panic!("UTC has no transitions");
    };
    let from = Instant::from_minutes((anchor.minutes() - 2 * 24 * 60).max(Instant::MIN.minutes()))
        .unwrap();
    let until = Instant::from_minutes((anchor.minutes() + 2 * 24 * 60).min(Instant::MAX.minutes()))
        .unwrap();
    let mut minute = from.minutes();
    while minute < until.minutes() {
        let instant = Instant::from_minutes(minute).unwrap();
        if zone.zoned(instant).date() >= date {
            return instant;
        }
        minute += 1;
    }
    panic!("no instant reaches {date} in {zone:?}");
}

/// The earliest and latest local dates the instants of `[from, until)` fall on.
fn date_bounds(zone: &Zone, from: Instant, until: Instant) -> (Date, Date) {
    let mut first: Option<Date> = None;
    let mut last: Option<Date> = None;
    let mut minute = from.minutes();
    while minute < until.minutes() {
        let date = zone.zoned(Instant::from_minutes(minute).unwrap()).date();
        first = Some(first.map_or(date, |current| current.min(date)));
        last = Some(last.map_or(date, |current| current.max(date)));
        minute += 1;
    }
    (first.unwrap(), last.unwrap())
}

#[test]
fn required_ranges_cover_every_zone_the_participant_occupies_near_the_window() {
    let engine = Engine::builder().build().unwrap();
    let db = engine.time_zone_database();

    let search = span(utc(2027, 3, 8, 6, 0), utc(2027, 3, 9, 18, 0));
    let mut participant = Participant::new("a", name("Pacific/Kiritimati"));
    participant.zone_periods = vec![
        ZonePeriod::new(
            name("Etc/GMT+12"),
            utc(2027, 3, 8, 9, 0),
            utc(2027, 3, 8, 21, 0),
        )
        .unwrap(),
    ];
    let mut part = Part::new(
        Duration::from_minutes(90),
        Requirement::new(vec![Term::person("a")]),
    );
    part.travel = Travel {
        before: Duration::from_minutes(60),
        after: Duration::from_minutes(45),
        by_participant: BTreeMap::default(),
    };
    let mut query = RangesQuery::new(
        utc(2027, 3, 8, 6, 0),
        Goal::new(name("UTC"), search, vec![part]),
    );
    query.participants.push(participant);
    query.rules.push(Rule::new(
        "buffers",
        RuleKind::Clearance(Clearance {
            before: Duration::from_minutes(30),
            after: Duration::from_hours_minutes(2, 0),
            min_gap: Duration::from_minutes(10),
        }),
    ));
    let ranges = engine.required_ranges(&query).unwrap();

    // The padded window the request declares: travel and clearance before the
    // window, the booking's length and travel and clearance after it.
    let padded_start = Instant::from_minutes(search.start().minutes() - 60 - 30).unwrap();
    let padded_end = Instant::from_minutes(search.end().minutes() + 90 + 45 + 120).unwrap();
    for zone_name in ["Pacific/Kiritimati", "Etc/GMT+12"] {
        let zone = db.zone(zone_name).unwrap();
        let (first, last) = date_bounds(&zone, padded_start, padded_end);
        let need_start = start_of_date(&zone, first.plus_days(-1).unwrap());
        let need_end = start_of_date(&zone, last.plus_days(2).unwrap());
        assert!(
            ranges.calendar.start() <= need_start,
            "{zone_name}: the hull starts at {} but the day before {first} begins at {need_start}",
            ranges.calendar.start()
        );
        assert!(
            ranges.calendar.end() >= need_end,
            "{zone_name}: the hull ends at {} but the day after {last} begins at {need_end}",
            ranges.calendar.end()
        );
    }
}
