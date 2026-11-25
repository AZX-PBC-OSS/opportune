//! Performance benchmark workloads: fifty participants over fourteen days on a
//! five-minute grid.
//!
//! Latency targets at this scale on one modern core are under 5 ms for a
//! single-term requirement and under 25 ms for a requirement whose pools
//! overlap. Both requests here are that scale and carry what a real calendar
//! set carries — working hours read in each participant's own clock, three
//! travelers reading theirs in the destination's, existing meetings with event
//! keys, tentative holds, leave, a company holiday, journeys either side of a
//! client visit, late finishes a rest rule turns into excluded mornings,
//! clearance, notice and horizon, a cap on each participant and one on the pool,
//! and the caller's own occurrences.
//!
//! The pattern is arithmetic on the host and day indices rather than random, so
//! every run evaluates byte-identical input and two runs can be compared. The
//! production caller supplies this data by fetching `required_ranges`; here it is
//! already in the request, which is what makes the measurement the engine's cost
//! rather than a fetch's.

use opportune::Engine;
use opportune::model::{
    Block, BlockStatus, Cap, CapMeasure, CapPer, CapPeriod, Explain, Goal, LeadTimeMax, Occurrence,
    Participant, Proposal, Request, Requirement, Rule, RuleKind, Span, StatusEffect, StatusPolicy,
    StatusPolicyEntry, Term, Travel, ZonePeriod,
};
use opportune::presets;
use opportune::tags::{Selector, Tag};
use opportune::time::{
    BlackoutSpan, BlackoutSpec, Date, Duration, Grid, GridZone, Instant, LocalDateTime,
    LocalResolution, RuleZone, TimeOfDay, TimeZoneDatabase, WallEnd, Weekday, Weekdays, ZoneName,
};

/// The participants each workload names (50).
pub const PARTICIPANTS: usize = 50;

/// The search window's length in days.
pub const WINDOW_DAYS: i64 = 14;

/// The grid's step in minutes.
pub const GRID_STEP_MINUTES: u32 = 5;

/// One benchmark workload: its name and the request to evaluate.
pub struct Workload {
    /// A stable identifier, used as the benchmark's name.
    pub id: &'static str,
    /// The request to evaluate against the real engine.
    pub request: Request,
}

/// The two standard benchmark workloads, built against `engine`'s time zone database.
///
/// The first names one term the whole pool fills; the second names three terms
/// whose pools overlap, so the matching graph has several ways to fill every
/// start.
#[must_use]
pub fn spec_workloads(engine: &Engine) -> [Workload; 2] {
    let db = engine.time_zone_database();
    [
        Workload {
            id: "single_term",
            request: request(
                db,
                Requirement::new(vec![Term::at_least(1, Selector::Tag(tag("pool/hosts")))]),
                Variant::default(),
            ),
        },
        Workload {
            id: "overlapping_groups",
            request: request(
                db,
                Requirement::new(vec![
                    Term::at_least(2, Selector::Tag(tag("pool/backend"))),
                    Term::at_least(1, Selector::Tag(tag("level/senior"))),
                    Term::at_least(1, Selector::Tag(tag("team/sales"))),
                ]),
                Variant::default(),
            ),
        },
    ]
}

/// One confirm-time workload: the request, and the proposal a caller checks
/// before committing.
pub struct SlotWorkload {
    /// A stable identifier, used as the benchmark's name.
    pub id: &'static str,
    /// The request the proposal belongs to.
    pub request: Request,
    /// The booking a caller is about to confirm.
    pub proposal: Proposal,
}

/// The confirm-time workload: the first offer a search over the single-term
/// request reports, proposed back for confirmation.
///
/// A confirmation runs against the same fifty calendars and rule set as the
/// search, but judges one booking rather than every start of the window, so it
/// is the cost of a caller re-checking a slot.
///
/// # Panics
/// Panics when the single-term workload reports no offer, which its own tests
/// rule out.
#[must_use]
pub fn slot_workload(engine: &Engine) -> SlotWorkload {
    let workload = &spec_workloads(engine)[0];
    let response = engine
        .find_availability(&workload.request)
        .expect("the single-term workload is valid");
    let proposal = response
        .offers
        .first()
        .expect("the single-term workload offers starts")
        .to_proposal();
    SlotWorkload {
        id: "check_slot",
        request: workload.request.clone(),
        proposal,
    }
}

/// The bounded-limit workloads: the base benchmark calendars made dense, once with
/// the default offer limit and once with a limit that truncates almost every
/// offer the search could report.
///
/// Both carry the same fifty participants and the same rules as
/// [`spec_workloads`]; the dense calendars add eight half-hour focus blocks to
/// each working day, which takes a host from about thirty blocks over the window
/// to about a hundred, and the truncated variant asks for twenty-five offers
/// instead of a thousand. Together they measure that the block scan, the cap
/// counting and the post-limit feasibility check stay proportional to the input
/// rather than to the product of starts and blocks.
#[must_use]
pub fn limit_workloads(engine: &Engine) -> [Workload; 2] {
    let db = engine.time_zone_database();
    let requirement =
        || Requirement::new(vec![Term::at_least(1, Selector::Tag(tag("pool/hosts")))]);
    let dense = Variant {
        extra_blocks_per_day: DENSE_EXTRA_BLOCKS_PER_DAY,
        limit: None,
    };
    [
        Workload {
            id: "dense_blocks",
            request: request(db, requirement(), dense),
        },
        Workload {
            id: "dense_blocks_truncated",
            request: request(
                db,
                requirement(),
                Variant {
                    limit: Some(TRUNCATED_LIMIT),
                    ..dense
                },
            ),
        },
    ]
}

/// The zone-period counts the `adversarial` benchmark group measures.
///
/// The largest is the engine's default `max_zone_periods_per_participant`, so the
/// group spans the most fragmented schedule a valid request can carry. There is no
/// latency target for this group: the readings show how the self-conflict
/// construction scales with the schedule, and no gate asserts a wall-clock bound.
pub const REST_ZONE_PERIOD_SIZES: [usize; 6] = [32, 64, 128, 256, 512, 1_000];

/// The zone the rest-scaling participant is normally in: no daylight saving, so
/// its local reading is plain arithmetic on the home clock.
const REST_SCALING_HOME: &str = "UTC";

/// The far side of the date line from [`REST_SCALING_HOME`]: UTC+14, thirteen to
/// fourteen hours ahead of the home clock.
const REST_SCALING_EAST: &str = "Pacific/Kiritimati";

/// The near side of the date line from [`REST_SCALING_HOME`]: UTC-12, twelve hours
/// behind the home clock.
const REST_SCALING_WEST: &str = "Etc/GMT+12";

/// Where a day's evening legs begin, in home-clock minutes: 05:30, which is 19:30
/// in [`REST_SCALING_EAST`]'s reading of the day and 17:30 in
/// [`REST_SCALING_WEST`]'s reading of the day before it.
const REST_SCALING_EVENING_START: u16 = 5 * 60 + 30;

/// How many minutes of a day's home-clock morning carry evening legs: three and
/// a half hours, keeping both leg zones' readings inside their local evening.
const REST_SCALING_EVENING_MINUTES: usize = 210;

/// Where a day's morning legs begin, in home-clock minutes: 12:00, which is
/// 00:00 in [`REST_SCALING_WEST`]'s reading of the day and 02:00 in
/// [`REST_SCALING_EAST`]'s reading of the day after it.
const REST_SCALING_MORNING_START: u16 = 12 * 60;

/// How many minutes of a day's home-clock afternoon carry morning legs: eight
/// hours, keeping both leg zones' readings inside their local morning.
const REST_SCALING_MORNING_MINUTES: usize = 8 * 60;

/// How many consecutive days carry each kind of leg, either side of the day the
/// search starts on.
const REST_SCALING_DAYS: i64 = 3;

/// The `rest` self-conflict workload at `zone_periods` zone periods: one
/// participant whose clock shuttles across the date line, so the rule's late and
/// early minute sets are cut into many segments per local date.
///
/// The participant is in UTC and takes one-minute legs that alternate between
/// `Pacific/Kiritimati` (UTC+14) and `Etc/GMT+12` (UTC-12). Half the legs sit in
/// the home clock's 05:30-09:00, where UTC+14 reads the same date's evening and
/// UTC-12 reads the previous date's; half sit in its 12:00-20:00, where UTC-12
/// reads the same date's morning and UTC+14 reads the date after's. Because the
/// legs alternate zones, consecutive minutes read as different local dates, so
/// `rest` pairs many late segments with many early segments per date pair rather
/// than a handful.
///
/// Nothing else varies with `zone_periods`: one participant, one `rest` rule, one
/// candidate start and a fixed five-hour booking, so the measurement is the cost
/// of reading that schedule and not of searching.
///
/// # Panics
/// Panics when a leg zone is missing from the database or a leg's instants leave
/// the supported range, which no size in [`REST_ZONE_PERIOD_SIZES`] does.
#[must_use]
pub fn rest_zone_period_workload(engine: &Engine, zone_periods: usize) -> Request {
    let db = engine.time_zone_database();
    let day = first_day();
    let home = zone_name(REST_SCALING_HOME);
    let mut participant = Participant::new("rich", home.clone());
    participant.zone_periods = rest_zone_periods(db, day, zone_periods);

    let start = local(db, REST_SCALING_HOME, day, 9 * 60);
    let search = Span::new(start, local(db, REST_SCALING_HOME, day, 11 * 60))
        .expect("the rest-scaling search window is non-empty");
    let mut goal = Goal::single(
        home,
        search,
        Duration::from_minutes(5 * 60),
        Requirement::new(vec![Term::person("rich")]),
    );
    goal.candidates = Some(vec![start]);
    let mut request = Request::new(local(db, REST_SCALING_HOME, day, 0), goal);
    request.participants.push(participant);
    request.rules = vec![presets::rest(time(17 * 60 + 30), time(10 * 60))];
    request.explain = Some(Explain::new(1_000));
    request
}

/// The alternating legs of [`rest_zone_period_workload`], in any order: the
/// evening legs first, then the morning ones.
fn rest_zone_periods(db: &TimeZoneDatabase, day: Date, zone_periods: usize) -> Vec<ZonePeriod> {
    let zones = [zone_name(REST_SCALING_EAST), zone_name(REST_SCALING_WEST)];
    // Half the legs read as an evening, on `day - 1`, `day` and `day + 1`; the
    // other half read as a morning, on `day`, `day + 1` and `day + 2`.
    let ours = [zone_periods.div_ceil(2), zone_periods / 2];
    let windows = [
        (
            REST_SCALING_EVENING_START,
            REST_SCALING_EVENING_MINUTES,
            -1_i64,
        ),
        (
            REST_SCALING_MORNING_START,
            REST_SCALING_MORNING_MINUTES,
            0_i64,
        ),
    ];
    let mut periods = Vec::with_capacity(zone_periods);
    for ((first_minute, minutes, day_offset), mut wanted) in windows.into_iter().zip(ours) {
        for index in 0..REST_SCALING_DAYS {
            let date = day
                .plus_days(day_offset + index)
                .expect("a rest-scaling leg day is a date");
            for slot in 0..minutes {
                if wanted == 0 {
                    break;
                }
                let minute = first_minute + u16::try_from(slot).expect("a window fits a day");
                periods.push(
                    ZonePeriod::new(
                        zones[slot % zones.len()].clone(),
                        local(db, REST_SCALING_HOME, date, minute),
                        local(db, REST_SCALING_HOME, date, minute + 1),
                    )
                    .expect("a one-minute leg is non-empty"),
                );
                wanted -= 1;
            }
        }
    }
    periods
}

/// How the bounded-limit workloads differ from the base benchmark shape.
#[derive(Clone, Copy, Default)]
struct Variant {
    /// Focus time each host's working day carries besides the base benchmark
    /// calendar, in half-hour blocks.
    extra_blocks_per_day: usize,
    /// The number of offers the request asks for, or `None` for the default.
    limit: Option<usize>,
}

/// The offers the truncated workload asks for: far fewer than the window holds,
/// so most starts are judged for feasibility and then dropped.
pub const TRUNCATED_LIMIT: usize = 25;

/// The extra half-hour blocks each host's day carries in the dense workload.
pub const DENSE_EXTRA_BLOCKS_PER_DAY: usize = FOCUS_BLOCK_STARTS_MINUTES.len();

/// Where a dense calendar's focus blocks start, in local minutes of the day:
/// half-hour meetings across the working day.
const FOCUS_BLOCK_STARTS_MINUTES: [u16; 8] = [
    9 * 60,
    9 * 60 + 45,
    10 * 60 + 30,
    11 * 60 + 15,
    13 * 60,
    13 * 60 + 45,
    15 * 60 + 30,
    16 * 60 + 15,
];

/// The event zone: rules that say `zone: event`, the grid and the displayed
/// dates are read in it.
const EVENT_ZONE: &str = "Europe/London";

/// The zone of the host group each ten participants belong to, so a standup
/// shared by five calendars happens at one instant rather than five.
const HOST_ZONES: [&str; 5] = [
    "Europe/London",
    "America/New_York",
    "Europe/Berlin",
    "America/Los_Angeles",
    "Asia/Kolkata",
];

/// The window's first day: a Monday, so both weeks hold five working days.
const FIRST_DAY: (i16, i8, i8) = (2027, 6, 14);

/// The day of the window the company takes off: the second Monday, a week into
/// the window. Nobody's calendar holds anything that day.
const HOLIDAY_DAY: i64 = 7;

/// A host's ID, zero-padded so ascending ID order matches ascending index.
fn host_id(host: usize) -> String {
    format!("host-{host:02}")
}

fn tag(path: &str) -> Tag {
    Tag::new(path).expect("a workload tag is a valid tag path")
}

fn zone_name(name: &str) -> ZoneName {
    ZoneName::new(name).expect("a workload zone is in the bundled database")
}

fn time(minute_of_day: u16) -> TimeOfDay {
    TimeOfDay::from_minute_of_day(minute_of_day).expect("a minute below 1440 is a time of day")
}

fn first_day() -> Date {
    Date::new(FIRST_DAY.0, FIRST_DAY.1, FIRST_DAY.2).expect("the workload's first day is a date")
}

/// The instant `minute_of_day` of `date` falls on in `zone`.
///
/// # Panics
/// Panics when the local time is skipped or repeated by a transition, which none
/// of the workload's clocks does, or when `zone` is not in the database.
fn local(db: &TimeZoneDatabase, zone: &str, date: Date, minute_of_day: u16) -> Instant {
    let resolved = db
        .zone(zone)
        .unwrap_or_else(|err| panic!("`{zone}` is in the bundled database: {err}"))
        .resolve_local(date, time(minute_of_day))
        .expect("a workload instant is inside the supported range");
    match resolved {
        LocalResolution::Unique(instant) => instant,
        LocalResolution::Skipped | LocalResolution::Repeated(_, _) => panic!(
            "{} on {} in {zone} must exist exactly once",
            time(minute_of_day),
            date
        ),
    }
}

/// The half-open span between two local times of `date` in `zone`.
fn span(db: &TimeZoneDatabase, zone: &str, date: Date, from: u16, until: u16) -> Span {
    Span::new(local(db, zone, date, from), local(db, zone, date, until))
        .expect("a workload span covers at least one minute")
}

/// The zone `host` is in on `day`: their own, except for the travellers, who
/// spend the middle of the window in Tokyo.
fn day_zone(host: usize, day: i64) -> &'static str {
    if host % 17 == 7 && (3..6).contains(&day) {
        "Asia/Tokyo"
    } else {
        HOST_ZONES[host / 10]
    }
}

/// What `host` is tagged with. The pools overlap: the senior half reaches into
/// the backend pool, the sales team sits inside both, and every host is in the
/// single term's pool.
fn host_tags(host: usize) -> Vec<Tag> {
    let mut tags = vec![tag("pool/hosts")];
    if host < 30 {
        tags.push(tag("pool/backend"));
    }
    if (20..50).contains(&host) {
        tags.push(tag("level/senior"));
    } else {
        tags.push(tag("level/junior"));
    }
    if (25..45).contains(&host) {
        tags.push(tag("team/sales"));
    }
    tags
}

/// The request the workloads run: the specification's scale, the same rules and
/// the same calendars, differing in the requirement and in how much the
/// bounded-limit variants add.
fn request(db: &TimeZoneDatabase, requirement: Requirement, variant: Variant) -> Request {
    let first = first_day();
    let event_zone = zone_name(EVENT_ZONE);
    let window = Span::new(
        local(db, EVENT_ZONE, first, 0),
        local(
            db,
            EVENT_ZONE,
            first
                .plus_days(WINDOW_DAYS)
                .expect("the window's last day is a date"),
            0,
        ),
    )
    .expect("the workload window is non-empty");

    let mut goal = Goal::single(event_zone, window, Duration::from_minutes(30), requirement);
    goal.grid = Some(
        Grid::new(
            Duration::from_minutes(GRID_STEP_MINUTES),
            TimeOfDay::MIDNIGHT,
            GridZone::Event,
        )
        .expect("a five-minute step is inside the grid's range"),
    );

    let mut request = Request::new(local(db, EVENT_ZONE, first, 8 * 60), goal);
    let part = &mut request.goal.parts[0];
    part.tags = vec![tag("interview")];
    // An in-person interview: ten minutes to set the room up before it and ten
    // to clear it after.
    part.travel = Travel {
        before: Duration::from_minutes(10),
        after: Duration::from_minutes(10),
        ..Travel::default()
    };
    request.rules = rules(first);
    request.explain = Some(Explain::new(10_000));
    request.limit = variant.limit;
    for host in 0..PARTICIPANTS {
        request
            .participants
            .push(participant(db, host, variant.extra_blocks_per_day));
        request.weights.insert(
            host_id(host),
            i32::try_from(host % 7).expect("a weight below seven"),
        );
    }
    request.occurrences = occurrences(db, first);
    request
}

/// The rules a real org-wide interview schedule carries: one of every kind.
fn rules(first: Date) -> Vec<Rule> {
    let weekdays = Weekdays::range(Weekday::Monday, Weekday::Friday);
    let mut rules = vec![
        // Working hours in each person's own clock.
        presets::business_hours(
            RuleZone::Participant,
            weekdays,
            time(9 * 60),
            WallEnd::Time(time(17 * 60)),
        )
        .expect("the working hours cover the day"),
        // Ten minutes either side of a meeting.
        presets::buffers(Duration::from_minutes(10), Duration::from_minutes(10)),
        // Two hours' notice, and bookings up to three weeks ahead.
        presets::min_notice(Duration::from_minutes(2 * 60)),
        presets::horizon(LeadTimeMax::Days(21)),
        // A tentatively held slot is free until it is confirmed.
        Rule::new(
            "holds_are_free",
            RuleKind::StatusPolicy(StatusPolicy {
                entries: vec![StatusPolicyEntry::status(
                    BlockStatus::Tentative,
                    StatusEffect::Ignore,
                )],
            }),
        ),
        // Nothing before ten after a finish past half past five.
        presets::rest(time(17 * 60 + 30), time(10 * 60)),
        // Three interviews a day each, and a hundred and twenty across the pool:
        // the pool runs a hiring session on Mondays, which fills the day's quota
        // and leaves no further interview to book.
        presets::cap_per_day(Selector::Tag(tag("interview")), 3),
        Rule::new(
            "pool_day",
            RuleKind::Cap(Cap::new(
                Selector::Tag(tag("interview")),
                CapMeasure::Count,
                120,
                CapPeriod::Day,
                CapPer::Pooled,
                RuleZone::Event,
                Weekday::Monday,
            )),
        ),
    ];
    // The company's summer day off, in the event's own clock.
    let holiday = first
        .plus_days(HOLIDAY_DAY)
        .expect("the holiday is inside the window");
    rules.push(Rule::new(
        "summer_day",
        RuleKind::Blackout(BlackoutSpec::new(
            RuleZone::Event,
            vec![
                BlackoutSpan::new(
                    LocalDateTime::new(holiday, TimeOfDay::MIDNIGHT),
                    LocalDateTime::new(
                        holiday
                            .plus_days(1)
                            .expect("the day after the holiday is a date"),
                        TimeOfDay::MIDNIGHT,
                    ),
                )
                .expect("the holiday covers a day"),
            ],
        )),
    ));
    rules
}

/// One host: their tags, their calendar, and the trip that moves their clock for
/// the middle of the window.
fn participant(db: &TimeZoneDatabase, host: usize, extra_blocks_per_day: usize) -> Participant {
    let mut participant = Participant::new(host_id(host), zone_name(HOST_ZONES[host / 10]));
    participant.tags = host_tags(host);
    if host % 17 == 7 {
        let from = first_day().plus_days(3).expect("the trip starts on a date");
        let until = first_day().plus_days(6).expect("the trip ends on a date");
        participant.zone_periods = vec![
            ZonePeriod::new(
                zone_name("Asia/Tokyo"),
                local(db, EVENT_ZONE, from, 0),
                local(db, EVENT_ZONE, until, 0),
            )
            .expect("the trip is a non-empty period"),
        ];
    }
    participant.calendar = Some(calendar(db, host, extra_blocks_per_day));
    participant
}

/// One host's fortnight: standups, interviews, holds, a client visit, an evening
/// finish and a day of leave, plus `extra_blocks_per_day` focus blocks of their
/// own.
fn calendar(db: &TimeZoneDatabase, host: usize, extra_blocks_per_day: usize) -> Vec<Block> {
    let mut blocks = Vec::new();
    for day in 0..WINDOW_DAYS {
        let date = first_day().plus_days(day).expect("a window day is a date");
        if day == HOLIDAY_DAY || matches!(date.weekday(), Weekday::Saturday | Weekday::Sunday) {
            continue;
        }
        let zone = day_zone(host, day);
        let today = usize::try_from(day).expect("a window day fits a usize");
        // Focus time through the working day, so a host with a full week is busy
        // in the middle of it. The blocks are read by every scan, every interval
        // set and the cap counting, and they leave the window only the starts no
        // host is busy for.
        for &from in FOCUS_BLOCK_STARTS_MINUTES.iter().take(extra_blocks_per_day) {
            let mut focus = Block::new(span(db, zone, date, from, from + 30));
            focus.event_key = Some(format!("focus/{host}/{day}/{from}"));
            focus.tags = vec![tag("focus")];
            blocks.push(focus);
        }
        // One host in four takes the first Thursday off.
        if host.is_multiple_of(4) && day == 3 {
            let next = date
                .plus_days(1)
                .expect("the day after a window day is a date");
            let mut leave = Block::new(
                Span::new(local(db, zone, date, 0), local(db, zone, next, 0))
                    .expect("a civil day covers at least one minute"),
            );
            leave.all_day = true;
            leave.event_key = Some(format!("leave/{host}/{day}"));
            leave.tags = vec![tag("leave")];
            blocks.push(leave);
            continue;
        }
        // The group's standup: one event on five calendars, with no occurrence.
        if host % 5 != 4 {
            let mut standup = Block::new(span(db, zone, date, 9 * 60 + 15, 9 * 60 + 30));
            standup.event_key = Some(format!("standup/{}/{day}", host / 5));
            standup.tags = vec![tag("team")];
            blocks.push(standup);
        }
        // The interviews they already hold, and the Monday hiring session most
        // of the pool sits on.
        for (slot, from) in interview_slots(host, day).into_iter().enumerate() {
            let mut interview = Block::new(span(db, zone, date, from, from + 30));
            interview.event_key = Some(format!("interview/{host}/{day}/{slot}"));
            interview.tags = vec![tag("interview")];
            blocks.push(interview);
        }
        // A hold nobody has confirmed, which the status policy treats as free.
        if (host + today).is_multiple_of(3) {
            let mut hold = Block::new(span(db, zone, date, 13 * 60, 13 * 60 + 30));
            hold.status = BlockStatus::Tentative;
            hold.event_key = Some(format!("hold/{host}/{day}"));
            hold.tags = vec![tag("hold")];
            blocks.push(hold);
        }
        // A client visit across town, with the journeys either side.
        if (host + today) % 4 == 1 {
            let mut visit = Block::new(span(db, zone, date, 15 * 60, 16 * 60));
            visit.travel_before = Duration::from_minutes(20);
            visit.travel_after = Duration::from_minutes(25);
            visit.event_key = Some(format!("client/{host}/{day}"));
            visit.tags = vec![tag("client")];
            blocks.push(visit);
        }
        // An evening finish on some days, which excludes the next morning.
        if (host * 3 + today).is_multiple_of(5) {
            let mut evening = Block::new(span(db, zone, date, 17 * 60, 18 * 60 + 30));
            evening.travel_after = Duration::from_minutes(15);
            evening.event_key = Some(format!("onsite/{host}/{day}"));
            evening.tags = vec![tag("onsite")];
            blocks.push(evening);
        }
    }
    blocks
}

/// The caller's own booked interviews, listed as occurrences so the cap counts
/// them once however many calendars carry them.
fn occurrences(db: &TimeZoneDatabase, first: Date) -> Vec<Occurrence> {
    let mut occurrences = Vec::new();
    for day in 0..WINDOW_DAYS {
        let date = first.plus_days(day).expect("a window day is a date");
        if day == HOLIDAY_DAY || matches!(date.weekday(), Weekday::Saturday | Weekday::Sunday) {
            continue;
        }
        for host in 0..PARTICIPANTS {
            let zone = day_zone(host, day);
            for (slot, from) in interview_slots(host, day).into_iter().enumerate() {
                let mut occurrence = Occurrence::new(
                    format!("interview/{host}/{day}/{slot}"),
                    span(db, zone, date, from, from + 30),
                    vec![host_id(host)],
                )
                .expect("a booked interview names its host");
                occurrence.tags = vec![tag("interview")];
                occurrences.push(occurrence);
            }
        }
    }
    occurrences
}

/// The local minute each of a day's interviews starts at: one to three from ten
/// o'clock in forty-five minute steps, plus the hiring session most of the pool
/// sits on the window's first Monday.
fn interview_slots(host: usize, day: i64) -> Vec<u16> {
    let today = usize::try_from(day).expect("a window day fits a usize");
    let count = 1 + (host / 2 + today) % 3;
    let mut starts: Vec<u16> = (0..count).map(interview_start).collect();
    if day % 7 == 0 && host < 40 {
        starts.push(14 * 60);
    }
    starts
}

/// The local minute a day's `slot`-th interview starts at.
fn interview_start(slot: usize) -> u16 {
    let offset = u16::try_from(slot).expect("a day holds three interview slots");
    10 * 60 + 45 * offset
}
