//! `Rule::unless` and its participant-side twins `and_who` and `except_who`: a
//! broad tier loosened for the tag, kind or ID it excepts.
//!
//! The composition is asserted on the selector it builds, and the loosening is
//! asserted through a real engine, so a rule that reads as an exception but still
//! intersects with the tier it was meant to leave is red here.

#![cfg(feature = "bundled-tzdb")]

use opportune::Engine;
use opportune::model::{
    Block, BlockStatus, Explain, Goal, Part, Participant, ParticipantKind, Request, Requirement,
    Rule, Span, StatusEffect, StatusPolicyEntry, Term,
};
use opportune::presets;
use opportune::reasons::{ReasonCode, ReasonParams};
use opportune::tags::{Role, Selector, Tag};
use opportune::time::{
    Date, Duration, Instant, LocalResolution, RuleZone, TimeOfDay, WallEnd, Weekday, Weekdays,
    Zone, ZoneName,
};

/// Monday 14 June 2027, a working day for the fixture's global hours.
const MONDAY: (i16, i8, i8) = (2027, 6, 14);
/// Saturday 19 June 2027, a day the fixture's global hours close.
const SATURDAY: (i16, i8, i8) = (2027, 6, 19);

fn engine() -> Engine {
    Engine::builder()
        .build()
        .expect("the bundled database builds")
}

fn tag(path: &str) -> Tag {
    Tag::new(path).expect("a fixture tag is a valid tag")
}

fn time(hour: u8, minute: u8) -> TimeOfDay {
    TimeOfDay::new(hour, minute).expect("a fixture time is a time of day")
}

/// The UTC instant of a local date-time.
fn utc(day: (i16, i8, i8), hour: u8, minute: u8) -> Instant {
    let LocalResolution::Unique(instant) = Zone::utc()
        .resolve_local(
            Date::new(day.0, day.1, day.2).expect("a fixture date is a real date"),
            time(hour, minute),
        )
        .expect("the fixture instant is inside the supported range")
    else {
        panic!("UTC has no transitions");
    };
    instant
}

fn span(start: Instant, end: Instant) -> Span {
    Span::new(start, end).expect("the fixture span is non-empty")
}

/// The exception tag a layered policy loosens its broad tiers for.
fn vip() -> Selector {
    Selector::tag(tag("vip"))
}

/// The human-only clause a participant-side exception narrows a host rule with.
fn people() -> Selector {
    Selector::kind(ParticipantKind::Person)
}

/// A 30-minute, one-host request over `candidates`, whose part carries `tags`.
fn request(candidates: Vec<Instant>, rules: Vec<Rule>, part_tags: &[&str]) -> Request {
    let mut participant = Participant::new("ana", ZoneName::new("UTC").unwrap());
    participant.calendar = Some(Vec::new());
    let mut part = Part::new(
        Duration::from_minutes(30),
        Requirement::new(vec![Term::person("ana")]),
    );
    part.tags = part_tags.iter().map(|path| tag(path)).collect();
    let goal = Goal::new(
        ZoneName::new("UTC").unwrap(),
        span(utc(MONDAY, 0, 0), utc(SATURDAY, 23, 0)),
        vec![part],
    )
    .with_candidates(candidates);
    let mut request = Request::new(utc(MONDAY, 0, 0), goal);
    request.participants = vec![participant];
    request.rules = rules;
    request.explain = Some(Explain { limit: 500 });
    request
}

/// The starts the answer offers, in the order it offers them.
fn offered(response: &opportune::model::Response) -> Vec<Instant> {
    response
        .offers
        .iter()
        .map(|offer| offer.start().expect("a version 1 offer has a start"))
        .collect()
}

/// The code and rule of every reason the answer gives for excluding `start`.
fn reasons_at(
    response: &opportune::model::Response,
    start: Instant,
) -> Vec<(ReasonCode, Option<String>)> {
    response
        .excluded
        .iter()
        .filter(|excluded| excluded.start == start)
        .flat_map(|excluded| &excluded.reasons)
        .map(|reason| (reason.code, reason.rule.clone()))
        .collect()
}

/// An unconditional rule becomes the exception alone; a rule that is already
/// scoped keeps its scope and adds the exception beside it. Nothing but `when`
/// changes.
#[test]
fn unless_excludes_its_selector_from_an_unconditional_and_a_scoped_rule() {
    let hours = presets::business_hours(
        RuleZone::Participant,
        Weekdays::range(Weekday::Monday, Weekday::Friday),
        time(9, 0),
        WallEnd::Time(time(17, 0)),
    )
    .unwrap();

    let unconditional = hours.clone().unless(vip());
    assert_eq!(unconditional.when, Selector::not(vip()));
    assert_eq!(unconditional.who, hours.who, "`who` is untouched");
    assert_eq!(unconditional.id, hours.id, "the ID is untouched");
    assert_eq!(unconditional.kind, hours.kind, "the kind is untouched");

    let interview = Selector::tag(tag("interview"));
    let scoped = hours.clone().with_when(interview.clone()).unless(vip());
    assert_eq!(
        scoped.when,
        Selector::all([interview, Selector::not(vip())]),
        "a scoped rule keeps its scope and adds the exception"
    );

    // Chaining a second exception nests a new `all` rather than dropping or
    // replacing the first, so each call only ever narrows.
    let twice = scoped.clone().unless(Selector::tag(tag("board")));
    assert_eq!(
        twice.when,
        Selector::all([scoped.when, Selector::not(Selector::tag(tag("board")))])
    );
}

/// The global and event-group windows are lowered away for the excepted tag
/// instead of intersecting with each other to nothing: an ordinary interview only
/// gets the overlap of both tiers, while the excepted one is offered every
/// candidate in the search window.
#[test]
fn the_excepted_tag_is_exempt_from_the_global_and_group_windows() {
    let global = presets::business_hours(
        RuleZone::Event,
        Weekdays::range(Weekday::Monday, Weekday::Friday),
        time(9, 0),
        WallEnd::Time(time(17, 0)),
    )
    .unwrap()
    .unless(vip());
    let interviews = presets::business_hours(
        RuleZone::Event,
        Weekdays::range(Weekday::Monday, Weekday::Friday),
        time(9, 0),
        WallEnd::Time(time(12, 0)),
    )
    .unwrap()
    .with_id("interview_hours")
    .with_when(Selector::tag(tag("interview")))
    .unless(vip());
    let inside = utc(MONDAY, 10, 0);
    let after_hours = utc(MONDAY, 14, 0);
    let weekend = utc(SATURDAY, 10, 0);
    let candidates = vec![inside, after_hours, weekend];
    let rules = vec![global, interviews];
    let engine = engine();

    let ordinary = engine
        .find_availability(&request(candidates.clone(), rules.clone(), &["interview"]))
        .unwrap();
    assert_eq!(
        offered(&ordinary),
        [inside],
        "an ordinary interview gets only the hours both tiers are open"
    );
    assert_eq!(
        reasons_at(&ordinary, after_hours),
        [(
            ReasonCode::OutsideWindow,
            Some("interview_hours".to_owned())
        )],
        "the group tier is what closes the afternoon, not the global one"
    );

    // The group tier keeps its own scope through the exception: a booking it does
    // not select is the global tier's to exclude, which is what makes the exception
    // an easing of a tier rather than a replacement for the rule's `when`.
    let unrelated = engine
        .find_availability(&request(
            vec![after_hours, weekend],
            rules.clone(),
            &["demo"],
        ))
        .unwrap();
    assert_eq!(
        offered(&unrelated),
        [after_hours],
        "the group window still applies to interviews only, so the global hours are all a demo booking has"
    );
    assert_eq!(
        reasons_at(&unrelated, weekend),
        [(ReasonCode::OutsideWindow, Some("business_hours".to_owned()))],
        "the global tier is the one that closes the weekend for it"
    );

    let excepted = engine
        .find_availability(&request(candidates, rules, &["interview", "vip"]))
        .unwrap();
    assert_eq!(
        offered(&excepted),
        [inside, after_hours, weekend],
        "the excepted booking is exempt from both windows, so the search window is all it has"
    );
}

/// A daily cap loosened for the excepted tag stops counting it: the ordinary
/// booking is at the limit while the excepted one fits in the same bucket.
#[test]
fn the_excepted_tag_is_exempt_from_the_cap_the_global_tier_sets() {
    let booked = |hour: u8| {
        let mut block = Block::new(span(utc(MONDAY, hour, 0), utc(MONDAY, hour, 30)));
        block.tags = vec![tag("interview")];
        block
    };
    let mut participant = Participant::new("ana", ZoneName::new("UTC").unwrap());
    participant.calendar = Some(vec![booked(9), booked(10)]);
    let rules = vec![presets::cap_per_day(Selector::tag(tag("interview")), 2).unless(vip())];
    let candidate = utc(MONDAY, 14, 0);
    let engine = engine();

    let capped = |part_tags: &[&str]| {
        let mut request = self::request(vec![candidate], rules.clone(), part_tags);
        request.participants = vec![participant.clone()];
        engine.find_availability(&request).unwrap()
    };

    let ordinary = capped(&["interview"]);
    assert!(
        offered(&ordinary).is_empty(),
        "the day's two interviews are its limit"
    );
    let excluded = ordinary
        .excluded
        .iter()
        .find(|excluded| excluded.start == candidate)
        .expect("the candidate is excluded");
    let reached = excluded
        .reasons
        .iter()
        .find(|reason| reason.code == ReasonCode::CapReached)
        .expect("the ordinary booking is excluded by the cap");
    assert_eq!(reached.rule.as_deref(), Some("cap_per_day"));
    assert_eq!(
        reached.params,
        Some(ReasonParams::CapReached {
            used: 2,
            limit: 2,
            bucket: "2027-06-14".to_owned(),
        })
    );

    let excepted = capped(&["interview", "vip"]);
    assert_eq!(
        offered(&excepted),
        [candidate],
        "the cap's exception leaves its bucket unspent for the excepted booking"
    );
}

/// An exception takes a booking out of the cap's judgement, not out of its
/// bucket: the event the cap does not judge still carries the tags the cap's
/// `count` names, so it spends the budget every other booking is measured
/// against. A caller who wants an excepted event to stop counting narrows
/// `count` as well as `when`.
#[test]
fn an_excepted_booking_still_counts_toward_the_cap_it_escapes() {
    let booked = |hour: u8| {
        let mut block = Block::new(span(utc(MONDAY, hour, 0), utc(MONDAY, hour, 30)));
        block.tags = vec![tag("interview"), tag("vip")];
        block
    };
    let mut participant = Participant::new("ana", ZoneName::new("UTC").unwrap());
    participant.calendar = Some(vec![booked(9), booked(10)]);
    let rules = vec![presets::cap_per_day(Selector::tag(tag("interview")), 2).unless(vip())];
    let candidate = utc(MONDAY, 14, 0);
    let engine = engine();

    let mut ordinary = request(vec![candidate], rules, &["interview"]);
    ordinary.participants = vec![participant];
    let ordinary = engine.find_availability(&ordinary).unwrap();

    assert!(
        offered(&ordinary).is_empty(),
        "the two excepted interviews the cap never judges still fill its bucket"
    );
    assert_eq!(
        reasons_at(&ordinary, candidate),
        [(ReasonCode::CapReached, Some("cap_per_day".to_owned()))],
        "the bucket the ordinary booking is judged against is the two it never judges"
    );
}

/// A status policy the caller scopes with `unless` applies to the bookings the
/// exception leaves and to no others, which is what the preset's own rustdoc tells
/// a caller to do with it.
#[test]
fn the_status_policy_preset_is_scoped_by_the_exception_it_excludes() {
    let mut participant = Participant::new("ana", ZoneName::new("UTC").unwrap());
    let mut tentative = Block::new(span(utc(MONDAY, 10, 0), utc(MONDAY, 11, 0)));
    tentative.status = BlockStatus::Tentative;
    participant.calendar = Some(vec![tentative]);
    let rules = vec![
        presets::status_policy(vec![StatusPolicyEntry::status(
            BlockStatus::Tentative,
            StatusEffect::Ignore,
        )])
        .unless(vip()),
    ];
    let candidate = utc(MONDAY, 10, 30);
    let engine = engine();

    let asks = |part_tags: &[&str]| {
        let mut request = request(vec![candidate], rules.clone(), part_tags);
        request.participants = vec![participant.clone()];
        engine.find_availability(&request).unwrap()
    };

    assert_eq!(
        offered(&asks(&["interview"])),
        [candidate],
        "the policy's exception does not cover an ordinary booking"
    );
    assert!(
        offered(&asks(&["interview", "vip"])).is_empty(),
        "the excepted booking keeps the tentative block's own reading"
    );
}

/// `and_who` intersects with the rule's `who` instead of replacing it, and a chain
/// of narrowing calls stays one flat `all`, so the participant-side twins of
/// `unless` cannot silently drop the default `Role::Host`.
#[test]
fn and_who_and_except_who_narrow_the_default_host_selector_without_nesting() {
    let hours = presets::business_hours(
        RuleZone::Participant,
        Weekdays::range(Weekday::Monday, Weekday::Friday),
        time(9, 0),
        WallEnd::Time(time(17, 0)),
    )
    .unwrap();
    assert_eq!(hours.who, Selector::role(Role::Host));

    let narrowed = hours.clone().and_who(people());
    assert_eq!(
        narrowed.who,
        Selector::all([Selector::role(Role::Host), people()]),
        "the default host clause survives the narrowing"
    );

    let exempt = narrowed.clone().except_who(Selector::ids(["ceo"]));
    assert_eq!(
        exempt.who,
        Selector::all([
            Selector::role(Role::Host),
            people(),
            Selector::not(Selector::ids(["ceo"])),
        ]),
        "a chained exception appends to the same `all` instead of nesting one inside another"
    );
    assert_eq!(exempt.when, hours.when, "`when` is untouched");
    assert_eq!(exempt.kind, hours.kind, "the kind is untouched");
    assert_eq!(exempt.id, hours.id, "the ID is untouched");

    // An unconditional `who` takes the selector alone, and a single clause becomes
    // the first two of an `all`.
    let buffers = || presets::buffers(Duration::from_minutes(10), Duration::from_minutes(10));
    assert_eq!(
        buffers().with_who(Selector::True).and_who(people()).who,
        people()
    );
    assert_eq!(
        buffers()
            .with_who(Selector::ids(["ana"]))
            .and_who(people())
            .who,
        Selector::all([Selector::ids(["ana"]), people()])
    );
}

/// `except_who` exempts a named host from the rule's `who` while every other host
/// keeps being judged, and it never reaches an attendee because the default
/// `Role::Host` clause is still there.
#[test]
fn except_who_exempts_the_named_host_and_leaves_attendees_alone() {
    let meeting = |id: &str, role: Role| {
        let mut held = Block::new(span(utc(MONDAY, 9, 0), utc(MONDAY, 9, 30)));
        held.tags = vec![tag("meeting")];
        Participant::new(id, ZoneName::new("UTC").unwrap())
            .with_role(role)
            .with_calendar(vec![held])
    };
    let free =
        |id: &str| Participant::new(id, ZoneName::new("UTC").unwrap()).with_calendar(Vec::new());
    let candidate = utc(MONDAY, 14, 0);
    let cap = || {
        presets::cap_per_day(Selector::Tag(tag("meeting")), 1).except_who(Selector::ids(["ceo"]))
    };
    let asks = |host: &str, participants: Vec<Participant>| {
        let mut request = request(vec![candidate], vec![cap()], &["meeting"]);
        request.goal.parts[0].requirement = Requirement::new(vec![Term::person(host)]);
        request.participants = participants;
        engine().find_availability(&request).unwrap()
    };

    // An ordinary host already holds Monday's one meeting, so the cap reaches her.
    // `ceo` rides along because `who` names her, not because she is judged here.
    let ana = asks("ana", vec![meeting("ana", Role::Host), free("ceo")]);
    assert!(
        offered(&ana).is_empty(),
        "the cap still binds a host it does not except"
    );
    assert_eq!(
        reasons_at(&ana, candidate),
        [(ReasonCode::CapReached, Some("cap_per_day".to_owned()))],
    );

    // The excepted host is the same shape, and the cap does not judge her.
    assert_eq!(
        offered(&asks("ceo", vec![meeting("ceo", Role::Host)])),
        [candidate],
        "`except_who` narrows `who` instead of replacing it, so the exempt host fits"
    );

    // A free host carrying an attendee whose day is full: keeping the default host
    // clause is what leaves the attendee out of the cap's judgement.
    assert_eq!(
        offered(&asks(
            "ana",
            vec![free("ana"), meeting("tom", Role::Attendee), free("ceo")]
        )),
        [candidate],
        "the default `Role::Host` clause keeps an attendee out of the cap"
    );

    // Reaching people by replacing `who` instead drags the attendee in, which is
    // the footgun a host rule's `except_who` avoids.
    let people_cap = presets::cap_per_day(Selector::Tag(tag("meeting")), 1)
        .with_who(people())
        .except_who(Selector::ids(["ceo"]));
    let mut request = request(vec![candidate], vec![people_cap], &["meeting"]);
    request.goal.parts[0].requirement = Requirement::new(vec![Term::person("ana")]);
    request.participants = vec![free("ana"), meeting("tom", Role::Attendee), free("ceo")];
    assert!(
        offered(&engine().find_availability(&request).unwrap()).is_empty(),
        "a cap whose `who` is replaced with `kind: person` reaches the attendee"
    );
}
