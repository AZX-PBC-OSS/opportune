//! Reusable end-to-end scenarios for the A1 use cases and the seven rule
//! kinds.
//!
//! Each scenario is a request, the starts it must offer, and the starts it must
//! exclude with a specific reason code. The requests are built from the domain
//! types and presets a caller would use, in one zone's local clock, so the same
//! scenario runs in every zone of the specification's matrix.
//!
//! Scenarios are fixed values: the dates and times are hand-computed, and the
//! expectations are the numbers a reader can check by hand. The oracle property
//! tests cover generated inputs; these cover the use-case shapes.

use opportune::model::{
    Block, BlockStatus, CalendarSpec, Clearance, Explain, Goal, LeadTimeMax, Participant, Request,
    Requirement, Rule, RuleKind, Span, StatusEffect, StatusPolicy, StatusPolicyEntry, Term,
};
use opportune::presets;
use opportune::reasons::ReasonCode;
use opportune::tags::{Selector, Tag};
use opportune::time::{
    BlackoutSpan, Date, DateKey, DateRange, Duration, Grid, GridZone, Instant, LocalDateTime,
    LocalResolution, RuleZone, TimeOfDay, TimeZoneDatabase, WallEnd, WallRange, Weekday, Weekdays,
    WeeklyKey, WindowSpec, WorkingDays, ZoneName,
};

use crate::generators::ZONE_MATRIX;

/// One end-to-end scenario: its request, the starts it must offer, and the
/// starts it must exclude for a specific reason.
#[derive(Clone, Debug)]
pub struct Scenario {
    /// A stable identifier for the scenario.
    pub id: &'static str,
    /// The use case or rule kind the scenario pins.
    pub use_case: &'static str,
    /// The zone the scenario runs in.
    pub zone: String,
    /// The only host the request assigns.
    pub host: String,
    /// The request to run.
    pub request: Request,
    /// Starts that must be offered.
    pub offered: Vec<Instant>,
    /// Starts that must be excluded, with the code their first reason carries.
    pub excluded: Vec<(Instant, ReasonCode)>,
}

/// Every A1 use case scenario, built for `zone`.
///
/// The scenarios are U1, U2 (its weekly hours and its closed-date override), U14,
/// U16, U18, U21 and U22.
#[must_use]
pub fn a1_scenarios(db: &TimeZoneDatabase, zone: &str) -> Vec<Scenario> {
    vec![
        u1(db, zone),
        u2_weekly_hours(db, zone),
        u2_closed_date(db, zone),
        u14(db, zone),
        u16(db, zone),
        u18(db, zone),
        u21(db, zone),
        u22(db, zone),
    ]
}

/// The rule kinds the suite carries fixtures for.
///
/// [`RulePlan::kind`](crate::a1::RulePlan::kind) is the one spelling of these
/// names; this list is the order a reader and the canonical report's header see
/// them in.
pub const RULE_KINDS: [&str; 7] = [
    "window",
    "blackout",
    "clearance",
    "lead_time",
    "cap",
    "rest",
    "status_policy",
];

/// One excluding and one including fixture for each of the seven rule kinds,
/// built for `zone`.
#[must_use]
pub fn rule_kind_scenarios(db: &TimeZoneDatabase, zone: &str) -> Vec<Scenario> {
    vec![
        window_kind(db, zone),
        blackout_kind(db, zone),
        clearance_kind(db, zone),
        lead_time_min_kind(db, zone),
        lead_time_max_kind(db, zone),
        status_policy_kind(db, zone),
        cap_kind(db, zone),
        rest_kind(db, zone),
    ]
}

/// The zones the scenario suites run in.
#[must_use]
pub fn zone_matrix() -> &'static [&'static str; 9] {
    &ZONE_MATRIX
}

/// One compact A1 scenario: a short window, an explicit list of candidate starts,
/// and the search's whole answer pinned.
///
/// A breadth [`Scenario`] spans days, so its answer runs to hundreds of starts and
/// the fixture can only name a representative few. An exact scenario names every
/// start the search judges, so its whole `offered` and `excluded` lists are
/// compared with `assert_eq!`: moving one start from one side to the other cannot
/// pass.
#[derive(Clone, Debug)]
pub struct ExactScenario {
    /// A stable identifier for the scenario.
    pub id: &'static str,
    /// The use case or rule kind the scenario pins.
    pub use_case: &'static str,
    /// The zone the scenario runs in.
    pub zone: String,
    /// The only host the request assigns.
    pub host: String,
    /// The request to run, naming its candidate starts explicitly.
    pub request: Request,
    /// Every start the search must offer, in order.
    pub offered: Vec<Instant>,
    /// Every start the search must exclude, in order, with the reason it gives.
    pub excluded: Vec<(Instant, ReasonCode)>,
}

/// The compact exact-list scenarios, built for `zone`.
///
/// They are U1, both shapes of U2, U16, and one case for each of the seven rule
/// kinds plus a second `lead_time` case for the latest-start bound.
#[must_use]
pub fn exact_scenarios(db: &TimeZoneDatabase, zone: &str) -> Vec<ExactScenario> {
    vec![
        exact_u1(db, zone),
        exact_u2_weekly_hours(db, zone),
        exact_u2_closed_date(db, zone),
        exact_u16(db, zone),
        exact_window(db, zone),
        exact_blackout(db, zone),
        exact_clearance(db, zone),
        exact_lead_time_min(db, zone),
        exact_lead_time_max(db, zone),
        exact_status_policy(db, zone),
        exact_rest(db, zone),
        exact_cap(db, zone),
    ]
}

const HOST: &str = "rich";

fn date(year: i16, month: i8, day: i8) -> Date {
    Date::new(year, month, day).expect("a fixture date is a real date")
}

fn time(hour: u8, minute: u8) -> TimeOfDay {
    TimeOfDay::new(hour, minute).expect("a fixture time is a time of day")
}

fn zone_name(zone: &str) -> ZoneName {
    ZoneName::new(zone).expect("the fixture zone is a zone name")
}

/// The instant a fixture's local date-time falls on, in `zone`.
///
/// # Panics
/// Panics when the local time is skipped or repeated by a transition, which none
/// of the fixture dates does, or when the zone is not in the database.
fn local(db: &TimeZoneDatabase, zone: &str, day: Date, hour: u8, minute: u8) -> Instant {
    let resolved = db
        .zone(zone)
        .unwrap_or_else(|err| panic!("`{zone}` is in the bundled database: {err}"))
        .resolve_local(day, time(hour, minute))
        .expect("the fixture instant is inside the supported range");
    match resolved {
        LocalResolution::Unique(instant) => instant,
        LocalResolution::Skipped | LocalResolution::Repeated(_, _) => {
            panic!(
                "{} {} in {zone} must exist exactly once",
                day,
                time(hour, minute)
            );
        }
    }
}

/// A weekdays `from`–`until` window rule in the participant's zone.
fn participant_hours(from: (u8, u8), until: (u8, u8)) -> Rule {
    let spec = WindowSpec::new(
        RuleZone::Participant,
        vec![
            WeeklyKey::new(
                Weekdays::range(Weekday::Monday, Weekday::Friday),
                vec![
                    WallRange::new(time(from.0, from.1), WallEnd::Time(time(until.0, until.1)))
                        .expect("the fixture range is non-empty"),
                ],
            )
            .expect("the fixture key is valid"),
        ],
        Vec::new(),
    )
    .expect("the fixture window has no overlapping keys");
    Rule::new("hours", RuleKind::Window(spec))
}

/// A busy block over a local date-time span in `zone`.
fn block(
    db: &TimeZoneDatabase,
    zone: &str,
    day: Date,
    from: (u8, u8),
    until: (u8, u8),
    travel_before: u32,
) -> Block {
    let mut block = Block::new(
        Span::new(
            local(db, zone, day, from.0, from.1),
            local(db, zone, day, until.0, until.1),
        )
        .expect("the fixture block is non-empty"),
    );
    block.travel_before = Duration::from_minutes(travel_before);
    block
}

/// A request with one 30-minute part for [`HOST`] in `zone`, a known-empty
/// calendar, and an explanation so exclusions are reported.
fn base(
    db: &TimeZoneDatabase,
    zone: &str,
    search: (Date, Date),
    now: (Date, u8, u8),
    duration: u32,
) -> Request {
    let window = Span::new(
        local(db, zone, search.0, 0, 0),
        local(db, zone, search.1, 0, 0),
    )
    .expect("the fixture window is non-empty");
    let mut request = Request::new(
        local(db, zone, now.0, now.1, now.2),
        Goal::single(
            zone_name(zone),
            window,
            Duration::from_minutes(duration),
            Requirement::new(vec![Term::person(HOST)]),
        ),
    );
    let mut participant = Participant::new(HOST, zone_name(zone));
    participant.calendar = Some(Vec::new());
    request.participants.push(participant);
    request.explain = Some(Explain::new(10_000));
    request
}

fn scenario(
    id: &'static str,
    use_case: &'static str,
    zone: &str,
    request: Request,
    offered: Vec<Instant>,
    excluded: Vec<(Instant, ReasonCode)>,
) -> Scenario {
    Scenario {
        id,
        use_case,
        zone: zone.to_owned(),
        host: HOST.to_owned(),
        request,
        offered,
        excluded,
    }
}

fn u1(db: &TimeZoneDatabase, zone: &str) -> Scenario {
    let mut request = base(
        db,
        zone,
        (date(2027, 6, 14), date(2027, 6, 19)),
        (date(2027, 6, 14), 8, 0),
        30,
    );
    request.rules.push(
        presets::business_hours(
            RuleZone::Participant,
            Weekdays::range(Weekday::Monday, Weekday::Friday),
            time(9, 0),
            WallEnd::Time(time(17, 0)),
        )
        .expect("the fixture hours are valid"),
    );
    request.rules.push(presets::buffers(
        Duration::from_minutes(10),
        Duration::from_minutes(10),
    ));
    request
        .rules
        .push(presets::min_notice(Duration::from_hours_minutes(4, 0)));

    let monday = date(2027, 6, 14);
    scenario(
        "u1",
        "U1",
        zone,
        request,
        vec![
            local(db, zone, monday, 12, 0),
            local(db, zone, monday, 16, 30),
        ],
        vec![
            (local(db, zone, monday, 11, 30), ReasonCode::LeadTimeMin),
            (local(db, zone, monday, 17, 0), ReasonCode::OutsideWindow),
        ],
    )
}

fn u2_weekly_hours(db: &TimeZoneDatabase, zone: &str) -> Scenario {
    let mut request = base(
        db,
        zone,
        (date(2027, 6, 14), date(2027, 6, 19)),
        (date(2027, 6, 13), 12, 0),
        30,
    );
    let spec = WindowSpec::new(
        RuleZone::Participant,
        vec![
            WeeklyKey::new(
                Weekdays::list([Weekday::Monday, Weekday::Wednesday]),
                vec![
                    WallRange::new(time(9, 0), WallEnd::Time(time(17, 0)))
                        .expect("the fixture range is valid"),
                ],
            )
            .expect("the fixture key is valid"),
            WeeklyKey::new(
                Weekdays::single(Weekday::Tuesday),
                vec![
                    WallRange::new(time(10, 0), WallEnd::Time(time(12, 0)))
                        .expect("the fixture range is valid"),
                    WallRange::new(time(14, 0), WallEnd::Time(time(18, 0)))
                        .expect("the fixture range is valid"),
                ],
            )
            .expect("the fixture key is valid"),
            WeeklyKey::new(
                Weekdays::single(Weekday::Thursday),
                vec![
                    WallRange::new(time(8, 0), WallEnd::Time(time(14, 0)))
                        .expect("the fixture range is valid"),
                ],
            )
            .expect("the fixture key is valid"),
        ],
        Vec::new(),
    )
    .expect("the fixture window has disjoint keys");
    request
        .rules
        .push(Rule::new("hours", RuleKind::Window(spec)));

    scenario(
        "u2",
        "U2",
        zone,
        request,
        vec![
            local(db, zone, date(2027, 6, 14), 9, 30),
            local(db, zone, date(2027, 6, 15), 10, 30),
            local(db, zone, date(2027, 6, 15), 15, 0),
            local(db, zone, date(2027, 6, 17), 8, 30),
        ],
        vec![
            (
                local(db, zone, date(2027, 6, 15), 13, 0),
                ReasonCode::OutsideWindow,
            ),
            (
                local(db, zone, date(2027, 6, 16), 8, 30),
                ReasonCode::OutsideWindow,
            ),
            (
                local(db, zone, date(2027, 6, 18), 10, 0),
                ReasonCode::OutsideWindow,
            ),
        ],
    )
}

fn u2_closed_date(db: &TimeZoneDatabase, zone: &str) -> Scenario {
    let mut request = base(
        db,
        zone,
        (date(2028, 12, 25), date(2028, 12, 29)),
        (date(2028, 12, 24), 12, 0),
        30,
    );
    let spec = WindowSpec::new(
        RuleZone::Participant,
        vec![
            WeeklyKey::new(
                Weekdays::range(Weekday::Monday, Weekday::Friday),
                vec![
                    WallRange::new(time(9, 0), WallEnd::Time(time(17, 0)))
                        .expect("the fixture range is valid"),
                ],
            )
            .expect("the fixture key is valid"),
        ],
        vec![
            DateKey::new(DateRange::single(date(2028, 12, 25)), Vec::new())
                .expect("an empty date key is valid"),
        ],
    )
    .expect("the fixture window has disjoint keys");
    request
        .rules
        .push(Rule::new("hours", RuleKind::Window(spec)));

    scenario(
        "u2-closed-date",
        "U2",
        zone,
        request,
        vec![
            local(db, zone, date(2028, 12, 26), 10, 30),
            local(db, zone, date(2028, 12, 27), 9, 30),
        ],
        vec![(
            local(db, zone, date(2028, 12, 25), 9, 30),
            ReasonCode::OutsideWindow,
        )],
    )
}

fn u14(db: &TimeZoneDatabase, zone: &str) -> Scenario {
    let mut request = base(
        db,
        zone,
        (date(2027, 6, 17), date(2027, 6, 24)),
        (date(2027, 6, 17), 8, 0),
        30,
    );
    request.calendars.insert(
        "org".to_owned(),
        CalendarSpec {
            zone: zone_name(zone),
            working_days: WorkingDays::weekdays(),
            holidays: Vec::new(),
        },
    );
    request
        .rules
        .push(presets::min_notice(Duration::from_hours_minutes(1, 0)));
    request
        .rules
        .push(presets::horizon(LeadTimeMax::WorkingDays {
            count: 3,
            calendar: "org".to_owned(),
        }));

    scenario(
        "u14",
        "U14",
        zone,
        request,
        vec![
            local(db, zone, date(2027, 6, 17), 9, 0),
            local(db, zone, date(2027, 6, 22), 9, 0),
            local(db, zone, date(2027, 6, 22), 23, 30),
        ],
        vec![
            (
                local(db, zone, date(2027, 6, 17), 8, 30),
                ReasonCode::LeadTimeMin,
            ),
            (
                local(db, zone, date(2027, 6, 23), 0, 0),
                ReasonCode::LeadTimeMax,
            ),
        ],
    )
}

fn u16(db: &TimeZoneDatabase, zone: &str) -> Scenario {
    let mut request = base(
        db,
        zone,
        (date(2027, 6, 14), date(2027, 6, 15)),
        (date(2027, 6, 13), 12, 0),
        30,
    );
    let event_window = WindowSpec::new(
        RuleZone::Event,
        vec![
            WeeklyKey::new(
                Weekdays::range(Weekday::Monday, Weekday::Sunday),
                vec![
                    WallRange::new(time(10, 0), WallEnd::Time(time(12, 0)))
                        .expect("the fixture range is valid"),
                    WallRange::new(time(14, 0), WallEnd::Time(time(16, 0)))
                        .expect("the fixture range is valid"),
                ],
            )
            .expect("the fixture key is valid"),
        ],
        Vec::new(),
    )
    .expect("the fixture window has no overrides");
    request
        .rules
        .push(Rule::new("event", RuleKind::Window(event_window)));
    request.rules.push(participant_hours((9, 0), (17, 0)));

    let monday = date(2027, 6, 14);
    scenario(
        "u16",
        "U16",
        zone,
        request,
        vec![
            local(db, zone, monday, 10, 30),
            local(db, zone, monday, 14, 30),
        ],
        vec![
            (local(db, zone, monday, 13, 0), ReasonCode::OutsideWindow),
            (local(db, zone, monday, 16, 30), ReasonCode::OutsideWindow),
        ],
    )
}

fn u18(db: &TimeZoneDatabase, zone: &str) -> Scenario {
    let mut request = base(
        db,
        zone,
        (date(2027, 6, 14), date(2027, 6, 15)),
        (date(2027, 6, 13), 12, 0),
        30,
    );
    request.rules.push(participant_hours((9, 0), (17, 0)));
    request.goal.grid = None;
    request.goal.candidates = Some(vec![
        local(db, zone, date(2027, 6, 14), 8, 45),
        local(db, zone, date(2027, 6, 14), 10, 15),
        local(db, zone, date(2027, 6, 14), 15, 45),
        local(db, zone, date(2027, 6, 14), 16, 50),
    ]);

    let monday = date(2027, 6, 14);
    scenario(
        "u18",
        "U18",
        zone,
        request,
        vec![
            local(db, zone, monday, 10, 15),
            local(db, zone, monday, 15, 45),
        ],
        vec![
            (local(db, zone, monday, 8, 45), ReasonCode::OutsideWindow),
            (local(db, zone, monday, 16, 50), ReasonCode::OutsideWindow),
        ],
    )
}

fn u21(db: &TimeZoneDatabase, zone: &str) -> Scenario {
    let mut request = base(
        db,
        zone,
        (date(2027, 6, 14), date(2027, 6, 15)),
        (date(2027, 6, 13), 12, 0),
        30,
    );
    request.participants[0].calendar = Some(vec![block(
        db,
        zone,
        date(2027, 6, 14),
        (14, 0),
        (15, 0),
        25,
    )]);
    request.rules.push(participant_hours((9, 0), (18, 0)));
    request.rules.push(presets::buffers(
        Duration::from_minutes(10),
        Duration::from_minutes(10),
    ));
    request.goal.parts[0].travel.before = Duration::from_minutes(20);
    request.goal.parts[0].travel.after = Duration::from_minutes(20);
    request.goal.grid = Some(
        Grid::new(
            Duration::from_minutes(10),
            TimeOfDay::MIDNIGHT,
            GridZone::Event,
        )
        .expect("the fixture grid is valid"),
    );

    let monday = date(2027, 6, 14);
    scenario(
        "u21",
        "U21",
        zone,
        request,
        vec![local(db, zone, monday, 12, 30)],
        vec![
            (local(db, zone, monday, 12, 40), ReasonCode::Clearance),
            (local(db, zone, monday, 12, 50), ReasonCode::Travel),
            (local(db, zone, monday, 13, 40), ReasonCode::Blocked),
        ],
    )
}

fn u22(db: &TimeZoneDatabase, zone: &str) -> Scenario {
    let mut request = base(
        db,
        zone,
        (date(2027, 6, 14), date(2027, 6, 15)),
        (date(2027, 6, 13), 12, 0),
        30,
    );
    request.participants[0].calendar = Some(vec![block(
        db,
        zone,
        date(2027, 6, 14),
        (15, 0),
        (15, 30),
        0,
    )]);
    request.rules.push(participant_hours((9, 0), (17, 0)));

    let monday = date(2027, 6, 14);
    scenario(
        "u22",
        "U22",
        zone,
        request,
        vec![local(db, zone, monday, 14, 0)],
        vec![(local(db, zone, monday, 15, 0), ReasonCode::Blocked)],
    )
}

fn window_kind(db: &TimeZoneDatabase, zone: &str) -> Scenario {
    let mut request = base(
        db,
        zone,
        (date(2027, 6, 14), date(2027, 6, 15)),
        (date(2027, 6, 13), 12, 0),
        30,
    );
    request.rules.push(participant_hours((9, 0), (17, 0)));
    let monday = date(2027, 6, 14);
    scenario(
        "kind-window",
        "window",
        zone,
        request,
        vec![local(db, zone, monday, 9, 30)],
        vec![(local(db, zone, monday, 8, 30), ReasonCode::OutsideWindow)],
    )
}

fn blackout_kind(db: &TimeZoneDatabase, zone: &str) -> Scenario {
    let mut request = base(
        db,
        zone,
        (date(2027, 6, 14), date(2027, 6, 15)),
        (date(2027, 6, 13), 12, 0),
        30,
    );
    request.rules.push(participant_hours((9, 0), (17, 0)));
    let monday = date(2027, 6, 14);
    request.rules.push(Rule::new(
        "freeze",
        RuleKind::Blackout(opportune::time::BlackoutSpec::new(
            RuleZone::Event,
            vec![
                BlackoutSpan::new(
                    LocalDateTime::new(monday, time(10, 0)),
                    LocalDateTime::new(monday, time(11, 0)),
                )
                .expect("the fixture span is ascending"),
            ],
        )),
    ));
    scenario(
        "kind-blackout",
        "blackout",
        zone,
        request,
        vec![local(db, zone, monday, 9, 30)],
        vec![(local(db, zone, monday, 10, 30), ReasonCode::Blackout)],
    )
}

fn clearance_kind(db: &TimeZoneDatabase, zone: &str) -> Scenario {
    let mut request = base(
        db,
        zone,
        (date(2027, 6, 14), date(2027, 6, 15)),
        (date(2027, 6, 13), 12, 0),
        30,
    );
    let monday = date(2027, 6, 14);
    request.participants[0].calendar = Some(vec![block(db, zone, monday, (10, 0), (11, 0), 0)]);
    request.rules.push(participant_hours((9, 0), (18, 0)));
    request.rules.push(Rule::new(
        "gap",
        RuleKind::Clearance(Clearance {
            before: Duration::from_minutes(15),
            after: Duration::from_minutes(15),
            min_gap: Duration::ZERO,
        }),
    ));
    scenario(
        "kind-clearance",
        "clearance",
        zone,
        request,
        vec![local(db, zone, monday, 9, 0)],
        vec![(local(db, zone, monday, 9, 30), ReasonCode::Clearance)],
    )
}

fn lead_time_min_kind(db: &TimeZoneDatabase, zone: &str) -> Scenario {
    let mut request = base(
        db,
        zone,
        (date(2027, 6, 14), date(2027, 6, 15)),
        (date(2027, 6, 14), 8, 0),
        30,
    );
    request.rules.push(participant_hours((9, 0), (17, 0)));
    request
        .rules
        .push(presets::min_notice(Duration::from_hours_minutes(2, 0)));
    let monday = date(2027, 6, 14);
    scenario(
        "kind-lead-time-min",
        "lead_time",
        zone,
        request,
        vec![local(db, zone, monday, 10, 0)],
        vec![(local(db, zone, monday, 9, 30), ReasonCode::LeadTimeMin)],
    )
}

fn lead_time_max_kind(db: &TimeZoneDatabase, zone: &str) -> Scenario {
    let mut request = base(
        db,
        zone,
        (date(2027, 6, 14), date(2027, 6, 16)),
        (date(2027, 6, 14), 0, 0),
        30,
    );
    request.rules.push(participant_hours((9, 0), (17, 0)));
    request.rules.push(presets::horizon(LeadTimeMax::Days(0)));
    scenario(
        "kind-lead-time-max",
        "lead_time",
        zone,
        request,
        vec![local(db, zone, date(2027, 6, 14), 9, 30)],
        vec![(
            local(db, zone, date(2027, 6, 15), 9, 0),
            ReasonCode::LeadTimeMax,
        )],
    )
}

fn status_policy_kind(db: &TimeZoneDatabase, zone: &str) -> Scenario {
    let mut request = base(
        db,
        zone,
        (date(2027, 6, 14), date(2027, 6, 15)),
        (date(2027, 6, 13), 12, 0),
        30,
    );
    let monday = date(2027, 6, 14);
    let mut tentative = block(db, zone, monday, (10, 0), (11, 0), 0);
    tentative.status = BlockStatus::Tentative;
    let mut focus = block(db, zone, monday, (12, 0), (13, 0), 0);
    focus.status = BlockStatus::Free;
    focus.tags = vec![Tag::new("status/focus").expect("the fixture tag is valid")];
    request.participants[0].calendar = Some(vec![tentative, focus]);
    request.rules.push(participant_hours((9, 0), (17, 0)));
    request.rules.push(Rule::new(
        "policy",
        RuleKind::StatusPolicy(StatusPolicy {
            entries: vec![
                StatusPolicyEntry::status(BlockStatus::Tentative, StatusEffect::Ignore),
                StatusPolicyEntry::tags(
                    Selector::Tag(Tag::new("status/focus").expect("the fixture tag is valid")),
                    StatusEffect::Block,
                ),
            ],
        }),
    ));
    scenario(
        "kind-status-policy",
        "status_policy",
        zone,
        request,
        vec![local(db, zone, monday, 10, 30)],
        vec![(local(db, zone, monday, 12, 30), ReasonCode::Blocked)],
    )
}

fn cap_kind(db: &TimeZoneDatabase, zone: &str) -> Scenario {
    // A day that already holds one counted interview cannot take a second, so
    // every Monday start is excluded while the same starts on Tuesday are
    // offered. The part carries the counted tag: a cap applies to a booking only
    // when the booking itself is what the cap counts.
    let monday = date(2027, 6, 14);
    let tuesday = date(2027, 6, 15);
    let mut request = base(
        db,
        zone,
        (monday, date(2027, 6, 16)),
        (date(2027, 6, 13), 12, 0),
        30,
    );
    let mut existing = block(db, zone, monday, (10, 0), (10, 30), 0);
    existing.tags = vec![Tag::new("interview").expect("the fixture tag is valid")];
    request.participants[0].calendar = Some(vec![existing]);
    request.rules.push(participant_hours((9, 0), (17, 0)));
    request.goal.parts[0].tags = vec![Tag::new("interview").expect("the fixture tag is valid")];
    request.rules.push(presets::cap_per_day(
        Selector::Tag(Tag::new("interview").expect("the fixture tag is valid")),
        1,
    ));
    scenario(
        "kind-cap",
        "cap",
        zone,
        request,
        vec![local(db, zone, tuesday, 9, 30)],
        vec![(local(db, zone, monday, 9, 30), ReasonCode::CapReached)],
    )
}

fn rest_kind(db: &TimeZoneDatabase, zone: &str) -> Scenario {
    // A Monday finish at 19:00 protects the hours before 10:00 on Tuesday, and
    // nothing else: Monday's own starts are unaffected.
    let monday = date(2027, 6, 14);
    let tuesday = date(2027, 6, 15);
    let mut request = base(
        db,
        zone,
        (monday, date(2027, 6, 16)),
        (date(2027, 6, 13), 12, 0),
        30,
    );
    request.participants[0].calendar = Some(vec![block(db, zone, monday, (18, 0), (19, 0), 0)]);
    request.rules.push(participant_hours((9, 0), (17, 0)));
    request.rules.push(presets::rest(time(18, 0), time(10, 0)));
    scenario(
        "kind-rest",
        "rest",
        zone,
        request,
        vec![
            local(db, zone, monday, 9, 30),
            local(db, zone, tuesday, 10, 0),
        ],
        vec![(local(db, zone, tuesday, 9, 30), ReasonCode::Rest)],
    )
}

/// A request over `search`, the local midnights bounding it, whose candidate
/// starts are named explicitly: the search judges exactly those starts, so its
/// whole answer is the handful of entries the fixture writes down.
fn exact_base(
    db: &TimeZoneDatabase,
    zone: &str,
    search: (Date, Date),
    now: (Date, u8, u8),
    candidates: &[Instant],
) -> Request {
    let mut request = base(db, zone, search, now, 30);
    request.goal.grid = None;
    request.goal.candidates = Some(candidates.to_vec());
    request
}

fn exact(
    id: &'static str,
    use_case: &'static str,
    zone: &str,
    request: Request,
    offered: Vec<Instant>,
    excluded: Vec<(Instant, ReasonCode)>,
) -> ExactScenario {
    ExactScenario {
        id,
        use_case,
        zone: zone.to_owned(),
        host: HOST.to_owned(),
        request,
        offered,
        excluded,
    }
}

/// The U1 link: business hours, buffers either side, and four hours' notice. The
/// booking may start as soon as the notice ends, and not a minute before, and the
/// last start the hours admit ends at their close.
fn exact_u1(db: &TimeZoneDatabase, zone: &str) -> ExactScenario {
    let day = date(2027, 6, 14);
    let eleven_thirty = local(db, zone, day, 11, 30);
    let noon = local(db, zone, day, 12, 0);
    let sixteen_thirty = local(db, zone, day, 16, 30);
    let seventeen = local(db, zone, day, 17, 0);
    let mut request = exact_base(
        db,
        zone,
        (day, date(2027, 6, 15)),
        (day, 8, 0),
        &[eleven_thirty, noon, sixteen_thirty, seventeen],
    );
    request.rules.push(
        presets::business_hours(
            RuleZone::Participant,
            Weekdays::range(Weekday::Monday, Weekday::Friday),
            time(9, 0),
            WallEnd::Time(time(17, 0)),
        )
        .expect("the fixture hours are valid"),
    );
    request.rules.push(presets::buffers(
        Duration::from_minutes(10),
        Duration::from_minutes(10),
    ));
    request
        .rules
        .push(presets::min_notice(Duration::from_hours_minutes(4, 0)));

    exact(
        "exact-u1",
        "U1",
        zone,
        request,
        vec![noon, sixteen_thirty],
        vec![
            (eleven_thirty, ReasonCode::LeadTimeMin),
            (seventeen, ReasonCode::OutsideWindow),
        ],
    )
}

/// The U2 split shift: Monday is open 9–12 and 14–17, so the lunch hour and
/// either side of the shift are closed while both halves of the day are open.
fn exact_u2_weekly_hours(db: &TimeZoneDatabase, zone: &str) -> ExactScenario {
    let day = date(2027, 6, 14);
    let eight_forty_five = local(db, zone, day, 8, 45);
    let nine_fifteen = local(db, zone, day, 9, 15);
    let thirteen = local(db, zone, day, 13, 0);
    let fifteen = local(db, zone, day, 15, 0);
    let sixteen_forty_five = local(db, zone, day, 16, 45);
    let mut request = exact_base(
        db,
        zone,
        (day, date(2027, 6, 15)),
        (date(2027, 6, 13), 12, 0),
        &[
            eight_forty_five,
            nine_fifteen,
            thirteen,
            fifteen,
            sixteen_forty_five,
        ],
    );
    let spec = WindowSpec::new(
        RuleZone::Participant,
        vec![
            WeeklyKey::new(
                Weekdays::single(Weekday::Monday),
                vec![
                    WallRange::new(time(9, 0), WallEnd::Time(time(12, 0)))
                        .expect("the fixture range is valid"),
                    WallRange::new(time(14, 0), WallEnd::Time(time(17, 0)))
                        .expect("the fixture range is valid"),
                ],
            )
            .expect("the fixture key is valid"),
        ],
        Vec::new(),
    )
    .expect("the fixture window has no overrides");
    request
        .rules
        .push(Rule::new("hours", RuleKind::Window(spec)));

    exact(
        "exact-u2-weekly-hours",
        "U2",
        zone,
        request,
        vec![nine_fifteen, fifteen],
        vec![
            (eight_forty_five, ReasonCode::OutsideWindow),
            (thirteen, ReasonCode::OutsideWindow),
            (sixteen_forty_five, ReasonCode::OutsideWindow),
        ],
    )
}

/// The U2 closed date: the weekly hours admit weekdays, and a date override closes
/// the one day it names while its neighbours open normally.
fn exact_u2_closed_date(db: &TimeZoneDatabase, zone: &str) -> ExactScenario {
    let closed = date(2028, 12, 25);
    let first_open = date(2028, 12, 26);
    let second_open = date(2028, 12, 27);
    let closed_morning = local(db, zone, closed, 9, 30);
    let closed_afternoon = local(db, zone, closed, 13, 0);
    let first_start = local(db, zone, first_open, 9, 30);
    let second_start = local(db, zone, second_open, 10, 30);
    let mut request = exact_base(
        db,
        zone,
        (closed, date(2028, 12, 29)),
        (date(2028, 12, 24), 12, 0),
        &[closed_morning, closed_afternoon, first_start, second_start],
    );
    let spec = WindowSpec::new(
        RuleZone::Participant,
        vec![
            WeeklyKey::new(
                Weekdays::range(Weekday::Monday, Weekday::Friday),
                vec![
                    WallRange::new(time(9, 0), WallEnd::Time(time(17, 0)))
                        .expect("the fixture range is valid"),
                ],
            )
            .expect("the fixture key is valid"),
        ],
        vec![
            DateKey::new(DateRange::single(closed), Vec::new())
                .expect("an empty date key is valid"),
        ],
    )
    .expect("the fixture window has disjoint keys");
    request
        .rules
        .push(Rule::new("hours", RuleKind::Window(spec)));

    exact(
        "exact-u2-closed-date",
        "U2",
        zone,
        request,
        vec![first_start, second_start],
        vec![
            (closed_morning, ReasonCode::OutsideWindow),
            (closed_afternoon, ReasonCode::OutsideWindow),
        ],
    )
}

/// The U16 interaction: the event window opens 10–12 and 14–16 while the host
/// works 9–17, so the intersection decides and both rules' shoulders are closed.
fn exact_u16(db: &TimeZoneDatabase, zone: &str) -> ExactScenario {
    let day = date(2027, 6, 14);
    let nine_thirty = local(db, zone, day, 9, 30);
    let ten_thirty = local(db, zone, day, 10, 30);
    let thirteen = local(db, zone, day, 13, 0);
    let fourteen_thirty = local(db, zone, day, 14, 30);
    let fifteen_forty_five = local(db, zone, day, 15, 45);
    let mut request = exact_base(
        db,
        zone,
        (day, date(2027, 6, 15)),
        (date(2027, 6, 13), 12, 0),
        &[
            nine_thirty,
            ten_thirty,
            thirteen,
            fourteen_thirty,
            fifteen_forty_five,
        ],
    );
    let event_window = WindowSpec::new(
        RuleZone::Event,
        vec![
            WeeklyKey::new(
                Weekdays::range(Weekday::Monday, Weekday::Sunday),
                vec![
                    WallRange::new(time(10, 0), WallEnd::Time(time(12, 0)))
                        .expect("the fixture range is valid"),
                    WallRange::new(time(14, 0), WallEnd::Time(time(16, 0)))
                        .expect("the fixture range is valid"),
                ],
            )
            .expect("the fixture key is valid"),
        ],
        Vec::new(),
    )
    .expect("the fixture window has no overrides");
    request
        .rules
        .push(Rule::new("event", RuleKind::Window(event_window)));
    request.rules.push(participant_hours((9, 0), (17, 0)));

    exact(
        "exact-u16",
        "U16",
        zone,
        request,
        vec![ten_thirty, fourteen_thirty],
        vec![
            (nine_thirty, ReasonCode::OutsideWindow),
            (thirteen, ReasonCode::OutsideWindow),
            (fifteen_forty_five, ReasonCode::OutsideWindow),
        ],
    )
}

/// A `window` rule admits a start that ends inside it. The start at the opening
/// minute is offered and the one at the closing minute is not, because its booking
/// would run past the close.
fn exact_window(db: &TimeZoneDatabase, zone: &str) -> ExactScenario {
    let day = date(2027, 6, 14);
    let eight_thirty = local(db, zone, day, 8, 30);
    let nine = local(db, zone, day, 9, 0);
    let nine_thirty = local(db, zone, day, 9, 30);
    let seventeen = local(db, zone, day, 17, 0);
    let mut request = exact_base(
        db,
        zone,
        (day, date(2027, 6, 15)),
        (date(2027, 6, 13), 12, 0),
        &[eight_thirty, nine, nine_thirty, seventeen],
    );
    request.rules.push(participant_hours((9, 0), (17, 0)));

    exact(
        "exact-window",
        "window",
        zone,
        request,
        vec![nine, nine_thirty],
        vec![
            (eight_thirty, ReasonCode::OutsideWindow),
            (seventeen, ReasonCode::OutsideWindow),
        ],
    )
}

/// A `blackout` removes the starts whose booking overlaps it, including the one at
/// its opening minute, and leaves the minute it closes at free.
fn exact_blackout(db: &TimeZoneDatabase, zone: &str) -> ExactScenario {
    let day = date(2027, 6, 14);
    let nine_thirty = local(db, zone, day, 9, 30);
    let ten = local(db, zone, day, 10, 0);
    let ten_thirty = local(db, zone, day, 10, 30);
    let eleven = local(db, zone, day, 11, 0);
    let mut request = exact_base(
        db,
        zone,
        (day, date(2027, 6, 15)),
        (date(2027, 6, 13), 12, 0),
        &[nine_thirty, ten, ten_thirty, eleven],
    );
    request.rules.push(participant_hours((9, 0), (17, 0)));
    request.rules.push(Rule::new(
        "freeze",
        RuleKind::Blackout(opportune::time::BlackoutSpec::new(
            RuleZone::Event,
            vec![
                BlackoutSpan::new(
                    LocalDateTime::new(day, time(10, 0)),
                    LocalDateTime::new(day, time(11, 0)),
                )
                .expect("the fixture span is ascending"),
            ],
        )),
    ));

    exact(
        "exact-blackout",
        "blackout",
        zone,
        request,
        vec![nine_thirty, eleven],
        vec![
            (ten, ReasonCode::Blackout),
            (ten_thirty, ReasonCode::Blackout),
        ],
    )
}

/// A `clearance` widens every existing event by its gaps, so a booking exactly one
/// gap away from a block is admitted and one minute closer is excluded.
fn exact_clearance(db: &TimeZoneDatabase, zone: &str) -> ExactScenario {
    let day = date(2027, 6, 14);
    let nine = local(db, zone, day, 9, 0);
    let nine_fifteen = local(db, zone, day, 9, 15);
    let nine_thirty = local(db, zone, day, 9, 30);
    let eleven = local(db, zone, day, 11, 0);
    let eleven_fifteen = local(db, zone, day, 11, 15);
    let mut request = exact_base(
        db,
        zone,
        (day, date(2027, 6, 15)),
        (date(2027, 6, 13), 12, 0),
        &[nine, nine_fifteen, nine_thirty, eleven, eleven_fifteen],
    );
    request.participants[0].calendar = Some(vec![block(db, zone, day, (10, 0), (11, 0), 0)]);
    request.rules.push(participant_hours((9, 0), (18, 0)));
    request.rules.push(Rule::new(
        "gap",
        RuleKind::Clearance(Clearance {
            before: Duration::from_minutes(15),
            after: Duration::from_minutes(15),
            min_gap: Duration::ZERO,
        }),
    ));

    exact(
        "exact-clearance",
        "clearance",
        zone,
        request,
        vec![nine, nine_fifteen, eleven_fifteen],
        vec![
            (nine_thirty, ReasonCode::Clearance),
            (eleven, ReasonCode::Clearance),
        ],
    )
}

/// A `lead_time` minimum admits the start exactly at the end of the notice and
/// excludes every start before it.
fn exact_lead_time_min(db: &TimeZoneDatabase, zone: &str) -> ExactScenario {
    let day = date(2027, 6, 14);
    let nine_thirty = local(db, zone, day, 9, 30);
    let ten = local(db, zone, day, 10, 0);
    let ten_thirty = local(db, zone, day, 10, 30);
    let eleven = local(db, zone, day, 11, 0);
    let mut request = exact_base(
        db,
        zone,
        (day, date(2027, 6, 15)),
        (day, 8, 0),
        &[nine_thirty, ten, ten_thirty, eleven],
    );
    request.rules.push(participant_hours((9, 0), (17, 0)));
    request
        .rules
        .push(presets::min_notice(Duration::from_hours_minutes(2, 0)));

    exact(
        "exact-lead-time-min",
        "lead_time",
        zone,
        request,
        vec![ten, ten_thirty, eleven],
        vec![(nine_thirty, ReasonCode::LeadTimeMin)],
    )
}

/// A `lead_time` maximum admits today and excludes tomorrow: the horizon is
/// `now`'s local date, so the rest of today is inside it and the next day is not.
fn exact_lead_time_max(db: &TimeZoneDatabase, zone: &str) -> ExactScenario {
    let monday = date(2027, 6, 14);
    let tuesday = date(2027, 6, 15);
    let monday_start = local(db, zone, monday, 9, 30);
    let monday_noon = local(db, zone, monday, 12, 0);
    let monday_last = local(db, zone, monday, 16, 30);
    let tuesday_start = local(db, zone, tuesday, 9, 30);
    let mut request = exact_base(
        db,
        zone,
        (monday, date(2027, 6, 16)),
        (monday, 0, 0),
        &[monday_start, monday_noon, monday_last, tuesday_start],
    );
    request.rules.push(participant_hours((9, 0), (17, 0)));
    request.rules.push(presets::horizon(LeadTimeMax::Days(0)));

    exact(
        "exact-lead-time-max",
        "lead_time",
        zone,
        request,
        vec![monday_start, monday_noon, monday_last],
        vec![(tuesday_start, ReasonCode::LeadTimeMax)],
    )
}

/// A `status_policy` decides which blocks count: the tentative one is ignored and
/// the tagged focus block is held, so the answers swap sides against the same
/// calendar.
fn exact_status_policy(db: &TimeZoneDatabase, zone: &str) -> ExactScenario {
    let day = date(2027, 6, 14);
    let nine_thirty = local(db, zone, day, 9, 30);
    let ten_thirty = local(db, zone, day, 10, 30);
    let twelve_thirty = local(db, zone, day, 12, 30);
    let thirteen_thirty = local(db, zone, day, 13, 30);
    let mut request = exact_base(
        db,
        zone,
        (day, date(2027, 6, 15)),
        (date(2027, 6, 13), 12, 0),
        &[nine_thirty, ten_thirty, twelve_thirty, thirteen_thirty],
    );
    let mut tentative = block(db, zone, day, (10, 0), (11, 0), 0);
    tentative.status = BlockStatus::Tentative;
    let mut focus = block(db, zone, day, (12, 0), (13, 0), 0);
    focus.status = BlockStatus::Free;
    focus.tags = vec![Tag::new("status/focus").expect("the fixture tag is valid")];
    request.participants[0].calendar = Some(vec![tentative, focus]);
    request.rules.push(participant_hours((9, 0), (17, 0)));
    request.rules.push(Rule::new(
        "policy",
        RuleKind::StatusPolicy(StatusPolicy {
            entries: vec![
                StatusPolicyEntry::status(BlockStatus::Tentative, StatusEffect::Ignore),
                StatusPolicyEntry::tags(
                    Selector::Tag(Tag::new("status/focus").expect("the fixture tag is valid")),
                    StatusEffect::Block,
                ),
            ],
        }),
    ));

    exact(
        "exact-status-policy",
        "status_policy",
        zone,
        request,
        vec![nine_thirty, ten_thirty, thirteen_thirty],
        vec![(twelve_thirty, ReasonCode::Blocked)],
    )
}

/// A `rest` rule protects the hours after a late finish, so the next morning is
/// closed until the rest ends while the same start on the finishing day is open.
fn exact_rest(db: &TimeZoneDatabase, zone: &str) -> ExactScenario {
    let monday = date(2027, 6, 14);
    let tuesday = date(2027, 6, 15);
    let monday_start = local(db, zone, monday, 9, 30);
    let monday_evening = local(db, zone, monday, 16, 30);
    let tuesday_early = local(db, zone, tuesday, 9, 30);
    let tuesday_open = local(db, zone, tuesday, 10, 0);
    let mut request = exact_base(
        db,
        zone,
        (monday, date(2027, 6, 16)),
        (date(2027, 6, 13), 12, 0),
        &[monday_start, monday_evening, tuesday_early, tuesday_open],
    );
    request.participants[0].calendar = Some(vec![block(db, zone, monday, (18, 0), (19, 0), 0)]);
    request.rules.push(participant_hours((9, 0), (17, 0)));
    request.rules.push(presets::rest(time(18, 0), time(10, 0)));

    exact(
        "exact-rest",
        "rest",
        zone,
        request,
        vec![monday_start, monday_evening, tuesday_open],
        vec![(tuesday_early, ReasonCode::Rest)],
    )
}

/// A `cap` measures the bookings it counts: the day that already holds one counted
/// interview takes no second, while the same starts the next day are open.
fn exact_cap(db: &TimeZoneDatabase, zone: &str) -> ExactScenario {
    let monday = date(2027, 6, 14);
    let tuesday = date(2027, 6, 15);
    let monday_early = local(db, zone, monday, 9, 30);
    let monday_late = local(db, zone, monday, 11, 0);
    let tuesday_early = local(db, zone, tuesday, 9, 30);
    let tuesday_late = local(db, zone, tuesday, 11, 0);
    let mut request = exact_base(
        db,
        zone,
        (monday, date(2027, 6, 16)),
        (date(2027, 6, 13), 12, 0),
        &[monday_early, monday_late, tuesday_early, tuesday_late],
    );
    let mut existing = block(db, zone, monday, (10, 0), (10, 30), 0);
    existing.tags = vec![Tag::new("interview").expect("the fixture tag is valid")];
    request.participants[0].calendar = Some(vec![existing]);
    request.rules.push(participant_hours((9, 0), (17, 0)));
    request.goal.parts[0].tags = vec![Tag::new("interview").expect("the fixture tag is valid")];
    request.rules.push(presets::cap_per_day(
        Selector::Tag(Tag::new("interview").expect("the fixture tag is valid")),
        1,
    ));

    exact(
        "exact-cap",
        "cap",
        zone,
        request,
        vec![tuesday_early, tuesday_late],
        vec![
            (monday_early, ReasonCode::CapReached),
            (monday_late, ReasonCode::CapReached),
        ],
    )
}
