//! Reusable end-to-end scenarios for the multi-participant use cases: U3 to U10,
//! U15, U17, U19 and U20.
//!
//! Each scenario is a request, the starts it must offer with the assignment each
//! carries, and the starts it must exclude with the reasons each reports. The
//! requests are built from the domain types and presets a caller would use, in
//! one zone's local clock, so the zone-dependent scenarios run in every zone of
//! the specification's matrix.
//!
//! Scenarios are fixed values: the dates and times are hand-computed, and the
//! expectations are the numbers a reader can check by hand. The oracle property
//! tests cover generated inputs; these cover the use-case shapes.

use opportune::model::{
    Block, Cap, CapMeasure, CapPer, CapPeriod, Explain, Goal, Part, Participant, Request,
    Requirement, Rule, RuleKind, Span, Term, ZonePeriod,
};
use opportune::presets;
use opportune::reasons::ReasonCode;
use opportune::tags::{Role, Selector, Tag};
use opportune::time::{
    Date, Duration, Instant, LocalResolution, RuleZone, TimeOfDay, TimeZoneDatabase, WallEnd,
    WallRange, Weekday, Weekdays, WeeklyKey, WindowSpec, ZoneName,
};

/// One excluded start and the reasons it must report: one per reported
/// participant in ascending ID order, then a start-level reason where one
/// applies. A `None` participant names a start-level reason.
pub type ExcludedPlan = (Instant, Vec<(Option<&'static str>, ReasonCode)>);

/// A busy span written as `((hour, minute), (hour, minute))` in local time.
type ClockSpan = ((u8, u8), (u8, u8));

/// One end-to-end multi-participant scenario: its request, the offers it must
/// return and the exclusions it must explain.
#[derive(Clone, Debug)]
pub struct MultiScenario {
    /// A stable identifier for the scenario.
    pub id: &'static str,
    /// The use case the scenario pins.
    pub use_case: &'static str,
    /// The zone the scenario's own clock is read in.
    pub zone: String,
    /// The request to run.
    pub request: Request,
    /// Every start that must be offered, with the assignment it must carry.
    pub offered: Vec<(Instant, Vec<Vec<String>>)>,
    /// Every start that must be excluded, with the reasons it must report: one
    /// per reported participant in ascending ID order, then a start-level reason
    /// where one applies.
    pub excluded: Vec<ExcludedPlan>,
    /// A well-formed assignment for the requirement, used to confirm the excluded
    /// starts.
    pub probe: Vec<Vec<String>>,
}

/// Every multi-participant use case scenario, built for `zone`.
#[must_use]
pub fn a2_scenarios(db: &TimeZoneDatabase, zone: &str) -> Vec<MultiScenario> {
    vec![
        u3(db, zone),
        u4(db, zone),
        u5(db, zone),
        u6(db, zone),
        u7(db, zone),
        u8(db, zone),
        u9(db, zone),
        u10(db, zone),
        u17(db, zone),
        u19(db),
        u20(db),
        u15(db, zone),
        u11(db, zone),
        u12(db, zone),
        u13(db, zone),
    ]
}

/// The fixture's conference day: Monday 14 June 2027.
const MONDAY: (i16, i8, i8) = (2027, 6, 14);

fn date_of(day: (i16, i8, i8)) -> Date {
    Date::new(day.0, day.1, day.2).expect("a fixture date is a real date")
}

fn zone_name(zone: &str) -> ZoneName {
    ZoneName::new(zone).expect("the fixture zone is a zone name")
}

fn tag(path: &str) -> Tag {
    Tag::new(path).expect("the fixture tag is a valid tag")
}

fn ids(ids: &[&str]) -> Vec<String> {
    ids.iter().map(|id| (*id).to_owned()).collect()
}

/// The instant a fixture's local date-time falls on, in `zone`.
///
/// # Panics
/// Panics when the local time is skipped or repeated by a transition, which none
/// of the fixture dates does, or when the zone is not in the database.
fn local(db: &TimeZoneDatabase, zone: &str, day: (i16, i8, i8), hour: u8, minute: u8) -> Instant {
    let resolved = db
        .zone(zone)
        .unwrap_or_else(|err| panic!("`{zone}` is in the bundled database: {err}"))
        .resolve_local(date_of(day), TimeOfDay::new(hour, minute).unwrap())
        .expect("the fixture instant is inside the supported range");
    match resolved {
        LocalResolution::Unique(instant) => instant,
        LocalResolution::Skipped | LocalResolution::Repeated(_, _) => panic!(
            "{} {hour}:{minute:02} in {zone} must exist exactly once",
            date_of(day)
        ),
    }
}

/// The instant `minute_of_day` minutes after local midnight, in `zone`.
fn at(db: &TimeZoneDatabase, zone: &str, day: (i16, i8, i8), minute_of_day: u32) -> Instant {
    local(
        db,
        zone,
        day,
        u8::try_from(minute_of_day / 60).expect("a time within one day"),
        u8::try_from(minute_of_day % 60).expect("a minute below 60"),
    )
}

/// A window rule open `from`–`until` every day, read in the participant's own
/// zone and applying to every host.
fn host_hours(from: (u8, u8), until: (u8, u8)) -> Rule {
    window_rule("hours", RuleZone::Participant, from, until).with_who(Selector::Role(Role::Host))
}

/// A window rule open `from`–`until` every day, read in `zone`, naming one
/// participant.
fn named_hours(
    rule: &str,
    participant: &str,
    zone: RuleZone,
    from: (u8, u8),
    until: (u8, u8),
) -> Rule {
    window_rule(rule, zone, from, until).with_who(Selector::Ids(vec![participant.to_owned()]))
}

fn window_rule(id: &str, zone: RuleZone, from: (u8, u8), until: (u8, u8)) -> Rule {
    let spec = WindowSpec::new(
        zone,
        vec![
            WeeklyKey::new(
                Weekdays::range(Weekday::Monday, Weekday::Sunday),
                vec![
                    WallRange::new(
                        TimeOfDay::new(from.0, from.1).unwrap(),
                        WallEnd::Time(TimeOfDay::new(until.0, until.1).unwrap()),
                    )
                    .unwrap(),
                ],
            )
            .unwrap(),
        ],
        Vec::new(),
    )
    .unwrap();
    Rule::new(id, RuleKind::Window(spec))
}

/// A host with the given tags and a busy block over a local span in `zone`.
fn host(
    db: &TimeZoneDatabase,
    zone: &str,
    id: &str,
    tags: &[&str],
    blocked: &[ClockSpan],
) -> Participant {
    let mut participant = Participant::new(id, zone_name(zone));
    participant.tags = tags.iter().map(|path| tag(path)).collect();
    participant.calendar = Some(
        blocked
            .iter()
            .map(|(from, until)| {
                Block::new(
                    Span::new(
                        local(db, zone, MONDAY, from.0, from.1),
                        local(db, zone, MONDAY, until.0, until.1),
                    )
                    .expect("a fixture block is non-empty"),
                )
            })
            .collect(),
    );
    participant
}

/// An attendee with no calendar, which is normal for someone outside the
/// organisation.
fn attendee(id: &str, zone: &str) -> Participant {
    let mut participant = Participant::new(id, zone_name(zone));
    participant.role = Role::Attendee;
    participant
}

/// A 30-minute part with the given requirement.
fn part(requirement: Requirement) -> Part {
    Part::new(Duration::from_minutes(30), requirement)
}

/// A request booking a 30-minute part over a local day, with an explanation so
/// every exclusion is reported and the hosts' own hours as their only window.
fn request(db: &TimeZoneDatabase, zone: &str, search: (u8, u8), part: Part) -> Request {
    let window = Span::new(
        local(db, zone, MONDAY, search.0, 0),
        local(db, zone, MONDAY, search.1, 0),
    )
    .expect("the fixture window is non-empty");
    let mut request = Request::new(
        local(db, zone, MONDAY, 0, 0),
        Goal::new(zone_name(zone), window, vec![part]),
    );
    request.rules.push(host_hours((9, 0), (18, 0)));
    request.explain = Some(Explain::new(10_000));
    request
}

fn scenario(
    id: &'static str,
    use_case: &'static str,
    zone: &str,
    request: Request,
    offered: Vec<(Instant, Vec<Vec<String>>)>,
    excluded: Vec<ExcludedPlan>,
    probe: Vec<Vec<String>>,
) -> MultiScenario {
    MultiScenario {
        id,
        use_case,
        zone: zone.to_owned(),
        request,
        offered,
        excluded,
        probe,
    }
}

fn u3(db: &TimeZoneDatabase, zone: &str) -> MultiScenario {
    // First available from a pool. The whole pool is busy from 10:00 to 10:30,
    // so that start cannot be filled at all.
    let mut request = request(
        db,
        zone,
        (9, 12),
        part(Requirement::new(vec![Term::at_least(
            1,
            Selector::Tag(tag("team/sales")),
        )])),
    );
    let blocked = [((10, 0), (10, 30))];
    request.participants = vec![
        host(db, zone, "ana", &["team/sales"], &blocked),
        host(db, zone, "bo", &["team/sales"], &blocked),
        host(db, zone, "cy", &["team/sales"], &blocked),
    ];
    scenario(
        "u3",
        "U3",
        zone,
        request,
        vec![
            (at(db, zone, MONDAY, 540), vec![ids(&["ana"])]),
            (at(db, zone, MONDAY, 570), vec![ids(&["ana"])]),
            (at(db, zone, MONDAY, 630), vec![ids(&["ana"])]),
            (at(db, zone, MONDAY, 660), vec![ids(&["ana"])]),
            (at(db, zone, MONDAY, 690), vec![ids(&["ana"])]),
        ],
        vec![(
            at(db, zone, MONDAY, 600),
            vec![
                (Some("ana"), ReasonCode::Blocked),
                (Some("bo"), ReasonCode::Blocked),
                (Some("cy"), ReasonCode::Blocked),
            ],
        )],
        vec![ids(&["ana"])],
    )
}

fn u4(db: &TimeZoneDatabase, zone: &str) -> MultiScenario {
    // Fair round robin: the caller's weights put `cy` first, `bo` second and
    // `ana` last, so the assignment follows the weights and falls back to the
    // next host while `cy` is busy.
    let mut request = request(
        db,
        zone,
        (9, 12),
        part(Requirement::new(vec![Term::at_least(
            1,
            Selector::Tag(tag("team/sales")),
        )])),
    );
    request.participants = vec![
        host(db, zone, "ana", &["team/sales"], &[((10, 0), (11, 0))]),
        host(db, zone, "bo", &["team/sales"], &[((10, 30), (11, 0))]),
        host(
            db,
            zone,
            "cy",
            &["team/sales"],
            &[((9, 0), (10, 0)), ((10, 30), (11, 0))],
        ),
    ];
    request.weights.insert("bo".to_owned(), 2);
    request.weights.insert("cy".to_owned(), 5);
    scenario(
        "u4",
        "U4",
        zone,
        request,
        vec![
            (at(db, zone, MONDAY, 540), vec![ids(&["bo"])]),
            (at(db, zone, MONDAY, 570), vec![ids(&["bo"])]),
            (at(db, zone, MONDAY, 600), vec![ids(&["cy"])]),
            (at(db, zone, MONDAY, 660), vec![ids(&["cy"])]),
            (at(db, zone, MONDAY, 690), vec![ids(&["cy"])]),
        ],
        vec![(
            at(db, zone, MONDAY, 630),
            vec![
                (Some("ana"), ReasonCode::Blocked),
                (Some("bo"), ReasonCode::Blocked),
                (Some("cy"), ReasonCode::Blocked),
            ],
        )],
        vec![ids(&["bo"])],
    )
}

fn u5(db: &TimeZoneDatabase, zone: &str) -> MultiScenario {
    // A collective meeting: every named host must be free.
    let mut request = request(
        db,
        zone,
        (9, 12),
        part(Requirement::new(vec![
            Term::person("ana"),
            Term::person("bo"),
            Term::person("cy"),
        ])),
    );
    request.participants = vec![
        host(db, zone, "ana", &[], &[((10, 0), (11, 0))]),
        host(db, zone, "bo", &[], &[]),
        host(db, zone, "cy", &[], &[]),
    ];
    let all = || vec![ids(&["ana"]), ids(&["bo"]), ids(&["cy"])];
    scenario(
        "u5",
        "U5",
        zone,
        request,
        vec![
            (at(db, zone, MONDAY, 540), all()),
            (at(db, zone, MONDAY, 570), all()),
            (at(db, zone, MONDAY, 660), all()),
            (at(db, zone, MONDAY, 690), all()),
        ],
        vec![
            (
                at(db, zone, MONDAY, 600),
                vec![(Some("ana"), ReasonCode::Blocked)],
            ),
            (
                at(db, zone, MONDAY, 630),
                vec![(Some("ana"), ReasonCode::Blocked)],
            ),
        ],
        all(),
    )
}

fn u6(db: &TimeZoneDatabase, zone: &str) -> MultiScenario {
    // One named host plus one of two alternates: the alternates are tried in ID
    // order, and a busy named host still excludes the start.
    let mut request = request(
        db,
        zone,
        (9, 11),
        part(Requirement::new(vec![
            Term::person("ana"),
            Term::at_least(1, Selector::Ids(ids(&["bo", "cy"]))),
        ])),
    );
    request.participants = vec![
        host(db, zone, "ana", &[], &[((10, 0), (10, 30))]),
        host(db, zone, "bo", &[], &[((9, 0), (10, 0))]),
        host(db, zone, "cy", &[], &[]),
    ];
    scenario(
        "u6",
        "U6",
        zone,
        request,
        vec![
            (at(db, zone, MONDAY, 540), vec![ids(&["ana"]), ids(&["cy"])]),
            (at(db, zone, MONDAY, 570), vec![ids(&["ana"]), ids(&["cy"])]),
            (at(db, zone, MONDAY, 630), vec![ids(&["ana"]), ids(&["bo"])]),
        ],
        vec![(
            at(db, zone, MONDAY, 600),
            vec![(Some("ana"), ReasonCode::Blocked)],
        )],
        vec![ids(&["ana"]), ids(&["cy"])],
    )
}

fn u7(db: &TimeZoneDatabase, zone: &str) -> MultiScenario {
    // A panel needing two of a pool. `bo` and `cy` are busy from 10:00, so only
    // `ana` is free then and the term cannot be filled.
    let mut request = request(
        db,
        zone,
        (9, 11),
        part(Requirement::new(vec![Term::at_least(
            2,
            Selector::Tag(tag("pool/backend")),
        )])),
    );
    request.participants = vec![
        host(db, zone, "ana", &["pool/backend"], &[]),
        host(db, zone, "bo", &["pool/backend"], &[((10, 0), (11, 30))]),
        host(db, zone, "cy", &["pool/backend"], &[((10, 0), (11, 30))]),
    ];
    scenario(
        "u7",
        "U7",
        zone,
        request,
        vec![
            (at(db, zone, MONDAY, 540), vec![ids(&["ana", "bo"])]),
            (at(db, zone, MONDAY, 570), vec![ids(&["ana", "bo"])]),
        ],
        vec![
            (
                at(db, zone, MONDAY, 600),
                vec![
                    (Some("bo"), ReasonCode::Blocked),
                    (Some("cy"), ReasonCode::Blocked),
                ],
            ),
            (
                at(db, zone, MONDAY, 630),
                vec![
                    (Some("bo"), ReasonCode::Blocked),
                    (Some("cy"), ReasonCode::Blocked),
                ],
            ),
        ],
        vec![ids(&["ana", "bo"])],
    )
}

fn u8(db: &TimeZoneDatabase, zone: &str) -> MultiScenario {
    // A grouped quota: a room, someone senior or junior, and someone from sales.
    let sales = || vec![ids(&["room"]), ids(&["jun"]), ids(&["sales"])];
    let mut request = request(
        db,
        zone,
        (9, 10),
        part(Requirement::new(vec![
            Term::at_least(1, Selector::Tag(tag("kind/room"))),
            Term::at_least(
                1,
                Selector::Any(vec![
                    Selector::Tag(tag("level/senior")),
                    Selector::Tag(tag("level/junior")),
                ]),
            ),
            Term::at_least(1, Selector::Tag(tag("team/sales"))),
        ])),
    );
    request.participants = vec![
        host(db, zone, "room", &["kind/room"], &[]),
        host(db, zone, "sen", &["level/senior"], &[]),
        host(db, zone, "jun", &["level/junior"], &[]),
        host(db, zone, "sales", &["team/sales"], &[((9, 30), (10, 0))]),
    ];
    scenario(
        "u8",
        "U8",
        zone,
        request,
        vec![(at(db, zone, MONDAY, 540), sales())],
        vec![(
            at(db, zone, MONDAY, 570),
            vec![(Some("sales"), ReasonCode::Blocked)],
        )],
        sales(),
    )
}

fn u9(db: &TimeZoneDatabase, zone: &str) -> MultiScenario {
    // A host plus a room: the room with the smaller ID is chosen when it is
    // free, and the other one while it is not.
    let mut request = request(
        db,
        zone,
        (9, 10),
        part(Requirement::new(vec![
            Term::person("ana"),
            Term::at_least(1, Selector::Tag(tag("kind/room"))),
        ])),
    );
    request.participants = vec![
        host(db, zone, "ana", &[], &[]),
        host(db, zone, "room1", &["kind/room"], &[((9, 30), (10, 0))]),
        host(db, zone, "room2", &["kind/room"], &[]),
    ];
    scenario(
        "u9",
        "U9",
        zone,
        request,
        vec![
            (
                at(db, zone, MONDAY, 540),
                vec![ids(&["ana"]), ids(&["room1"])],
            ),
            (
                at(db, zone, MONDAY, 570),
                vec![ids(&["ana"]), ids(&["room2"])],
            ),
        ],
        vec![],
        vec![ids(&["ana"]), ids(&["room1"])],
    )
}

fn u10(db: &TimeZoneDatabase, zone: &str) -> MultiScenario {
    // A pool the caller flattened from a directory group and tagged.
    let mut request = request(
        db,
        zone,
        (9, 10),
        part(Requirement::new(vec![Term::at_least(
            1,
            Selector::Tag(tag("group/eng")),
        )])),
    );
    request.participants = vec![
        host(db, zone, "tom", &["group/eng"], &[((9, 0), (9, 30))]),
        host(db, zone, "uma", &["group/eng"], &[]),
    ];
    scenario(
        "u10",
        "U10",
        zone,
        request,
        vec![
            (at(db, zone, MONDAY, 540), vec![ids(&["uma"])]),
            (at(db, zone, MONDAY, 570), vec![ids(&["tom"])]),
        ],
        vec![],
        vec![ids(&["tom"])],
    )
}

fn u17(db: &TimeZoneDatabase, zone: &str) -> MultiScenario {
    // A booth window in Anchorage, narrower than the team's own hours, with a
    // zone period putting the team there for the conference day.
    let event = "America/Anchorage";
    let mut team = host(db, zone, "team", &[], &[]);
    team.zone_periods = vec![
        ZonePeriod::new(
            zone_name(event),
            local(db, event, (2027, 6, 13), 0, 0),
            local(db, event, (2027, 6, 15), 0, 0),
        )
        .unwrap(),
    ];
    let window = Span::new(
        local(db, event, MONDAY, 8, 0),
        local(db, event, MONDAY, 18, 0),
    )
    .unwrap();
    let mut request = Request::new(
        local(db, event, MONDAY, 0, 0),
        Goal::new(
            zone_name(event),
            window,
            vec![part(Requirement::new(vec![Term::person("team")]))],
        ),
    );
    request.participants = vec![team];
    // The booth's own hours, for booth bookings only.
    request.rules.push(
        window_rule("booth", RuleZone::Event, (10, 0), (16, 0))
            .with_when(Selector::Tag(tag("booth")))
            .with_who(Selector::Role(Role::Host)),
    );
    request.goal.parts[0].tags = vec![tag("booth")];
    // The team's own hours, read in the participant's zone, which the period
    // makes Anchorage for the whole day.
    request.rules.push(host_hours((8, 0), (18, 0)));
    request.explain = Some(Explain::new(10_000));

    let outside = || vec![(Some("team"), ReasonCode::OutsideWindow)];
    let mut offered = Vec::new();
    for minute in 0..12 {
        offered.push((
            at(db, event, MONDAY, 600 + minute * 30),
            vec![ids(&["team"])],
        ));
    }
    scenario(
        "u17",
        "U17",
        zone,
        request,
        offered,
        vec![
            (at(db, event, MONDAY, 480), outside()),
            (at(db, event, MONDAY, 510), outside()),
            (at(db, event, MONDAY, 540), outside()),
            (at(db, event, MONDAY, 570), outside()),
            (at(db, event, MONDAY, 960), outside()),
            (at(db, event, MONDAY, 990), outside()),
            (at(db, event, MONDAY, 1020), outside()),
            (at(db, event, MONDAY, 1050), outside()),
        ],
        vec![ids(&["team"])],
    )
}

fn u19(db: &TimeZoneDatabase) -> MultiScenario {
    // An external attendee in another zone, acceptable from 08:00 to 19:00 local
    // to them. The event is read in UTC, so the expectations are the same in
    // every zone of the matrix.
    let event = "UTC";
    let mut request = Request::new(
        local(db, event, MONDAY, 0, 0),
        Goal::new(
            zone_name(event),
            Span::new(
                local(db, event, MONDAY, 0, 0),
                local(db, event, MONDAY, 6, 0),
            )
            .unwrap(),
            vec![part(Requirement::new(vec![Term::person("ana")]))],
        ),
    );
    request.participants = vec![
        host(db, event, "ana", &[], &[]),
        attendee("cand", "Asia/Kolkata"),
    ];
    request.rules.push(named_hours(
        "attendee_hours",
        "cand",
        RuleZone::Participant,
        (8, 0),
        (19, 0),
    ));
    request.explain = Some(Explain::new(10_000));

    // 08:00 in Kolkata is 02:30 UTC, so the first five half hours of the search
    // end before the attendee is available.
    let mut excluded = Vec::new();
    for minute in 0..5 {
        excluded.push((
            at(db, event, MONDAY, minute * 30),
            vec![
                (Some("cand"), ReasonCode::OutsideWindow),
                (None, ReasonCode::AttendeeUnavailable),
            ],
        ));
    }
    let mut offered = Vec::new();
    for minute in 0..7 {
        offered.push((
            at(db, event, MONDAY, 150 + minute * 30),
            vec![ids(&["ana"])],
        ));
    }
    scenario(
        "u19",
        "U19",
        "UTC",
        request,
        offered,
        excluded,
        vec![ids(&["ana"])],
    )
}

fn u20(db: &TimeZoneDatabase) -> MultiScenario {
    // Candidate-submitted availability, built with the preset: two stated spans,
    // which become a UTC window on an attendee.
    let event = "UTC";
    let spans = [
        Span::new(
            local(db, event, MONDAY, 14, 0),
            local(db, event, MONDAY, 15, 0),
        )
        .unwrap(),
        Span::new(
            local(db, event, MONDAY, 15, 30),
            local(db, event, MONDAY, 16, 30),
        )
        .unwrap(),
    ];
    let (cand, rule) =
        presets::available_only("cand", &spans).expect("the stated spans fit the ceiling");
    let mut request = Request::new(
        local(db, event, MONDAY, 0, 0),
        Goal::new(
            zone_name(event),
            Span::new(
                local(db, event, MONDAY, 13, 0),
                local(db, event, MONDAY, 17, 0),
            )
            .unwrap(),
            vec![part(Requirement::new(vec![Term::person("ana")]))],
        ),
    );
    request.participants = vec![host(db, event, "ana", &[], &[]), cand];
    request.rules.push(rule);
    request.explain = Some(Explain::new(10_000));

    let outside = || {
        vec![
            (Some("cand"), ReasonCode::OutsideWindow),
            (None, ReasonCode::AttendeeUnavailable),
        ]
    };
    scenario(
        "u20",
        "U20",
        "UTC",
        request,
        vec![
            (at(db, event, MONDAY, 840), vec![ids(&["ana"])]),
            (at(db, event, MONDAY, 870), vec![ids(&["ana"])]),
            (at(db, event, MONDAY, 930), vec![ids(&["ana"])]),
            (at(db, event, MONDAY, 960), vec![ids(&["ana"])]),
        ],
        vec![
            (at(db, event, MONDAY, 780), outside()),
            (at(db, event, MONDAY, 810), outside()),
            (at(db, event, MONDAY, 900), outside()),
            (at(db, event, MONDAY, 990), outside()),
        ],
        vec![ids(&["ana"])],
    )
}

/// The fixture day after [`MONDAY`]: Tuesday 15 June 2027.
const TUESDAY: (i16, i8, i8) = (2027, 6, 15);

fn u15(db: &TimeZoneDatabase, zone: &str) -> MultiScenario {
    // Protect evenings and mornings: a host who finished late on Monday takes no
    // Tuesday booking before 10:00. The evening meeting runs from 18:30 to
    // 20:00, so every Tuesday start before 10:00 is inside the protected
    // morning; 10:00 and later are bookable.
    let mut request = Request::new(
        local(db, zone, MONDAY, 0, 0),
        Goal::new(
            zone_name(zone),
            Span::new(
                local(db, zone, TUESDAY, 6, 0),
                local(db, zone, TUESDAY, 12, 0),
            )
            .expect("the fixture window is non-empty"),
            vec![part(Requirement::new(vec![Term::person("ana")]))],
        ),
    );
    request.participants = vec![host(db, zone, "ana", &[], &[((18, 30), (20, 0))])];
    request.rules.push(host_hours((6, 0), (18, 0)));
    request.rules.push(presets::rest(
        TimeOfDay::new(18, 0).expect("a real time"),
        TimeOfDay::new(10, 0).expect("a real time"),
    ));
    request.explain = Some(Explain::new(10_000));

    let excluded: Vec<ExcludedPlan> = (360..600)
        .step_by(30)
        .map(|minute| {
            (
                at(db, zone, TUESDAY, minute),
                vec![(Some("ana"), ReasonCode::Rest)],
            )
        })
        .collect();
    scenario(
        "u15",
        "U15",
        zone,
        request,
        vec![
            (at(db, zone, TUESDAY, 600), vec![ids(&["ana"])]),
            (at(db, zone, TUESDAY, 630), vec![ids(&["ana"])]),
            (at(db, zone, TUESDAY, 660), vec![ids(&["ana"])]),
            (at(db, zone, TUESDAY, 690), vec![ids(&["ana"])]),
        ],
        excluded,
        vec![ids(&["ana"])],
    )
}

/// A busy block over a local span in `zone`, tagged with `tags`.
fn tagged_block(
    db: &TimeZoneDatabase,
    zone: &str,
    day: (i16, i8, i8),
    from: (u8, u8),
    until: (u8, u8),
    tags: &[&str],
) -> Block {
    let mut block = Block::new(
        Span::new(
            local(db, zone, day, from.0, from.1),
            local(db, zone, day, until.0, until.1),
        )
        .expect("a fixture block is non-empty"),
    );
    block.tags = tags.iter().map(|path| tag(path)).collect();
    block
}

/// The local instants `minutes` after midnight on `day`, in `zone`.
fn candidates_at(
    db: &TimeZoneDatabase,
    zone: &str,
    day: (i16, i8, i8),
    minutes: &[u32],
) -> Vec<Instant> {
    minutes
        .iter()
        .map(|minute| at(db, zone, day, *minute))
        .collect()
}

/// A `cap` rule over one counted tag.
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

fn u11(db: &TimeZoneDatabase, zone: &str) -> MultiScenario {
    // Interview load limits: pooled 8 a week and 2 a day each. Three
    // interviewers already hold two interviews each on the Monday morning, so
    // their daily cap is full; the pool's weekly usage is six of eight, which
    // still allows more. The fourth interviewer is free at 09:00 and busy from
    // 10:00, so the first two starts are offered to them and the rest explain
    // themselves.
    let mut request = Request::new(
        local(db, zone, MONDAY, 0, 0),
        Goal::new(
            zone_name(zone),
            Span::new(
                local(db, zone, MONDAY, 9, 0),
                local(db, zone, MONDAY, 12, 0),
            )
            .expect("the fixture window is non-empty"),
            vec![part(Requirement::new(vec![Term::at_least(
                1,
                Selector::Tag(tag("pool/interviewers")),
            )]))],
        ),
    );
    request.goal.candidates = Some(candidates_at(
        db,
        zone,
        MONDAY,
        &[540, 570, 600, 630, 660, 690],
    ));
    request.goal.parts[0].tags = vec![tag("interview")];
    request.rules.push(host_hours((9, 0), (18, 0)));
    request.rules.push(cap_rule(
        "each_day",
        "interview",
        CapMeasure::Count,
        2,
        CapPeriod::Day,
        CapPer::Each,
        RuleZone::Participant,
    ));
    request.rules.push(cap_rule(
        "pool_week",
        "interview",
        CapMeasure::Count,
        8,
        CapPeriod::Week,
        CapPer::Pooled,
        RuleZone::Event,
    ));
    request.explain = Some(Explain::new(10_000));
    for id in ["ana", "bo", "cy"] {
        let mut interviewer = Participant::new(id, zone_name(zone));
        interviewer.tags = vec![tag("pool/interviewers")];
        interviewer.calendar = Some(vec![
            tagged_block(db, zone, MONDAY, (6, 0), (6, 30), &["interview"]),
            tagged_block(db, zone, MONDAY, (6, 30), (7, 0), &["interview"]),
        ]);
        request.participants.push(interviewer);
    }
    let mut free = Participant::new("dee", zone_name(zone));
    free.tags = vec![tag("pool/interviewers")];
    free.calendar = Some(vec![tagged_block(db, zone, MONDAY, (10, 0), (12, 0), &[])]);
    request.participants.push(free);

    let capped = |id: &'static str| vec![(Some(id), ReasonCode::CapReached)];
    let mut excluded = Vec::new();
    for minute in [600, 630, 660, 690] {
        let mut reasons = capped("ana");
        reasons.extend(capped("bo"));
        reasons.extend(capped("cy"));
        reasons.push((Some("dee"), ReasonCode::Blocked));
        excluded.push((at(db, zone, MONDAY, minute), reasons));
    }
    scenario(
        "u11",
        "U11",
        zone,
        request,
        vec![
            (at(db, zone, MONDAY, 540), vec![ids(&["dee"])]),
            (at(db, zone, MONDAY, 570), vec![ids(&["dee"])]),
        ],
        excluded,
        vec![ids(&["ana"])],
    )
}

fn u12(db: &TimeZoneDatabase, zone: &str) -> MultiScenario {
    // Interview family limits: two a day on `interview` and one a day on
    // `interview/front-end`. The front-end interview the interviewer already has
    // on Monday fills the narrower cap, so every Monday start is excluded; on
    // Tuesday the one interview they hold is within both caps, so every Tuesday
    // start is offered. A front-end tag matches its parent, so Monday's existing
    // event counts toward both.
    let mut request = Request::new(
        local(db, zone, MONDAY, 0, 0),
        Goal::new(
            zone_name(zone),
            Span::new(
                local(db, zone, MONDAY, 9, 0),
                local(db, zone, TUESDAY, 12, 0),
            )
            .expect("the fixture window is non-empty"),
            vec![part(Requirement::new(vec![Term::person("ana")]))],
        ),
    );
    let mut candidates = candidates_at(db, zone, MONDAY, &[540, 570, 600, 630, 660, 690]);
    candidates.extend(candidates_at(
        db,
        zone,
        TUESDAY,
        &[540, 570, 600, 630, 660, 690],
    ));
    request.goal.candidates = Some(candidates);
    request.goal.parts[0].tags = vec![tag("interview/front-end")];
    request.rules.push(host_hours((9, 0), (18, 0)));
    request.rules.push(cap_rule(
        "interview_day",
        "interview",
        CapMeasure::Count,
        2,
        CapPeriod::Day,
        CapPer::Each,
        RuleZone::Participant,
    ));
    request.rules.push(cap_rule(
        "frontend_day",
        "interview/front-end",
        CapMeasure::Count,
        1,
        CapPeriod::Day,
        CapPer::Each,
        RuleZone::Participant,
    ));
    request.explain = Some(Explain::new(10_000));
    let mut interviewer = Participant::new("ana", zone_name(zone));
    interviewer.calendar = Some(vec![
        tagged_block(db, zone, MONDAY, (7, 0), (7, 30), &["interview/front-end"]),
        tagged_block(db, zone, TUESDAY, (7, 0), (7, 30), &["interview"]),
    ]);
    request.participants.push(interviewer);

    let excluded: Vec<ExcludedPlan> = [540u32, 570, 600, 630, 660, 690]
        .into_iter()
        .map(|minute| {
            (
                at(db, zone, MONDAY, minute),
                vec![(Some("ana"), ReasonCode::CapReached)],
            )
        })
        .collect();
    let offered: Vec<(Instant, Vec<Vec<String>>)> = [540u32, 570, 600, 630, 660, 690]
        .into_iter()
        .map(|minute| (at(db, zone, TUESDAY, minute), vec![ids(&["ana"])]))
        .collect();
    scenario(
        "u12",
        "U12",
        zone,
        request,
        offered,
        excluded,
        vec![ids(&["ana"])],
    )
}

fn u13(db: &TimeZoneDatabase, zone: &str) -> MultiScenario {
    // A booking time limit: no more than six hours of `sales/demo` per person a
    // week. `ana` holds five hours on Monday, so Tuesday's hour takes her to
    // exactly the limit and is allowed; `bo` holds five and a half, so their hour
    // would pass it. `ana` is busy from 10:00, which is where the starts nobody
    // can take are excluded.
    let search_minutes = [540u32, 570, 600, 630];
    let mut request = Request::new(
        local(db, zone, MONDAY, 0, 0),
        Goal::new(
            zone_name(zone),
            Span::new(
                local(db, zone, TUESDAY, 9, 0),
                local(db, zone, TUESDAY, 11, 30),
            )
            .expect("the fixture window is non-empty"),
            vec![Part::new(
                Duration::from_minutes(60),
                Requirement::new(vec![Term::at_least(1, Selector::Tag(tag("team/sales")))]),
            )],
        ),
    );
    request.goal.candidates = Some(candidates_at(db, zone, TUESDAY, &search_minutes));
    request.goal.parts[0].tags = vec![tag("sales/demo")];
    request.rules.push(host_hours((9, 0), (18, 0)));
    request.rules.push(cap_rule(
        "sales_week",
        "sales/demo",
        CapMeasure::Minutes,
        360,
        CapPeriod::Week,
        CapPer::Each,
        RuleZone::Participant,
    ));
    request.explain = Some(Explain::new(10_000));

    let mut ana = Participant::new("ana", zone_name(zone));
    ana.tags = vec![tag("team/sales")];
    ana.calendar = Some(vec![
        // Five hours of demos, tagged as the cap counts.
        tagged_block(db, zone, MONDAY, (6, 0), (11, 0), &["sales/demo"]),
        // A personal block that makes the middle starts ana's own exclusion.
        tagged_block(db, zone, TUESDAY, (10, 0), (10, 30), &[]),
    ]);
    let mut bo = Participant::new("bo", zone_name(zone));
    bo.tags = vec![tag("team/sales")];
    bo.calendar = Some(vec![tagged_block(
        db,
        zone,
        MONDAY,
        (6, 0),
        (11, 30),
        &["sales/demo"],
    )]);
    request.participants = vec![ana, bo];

    scenario(
        "u13",
        "U13",
        zone,
        request,
        vec![
            (at(db, zone, TUESDAY, 540), vec![ids(&["ana"])]),
            (at(db, zone, TUESDAY, 630), vec![ids(&["ana"])]),
        ],
        vec![
            (
                at(db, zone, TUESDAY, 570),
                vec![
                    (Some("ana"), ReasonCode::Blocked),
                    (Some("bo"), ReasonCode::CapReached),
                ],
            ),
            (
                at(db, zone, TUESDAY, 600),
                vec![
                    (Some("ana"), ReasonCode::Blocked),
                    (Some("bo"), ReasonCode::CapReached),
                ],
            ),
        ],
        vec![ids(&["ana"])],
    )
}
