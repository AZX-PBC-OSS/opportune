//! End-to-end A1 time-correctness fixtures: the hand-computed cases from the
//! plan's time section, run through the engine.
//!
//! Leap years, daylight saving's skipped and repeated hours, 23- and 25-hour
//! local days, midnight crossings, grids that do not divide a day, zone-period
//! switches, a date-line trip's required ranges, two database versions answering
//! from their own data, and the rounding and supported-range edges.

use std::sync::OnceLock;

use opportune::Engine;
use opportune::model::{
    Block, Clearance, Goal, LeadTime, Part, Participant, RangesQuery, Request, Requirement,
    Response, Rule, RuleKind, Span, StatusPolicy, StatusPolicyEntry, Term, ZonePeriod,
};
use opportune::reasons::ReasonCode;
use opportune::time::{
    Date, DateKey, DateRange, Duration, Grid, GridZone, Instant, LocalDateTime, LocalResolution,
    RuleZone, TimeOfDay, TimeZoneDatabase, WallEnd, WallRange, Weekday, Weekdays, WeeklyKey,
    WindowSpec, ZoneName,
};

fn database() -> &'static TimeZoneDatabase {
    static DATABASE: OnceLock<TimeZoneDatabase> = OnceLock::new();
    DATABASE.get_or_init(|| TimeZoneDatabase::bundled().expect("the bundled database is present"))
}

fn engine() -> &'static Engine {
    static ENGINE: OnceLock<Engine> = OnceLock::new();
    ENGINE.get_or_init(|| {
        Engine::builder()
            .build()
            .expect("the bundled database builds")
    })
}

fn name(zone: &str) -> ZoneName {
    ZoneName::new(zone).expect("the fixture zone is a zone name")
}

fn date(year: i16, month: i8, day: i8) -> Date {
    Date::new(year, month, day).expect("a fixture date is a real date")
}

fn time(hour: u8, minute: u8) -> TimeOfDay {
    TimeOfDay::new(hour, minute).expect("a fixture time is a time of day")
}

fn utc(year: i16, month: i8, day: i8, hour: u8, minute: u8) -> Instant {
    let resolved = opportune::time::Zone::utc()
        .resolve_local(date(year, month, day), time(hour, minute))
        .expect("UTC has no transitions");
    match resolved {
        LocalResolution::Unique(instant) => instant,
        LocalResolution::Skipped | LocalResolution::Repeated(_, _) => {
            panic!("UTC has no transitions")
        }
    }
}

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

fn span(start: Instant, end: Instant) -> Span {
    Span::new(start, end).expect("the fixture span is non-empty")
}

fn rule(id: &str, kind: RuleKind) -> Rule {
    Rule::new(id, kind)
}

fn window_rule(id: &str, zone: RuleZone, ranges: Vec<WallRange>, dates: Vec<DateKey>) -> Rule {
    let weekly = if ranges.is_empty() {
        Vec::new()
    } else {
        vec![
            WeeklyKey::new(Weekdays::range(Weekday::Monday, Weekday::Sunday), ranges)
                .expect("the fixture key is valid"),
        ]
    };
    rule(
        id,
        RuleKind::Window(
            WindowSpec::new(zone, weekly, dates).expect("the fixture window is valid"),
        ),
    )
}

fn wall_range(start: (u8, u8), end: WallEnd) -> WallRange {
    WallRange::new(time(start.0, start.1), end).expect("the fixture range is valid")
}

/// A request with one host in `zone`, a known-empty calendar and no rules.
fn base_request(zone: &str, search: (Instant, Instant), now: Instant, duration: u32) -> Request {
    let mut request = Request::new(
        now,
        Goal::single(
            name(zone),
            span(search.0, search.1),
            Duration::from_minutes(duration),
            Requirement::new(vec![Term::person("rich")]),
        ),
    );
    let mut participant = Participant::new("rich", name(zone));
    participant.calendar = Some(Vec::new());
    request.participants.push(participant);
    request.explain = Some(opportune::model::Explain::new(10_000));
    request
}

fn offered(response: &Response) -> Vec<Instant> {
    response
        .offers
        .iter()
        .map(|offer| offer.start().expect("an offer has an event"))
        .collect()
}

fn reason_at(response: &Response, start: Instant) -> Option<ReasonCode> {
    response
        .excluded
        .iter()
        .find(|entry| entry.start == start)
        .and_then(|entry| entry.reasons.first())
        .map(|reason| reason.code)
}

fn grid(request: &mut Request, step: u32, anchor: u16) {
    request.goal.grid = Some(
        Grid::new(
            Duration::from_minutes(step),
            TimeOfDay::from_minute_of_day(anchor).expect("the anchor is below 1440"),
            GridZone::Event,
        )
        .expect("the fixture grid is valid"),
    );
}

#[test]
fn a_leap_day_exists_only_in_a_leap_year() {
    assert!(Date::new(2027, 2, 29).is_err(), "2027 is not a leap year");
    let leap_day = date(2028, 2, 29);
    let mut request = base_request(
        "UTC",
        (utc(2028, 2, 28, 0, 0), utc(2028, 3, 1, 0, 0)),
        utc(2028, 2, 27, 0, 0),
        30,
    );
    let response = engine().find_availability(&request).unwrap();
    assert!(offered(&response).contains(&utc(2028, 2, 29, 0, 0)));
    let leap = response
        .dates
        .iter()
        .find(|count| count.date == leap_day)
        .expect("29 February is one of the window's civil dates");
    assert_eq!(leap.offers, 48, "a 30-minute grid over the leap day");

    // An override that opens only the leap day leaves exactly its half-hour
    // starts; with no weekly key the other dates are closed.
    request.rules = vec![window_rule(
        "leap-day",
        RuleZone::Event,
        Vec::new(),
        vec![
            DateKey::new(
                DateRange::single(leap_day),
                vec![wall_range((10, 0), WallEnd::Time(time(12, 0)))],
            )
            .expect("the fixture date key is valid"),
        ],
    )];
    let response = engine().find_availability(&request).unwrap();
    assert_eq!(
        offered(&response),
        [
            utc(2028, 2, 29, 10, 0),
            utc(2028, 2, 29, 10, 30),
            utc(2028, 2, 29, 11, 0),
            utc(2028, 2, 29, 11, 30),
        ]
    );
}

#[test]
fn daylight_saving_skips_and_repeats_local_hours() {
    let la = "America/Los_Angeles";
    // Fall back: 01:00-02:00 happens twice on 2027-11-07, so a 01:30-03:30 window
    // is two stretches of UTC with a gap between them.
    let mut request = base_request(
        la,
        (
            local(database(), la, date(2027, 11, 7), 0, 0),
            local(database(), la, date(2027, 11, 8), 0, 0),
        ),
        local(database(), la, date(2027, 11, 6), 12, 0),
        30,
    );
    request.rules = vec![window_rule(
        "repeated",
        RuleZone::Participant,
        vec![wall_range((1, 30), WallEnd::Time(time(3, 30)))],
        Vec::new(),
    )];
    let response = engine().find_availability(&request).unwrap();
    assert_eq!(
        offered(&response),
        [
            utc(2027, 11, 7, 8, 30),
            utc(2027, 11, 7, 9, 30),
            utc(2027, 11, 7, 10, 0),
            utc(2027, 11, 7, 10, 30),
            utc(2027, 11, 7, 11, 0),
        ],
        "the first 01:30 is inside and the second 01:00-01:30 is not"
    );
    let first = response.offers[0].start().unwrap();
    let second = response.offers[1].start().unwrap();
    assert_eq!(
        first.zoned(&database().zone(la).unwrap()).time(),
        time(1, 30)
    );
    assert_eq!(
        second.zoned(&database().zone(la).unwrap()).time(),
        time(1, 30)
    );
    assert_eq!(second.minutes() - first.minutes(), 60);

    // Spring forward: 02:00-03:00 does not exist on 2027-03-14, so the same
    // window is one continuous UTC stretch and no start falls at 02:00.
    let mut request = base_request(
        la,
        (
            local(database(), la, date(2027, 3, 14), 0, 0),
            local(database(), la, date(2027, 3, 15), 0, 0),
        ),
        local(database(), la, date(2027, 3, 13), 12, 0),
        30,
    );
    request.rules = vec![window_rule(
        "skipped",
        RuleZone::Participant,
        vec![wall_range((1, 30), WallEnd::Time(time(3, 30)))],
        Vec::new(),
    )];
    let response = engine().find_availability(&request).unwrap();
    assert_eq!(
        offered(&response),
        [utc(2027, 3, 14, 9, 30), utc(2027, 3, 14, 10, 0)]
    );
    let la_zone = database().zone(la).unwrap();
    assert!(
        offered(&response)
            .iter()
            .all(|start| start.zoned(&la_zone).time() != time(2, 0)
                && start.zoned(&la_zone).time() != time(2, 30)),
        "no start falls in the skipped hour"
    );
}

#[test]
fn an_all_day_block_is_a_local_midnight_to_local_midnight_day() {
    let la = "America/Los_Angeles";
    let la_zone = database().zone(la).unwrap();
    for (label, day, next, length) in [
        (
            "spring forward",
            date(2027, 3, 14),
            date(2027, 3, 15),
            23 * 60,
        ),
        ("fall back", date(2027, 11, 7), date(2027, 11, 8), 25 * 60),
    ] {
        let start = local(database(), la, day, 0, 0);
        let end = local(database(), la, next, 0, 0);
        assert_eq!(
            end.minutes() - start.minutes(),
            length,
            "{label}: the local day's length"
        );
        let mut request = base_request(
            la,
            (
                Instant::from_minutes(start.minutes() - 24 * 60).unwrap(),
                Instant::from_minutes(end.minutes() + 24 * 60).unwrap(),
            ),
            Instant::from_minutes(start.minutes() - 48 * 60).unwrap(),
            60,
        );
        grid(&mut request, 60, 0);
        let mut block = Block::new(span(start, end));
        block.all_day = true;
        request.participants[0].calendar = Some(vec![block]);

        let response = engine().find_availability(&request).unwrap();
        assert!(
            !offered(&response).is_empty(),
            "{label}: days either side are free"
        );
        assert!(
            offered(&response)
                .iter()
                .all(|instant| instant.zoned(&la_zone).date() != day),
            "{label}: no start falls on the all-day block's date"
        );
        assert_eq!(
            reason_at(&response, start),
            Some(ReasonCode::Blocked),
            "{label}: the block's first minute is blocked"
        );
    }
}

#[test]
fn travel_across_a_transition_is_occupied_time() {
    let la = "America/Los_Angeles";
    let mut request = base_request(
        la,
        (
            local(database(), la, date(2027, 3, 14), 0, 0),
            local(database(), la, date(2027, 3, 15), 0, 0),
        ),
        local(database(), la, date(2027, 3, 13), 12, 0),
        60,
    );
    grid(&mut request, 30, 0);
    request.rules = vec![window_rule(
        "daytime",
        RuleZone::Participant,
        vec![wall_range((0, 0), WallEnd::Time(time(6, 0)))],
        Vec::new(),
    )];
    request.goal.parts[0].travel.before = Duration::from_minutes(60);
    request.goal.parts[0].travel.after = Duration::from_minutes(60);

    let response = engine().find_availability(&request).unwrap();
    let start = utc(2027, 3, 14, 10, 0);
    let offer = response
        .offers
        .iter()
        .find(|offer| offer.start() == Some(start))
        .expect("03:00 local is offered");
    let reserve = &offer.events[0].reserve;
    assert_eq!(
        reserve.len(),
        2,
        "travel before and after are both reserved"
    );
    assert_eq!(reserve[0].participant, "rich");
    assert_eq!(reserve[0].kind, opportune::model::ReserveKind::Travel);
    assert_eq!(reserve[0].span, span(utc(2027, 3, 14, 9, 0), start));
    assert_eq!(reserve[1].participant, "rich");
    assert_eq!(reserve[1].kind, opportune::model::ReserveKind::Travel);
    assert_eq!(
        reserve[1].span,
        span(utc(2027, 3, 14, 11, 0), utc(2027, 3, 14, 12, 0)),
        "travel before spans the skipped hour and travel after is 04:00-05:00 local"
    );
    assert_eq!(
        reason_at(&response, utc(2027, 3, 14, 8, 0)),
        Some(ReasonCode::Travel),
        "travel before local midnight falls outside the window"
    );
}

#[test]
fn a_midnight_crossing_range_spills_into_the_next_day() {
    let la = "America/Los_Angeles";
    let mut request = base_request(
        la,
        (
            local(database(), la, date(2027, 6, 17), 12, 0),
            local(database(), la, date(2027, 6, 19), 12, 0),
        ),
        local(database(), la, date(2027, 6, 16), 12, 0),
        30,
    );
    grid(&mut request, 15, 0);
    let range = wall_range((22, 0), WallEnd::Time(time(2, 17)));
    request.rules = vec![window_rule(
        "nights",
        RuleZone::Participant,
        vec![range],
        vec![
            DateKey::new(DateRange::single(date(2027, 6, 18)), Vec::new())
                .expect("an empty date key is valid"),
        ],
    )];

    let response = engine().find_availability(&request).unwrap();
    let friday = date(2027, 6, 18);
    assert!(
        offered(&response).contains(&local(database(), la, date(2027, 6, 17), 22, 30)),
        "the crossing range opens the evening"
    );
    assert!(
        offered(&response).contains(&local(database(), la, friday, 1, 45)),
        "the spill ends at 02:17, not at a grid boundary, and 01:45-02:15 is inside"
    );
    assert_eq!(
        reason_at(&response, local(database(), la, friday, 2, 0)),
        Some(ReasonCode::OutsideWindow),
        "02:00-02:30 runs past the spill's 02:17 end"
    );
    assert_eq!(
        reason_at(&response, local(database(), la, friday, 22, 30)),
        Some(ReasonCode::OutsideWindow),
        "the override closes its own evening"
    );
    assert_eq!(
        reason_at(&response, local(database(), la, date(2027, 6, 19), 1, 0)),
        Some(ReasonCode::OutsideWindow),
        "the closed date spills nothing into the next morning"
    );
}

#[test]
fn grids_that_do_not_divide_a_day_come_from_the_local_clock() {
    let cases = [(25, 0, 58), (7, 0, 206), (25, 7, 58)];
    for (step, anchor, expected) in cases {
        let mut request = base_request(
            "UTC",
            (utc(2027, 6, 14, 0, 0), utc(2027, 6, 15, 0, 0)),
            utc(2027, 6, 13, 0, 0),
            30,
        );
        grid(&mut request, step, anchor);
        request.rules = vec![window_rule(
            "always",
            RuleZone::Event,
            vec![wall_range((0, 0), WallEnd::Midnight)],
            Vec::new(),
        )];
        let response = engine().find_availability(&request).unwrap();
        assert_eq!(
            response.offers.len(),
            expected,
            "a {step}-minute grid anchored at minute {anchor}"
        );
        assert_eq!(
            response.offers[0]
                .start()
                .unwrap()
                .zoned(&database().zone("UTC").unwrap())
                .time(),
            TimeOfDay::from_minute_of_day(anchor).unwrap(),
            "the first start sits on the anchor"
        );
    }
}

#[test]
fn a_zone_period_switch_reads_each_zones_own_clock() {
    let la = "America/Los_Angeles";
    let tokyo = "Asia/Tokyo";
    let switch = utc(2027, 6, 15, 20, 0);
    let back = utc(2027, 6, 16, 20, 0);
    let mut request = base_request(
        la,
        (
            local(database(), la, date(2027, 6, 15), 0, 0),
            local(database(), la, date(2027, 6, 17), 0, 0),
        ),
        local(database(), la, date(2027, 6, 14), 12, 0),
        30,
    );
    request.participants[0].zone_periods =
        vec![ZonePeriod::new(name(tokyo), switch, back).expect("the leg is non-empty")];
    request.rules = vec![window_rule(
        "hours",
        RuleZone::Participant,
        vec![wall_range((9, 0), WallEnd::Time(time(17, 0)))],
        Vec::new(),
    )];
    request.goal.candidates = Some(vec![
        Instant::from_minutes(switch.minutes() - 30).unwrap(),
        switch,
        utc(2027, 6, 16, 1, 0),
        back,
    ]);
    request.goal.grid = None;

    let response = engine().find_availability(&request).unwrap();
    assert!(
        offered(&response).contains(&Instant::from_minutes(switch.minutes() - 30).unwrap()),
        "12:30 in Los Angeles is inside the window; offers are {:?}, exclusions are {:?}",
        offered(&response),
        response.excluded
    );
    assert_eq!(
        reason_at(&response, switch),
        Some(ReasonCode::OutsideWindow),
        "the switch instant is 05:00 in Tokyo"
    );
    assert!(
        offered(&response).contains(&utc(2027, 6, 16, 1, 0)),
        "10:00 in Tokyo is inside the window"
    );
    assert!(
        offered(&response).contains(&back),
        "13:00 back in Los Angeles is inside the window"
    );
}

#[test]
fn only_clearance_widens_required_ranges() {
    let part = Part::new(
        Duration::from_minutes(30),
        Requirement::new(vec![Term::person("rich")]),
    );
    let goal = Goal::new(
        name("UTC"),
        span(utc(2027, 6, 14, 0, 10), utc(2027, 6, 14, 2, 0)),
        vec![part],
    );
    let query = |rules: Vec<Rule>| {
        let mut query = RangesQuery::new(utc(2027, 6, 13, 0, 0), goal.clone());
        query.rules = rules;
        query
            .participants
            .push(Participant::new("rich", name("UTC")));
        query
    };
    let bare = engine().required_ranges(&query(Vec::new())).unwrap();
    // The padded window starts inside 14 June, so its local dates run from 14
    // June and the hull begins the day before, on 13 June.
    assert_eq!(bare.calendar.start(), utc(2027, 6, 13, 0, 0));

    // A 20-minute clearance before the booking pushes the padded start back
    // across local midnight, so the hull starts a day earlier. This is
    // clearance's only contribution to the required ranges.
    let clearance = query(vec![Rule::new(
        "gap",
        RuleKind::Clearance(Clearance {
            before: Duration::from_minutes(20),
            after: Duration::ZERO,
            min_gap: Duration::ZERO,
        }),
    )]);
    let padded = engine().required_ranges(&clearance).unwrap();
    assert_eq!(padded.calendar.start(), utc(2027, 6, 12, 0, 0));

    // The other four kinds count nothing toward the range.
    let others = [
        window_rule(
            "hours",
            RuleZone::Event,
            vec![wall_range((9, 0), WallEnd::Time(time(17, 0)))],
            Vec::new(),
        ),
        rule(
            "freeze",
            RuleKind::Blackout(opportune::time::BlackoutSpec::new(
                RuleZone::Event,
                vec![
                    opportune::time::BlackoutSpan::new(
                        LocalDateTime::new(date(2027, 6, 14), time(10, 0)),
                        LocalDateTime::new(date(2027, 6, 14), time(11, 0)),
                    )
                    .expect("the fixture span is ascending"),
                ],
            )),
        ),
        rule(
            "notice",
            RuleKind::LeadTime(LeadTime {
                min: Duration::from_hours_minutes(2, 0),
                max: None,
            }),
        ),
        rule(
            "policy",
            RuleKind::StatusPolicy(StatusPolicy {
                entries: vec![StatusPolicyEntry::status(
                    opportune::model::BlockStatus::Tentative,
                    opportune::model::StatusEffect::Ignore,
                )],
            }),
        ),
    ];
    for other in others {
        assert_eq!(
            engine()
                .required_ranges(&query(vec![other]))
                .unwrap()
                .calendar,
            bare.calendar,
            "only clearance changes the range"
        );
    }
}

#[test]
fn required_ranges_cover_a_trip_across_the_date_line() {
    let search = span(utc(2027, 3, 8, 0, 0), utc(2027, 3, 9, 0, 0));
    let mut participant = Participant::new("rich", name("Pacific/Kiritimati"));
    participant.zone_periods = vec![
        ZonePeriod::new(
            name("Etc/GMT+12"),
            utc(2027, 3, 8, 6, 0),
            utc(2027, 3, 8, 18, 0),
        )
        .expect("the leg is non-empty"),
    ];
    let part = Part::new(
        Duration::from_minutes(30),
        Requirement::new(vec![Term::person("rich")]),
    );
    let mut query = RangesQuery::new(
        utc(2027, 3, 8, 0, 0),
        Goal::new(name("UTC"), search, vec![part]),
    );
    query.participants.push(participant);
    let ranges = engine().required_ranges(&query).unwrap();
    assert!(ranges.occurrences.is_none(), "no cap applies");
    // The earlier of the two hulls wins: midnight on 7 March in Kiritimati,
    // which is 10:00Z on 6 March, beats midnight on 6 March in the -12 leg.
    assert_eq!(ranges.calendar.start(), utc(2027, 3, 6, 10, 0));
    // The later of the two hulls wins: midnight on 10 March in the -12 leg,
    // which is 12:00Z, beats midnight on 11 March in Kiritimati at 10:00Z.
    assert_eq!(ranges.calendar.end(), utc(2027, 3, 10, 12, 0));
}

#[cfg(not(target_family = "wasm"))]
#[test]
fn two_database_versions_answer_from_their_own_data() {
    let fixtures =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../opportune/tests/fixtures");
    let before = Engine::builder()
        .tzdb_dir(fixtures.join("tzdb-2018"), None)
        .build()
        .expect("the 2018f fixture database loads");
    let after = Engine::builder()
        .tzdb_dir(fixtures.join("tzdb-2019"), None)
        .build()
        .expect("the 2019b fixture database loads");
    assert_eq!(before.time_zone_database().version(), "2018f");
    assert_eq!(after.time_zone_database().version(), "2019b");

    let candidates: Vec<Instant> = (8..=20).map(|hour| utc(2019, 12, 15, hour, 0)).collect();
    let build = || {
        let mut request = base_request(
            "America/Sao_Paulo",
            (utc(2019, 12, 15, 0, 0), utc(2019, 12, 16, 0, 0)),
            utc(2019, 12, 14, 0, 0),
            60,
        );
        request.rules = vec![window_rule(
            "hours",
            RuleZone::Participant,
            vec![wall_range((9, 0), WallEnd::Time(time(17, 0)))],
            Vec::new(),
        )];
        request.goal.grid = None;
        request.goal.candidates = Some(candidates.clone());
        request
    };

    // Brazil still observed daylight saving in the 2018f data (UTC-2) and had
    // abolished it by 2019b (UTC-3), so the same local window starts an hour
    // earlier in UTC against the older data.
    let before_response = before.find_availability(&build()).unwrap();
    let after_response = after.find_availability(&build()).unwrap();
    assert_eq!(before_response.meta.tzdb, "2018f");
    assert_eq!(after_response.meta.tzdb, "2019b");
    assert_eq!(
        offered(&before_response),
        (11..=18)
            .map(|hour| utc(2019, 12, 15, hour, 0))
            .collect::<Vec<_>>()
    );
    assert_eq!(
        offered(&after_response),
        (12..=19)
            .map(|hour| utc(2019, 12, 15, hour, 0))
            .collect::<Vec<_>>()
    );
}

#[test]
fn rounding_and_the_supported_range_are_enforced_at_the_edges() {
    // `now` rounds up to the next minute; block starts round down and ends up.
    let unaligned = Instant::MIN.unix_seconds() + 90;
    assert_eq!(
        Instant::from_unix_seconds(unaligned).unwrap_err().code(),
        "instant_not_aligned"
    );
    assert_eq!(
        Instant::ceil_to_minute(unaligned).unwrap(),
        Instant::from_minutes(Instant::MIN.minutes() + 2).unwrap()
    );
    assert_eq!(
        Instant::floor_to_minute(unaligned).unwrap(),
        Instant::from_minutes(Instant::MIN.minutes() + 1).unwrap()
    );

    // Instants beyond 1975-2100 are rejected where they are constructed.
    assert_eq!(
        Instant::from_minutes(Instant::MIN.minutes() - 1)
            .unwrap_err()
            .code(),
        "instant_out_of_range"
    );
    assert_eq!(
        Instant::from_minutes(Instant::MAX.minutes() + 1)
            .unwrap_err()
            .code(),
        "instant_out_of_range"
    );

    // A window at either edge of the supported range still offers its starts.
    let mut start_edge = base_request(
        "UTC",
        (
            Instant::MIN,
            Instant::from_minutes(Instant::MIN.minutes() + 60).unwrap(),
        ),
        Instant::MIN,
        30,
    );
    start_edge.rules.clear();
    assert_eq!(
        offered(&engine().find_availability(&start_edge).unwrap()).len(),
        2
    );

    let last = Instant::MAX;
    let mut end_edge = base_request(
        "UTC",
        (Instant::from_minutes(last.minutes() - 60).unwrap(), last),
        Instant::from_minutes(last.minutes() - 120).unwrap(),
        30,
    );
    end_edge.rules.clear();
    assert_eq!(
        offered(&engine().find_availability(&end_edge).unwrap()).len(),
        2
    );

    // A proposal that ends at the last supported instant is judged on the minutes
    // that exist. No rule applies here, so nothing excludes it: the end of the
    // supported range is not a rule the caller broke.
    let mut proposal = base_request(
        "UTC",
        (
            Instant::from_minutes(Instant::MAX.minutes() - 60).unwrap(),
            Instant::MAX,
        ),
        Instant::from_minutes(Instant::MAX.minutes() - 120).unwrap(),
        30,
    );
    proposal.participants[0].calendar = Some(Vec::new());
    let last = Instant::from_minutes(Instant::MAX.minutes() - 30).unwrap();
    let slot = engine()
        .check_slot(
            &proposal,
            &opportune::model::Proposal {
                events: vec![opportune::model::ProposalEvent {
                    part: "main".to_owned(),
                    start: last,
                    assignment: vec![vec!["rich".to_owned()]],
                }],
            },
        )
        .unwrap();
    assert!(slot.valid, "{:?}", slot.reasons);
    assert!(slot.reasons.is_empty());
}
