// Evening-to-morning rest, a working-day horizon, and a minimum notice.

use std::collections::BTreeMap;

use opportune::Engine;
use opportune::model::{
    Block, CalendarSpec, Goal, LeadTimeMax, Participant, Request, Requirement, Span, Term,
};
use opportune::presets;
use opportune::time::{
    Date, Duration, Instant, RuleZone, TimeOfDay, WallEnd, Weekday, Weekdays, WorkingDays, ZoneName,
};

fn utc(date: (i16, i8, i8), clock: (u8, u8)) -> Instant {
    Instant::from_utc(
        Date::new(date.0, date.1, date.2).expect("a valid civil date"),
        TimeOfDay::new(clock.0, clock.1).expect("a valid time of day"),
    )
    .expect("UTC has no transitions")
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let new_york = ZoneName::new("America/New_York")?;
    // Sunday 18:00 in New York, with a fundraiser that runs to 21:00.
    let now = utc((2027, 6, 13), (22, 0));
    let mut fundraiser = Block::new(Span::new(
        utc((2027, 6, 13), (23, 0)),
        utc((2027, 6, 14), (1, 0)),
    )?);
    fundraiser.event_key = Some("fundraiser".to_owned());

    let window = Span::new(utc((2027, 6, 14), (4, 0)), utc((2027, 6, 19), (4, 0)))?;
    let mut request = Request::new(
        now,
        Goal::single(
            new_york.clone(),
            window,
            Duration::from_minutes(30),
            Requirement::new(vec![Term::person("ana")]),
        ),
    );
    request
        .participants
        .push(Participant::new("ana", new_york.clone()).with_calendar(vec![fundraiser]));
    request.rules = vec![
        presets::business_hours(
            RuleZone::Participant,
            Weekdays::range(Weekday::Monday, Weekday::Friday),
            TimeOfDay::new(9, 0)?,
            WallEnd::Time(TimeOfDay::new(17, 0)?),
        )?,
        presets::rest(TimeOfDay::new(18, 0)?, TimeOfDay::new(10, 0)?),
        presets::min_notice(Duration::from_hours_minutes(16, 0)),
        presets::horizon(LeadTimeMax::WorkingDays {
            count: 4,
            calendar: "org".to_owned(),
        }),
    ];
    request.calendars = BTreeMap::from([(
        "org".to_owned(),
        CalendarSpec {
            zone: new_york.clone(),
            working_days: WorkingDays::weekdays(),
            holidays: Vec::new(),
        },
    )]);

    let engine = Engine::builder().build()?;
    let response = engine.find_availability(&request)?;

    // The late fundraiser protects Monday before 10:00, and sixteen hours of
    // notice from Sunday 18:00 lands at the same minute.
    let first = response.offers.first().and_then(opportune::Offer::start);
    assert_eq!(first, Some(utc((2027, 6, 14), (14, 0))));
    let monday = response
        .dates
        .iter()
        .find(|count| count.date == Date::new(2027, 6, 14).expect("a date"))
        .map(|count| count.offers);
    assert_eq!(monday, Some(14));

    // Four working days after Sunday is Thursday, so Friday stays empty even
    // though the search window reaches it.
    let friday = response
        .dates
        .iter()
        .find(|count| count.date == Date::new(2027, 6, 18).expect("a date"))
        .map(|count| count.offers);
    assert_eq!(friday, Some(0));
    let last = response.offers.last().and_then(opportune::Offer::start);
    let display = engine
        .time_zone_database()
        .zone(request.goal.display_zone.as_str())?;
    assert_eq!(
        last.map(|start| display.zoned(start).date()),
        Some(Date::new(2027, 6, 17).expect("a date"))
    );
    Ok(())
}
