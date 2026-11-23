// A field-service visit with travel holds, confirmed at booking time.

use opportune::Engine;
use opportune::model::{
    Block, Goal, Participant, Part, Proposal, ProposalEvent, Request, Requirement, Span, Term,
    Travel,
};
use opportune::presets;
use opportune::reasons::ReasonCode;
use opportune::tags::Tag;
use opportune::time::{
    Date, Duration, Instant, RuleZone, TimeOfDay, WallEnd, Weekday, Weekdays, ZoneName,
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
    let mut prior_job = Block::new(Span::new(
        utc((2027, 3, 8), (18, 0)),
        utc((2027, 3, 8), (20, 0)),
    )?);
    prior_job.tags = vec![Tag::new("job/plumbing")?];
    prior_job.event_key = Some("job-101".to_owned());
    prior_job.travel_after = Duration::from_minutes(20);

    // The visit needs travel either side. The customer is at home, so their own
    // travel is zero even though the engineer's is not.
    let visit = Part {
        id: "visit".to_owned(),
        tags: vec![Tag::new("job/plumbing")?],
        travel: Travel::new(Duration::from_minutes(25), Duration::from_minutes(25))
            .with_participant("joe", Duration::ZERO, Duration::ZERO),
        ..Part::new(
            Duration::from_minutes(60),
            Requirement::new(vec![Term::person("bob")]),
        )
    };

    let mut goal = Goal::new(
        new_york.clone(),
        Span::new(utc((2027, 3, 8), (18, 0)), utc((2027, 3, 8), (23, 0)))?,
        vec![visit],
    );
    goal.candidates = Some(vec![utc((2027, 3, 8), (20, 54)), utc((2027, 3, 8), (20, 55))]);

    let mut request = Request::new(utc((2027, 3, 6), (12, 0)), goal);
    request.participants = vec![
        Participant::new("bob", new_york.clone()).with_calendar(vec![prior_job]),
        Participant::attendee("joe", new_york).with_calendar(vec![]),
    ];
    request.rules = vec![
        presets::business_hours(
            RuleZone::Participant,
            Weekdays::range(Weekday::Monday, Weekday::Saturday),
            TimeOfDay::new(8, 0)?,
            WallEnd::Time(TimeOfDay::new(18, 0)?),
        )?,
        presets::buffers(Duration::from_minutes(10), Duration::from_minutes(10)),
    ];

    let engine = Engine::builder().build()?;
    let response = engine.find_availability(&request)?;

    // Only the later candidate clears the prior job's travel and the buffer.
    let offer = response.offers.first().ok_or("the visit is bookable")?;
    assert_eq!(offer.start(), Some(utc((2027, 3, 8), (20, 55))));
    let holds: Vec<(&str, Instant, Instant)> = offer
        .reserve()
        .into_iter()
        .map(|reserve| (reserve.participant.as_str(), reserve.span.start(), reserve.span.end()))
        .collect();
    assert_eq!(
        holds,
        vec![
            ("bob", utc((2027, 3, 8), (20, 30)), utc((2027, 3, 8), (20, 55))),
            ("bob", utc((2027, 3, 8), (21, 55)), utc((2027, 3, 8), (22, 20))),
        ]
    );

    // Confirming it at booking time preserves the holds.
    let confirmed = engine.check_slot(&request, &offer.to_proposal())?;
    assert!(confirmed.valid);
    assert_eq!(confirmed.reserve.iter().collect::<Vec<_>>(), offer.reserve());

    // A start inside the prior job's travel is refused, naming the event.
    let too_soon = Proposal {
        events: vec![ProposalEvent {
            part: "visit".to_owned(),
            start: utc((2027, 3, 8), (20, 15)),
            assignment: vec![vec!["bob".to_owned()]],
        }],
    };
    let refused = engine.check_slot(&request, &too_soon)?;
    assert!(!refused.valid);
    let reason = refused.reasons.first().ok_or("the slot fails for a reason")?;
    assert_eq!(reason.code, ReasonCode::Blocked);
    assert_eq!(reason.event_key.as_deref(), Some("job-101"));
    Ok(())
}
