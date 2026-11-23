// A three-tier scheduling policy: the week's global hours, caps and rest; the
// interview group's narrower window and cap; and a frontend tier with its own
// notice and cap. The `vip` tier is *loosened* out of the broader tiers with
// `Rule::unless` rather than intersected with them, so the exception is wider
// than the rule it escapes instead of narrower.

use opportune::Engine;
use opportune::model::{
    Block, CapPer, Explain, Goal, Part, Participant, Request, Requirement, Span, Term,
};
use opportune::presets;
use opportune::reasons::ReasonCode;
use opportune::tags::{Selector, Tag};
use opportune::time::{
    Date, Duration, Instant, RuleZone, TimeOfDay, WallEnd, WallRange, Weekday, Weekdays, WeeklyKey,
    ZoneName,
};

fn tag(path: &str) -> Tag {
    Tag::new(path).expect("a valid tag path")
}

fn utc(date: (i16, i8, i8), clock: (u8, u8)) -> Instant {
    Instant::from_utc(
        Date::new(date.0, date.1, date.2).expect("a valid civil date"),
        TimeOfDay::new(clock.0, clock.1).expect("a valid time of day"),
    )
    .expect("UTC has no transitions")
}

fn wall(from: (u8, u8), until: (u8, u8)) -> WallRange {
    WallRange::new(
        TimeOfDay::new(from.0, from.1).expect("a valid time of day"),
        WallEnd::Time(TimeOfDay::new(until.0, until.1).expect("a valid time of day")),
    )
    .expect("a non-empty wall range")
}

fn vip() -> Selector {
    Selector::tag(tag("vip"))
}

/// An event on 14-18 June 2027, over `[from, until)` UTC, carrying `tags` and
/// identified by `key`.
fn booked(day: i8, from: (u8, u8), until: (u8, u8), tags: &[&str], key: &str) -> Block {
    let mut block = Block::new(
        Span::new(utc((2027, 6, day), from), utc((2027, 6, day), until))
            .expect("a non-empty span"),
    );
    block.tags = tags.iter().map(|path| tag(path)).collect();
    block.event_key = Some(key.to_owned());
    block
}

/// The one participant, with the week already in their calendar.
fn ana() -> Participant {
    Participant::new("ana", ZoneName::new("UTC").expect("UTC is a zone name")).with_calendar(vec![
        booked(14, (17, 30), (19, 30), &["meeting"], "mon_late"),
        booked(15, (9, 0), (10, 0), &["interview"], "tue_1"),
        booked(15, (10, 0), (11, 0), &["interview"], "tue_2"),
        booked(16, (9, 0), (10, 0), &["interview/frontend"], "wed_front"),
    ])
}

/// The 30-minute request for a booking tagged `tags`, offered only at
/// `candidates`, under all three tiers.
fn request(
    candidates: Vec<Instant>,
    tags: &[&str],
) -> Result<Request, Box<dyn std::error::Error>> {
    let mut part = Part::new(
        Duration::from_minutes(30),
        Requirement::new(vec![Term::person("ana")]),
    );
    part.tags = tags.iter().map(|path| tag(path)).collect();
    let goal = Goal::new(
        ZoneName::new("UTC")?,
        Span::new(utc((2027, 6, 14), (0, 0)), utc((2027, 6, 19), (0, 0)))?,
        vec![part],
    )
    .with_candidates(candidates);

    // The global tier: the ordinary week, a daily cap on interviews, and rest
    // after a late evening.
    let week = presets::weekly_hours(
        RuleZone::Event,
        vec![WeeklyKey::new(
            Weekdays::range(Weekday::Monday, Weekday::Friday),
            vec![wall((7, 0), (19, 0))],
        )?],
        Vec::new(),
    )?
    .with_id("week_hours")
    .unless(vip());
    let daily_limit = presets::cap_per_day(Selector::tag(tag("interview")), 2)
        .with_id("daily_limit")
        .unless(vip());
    let rest = presets::rest(TimeOfDay::new(18, 0)?, TimeOfDay::new(8, 0)?);

    // The event-group tier: interviews meet Monday to Thursday, 10:00 to 16:00,
    // and the group carries its own weekly count.
    let interview_hours = presets::business_hours(
        RuleZone::Event,
        Weekdays::range(Weekday::Monday, Weekday::Thursday),
        TimeOfDay::new(10, 0)?,
        WallEnd::Time(TimeOfDay::new(16, 0)?),
    )?
    .with_id("interview_hours")
    .with_when(Selector::tag(tag("interview")))
    .unless(vip());
    let interview_week = presets::cap_per_week(
        Selector::tag(tag("interview")),
        5,
        CapPer::Each,
        RuleZone::Participant,
    )
    .with_id("interview_week")
    .with_when(Selector::tag(tag("interview")));

    // The event-type tier: a frontend interview is the only kind that needs 48
    // hours' notice, and the only kind ana takes one of a day.
    let frontend = Selector::tag(tag("interview/frontend"));
    let frontend_day = presets::cap_per_day(Selector::tag(tag("interview/frontend")), 1)
        .with_id("frontend_day")
        .with_when(frontend.clone());
    let frontend_notice = presets::min_notice(Duration::from_hours_minutes(48, 0))
        .with_id("frontend_notice")
        .with_when(frontend);

    // The exception's own tier: the hours a vip booking is read in. Because the
    // two broader windows except `vip`, these are the only hours it gets.
    let vip_hours = presets::business_hours(
        RuleZone::Event,
        Weekdays::range(Weekday::Monday, Weekday::Friday),
        TimeOfDay::new(7, 0)?,
        WallEnd::Time(TimeOfDay::new(20, 0)?),
    )?
    .with_id("vip_hours")
    .with_when(vip());

    let mut request = Request::new(utc((2027, 6, 14), (6, 0)), goal);
    request.participants = vec![ana()];
    request.rules = vec![
        week,
        interview_hours,
        interview_week,
        daily_limit,
        frontend_day,
        frontend_notice,
        rest,
        vip_hours,
    ];
    request.explain = Some(Explain::new(100));
    Ok(request)
}

/// The code and rule of every reason `excluded` gives for `start`.
fn reasons_at(
    excluded: &[opportune::Excluded],
    start: Instant,
) -> Vec<(ReasonCode, Option<String>)> {
    excluded
        .iter()
        .filter(|entry| entry.start == start)
        .flat_map(|entry| &entry.reasons)
        .map(|reason| (reason.code, reason.rule.clone()))
        .collect()
}

/// The group's weekly count is the tier's own cap. Tightened to the three
/// interviews ana already has this week it closes the week, and the reason names
/// that cap rather than a window.
fn week_cap_closes_the_week(
    engine: &Engine,
    candidates: Vec<Instant>,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut tightened = request(candidates, &["interview"])?;
    tightened.rules.retain(|rule| rule.id != "interview_week");
    tightened.rules.push(
        presets::cap_per_week(
            Selector::tag(tag("interview")),
            3,
            CapPer::Each,
            RuleZone::Participant,
        )
        .with_id("interview_week")
        .with_when(Selector::tag(tag("interview"))),
    );
    let week_full = engine.find_availability(&tightened)?;
    assert!(week_full.offers.is_empty());
    assert_eq!(
        reasons_at(&week_full.excluded, utc((2027, 6, 14), (10, 30))),
        [(ReasonCode::CapReached, Some("interview_week".to_owned()))]
    );
    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let engine = Engine::builder().build()?;
    let candidates = vec![
        utc((2027, 6, 14), (10, 30)), // Monday, inside both windows
        utc((2027, 6, 14), (16, 30)), // Monday, after the group window closes
        utc((2027, 6, 15), (7, 30)),  // Tuesday, before the group window opens
        utc((2027, 6, 15), (13, 0)),  // Tuesday, at the global daily cap
        utc((2027, 6, 16), (13, 0)),  // Wednesday, at the frontend daily cap
        utc((2027, 6, 17), (11, 0)),  // Thursday, inside both windows
        utc((2027, 6, 18), (11, 0)),  // Friday, a day the group tier closes
    ];
    let starts = |offers: &[opportune::Offer]| -> Vec<Instant> {
        offers
            .iter()
            .filter_map(opportune::Offer::start)
            .collect()
    };

    // An ordinary interview sees the intersection of the global and group tiers
    // and is limited by the global daily cap, so the Friday start the global hours
    // allow is closed and Tuesday is full.
    let ordinary = engine.find_availability(&request(candidates.clone(), &["interview"])?)?;
    assert_eq!(
        starts(&ordinary.offers),
        [
            utc((2027, 6, 14), (10, 30)),
            utc((2027, 6, 16), (13, 0)),
            utc((2027, 6, 17), (11, 0)),
        ]
    );
    assert_eq!(
        reasons_at(&ordinary.excluded, utc((2027, 6, 14), (16, 30))),
        [(
            ReasonCode::OutsideWindow,
            Some("interview_hours".to_owned())
        )]
    );
    assert_eq!(
        reasons_at(&ordinary.excluded, utc((2027, 6, 15), (13, 0))),
        [(ReasonCode::CapReached, Some("daily_limit".to_owned()))]
    );
    assert_eq!(
        reasons_at(&ordinary.excluded, utc((2027, 6, 18), (11, 0))),
        [(
            ReasonCode::OutsideWindow,
            Some("interview_hours".to_owned())
        )]
    );

    // The group's weekly count is the tier's own cap.
    week_cap_closes_the_week(&engine, candidates.clone())?;

    // A vip booking is excepted from both windows and from the daily cap, so it
    // gets the wider vip hours — Friday included, which is the loosening that
    // matters: intersecting the two tiers would have left Friday closed.
    let vip_booking = engine.find_availability(&request(candidates, &["interview", "vip"])?)?;
    assert_eq!(
        starts(&vip_booking.offers),
        [
            utc((2027, 6, 14), (10, 30)),
            utc((2027, 6, 14), (16, 30)),
            utc((2027, 6, 15), (13, 0)),
            utc((2027, 6, 16), (13, 0)),
            utc((2027, 6, 17), (11, 0)),
            utc((2027, 6, 18), (11, 0)),
        ]
    );
    // The excepted tiers are only the windows and the daily cap: rest still
    // protects Tuesday's early morning after Monday's late meeting.
    assert_eq!(
        reasons_at(&vip_booking.excluded, utc((2027, 6, 15), (7, 30))),
        [(ReasonCode::Rest, Some("rest".to_owned()))]
    );

    // A frontend interview inherits every tier and adds its own: 48 hours' notice
    // and at most one a day.
    let frontend_candidates = vec![
        utc((2027, 6, 14), (10, 30)),
        utc((2027, 6, 16), (13, 0)),
        utc((2027, 6, 17), (11, 0)),
        utc((2027, 6, 18), (11, 0)),
    ];
    let frontend = engine.find_availability(&request(
        frontend_candidates,
        &["interview", "interview/frontend"],
    )?)?;
    assert_eq!(
        starts(&frontend.offers),
        [utc((2027, 6, 17), (11, 0))]
    );
    assert_eq!(
        reasons_at(&frontend.excluded, utc((2027, 6, 14), (10, 30))),
        [(
            ReasonCode::LeadTimeMin,
            Some("frontend_notice".to_owned())
        )]
    );
    assert_eq!(
        reasons_at(&frontend.excluded, utc((2027, 6, 16), (13, 0))),
        [(ReasonCode::CapReached, Some("frontend_day".to_owned()))]
    );
    Ok(())
}
