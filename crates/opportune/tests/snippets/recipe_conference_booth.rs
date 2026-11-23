// A conference booth that is open only on the event's dated hours, with a
// delegate whose zone follows them to the venue.

use opportune::Engine;
use opportune::model::{
    Goal, Participant, Part, Request, Requirement, Span, Term, ZonePeriod,
};
use opportune::presets;
use opportune::tags::{Selector, Tag};
use opportune::time::{
    Date, DateKey, DateRange, Duration, Instant, RuleZone, TimeOfDay, WallEnd, WallRange, Weekday,
    Weekdays, ZoneName,
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

fn booth_day(day: i8) -> Result<DateKey, Box<dyn std::error::Error>> {
    Ok(DateKey::new(
        DateRange::single(Date::new(2027, 6, day)?),
        vec![WallRange::new(
            TimeOfDay::new(10, 0)?,
            WallEnd::Time(TimeOfDay::new(16, 0)?),
        )?],
    )?)
}

fn delegate(id: &str, home: &str) -> Result<Participant, Box<dyn std::error::Error>> {
    let mut host = Participant::new(id, ZoneName::new(home)?);
    host.tags = vec![tag("gtm")];
    host.calendar = Some(Vec::new());
    Ok(host)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let venue = ZoneName::new("America/New_York")?;
    let mut london = delegate("gtm-london", "Europe/London")?;
    // The London delegate is at the venue for the conference, so their own hours
    // are read in the venue's clock for those days.
    london.zone_periods = vec![ZonePeriod::new(
        venue.clone(),
        utc((2027, 6, 13), (0, 0)),
        utc((2027, 6, 17), (0, 0)),
    )?];

    let part = Part {
        id: "booth".to_owned(),
        tags: vec![tag("conference")],
        ..Part::new(
            Duration::from_minutes(20),
            Requirement::new(vec![Term::at_least(1, Selector::tag(tag("gtm")))]),
        )
    };

    let mut request = Request::new(
        utc((2027, 6, 1), (12, 0)),
        Goal::new(
            venue.clone(),
            Span::new(utc((2027, 6, 14), (0, 0)), utc((2027, 6, 17), (4, 0)))?,
            vec![part],
        ),
    );
    request.participants = vec![london, delegate("gtm-berlin", "Europe/Berlin")?];
    request.rules = vec![
        presets::dated_hours(
            RuleZone::Named(venue),
            vec![booth_day(14)?, booth_day(15)?, booth_day(16)?],
        )?
        .with_id("booth_hours"),
        presets::business_hours(
            RuleZone::Participant,
            Weekdays::range(Weekday::Monday, Weekday::Friday),
            TimeOfDay::new(9, 0)?,
            WallEnd::Time(TimeOfDay::new(17, 0)?),
        )?,
        presets::buffers(Duration::from_minutes(5), Duration::from_minutes(5)),
    ];

    let engine = Engine::builder().build()?;
    let response = engine.find_availability(&request)?;

    // Eighteen twenty-minute slots on each of the three booth days.
    assert_eq!(response.offers.len(), 54);
    for day in [14, 15, 16] {
        assert_eq!(
            response
                .dates
                .iter()
                .find(|count| count.date == Date::new(2027, 6, day).expect("a date"))
                .map(|count| count.offers),
            Some(18)
        );
    }
    let eligible_at = |hour: u8| -> Vec<Vec<String>> {
        response
            .offers
            .iter()
            .find(|offer| offer.start() == Some(utc((2027, 6, 14), (hour, 0))))
            .expect("the slot is offered")
            .events[0]
            .eligible
            .clone()
    };
    // At 10:00 New York both delegates are inside their own working hours; at
    // 15:00 only the one whose zone period moved them to the venue is.
    assert_eq!(
        eligible_at(14),
        vec![vec!["gtm-berlin".to_owned(), "gtm-london".to_owned()]]
    );
    assert_eq!(eligible_at(19), vec![vec!["gtm-london".to_owned()]]);

    // Without the zone period the London delegate keeps London hours, which end
    // at 12:00 in New York, and the afternoon slots disappear.
    let mut home_only = request.clone();
    home_only.participants[0].zone_periods.clear();
    let shortened = engine.find_availability(&home_only)?;
    assert_eq!(shortened.offers.len(), 18);
    Ok(())
}
