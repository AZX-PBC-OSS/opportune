//! Adversarial probes over the wire layer's edges: a document arriving at a
//! trust boundary must be rejected rather than silently reinterpreted, must never
//! abort the process, and must survive the wire round trip when it is accepted.
//!
//! Every case here attacks a specific assumption the layer makes rather than
//! restating the contract tests in `wire_conversion_test.rs`.

use opportune_json::{WireRequest, parse_calendar, parse_goal, parse_request, parse_rule, to_json};

/// A minimal request around `participants`, `rules` and any trailing members.
fn request_with(participants: &str, rules: &str, extra: &str) -> String {
    format!(
        r#"{{ "now": "2027-03-09T15:00Z",
              "goal": {{ "parts": [{{ "duration": "30m",
                                    "requirement": {{ "all": [{{ "person": "ana" }}] }} }}],
                        "search_window": ["2027-03-09T15:00Z","2027-03-09T16:00Z"],
                        "zone": "UTC" }},
              "participants": [{{ {participants} }}],
              "rules": [{rules}]{extra} }}"#
    )
}

/// A `status_policy` entry is caller-authored configuration, not provider data,
/// so a status the layer does not know must not become `busy`.
///
/// A `status_policy` entry is caller-authored configuration, so a status the wire
/// does not know is rejected instead of being read as `busy`. Reading it as `busy`
/// is not a harmless default: an entry written to ignore "out of office" blocks
/// becomes an entry that ignores *every busy block*, and the engine then offers
/// times the participant is not free.
#[cfg(feature = "bundled-tzdb")]
#[test]
fn an_unknown_status_in_a_status_policy_entry_does_not_become_busy() {
    let document = |status: &str| {
        format!(
            r#"{{ "now": "2027-03-09T14:00Z",
                  "goal": {{ "parts": [{{ "duration": "30m",
                                        "requirement": {{ "all": [{{ "person": "ana" }}] }} }}],
                            "search_window": ["2027-03-09T15:00Z","2027-03-09T17:00Z"],
                            "zone": "UTC" }},
                  "participants": [{{ "id": "ana", "zone": "UTC",
                                     "calendar": [{{ "span": ["2027-03-09T15:00Z","2027-03-09T16:00Z"],
                                                    "status": "busy" }}] }}],
                  "rules": [{{ "id": "p", "kind": "status_policy",
                               "params": {{ "entries": [{{ "status": "{status}",
                                                          "effect": "ignore" }}] }} }}] }}"#
        )
    };
    let engine = opportune::Engine::builder().build().unwrap();
    let offers = |status: &str| {
        let request =
            parse_request(&document(status)).unwrap_or_else(|err| panic!("{status}: {err}"));
        let response = engine
            .find_availability(&request)
            .unwrap_or_else(|err| panic!("{status}: {err}"));
        response
            .offers
            .iter()
            .map(|offer| offer.events[0].start.to_string())
            .collect::<Vec<_>>()
    };

    // Every status the wire knows reads as itself, so the entry names a status the
    // block does not carry and the block still blocks.
    for known in ["free", "tentative"] {
        assert_eq!(
            offers(known),
            ["2027-03-09T16:00Z", "2027-03-09T16:30Z"],
            "`{known}` must not match the busy block"
        );
    }
    // `busy` deliberately does match it, which is what the coercion would have
    // made of *any* spelling — the over-availability this guard exists to stop.
    assert_eq!(
        offers("busy"),
        [
            "2027-03-09T15:00Z",
            "2027-03-09T15:30Z",
            "2027-03-09T16:00Z",
            "2027-03-09T16:30Z"
        ],
        "an entry naming `busy` ignores the busy block, by design"
    );

    // A spelling the wire does not know never reaches the engine.
    let error = parse_request(&document("out_of_office")).unwrap_err();
    assert_eq!(error.code(), "unknown_variant");
    assert!(error.message().contains("out_of_office"), "{error}");
}

/// The supported range and the offset arithmetic, checked at the edges an offset
/// can push a reading across: a wrong sign or a wrong minute count moves every
/// instant a non-UTC caller sends.
#[test]
fn offsets_convert_to_the_same_instant() {
    use opportune_json::primitives::{InstantRounding, parse_instant};
    let exact = |input: &str| {
        parse_instant(input, InstantRounding::Exact, "/now")
            .map(|instant| instant.to_string())
            .ok()
    };
    for input in [
        "2027-03-09T15:00:00Z",
        "2027-03-09T20:45:00+05:45",
        "2027-03-09T11:30:00-03:30",
        "2027-03-10T04:45:00+13:45",
        "2027-03-08T23:30:00-15:30",
    ] {
        assert_eq!(
            exact(input).as_deref(),
            Some("2027-03-09T15:00Z"),
            "{input}"
        );
    }
    // The two ends of the supported range, reached through an offset.
    assert_eq!(
        exact("1975-01-01T05:45+05:45").as_deref(),
        Some("1975-01-01T00:00Z")
    );
    assert_eq!(
        exact("2099-12-31T18:15-05:45").as_deref(),
        Some("2100-01-01T00:00Z")
    );
    // Rounding still happens in the direction asked for after the offset is
    // applied: a fraction of a second carries `now` into the next minute.
    assert_eq!(
        parse_instant(
            "2027-03-09T20:45:00.001+05:45",
            InstantRounding::Ceil,
            "/now"
        )
        .map(|instant| instant.to_string())
        .as_deref()
        .ok(),
        Some("2027-03-09T15:01Z")
    );
}

/// RFC 3339 section 5.6 note 2 lets the `T` and `Z` be lower case, and the
/// specification asks for RFC 3339, so a producer emitting either case must read.
#[test]
fn a_lower_case_rfc_3339_timestamp_reads() {
    use opportune_json::primitives::{InstantRounding, parse_instant};
    let exact = |input: &str| {
        parse_instant(input, InstantRounding::Exact, "/now")
            .map(|instant| instant.to_string())
            .ok()
    };
    for input in [
        "2027-03-09t15:00:00z",
        "2027-03-09T15:00:00z",
        "2027-03-09t15:00:00Z",
    ] {
        assert_eq!(
            exact(input).as_deref(),
            Some("2027-03-09T15:00Z"),
            "{input}"
        );
    }
    // A lower-case offset still has to be a real offset.
    assert!(
        parse_instant("2027-03-09t15:00:00q", InstantRounding::Exact, "/now").is_err(),
        "a bogus offset must not be read as UTC"
    );
}

/// The time-of-day parser's code is one a caller matches on, so it is pinned
/// where the wire reaches it (a `rest` rule's clock times).
#[test]
fn a_malformed_time_of_day_names_its_field_and_code() {
    let error = parse_request(&request_with(
        "\"id\":\"ana\",\"zone\":\"UTC\"",
        r#"{"id":"r","kind":"rest","params":{"late_after":"25:00","not_before":"10:00"}}"#,
        "",
    ))
    .unwrap_err();
    assert_eq!(error.code(), "time_of_day_invalid");
    assert_eq!(error.path(), "/rules/0/params/late_after");

    // `24:00` is a valid end of a wall range, never a start.
    let error = parse_request(&request_with(
        "\"id\":\"ana\",\"zone\":\"UTC\"",
        r#"{"id":"w","kind":"window","params":{"zone":"UTC","weekly":{"mon":["09:00-24:00"]},"dates":{}}}"#,
        "",
    ));
    assert!(error.is_ok(), "24:00 is a valid range end: {error:?}");
}

/// A document that is not the wire shape is rejected, and the number of digits a
/// caller can put in a field never aborts the process.
#[test]
fn hostile_documents_are_rejected_rather_than_read() {
    let invalid = [
        String::new(),
        "\u{feff}".to_owned(),
        "null".to_owned(),
        "[]".to_owned(),
        r#"{"now":null}"#.to_owned(),
        r#"{"now":"2027-03-09T15:00Z","goal":[]}"#.to_owned(),
        request_with("", "", ""),
        request_with("\"id\":\"ana\",\"zone\":\"\"", "", ""),
        request_with("\"id\":\"ana\",\"zone\":\"UTC\",\"zone\":\"UTC\"", "", ""),
        request_with(
            "\"id\":\"ana\",\"zone\":\"UTC\",\"calendar\":[null]",
            "",
            "",
        ),
        request_with(
            "\"id\":\"ana\",\"zone\":\"UTC\",\"calendar\":[{\"span\":[\"2027-03-09T16:00Z\",\"2027-03-09T15:00Z\"]}]",
            "",
            "",
        ),
        request_with(
            "\"id\":\"ana\",\"zone\":\"UTC\"",
            "",
            r#","occurrences":[{"event_key":"e","span":["2027-03-09T15:00Z","2027-03-09T15:00Z"],"participants":["ana"]}]"#,
        ),
    ];
    for document in invalid {
        let error = parse_request(&document)
            .err()
            .unwrap_or_else(|| panic!("accepted: {document}"));
        assert!(!error.code().is_empty());
    }

    // Values at the edge of their types are legal documents; they must parse
    // without overflowing anything, whatever the engine then makes of them.
    let extreme = [
        request_with(
            "\"id\":\"ana\",\"zone\":\"UTC\"",
            r#"{"id":"r","kind":"clearance","params":{"before":"4294967295h","after":"0m","min_gap":"0m"}}"#,
            "",
        ),
        request_with(
            "\"id\":\"ana\",\"zone\":\"UTC\"",
            r#"{"id":"r","kind":"cap","params":{"count":true,"measure":"count","limit":0,"period":"day","per":"each","zone":"participant"}}"#,
            "",
        ),
        request_with(
            "\"id\":\"ana\",\"zone\":\"UTC\"",
            "",
            r#","weights":{"ana":-2147483648}"#,
        ),
        request_with(
            "\"id\":\"ana\",\"zone\":\"UTC\"",
            "",
            r#","limit":18446744073709551615"#,
        ),
        request_with(
            "\"id\":\"ana\",\"zone\":\"UTC\"",
            "",
            r#","explain":{"limit":18446744073709551615}"#,
        ),
    ];
    for document in extreme {
        let _ = parse_request(&document);
    }

    // A selector nested past serde_json's recursion limit is a rejection, not a
    // stack overflow.
    let mut selector = String::from("true");
    for _ in 0..500 {
        selector = format!("{{\"not\":{selector}}}");
    }
    let deep = request_with("\"id\":\"ana\",\"zone\":\"UTC\"", "", "").replace(
        r#""requirement": { "all": [{ "person": "ana" }] }"#,
        &format!(r#""requirement": {{ "all": [{{ "at_least": 1, "of": {selector} }}] }}"#),
    );
    let error = parse_request(&deep).unwrap_err();
    assert!(
        error.message().contains("recursion limit"),
        "expected serde_json's own limit to stop it: {error}"
    );
}

/// Every spelling the wire accepts has to survive being written back out: a
/// default the encoder drops, or a key it renames on one side only, would cost a
/// caller data on a round trip while every other test still passed.
#[test]
fn accepted_documents_round_trip_through_the_wire() {
    let documents = [
        // A closed day key beside a closed date, and an empty day set.
        request_with(
            "\"id\":\"ana\",\"zone\":\"UTC\"",
            r#"{"id":"w","kind":"window","params":{"zone":"participant","weekly":{"mon":[]},"dates":{"2026-12-25":[]}}}"#,
            r#","calendars":{"":{"zone":"UTC","working_days":"sat,sun","holidays":[]}}"#,
        ),
        // Crossing ranges, `24:00`, and an empty key set.
        request_with(
            "\"id\":\"ana\",\"zone\":\"UTC\"",
            r#"{"id":"w","kind":"window","params":{"zone":"UTC","weekly":{"mon-fri":["09:00-17:00","22:00-02:00"]},"dates":{}}}"#,
            "",
        ),
        // Selectors with no members are still selectors.
        request_with(
            "\"id\":\"ana\",\"zone\":\"UTC\"",
            r#"{"id":"w","kind":"window","who":true,"when":{"not":true},"params":{"zone":"UTC","weekly":{},"dates":{"2027-03-08..2027-03-10":["00:00-24:00"]}}}"#,
            r#","occurrences":[{"event_key":"e","span":["2027-03-09T15:00Z","2027-03-09T15:30Z"],"tags":[],"participants":["ana"]}]"#,
        ),
        // Zero travel, an empty calendar (known and empty) and a negative weight.
        request_with(
            "\"id\":\"ana\",\"zone\":\"UTC\",\"calendar\":[]",
            r#"{"id":"c","kind":"cap","who":{"all":[]},"params":{"count":{"not":true},"measure":"count","limit":0,"period":"month","per":"pooled","zone":"event"}}"#,
            r#","weights":{"ana":-7},"limit":0,"explain":{"limit":0}"#,
        ),
        // A block whose travel is written as zero, and an all-day free block.
        request_with(
            "\"id\":\"ana\",\"zone\":\"UTC\",\"calendar\":[{\"span\":[\"2027-03-09T15:00Z\",\"2027-03-09T15:01Z\"],\"status\":\"free\",\"all_day\":true,\"travel_before\":\"0m\",\"travel_after\":\"0m\"}]",
            "",
            "",
        ),
    ];
    for document in documents {
        let request = parse_request(&document).unwrap_or_else(|err| panic!("{document}: {err}"));
        let written = to_json(&WireRequest::from(&request)).unwrap();
        let reparsed = parse_request(&written).unwrap_or_else(|err| panic!("{written}: {err}"));
        assert_eq!(reparsed, request, "{written}");
        // And the written form is stable: a second pass changes nothing.
        assert_eq!(to_json(&WireRequest::from(&reparsed)).unwrap(), written);
    }
}

/// A validation diagnostic's pointer is passed through the wire envelope
/// unchanged, so the escaping the engine applied to a calendar name that carries
/// a pointer character has to survive serialisation: this is the output half of
/// pointer fidelity, where the parse half is already covered.
#[cfg(feature = "bundled-tzdb")]
#[test]
fn engine_diagnostic_pointers_keep_their_escaping_through_the_wire() {
    use opportune_json::WireDiagnostics;
    use serde_json::Value;

    let document = r#"{
      "now": "2027-03-09T15:00Z",
      "goal": { "parts": [{ "duration": "30m",
                           "requirement": { "all": [{ "person": "ana" }] } }],
                "search_window": ["2027-03-09T15:00Z","2027-03-09T16:00Z"], "zone": "UTC" },
      "participants": [{ "id": "ana", "zone": "UTC" }],
      "calendars": { "org/west": { "zone": "Mars/Olympus_Mons" },
                     "org~east": { "zone": "Mars/Olympus_Mons" } }
    }"#;
    // A calendar name is an opaque map key, and a zone name that is well formed
    // but unknown is the engine's finding, not the parser's.
    let request = parse_request(document).unwrap();
    let engine = opportune::Engine::builder().build().unwrap();
    let written = to_json(&WireDiagnostics::from(&engine.validate(&request))).unwrap();
    let value: Value = serde_json::from_str(&written).unwrap();
    let paths: Vec<&str> = value["errors"]
        .as_array()
        .unwrap()
        .iter()
        .map(|error| error["path"].as_str().unwrap())
        .collect();
    assert!(paths.contains(&"/calendars/org~1west/zone"), "{written}");
    assert!(paths.contains(&"/calendars/org~0east/zone"), "{written}");
}

/// `null` and an absent `calendar` both mean "nothing was supplied", which is a
/// different fact from an empty one: only `[]` tells the engine the calendar is
/// known and free, which is what decides the `calendar_unknown` flag.
#[test]
fn a_null_calendar_is_no_calendar_not_an_empty_one() {
    let absent = request_with("\"id\":\"ana\",\"zone\":\"UTC\"", "", "");
    let null = request_with("\"id\":\"ana\",\"zone\":\"UTC\",\"calendar\":null", "", "");
    let empty = request_with("\"id\":\"ana\",\"zone\":\"UTC\",\"calendar\":[]", "", "");

    for document in [&absent, &null] {
        let request = parse_request(document).unwrap();
        assert!(request.participants[0].calendar.is_none(), "{document}");
        // ... and it is written back as absent, never as an empty calendar.
        let written = to_json(&WireRequest::from(&request)).unwrap();
        assert!(!written.contains("\"calendar\""), "{written}");
    }
    let request = parse_request(&empty).unwrap();
    assert_eq!(request.participants[0].calendar.as_deref(), Some(&[][..]));
    assert!(
        to_json(&WireRequest::from(&request))
            .unwrap()
            .contains("\"calendar\":[]"),
        "a known-empty calendar must stay distinguishable"
    );
}

/// The only wire name a status-policy entry takes from `rename_all` rather than
/// an explicit `as_str` is its status, so every spelling the entry may name is
/// written and read back — a renamed variant would otherwise pass every other
/// test by never being exercised.
#[test]
fn every_status_a_policy_entry_may_name_round_trips() {
    for status in ["free", "tentative", "busy"] {
        let document = request_with(
            "\"id\":\"ana\",\"zone\":\"UTC\"",
            &format!(
                r#"{{"id":"s","kind":"status_policy","params":{{"entries":[{{"status":"{status}","match":{{"tag":"x"}},"effect":"block"}}]}}}}"#
            ),
            "",
        );
        let request = parse_request(&document).unwrap_or_else(|err| panic!("{status}: {err}"));
        let written = to_json(&WireRequest::from(&request)).unwrap();
        assert!(
            written.contains(&format!("\"status\":\"{status}\"")),
            "{written}"
        );
        assert_eq!(
            parse_request(&written).unwrap(),
            request,
            "`{status}` must survive the round trip"
        );
    }
}

/// Every text parser is public API, so arbitrary text reaches each of them from a
/// binding or an adapter. No input may abort the process, and the spellings that
/// cannot mean anything are rejected rather than defaulted.
#[test]
fn the_text_parsers_reject_rather_than_abort_on_hostile_text() {
    use opportune_json::primitives::{
        InstantRounding, parse_date, parse_date_range, parse_duration, parse_instant,
        parse_local_date_time, parse_time_of_day, parse_wall_end, parse_wall_range, parse_weekday,
        parse_weekdays, parse_working_days,
    };

    // Panic-freedom first: these include inputs each parser *does* accept, so the
    // only assertion is that the call returns at all.
    let any_text = [
        "",
        " ",
        "\u{0}",
        "é",
        "éééé",
        "T",
        "z",
        "Z",
        "t",
        "2027-03-09",
        "2027-03-09T",
        "2027-03-09T15:00Z",
        "2027-03-09t15:00z",
        "2027-03-09T15:00+5:00",
        "2027-03-09T15:00:00.1234567890Z",
        "1975-01-01T00:00:00.000000001Z",
        "9999-12-31T23:59:59Z",
        "10000-01-01T00:00Z",
        "00:00-24:00",
        "24:00-24:00",
        "mon-fri-sat",
        ",,,",
        "4294967296h",
        "99999999999999999999m",
    ];
    for input in any_text {
        for rounding in [
            InstantRounding::Exact,
            InstantRounding::Ceil,
            InstantRounding::Floor,
        ] {
            let _ = parse_instant(input, rounding, "/now");
        }
        let _ = parse_duration(input, "/d");
        let _ = parse_date(input, "/d");
        let _ = parse_date_range(input, "/d");
        let _ = parse_time_of_day(input, "/t");
        let _ = parse_wall_end(input, "/t");
        let _ = parse_wall_range(input, "/w");
        let _ = parse_local_date_time(input, "/l");
        let _ = parse_weekday(input, "/w");
        let _ = parse_weekdays(input, "/w");
        let _ = parse_working_days(input, "/w");
    }

    // A unit of the wrong kind is never guessed at: none of these can be an
    // instant, a date, a duration, a clock time, a wall range or a weekday.
    for input in ["", " ", "\u{0}", "é", "T", "z", "-", "..", "1d"] {
        assert!(
            parse_instant(input, InstantRounding::Exact, "/now").is_err(),
            "instant {input:?}"
        );
        assert!(parse_date(input, "/d").is_err(), "date {input:?}");
        assert!(
            parse_date_range(input, "/d").is_err(),
            "date range {input:?}"
        );
        assert!(parse_duration(input, "/d").is_err(), "duration {input:?}");
        assert!(parse_time_of_day(input, "/t").is_err(), "time {input:?}");
        assert!(parse_wall_end(input, "/t").is_err(), "wall end {input:?}");
        assert!(
            parse_wall_range(input, "/w").is_err(),
            "wall range {input:?}"
        );
        assert!(
            parse_local_date_time(input, "/l").is_err(),
            "local date-time {input:?}"
        );
        assert!(parse_weekday(input, "/w").is_err(), "weekday {input:?}");
        assert!(parse_weekdays(input, "/w").is_err(), "weekdays {input:?}");
    }
}

/// The standalone entry points are public API too, and a binding will hand each of
/// them whatever the caller stored. Each reads its own shape and nothing else: a
/// document for another shape is a shape failure at the document, not a partial
/// read.
#[test]
fn every_standalone_entry_point_reads_only_its_own_shape() {
    use opportune_json::{
        JsonError, parse_calendar, parse_goal, parse_limits, parse_proposal, parse_ranges_query,
        parse_rule,
    };

    const ENTRY_POINTS: &[&str] = &[
        "rule",
        "calendar",
        "goal",
        "proposal",
        "limits",
        "ranges_query",
    ];

    fn entry_point(entry: &str, document: &str) -> Result<(), JsonError> {
        match entry {
            "rule" => parse_rule(document).map(|_| ()),
            "calendar" => parse_calendar(document).map(|_| ()),
            "goal" => parse_goal(document).map(|_| ()),
            "proposal" => parse_proposal(document).map(|_| ()),
            "limits" => parse_limits(document).map(|_| ()),
            "ranges_query" => parse_ranges_query(document).map(|_| ()),
            other => panic!("`{other}` is not an entry point"),
        }
    }

    let goal = r#"{"parts":[{"duration":"30m","requirement":{"all":[{"person":"ana"}]}}],"search_window":["2027-03-09T15:00Z","2027-03-09T16:00Z"],"zone":"UTC"}"#;
    let shapes: &[(&str, &str)] = &[
        (
            "rule",
            r#"{"id":"r","kind":"rest","params":{"late_after":"18:00","not_before":"10:00"}}"#,
        ),
        ("calendar", r#"{"zone":"UTC","working_days":"mon-fri"}"#),
        ("goal", goal),
        (
            "proposal",
            r#"{"events":[{"part":"main","start":"2027-03-09T15:00Z","assignment":[["ana"]]}]}"#,
        ),
        (
            "limits",
            r#"{"max_participants":1,"max_blocks_per_participant":2,"max_search_window_days":3,"max_candidate_starts":4,"max_explain_limit":5}"#,
        ),
        (
            "ranges_query",
            r#"{"now":"2027-03-09T15:00Z","goal":{"parts":[{"duration":"30m","requirement":{"all":[{"person":"ana"}]}}],"search_window":["2027-03-09T15:00Z","2027-03-09T16:00Z"],"zone":"UTC"}}"#,
        ),
    ];

    for (owner, document) in shapes {
        for entry in ENTRY_POINTS {
            let outcome = entry_point(entry, document);
            if entry == owner {
                outcome.unwrap_or_else(|err| panic!("`{owner}` must read its own document: {err}"));
                continue;
            }
            let error = outcome.expect_err("no entry point reads another shape's document");
            assert!(
                matches!(
                    error.code(),
                    "unknown_field"
                        | "missing_field"
                        | "invalid_type"
                        | "invalid_json"
                        | "invalid_value"
                        | "invalid_length"
                ),
                "a {owner} document read as {entry}: {} at {:?}",
                error.code(),
                error.path()
            );
            assert_eq!(
                error.path(),
                "",
                "a document-level shape failure names the document"
            );
        }
    }
}

/// The wire layer accepts what is structurally representable and leaves the
/// rest to `validate`; this is the seam where a wire-accepted-but-degenerate
/// request would abort the process if the engine indexed into it.
#[cfg(feature = "bundled-tzdb")]
#[test]
fn degenerate_requests_fail_validation_rather_than_the_process() {
    let engine = opportune::Engine::builder().build().unwrap();
    let request = |parts: &str| {
        format!(
            r#"{{"now":"2027-03-09T15:00Z",
                 "goal":{{"parts":[{parts}],"search_window":["2027-03-09T15:00Z","2027-03-09T16:00Z"],"zone":"UTC"}},
                 "participants":[{{"id":"ana","zone":"UTC"}}]}}"#
        )
    };
    // The data envelope depends on the part count but not on the terms or the
    // length, so only the first case has to fail there.
    let cases = [
        // No part at all: the engine must not index `parts[0]`.
        (request(""), true),
        // No term, a part of no length, and a term larger than any request.
        (
            request(r#"{"duration":"30m","requirement":{"all":[]}}"#),
            false,
        ),
        (
            request(r#"{"duration":"0m","requirement":{"all":[{"person":"ana"}]}}"#),
            false,
        ),
        (
            request(
                r#"{"duration":"30m","requirement":{"all":[{"at_least":4294967295,"of":true}]}}"#,
            ),
            false,
        ),
    ];
    for (document, envelope_fails) in cases {
        let request = parse_request(&document).unwrap_or_else(|err| panic!("{document}: {err}"));
        let diagnostics = engine.validate(&request);
        assert!(!diagnostics.errors.is_empty(), "{document}");
        assert!(
            engine.find_availability(&request).is_err(),
            "{document}: an invalid request must not produce offers"
        );
        let envelope = engine.required_ranges(&opportune::model::RangesQuery {
            now: request.now,
            goal: request.goal.clone(),
            rules: request.rules.clone(),
            participants: request.participants.clone(),
        });
        assert_eq!(
            envelope.is_err(),
            envelope_fails,
            "{document}: the envelope must answer for the part count only"
        );
    }
}

/// A count the document carries is read as `u64`, so a request naming four billion
/// offers or explanations is the same document on `wasm32` as on a 64-bit server.
///
/// Reading it as pointer-width `usize` made it a parse failure on one target and a
/// validation finding on the other, which is the sort of difference a caller cannot
/// see from its own side of the boundary.
#[test]
fn a_count_beyond_four_billion_reads_the_same_on_every_target() {
    let document = request_with(
        "\"id\":\"ana\",\"zone\":\"UTC\"",
        "",
        r#","limit":4294967296,"explain":{"limit":4294967296}"#,
    );
    // The wire value keeps what the document wrote, whatever this target can hold.
    let wire: WireRequest = serde_json::from_str(&document).unwrap();
    assert_eq!(wire.limit, Some(4_294_967_296));
    assert_eq!(
        wire.explain.map(|explain| explain.limit),
        Some(4_294_967_296)
    );
    // The widths are the interface, and a `usize` here would compile and pass on a
    // 64-bit host while failing on `wasm32`, so each is named.
    let _: Option<u64> = wire.limit;
    let _: Option<u64> = wire.explain.map(|explain| explain.limit);

    // The domain's `usize` saturates where the target cannot hold the count, so the
    // request parses either way and the engine's configured limits decide.
    let request = parse_request(&document).expect("a count past a 32-bit `usize` still parses");
    assert_eq!(
        request.limit,
        Some(usize::try_from(4_294_967_296u64).unwrap_or(usize::MAX)),
    );
    assert_eq!(
        request.explain.map(|explain| explain.limit),
        Some(usize::try_from(4_294_967_296u64).unwrap_or(usize::MAX)),
    );

    // `u64::MAX` is a legal document too, and it parses without overflowing.
    let widest = request_with(
        "\"id\":\"ana\",\"zone\":\"UTC\"",
        "",
        r#","limit":18446744073709551615,"explain":{"limit":18446744073709551615}"#,
    );
    let request = parse_request(&widest).expect("`u64::MAX` is a count, not a shape failure");
    assert_eq!(request.limit, Some(usize::MAX));
    assert_eq!(
        request.explain.map(|explain| explain.limit),
        Some(usize::MAX)
    );
}

/// An `explain.limit` past what a 32-bit `usize` holds is the configured-limits
/// finding, not a deserialisation failure: the same request has to fail the same way
/// in a browser and on a server.
#[cfg(feature = "bundled-tzdb")]
#[test]
fn an_explain_limit_past_four_billion_is_the_configured_limits_finding() {
    let engine = opportune::Engine::builder().build().unwrap();
    let request = parse_request(&request_with(
        "\"id\":\"ana\",\"zone\":\"UTC\"",
        "",
        r#","explain":{"limit":4294967296}"#,
    ))
    .expect("the document parses; the limit is validation's question");

    let diagnostics = engine.validate(&request);
    let finding = diagnostics
        .errors
        .iter()
        .find(|finding| finding.code() == "explain_limit")
        .unwrap_or_else(|| panic!("no `explain_limit` finding: {:?}", diagnostics.errors));
    assert_eq!(finding.path(), Some("/explain/limit"));

    // And through a search, where the same finding is reported as the request being
    // invalid rather than as something the document got wrong.
    let error = engine.find_availability(&request).unwrap_err();
    assert_eq!(error.code(), "invalid_request");
}

/// A rule's limits are counts too, so a caller that raises one past a 32-bit
/// `usize` configures the same engine on every target.
#[test]
fn a_configured_limit_beyond_four_billion_reads_the_same_on_every_target() {
    use opportune_json::{WireLimits, parse_limits};

    let document = r#"{
      "max_participants": 4294967296,
      "max_blocks_per_participant": 18446744073709551615,
      "max_search_window_days": 92,
      "max_candidate_starts": 4294967296,
      "max_explain_limit": 4294967296
    }"#;
    let wire: WireLimits = serde_json::from_str(document).unwrap();
    assert_eq!(wire.max_participants, Some(4_294_967_296));
    assert_eq!(wire.max_blocks_per_participant, Some(u64::MAX));
    assert_eq!(wire.max_candidate_starts, Some(4_294_967_296));
    assert_eq!(wire.max_explain_limit, Some(4_294_967_296));
    // Every limit is optional on the wire, so each field keeps the width the
    // document wrote rather than a pointer-width `usize`.
    let _: Option<u64> = wire.max_participants;
    let _: Option<u64> = wire.max_blocks_per_participant;
    let _: Option<u64> = wire.max_candidate_starts;
    let _: Option<u64> = wire.max_explain_limit;
    let _: Option<u32> = wire.max_search_window_days;
    let _: Option<u32> = wire.max_duration_minutes;

    let limits = parse_limits(document).expect("a limit past a 32-bit `usize` still parses");
    assert_eq!(
        limits.max_participants,
        usize::try_from(4_294_967_296u64).unwrap_or(usize::MAX),
    );
    assert_eq!(limits.max_blocks_per_participant, usize::MAX);
    // The window-length cap is a domain `u32` rather than a count, so it is read as
    // one and its own value survives unchanged.
    assert_eq!(limits.max_search_window_days, 92);
}

/// A binding that writes every key presents `null` where a human would leave the
/// field out. Both have to mean the default the specification states, or the same
/// request is accepted in one language and rejected in another.
#[test]
fn an_explicit_null_is_the_default_the_specification_states() {
    use opportune::model::BlockStatus;
    use opportune::tags::{Role, Selector};
    use opportune_json::parse_ranges_query;

    let document = r#"{
      "now": "2027-03-09T15:00Z",
      "goal": {
        "parts": [{ "tags": null, "duration": "30m",
                    "requirement": { "all": [{ "person": "ana" }] },
                    "travel": { "before": "0m", "after": "0m", "by_participant": null } }],
        "search_window": ["2027-03-09T15:00Z", "2027-03-09T16:00Z"],
        "zone": "UTC"
      },
      "participants": [{ "id": "ana", "zone": "UTC", "tags": null,
                         "zone_periods": null,
                         "calendar": [{ "span": ["2027-03-09T15:00Z", "2027-03-09T16:00Z"],
                                        "status": null, "all_day": null, "tags": null }] }],
      "rules": [{ "id": "w", "kind": "window", "who": null, "when": null,
                  "params": { "zone": "UTC", "weekly": null, "dates": null } }],
      "occurrences": null,
      "calendars": { "org": { "zone": "UTC", "working_days": "mon-fri",
                                "holidays": null } },
      "weights": null
    }"#;
    let request = parse_request(document).expect("every explicit null is its default");

    // The participant's and the part's lists default to empty.
    assert!(request.participants[0].tags.is_empty());
    assert!(request.participants[0].zone_periods.is_empty());
    assert!(request.goal.parts[0].tags.is_empty());
    assert!(request.goal.parts[0].travel.by_participant.is_empty());
    // A block is `busy`, not all-day and untagged unless the document says otherwise.
    let block = &request.participants[0].calendar.as_ref().unwrap()[0];
    assert_eq!(block.status, BlockStatus::Busy);
    assert!(!block.all_day);
    assert!(block.tags.is_empty());
    // A rule constrains every host and every booking.
    assert_eq!(request.rules[0].who, Selector::Role(Role::Host));
    assert_eq!(request.rules[0].when, Selector::True);
    // A window with no hours is a window that shares none, not a failure.
    let window = request.rules[0].window().expect("the rule is a window");
    assert!(window.weekly().iter().all(Vec::is_empty));
    assert!(window.dates().is_empty());
    // The request-level collections and maps default to empty.
    assert!(request.occurrences.is_empty());
    assert!(request.calendars["org"].holidays.is_empty());
    assert!(request.weights.is_empty());

    // A whole list written as `null` is the empty list, so a binding that fills
    // every key in with `null` still presents a valid request.
    let empty = parse_request(
        r#"{ "now": "2027-03-09T15:00Z",
              "goal": { "parts": [{ "duration": "30m",
                                    "requirement": { "all": [{ "person": "ana" }] } }],
                        "search_window": ["2027-03-09T15:00Z", "2027-03-09T16:00Z"],
                        "zone": "UTC" },
              "participants": null, "rules": null, "occurrences": null,
              "calendars": null, "weights": null }"#,
    )
    .expect("a null is the empty default");
    assert!(empty.participants.is_empty());
    assert!(empty.rules.is_empty());
    assert!(empty.occurrences.is_empty());
    assert!(empty.calendars.is_empty());
    assert!(empty.weights.is_empty());

    // The ranges query carries the same two lists, so a null means the same thing
    // at that entry point.
    let query = parse_ranges_query(
        r#"{ "now": "2027-03-09T15:00Z",
              "goal": { "parts": [{ "duration": "30m",
                                    "requirement": { "all": [{ "person": "ana" }] } }],
                        "search_window": ["2027-03-09T15:00Z", "2027-03-09T16:00Z"],
                        "zone": "UTC" },
              "rules": null, "participants": null }"#,
    )
    .expect("a null list is an empty one");
    assert!(query.rules.is_empty());
    assert!(query.participants.is_empty());

    // The whole document round-trips: the defaults the parser applied are what the
    // wire writes back, so a null never survives as one.
    let written = to_json(&WireRequest::from(&request)).expect("the request writes");
    assert!(!written.contains("null"), "{written}");
    assert_eq!(parse_request(&written).unwrap(), request);
}

/// An occurrence's tags are one of the lists a binding that writes every key
/// spells as `null`, so an occurrence written that way is untagged rather than
/// unreadable.
#[test]
fn an_occurrence_written_with_null_tags_is_untagged() {
    let request = parse_request(&request_with(
        "\"id\":\"ana\",\"zone\":\"UTC\"",
        "",
        r#", "occurrences": [{ "event_key": "e",
                                 "span": ["2027-03-09T15:00Z", "2027-03-09T15:30Z"],
                                 "tags": null, "participants": ["ana"] }]"#,
    ))
    .expect("a null tag list is an empty one");

    assert!(request.occurrences[0].tags.is_empty());
}

/// Every kind of rule reads a `null` `who` and `when` as the defaults the schema
/// advertises, not only the `window` one a request happens to carry: the default
/// is the `deserialize_who`/`deserialize_when` pair each rule kind states for
/// itself, and one left out of a kind is a field the bundle says accepts `null`
/// and the parser does not.
#[test]
fn every_rule_kind_reads_a_null_who_and_when_as_its_defaults() {
    use opportune::tags::{Role, Selector};

    let kinds = [
        ("window", r#"{ "zone": "UTC" }"#),
        ("blackout", r#"{ "zone": "UTC", "spans": [] }"#),
        (
            "clearance",
            r#"{ "before": "0m", "after": "0m", "min_gap": "0m" }"#,
        ),
        ("lead_time", r#"{ "min": "0m" }"#),
        ("status_policy", r#"{ "entries": [] }"#),
        (
            "rest",
            r#"{ "late_after": "17:00", "not_before": "09:00" }"#,
        ),
        (
            "cap",
            r#"{ "count": true, "limit": 1, "measure": "count",
                 "per": "each", "period": "day", "zone": "UTC" }"#,
        ),
    ];

    for (kind, params) in kinds {
        let document = format!(
            r#"{{ "id": "r", "kind": "{kind}", "who": null, "when": null, "params": {params} }}"#
        );
        let rule =
            opportune_json::parse_rule(&document).unwrap_or_else(|error| panic!("{kind}: {error}"));
        assert_eq!(rule.who, Selector::Role(Role::Host), "{kind}");
        assert_eq!(rule.when, Selector::True, "{kind}");
    }
}

/// Neither date nor clock fields have a sign, and the integer parser accepts one, so
/// a signed digit has to be rejected where the field is read rather than silently
/// treated as the unsigned number it resembles.
#[test]
fn a_signed_digit_is_neither_a_date_nor_a_time() {
    use opportune_json::primitives::{parse_date, parse_time_of_day};

    for input in ["+123-01-01", "2027-+1-09", "2027-01-+1", "+123"] {
        assert_eq!(
            parse_date(input, "/date").unwrap_err().code(),
            "date_invalid",
            "{input}",
        );
    }
    for input in ["09:+0", "+9:00", "09:-0"] {
        assert_eq!(
            parse_time_of_day(input, "/anchor").unwrap_err().code(),
            "time_of_day_invalid",
            "{input}",
        );
    }

    // Through the wire: a blackout span is a local date-time and a rest rule reads
    // two clock times, so both spellings are reachable from a document.
    let error = parse_request(&request_with(
        "\"id\":\"ana\",\"zone\":\"UTC\"",
        r#"{"id":"b","kind":"blackout","params":{"zone":"UTC","spans":[["+123-12-24T00:00","2026-12-25T00:00"]]}}"#,
        "",
    ))
    .unwrap_err();
    assert_eq!(error.code(), "date_invalid");
    assert_eq!(error.path(), "/rules/0/params/spans/0/0");

    let error = parse_request(&request_with(
        "\"id\":\"ana\",\"zone\":\"UTC\"",
        r#"{"id":"r","kind":"rest","params":{"late_after":"09:+0","not_before":"10:00"}}"#,
        "",
    ))
    .unwrap_err();
    assert_eq!(error.code(), "time_of_day_invalid");
    assert_eq!(error.path(), "/rules/0/params/late_after");
}

/// Defaulted fields now read through an intermediate `Option`, so the pointer must
/// still name the element at fault rather than stopping at the field: a binding
/// points its caller at the value to fix, and a pointer that names the whole list
/// is a pointer the caller cannot act on.
///
/// Each shape a defaulted field takes is probed — a list of objects, a list of
/// strings inside one of those objects, a map of numbers, and a map of objects
/// inside a defaulted map — because the tracking has to survive the wrapper at
/// every one of them, not only the first.
#[test]
fn a_bad_element_inside_a_defaulted_field_still_names_its_own_pointer() {
    let cases: [(&str, &str); 4] = [
        // A list of objects.
        (
            &request_with("\"id\": 1, \"zone\": \"UTC\"", "", ""),
            "/participants/0/id",
        ),
        // A list of scalars inside one of those objects.
        (
            &request_with("\"id\": \"ana\", \"zone\": \"UTC\", \"tags\": [1]", "", ""),
            "/participants/0/tags/0",
        ),
        // A map of scalars.
        (
            &request_with(
                "\"id\": \"ana\", \"zone\": \"UTC\"",
                "",
                r#","weights": {"ana": "heavy"}"#,
            ),
            "/weights/ana",
        ),
        // A map of objects, itself the defaulted field.
        (
            &request_with(
                "\"id\": \"ana\", \"zone\": \"UTC\"",
                "",
                r#","occurrences": [{
                     "event_key": "e", "span": ["2027-03-09T15:00Z", "2027-03-09T16:00Z"],
                     "tags": 1, "participants": ["ana"] }]"#,
            ),
            "/occurrences/0/tags",
        ),
    ];
    for (document, pointer) in cases {
        let error = parse_request(document).unwrap_err();
        assert_eq!(error.code(), "invalid_type", "{document}");
        assert_eq!(error.path(), pointer, "{document}");
    }
}

/// The four facade entry points are one pipeline in one call, so their answers have
/// to agree with the steps a caller would otherwise run: a binding may use either,
/// and a caller reading `errors` must see the same findings it would get from the
/// diagnostics document.
#[cfg(feature = "bundled-tzdb")]
#[test]
fn the_facade_reports_every_finding_the_diagnostics_document_carries() {
    use opportune_json::{find_availability, validate};

    let engine = opportune::Engine::builder().build().unwrap();
    let document = r#"{ "now": "2027-03-09T15:00Z",
          "goal": { "parts": [{ "duration": "30m",
                                "requirement": { "all": [{ "person": "ana" }] } }],
                    "search_window": ["2027-03-09T15:00Z", "2027-03-09T16:00Z"],
                    "zone": "UTC" },
          "participants": [{ "id": "ana", "zone": "Mars/Olympus_Mons" },
                            { "id": "bo", "zone": "Mars/Arsia_Mons" }] }"#;

    let diagnostics: serde_json::Value =
        serde_json::from_str(&validate(&engine, document).expect("the document parses")).unwrap();
    let written = diagnostics["errors"]
        .as_array()
        .expect("errors is an array");
    assert!(
        written.len() >= 2,
        "both unknown zones are findings: {diagnostics}"
    );

    let error = find_availability(&engine, document).unwrap_err();
    assert_eq!(error.code(), "invalid_request");
    let findings = error.errors();
    assert_eq!(
        findings.len(),
        written.len(),
        "the search reports the findings the diagnostics document does: {error}",
    );
    for (finding, expected) in findings.iter().zip(written) {
        assert_eq!(finding.path(), expected["path"].as_str(), "{error}");
        assert_eq!(Some(finding.code()), expected["code"].as_str(), "{error}");
    }
    let paths: Vec<Option<&str>> = findings.iter().map(opportune::Diagnostic::path).collect();
    assert!(paths.contains(&Some("/participants/0/zone")), "{paths:?}");
    assert!(paths.contains(&Some("/participants/1/zone")), "{paths:?}");
}

/// A proposal the request does not fit is the caller's own document's problem, so the
/// facade has to keep both the code and the pointer into the proposal that the engine
/// reported: a binding passes one document and needs the pointer to name a field in
/// the document it just sent.
#[cfg(feature = "bundled-tzdb")]
#[test]
fn the_facade_keeps_a_proposal_fault_at_the_proposal_field() {
    use opportune_json::check_slot;

    let engine = opportune::Engine::builder().build().unwrap();
    let request = request_with("\"id\":\"ana\",\"zone\":\"UTC\"", "", "");
    let proposal = r#"{ "events": [{ "part": "other", "start": "2027-03-09T15:00Z",
                                      "assignment": [["ana"]] }] }"#;
    let error = check_slot(&engine, &request, proposal).unwrap_err();
    assert_eq!(error.code(), "proposal_part_unknown");
    assert_eq!(error.path(), "/events/0/part");

    // A request that does not validate is the request's own answer, with its
    // findings, even though the proposal would fit.
    let unknown_zone = request.replace("\"zone\":\"UTC\"", "\"zone\":\"Mars/Olympus_Mons\"");
    let error = check_slot(&engine, &unknown_zone, proposal).unwrap_err();
    assert_eq!(error.code(), "invalid_request");
    assert!(
        error
            .errors()
            .iter()
            .any(|finding| finding.code() == "zone_unknown"),
        "{error}",
    );
}

/// The documents the width work exists for have to survive the whole pipeline, not
/// only the parser: a count is clamped into the domain's `usize` and then read by
/// guards that decide how much of the answer to keep, so the ends of that clamp are
/// where an arithmetic guard would show up.
///
/// `limit` is compared against the number of offers already kept rather than used to
/// reserve anything, so the floor is the case that matters: `0` must mean "keep none"
/// and say so with `truncated`, never "unlimited" and never "nothing was free".
#[cfg(feature = "bundled-tzdb")]
#[test]
fn the_ends_of_the_count_clamp_reach_the_engine_with_their_meanings() {
    use opportune_json::find_availability;

    let engine = opportune::Engine::builder().build().unwrap();
    let document = |limit: &str| {
        format!(
            r#"{{ "now": "2027-03-09T12:00Z",
                  "goal": {{ "parts": [{{ "duration": "30m",
                                        "requirement": {{ "all": [{{ "person": "ana" }}] }} }}],
                            "search_window": ["2027-03-09T15:00Z","2027-03-09T16:00Z"],
                            "zone": "UTC" }},
                  "participants": [{{ "id": "ana", "zone": "UTC" }}]{limit} }}"#
        )
    };
    let search = |limit: &str| {
        let written = find_availability(&engine, &document(limit))
            .unwrap_or_else(|err| panic!("`{limit}`: {err}"));
        let response: serde_json::Value =
            serde_json::from_str(&written).expect("a Response document");
        (
            response["offers"].as_array().map(Vec::len),
            response["truncated"].as_bool(),
        )
    };

    // The default, and a count past a 32-bit `usize`: two half-hour offers fit, and
    // neither limit cuts the list.
    assert_eq!(search(""), (Some(2), Some(false)));
    assert_eq!(search(r#", "limit": 4294967296"#), (Some(2), Some(false)));
    assert_eq!(
        search(r#", "limit": 18446744073709551615"#),
        (Some(2), Some(false))
    );

    // The floor: none kept, and the answer says the list was cut rather than that
    // the search found no free time.
    assert_eq!(search(r#", "limit": 0"#), (Some(0), Some(true)));

    // The ceiling of `explain` is the configured-limits finding, reached before any
    // work is done for it, so the widest legal document is answered rather than
    // spending the search's memory on it.
    let error = find_availability(
        &engine,
        &document(r#", "explain": { "limit": 18446744073709551615 }"#),
    )
    .unwrap_err();
    assert_eq!(error.code(), "invalid_request");
    assert!(
        error
            .errors()
            .iter()
            .any(|finding| finding.code() == "explain_limit"
                && finding.path() == Some("/explain/limit")),
        "{error}",
    );
}

/// A configured limit raised past what a 32-bit `usize` holds configures the same
/// engine a 64-bit target builds, so the configured-limits guards must compare
/// against it without reserving anything up front: a guard that pre-allocated would
/// turn an operator's generous ceiling into a failed search.
#[cfg(feature = "bundled-tzdb")]
#[test]
fn a_configured_limit_past_four_billion_searches_like_the_default_one() {
    use opportune_json::parse_limits;

    let wide = parse_limits(
        r#"{ "max_participants": 18446744073709551615,
              "max_blocks_per_participant": 18446744073709551615,
              "max_search_window_days": 4294967295,
              "max_candidate_starts": 18446744073709551615,
              "max_explain_limit": 18446744073709551615 }"#,
    )
    .expect("a limit past a 32-bit `usize` still parses");
    let limited = opportune::Engine::builder().limits(wide).build().unwrap();
    let default = opportune::Engine::builder().build().unwrap();

    let document = request_with("\"id\":\"ana\",\"zone\":\"UTC\"", "", "");
    let offers = |engine: &opportune::Engine| {
        let request = parse_request(&document).expect("the request parses");
        let response = engine
            .find_availability(&request)
            .unwrap_or_else(|err| panic!("{err}"));
        let written = to_json(&opportune_json::WireResponse::from(&response)).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&written).unwrap();
        (
            parsed["offers"].as_array().map(Vec::len),
            parsed["truncated"].clone(),
            parsed["dates"].clone(),
        )
    };
    assert_eq!(offers(&limited), offers(&default));
}

/// A proposal whose required travel has no `Span` past the end of the supported
/// range is rejected with the engine's own code and the pointer into the
/// proposal, so a wire caller learns the same thing a Rust caller does. The
/// search withholds the same start rather than offering a booking it cannot
/// describe, and reports no exclusion for it: the range ending is not a rule the
/// caller wrote.
#[cfg(feature = "bundled-tzdb")]
#[test]
fn an_out_of_range_proposal_keeps_its_code_and_pointer_through_the_wire() {
    let engine = opportune::Engine::builder().build().unwrap();
    let request = r#"{
      "now": "2099-12-31T22:00Z",
      "goal": {
        "parts": [{ "duration": "30m",
                    "requirement": { "all": [{ "person": "ana" }] },
                    "travel": { "before": "0m", "after": "1h" } }],
        "candidates": ["2099-12-31T23:30Z"],
        "search_window": ["2099-12-31T22:00Z", "2100-01-01T00:00Z"],
        "zone": "UTC"
      },
      "explain": { "limit": 10 },
      "participants": [{ "id": "ana", "zone": "UTC", "calendar": [] }]
    }"#;
    let proposal = r#"{
      "events": [{ "part": "main", "start": "2099-12-31T23:30Z",
                   "assignment": [["ana"]] }]
    }"#;

    let error = opportune_json::check_slot(&engine, request, proposal)
        .expect_err("the travel after the booking has no span past the range");
    assert_eq!(error.code(), "proposal_out_of_range");
    assert_eq!(error.path(), "/events/0/start");
    assert!(error.message().contains("ana"), "{}", error.message());

    let written = opportune_json::find_availability(&engine, request).expect("the search runs");
    let response: serde_json::Value = serde_json::from_str(&written).unwrap();
    assert_eq!(response["offers"], serde_json::json!([]));
    assert_eq!(response["excluded"], serde_json::json!([]));
}

/// Every branch of the shape-failure classification has a document that reaches
/// it, and each branch reports its own code.
///
/// The classifier reads serde's message prefix, so a branch with no document is a
/// branch whose behaviour a caller cannot rely on: an unexercised one could be
/// merged into another, or lost to a reworded message, without any test noticing.
/// This gate is the list of branches in `classify` (`src/error.rs`), one document
/// each, in the order the function tests them.
#[test]
fn every_shape_failure_branch_is_classified_into_its_own_code() {
    // `invalid_json`: not JSON at all, which serde reports as a syntax error.
    let invalid_json = parse_calendar("").unwrap_err();
    // `unknown_field`: a key the shape does not name.
    let unknown_field = parse_calendar(r#"{ "zone": "UTC", "bogus": 1 }"#).unwrap_err();
    // `missing_field`: a key the shape requires.
    let missing_field = parse_calendar(r"{ }").unwrap_err();
    // `duplicate_field`: the same key twice.
    let duplicate_field = parse_calendar(r#"{ "zone": "UTC", "zone": "UTC" }"#).unwrap_err();
    // `unknown_variant`: a string outside a closed set.
    let unknown_variant =
        parse_rule(r#"{ "id": "r", "kind": "clearence", "params": {} }"#).unwrap_err();
    // `invalid_type`: the right key holding the wrong JSON type.
    let invalid_type = parse_calendar(r#"{ "zone": 5 }"#).unwrap_err();
    // `invalid_value`: a custom visitor's rejection, which has no serde category of
    // its own — `false` is not a selector.
    let invalid_value = parse_rule(
        r#"{ "id": "r", "kind": "lead_time", "who": false, "params": { "min": "1h" } }"#,
    )
    .unwrap_err();
    // `invalid_length`: an array that is not the arity the shape states.
    let invalid_length = parse_goal(
        r#"{ "parts": [],
             "search_window": ["2027-03-09T15:00Z"],
             "zone": "UTC" }"#,
    )
    .unwrap_err();
    // `invalid_shape`: an untagged union no form of which matched — a term that
    // names a person and a key no form accepts.
    let invalid_shape = parse_request(
        r#"{ "now": "2027-03-09T15:00Z",
             "goal": { "parts": [{ "duration": "30m",
                                   "requirement": { "all": [{ "person": "ana", "bogus": 1 }] } }],
                       "search_window": ["2027-03-09T15:00Z", "2027-03-09T16:00Z"],
                       "zone": "UTC" } }"#,
    )
    .unwrap_err();

    let classified = [
        ("invalid_json", &invalid_json),
        ("unknown_field", &unknown_field),
        ("missing_field", &missing_field),
        ("duplicate_field", &duplicate_field),
        ("unknown_variant", &unknown_variant),
        ("invalid_type", &invalid_type),
        ("invalid_value", &invalid_value),
        ("invalid_length", &invalid_length),
        ("invalid_shape", &invalid_shape),
    ];
    for (expected, error) in classified {
        assert_eq!(
            error.code(),
            expected,
            "the branch reports its own code: {error}"
        );
    }

    // A duplicate and an unknown key agree on the object, which is what a handler
    // has to point its caller at; the message carries the key in both.
    assert_eq!(duplicate_field.path(), "");
    assert!(duplicate_field.message().contains("`zone`"));
    assert_eq!(unknown_field.path(), "");
    assert!(unknown_field.message().contains("bogus"));
}
