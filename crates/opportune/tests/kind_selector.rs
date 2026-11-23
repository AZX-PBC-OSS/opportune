//! A participant's kind is a first-class selector, and the human-only rules it
//! scopes away from a room, plus the guard that reports an ignoring status policy
//! reaching one.
//!
//! Every test builds a real request and runs it on a real engine, and each rule
//! kind is driven twice — once with the rule scoped to people and once without —
//! so the answer that changes is the scoping and not the fixture.

#![cfg(feature = "bundled-tzdb")]

use opportune::Engine;
use opportune::model::{
    Block, BlockStatus, Explain, Goal, Participant, ParticipantKind, Request, Requirement, Rule,
    RuleKind, Span, StatusEffect, StatusPolicy, StatusPolicyEntry, Term,
};
use opportune::presets;
use opportune::tags::{Role, Selector, Tag};
use opportune::time::{Date, Duration, Instant, LocalResolution, TimeOfDay, Zone, ZoneName};

/// Monday 14 June 2027, the fixture's first day.
const MONDAY: (i16, i8, i8) = (2027, 6, 14);
/// Tuesday 15 June 2027, the fixture's second day.
const TUESDAY: (i16, i8, i8) = (2027, 6, 15);

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

/// A 30-minute block over `[start, end)`.
fn block(start: Instant, end: Instant) -> Block {
    Block::new(span(start, end))
}

/// A participant in UTC with the given kind and blocks.
fn participant(id: &str, kind: ParticipantKind, blocks: Vec<Block>) -> Participant {
    Participant::new(id, ZoneName::new("UTC").expect("a zone name"))
        .with_kind(kind)
        .with_calendar(blocks)
}

/// A request for one 30-minute booking of `id`, at exactly `candidates`, with
/// `tags` on the part so a `when` or `count` selector has something to match.
fn request(
    search: Span,
    candidates: Vec<Instant>,
    participants: Vec<Participant>,
    rules: Vec<Rule>,
    id: &str,
    tags: &[&str],
) -> Request {
    let mut goal = Goal::single(
        ZoneName::new("UTC").expect("a zone name"),
        search,
        Duration::from_minutes(30),
        Requirement::new(vec![Term::person(id)]),
    );
    goal.candidates = Some(candidates);
    goal.parts[0].tags = tags.iter().map(|path| tag(path)).collect();
    let mut request = Request::new(utc(MONDAY, 0, 0), goal);
    request.participants = participants;
    request.rules = rules;
    request.explain = Some(Explain::new(10_000));
    request
}

/// The starts the engine offers, in offer order.
fn offered(request: &Request) -> Vec<Instant> {
    engine()
        .find_availability(request)
        .expect("the request is valid")
        .offers
        .iter()
        .map(|offer| offer.start().expect("an offer has an event"))
        .collect()
}

/// A room holding one block over `[start, end)` with `status`.
fn held_room(id: &str, start: Instant, end: Instant, status: BlockStatus) -> Participant {
    held(id, ParticipantKind::Resource, start, end, status)
}

/// A participant of `kind` holding one block over `[start, end)` with `status`.
fn held(
    id: &str,
    kind: ParticipantKind,
    start: Instant,
    end: Instant,
    status: BlockStatus,
) -> Participant {
    let mut hold = block(start, end);
    hold.status = status;
    participant(id, kind, vec![hold])
}

/// An `ignore` policy for tentative blocks: the default `who` is every host, which
/// is the shape that also reaches a room.
fn ignore_tentative() -> Rule {
    Rule::new(
        "ignore_tentative",
        RuleKind::StatusPolicy(StatusPolicy {
            entries: vec![StatusPolicyEntry::status(
                BlockStatus::Tentative,
                StatusEffect::Ignore,
            )],
        }),
    )
}

fn people() -> Selector {
    Selector::kind(ParticipantKind::Person)
}

#[test]
fn a_human_only_status_policy_still_lets_a_rooms_tentative_hold_block() {
    // The room is held tentatively from 10:00 to 11:00, so the only two starts a
    // 30-minute booking can take are the ones the hold decides.
    let room = |id: &str| {
        held_room(
            id,
            utc(MONDAY, 10, 0),
            utc(MONDAY, 11, 0),
            BlockStatus::Tentative,
        )
    };
    let booking = |rules: Vec<Rule>| {
        request(
            span(utc(MONDAY, 10, 0), utc(MONDAY, 11, 0)),
            vec![utc(MONDAY, 10, 0), utc(MONDAY, 10, 30)],
            vec![room("room-4")],
            rules,
            "room-4",
            &[],
        )
    };

    // Unscoped — the default `who` is every host — the policy drops the room's
    // tentative hold and both starts are offered.
    assert_eq!(
        offered(&booking(vec![ignore_tentative()])),
        [utc(MONDAY, 10, 0), utc(MONDAY, 10, 30)],
        "an unscoped ignore policy reads the room's hold as free"
    );

    // Scoped to people, the policy has nothing to say about a room, so the hold
    // blocks both starts.
    assert!(
        offered(&booking(vec![ignore_tentative().with_who(people())])).is_empty(),
        "a policy scoped to people must leave a room's tentative hold blocking"
    );

    // The scoping, not the policy, is what changed: the same room with no rule
    // behaves as the scoped policy does.
    assert!(offered(&booking(Vec::new())).is_empty());
}

#[test]
fn a_human_only_rest_rule_does_not_protect_a_rooms_early_morning() {
    // The room's own meeting runs to 23:30 on Monday, so a rest rule that applied
    // to it would keep Tuesday before 08:00 clear.
    let room = || {
        participant(
            "room-4",
            ParticipantKind::Resource,
            vec![block(utc(MONDAY, 22, 0), utc(MONDAY, 23, 30))],
        )
    };
    let booking = |rules: Vec<Rule>| {
        request(
            span(utc(TUESDAY, 7, 0), utc(TUESDAY, 7, 30)),
            vec![utc(TUESDAY, 7, 0)],
            vec![room()],
            rules,
            "room-4",
            &[],
        )
    };
    let rest = || presets::rest(time(22, 0), time(8, 0));

    // Unscoped, the rule protects the night after the room's own late meeting.
    assert!(
        offered(&booking(vec![rest()])).is_empty(),
        "an unscoped rest rule protects the night after a room's late meeting"
    );

    // Scoped to people, the room's calendar is not the rule's business.
    assert_eq!(
        offered(&booking(vec![rest().with_who(people())])),
        [utc(TUESDAY, 7, 0)],
        "a rest rule scoped to people must not protect a room's early morning"
    );
}

#[test]
fn and_who_keeps_a_host_rule_off_a_room_and_an_attendee() {
    // A rest rule reaches whoever its `who` selects. Everyone here finished late
    // on Monday, so the rule's reach is exactly what decides Tuesday's early slot.
    let late_meeting = || vec![block(utc(MONDAY, 22, 0), utc(MONDAY, 23, 30))];
    let rest = || presets::rest(time(22, 0), time(8, 0));
    let tuesday = || span(utc(TUESDAY, 7, 0), utc(TUESDAY, 7, 30));
    let seven = || vec![utc(TUESDAY, 7, 0)];

    // A room whose own late meeting would cost it the early Tuesday slot if the
    // rest rule reached it.
    let room = || participant("room-4", ParticipantKind::Resource, late_meeting());
    let for_room =
        |rule: Rule| request(tuesday(), seven(), vec![room()], vec![rule], "room-4", &[]);

    assert!(
        offered(&for_room(rest())).is_empty(),
        "the default `who` is every host, and a room is a host"
    );
    assert_eq!(
        offered(&for_room(rest().and_who(people()))),
        [utc(TUESDAY, 7, 0)],
        "`and_who(person)` keeps a resource room out of a host rule"
    );

    // A human host with an attendee whose own late meeting only a rule that
    // reaches people would see.
    let pair = || {
        vec![
            participant("ana", ParticipantKind::Person, Vec::new()),
            participant("tom", ParticipantKind::Person, late_meeting()).with_role(Role::Attendee),
        ]
    };
    let for_pair = |rule: Rule| request(tuesday(), seven(), pair(), vec![rule], "ana", &[]);

    assert_eq!(
        offered(&for_pair(rest())),
        [utc(TUESDAY, 7, 0)],
        "the default `who` never reaches an attendee"
    );
    assert_eq!(
        offered(&for_pair(rest().and_who(people()))),
        [utc(TUESDAY, 7, 0)],
        "`and_who(person)` keeps the default `Role::Host` clause, so the attendee stays out"
    );
    assert!(
        offered(&for_pair(rest().with_who(people()))).is_empty(),
        "the twin call `with_who(person)` drops the host clause and reaches the attendee"
    );
}

#[test]
fn a_human_only_cap_does_not_limit_a_room() {
    // Two meetings on the room's Monday and a cap of two a day: a cap that applied
    // to the room leaves nothing on that day.
    let room = || {
        let mut first = block(utc(MONDAY, 9, 0), utc(MONDAY, 10, 0));
        first.tags = vec![tag("meeting")];
        let mut second = block(utc(MONDAY, 11, 0), utc(MONDAY, 12, 0));
        second.tags = vec![tag("meeting")];
        participant("room-4", ParticipantKind::Resource, vec![first, second])
    };
    let booking = |rules: Vec<Rule>| {
        request(
            span(utc(MONDAY, 15, 0), utc(MONDAY, 15, 30)),
            vec![utc(MONDAY, 15, 0)],
            vec![room()],
            rules,
            "room-4",
            &["meeting"],
        )
    };
    let cap = || presets::cap_per_day(Selector::Tag(tag("meeting")), 2);

    assert!(
        offered(&booking(vec![cap()])).is_empty(),
        "an unscoped cap counts the room's own meetings and is reached"
    );
    assert_eq!(
        offered(&booking(vec![cap().with_who(people())])),
        [utc(MONDAY, 15, 0)],
        "a cap scoped to people must not count a room's meetings"
    );
}

#[test]
fn a_kind_clause_keeps_a_team_tagged_room_out_of_a_human_slot() {
    // A busy person and a free room, both on the team, and one slot the team fills:
    // only the person can fill a human slot.
    let week = span(utc(MONDAY, 9, 0), utc(MONDAY, 17, 0));
    let candidates = vec![utc(MONDAY, 9, 0)];
    let team = || tag("team/eng");
    let people_only = Selector::all([Selector::Tag(team()), people()]);
    let members = || {
        vec![
            participant(
                "ana",
                ParticipantKind::Person,
                vec![block(utc(MONDAY, 0, 0), utc(TUESDAY, 0, 0))],
            )
            .with_tags(vec![team()]),
            participant("room-4", ParticipantKind::Resource, Vec::new()).with_tags(vec![team()]),
        ]
    };
    let slot = |who: Selector| {
        let mut goal = Goal::single(
            ZoneName::new("UTC").expect("a zone name"),
            week,
            Duration::from_minutes(30),
            Requirement::new(vec![Term::at_least(1, who)]),
        );
        goal.candidates = Some(candidates.clone());
        let mut request = Request::new(utc(MONDAY, 0, 0), goal);
        request.participants = members();
        request
    };

    // Tagged only, the room fills the slot the person cannot, and the offer says
    // whose time it is.
    let unscoped = engine()
        .find_availability(&slot(Selector::Tag(team())))
        .expect("the request is valid");
    let event = unscoped.offers[0].events[0].clone();
    assert_eq!(event.assignment, [vec!["room-4".to_owned()]]);
    assert_eq!(
        event.eligible,
        [vec!["room-4".to_owned()]],
        "the free room is the only team member who can take the slot"
    );

    assert!(
        offered(&slot(people_only)).is_empty(),
        "a slot only people fill must not be filled by a team-tagged room"
    );
}

#[test]
fn an_ignoring_status_policy_that_reaches_a_room_is_reported() {
    let room = || {
        held_room(
            "room-4",
            utc(MONDAY, 10, 0),
            utc(MONDAY, 11, 0),
            BlockStatus::Tentative,
        )
    };
    let validate = |rules: Vec<Rule>| {
        let request = request(
            span(utc(MONDAY, 10, 0), utc(MONDAY, 11, 0)),
            vec![utc(MONDAY, 10, 0)],
            vec![room()],
            rules,
            "room-4",
            &[],
        );
        engine().validate(&request)
    };
    let codes = |rules: Vec<Rule>| {
        validate(rules)
            .warnings
            .iter()
            .map(|finding| (finding.code(), finding.path().map(str::to_owned)))
            .collect::<Vec<_>>()
    };

    // The default `who` is every host, so a plain "ignore tentative" reaches the
    // room and drops its hold; the warning says so at the rule.
    assert_eq!(
        codes(vec![ignore_tentative()]),
        [(
            "status_policy_ignores_resource",
            Some("/rules/0".to_owned())
        )]
    );

    // Scoped to people, the same policy is exactly what the caller meant.
    assert!(codes(vec![ignore_tentative().with_who(people())]).is_empty());

    // A policy that blocks rather than ignores holds no footgun.
    let blocking = Rule::new(
        "block_tentative",
        RuleKind::StatusPolicy(StatusPolicy {
            entries: vec![StatusPolicyEntry::status(
                BlockStatus::Tentative,
                StatusEffect::Block,
            )],
        }),
    );
    assert!(codes(vec![blocking]).is_empty());

    // And a request that carries no resource is never warned about.
    let people_only = request(
        span(utc(MONDAY, 10, 0), utc(MONDAY, 11, 0)),
        vec![utc(MONDAY, 10, 0)],
        vec![participant("ana", ParticipantKind::Person, Vec::new())],
        vec![ignore_tentative().with_who(Selector::True)],
        "ana",
        &[],
    );
    assert!(
        engine()
            .validate(&people_only)
            .warnings
            .iter()
            .all(|finding| finding.code() != "status_policy_ignores_resource")
    );
}

#[test]
fn a_kind_clause_in_item_position_is_rejected() {
    // `kind` selects participants, so a `when` or a `count` holding one can never
    // match a part's tags. Silence there would read as a narrow rule that applies
    // to nothing.
    let mut request = request(
        span(utc(MONDAY, 9, 0), utc(MONDAY, 9, 30)),
        vec![utc(MONDAY, 9, 0)],
        vec![participant("ana", ParticipantKind::Person, Vec::new())],
        vec![presets::rest(time(22, 0), time(8, 0)).with_when(people())],
        "ana",
        &[],
    );
    let diagnostics = engine().validate(&request);
    assert!(
        diagnostics
            .errors
            .iter()
            .any(|finding| finding.code() == "selector_not_for_items"
                && finding.path() == Some("/rules/0/when")),
        "{diagnostics:?}"
    );

    // A cap counting a kind is the same mistake at its own field.
    request.rules = vec![presets::cap_per_day(people(), 1).with_when(Selector::True)];
    let diagnostics = engine().validate(&request);
    assert!(
        diagnostics
            .errors
            .iter()
            .any(|finding| finding.code() == "selector_not_for_items"
                && finding.path() == Some("/rules/0/params/count")),
        "{diagnostics:?}"
    );
}

#[test]
fn a_human_only_status_policy_still_ignores_a_persons_hold() {
    // The mitigation the warning names is `who: { "kind": "person" }`, so the
    // clause has to keep reaching the people it is scoped to. A `kind` clause that
    // matched nobody would silence the policy for humans instead of for rooms —
    // a wider regression than the footgun it is meant to fix, and one no test of
    // the room's behaviour would notice.
    let ana = || {
        held(
            "ana",
            ParticipantKind::Person,
            utc(MONDAY, 10, 0),
            utc(MONDAY, 11, 0),
            BlockStatus::Tentative,
        )
    };
    let booking = |rules: Vec<Rule>| {
        request(
            span(utc(MONDAY, 10, 0), utc(MONDAY, 11, 0)),
            vec![utc(MONDAY, 10, 0), utc(MONDAY, 10, 30)],
            vec![ana()],
            rules,
            "ana",
            &[],
        )
    };

    assert_eq!(
        offered(&booking(Vec::new())),
        Vec::<Instant>::new(),
        "ana's tentative hold blocks both starts without a policy"
    );
    assert_eq!(
        offered(&booking(vec![ignore_tentative().with_who(people())])),
        [utc(MONDAY, 10, 0), utc(MONDAY, 10, 30)],
        "a policy scoped to people still ignores a person's tentative hold"
    );
    assert!(
        offered(&booking(vec![
            ignore_tentative().with_who(Selector::kind(ParticipantKind::Resource))
        ]))
        .is_empty(),
        "the same policy scoped to resources must not reach a person"
    );
}

#[test]
fn a_resource_only_status_policy_reaches_a_room() {
    // The other half of the same clause: `kind` selects the kind it names, so a
    // policy scoped to resources applies to the room whose hold it is about.
    let room = || {
        held_room(
            "room-4",
            utc(MONDAY, 10, 0),
            utc(MONDAY, 11, 0),
            BlockStatus::Tentative,
        )
    };
    let booking = request(
        span(utc(MONDAY, 10, 0), utc(MONDAY, 11, 0)),
        vec![utc(MONDAY, 10, 0), utc(MONDAY, 10, 30)],
        vec![room()],
        vec![ignore_tentative().with_who(Selector::kind(ParticipantKind::Resource))],
        "room-4",
        &[],
    );
    assert_eq!(
        offered(&booking),
        [utc(MONDAY, 10, 0), utc(MONDAY, 10, 30)],
        "a policy scoped to resources must reach the room"
    );
}

#[test]
fn a_kind_clause_in_a_status_policy_entry_is_rejected() {
    // A status policy's `match` is a block-tag selector, the third place a clause
    // is judged against an item rather than a participant. A `kind` there reads as
    // a narrow entry and matches nothing, which is the mistake the other two
    // positions already reject.
    let mut request = request(
        span(utc(MONDAY, 9, 0), utc(MONDAY, 9, 30)),
        vec![utc(MONDAY, 9, 0)],
        vec![participant("ana", ParticipantKind::Person, Vec::new())],
        vec![Rule::new(
            "ignore_by_kind",
            RuleKind::StatusPolicy(StatusPolicy {
                entries: vec![StatusPolicyEntry::tags(people(), StatusEffect::Ignore)],
            }),
        )],
        "ana",
        &[],
    );
    let diagnostics = engine().validate(&request);
    assert!(
        diagnostics
            .errors
            .iter()
            .any(|finding| finding.code() == "selector_not_for_items"
                && finding.path() == Some("/rules/0/params/entries/0/match")),
        "{diagnostics:?}"
    );

    // The same clause against the part's tags is rejected where a block selector is
    // not involved: the rule's own `when`.
    request.rules = vec![ignore_tentative().with_when(people())];
    let diagnostics = engine().validate(&request);
    assert!(
        diagnostics
            .errors
            .iter()
            .any(|finding| finding.code() == "selector_not_for_items"
                && finding.path() == Some("/rules/0/when")),
        "{diagnostics:?}"
    );
}

#[test]
fn a_policy_that_cannot_apply_to_the_booking_is_not_reported_as_ignoring_holds() {
    // A status policy whose `when` excludes this booking's part cannot drop this
    // room's hold, so warning that it does would name a footgun the caller cannot
    // trip. Its real problem — a `when` that matches no part — is reported instead.
    let request = request(
        span(utc(MONDAY, 10, 0), utc(MONDAY, 11, 0)),
        vec![utc(MONDAY, 10, 0)],
        vec![held_room(
            "room-4",
            utc(MONDAY, 10, 0),
            utc(MONDAY, 11, 0),
            BlockStatus::Tentative,
        )],
        vec![ignore_tentative().with_when(Selector::Tag(tag("interview")))],
        "room-4",
        &["standup"],
    );
    let diagnostics = engine().validate(&request);
    let codes: Vec<&str> = diagnostics
        .warnings
        .iter()
        .map(opportune::Diagnostic::code)
        .collect();
    assert!(
        codes.contains(&"when_matches_no_part"),
        "the unreachable `when` is the finding: {codes:?}"
    );
    assert!(
        !codes.contains(&"status_policy_ignores_resource"),
        "a rule that cannot apply drops nothing: {codes:?}"
    );
}

#[test]
fn the_warning_points_at_the_rule_it_is_about_not_the_first_rule() {
    // A rule that ignores nothing leads the request, so the pointer has to name the
    // ignoring policy's own position in the whole rule list. A position counted
    // among the reported rules, or a fixed `0`, would send the caller to a rule that
    // is not at fault while the request it describes stays the same.
    let request = request(
        span(utc(MONDAY, 10, 0), utc(MONDAY, 11, 0)),
        vec![utc(MONDAY, 10, 0)],
        vec![held_room(
            "room-4",
            utc(MONDAY, 10, 0),
            utc(MONDAY, 11, 0),
            BlockStatus::Tentative,
        )],
        vec![presets::rest(time(22, 0), time(8, 0)), ignore_tentative()],
        "room-4",
        &[],
    );
    let codes: Vec<(String, Option<String>)> = engine()
        .validate(&request)
        .warnings
        .iter()
        .map(|finding| (finding.code().to_owned(), finding.path().map(str::to_owned)))
        .collect();
    assert_eq!(
        codes,
        [(
            "status_policy_ignores_resource".to_owned(),
            Some("/rules/1".to_owned())
        )]
    );
}

#[test]
fn the_warning_says_what_the_guard_actually_found() {
    // The guard reports an `ignore` entry whose rule reaches a resource participant.
    // It never reads the entry's status, and it never checks that the same rule also
    // reaches a person, so the message may not claim either: a `busy -> ignore`
    // policy drops a room's confirmed booking rather than a tentative hold, and a
    // policy scoped to resources on purpose reaches no people at all — advising
    // `who: { "kind": "person" }` as the one thing to do there inverts what the
    // caller wrote and swaps whose holds the policy drops. The guard reads no
    // calendar, so the room's own block status is not part of what is judged here.
    let reported = |rule: Rule| {
        let request = request(
            span(utc(MONDAY, 10, 0), utc(MONDAY, 11, 0)),
            vec![utc(MONDAY, 10, 0)],
            vec![held_room(
                "room-4",
                utc(MONDAY, 10, 0),
                utc(MONDAY, 11, 0),
                BlockStatus::Tentative,
            )],
            vec![rule],
            "room-4",
            &[],
        );
        engine()
            .validate(&request)
            .warnings
            .iter()
            .find(|warning| warning.code() == "status_policy_ignores_resource")
            .map(|warning| warning.message().to_owned())
            .expect("an ignoring policy that reaches the room is reported")
    };

    let busy = reported(Rule::new(
        "ignore_busy",
        RuleKind::StatusPolicy(StatusPolicy {
            entries: vec![StatusPolicyEntry::status(
                BlockStatus::Busy,
                StatusEffect::Ignore,
            )],
        }),
    ));
    assert!(busy.contains("room-4"), "{busy}");
    assert!(
        busy.contains("\"kind\": \"person\""),
        "the narrower `who` is still the remedy on offer: {busy}"
    );
    assert!(
        !busy.contains("tentative"),
        "the entry ignores `busy`; the message names a status the guard never read: {busy}"
    );

    let scoped_to_resources =
        reported(ignore_tentative().with_who(Selector::kind(ParticipantKind::Resource)));
    assert!(
        !scoped_to_resources.contains("as well as for people"),
        "a policy scoped to resources reaches no people: {scoped_to_resources}"
    );
}
