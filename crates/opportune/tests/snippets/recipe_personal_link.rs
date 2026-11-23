// A personal 30-minute booking link, and the booking page it fills.

use std::collections::BTreeMap;

use opportune::Engine;
use opportune::model::{Goal, Participant, Request, Requirement, Span, Term};
use opportune::presets;
use opportune::time::{
    Date, Duration, Instant, RuleZone, TimeOfDay, WallEnd, Weekday, Weekdays, ZoneName,
};

/// The UTC instant of a civil date and time, for readable request data.
fn utc(date: (i16, i8, i8), clock: (u8, u8)) -> Instant {
    Instant::from_utc(
        Date::new(date.0, date.1, date.2).expect("a valid civil date"),
        TimeOfDay::new(clock.0, clock.1).expect("a valid time of day"),
    )
    .expect("UTC has no transitions")
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let london = ZoneName::new("Europe/London")?;
    let now = utc((2027, 6, 14), (7, 0)); // Monday, 08:00 in London
    let window = Span::new(utc((2027, 6, 14), (0, 0)), utc((2027, 6, 19), (0, 0)))?;
    let mut request = Request::new(
        now,
        Goal::single(
            london.clone(),
            window,
            Duration::from_minutes(30),
            Requirement::new(vec![Term::person("rich")]),
        ),
    );
    request
        .participants
        .push(Participant::new("rich", london).with_calendar(vec![]));
    request.rules.push(presets::business_hours(
        RuleZone::Participant,
        Weekdays::range(Weekday::Monday, Weekday::Friday),
        TimeOfDay::new(9, 0)?,
        WallEnd::Time(TimeOfDay::new(17, 0)?),
    )?);
    request
        .rules
        .push(presets::buffers(Duration::from_minutes(10), Duration::from_minutes(10)));
    request
        .rules
        .push(presets::min_notice(Duration::from_hours_minutes(4, 0)));

    let engine = Engine::builder().build()?;
    let response = engine.find_availability(&request)?;

    // A booking page: every date the window touches gets a row, and each offer is
    // listed under its date in the display zone.
    let display = engine
        .time_zone_database()
        .zone(request.goal.display_zone.as_str())?;
    let mut page: BTreeMap<String, Vec<String>> = response
        .dates
        .iter()
        .map(|count| (count.date.to_string(), Vec::new()))
        .collect();
    for offer in &response.offers {
        let start = offer.start().ok_or("an offer has an event")?;
        page.entry(display.zoned(start).date().to_string())
            .or_default()
            .push(start.to_string());
    }

    // Four hours of notice from Monday 08:00 opens the week at 12:00 London.
    assert_eq!(page["2027-06-14"].first().map(String::as_str), Some("2027-06-14T11:00Z"));
    assert_eq!(page["2027-06-14"].len(), 10);
    assert_eq!(page["2027-06-15"].len(), 16);
    // The window reaches into Saturday, so the page lists it with nothing to book.
    assert!(page["2027-06-19"].is_empty());
    Ok(())
}
