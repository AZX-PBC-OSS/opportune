//! The wire layer's contract: every domain type round-trips through JSON without
//! loss, every default the specification states is applied, rounding moves the
//! way the specification says, and a rejection names the field at fault with an
//! RFC 6901 pointer.
//!
//! The output half runs against a real engine, because that is the only producer
//! of a response, a slot check, a diagnostics report and a required-ranges
//! envelope: constructing a `Response` by hand is not possible, and a fake one
//! would prove nothing about what a caller actually receives. Those cases run
//! where the crate compiles in a time zone database (`bundled-tzdb`); the rest of
//! the file needs none and is what the stripped test build runs.

#[cfg(feature = "bundled-tzdb")]
use opportune::Engine;
use opportune::Limits;
#[cfg(feature = "bundled-tzdb")]
use opportune::model::SlotCheck;
use opportune::model::{
    Block, BlockStatus, CalendarSpec, CapMeasure, CapPer, CapPeriod, Goal, LeadTimeMax, Occurrence,
    Participant, ParticipantKind, Proposal, RangesQuery, Request, Rule, StatusEffect,
};
use opportune::presets;
use opportune::tags::{Role, Selector, Tag};
use opportune::time::{
    Date, DateKey, DateRange, Duration, Instant, RuleZone, TimeOfDay, WallEnd, WallRange, Weekday,
    Weekdays, WeeklyKey,
};
use opportune_json::{
    LeadTimeParams, LeadTimeRule, MatchAll, WireBlock, WireCalendarSpec, WireGoal, WireLimits,
    WireOccurrence, WireParticipant, WireProposal, WireRangesQuery, WireRequest, WireRule,
    WireSelector, parse_calendar, parse_goal, parse_limits, parse_proposal, parse_ranges_query,
    parse_request, parse_rule, to_json,
};
use opportune_json::{
    WireDateCount, WireDiagnostic, WireDiagnostics, WireExcluded, WireMeta, WireOffer,
    WireOfferEvent, WireReason, WireReasonParams, WireRequiredRanges, WireReserveSpan,
    WireResponse, WireSlotCheck, WireSpan,
};
use serde_json::Value;

/// The search window every helper request uses: an hour inside the supported
/// range, which the engine can grid at the default step.
const HOUR: &str = r#"["2027-03-09T15:00Z","2027-03-09T16:00Z"]"#;

/// A request carrying every field the specification's examples use: all seven
/// rule kinds, calendars, zone periods, weights, occurrences, travel, an
/// explanation and an offer limit.
fn full_request() -> String {
    r#"{
  "now": "2027-03-09T15:00:00Z",
  "goal": {
    "parts": [{
      "id": "main",
      "tags": ["interview/front-end"],
      "duration": "45m",
      "requirement": { "all": [{ "at_least": 1, "of": { "tag": "pool/frontend" } }] },
      "travel": { "before": "20m", "after": "20m",
                   "by_participant": { "tom": { "before": "35m", "after": "35m" } } }
    }],
    "grid": { "step": "15m", "anchor": "00:00", "zone": "event" },
    "search_window": ["2027-03-08T05:00Z", "2027-03-15T05:00Z"],
    "zone": "America/New_York",
    "display_zone": "Europe/London"
  },
  "participants": [
    { "id": "tom", "kind": "person", "role": "host", "tags": ["team/sales"],
       "zone": "America/New_York",
       "zone_periods": [{ "zone": "America/Anchorage",
                           "from": "2027-03-07T22:00Z", "until": "2027-03-11T03:00Z" }],
       "calendar": [{ "span": ["2027-03-09T14:00:30Z", "2027-03-09T15:00:01Z"],
                       "status": "tentative", "all_day": false,
                       "tags": ["status/tentative"], "event_key": "evt_1",
                       "travel_before": "10m", "travel_after": "10m" }] },
    { "id": "ana", "kind": "resource", "role": "attendee", "tags": ["kind/room"],
       "zone": "Europe/London" }
  ],
  "rules": [
    { "id": "r_window", "kind": "window", "who": { "role": "host" },
       "when": { "tag": "interview" },
       "params": { "zone": "participant",
                    "weekly": { "mon,wed": ["09:00-17:00"],
                                 "tue": ["10:00-12:00", "14:00-18:00"],
                                 "fri": ["22:00-02:00"] },
                    "dates": { "2027-03-08..2027-03-10": ["10:00-16:00"],
                                "2026-12-25": [] } } },
    { "id": "r_blackout", "kind": "blackout", "when": { "tag": "interview" },
       "params": { "zone": "event",
                    "spans": [["2026-12-24T00:00", "2027-01-02T00:00"]] } },
    { "id": "r_clearance", "kind": "clearance",
       "params": { "before": "10m", "after": "10m", "min_gap": "5m" } },
    { "id": "r_lead", "kind": "lead_time",
       "params": { "min": "4h",
                    "max": { "working_days": 3, "calendar": "org" } } },
    { "id": "r_status", "kind": "status_policy",
       "params": { "entries": [
         { "match": { "tag": "status/focus" }, "effect": "ignore" },
         { "status": "tentative", "effect": "ignore" },
         { "status": "busy", "match": { "tag": "status/oof" }, "effect": "block" } ] } },
    { "id": "r_rest", "kind": "rest",
       "params": { "late_after": "18:00", "not_before": "10:00" } },
    { "id": "r_cap", "kind": "cap", "who": { "tag": "pool/interviewers" },
       "when": { "tag": "interview" },
       "params": { "count": { "tag": "interview" }, "measure": "minutes",
                    "limit": 120, "period": "week", "per": "pooled",
                    "zone": "UTC", "week_starts": "sun" } }
  ],
  "occurrences": [{ "event_key": "evt_91",
                     "span": ["2027-03-09T14:00Z", "2027-03-09T15:00Z"],
                     "tags": ["interview/front-end"],
                     "participants": ["tom", "ana"] }],
  "exclude_event_keys": ["evt_91"],
  "calendars": { "org": { "zone": "America/New_York", "working_days": "mon-fri",
                            "holidays": ["2027-01-01", "2027-05-31"] } },
  "weights": { "ana": 3 },
  "limit": 20,
  "explain": { "limit": 200 }
}"#
    .to_owned()
}

/// The round-trip of a parsed document through the wire and back must not lose a
/// single domain value: every rule kind, calendar, zone period, weight,
/// occurrence, travel override, explanation and limit survives it.
#[test]
fn a_full_request_round_trips_without_loss() {
    let request = parse_request(&full_request()).expect("the specification's shapes parse");
    let wire = WireRequest::from(&request);
    let reparsed = parse_request(&to_json(&wire).expect("the wire value writes"))
        .expect("what the wire wrote reads back");

    assert_eq!(reparsed, request);
    // Spot-check that the equality is not two equally empty values.
    assert_eq!(request.rules.len(), 7);
    assert_eq!(request.participants.len(), 2);
    assert_eq!(request.occurrences.len(), 1);
    assert_eq!(request.limit, Some(20));
    assert_eq!(request.explain.map(|explain| explain.limit), Some(200));
    assert_eq!(request.weight("ana"), 3);
    assert_eq!(request.exclude_event_keys, ["evt_91"]);
    assert_eq!(
        request.participants[0].zone_periods[0].zone().as_str(),
        "America/Anchorage"
    );
    assert_eq!(
        request.goal.parts[0].travel.by_participant["tom"].before,
        Duration::from_minutes(35)
    );
    assert_eq!(request.calendars["org"].working_days, Weekdays::weekdays());
    let kinds: Vec<&str> = request
        .rules
        .iter()
        .map(|rule| rule.kind.as_str())
        .collect();
    assert_eq!(
        kinds,
        [
            "window",
            "blackout",
            "clearance",
            "lead_time",
            "status_policy",
            "rest",
            "cap"
        ]
    );
}

/// Each rule kind's parameters reach the domain shapes the engine evaluates.
#[test]
fn every_rule_kind_converts_to_its_domain_parameters() {
    let request = parse_request(&full_request()).unwrap();
    let rules = &request.rules;

    let window = rules[0].window().expect("a window rule carries a window");
    assert_eq!(
        window.weekly()[0].len(),
        1,
        "Monday shares the `mon,wed` key"
    );
    assert_eq!(window.weekly()[1].len(), 2, "Tuesday has two ranges");
    assert!(window.weekly()[3].is_empty(), "Thursday is closed");
    assert_eq!(window.weekly()[4].len(), 1, "Friday crosses midnight");
    assert_eq!(window.dates().len(), 2);
    assert_eq!(rules[1].blackout().unwrap().spans().len(), 1);
    assert_eq!(
        rules[2].clearance().unwrap().min_gap,
        Duration::from_minutes(5)
    );
    let lead_time = rules[3].lead_time().unwrap();
    assert_eq!(lead_time.min, Duration::from_hours(4));
    assert_eq!(
        lead_time.max,
        Some(LeadTimeMax::WorkingDays {
            count: 3,
            calendar: "org".to_owned()
        })
    );
    let policy = rules[4].status_policy().unwrap();
    assert_eq!(policy.entries[0].effect, StatusEffect::Ignore);
    assert_eq!(
        policy.entries[0].tags,
        Some(Selector::Tag(
            opportune::tags::Tag::new("status/focus").unwrap()
        ))
    );
    assert_eq!(policy.entries[1].status, Some(BlockStatus::Tentative));
    // An entry may match on both, which is the third form the specification names.
    assert_eq!(policy.entries[2].status, Some(BlockStatus::Busy));
    assert_eq!(
        policy.entries[2].tags,
        Some(Selector::Tag(
            opportune::tags::Tag::new("status/oof").unwrap()
        ))
    );
    assert_eq!(policy.entries[2].effect, StatusEffect::Block);
    assert_eq!(
        rules[5].rest().unwrap().late_after,
        TimeOfDay::new(18, 0).unwrap()
    );
    let cap = rules[6].cap().unwrap();
    assert_eq!(cap.measure, CapMeasure::Minutes);
    assert_eq!(cap.period, CapPeriod::Week);
    assert_eq!(cap.per, CapPer::Pooled);
    assert_eq!(cap.week_starts, Weekday::Sunday);
}

/// `now` rounds up and a block's endpoints round outward; the block in the full
/// request carries seconds at both ends for exactly that reason.
#[test]
fn rounding_moves_the_way_the_specification_says() {
    let request = parse_request(&full_request()).unwrap();
    assert_eq!(request.now.to_string(), "2027-03-09T15:00Z");
    let block = &request.participants[0].calendar.as_ref().unwrap()[0];
    assert_eq!(block.span.start().to_string(), "2027-03-09T14:00Z");
    assert_eq!(block.span.end().to_string(), "2027-03-09T15:01Z");

    let rounded = parse_request(&format!(
        r#"{{ "now": "2027-03-09T15:00:01Z",
              "goal": {{ "parts": [{{ "duration": "30m",
                                    "requirement": {{ "all": [{{ "person": "ana" }}] }} }}],
                        "search_window": {HOUR}, "zone": "UTC" }} }}"#
    ))
    .unwrap();
    assert_eq!(rounded.now.to_string(), "2027-03-09T15:01Z");
}

/// A candidate start is not a rounded value: a caller that supplies one must
/// already be on a minute, and the rejection points at the candidate.
#[test]
fn an_unaligned_candidate_start_is_rejected_at_its_index() {
    let error = parse_request(&format!(
        r#"{{ "now": "2027-03-09T15:00Z",
              "goal": {{ "parts": [{{ "duration": "30m",
                                    "requirement": {{ "all": [{{ "person": "ana" }}] }} }}],
                        "candidates": ["2027-03-09T15:00:15Z"],
                        "search_window": {HOUR}, "zone": "UTC" }} }}"#
    ))
    .unwrap_err();
    assert_eq!(error.code(), "instant_not_aligned");
    assert_eq!(error.path(), "/goal/candidates/0");
}

/// Every default the specification states is applied while parsing.
#[test]
fn the_specification_defaults_are_applied() {
    let request = parse_request(&format!(
        r#"{{ "now": "2027-03-09T15:00Z",
              "goal": {{ "parts": [{{ "duration": "30m",
                                    "requirement": {{ "all": [{{ "person": "ana" }}] }} }}],
                        "search_window": {HOUR}, "zone": "Europe/London" }},
              "participants": [{{ "id": "ana", "zone": "Europe/London",
                                 "calendar": [{{ "span": {HOUR} }}] }}],
              "rules": [
                {{ "id": "r_rest", "kind": "rest",
                   "params": {{ "late_after": "18:00", "not_before": "10:00" }} }},
                {{ "id": "r_cap", "kind": "cap",
                   "params": {{ "count": {{ "tag": "interview" }}, "measure": "count",
                                "limit": 2, "period": "day", "per": "each",
                                "zone": "participant" }} }}
              ] }}"#
    ))
    .unwrap();

    assert_eq!(request.participants[0].kind, ParticipantKind::Person);
    assert_eq!(request.participants[0].role, Role::Host);
    assert!(request.participants[0].tags.is_empty());
    assert!(request.participants[0].zone_periods.is_empty());
    assert_eq!(
        request.participants[0].calendar.as_ref().unwrap()[0].status,
        BlockStatus::Busy
    );
    assert!(!request.participants[0].calendar.as_ref().unwrap()[0].all_day);
    assert!(
        request.participants[0].calendar.as_ref().unwrap()[0]
            .tags
            .is_empty()
    );
    assert_eq!(request.goal.parts[0].id, "main");
    assert_eq!(request.goal.display_zone, request.goal.zone);
    assert!(request.goal.parts[0].tags.is_empty());
    assert_eq!(
        request.goal.parts[0].travel,
        opportune::model::Travel::default()
    );
    assert_eq!(request.rules[0].who, Selector::Role(Role::Host));
    assert_eq!(request.rules[0].when, Selector::True);
    assert_eq!(request.rules[1].cap().unwrap().week_starts, Weekday::Monday);
    // Everything the document left out is empty or absent, not invented.
    assert!(request.occurrences.is_empty());
    assert!(request.calendars.is_empty());
    assert!(request.weights.is_empty());
    assert_eq!(request.limit, None);
    assert_eq!(request.explain, None);
    assert!(request.exclude_event_keys.is_empty());
    assert_eq!(request.limit(), 1_000, "the engine's published default");
}

/// The wire's key names are the specification's, not whatever a round trip would
/// tolerate: a field renamed on both sides still round-trips, so the names are
/// asserted against the JSON a caller and its generated models read.
#[test]
fn the_serialised_names_are_the_specifications() {
    let request = parse_request(&full_request()).unwrap();
    let value: Value =
        serde_json::from_str(&to_json(&WireRequest::from(&request)).unwrap()).unwrap();

    let kinds: Vec<Value> = value["rules"]
        .as_array()
        .unwrap()
        .iter()
        .map(|rule| rule["kind"].clone())
        .collect();
    assert_eq!(
        kinds,
        [
            Value::from("window"),
            Value::from("blackout"),
            Value::from("clearance"),
            Value::from("lead_time"),
            Value::from("status_policy"),
            Value::from("rest"),
            Value::from("cap"),
        ]
    );
    // The status-policy entry's tag selector is `match`, never `matched`.
    assert_eq!(
        value["rules"][4]["params"]["entries"][0]["match"]["tag"],
        Value::from("status/focus")
    );
    assert!(
        value["rules"][4]["params"]["entries"][0]
            .get("matched")
            .is_none()
    );
    assert_eq!(
        value["goal"]["parts"][0]["requirement"]["all"][0]["at_least"],
        Value::from(1)
    );
    assert_eq!(
        value["rules"][3]["params"]["max"]["working_days"],
        Value::from(3)
    );
    assert_eq!(
        value["rules"][6]["params"]["week_starts"],
        Value::from("sun")
    );
    assert_eq!(value["rules"][2]["params"]["min_gap"], Value::from("5m"));
    assert_eq!(
        value["rules"][5]["params"]["late_after"],
        Value::from("18:00")
    );
    assert_eq!(
        value["rules"][5]["params"]["not_before"],
        Value::from("10:00")
    );
    assert_eq!(
        value["goal"]["parts"][0]["travel"]["by_participant"]["tom"]["before"],
        Value::from("35m")
    );
    assert_eq!(
        value["goal"]["search_window"][0],
        Value::from("2027-03-08T05:00Z")
    );
    assert_eq!(
        value["participants"][0]["zone_periods"][0]["until"],
        Value::from("2027-03-11T03:00Z")
    );
    assert_eq!(
        value["participants"][0]["calendar"][0]["event_key"],
        Value::from("evt_1")
    );
    assert_eq!(
        value["participants"][0]["calendar"][0]["all_day"],
        Value::Bool(false)
    );
    assert_eq!(
        value["participants"][0]["calendar"][0]["travel_before"],
        Value::from("10m")
    );
    assert_eq!(value["occurrences"][0]["event_key"], Value::from("evt_91"));
    assert_eq!(value["explain"]["limit"], Value::from(200));
    // Serialising writes the day set as a list; parsing accepts a list, a range
    // or a wrapping range, so `mon-fri` reads back to the same set.
    assert_eq!(
        value["calendars"]["org"]["working_days"],
        Value::from("mon,tue,wed,thu,fri")
    );
}

/// The exclusion list reads the three spellings the specification allows — absent,
/// `null` and an array — and a request that excludes nothing writes no key at all,
/// so a document from before the field existed and one from a binding that wrote
/// every key are the same document the engine answers.
#[test]
fn the_excluded_event_keys_read_absent_null_and_array_and_write_only_when_named() {
    let document = |keys: &str| {
        format!(
            r#"{{ "now": "2027-03-09T15:00Z",
                  "goal": {{ "parts": [{{ "duration": "30m",
                                        "requirement": {{ "all": [{{ "person": "ana" }}] }} }}],
                            "search_window": {HOUR}, "zone": "UTC" }}
                  {keys} }}"#
        )
    };

    assert!(
        parse_request(&document(""))
            .unwrap()
            .exclude_event_keys
            .is_empty(),
        "an absent list excludes nothing"
    );
    assert!(
        parse_request(&document(r#", "exclude_event_keys": null"#))
            .unwrap()
            .exclude_event_keys
            .is_empty(),
        "an explicit null is the absent list"
    );
    assert_eq!(
        parse_request(&document(r#", "exclude_event_keys": ["evt_1", "evt_2"]"#))
            .unwrap()
            .exclude_event_keys,
        ["evt_1", "evt_2"],
        "an array is the keys it names, in order"
    );

    let request = parse_request(&document("")).unwrap();
    let value: Value =
        serde_json::from_str(&to_json(&WireRequest::from(&request)).unwrap()).unwrap();
    assert!(
        value.get("exclude_event_keys").is_none(),
        "a request excluding nothing writes no key: {value}"
    );
}

/// The exclusion list is a `Request` field and not a ranges-query one: the ranges
/// entry point still refuses it at the document root. This is what the TypeScript
/// projection strips it for, and a query that read it would accept a document the
/// projection had no reason to narrow.
#[test]
fn a_ranges_query_has_no_place_for_the_exclusion_list() {
    let error = parse_ranges_query(&format!(
        r#"{{ "now": "2027-03-09T15:00Z",
              "goal": {{ "parts": [{{ "duration": "30m",
                                    "requirement": {{ "all": [{{ "person": "ana" }}] }} }}],
                        "search_window": {HOUR}, "zone": "UTC" }},
              "exclude_event_keys": ["evt_1"] }}"#
    ))
    .unwrap_err();
    assert_eq!(error.code(), "unknown_field");
    assert_eq!(error.path(), "");
    assert!(error.message().contains("exclude_event_keys"), "{error}");
}

/// An excluded event reaches the engine from a document: the same request, with and
/// without the key, is a different answer, so the wire field the caller writes is
/// the one the caps read.
#[cfg(feature = "bundled-tzdb")]
#[test]
fn a_document_reschedules_an_event_the_day_is_already_full_of() {
    let engine = Engine::builder()
        .build()
        .expect("the bundled database builds");
    let request = |exclusion: &str| {
        format!(
            r#"{{ "now": "2027-03-09T05:00Z",
                  "goal": {{ "parts": [{{ "tags": ["interview"], "duration": "60m",
                                        "requirement": {{ "all": [{{ "person": "ana" }}] }} }}],
                            "candidates": ["2027-03-09T14:00Z"],
                            "search_window": ["2027-03-09T05:00Z", "2027-03-10T05:00Z"],
                            "zone": "UTC" }},
                  "participants": [{{ "id": "ana", "zone": "UTC",
                     "calendar": [
                       {{ "span": ["2027-03-09T09:00Z", "2027-03-09T10:00Z"],
                          "tags": ["interview"], "event_key": "evt_a" }},
                       {{ "span": ["2027-03-09T11:00Z", "2027-03-09T12:00Z"],
                          "tags": ["interview"], "event_key": "evt_b" }} ] }}],
                  "rules": [{{ "id": "each_day", "kind": "cap",
                     "params": {{ "count": {{ "tag": "interview" }}, "measure": "count",
                                  "limit": 2, "period": "day", "per": "each",
                                  "zone": "event" }} }}]{exclusion} }}"#
        )
    };
    let starts = |exclusion: &str| -> Vec<String> {
        let written = opportune_json::find_availability(&engine, &request(exclusion))
            .expect("the request is valid");
        let response: Value = serde_json::from_str(&written).expect("a Response document");
        response["offers"]
            .as_array()
            .expect("offers is an array")
            .iter()
            .map(|offer| offer["events"][0]["start"].as_str().unwrap().to_owned())
            .collect()
    };

    assert!(
        starts("").is_empty(),
        "the day already holds its two events"
    );
    assert_eq!(
        starts(r#", "exclude_event_keys": ["evt_a"]"#),
        ["2027-03-09T14:00Z"],
        "the event being moved no longer fills the day"
    );
}

/// A provider status the wire does not know reads as `busy`, while an unknown
/// key on the same block object is still rejected at the block.
#[test]
fn an_unrecognised_block_status_reads_as_busy_but_an_unknown_key_fails() {
    let request_with_status = parse_request(&format!(
        r#"{{ "now": "2027-03-09T15:00Z",
              "goal": {{ "parts": [{{ "duration": "30m",
                                    "requirement": {{ "all": [{{ "person": "ana" }}] }} }}],
                        "search_window": {HOUR}, "zone": "UTC" }},
              "participants": [{{ "id": "ana", "zone": "UTC",
                                 "calendar": [{{ "span": {HOUR},
                                                "status": "custom_provider_status" }}] }}] }}"#
    ))
    .unwrap();
    assert_eq!(
        request_with_status.participants[0]
            .calendar
            .as_ref()
            .unwrap()[0]
            .status,
        BlockStatus::Busy
    );

    let error = parse_request(&format!(
        r#"{{ "now": "2027-03-09T15:00Z",
              "goal": {{ "parts": [{{ "duration": "30m",
                                    "requirement": {{ "all": [{{ "person": "ana" }}] }} }}],
                        "search_window": {HOUR}, "zone": "UTC" }},
              "participants": [{{ "id": "ana", "zone": "UTC",
                                 "calendar": [{{ "span": {HOUR}, "extra_field": 1 }}] }}] }}"#
    ))
    .unwrap_err();
    assert_eq!(error.code(), "unknown_field");
    assert_eq!(error.path(), "/participants/0/calendar/0");
    assert!(error.message().contains("extra_field"), "{error}");
}

/// A rejection inside a nested value names the array index and the key it was
/// reading, not just the object that contained them.
#[test]
fn deep_failures_carry_the_json_pointer_of_the_field() {
    let document = full_request();

    let error = parse_request(&document.replace("\"min\": \"4h\"", "\"min\": \"1d\"")).unwrap_err();
    assert_eq!(error.code(), "duration_contains_days");
    assert_eq!(error.path(), "/rules/3/params/min");

    let error = parse_request(&document.replace("\"team/sales\"", "\"bad tag\"")).unwrap_err();
    assert_eq!(error.code(), "tag_invalid_character");
    assert_eq!(error.path(), "/participants/0/tags/0");

    let error = parse_request(&document.replace("\"tue\":", "\"mon\":")).unwrap_err();
    assert_eq!(error.code(), "window_weekday_repeated");
    assert_eq!(error.path(), "/rules/0/params/weekly");

    // A selector key the wire does not know fails where the selector is read.
    let error = parse_request(&document.replace(
        "{ \"match\": { \"tag\": \"status/focus\" }, \"effect\": \"ignore\" }",
        "{ \"match\": { \"bogus\": 1 }, \"effect\": \"ignore\" }",
    ))
    .unwrap_err();
    assert_eq!(error.code(), "unknown_field");
    // A rule is an internally tagged enum, and serde buffers its content before
    // reading the variant, so the track stops at the rule object rather than at
    // the field inside it. The message still names the offending key.
    assert_eq!(error.path(), "/rules/4");
    assert!(error.message().contains("bogus"), "{error}");
}

/// A union's mismatched form fails where the union is, and its message names the
/// forms the field accepts rather than the Rust type that was reading them.
#[test]
fn a_union_that_matches_no_form_fails_where_it_was_written() {
    let term = parse_request(&format!(
        r#"{{ "now": "2027-03-09T15:00Z",
              "goal": {{ "parts": [{{ "duration": "30m",
                                    "requirement": {{ "all": [
                                      {{ "person": "ana", "bogus": 1 }}] }} }}],
                        "search_window": {HOUR}, "zone": "UTC" }} }}"#
    ))
    .unwrap_err();
    assert_eq!(term.code(), "invalid_shape");
    assert_eq!(term.path(), "/goal/parts/0/requirement/all/0");
    assert!(term.message().contains("person"), "{term}");
    assert!(!term.message().contains("untagged"), "{term}");

    // A rule's content is buffered by its internally tagged enum, so this
    // pointer names the rule rather than the maximum inside it.
    let maximum = parse_request(&format!(
        r#"{{ "now": "2027-03-09T15:00Z",
              "goal": {{ "parts": [{{ "duration": "30m",
                                    "requirement": {{ "all": [{{ "person": "ana" }}] }} }}],
                        "search_window": {HOUR}, "zone": "UTC" }},
              "rules": [{{ "id": "r", "kind": "lead_time",
                           "params": {{ "min": "1h", "max": {{ "days": 3, "bogus": 1 }} }} }}] }}"#
    ))
    .unwrap_err();
    assert_eq!(maximum.code(), "invalid_shape");
    assert_eq!(maximum.path(), "/rules/0");
    assert!(maximum.message().contains("working_days"), "{maximum}");
}

/// An unknown key at the document root names the document, because the document
/// object is what has the wrong shape.
#[test]
fn an_unknown_key_at_the_root_names_the_document() {
    let error = parse_request(&format!(
        r#"{{ "now": "2027-03-09T15:00Z",
              "goal": {{ "parts": [{{ "duration": "30m",
                                    "requirement": {{ "all": [{{ "person": "ana" }}] }} }}],
                        "search_window": {HOUR}, "zone": "UTC" }},
              "bogus": 1 }}"#
    ))
    .unwrap_err();
    assert_eq!(error.code(), "unknown_field");
    assert_eq!(error.path(), "");
    assert!(error.message().contains("bogus"), "{error}");
}

/// Object keys are escaped as RFC 6901 requires, and a map key is a segment like
/// any other.
#[test]
fn pointers_escape_reserved_characters_in_keys() {
    let calendar = |name: &str| {
        format!(
            r#"{{ "now": "2027-03-09T15:00Z",
                  "goal": {{ "parts": [{{ "duration": "30m",
                                        "requirement": {{ "all": [{{ "person": "ana" }}] }} }}],
                            "search_window": {HOUR}, "zone": "UTC" }},
                  "calendars": {{ "{name}": {{ "zone": "  " }} }} }}"#
        )
    };
    let slashed = parse_request(&calendar("org/west")).unwrap_err();
    assert_eq!(slashed.code(), "zone_name_invalid");
    assert_eq!(slashed.path(), "/calendars/org~1west/zone");
    let tilde = parse_request(&calendar("org~west")).unwrap_err();
    assert_eq!(tilde.path(), "/calendars/org~0west/zone");
    // RFC 6901 leaves the empty reference token legal, and a calendar name may be
    // empty, so the segment stays rather than collapsing into its parent.
    let empty = parse_request(&calendar("")).unwrap_err();
    assert_eq!(empty.code(), "zone_name_invalid");
    assert_eq!(empty.path(), "/calendars//zone");
}

/// The escaping a parse failure spells and the escaping an engine diagnostic
/// spells are one rule, so a handler that resolves either pointer with one resolver
/// lands on the field the caller sent.
///
/// The core builds its own diagnostic pointers with a private escaper, and this
/// crate builds parse pointers with [`join_pointer`]; the two have to agree for the
/// same key, or a finding about `/calendars/org~1west/zone` cannot be resolved in
/// the document whose parse failure says the same field is at `…/org~1west/zone`.
#[cfg(feature = "bundled-tzdb")]
#[test]
fn wire_pointers_and_engine_diagnostics_escape_a_key_the_same_way() {
    use opportune_json::join_pointer;

    let engine = Engine::builder()
        .build()
        .expect("the bundled database builds");
    for name in ["org/west", "org~west", "org/~west", ""] {
        let expected = join_pointer(&join_pointer("/calendars", name), "zone");

        // The wire layer's own pointer: a zone name no document may carry fails
        // during parsing, at the calendar the name selects.
        let unreadable = format!(
            r#"{{ "now": "2027-03-09T15:00Z",
                  "goal": {{ "parts": [{{ "duration": "30m",
                                        "requirement": {{ "all": [{{ "person": "ana" }}] }} }}],
                            "search_window": {HOUR}, "zone": "UTC" }},
                  "calendars": {{ "{name}": {{ "zone": "  " }} }} }}"#
        );
        assert_eq!(
            parse_request(&unreadable).unwrap_err().path(),
            expected,
            "the parse failure for `{name}` names the field",
        );

        // The engine's diagnostic pointer for the same key: a zone name that is
        // well formed but unknown to the database fails validation instead.
        let unknown = format!(
            r#"{{ "now": "2027-03-09T15:00Z",
                  "goal": {{ "parts": [{{ "duration": "30m",
                                        "requirement": {{ "all": [{{ "person": "ana" }}] }} }}],
                            "search_window": {HOUR}, "zone": "UTC" }},
                  "calendars": {{ "{name}": {{ "zone": "Mars/Olympus_Mons" }} }} }}"#
        );
        let request = parse_request(&unknown).expect("the document is structurally valid");
        let diagnostics = engine.validate(&request);
        let finding = diagnostics
            .errors
            .iter()
            .find(|finding| finding.code() == "zone_unknown")
            .unwrap_or_else(|| panic!("no `zone_unknown` finding for `{name}`"));
        assert_eq!(
            finding.path(),
            Some(expected.as_str()),
            "the finding for `{name}` names the same segment the wire layer does",
        );
    }
}

/// A document that is not JSON, or that carries trailing content, is rejected as
/// a whole rather than partly read.
#[test]
fn a_document_that_is_not_json_names_the_document() {
    for document in ["not json", "", "{\"now\": }"] {
        let error = parse_request(document).unwrap_err();
        assert_eq!(error.code(), "invalid_json", "{document}");
    }
    // A document that is not JSON at all has no field to name.
    assert_eq!(parse_request("not json").unwrap_err().path(), "");
    assert_eq!(parse_request("").unwrap_err().path(), "");
    // A document that parses but is not the whole input is rejected too, rather
    // than silently reading the first value and ignoring the rest.
    let trailing = format!("{} {{}}", full_request());
    let error = parse_request(&trailing).unwrap_err();
    assert_eq!(error.code(), "invalid_json");
    assert_eq!(error.path(), "");
}

/// A missing key and a wrong type are distinguished, and a value outside a closed
/// set of strings names that set.
#[test]
fn shape_failures_name_what_was_wrong() {
    let missing = parse_proposal(r#"{"events": [{"part": "main", "start": "2027-03-09T15:00Z"}]}"#)
        .unwrap_err();
    assert_eq!(missing.code(), "missing_field");
    assert_eq!(missing.path(), "/events/0");

    let wrong_type = parse_request(r#"{"now": 5}"#).unwrap_err();
    assert_eq!(wrong_type.code(), "invalid_type");
    assert_eq!(wrong_type.path(), "/now");

    let unknown_variant = parse_request(&format!(
        r#"{{ "now": "2027-03-09T15:00Z",
              "goal": {{ "parts": [{{ "duration": "30m",
                                    "requirement": {{ "all": [{{ "person": "ana" }}] }} }}],
                        "search_window": {HOUR}, "zone": "UTC" }},
              "participants": [{{ "id": "ana", "zone": "UTC", "kind": "robot" }}] }}"#
    ))
    .unwrap_err();
    assert_eq!(unknown_variant.code(), "unknown_variant");
    assert_eq!(unknown_variant.path(), "/participants/0/kind");

    // A rule's tag is a document field like any other, so a typo names it.
    let unknown_kind = parse_request(&format!(
        r#"{{ "now": "2027-03-09T15:00Z",
              "goal": {{ "parts": [{{ "duration": "30m",
                                    "requirement": {{ "all": [{{ "person": "ana" }}] }} }}],
                        "search_window": {HOUR}, "zone": "UTC" }},
              "rules": [{{ "id": "r", "kind": "clearence",
                           "params": {{ "before": "10m", "after": "10m",
                                        "min_gap": "5m" }} }}] }}"#
    ))
    .unwrap_err();
    assert_eq!(unknown_kind.code(), "unknown_variant");
    assert_eq!(unknown_kind.path(), "/rules/0/kind");
    assert!(
        unknown_kind.message().contains("clearence"),
        "{unknown_kind}"
    );
}

/// The selector forms all read, and `false` — which is not a selector — is
/// rejected instead of guessed at.
#[test]
fn every_selector_form_reads_and_false_is_rejected() {
    let rule = parse_rule(
        r#"{ "id": "r", "kind": "clearance",
             "who": { "all": [ { "any": [ { "tag": "a" }, { "ids": ["tom"] } ] },
                               { "not": { "role": "attendee" } } ] },
             "when": true,
             "params": { "before": "10m", "after": "10m", "min_gap": "5m" } }"#,
    )
    .unwrap();
    assert_eq!(
        rule.who,
        Selector::All(vec![
            Selector::Any(vec![
                Selector::Tag(opportune::tags::Tag::new("a").unwrap()),
                Selector::Ids(vec!["tom".to_owned()]),
            ]),
            Selector::Not(Box::new(Selector::Role(Role::Attendee))),
        ])
    );
    assert_eq!(rule.when, Selector::True);

    let written = to_json(&WireRule::from(&rule)).unwrap();
    assert!(written.contains("\"when\":true"), "{written}");
    assert_eq!(parse_rule(&written).unwrap().who, rule.who);

    // A kind selector is one key whose value is the participant kind, and it
    // round-trips as the same selector.
    let kind = parse_rule(
        r#"{ "id": "r", "kind": "window", "who": { "kind": "resource" },
             "params": { "zone": "participant", "weekly": {} } }"#,
    )
    .unwrap();
    assert_eq!(
        kind.who,
        Selector::Kind(opportune::model::ParticipantKind::Resource)
    );
    let written = to_json(&WireRule::from(&kind)).unwrap();
    assert!(
        written.contains(r#""who":{"kind":"resource"}"#),
        "{written}"
    );
    assert_eq!(parse_rule(&written).unwrap().who, kind.who);

    let error = parse_rule(
        r#"{ "id": "r", "kind": "clearance", "who": false,
             "params": { "before": "10m", "after": "10m", "min_gap": "5m" } }"#,
    )
    .unwrap_err();
    assert!(
        error.message().contains("`false` is not a selector"),
        "{error}"
    );

    let two_keys = parse_rule(
        r#"{ "id": "r", "kind": "clearance", "who": { "tag": "a", "ids": ["tom"] },
             "params": { "before": "10m", "after": "10m", "min_gap": "5m" } }"#,
    )
    .unwrap_err();
    assert!(two_keys.message().contains("exactly one key"), "{two_keys}");
}

/// A kind selector reaches the engine from a document: the same room, held
/// tentatively, is ignored by an unscoped policy and blocks under a policy
/// scoped to people.
#[cfg(feature = "bundled-tzdb")]
#[test]
fn a_kind_selector_scopes_a_rule_from_a_document() {
    let engine = Engine::builder()
        .build()
        .expect("the bundled database builds");
    let request = |who: &str| {
        format!(
            r#"{{ "now": "2027-03-09T15:00Z",
                  "goal": {{ "parts": [{{ "duration": "30m",
                                        "requirement": {{ "all": [{{ "person": "room-4" }}] }} }}],
                            "search_window": {HOUR}, "zone": "UTC" }},
                  "participants": [{{ "id": "room-4", "kind": "resource", "zone": "UTC",
                                     "calendar": [{{ "span": ["2027-03-09T15:00Z",
                                                             "2027-03-09T16:00Z"],
                                                     "status": "tentative" }}] }}],
                  "rules": [{{ "id": "r", "kind": "status_policy", "who": {who},
                               "params": {{ "entries": [{{ "status": "tentative",
                                                          "effect": "ignore" }}] }} }}] }}"#
        )
    };
    let starts = |who: &str| -> Vec<String> {
        let written = opportune_json::find_availability(&engine, &request(who))
            .expect("the request is valid");
        let response: Value = serde_json::from_str(&written).expect("a Response document");
        response["offers"]
            .as_array()
            .expect("offers is an array")
            .iter()
            .map(|offer| offer["events"][0]["start"].as_str().unwrap().to_owned())
            .collect()
    };
    // The advisory reaches a client the same way the offers do: a caller that only
    // ever reads the answer is told the policy it wrote also drops a room's hold.
    let warned = |who: &str| -> Vec<(String, Option<String>)> {
        let written = opportune_json::find_availability(&engine, &request(who))
            .expect("the request is valid");
        let response: Value = serde_json::from_str(&written).expect("a Response document");
        response["warnings"]
            .as_array()
            .expect("warnings is an array")
            .iter()
            .map(|warning| {
                (
                    warning["code"].as_str().expect("a code").to_owned(),
                    warning["path"].as_str().map(str::to_owned),
                )
            })
            .collect()
    };

    assert_eq!(
        starts(r#"{ "role": "host" }"#),
        ["2027-03-09T15:00Z", "2027-03-09T15:30Z"],
        "an unscoped policy reads the room's tentative hold as free"
    );
    assert_eq!(
        warned(r#"{ "role": "host" }"#),
        [(
            "status_policy_ignores_resource".to_owned(),
            Some("/rules/0".to_owned())
        )],
        "the response names the policy that drops the room's hold"
    );
    assert!(
        starts(r#"{ "kind": "person" }"#).is_empty(),
        "a policy scoped to people leaves the room's hold blocking"
    );
    assert!(
        warned(r#"{ "kind": "person" }"#).is_empty(),
        "a policy scoped to people is exactly what the caller meant"
    );
}

/// A `kind` in a requirement term selects the participants of that kind, so the
/// wire's non-rule selector position carries it too and it picks rather than only
/// excludes: two free team members of different kinds, and the clause alone decides
/// which one is assigned.
#[cfg(feature = "bundled-tzdb")]
#[test]
fn a_kind_selector_in_a_requirement_term_decides_who_is_assigned() {
    let engine = Engine::builder()
        .build()
        .expect("the bundled database builds");
    let request = |of: &str| {
        format!(
            r#"{{ "now": "2027-03-09T15:00Z",
                  "goal": {{ "parts": [{{ "duration": "30m",
                                        "requirement": {{ "all": [{{ "at_least": 1,
                                                                      "of": {of} }}] }} }}],
                            "search_window": {HOUR}, "zone": "UTC" }},
                  "participants": [{{ "id": "ana", "kind": "person", "zone": "UTC",
                                     "tags": ["team/eng"] }},
                                   {{ "id": "room-4", "kind": "resource", "zone": "UTC",
                                     "tags": ["team/eng"] }}] }}"#
        )
    };
    let assigned = |of: &str| -> Vec<String> {
        let written =
            opportune_json::find_availability(&engine, &request(of)).expect("the request is valid");
        let response: Value = serde_json::from_str(&written).expect("a Response document");
        response["offers"]
            .as_array()
            .expect("offers is an array")
            .iter()
            .take(1)
            .flat_map(|offer| {
                offer["events"]
                    .as_array()
                    .expect("events is an array")
                    .iter()
                    .flat_map(|event| {
                        event["assignment"]
                            .as_array()
                            .expect("assignment is an array")
                            .iter()
                            .flat_map(|term| {
                                term.as_array()
                                    .expect("a term assignment is an array")
                                    .iter()
                                    .map(|id| id.as_str().expect("an ID").to_owned())
                            })
                    })
            })
            .collect()
    };

    assert_eq!(
        assigned(r#"{ "kind": "resource" }"#),
        ["room-4"],
        "the resource term is filled by the room, not by the equally free person"
    );
    assert_eq!(
        assigned(r#"{ "kind": "person" }"#),
        ["ana"],
        "the person term is filled by the person, not by the equally free room"
    );
}

/// A status-policy entry that matches neither a status nor a tag selector could
/// never apply while reading as if it would, so it is rejected where it was
/// written.
#[test]
fn a_status_policy_entry_that_matches_nothing_is_rejected() {
    let error = parse_request(&format!(
        r#"{{ "now": "2027-03-09T15:00Z",
              "goal": {{ "parts": [{{ "duration": "30m",
                                    "requirement": {{ "all": [{{ "person": "ana" }}] }} }}],
                        "search_window": {HOUR}, "zone": "UTC" }},
              "rules": [{{ "id": "r", "kind": "status_policy",
                           "params": {{ "entries": [{{ "effect": "ignore" }}] }} }}] }}"#
    ))
    .unwrap_err();
    assert_eq!(error.code(), "status_policy_entry_empty");
    assert_eq!(error.path(), "/rules/0/params/entries/0");
}

/// A proposal, a ranges query, a goal, a calendar and the limits each round-trip
/// through the wire on their own, because a caller stores them on their own.
#[test]
fn the_standalone_shapes_round_trip() {
    let proposal = Proposal::single(
        "main",
        Instant::from_minutes(Instant::MIN.minutes() + 60).unwrap(),
        vec![vec!["ana".to_owned()]],
    );
    let parsed_proposal =
        parse_proposal(&to_json(&WireProposal::from(&proposal)).unwrap()).unwrap();
    assert_eq!(parsed_proposal, proposal);

    let request = parse_request(&full_request()).unwrap();
    let query = RangesQuery {
        now: request.now,
        goal: request.goal.clone(),
        rules: request.rules.clone(),
        participants: request.participants.clone(),
    };
    assert_eq!(
        parse_ranges_query(&to_json(&WireRangesQuery::from(&query)).unwrap()).unwrap(),
        query
    );

    let goal: Goal = parse_goal(&to_json(&WireGoal::from(&request.goal)).unwrap()).unwrap();
    assert_eq!(goal, request.goal);

    let calendar = &request.calendars["org"];
    assert_eq!(
        parse_calendar(&to_json(&opportune_json::WireCalendarSpec::from(calendar)).unwrap())
            .unwrap(),
        *calendar
    );

    let limits = Limits::default()
        .with_max_participants(50)
        .with_max_search_window_days(31);
    assert_eq!(
        parse_limits(&to_json(&WireLimits::from(&limits)).unwrap()).unwrap(),
        limits
    );
}

/// A calendar that works no weekday at all is representable: the day-list syntax
/// has no spelling for it, so an absent `working_days` is the empty set.
#[test]
fn a_calendar_with_no_working_days_round_trips() {
    let calendar = opportune::model::CalendarSpec {
        zone: opportune::time::ZoneName::new("UTC").unwrap(),
        working_days: Weekdays::NONE,
        holidays: Vec::new(),
    };
    let wire = opportune_json::WireCalendarSpec::from(&calendar);
    assert_eq!(wire.working_days, None);
    assert_eq!(parse_calendar(&to_json(&wire).unwrap()).unwrap(), calendar);
}

/// `MatchAll` and its negation are the two selector spellings every rule may use.
#[test]
fn match_all_writes_as_true_and_reads_only_true() {
    let rule = parse_rule(
        r#"{ "id": "r", "kind": "rest", "who": true, "when": { "not": true },
             "params": { "late_after": "18:00", "not_before": "10:00" } }"#,
    )
    .unwrap();
    let written = to_json(&WireRule::from(&rule)).unwrap();
    assert!(written.contains("\"who\":true"), "{written}");
    assert!(written.contains("\"when\":{\"not\":true}"), "{written}");
    assert_eq!(parse_rule(&written).unwrap(), rule);
}

/// The engine's answer round-trips through the wire with every field the response
/// contract requires, deterministically, and reads back into the same value: the
/// answer types are what an HTTP client builds from the engine's JSON.
#[cfg(feature = "bundled-tzdb")]
#[test]
fn a_response_round_trips_through_the_wire() {
    let request = parse_request(&format!(
        r#"{{ "now": "2027-03-09T15:00Z",
              "goal": {{ "parts": [{{ "duration": "30m", "travel": {{ "before": "10m", "after": "10m" }},
                                    "requirement": {{ "all": [{{ "person": "ana" }}] }} }}],
                        "search_window": {HOUR}, "zone": "UTC" }},
              "participants": [{{ "id": "ana", "zone": "UTC", "calendar": [] }}],
              "rules": [{{ "id": "r_buf", "kind": "clearance",
                           "params": {{ "before": "10m", "after": "10m",
                                        "min_gap": "5m" }} }}],
              "explain": {{ "limit": 10 }} }}"#
    ))
    .unwrap();
    let engine = Engine::builder()
        .build()
        .expect("the bundled database builds");
    let response = engine.find_availability(&request).unwrap();
    let wire = WireResponse::from(&response);
    let written = to_json(&wire).unwrap();
    assert_eq!(
        written,
        to_json(&wire).unwrap(),
        "serialisation is deterministic"
    );
    assert_eq!(
        serde_json::from_str::<WireResponse>(&written).expect("a response reads back"),
        wire,
        "the document the engine wrote is the answer type read back",
    );

    let value: Value = serde_json::from_str(&written).unwrap();
    let object = value.as_object().expect("a response is an object");
    let mut keys: Vec<&str> = object.keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        [
            "dates",
            "excluded",
            "excluded_truncated",
            "meta",
            "offers",
            "truncated",
            "warnings"
        ]
    );
    assert!(object["offers"].is_array());
    assert!(object["dates"].is_array());
    // A date's offer count crosses the boundary as a `u64`, so a target whose
    // `usize` is narrower reads the same document.
    let counts: Vec<opportune_json::WireDateCount> = response
        .dates
        .iter()
        .map(opportune_json::WireDateCount::from)
        .collect();
    let _: Option<u64> = counts.first().map(|count| count.offers);
    assert_eq!(object["truncated"], Value::Bool(false));
    assert_eq!(object["excluded_truncated"], Value::Bool(false));
    assert!(object["warnings"].is_array());
    let meta = object["meta"].as_object().unwrap();
    assert_eq!(meta["engine"], opportune::VERSION);
    assert_eq!(
        meta["schema"],
        Value::from(opportune::model::SCHEMA_VERSION)
    );
    assert_eq!(
        meta["tzdb"],
        Value::from(engine.time_zone_database().version())
    );

    let offer = &object["offers"][0];
    assert_eq!(offer_fields(offer), ["events", "flags"]);
    let event = &offer["events"][0];
    let mut event_keys: Vec<&str> = event
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    event_keys.sort_unstable();
    assert_eq!(
        event_keys,
        ["assignment", "eligible", "end", "part", "reserve", "start"]
    );
    // Travel the booking needs is a reserve span, and the round trip above proved
    // those read back too.
    assert!(
        !event["reserve"].as_array().unwrap().is_empty(),
        "the request's travel is a reserve span on the offer: {event}",
    );
}

/// The answer types accept a field they do not name, so a service reading a newer
/// engine's additive field still deserializes instead of failing on a document it
/// did nothing wrong to receive. The request types keep refusing an unknown key,
/// where the mistake is the caller's.
#[cfg(feature = "bundled-tzdb")]
#[test]
fn a_response_accepts_a_field_a_newer_engine_added() {
    let request = parse_request(&format!(
        r#"{{ "now": "2027-03-09T15:00Z",
              "goal": {{ "parts": [{{ "duration": "30m",
                                    "requirement": {{ "all": [{{ "person": "ana" }}] }} }}],
                        "search_window": {HOUR}, "zone": "UTC" }},
              "participants": [{{ "id": "ana", "zone": "UTC", "calendar": [] }}] }}"#
    ))
    .unwrap();
    let engine = Engine::builder().build().unwrap();
    let wire = WireResponse::from(&engine.find_availability(&request).unwrap());
    let mut value: Value = serde_json::from_str(&to_json(&wire).unwrap()).unwrap();
    value["offers"][0]["events"][0]["added_later"] = Value::from(1);
    value["offers"][0]["added_later"] = Value::from(1);
    value["added_later"] = Value::from(1);

    let read: WireResponse =
        serde_json::from_value(value).expect("an additive field is ignored when read");
    assert_eq!(read, wire);
}

/// Adds an unknown key to `value` and to every answer object nested inside it,
/// which is where a newer engine would add one.
fn with_an_unread_field(value: &mut Value) {
    match value {
        Value::Object(object) => {
            object.insert("added_later".to_owned(), Value::from(1));
            for inner in object.values_mut() {
                with_an_unread_field(inner);
            }
        }
        Value::Array(items) => items.iter_mut().for_each(with_an_unread_field),
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
    }
}

/// The answer `answer` reads back from a document that names one field the type does
/// not, at the answer's own level and inside every answer object it holds.
fn reads_an_additive_field<T>(answer: &T) -> T
where
    T: serde::Serialize + serde::de::DeserializeOwned,
{
    let mut value: Value = serde_json::from_str(&to_json(answer).unwrap()).unwrap();
    with_an_unread_field(&mut value);
    serde_json::from_value(value).expect("an additive field is ignored when an answer is read")
}

/// Every answer type reads a field it does not name, at every level an engine could
/// add one: a slot check, a diagnostics report and a required-ranges envelope are
/// read back by the same clients that read a response, so the tolerance has to hold
/// across all four and inside the pieces they hold.
///
/// The answers are built here rather than taken from an engine so the tolerance is
/// pinned in the stripped build too, where no zone database is compiled in; the
/// engine-produced response above proves the same against the real producer.
#[test]
fn every_answer_type_reads_a_field_a_newer_engine_added() {
    let span = || {
        WireSpan(
            "2027-03-09T15:00Z".to_owned(),
            "2027-03-09T15:45Z".to_owned(),
        )
    };
    let meta = || WireMeta {
        engine: "1.0.0".to_owned(),
        schema: 1,
        tzdb: "2026f".to_owned(),
    };
    let diagnostic = || WireDiagnostic {
        code: "zone_unknown".to_owned(),
        message: "no such zone".to_owned(),
        path: Some("/goal/zone".to_owned()),
    };
    let reason = |params: Option<WireReasonParams>| WireReason {
        participant: Some("ana".to_owned()),
        code: "cap".to_owned(),
        rule: Some("r_cap".to_owned()),
        event_key: None,
        tags: vec!["focus".to_owned()],
        params,
    };

    let response = WireResponse {
        offers: vec![WireOffer {
            events: vec![WireOfferEvent {
                part: "main".to_owned(),
                start: "2027-03-09T15:00Z".to_owned(),
                end: "2027-03-09T15:45Z".to_owned(),
                eligible: vec![vec!["ana".to_owned()]],
                assignment: vec![vec!["ana".to_owned()]],
                reserve: vec![WireReserveSpan {
                    participant: "ana".to_owned(),
                    kind: "travel_before".to_owned(),
                    span: span(),
                }],
            }],
            flags: vec!["travel".to_owned()],
        }],
        dates: vec![WireDateCount {
            date: "2027-03-09".to_owned(),
            offers: 1,
        }],
        truncated: false,
        excluded: vec![WireExcluded {
            start: "2027-03-09T15:45Z".to_owned(),
            part: "main".to_owned(),
            reasons: vec![
                reason(Some(WireReasonParams {
                    used: 1,
                    limit: 1,
                    bucket: "2027-03-09".to_owned(),
                })),
                reason(None),
            ],
        }],
        excluded_truncated: false,
        warnings: vec![diagnostic()],
        meta: meta(),
    };
    let slot_check = WireSlotCheck {
        valid: true,
        reasons: vec![reason(None)],
        flags: vec!["travel".to_owned()],
        reserve: vec![WireReserveSpan {
            participant: "ana".to_owned(),
            kind: "travel_before".to_owned(),
            span: span(),
        }],
        warnings: vec![diagnostic()],
        meta: meta(),
    };
    let diagnostics = WireDiagnostics {
        errors: vec![diagnostic()],
        warnings: vec![diagnostic()],
    };
    let required_ranges = WireRequiredRanges {
        calendar: span(),
        occurrences: None,
    };

    assert_eq!(reads_an_additive_field(&response), response);
    assert_eq!(reads_an_additive_field(&slot_check), slot_check);
    assert_eq!(reads_an_additive_field(&diagnostics), diagnostics);
    assert_eq!(reads_an_additive_field(&required_ranges), required_ranges);
}

/// The keys of an offer, so the assertion names the whole contract once.
#[cfg(feature = "bundled-tzdb")]
fn offer_fields(offer: &Value) -> Vec<&str> {
    let mut keys: Vec<&str> = offer
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    keys
}

/// A slot check always answers with its validity, its reasons, its flags, the
/// travel to reserve, the request's warnings and the versions.
#[cfg(feature = "bundled-tzdb")]
#[test]
fn a_slot_check_serialises_every_required_field() {
    let request = parse_request(&format!(
        r#"{{ "now": "2027-03-09T15:00Z",
              "goal": {{ "parts": [{{ "duration": "30m",
                                    "requirement": {{ "all": [{{ "person": "ana" }}] }} }}],
                        "search_window": {HOUR}, "zone": "UTC" }},
              "participants": [{{ "id": "ana", "zone": "UTC" }}] }}"#
    ))
    .unwrap();
    let engine = Engine::builder().build().unwrap();
    let offer = engine.find_availability(&request).unwrap().offers[0].clone();
    let check: SlotCheck = engine.check_slot(&request, &offer.to_proposal()).unwrap();
    let wire = WireSlotCheck::from(&check);
    let written = to_json(&wire).unwrap();
    let value: Value = serde_json::from_str(&written).unwrap();
    assert_eq!(value["valid"], Value::Bool(true));
    assert!(value["reasons"].is_array());
    assert!(value["flags"].is_array());
    assert!(value["reserve"].is_array());
    assert!(value["warnings"].is_array());
    assert!(value["meta"].is_object());
    assert_eq!(
        serde_json::from_str::<WireSlotCheck>(&written).expect("a slot check reads back"),
        wire,
    );
}

/// Excluded starts explain themselves, and a cap's reason carries the bucket and
/// usage a caller renders.
#[cfg(feature = "bundled-tzdb")]
#[test]
fn excluded_reasons_carry_their_codes_and_cap_parameters() {
    let request = parse_request(&format!(
        r#"{{ "now": "2027-03-09T15:00Z",
              "goal": {{ "parts": [{{ "duration": "30m", "tags": ["interview"],
                                    "requirement": {{ "all": [{{ "person": "ana" }}] }} }}],
                        "search_window": {HOUR}, "zone": "UTC" }},
              "participants": [{{ "id": "ana", "zone": "UTC" }}],
              "rules": [{{ "id": "r_cap", "kind": "cap",
                           "params": {{ "count": {{ "tag": "interview" }},
                                        "measure": "count", "limit": 1,
                                        "period": "day", "per": "each",
                                        "zone": "participant" }} }}],
              "occurrences": [{{ "event_key": "evt_9",
                                 "span": ["2027-03-09T09:00Z", "2027-03-09T09:30Z"],
                                 "tags": ["interview"], "participants": ["ana"] }}],
              "explain": {{ "limit": 10 }} }}"#
    ))
    .unwrap();
    let engine = Engine::builder().build().unwrap();
    let response = engine.find_availability(&request).unwrap();
    let wire = WireResponse::from(&response);
    let written = to_json(&wire).unwrap();
    let value: Value = serde_json::from_str(&written).unwrap();
    assert_eq!(value["offers"].as_array().unwrap().len(), 0);
    let excluded = value["excluded"]
        .as_array()
        .expect("explain lists exclusions");
    assert!(!excluded.is_empty());
    let reason = &excluded[0]["reasons"][0];
    assert_eq!(reason["code"], Value::from("cap_reached"));
    assert_eq!(reason["participant"], Value::from("ana"));
    assert_eq!(reason["rule"], Value::from("r_cap"));
    assert_eq!(reason["params"]["used"], Value::from(1));
    assert_eq!(reason["params"]["limit"], Value::from(1));
    assert_eq!(reason["params"]["bucket"], Value::from("2027-03-09"));
    assert_eq!(excluded[0]["part"], Value::from("main"));
    assert_eq!(
        serde_json::from_str::<WireResponse>(&written).expect("a response reads back"),
        wire,
        "the excluded starts and their cap parameters read back",
    );

    // A blocked start names the event that blocked it, so a caller can render
    // the reason without fetching the calendar again.
    let blocked = parse_request(&format!(
        r#"{{ "now": "2027-03-09T15:00Z",
              "goal": {{ "parts": [{{ "duration": "30m",
                                    "requirement": {{ "all": [{{ "person": "ana" }}] }} }}],
                        "search_window": {HOUR}, "zone": "UTC" }},
              "participants": [{{ "id": "ana", "zone": "UTC",
                                 "calendar": [{{ "span": {HOUR}, "event_key": "evt_1",
                                                "tags": ["veto/crm-freeze"] }}] }}],
              "explain": {{ "limit": 10 }} }}"#
    ))
    .unwrap();
    let response = engine.find_availability(&blocked).unwrap();
    let value: Value =
        serde_json::from_str(&to_json(&WireResponse::from(&response)).unwrap()).unwrap();
    let reason = &value["excluded"][0]["reasons"][0];
    assert_eq!(reason["code"], Value::from("blocked"));
    assert_eq!(reason["event_key"], Value::from("evt_1"));
    assert_eq!(reason["tags"][0], Value::from("veto/crm-freeze"));
}

/// The caller's fetch responsibility serialises as one UTC envelope, with
/// `occurrences` present but `null` when no cap applies.
#[cfg(feature = "bundled-tzdb")]
#[test]
fn required_ranges_serialise_both_edges() {
    let request = parse_request(&format!(
        r#"{{ "now": "2027-03-09T15:00Z",
              "goal": {{ "parts": [{{ "duration": "30m",
                                    "requirement": {{ "all": [{{ "person": "ana" }}] }} }}],
                        "search_window": {HOUR}, "zone": "UTC" }},
              "participants": [{{ "id": "ana", "zone": "UTC" }}] }}"#
    ))
    .unwrap();
    let engine = Engine::builder().build().unwrap();
    let query = RangesQuery {
        now: request.now,
        goal: request.goal.clone(),
        rules: request.rules.clone(),
        participants: request.participants.clone(),
    };
    let ranges = engine.required_ranges(&query).unwrap();
    let wire = WireRequiredRanges::from(&ranges);
    let written = to_json(&wire).unwrap();
    let value: Value = serde_json::from_str(&written).unwrap();
    assert_eq!(value["calendar"].as_array().unwrap().len(), 2);
    assert!(
        !value.as_object().unwrap().contains_key("occurrences"),
        "an absent occurrence range is omitted rather than written as `null`: {written}",
    );
    assert_eq!(
        serde_json::from_str::<WireRequiredRanges>(&written)
            .expect("a required-ranges envelope without occurrences reads back"),
        wire,
    );

    // An explicit `null` is the same document as an omission, so both read back.
    let mut explicit_null = value.as_object().unwrap().clone();
    explicit_null.insert("occurrences".to_owned(), Value::Null);
    assert_eq!(
        serde_json::from_value::<WireRequiredRanges>(Value::Object(explicit_null))
            .expect("an explicit null occurrences reads back"),
        wire,
    );
}

/// `validate` reports its findings with the code, message and pointer of each.
#[cfg(feature = "bundled-tzdb")]
#[test]
fn diagnostics_serialise_their_findings() {
    let request = parse_request(&format!(
        r#"{{ "now": "2027-03-09T15:00Z",
              "goal": {{ "parts": [{{ "duration": "30m",
                                    "requirement": {{ "all": [{{ "person": "ana" }}] }} }}],
                        "search_window": {HOUR}, "zone": "Mars/Olympus_Mons" }},
              "participants": [{{ "id": "ana", "zone": "UTC" }}] }}"#
    ))
    .unwrap();
    let engine = Engine::builder().build().unwrap();
    let diagnostics = engine.validate(&request);
    let wire = opportune_json::WireDiagnostics::from(&diagnostics);
    let written = to_json(&wire).unwrap();
    let value: Value = serde_json::from_str(&written).unwrap();
    let errors = value["errors"].as_array().expect("errors is a list");
    assert_eq!(errors[0]["code"], Value::from("zone_unknown"));
    assert_eq!(errors[0]["path"], Value::from("/goal/zone"));
    assert!(value["warnings"].is_array());
    assert_eq!(
        serde_json::from_str::<opportune_json::WireDiagnostics>(&written)
            .expect("a diagnostics report reads back"),
        wire,
    );
}

/// A rule's parameters reach every `lead_time` maximum form, because the four
/// forms are four different comparisons in the engine.
#[test]
fn every_lead_time_maximum_form_round_trips() {
    let cases = [
        (
            r#""max": { "duration": "2h" }"#,
            LeadTimeMax::Duration(Duration::from_hours(2)),
        ),
        (r#""max": { "days": 3 }"#, LeadTimeMax::Days(3)),
        (
            r#""max": { "date": "2027-03-12" }"#,
            LeadTimeMax::Date(opportune::time::Date::new(2027, 3, 12).unwrap()),
        ),
        (
            r#""max": { "working_days": 2, "calendar": "org" }"#,
            LeadTimeMax::WorkingDays {
                count: 2,
                calendar: "org".to_owned(),
            },
        ),
    ];
    for (params, expected) in cases {
        let document = format!(
            r#"{{ "id": "r", "kind": "lead_time", "params": {{ "min": "1h", {params} }} }}"#
        );
        let rule = parse_rule(&document).unwrap();
        assert_eq!(rule.lead_time().unwrap().max, Some(expected), "{params}");
        assert_eq!(
            parse_rule(&to_json(&WireRule::from(&rule)).unwrap()).unwrap(),
            rule
        );
    }
}

/// The empty requirement and the empty occurrence participant list are rejected
/// where the domain type refuses them.
#[test]
fn a_value_the_domain_type_cannot_hold_is_rejected_at_its_field() {
    let error = parse_request(&format!(
        r#"{{ "now": "2027-03-09T15:00Z",
              "goal": {{ "parts": [{{ "duration": "30m",
                                    "requirement": {{ "all": [{{ "person": "ana" }}] }} }}],
                        "search_window": {HOUR}, "zone": "UTC" }},
              "occurrences": [{{ "event_key": "evt_9", "span": {HOUR},
                                 "participants": [] }}] }}"#
    ))
    .unwrap_err();
    assert_eq!(error.code(), "occurrence_without_participants");
    assert_eq!(error.path(), "/occurrences/0/participants");
}

/// A span is exactly two strings. Every other array length is one failure with
/// one code, including the too-long case serde reports as trailing characters
/// when the shape is derived.
#[test]
fn a_span_is_an_array_of_two_strings() {
    for (window, length) in [
        ("[]", 0),
        (r#"["2027-03-09T15:00Z"]"#, 1),
        (r#"["2027-03-09T15:00Z","2027-03-09T16:00Z","x"]"#, 3),
    ] {
        let error = parse_request(&format!(
            r#"{{ "now": "2027-03-09T15:00Z",
                  "goal": {{ "parts": [{{ "duration": "30m",
                                        "requirement": {{ "all": [{{ "person": "ana" }}] }} }}],
                            "search_window": {window}, "zone": "UTC" }} }}"#
        ))
        .unwrap_err();
        assert_eq!(error.code(), "invalid_length", "{window}");
        assert_eq!(error.path(), "/goal/search_window", "{window}");
        assert!(
            error
                .message()
                .contains(&format!("invalid length {length}")),
            "{error}"
        );
        assert!(
            error.message().contains("an array of two strings"),
            "{error}"
        );
    }
}

/// An inverted or empty span inside a participant is rejected where it was
/// written, whatever kind of span it is.
#[test]
fn an_inverted_zone_period_or_block_span_is_rejected_at_its_field() {
    let request = |body: &str| {
        format!(
            r#"{{ "now": "2027-03-09T15:00Z",
                  "goal": {{ "parts": [{{ "duration": "30m",
                                        "requirement": {{ "all": [{{ "person": "ana" }}] }} }}],
                            "search_window": {HOUR}, "zone": "UTC" }},
                  "participants": [{{ "id": "ana", "zone": "UTC", {body} }}] }}"#
        )
    };
    let period = parse_request(&request(
        r#""zone_periods": [{ "zone": "Europe/London",
                              "from": "2027-03-09T16:00Z", "until": "2027-03-09T15:00Z" }]"#,
    ))
    .unwrap_err();
    assert_eq!(period.code(), "zone_period_empty");
    assert_eq!(period.path(), "/participants/0/zone_periods/0");

    let block = parse_request(&request(
        r#""calendar": [{ "span": ["2027-03-09T17:00Z", "2027-03-09T16:00Z"] }]"#,
    ))
    .unwrap_err();
    assert_eq!(block.code(), "span_reversed");
    assert_eq!(block.path(), "/participants/0/calendar/0/span");
}

/// A status-policy entry may only name a status the engine has.
///
/// A block's status is provider data, so an unrecognized one is coerced
/// fail-closed to `busy`. An entry is the caller's own
/// configuration: coercing a typo there turns "ignore out-of-office events" into
/// "ignore every busy block", which offers time the participant is not free, so
/// the spelling is rejected instead.
#[test]
fn a_status_policy_entry_names_a_status_the_engine_has() {
    for (spelling, status) in [
        ("free", BlockStatus::Free),
        ("tentative", BlockStatus::Tentative),
        ("busy", BlockStatus::Busy),
    ] {
        let rule = parse_rule(&format!(
            r#"{{ "id": "r", "kind": "status_policy",
                  "params": {{ "entries": [{{ "status": "{spelling}",
                                             "effect": "ignore" }}] }} }}"#
        ))
        .unwrap();
        assert_eq!(
            rule.status_policy().unwrap().entries[0].status,
            Some(status),
            "{spelling}"
        );
    }

    let error = parse_request(&format!(
        r#"{{ "now": "2027-03-09T15:00Z",
              "goal": {{ "parts": [{{ "duration": "30m",
                                    "requirement": {{ "all": [{{ "person": "ana" }}] }} }}],
                        "search_window": {HOUR}, "zone": "UTC" }},
              "rules": [{{ "id": "r", "kind": "status_policy",
                           "params": {{ "entries": [{{ "status": "out_of_office",
                                                      "effect": "ignore" }}] }} }}] }}"#
    ))
    .unwrap_err();
    assert_eq!(error.code(), "unknown_variant");
    // The pointer names the rule, because a rule's content is buffered by its
    // internally tagged enum (recorded as a known limitation); the message names
    // the spelling and the set that would have been accepted.
    assert_eq!(error.path(), "/rules/0");
    assert!(error.message().contains("out_of_office"), "{error}");
    assert!(error.message().contains("free"), "{error}");
}

/// A span that covers no minute is rejected at the span, whether it is a search
/// window, a block or an occurrence.
#[test]
fn an_empty_span_is_rejected_at_the_span() {
    let error = parse_request(
        r#"{ "now": "2027-03-09T15:00Z",
              "goal": { "parts": [{ "duration": "30m",
                                    "requirement": { "all": [{ "person": "ana" }] } }],
                        "search_window": ["2027-03-09T15:00Z", "2027-03-09T15:00Z"],
                        "zone": "UTC" } }"#,
    )
    .unwrap_err();
    assert_eq!(error.code(), "span_empty");
    assert_eq!(error.path(), "/goal/search_window");
}

/// The engine facade is the whole request-to-response pipeline in one call, so a
/// binding hands a document over and passes the answer on unchanged: each entry
/// point writes the canonical document the wire types write, and a rejected request
/// keeps every validation finding for the caller to read.
#[cfg(feature = "bundled-tzdb")]
#[test]
fn the_engine_facade_writes_the_canonical_answer() {
    use opportune_json::{check_slot, find_availability, required_ranges, validate};

    let engine = Engine::builder()
        .build()
        .expect("the bundled database builds");
    let request = format!(
        r#"{{ "now": "2027-03-09T15:00Z",
              "goal": {{ "parts": [{{ "duration": "30m",
                                    "requirement": {{ "all": [{{ "person": "ana" }}] }} }}],
                        "search_window": {HOUR}, "zone": "UTC" }},
              "participants": [{{ "id": "ana", "zone": "UTC", "calendar": [] }}],
              "rules": [{{ "id": "w", "kind": "window",
                           "params": {{ "zone": "participant",
                                        "weekly": {{ "mon-fri": ["09:00-17:00"] }} }} }}] }}"#
    );

    // Offers: the document the engine answered with, in the wire's own shape.
    let written = find_availability(&engine, &request).expect("the request is valid");
    let response: Value = serde_json::from_str(&written).expect("a Response document");
    let starts: Vec<&str> = response["offers"]
        .as_array()
        .expect("offers is an array")
        .iter()
        .map(|offer| offer["events"][0]["start"].as_str().unwrap())
        .collect();
    assert_eq!(starts, ["2027-03-09T15:00Z", "2027-03-09T15:30Z"]);
    assert_eq!(
        to_json(&WireResponse::from(
            &engine
                .find_availability(&parse_request(&request).unwrap())
                .unwrap()
        ))
        .unwrap(),
        written,
        "the facade writes the same canonical document the wire value does",
    );

    // Slot check: the same request and the offered start, judged.
    let proposal = r#"{ "events": [{ "part": "main", "start": "2027-03-09T15:00Z",
                                      "assignment": [["ana"]] }] }"#;
    let checked: Value = serde_json::from_str(
        &check_slot(&engine, &request, proposal).expect("the proposal fits the request"),
    )
    .expect("a SlotCheck document");
    assert_eq!(checked["valid"], Value::Bool(true));
    assert!(checked["reasons"].as_array().unwrap().is_empty());

    // Diagnostics: a request validation finds impossible is still an answer.
    let unknown_zone = request.replace(
        "\"id\": \"ana\", \"zone\": \"UTC\"",
        "\"id\": \"ana\", \"zone\": \"Mars/Olympus_Mons\"",
    );
    let diagnostics: Value =
        serde_json::from_str(&validate(&engine, &unknown_zone).expect("the document parses"))
            .expect("a Diagnostics document");
    let errors = diagnostics["errors"]
        .as_array()
        .expect("errors is an array");
    assert_eq!(errors.len(), 1, "{diagnostics}");
    assert_eq!(errors[0]["code"], "zone_unknown");
    assert_eq!(errors[0]["path"], "/participants/0/zone");

    // Required ranges: the same request, as the envelopes a caller must fetch. The
    // window rule reads whole civil days, so the envelope is wider than the search
    // window and `occurrences` is absent because no cap applies.
    let ranges: Value =
        serde_json::from_str(&required_ranges(&engine, &request).expect("the query is valid"))
            .expect("a RequiredRanges document");
    assert_eq!(
        ranges["calendar"],
        Value::from(vec!["2027-03-08T00:00Z", "2027-03-11T00:00Z"]),
        "{ranges}",
    );
    assert_eq!(ranges["occurrences"], Value::Null, "{ranges}");

    // And a request the engine rejects keeps every finding it produced.
    let error = find_availability(&engine, &unknown_zone).unwrap_err();
    assert_eq!(error.code(), "invalid_request");
    let findings = error.errors();
    assert_eq!(findings.len(), 1, "{error}");
    assert_eq!(findings[0].code(), "zone_unknown");
    assert_eq!(findings[0].path(), Some("/participants/0/zone"));
}

/// The wire value a document decodes into, which is what a service embedding the
/// wire types holds: a request body field, not JSON text.
fn wire<T: serde::de::DeserializeOwned>(json: &str) -> T {
    serde_json::from_str(json).expect("the document matches the wire shape")
}

/// The conversions the `parse_*` entry points run are public `TryFrom` impls, so a
/// service that already holds a wire value converts it without writing it back to
/// JSON text and parsing it again. The owned forms forward to the borrowed ones, so
/// a caller that owns its value does not keep it alive only to convert it.
#[test]
fn every_wire_value_converts_to_its_domain_type_without_a_json_round_trip() {
    let request_json = full_request();
    let wire_request: WireRequest = wire(&request_json);
    let parsed = parse_request(&request_json).expect("the full request parses");
    assert_eq!(
        Request::try_from(&wire_request).unwrap(),
        parsed,
        "the borrowed conversion is the one `parse_request` runs",
    );
    assert_eq!(
        Request::try_from(wire_request).unwrap(),
        parsed,
        "the owned conversion forwards to the borrowed one",
    );

    let goal_json = to_json(&WireGoal::from(&parsed.goal)).unwrap();
    let wire_goal: WireGoal = wire(&goal_json);
    assert_eq!(
        Goal::try_from(&wire_goal).unwrap(),
        parse_goal(&goal_json).unwrap(),
    );
    assert_eq!(
        Goal::try_from(wire_goal).unwrap(),
        parse_goal(&goal_json).unwrap(),
    );

    let rule_json = to_json(&WireRule::from(&parsed.rules[0])).unwrap();
    let wire_rule: WireRule = wire(&rule_json);
    assert_eq!(
        Rule::try_from(&wire_rule).unwrap(),
        parse_rule(&rule_json).unwrap(),
    );
    assert_eq!(
        Rule::try_from(wire_rule).unwrap(),
        parse_rule(&rule_json).unwrap(),
    );

    let calendar_json = r#"{
      "zone": "America/New_York",
      "working_days": "mon-fri",
      "holidays": ["2027-01-01"]
    }"#;
    let wire_calendar: WireCalendarSpec = wire(calendar_json);
    assert_eq!(
        CalendarSpec::try_from(&wire_calendar).unwrap(),
        parse_calendar(calendar_json).unwrap(),
    );
    assert_eq!(
        CalendarSpec::try_from(wire_calendar).unwrap(),
        parse_calendar(calendar_json).unwrap(),
    );

    let proposal_json = r#"{
      "events": [{ "part": "main", "start": "2027-03-09T15:00Z", "assignment": [["ana"]] }]
    }"#;
    let wire_proposal: WireProposal = wire(proposal_json);
    assert_eq!(
        Proposal::try_from(&wire_proposal).unwrap(),
        parse_proposal(proposal_json).unwrap(),
    );
    assert_eq!(
        Proposal::try_from(wire_proposal).unwrap(),
        parse_proposal(proposal_json).unwrap(),
    );

    let query = RangesQuery {
        now: parsed.now,
        goal: parsed.goal.clone(),
        rules: parsed.rules.clone(),
        participants: parsed.participants.clone(),
    };
    let query_json = to_json(&WireRangesQuery::from(&query)).unwrap();
    let wire_query: WireRangesQuery = wire(&query_json);
    assert_eq!(
        RangesQuery::try_from(&wire_query).unwrap(),
        parse_ranges_query(&query_json).unwrap(),
    );
    assert_eq!(
        RangesQuery::try_from(wire_query).unwrap(),
        parse_ranges_query(&query_json).unwrap(),
    );

    let limits_json = to_json(&WireLimits::from(&Limits::default())).unwrap();
    let wire_limits: WireLimits = wire(&limits_json);
    assert_eq!(
        Limits::try_from(&wire_limits).unwrap(),
        parse_limits(&limits_json).unwrap(),
    );
    assert_eq!(
        Limits::try_from(wire_limits).unwrap(),
        parse_limits(&limits_json).unwrap(),
    );
}

/// The records a service caches on their own — a calendar row, a participant, an
/// occurrence — convert from the wire value it holds, without the request they
/// arrived in and without writing them back to JSON text. Each conversion is the
/// same walk the request parse runs, which is what the equality with the parsed
/// request pins.
#[test]
fn a_cached_record_converts_without_the_request_it_arrived_in() {
    let request_json = full_request();
    let wire_request: WireRequest = wire(&request_json);
    let parsed = parse_request(&request_json).expect("the full request parses");

    let participant = &wire_request.participants[0];
    assert_eq!(
        Participant::try_from(participant).unwrap(),
        parsed.participants[0],
        "the borrowed conversion is what the request parse runs",
    );
    assert_eq!(
        Participant::try_from(participant.clone()).unwrap(),
        parsed.participants[0],
        "the owned conversion forwards to the borrowed one",
    );

    let block = &participant
        .calendar
        .as_ref()
        .expect("the fixture participant carries a calendar")[0];
    assert_eq!(
        Block::try_from(block).unwrap(),
        parsed.participants[0].calendar.as_ref().unwrap()[0]
    );
    assert_eq!(
        Block::try_from(block.clone()).unwrap(),
        parsed.participants[0].calendar.as_ref().unwrap()[0],
    );

    let occurrence = &wire_request.occurrences[0];
    assert_eq!(
        Occurrence::try_from(occurrence).unwrap(),
        parsed.occurrences[0]
    );
    assert_eq!(
        Occurrence::try_from(occurrence.clone()).unwrap(),
        parsed.occurrences[0],
    );
}

/// A cached record the domain cannot hold is rejected at the field that held it,
/// with the same code and pointer a request parse reports — the service shows its
/// own client the place to fix, not a pointer into a document it never wrote.
#[test]
fn a_rejected_cached_record_names_the_field_at_fault() {
    let reversed: WireBlock = wire(r#"{ "span": ["2027-03-09T17:00Z", "2027-03-09T16:00Z"] }"#);
    let error = Block::try_from(reversed).unwrap_err();
    assert_eq!(error.code(), "span_reversed");
    assert_eq!(error.path(), "/span");

    let travel: WireBlock =
        wire(r#"{ "span": ["2027-03-09T15:00Z", "2027-03-09T16:00Z"], "travel_before": "1d" }"#);
    let error = Block::try_from(travel).unwrap_err();
    assert_eq!(error.path(), "/travel_before");

    let nameless: WireParticipant = wire(r#"{ "id": "ana", "zone": "  " }"#);
    let error = Participant::try_from(&nameless).unwrap_err();
    assert_eq!(error.code(), "zone_name_invalid");
    assert_eq!(error.path(), "/zone");

    let keyless: WireOccurrence = wire(
        r#"{ "event_key": "", "span": ["2027-03-09T15:00Z", "2027-03-09T16:00Z"],
              "participants": ["ana"] }"#,
    );
    let error = Occurrence::try_from(keyless).unwrap_err();
    assert_eq!(error.code(), "event_key_empty");
    assert_eq!(error.path(), "/event_key");
}

/// A conversion reports the same code and pointer `parse_*` does, because it is the
/// same walk: a service that hands a wire value over gets the field it would show
/// its client.
#[test]
fn a_rejected_wire_value_names_the_field_at_fault() {
    let unaligned: WireProposal = wire(
        r#"{
          "events": [{
            "part": "main",
            "start": "2027-03-09T15:00:30Z",
            "assignment": [["ana"]]
          }]
        }"#,
    );
    let error = Proposal::try_from(unaligned).unwrap_err();
    assert_eq!(error.code(), "instant_not_aligned");
    assert_eq!(error.path(), "/events/0/start");
}

/// The U2 schedule the Python and TypeScript presets build from the same inputs:
/// per-weekday hours, a split shift, a midnight-crossing range, a closed holiday
/// and a date override, scoped to one attendee and one kind of booking.
///
/// The three bindings spell the preset differently — Rust takes day sets, Python
/// and TypeScript wire day keys — so the document is what has to agree, and each
/// suite holds this same JSON value against its own builder.
const U2_WEEKLY_HOURS: &str = r#"{
  "id": "weekly_hours",
  "kind": "window",
  "who": { "ids": ["ana"] },
  "when": { "tag": "interview" },
  "params": {
    "zone": "event",
    "weekly": {
      "mon": ["09:00-17:00"],
      "tue": ["09:00-12:00", "14:00-18:00"],
      "wed": ["09:00-17:00"],
      "thu": ["10:00-16:00"],
      "fri": ["22:00-02:00"]
    },
    "dates": {
      "2027-06-15": ["10:00-12:00"],
      "2027-12-25": []
    }
  }
}"#;

fn wall(from: (u8, u8), until: (u8, u8)) -> WallRange {
    WallRange::new(
        TimeOfDay::new(from.0, from.1).unwrap(),
        WallEnd::Time(TimeOfDay::new(until.0, until.1).unwrap()),
    )
    .unwrap()
}

#[test]
fn the_weekly_hours_preset_writes_the_document_every_binding_reads() {
    let day = |weekday: Weekday, hours: Vec<WallRange>| {
        WeeklyKey::new(Weekdays::single(weekday), hours).unwrap()
    };
    let rule = presets::weekly_hours(
        RuleZone::Event,
        vec![
            day(Weekday::Monday, vec![wall((9, 0), (17, 0))]),
            day(
                Weekday::Tuesday,
                vec![wall((9, 0), (12, 0)), wall((14, 0), (18, 0))],
            ),
            day(Weekday::Wednesday, vec![wall((9, 0), (17, 0))]),
            day(Weekday::Thursday, vec![wall((10, 0), (16, 0))]),
            day(Weekday::Friday, vec![wall((22, 0), (2, 0))]),
        ],
        vec![
            DateKey::new(
                DateRange::single(Date::new(2027, 6, 15).unwrap()),
                vec![wall((10, 0), (12, 0))],
            )
            .unwrap(),
            DateKey::new(
                DateRange::single(Date::new(2027, 12, 25).unwrap()),
                Vec::new(),
            )
            .unwrap(),
        ],
    )
    .unwrap()
    .with_who(Selector::ids(["ana"]))
    .with_when(Selector::Tag(Tag::new("interview").unwrap()));

    assert_eq!(
        serde_json::to_value(WireRule::from(&rule)).unwrap(),
        serde_json::from_str::<Value>(U2_WEEKLY_HOURS).unwrap(),
    );
}

/// A rule object rejects a key it does not name, which the bundle states as
/// `additionalProperties: false` on every variant.
///
/// serde cannot put `deny_unknown_fields` on the tagged enum itself, so each
/// variant struct carries it and the enum's buffered content still goes through
/// that struct's visitor. The schema's closure rests on this, so it is pinned
/// here rather than assumed.
#[test]
fn a_key_beside_kind_is_rejected_on_a_rule_object() {
    let document = r#"{ "id": "r", "kind": "rest", "typo": 1,
                       "params": { "late_after": "18:00", "not_before": "10:00" } }"#;
    let error = parse_rule(document).expect_err("a rule names no `typo`");
    assert_eq!(error.code(), "unknown_field");
    assert!(error.message().contains("typo"), "{error}");
    assert_eq!(
        error.path(),
        "",
        "the rule is the whole document, so the pointer names it",
    );

    // Inside a request the same failure names the rule's position, for the same
    // reason every other rejection inside a rule does.
    let error = parse_request(&format!(
        r#"{{ "now": "2027-03-09T15:00Z",
              "goal": {{ "parts": [{{ "duration": "30m",
                                    "requirement": {{ "all": [{{ "person": "ana" }}] }} }}],
                        "search_window": {HOUR}, "zone": "UTC" }},
              "rules": [{{ "id": "r", "kind": "rest", "typo": 1,
                          "params": {{ "late_after": "18:00", "not_before": "10:00" }} }}] }}"#
    ))
    .expect_err("the rule names no `typo`");
    assert_eq!(error.code(), "unknown_field");
    assert_eq!(error.path(), "/rules/0");
}

/// A `lead_time` rule whose `who` is `depth` nested `not`s around `true`.
fn nested_who_rule(depth: usize) -> WireRule {
    let mut who = WireSelector::MatchAll(MatchAll);
    for _ in 0..depth {
        who = WireSelector::Not(Box::new(who));
    }
    WireRule::LeadTime(LeadTimeRule {
        id: "r".to_owned(),
        who,
        when: WireSelector::MatchAll(MatchAll),
        params: LeadTimeParams {
            min: "1h".to_owned(),
            max: None,
        },
    })
}

/// A selector is the wire's one recursive shape, so a caller that builds one in
/// memory and converts it — `Rule::try_from`, with no `serde_json` nesting limit
/// in the way — gets a rejection rather than a stack overflow.
#[test]
fn a_selector_nested_past_the_bound_is_rejected_rather_than_overflowing() {
    let error = Rule::try_from(&nested_who_rule(70)).expect_err("70 levels is past the bound");
    assert_eq!(error.code(), "selector_too_deep");
    assert_eq!(
        error.path(),
        format!("/who{}", "/not".repeat(64)),
        "the pointer names the level that went past the bound",
    );

    // One level short of the bound converts, so the guard is a ceiling and not a
    // blanket refusal of `not`.
    let rule = Rule::try_from(&nested_who_rule(63)).expect("63 levels is inside the bound");
    let mut selector = &rule.who;
    let mut nots = 0;
    while let Selector::Not(inner) = selector {
        nots += 1;
        selector = inner;
    }
    assert_eq!(nots, 63, "the conversion kept every level");
    assert_eq!(
        *selector,
        Selector::True,
        "the innermost selector is the `true` the chain was built around",
    );
}
