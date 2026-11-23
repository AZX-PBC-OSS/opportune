//! End-to-end cap time-correctness fixtures: the hand-computed cases from the
//! plan's time section, run through the engine.
//!
//! Every fixture fixes the calendar, the cap and the booking by hand and states
//! the exact answer a reader can check: February month buckets in a leap year and
//! an ordinary one, day buckets across daylight saving transitions, a week bucket
//! switching mid-week when a participant changes zone, the occurrence hull across
//! the date line, weekly caps across New Year, and weeks that start on a weekday
//! other than Monday.

use std::sync::OnceLock;

use opportune::Engine;
use opportune::model::{
    Block, Cap, CapMeasure, CapPer, CapPeriod, Explain, Goal, Participant, RangesQuery, Request,
    Requirement, Rule, RuleKind, Span, Term, ZonePeriod,
};
use opportune::reasons::{ReasonCode, ReasonParams};
use opportune::tags::{Selector, Tag};
use opportune::time::{
    Date, Duration, Instant, LocalResolution, RuleZone, TimeOfDay, TimeZoneDatabase, Weekday,
    ZoneName,
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

/// The instant a local date-time falls on, in `zone`, when it exists once.
fn local(zone: &str, day: Date, hour: u8, minute: u8) -> Instant {
    match database()
        .zone(zone)
        .unwrap_or_else(|err| panic!("`{zone}` is in the bundled database: {err}"))
        .resolve_local(day, time(hour, minute))
        .expect("the fixture instant is inside the supported range")
    {
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

/// The earlier instant a repeated local date-time falls on, in `zone`.
fn local_earlier(zone: &str, day: Date, hour: u8, minute: u8) -> Instant {
    match database()
        .zone(zone)
        .unwrap_or_else(|err| panic!("`{zone}` is in the bundled database: {err}"))
        .resolve_local(day, time(hour, minute))
        .expect("the fixture instant is inside the supported range")
    {
        LocalResolution::Repeated(earlier, _) => earlier,
        LocalResolution::Unique(_) | LocalResolution::Skipped => {
            panic!("{} {} in {zone} must occur twice", day, time(hour, minute));
        }
    }
}

/// The later instant a repeated local date-time falls on, in `zone`.
fn local_later(zone: &str, day: Date, hour: u8, minute: u8) -> Instant {
    match database()
        .zone(zone)
        .unwrap_or_else(|err| panic!("`{zone}` is in the bundled database: {err}"))
        .resolve_local(day, time(hour, minute))
        .expect("the fixture instant is inside the supported range")
    {
        LocalResolution::Repeated(_, later) => later,
        LocalResolution::Unique(_) | LocalResolution::Skipped => {
            panic!("{} {} in {zone} must occur twice", day, time(hour, minute));
        }
    }
}

fn tag(path: &str) -> Tag {
    Tag::new(path).expect("a fixture tag is a valid tag")
}

fn span(start: Instant, end: Instant) -> Span {
    Span::new(start, end).expect("the fixture span is non-empty")
}

/// A keyless block tagged `interview` over `[start, end)`.
fn interview(start: Instant, end: Instant) -> Block {
    let mut block = Block::new(span(start, end));
    block.tags = vec![tag("interview")];
    block
}

/// A day, week or month cap counting `interview` events for one participant.
fn cap(limit: u32, period: CapPeriod, week_starts: Weekday) -> Rule {
    Rule::new(
        "interview_cap",
        RuleKind::Cap(Cap::new(
            Selector::Tag(tag("interview")),
            CapMeasure::Count,
            limit,
            period,
            CapPer::Each,
            RuleZone::Participant,
            week_starts,
        )),
    )
}

/// A one-participant request with the given search, candidates and calendar.
fn request(
    zone: &str,
    search: Span,
    candidates: Vec<Instant>,
    participant: Participant,
    rules: Vec<Rule>,
) -> Request {
    let mut goal = Goal::single(
        name(zone),
        search,
        Duration::from_minutes(30),
        Requirement::new(vec![Term::person("ana")]),
    );
    goal.candidates = Some(candidates);
    goal.parts[0].tags = vec![tag("interview")];
    let mut request = Request::new(search.start(), goal);
    request.participants.push(participant);
    request.rules = rules;
    request.explain = Some(Explain::new(10_000));
    request
}

fn host(zone: &str, blocks: Vec<Block>) -> Participant {
    let mut participant = Participant::new("ana", name(zone));
    participant.calendar = Some(blocks);
    participant
}

/// The bucket label and usage the exclusion at `start` reports.
fn cap_params(request: &Request, start: Instant) -> (u64, u32, String) {
    let response = engine()
        .find_availability(request)
        .expect("the fixture request is valid");
    let reason = response
        .excluded
        .iter()
        .find(|entry| entry.start == start)
        .unwrap_or_else(|| panic!("{start} is excluded: {:?}", response.offers))
        .reasons
        .iter()
        .find(|reason| reason.code == ReasonCode::CapReached)
        .unwrap_or_else(|| panic!("{start} is excluded by the cap"))
        .clone();
    let params = reason.params.expect("a cap reason carries parameters");
    let ReasonParams::CapReached {
        used,
        limit,
        bucket,
    } = params
    else {
        panic!("a cap_reached reason carries cap parameters");
    };
    (used, limit, bucket)
}

fn offered(request: &Request) -> Vec<Instant> {
    engine()
        .find_availability(request)
        .expect("the fixture request is valid")
        .offers
        .iter()
        .map(|offer| offer.start().expect("an offer has an event"))
        .collect()
}

/// The occurrence hull a request needs.
fn occurrence_hull(request: &Request) -> Span {
    let mut query = RangesQuery::new(request.now, request.goal.clone());
    query.rules.clone_from(&request.rules);
    query.participants.clone_from(&request.participants);
    engine()
        .required_ranges(&query)
        .expect("the fixture query resolves")
        .occurrences
        .expect("a cap applies, so occurrences are needed")
}

#[test]
fn a_february_month_bucket_counts_the_leap_day_and_ends_on_the_first_of_march() {
    // One interview on the leap day fills a one-a-month cap, so a later February
    // start is excluded; the bucket runs from 1 February to 1 March, and its label
    // is the first of the month. February 2027 has no 29th, and its bucket is the
    // same shape.
    let leap_day = date(2028, 2, 29);
    let leap_request = request(
        "UTC",
        span(local("UTC", leap_day, 9, 0), local("UTC", leap_day, 20, 0)),
        vec![local("UTC", leap_day, 15, 0)],
        host(
            "UTC",
            vec![interview(
                local("UTC", leap_day, 9, 0),
                local("UTC", leap_day, 10, 0),
            )],
        ),
        vec![cap(1, CapPeriod::Month, Weekday::Monday)],
    );
    assert_eq!(offered(&leap_request), Vec::<Instant>::new());
    assert_eq!(
        cap_params(&leap_request, local("UTC", leap_day, 15, 0)),
        (1, 1, "2028-02-01".to_owned())
    );
    assert_eq!(
        occurrence_hull(&leap_request),
        span(
            local("UTC", date(2028, 2, 1), 0, 0),
            local("UTC", date(2028, 3, 1), 0, 0)
        ),
        "February 2028's bucket runs over the leap day"
    );

    // 2027: one interview on 1 February fills the same bucket, whose label is
    // 2027-02-01 and whose end is 1 March.
    let ordinary_request = request(
        "UTC",
        span(
            local("UTC", date(2027, 2, 28), 9, 0),
            local("UTC", date(2027, 2, 28), 20, 0),
        ),
        vec![local("UTC", date(2027, 2, 28), 15, 0)],
        host(
            "UTC",
            vec![interview(
                local("UTC", date(2027, 2, 1), 9, 0),
                local("UTC", date(2027, 2, 1), 10, 0),
            )],
        ),
        vec![cap(1, CapPeriod::Month, Weekday::Monday)],
    );
    assert_eq!(
        cap_params(&ordinary_request, local("UTC", date(2027, 2, 28), 15, 0)),
        (1, 1, "2027-02-01".to_owned())
    );
    assert_eq!(
        occurrence_hull(&ordinary_request),
        span(
            local("UTC", date(2027, 2, 1), 0, 0),
            local("UTC", date(2027, 3, 1), 0, 0)
        )
    );
}

#[test]
fn a_day_bucket_across_a_daylight_saving_change_is_the_civil_day() {
    // New York's fall-back day is 25 hours long and its spring-forward day 23. An
    // interview at 01:30 local, before the fall-back, still belongs to that day,
    // and the hull covers the whole 25 hours.
    let fall_back = date(2027, 11, 7);
    let fall_request = request(
        "America/New_York",
        span(
            local("America/New_York", fall_back, 0, 0),
            local("America/New_York", fall_back, 20, 0),
        ),
        vec![local("America/New_York", fall_back, 14, 0)],
        host(
            "America/New_York",
            vec![interview(
                local_earlier("America/New_York", fall_back, 1, 30),
                local_earlier("America/New_York", fall_back, 1, 45),
            )],
        ),
        vec![cap(1, CapPeriod::Day, Weekday::Monday)],
    );
    assert_eq!(
        cap_params(&fall_request, local("America/New_York", fall_back, 14, 0)),
        (1, 1, "2027-11-07".to_owned())
    );
    let hull = occurrence_hull(&fall_request);
    assert_eq!(hull.minutes(), 25 * 60, "the fall-back day is 25 hours");
    assert_eq!(hull.start().to_string(), "2027-11-07T04:00Z");
    assert_eq!(hull.end().to_string(), "2027-11-08T05:00Z");

    let spring_forward = date(2027, 3, 14);
    let spring_request = request(
        "America/New_York",
        span(
            local("America/New_York", spring_forward, 0, 0),
            local("America/New_York", spring_forward, 20, 0),
        ),
        vec![local("America/New_York", spring_forward, 14, 0)],
        host(
            "America/New_York",
            vec![interview(
                local("America/New_York", spring_forward, 4, 0),
                local("America/New_York", spring_forward, 5, 0),
            )],
        ),
        vec![cap(1, CapPeriod::Day, Weekday::Monday)],
    );
    assert_eq!(
        cap_params(
            &spring_request,
            local("America/New_York", spring_forward, 14, 0)
        ),
        (1, 1, "2027-03-14".to_owned())
    );
    assert_eq!(
        occurrence_hull(&spring_request).minutes(),
        23 * 60,
        "the spring-forward day is 23 hours"
    );
}

#[test]
fn a_week_bucket_switches_when_the_participant_changes_zone_mid_week() {
    // A week that starts on Wednesday: ana is in London until Wednesday, then in
    // Tokyo for a day, then home again. The London days fall in the week of 9
    // June; the Tokyo day falls in the week of 16 June. Each existing interview
    // fills its own bucket, so both candidates are excluded and each reason names
    // the bucket its own zone produced.
    let start = local("Europe/London", date(2027, 6, 14), 0, 0);
    let leg_from = local("Europe/London", date(2027, 6, 16), 0, 0);
    let leg_until = local("Europe/London", date(2027, 6, 17), 0, 0);
    let mut ana = host(
        "Europe/London",
        vec![
            interview(
                local("Europe/London", date(2027, 6, 14), 10, 0),
                local("Europe/London", date(2027, 6, 14), 11, 0),
            ),
            interview(
                local("Asia/Tokyo", date(2027, 6, 16), 10, 0),
                local("Asia/Tokyo", date(2027, 6, 16), 11, 0),
            ),
        ],
    );
    ana.zone_periods = vec![
        ZonePeriod::new(name("Asia/Tokyo"), leg_from, leg_until).expect("the leg is non-empty"),
    ];
    let monday_candidate = local("Europe/London", date(2027, 6, 14), 14, 0);
    let wednesday_candidate = local("Asia/Tokyo", date(2027, 6, 16), 14, 0);
    let request = request(
        "Europe/London",
        span(start, local("Europe/London", date(2027, 6, 18), 0, 0)),
        vec![monday_candidate, wednesday_candidate],
        ana,
        vec![cap(1, CapPeriod::Week, Weekday::Wednesday)],
    );
    assert_eq!(
        cap_params(&request, monday_candidate),
        (1, 1, "2027-06-09".to_owned()),
        "the London day falls in the week that began on 9 June"
    );
    assert_eq!(
        cap_params(&request, wednesday_candidate),
        (1, 1, "2027-06-16".to_owned()),
        "the Tokyo day falls in the week that began on 16 June"
    );

    // The hull covers both weeks, each read in the zone that was in force: the
    // London days pull it back to 9 June in London, the Tokyo day pushes it out
    // to the end of Tokyo's week of 16 June, and the London days after the trip
    // close the week in London's own zone.
    let hull = occurrence_hull(&request);
    assert_eq!(
        hull.start().to_string(),
        "2027-06-08T23:00Z",
        "9 June in London"
    );
    assert_eq!(
        hull.end().to_string(),
        "2027-06-22T23:00Z",
        "23 June in London"
    );
}

#[test]
fn the_occurrence_hull_across_the_date_line_covers_both_of_a_repeated_day() {
    // Ana flies from Kiritimati (UTC+14) to Midway (UTC-11), so the local date
    // falls back and 14 June happens twice. A day cap in her own zone must fetch
    // both the Kiritimati day that begins on 13 June UTC and the Midway day that
    // ends on 16 June UTC.
    let leg_from = local("UTC", date(2027, 6, 15), 0, 0);
    let leg_while = local("UTC", date(2027, 6, 15), 12, 0);
    let leg_until = local("UTC", date(2027, 6, 16), 0, 0);
    let mut ana = host("Pacific/Kiritimati", Vec::new());
    ana.zone_periods = vec![
        ZonePeriod::new(name("Pacific/Midway"), leg_from, leg_until).expect("the leg is non-empty"),
    ];
    let request = request(
        "Pacific/Kiritimati",
        span(local("UTC", date(2027, 6, 14), 0, 0), leg_while),
        vec![leg_while],
        ana,
        vec![cap(3, CapPeriod::Day, Weekday::Monday)],
    );
    let hull = occurrence_hull(&request);
    assert_eq!(
        hull.start().to_string(),
        "2027-06-13T10:00Z",
        "14 June begins in Kiritimati"
    );
    assert_eq!(
        hull.end().to_string(),
        "2027-06-16T00:00Z",
        "the Midway day ends when the participant leaves, after the repeated 14th"
    );
}

#[test]
fn a_weekly_cap_runs_across_new_year_as_seven_civil_days() {
    // The week that starts on Monday 27 December 2027 ends on Monday 3 January
    // 2028, so the interview on New Year's Eve fills the cap for 30 December but
    // not for 3 January.
    let request_for = |window: (Instant, Instant), candidate: Instant| {
        request(
            "UTC",
            span(window.0, window.1),
            vec![candidate],
            host(
                "UTC",
                vec![interview(
                    local("UTC", date(2027, 12, 31), 9, 0),
                    local("UTC", date(2027, 12, 31), 10, 0),
                )],
            ),
            vec![cap(1, CapPeriod::Week, Weekday::Monday)],
        )
    };
    let year_end_window = (
        local("UTC", date(2027, 12, 30), 9, 0),
        local("UTC", date(2027, 12, 30), 20, 0),
    );
    let year_end = request_for(year_end_window, local("UTC", date(2027, 12, 30), 14, 0));
    assert_eq!(
        cap_params(&year_end, local("UTC", date(2027, 12, 30), 14, 0)),
        (1, 1, "2027-12-27".to_owned())
    );
    assert_eq!(
        occurrence_hull(&year_end),
        span(
            local("UTC", date(2027, 12, 27), 0, 0),
            local("UTC", date(2028, 1, 3), 0, 0)
        )
    );

    let new_year_window = (
        local("UTC", date(2028, 1, 3), 9, 0),
        local("UTC", date(2028, 1, 3), 20, 0),
    );
    let new_year = request_for(new_year_window, local("UTC", date(2028, 1, 3), 14, 0));
    assert_eq!(
        offered(&new_year),
        [local("UTC", date(2028, 1, 3), 14, 0)],
        "the new year begins a new week"
    );
    assert_eq!(
        occurrence_hull(&new_year),
        span(
            local("UTC", date(2028, 1, 3), 0, 0),
            local("UTC", date(2028, 1, 10), 0, 0)
        )
    );
}

#[test]
fn a_week_can_start_on_a_weekday_other_than_monday() {
    // With weeks starting on Sunday, Saturday 9 January 2027 shares a bucket with
    // Sunday 3 January, and Sunday 10 January begins the next.
    let request_for = |window: (Instant, Instant), candidate: Instant| {
        request(
            "UTC",
            span(window.0, window.1),
            vec![candidate],
            host(
                "UTC",
                vec![interview(
                    local("UTC", date(2027, 1, 6), 9, 0),
                    local("UTC", date(2027, 1, 6), 10, 0),
                )],
            ),
            vec![cap(1, CapPeriod::Week, Weekday::Sunday)],
        )
    };
    let saturday_window = (
        local("UTC", date(2027, 1, 9), 9, 0),
        local("UTC", date(2027, 1, 9), 20, 0),
    );
    let saturday = request_for(saturday_window, local("UTC", date(2027, 1, 9), 14, 0));
    assert_eq!(
        cap_params(&saturday, local("UTC", date(2027, 1, 9), 14, 0)),
        (1, 1, "2027-01-03".to_owned())
    );
    assert_eq!(
        occurrence_hull(&saturday),
        span(
            local("UTC", date(2027, 1, 3), 0, 0),
            local("UTC", date(2027, 1, 10), 0, 0)
        )
    );

    let sunday_window = (
        local("UTC", date(2027, 1, 10), 9, 0),
        local("UTC", date(2027, 1, 10), 20, 0),
    );
    let sunday = request_for(sunday_window, local("UTC", date(2027, 1, 10), 14, 0));
    assert_eq!(offered(&sunday), [local("UTC", date(2027, 1, 10), 14, 0)]);
    assert_eq!(
        occurrence_hull(&sunday),
        span(
            local("UTC", date(2027, 1, 10), 0, 0),
            local("UTC", date(2027, 1, 17), 0, 0)
        )
    );
}

#[test]
fn a_day_bucket_holds_the_tail_of_a_date_the_next_midnight_repeats() {
    // Newfoundland once ended daylight saving by moving 00:01 back to 23:01 the
    // previous day, so 31 October 2009 runs an hour, 1 November begins, and then
    // 31 October runs again for its last fifty-nine minutes. An interview in that
    // tail is still read on 31 October, so the day bucket's hull has to reach past
    // the *first* 1 November midnight to the second.
    let day = date(2009, 10, 31);
    let next = date(2009, 11, 1);
    let window = span(
        local("America/St_Johns", day, 22, 0),
        local("America/St_Johns", day, 23, 0),
    );
    let candidate = local("America/St_Johns", day, 22, 30);
    let tail = local_later("America/St_Johns", day, 23, 30);
    let request = request(
        "America/St_Johns",
        window,
        vec![candidate],
        host(
            "America/St_Johns",
            vec![interview(
                tail,
                local_later("America/St_Johns", day, 23, 59),
            )],
        ),
        vec![cap(1, CapPeriod::Day, Weekday::Monday)],
    );
    assert_eq!(
        cap_params(&request, candidate),
        (1, 1, "2009-10-31".to_owned()),
        "the tail of 31 October still counts toward that day"
    );
    let hull = occurrence_hull(&request);
    assert!(
        hull.start() <= tail && tail < hull.end(),
        "the hull {hull:?} must reach past the first {next} midnight to the tail at {tail}"
    );
}
