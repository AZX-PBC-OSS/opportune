use opportune::Engine;
use opportune::model::{Goal, Participant, Request, Requirement, Span, Term};
use opportune::presets;
use opportune::time::{
    Date, Duration, Instant, RuleZone, TimeOfDay, Weekday, Weekdays, ZoneName,
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let zone = ZoneName::new("Europe/London")?;
    let now = Instant::from_utc(Date::new(2027, 6, 14)?, TimeOfDay::new(7, 0)?)?;
    let until = Instant::from_utc(Date::new(2027, 6, 19)?, TimeOfDay::new(0, 0)?)?;
    let mut request = Request::new(
        now,
        Goal::single(
            zone.clone(),
            Span::new(now, until)?,
            Duration::from_minutes(30),
            Requirement::new(vec![Term::person("rich")]),
        ),
    );
    request
        .participants
        .push(Participant::new("rich", zone).with_calendar(Vec::new()));
    request.rules.push(presets::business_hours(
        RuleZone::Participant,
        Weekdays::range(Weekday::Monday, Weekday::Friday),
        TimeOfDay::new(9, 0)?,
        TimeOfDay::new(17, 0)?,
    )?);
    request
        .rules
        .push(presets::buffers(Duration::from_minutes(10), Duration::from_minutes(10)));
    request
        .rules
        .push(presets::min_notice(Duration::from_hours_minutes(4, 0)));

    let response = Engine::builder().build()?.find_availability(&request)?;
    assert!(!response.offers.is_empty());
    Ok(())
}
