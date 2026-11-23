use opportune::{
    Engine,
    model::{Goal, Participant, Request, Requirement, Span, Term},
};
use opportune::time::{Date, Duration, Instant, TimeOfDay, ZoneName};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let zone = ZoneName::utc();
    let now = Instant::from_utc(Date::new(2027, 6, 14)?, TimeOfDay::new(0, 0)?)?;
    let until = Instant::from_utc(Date::new(2027, 6, 15)?, TimeOfDay::new(0, 0)?)?;
    let window = Span::new(now, until)?;
    let mut request = Request::new(
        now,
        Goal::single(
            zone.clone(),
            window,
            Duration::from_minutes(30),
            Requirement::new(vec![Term::person("ana")]),
        ),
    );
    request
        .participants
        .push(Participant::new("ana", zone).with_calendar(Vec::new()));

    let response = opportune::find_availability(&request)?;
    assert_eq!(response.offers.len(), 48);
    assert_eq!(
        response.meta.tzdb,
        Engine::builder().build()?.time_zone_database().version()
    );
    Ok(())
}
