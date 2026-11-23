// An interview panel under a pooled weekly cap, a per-interviewer
// daily cap, and a weekly minutes cap.

use opportune::Engine;
use opportune::model::{
    Block, CapPer, Explain, Goal, Participant, Part, Request, Requirement, Span, Term,
};
use opportune::presets;
use opportune::reasons::{ReasonCode, ReasonParams};
use opportune::tags::{Selector, Tag};
use opportune::time::{
    Date, Duration, Instant, RuleZone, TimeOfDay, WallEnd, Weekday, Weekdays, ZoneName,
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

fn span(start: Instant, end: Instant) -> Span {
    Span::new(start, end).expect("a non-empty span")
}

fn booked(start: Instant, end: Instant, key: &str) -> Block {
    let mut block = Block::new(span(start, end));
    block.tags = vec![tag("interview")];
    block.event_key = Some(key.to_owned());
    block
}

fn interviewer(id: &str, calendar: Vec<Block>) -> Participant {
    let mut host = Participant::new(id, ZoneName::new("UTC").expect("UTC is a zone name"));
    host.tags = vec![tag("interviewer")];
    host.calendar = Some(calendar);
    host
}

/// The three interviewers and what is already on their calendars this week.
fn panel() -> Vec<Participant> {
    vec![
        interviewer(
            "ana",
            vec![
                booked(utc((2027, 6, 14), (9, 0)), utc((2027, 6, 14), (10, 0)), "ana-1"),
                booked(utc((2027, 6, 14), (10, 0)), utc((2027, 6, 14), (11, 0)), "ana-2"),
            ],
        ),
        interviewer(
            "bo",
            vec![
                booked(utc((2027, 6, 14), (9, 0)), utc((2027, 6, 14), (10, 0)), "bo-1"),
                booked(utc((2027, 6, 15), (9, 0)), utc((2027, 6, 15), (12, 0)), "bo-2"),
            ],
        ),
        interviewer("cy", Vec::new()),
    ]
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let utc_zone = ZoneName::new("UTC")?;
    let part = Part {
        id: "interview".to_owned(),
        tags: vec![tag("interview")],
        ..Part::new(
            Duration::from_minutes(60),
            Requirement::new(vec![Term::at_least(1, Selector::tag(tag("interviewer")))]),
        )
    };

    let mut request = Request::new(
        utc((2027, 6, 13), (12, 0)),
        Goal::new(
            utc_zone,
            span(utc((2027, 6, 14), (0, 0)), utc((2027, 6, 15), (0, 0))),
            vec![part],
        ),
    );
    request.participants = panel();
    request.rules = vec![
        presets::business_hours(
            RuleZone::Participant,
            Weekdays::range(Weekday::Monday, Weekday::Friday),
            TimeOfDay::new(9, 0)?,
            WallEnd::Time(TimeOfDay::new(17, 0)?),
        )?,
        presets::cap_per_day(Selector::tag(tag("interview")), 2),
        presets::cap_per_week(
            Selector::tag(tag("interview")),
            8,
            CapPer::Pooled,
            RuleZone::Event,
        ),
        presets::cap_minutes_per_week(
            Selector::tag(tag("interview")),
            240,
            CapPer::Each,
            RuleZone::Participant,
        ),
    ];
    request.limit = Some(50);

    let engine = Engine::builder().build()?;
    let response = engine.find_availability(&request)?;

    // Ana is at her daily limit and Bo at his weekly minutes, so every Monday slot
    // goes to Cy, the only interviewer with room left.
    assert_eq!(response.offers.len(), 15);
    assert!(response.offers.iter().all(|offer| {
        offer.events[0].eligible == vec![vec!["cy".to_owned()]]
            && offer.events[0].assignment == vec![vec!["cy".to_owned()]]
    }));
    assert_eq!(
        response
            .dates
            .iter()
            .find(|count| count.date == Date::new(2027, 6, 14).expect("a date"))
            .map(|count| count.offers),
        Some(15)
    );

    // Lowering the pooled weekly cap to the four interviews already booked closes
    // the pool for the week, and the explanation names that cap and its bucket.
    let mut pooled = request.clone();
    pooled.rules.retain(|rule| rule.id != "cap_per_week");
    pooled.rules.push(presets::cap_per_week(
        Selector::tag(tag("interview")),
        4,
        CapPer::Pooled,
        RuleZone::Event,
    ));
    pooled.explain = Some(Explain { limit: 10 });
    pooled.goal.search_window = span(utc((2027, 6, 14), (9, 0)), utc((2027, 6, 14), (11, 0)));
    let closed = engine.find_availability(&pooled)?;
    assert!(closed.offers.is_empty());
    let entry = closed
        .excluded
        .iter()
        .find(|entry| entry.start == utc((2027, 6, 14), (9, 0)))
        .expect("the first business-hours start is excluded");
    let reason = entry
        .reasons
        .iter()
        .find(|reason| reason.rule.as_deref() == Some("cap_per_week"))
        .expect("the pooled cap is one of the reasons");
    assert_eq!(reason.code, ReasonCode::CapReached);
    assert_eq!(reason.participant.as_deref(), Some("cy"));
    assert_eq!(
        reason.params,
        Some(ReasonParams::CapReached {
            used: 4,
            limit: 4,
            bucket: "2027-06-14".to_owned(),
        })
    );
    Ok(())
}
