// An internal team plus a conference room. The room is a `ParticipantKind::Resource`
// participant, so the human-only rules — hours, rest and the daily cap — are scoped
// with `Selector::kind(ParticipantKind::Person)` and never reach it, while its own
// tentative hold keeps blocking. The second request is the same booking with one
// room instead of two: optional-room tiering is a second request, not a flag.

use opportune::Engine;
use opportune::model::{
    Block, BlockStatus, Explain, Goal, Part, Participant, ParticipantKind, Request, Requirement,
    Rule, Span, StatusEffect, StatusPolicyEntry, Term,
};
use opportune::presets;
use opportune::reasons::ReasonCode;
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

/// A person participant, tagged, with the given calendar.
fn person(id: &str, tags: &[&str], blocks: Vec<Block>) -> Participant {
    let mut participant =
        Participant::new(id, ZoneName::new("UTC").expect("UTC is a zone name")).with_calendar(blocks);
    participant.tags = tags.iter().map(|path| tag(path)).collect();
    participant
}

fn room(id: &str, blocks: Vec<Block>) -> Participant {
    person(id, &["kind/room"], blocks).with_kind(ParticipantKind::Resource)
}

/// The block from `[from, until)` UTC on Monday 14 June 2027, with `status`.
fn held(from: (u8, u8), until: (u8, u8), status: BlockStatus) -> Block {
    let mut block = Block::new(
        Span::new(utc((2027, 6, 14), from), utc((2027, 6, 14), until)).expect("a non-empty span"),
    );
    block.status = status;
    block
}

fn people() -> Selector {
    Selector::kind(ParticipantKind::Person)
}

/// The rules an internal meeting runs under: the team's hours, rest and daily cap
/// are statements about people, and the room's own holds are not.
fn rules(scope_status_policy: bool) -> Result<Vec<Rule>, Box<dyn std::error::Error>> {
    let status_policy =
        presets::status_policy(vec![StatusPolicyEntry::status(
            BlockStatus::Tentative,
            StatusEffect::Ignore,
        )])
        .with_id("ignore_tentative");
    Ok(vec![
        presets::business_hours(
            RuleZone::Participant,
            Weekdays::range(Weekday::Monday, Weekday::Friday),
            TimeOfDay::new(9, 0)?,
            WallEnd::Time(TimeOfDay::new(17, 0)?),
        )?
        .with_who(people()),
        presets::rest(TimeOfDay::new(18, 0)?, TimeOfDay::new(8, 0)?).with_who(people()),
        presets::cap_per_day(Selector::tag(tag("meeting")), 3).with_who(people()),
        if scope_status_policy {
            status_policy.with_who(people())
        } else {
            status_policy
        },
    ])
}

/// The 60-minute team meeting at `candidates` that needs `rooms` conference rooms
/// as well as ana and bo.
fn request(
    candidates: Vec<Instant>,
    rooms: u32,
    scope_status_policy: bool,
) -> Result<Request, Box<dyn std::error::Error>> {
    let requirement = Requirement::new(vec![
        Term::person("ana"),
        Term::person("bo"),
        Term::at_least(rooms, Selector::tag(tag("kind/room"))),
    ]);
    let mut part = Part::new(Duration::from_minutes(60), requirement);
    part.tags = vec![tag("meeting")];
    let goal = Goal::new(
        ZoneName::new("UTC")?,
        Span::new(utc((2027, 6, 14), (0, 0)), utc((2027, 6, 15), (0, 0)))?,
        vec![part],
    )
    .with_candidates(candidates);

    let mut request = Request::new(utc((2027, 6, 14), (8, 0)), goal);
    request.participants = vec![
        person("ana", &["team"], vec![]),
        person("bo", &["team"], vec![]),
        // Held tentatively from 10:00 to 11:00: taken, whatever a person's own
        // tentative invite means.
        room("room-4", vec![held((10, 0), (11, 0), BlockStatus::Tentative)]),
        // Booked solid all day, so the two-room tier has nothing to offer.
        room("room-9", vec![held((8, 0), (18, 0), BlockStatus::Busy)]),
    ];
    request.rules = rules(scope_status_policy)?;
    request.explain = Some(Explain::new(100));
    Ok(request)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let engine = Engine::builder().build()?;
    let candidates = vec![
        utc((2027, 6, 14), (9, 0)),
        utc((2027, 6, 14), (10, 0)),
        utc((2027, 6, 14), (11, 0)),
    ];
    let starts = |offers: &[opportune::Offer]| -> Vec<Instant> {
        offers
            .iter()
            .filter_map(opportune::Offer::start)
            .collect()
    };

    // Tier one: both rooms. room-9 is booked all day, so the request has no
    // answer at all — the trigger a caller uses to fall back to one room.
    let both = engine.find_availability(&request(candidates.clone(), 2, true)?)?;
    assert!(both.offers.is_empty());

    // Tier two: one room. The tentative hold at 10:00 still blocks, because the
    // policy that ignores tentative holds is scoped to people.
    let one = engine.find_availability(&request(candidates.clone(), 1, true)?)?;
    assert_eq!(
        starts(&one.offers),
        [utc((2027, 6, 14), (9, 0)), utc((2027, 6, 14), (11, 0))]
    );
    let ten = one
        .excluded
        .iter()
        .find(|excluded| excluded.start == utc((2027, 6, 14), (10, 0)))
        .ok_or("the held hour is excluded")?;
    let blocked = ten
        .reasons
        .iter()
        .find(|reason| reason.code == ReasonCode::Blocked)
        .ok_or("the room's hold is the reason")?;
    assert_eq!(blocked.participant.as_deref(), Some("room-4"));

    // The scoping is what protects the room. Unscoped, the same policy reads the
    // room's hold as free and offers the hour a second meeting would double-book,
    // and `validate` warns about exactly that.
    let unscoped_request = request(candidates.clone(), 1, false)?;
    let unscoped = engine.find_availability(&unscoped_request)?;
    assert_eq!(
        starts(&unscoped.offers),
        [
            utc((2027, 6, 14), (9, 0)),
            utc((2027, 6, 14), (10, 0)),
            utc((2027, 6, 14), (11, 0)),
        ]
    );
    let warning = engine
        .validate(&unscoped_request)
        .warnings
        .into_iter()
        .find(|warning| warning.code() == "status_policy_ignores_resource")
        .ok_or("the unscoped policy is reported")?;
    assert_eq!(warning.path(), Some("/rules/3"));

    // Both remaining offers fill the room term with room-4: room-9 is booked all
    // day, so the room the requirement lands on is the one whose hold closed 10:00.
    let assigned = one
        .offers
        .iter()
        .flat_map(opportune::Offer::assigned_hosts)
        .collect::<Vec<_>>();
    assert_eq!(assigned, ["ana", "bo", "room-4", "ana", "bo", "room-4"]);
    Ok(())
}
