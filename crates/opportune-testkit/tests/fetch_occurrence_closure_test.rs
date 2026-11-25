//! The occurrence half of the one-round key closure.
//!
//! Step 3 of the documented fetch (`RequiredRanges`, "The fetch is two-phase, not
//! one pass") fetches *every* record carrying an observed event key, occurrences
//! included. [`key_closed_request`] models that with
//! `overlaps(occurrence.span, occurrences) || observed.contains(&occurrence.event_key)`,
//! and this pins the second disjunct: an occurrence that lies outside the
//! occurrence hull, whose key is observed on a block whose own span *is* inside it.
//!
//! The shape is load-bearing because an occurrence is authoritative for its
//! event's span and participants — `caps.rs::counted_events` inserts it first and
//! a same-key block then only unions tags — so a dropped out-of-hull occurrence
//! turns the in-hull block into a standalone event in the candidate's own bucket.
//! A model that kept only hull-overlapping occurrences would therefore answer a
//! different question from the caller who supplied everything, and every
//! differential suite built on it would bless the narrower fetch.

use std::sync::OnceLock;

use opportune::Engine;
use opportune::model::{
    Block, Cap, CapMeasure, CapPer, CapPeriod, Explain, Goal, Occurrence, Participant, RangesQuery,
    Request, RequiredRanges, Requirement, Rule, RuleKind, Span, Term,
};
use opportune::reasons::ReasonCode;
use opportune::tags::{Selector, Tag};
use opportune::time::{
    Date, Duration, Instant, LocalResolution, RuleZone, TimeOfDay, Weekday, Zone, ZoneName,
};
use opportune_testkit::fetch::{key_closed_request, one_pass_request};

fn engine() -> &'static Engine {
    static ENGINE: OnceLock<Engine> = OnceLock::new();
    ENGINE.get_or_init(|| {
        Engine::builder()
            .build()
            .expect("the bundled database builds")
    })
}

fn tag(path: &str) -> Tag {
    Tag::new(path).expect("a fixture tag is a valid tag")
}

fn zone(name: &str) -> ZoneName {
    ZoneName::new(name).expect("a fixture zone name")
}

/// The instant of a UTC date-time.
fn utc(day: Date, hour: u8, minute: u8) -> Instant {
    let LocalResolution::Unique(instant) = Zone::utc()
        .resolve_local(day, TimeOfDay::new(hour, minute).expect("a fixture time"))
        .expect("the fixture instant is inside the supported range")
    else {
        panic!("UTC has no transitions");
    };
    instant
}

/// A block over `span`, carrying `key` and the counted tag.
fn keyed_block(span: Span, key: &str) -> Block {
    let mut block = Block::new(span);
    block.event_key = Some(key.to_owned());
    block.tags = vec![tag("interview")];
    block
}

fn required_ranges_of(request: &Request) -> RequiredRanges {
    let mut query = RangesQuery::new(request.now, request.goal.clone());
    query.rules.clone_from(&request.rules);
    query.participants.clone_from(&request.participants);
    engine()
        .required_ranges(&query)
        .expect("the fixture query resolves")
}

/// `true` when the response excludes `start` on the cap.
fn cap_reaches(response: &opportune::Response, start: Instant) -> bool {
    response.excluded.iter().any(|entry| {
        entry.start == start
            && entry
                .reasons
                .iter()
                .any(|reason| reason.code == ReasonCode::CapReached)
    })
}

/// The fixture's dates and instants: the day the window sits on, the next local
/// day's midnight (the end of the day bucket), and the far day the authoritative
/// occurrence starts on.
struct Days {
    day: Date,
    next: Date,
    far: Date,
}

fn days() -> Days {
    Days {
        day: Date::new(2027, 6, 16).expect("a fixture date"),
        next: Date::new(2027, 6, 17).expect("a fixture date"),
        // Outside the day bucket the window touches, so the initial
        // occurrence-range fetch never reaches it.
        far: Date::new(2027, 6, 22).expect("a fixture date"),
    }
}

/// One split event: a tagged block inside the candidate's day bucket and the
/// authoritative occurrence of the same key five days past it.
fn split_event_request(days: &Days) -> Request {
    let Days { day, far, .. } = *days;
    let in_hull = keyed_block(
        Span::new(utc(day, 10, 0), utc(day, 10, 30)).expect("a span"),
        "evt_split",
    );
    let mut ana = Participant::new("ana", zone("UTC"));
    ana.calendar = Some(vec![in_hull]);

    let mut goal = Goal::single(
        zone("UTC"),
        Span::new(utc(day, 9, 0), utc(day, 15, 0)).expect("a window"),
        Duration::from_minutes(30),
        Requirement::new(vec![Term::person("ana")]),
    );
    goal.candidates = Some(vec![utc(day, 13, 0)]);
    goal.parts[0].tags = vec![tag("interview")];
    let mut request = Request::new(utc(day, 9, 0), goal);
    request.participants.push(ana);
    request.occurrences.push(
        Occurrence::new(
            "evt_split",
            Span::new(utc(far, 10, 0), utc(far, 10, 30)).expect("a span"),
            vec!["ana".to_owned()],
        )
        .expect("the occurrence names a participant"),
    );
    request.rules.push(Rule::new(
        "cap",
        RuleKind::Cap(Cap::new(
            Selector::Tag(tag("interview")),
            CapMeasure::Count,
            1,
            CapPeriod::Day,
            CapPer::Each,
            RuleZone::Participant,
            Weekday::Monday,
        )),
    ));
    request.explain = Some(Explain::new(10_000));
    request
}

/// A same-key occurrence further out than the occurrence hull is fetched by the
/// second phase, and without it the in-hull block charges the wrong bucket.
#[test]
fn closure_retrieves_an_out_of_hull_occurrence_sharing_an_observed_key() {
    let days = days();
    let Days { day, next, .. } = days;
    let candidate = utc(day, 13, 0);
    let request = split_event_request(&days);

    let ranges = required_ranges_of(&request);
    let hull = ranges.occurrences.expect("the cap applies");
    assert_eq!(
        (hull.start(), hull.end()),
        (utc(day, 0, 0), utc(next, 0, 0)),
        "the occurrence hull is the candidate's day bucket"
    );
    let far_occurrence = &request.occurrences[0];
    assert!(
        far_occurrence.span.start() >= hull.end(),
        "the fixture's occurrence lies outside the occurrence hull"
    );

    let full = engine()
        .find_availability(&request)
        .expect("the full fixture request is valid");
    let one_pass = one_pass_request(&request, &ranges);
    let closed = key_closed_request(&request, &ranges);

    // The initial fetch alone drops the authoritative occurrence.
    assert!(
        one_pass.occurrences.is_empty(),
        "the one-pass fetch never sees the out-of-hull occurrence"
    );
    assert_eq!(
        closed.occurrences, request.occurrences,
        "closure must retain an out-of-hull occurrence whose key a block observed"
    );

    // Full data: the occurrence fixes the event's span five days out, so the
    // candidate's own day bucket is free.
    assert_eq!(
        full.offers.len(),
        1,
        "the occurrence moves the event out of the candidate's bucket"
    );
    assert!(
        !cap_reaches(&full, candidate),
        "full data does not charge the candidate's day: {:?}",
        full.excluded
    );

    let closed_response = engine()
        .find_availability(&closed)
        .expect("the key-closed fixture request is valid");
    assert_eq!(closed_response.offers, full.offers, "offers differ");
    assert_eq!(closed_response.excluded, full.excluded, "exclusions differ");
    assert_eq!(closed_response.dates, full.dates, "date counts differ");

    // The clause is load-bearing, not decoration: without the occurrence the
    // in-hull block stands alone in the candidate's bucket and fills the cap.
    let one_pass_response = engine()
        .find_availability(&one_pass)
        .expect("the one-pass fixture request is valid");
    assert!(
        one_pass_response.offers.is_empty(),
        "the one-pass fetch must diverge: {:?}",
        one_pass_response.offers
    );
    assert!(
        cap_reaches(&one_pass_response, candidate),
        "the one-pass fetch charges the block to the candidate's day: {:?}",
        one_pass_response.excluded
    );

    // The same control on the closed request: drop only the occurrence and the
    // answer flips, so retaining it is what preserves the full answer.
    let mut occurrence_dropped = closed.clone();
    occurrence_dropped.occurrences.clear();
    let dropped = engine()
        .find_availability(&occurrence_dropped)
        .expect("the control request is valid");
    assert!(
        cap_reaches(&dropped, candidate),
        "dropping the authoritative occurrence falsely charges ana's block: {:?}",
        dropped.excluded
    );
    assert_eq!(
        closed.participants[0].blocks(),
        request.participants[0].blocks(),
        "the in-hull block itself is unaffected by the occurrence"
    );
}
