// A true panel plus a reschedule: a hiring manager and two of three engineers,
// weighted so the assignment prefers who the dispatcher wants, for a candidate
// whose stated availability is the only time they can be booked. The interview is
// then moved with `Request::exclude_event_keys`, which takes the booking being
// moved out of the cap it was consuming — so the interviewer who is already at
// their daily cap can keep the panel.

use std::collections::BTreeMap;

use opportune::Engine;
use opportune::model::{
    Block, Explain, Goal, Part, Participant, Proposal, ProposalEvent, Request, Requirement, Span,
    Term,
};
use opportune::presets;
use opportune::reasons::ReasonCode;
use opportune::tags::{Selector, Tag};
use opportune::time::{Date, Duration, Instant, RuleZone, TimeOfDay, WallEnd, Weekday, Weekdays, ZoneName};

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

/// An interview already on the calendar, over `[from, until)` UTC on Tuesday
/// 15 June 2027, identified by `key`.
fn booked(from: (u8, u8), until: (u8, u8), key: &str) -> Block {
    let mut block = Block::new(
        Span::new(utc((2027, 6, 15), from), utc((2027, 6, 15), until)).expect("a non-empty span"),
    );
    block.tags = vec![tag("interview")];
    block.event_key = Some(key.to_owned());
    block
}

fn host(id: &str, tags: &[&str], blocks: Vec<Block>) -> Participant {
    let mut participant = Participant::new(id, ZoneName::new("UTC").expect("UTC is a zone name"))
        .with_calendar(blocks);
    participant.tags = tags.iter().map(|path| tag(path)).collect();
    participant
}

/// The one-hour panel interview at `candidates`, with `exclude` naming the event
/// being moved — empty for a fresh booking.
fn request(
    candidates: Vec<Instant>,
    exclude: &[&str],
) -> Result<Request, Box<dyn std::error::Error>> {
    let requirement = Requirement::new(vec![
        Term::person("dana"),
        Term::at_least(2, Selector::tag(tag("eng/frontend"))),
    ]);
    let part = Part {
        id: "panel".to_owned(),
        tags: vec![tag("interview")],
        ..Part::new(Duration::from_minutes(60), requirement)
    };
    let goal = Goal::new(
        ZoneName::new("UTC")?,
        Span::new(utc((2027, 6, 15), (8, 0)), utc((2027, 6, 15), (18, 0)))?,
        vec![part],
    )
    .with_candidates(candidates);

    // The candidate states the only hours they can attend; `available_only` hands
    // back the attendee and the window that holds them to it.
    let (candidate, availability) = presets::available_only(
        "candidate",
        &[Span::new(
            utc((2027, 6, 15), (11, 0)),
            utc((2027, 6, 15), (16, 0)),
        )?],
    )?;

    let mut request = Request::new(utc((2027, 6, 15), (8, 0)), goal);
    request.participants = vec![
        host("dana", &["hiring_manager"], vec![]),
        host("ana", &["eng/frontend"], vec![]),
        host("bo", &["eng/frontend"], vec![]),
        // Ana and cy are the interviewers the dispatcher wants on this panel.
        host(
            "cy",
            &["eng/frontend"],
            vec![booked((9, 0), (10, 0), "evt_prev"), booked((13, 0), (14, 0), "evt_move")],
        ),
        candidate,
    ];
    request.weights = BTreeMap::from([
        ("ana".to_owned(), 3),
        ("bo".to_owned(), 1),
        ("cy".to_owned(), 2),
    ]);
    request.rules = vec![
        presets::business_hours(
            RuleZone::Participant,
            Weekdays::range(Weekday::Monday, Weekday::Friday),
            TimeOfDay::new(9, 0)?,
            WallEnd::Time(TimeOfDay::new(17, 0)?),
        )?,
        presets::cap_per_day(Selector::tag(tag("interview")), 2).with_id("daily_limit"),
        availability,
    ];
    request.explain = Some(Explain::new(100));
    request.exclude_event_keys = exclude.iter().map(|key| (*key).to_owned()).collect();
    Ok(request)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let engine = Engine::builder().build()?;
    let candidates = vec![utc((2027, 6, 15), (10, 0)), utc((2027, 6, 15), (11, 0))];
    let starts = |offers: &[opportune::Offer]| -> Vec<Instant> {
        offers
            .iter()
            .filter_map(opportune::Offer::start)
            .collect()
    };

    // Before the reschedule: cy is at their daily cap of two, counting the very
    // interview being moved, so the panel is filled by the next two engineers and
    // the candidate's unavailable hour is excluded.
    let at_capacity = request(candidates.clone(), &[])?;
    let before = engine.find_availability(&at_capacity)?;
    assert_eq!(starts(&before.offers), [utc((2027, 6, 15), (11, 0))]);
    assert_eq!(before.offers[0].events[0].eligible[0], ["dana"]);
    assert_eq!(before.offers[0].events[0].eligible[1], ["ana", "bo"]);
    assert_eq!(before.offers[0].assigned_hosts(), ["dana", "ana", "bo"]);
    let unavailable = before
        .excluded
        .iter()
        .find(|excluded| excluded.start == utc((2027, 6, 15), (10, 0)))
        .ok_or("the candidate's unavailable hour is excluded")?;
    assert_eq!(
        (
            unavailable.reasons[0].code,
            unavailable.reasons[0].rule.as_deref()
        ),
        (
            ReasonCode::OutsideWindow,
            Some("available_only:candidate")
        )
    );

    // A proposal that keeps cy — the interviewer who already knows the candidate —
    // is refused while the moved interview still counts.
    let keep_cy = Proposal {
        events: vec![ProposalEvent {
            part: "panel".to_owned(),
            start: utc((2027, 6, 15), (11, 0)),
            assignment: vec![
                vec!["dana".to_owned()],
                vec!["ana".to_owned(), "cy".to_owned()],
            ],
        }],
    };
    let refused = engine.check_slot(&at_capacity, &keep_cy)?;
    assert!(!refused.valid);
    let capped = refused
        .reasons
        .iter()
        .find(|reason| reason.code == ReasonCode::CapReached)
        .ok_or("the cap is the reason")?;
    assert_eq!(capped.participant.as_deref(), Some("cy"));
    assert_eq!(capped.rule.as_deref(), Some("daily_limit"));

    // Naming the event the caller is moving takes it out of the cap. cy is under
    // the limit again, so the weights choose ana and cy, and the moved slot
    // confirms.
    let moving = request(candidates, &["evt_move"])?;
    let after = engine.find_availability(&moving)?;
    assert_eq!(starts(&after.offers), [utc((2027, 6, 15), (11, 0))]);
    assert_eq!(after.offers[0].events[0].eligible[1], ["ana", "bo", "cy"]);
    assert_eq!(after.offers[0].assigned_hosts(), ["dana", "ana", "cy"]);
    let confirmed = engine.check_slot(&moving, &after.offers[0].to_proposal())?;
    assert!(confirmed.valid);
    assert!(confirmed.reasons.is_empty());
    Ok(())
}
