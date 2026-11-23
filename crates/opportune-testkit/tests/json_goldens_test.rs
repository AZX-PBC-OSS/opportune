//! The JSON wire layer's exact round trip over the whole scenario suite, and the
//! committed golden corpus.
//!
//! The round trip is the claim the corpus rests on: every scenario the suite
//! carries, in every zone of the specification's matrix, survives
//! `domain → JSON → domain` as an exactly equal domain value, so no consumer of
//! the wire layer can be reading a subtly different request than the fixtures
//! pin. The corpus then freezes what the engine answers over that layer, byte for
//! byte, so another language can compare its own answers against the same text.
//!
//! The corpus tests read the committed files rather than the in-memory case list:
//! a file that drifted from the engine, or that no longer holds what the exporter
//! writes, has to fail here rather than be regenerated into agreement.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use opportune::Engine;
use opportune::model::Request;
use opportune::time::TimeZoneDatabase;
use opportune_json::{
    WireRequest, WireRequiredRanges, WireResponse, WireSlotCheck, parse_proposal,
    parse_ranges_query, parse_request, to_json,
};
use opportune_testkit::json_goldens::{GoldenFile, golden_cases, render};
use opportune_testkit::scenarios::{RULE_KINDS, a1_scenarios, rule_kind_scenarios, zone_matrix};
use opportune_testkit::scenarios_a2::a2_scenarios;

/// The use cases the corpus must carry, spelled out here rather than read from
/// the suites so the coverage claim is pinned independently of the fixtures it
/// checks.
const USE_CASES: [&str; 22] = [
    "U1", "U2", "U3", "U4", "U5", "U6", "U7", "U8", "U9", "U10", "U11", "U12", "U13", "U14", "U15",
    "U16", "U17", "U18", "U19", "U20", "U21", "U22",
];

/// The scenarios one zone contributes: eight A1 use cases, eight rule-kind
/// fixtures and fifteen multi-participant cases.
const SCENARIOS_PER_ZONE: usize = 8 + 8 + 15;

/// The zones the corpus must carry a golden in.
///
/// Spelled out here rather than read from `json_goldens::ZONES`, which is the list
/// the corpus is rendered from: a corpus asserted against its own generator can
/// only disagree with the exporter's output, which the freshness test already
/// rejects, so it would assert nothing about the coverage this corpus exists to
/// carry. These are that coverage — a northern and a southern daylight-saving
/// zone, a half-hour shift, a 45-minute offset, and UTC — so dropping a
/// representative is a visible loss rather than a smaller corpus that still
/// agrees with itself.
const REPRESENTATIVE_ZONES: [&str; 5] = [
    "Europe/London",
    "America/Los_Angeles",
    "Australia/Lord_Howe",
    "Asia/Kathmandu",
    "UTC",
];

fn database() -> &'static TimeZoneDatabase {
    static DATABASE: OnceLock<TimeZoneDatabase> = OnceLock::new();
    DATABASE.get_or_init(|| TimeZoneDatabase::bundled().expect("the bundled database is present"))
}

fn engine() -> &'static Engine {
    static ENGINE: OnceLock<Engine> = OnceLock::new();
    ENGINE.get_or_init(|| {
        Engine::builder()
            .build()
            .expect("the bundled database builds")
    })
}

fn corpus_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("goldens")
}

/// The committed corpus, in file-name order.
fn committed() -> Vec<(PathBuf, GoldenFile)> {
    let mut paths: Vec<PathBuf> = fs::read_dir(corpus_dir())
        .expect("the goldens directory is committed")
        .map(|entry| entry.expect("the directory entry is readable").path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "json")
        })
        .collect();
    paths.sort();
    paths
        .into_iter()
        .map(|path| {
            let text = fs::read_to_string(&path).expect("a golden is readable");
            let file: GoldenFile = serde_json::from_str(&text)
                .unwrap_or_else(|error| panic!("{} is not a golden: {error}", path.display()));
            (path, file)
        })
        .collect()
}

/// The document text of a stored JSON value, as a caller would send it.
fn document(value: &serde_json::Value) -> String {
    serde_json::to_string(value).expect("a stored document is JSON")
}

#[test]
fn every_scenario_round_trips_through_the_wire_layer_exactly() {
    let db = database();
    let mut checked = 0;
    for zone in zone_matrix() {
        let mut requests: Vec<(&str, Request)> = Vec::new();
        for scenario in a1_scenarios(db, zone)
            .into_iter()
            .chain(rule_kind_scenarios(db, zone))
        {
            requests.push((scenario.id, scenario.request));
        }
        for scenario in a2_scenarios(db, zone) {
            requests.push((scenario.id, scenario.request));
        }
        for (id, request) in requests {
            let text = to_json(&WireRequest::from(&request)).expect("a request serialises");
            let parsed = parse_request(&text)
                .unwrap_or_else(|error| panic!("`{id}` in {zone} does not parse back: {error}"));
            assert_eq!(
                parsed, request,
                "`{id}` in {zone} does not round-trip exactly"
            );
            checked += 1;
        }
    }
    assert_eq!(
        checked,
        zone_matrix().len() * SCENARIOS_PER_ZONE,
        "the round trip covered {checked} requests; the suites carry {SCENARIOS_PER_ZONE} \
         scenarios per zone, so a scenario appeared or disappeared"
    );
}

/// Substitutes the current crate version for the golden's recorded `meta.engine`
/// so a `release-please` bump never invalidates the committed corpus while every
/// other byte (and `meta.engine == CARGO_PKG_VERSION`) remains pinned.
fn for_current_engine(golden_json: &str) -> String {
    let parsed: serde_json::Value =
        serde_json::from_str(golden_json).expect("a committed golden answer is JSON");
    let recorded = parsed["meta"]["engine"]
        .as_str()
        .expect("a committed golden answer carries `meta.engine`");
    let needle = format!(r#""engine":"{recorded}""#);
    assert_eq!(
        golden_json.matches(&needle).count(),
        1,
        "expected exactly one `{needle}` in golden answer"
    );
    let replacement = format!(r#""engine":"{}""#, env!("CARGO_PKG_VERSION"));
    golden_json.replacen(&needle, &replacement, 1)
}

/// Normalises the escaped `meta.engine` version inside a committed golden file
/// on disk so `the_corpus_is_exactly_what_the_exporter_writes` stays green across
/// release bumps without loosening byte equality on any other field.
fn golden_file_for_current_engine(on_disk: &str) -> String {
    let parsed: GoldenFile =
        serde_json::from_str(on_disk).expect("a committed golden file parses as GoldenFile");
    let response: serde_json::Value =
        serde_json::from_str(&parsed.response).expect("a committed golden response is JSON");
    let recorded = response["meta"]["engine"]
        .as_str()
        .expect("a committed golden response carries `meta.engine`");
    let needle = format!(r#"\"engine\":\"{recorded}\""#);
    let expected_count = 1 + usize::from(parsed.slot_check.is_some());
    assert_eq!(
        on_disk.matches(&needle).count(),
        expected_count,
        "expected {expected_count} `{needle}` occurrence(s) in golden file"
    );
    let replacement = format!(r#"\"engine\":\"{}\""#, env!("CARGO_PKG_VERSION"));
    on_disk.replace(&needle, &replacement)
}

#[test]
fn every_committed_golden_evaluates_to_its_committed_answer() {
    let engine = engine();
    for (path, golden) in committed() {
        let context = format!("{} (golden `{}`)", path.display(), golden.id);
        let request = parse_request(&document(&golden.request))
            .unwrap_or_else(|error| panic!("{context}: the request does not parse: {error}"));
        let response = engine
            .find_availability(&request)
            .unwrap_or_else(|error| panic!("{context}: the request fails: {error}"));
        assert_eq!(
            response.meta.engine,
            env!("CARGO_PKG_VERSION"),
            "{context}: the response must stamp the current crate version"
        );
        assert_eq!(
            to_json(&WireResponse::from(&response)).expect("a response serialises"),
            for_current_engine(&golden.response),
            "{context}: the answer differs from the committed response"
        );
        assert_eq!(
            golden.proposal.is_some(),
            golden.slot_check.is_some(),
            "{context}: a proposal and its slot check are committed together"
        );
        if let Some(proposal) = &golden.proposal {
            let proposal = parse_proposal(&document(proposal))
                .unwrap_or_else(|error| panic!("{context}: the proposal does not parse: {error}"));
            let check = engine
                .check_slot(&request, &proposal)
                .unwrap_or_else(|error| panic!("{context}: the slot check fails: {error}"));
            assert_eq!(
                check.meta.engine,
                env!("CARGO_PKG_VERSION"),
                "{context}: the slot check must stamp the current crate version"
            );
            assert_eq!(
                to_json(&WireSlotCheck::from(&check)).expect("a slot check serialises"),
                for_current_engine(
                    golden
                        .slot_check
                        .as_deref()
                        .expect("a proposal has its check"),
                ),
                "{context}: the slot check differs from the committed check"
            );
        }
        assert_eq!(
            golden.ranges_query.is_some(),
            golden.required_ranges.is_some(),
            "{context}: a ranges query and its answer are committed together"
        );
        if let Some(query) = &golden.ranges_query {
            let query = parse_ranges_query(&document(query)).unwrap_or_else(|error| {
                panic!("{context}: the ranges query does not parse: {error}")
            });
            let ranges = engine
                .required_ranges(&query)
                .unwrap_or_else(|error| panic!("{context}: the ranges query fails: {error}"));
            assert_eq!(
                to_json(&WireRequiredRanges::from(&ranges)).expect("ranges serialise"),
                golden
                    .required_ranges
                    .as_deref()
                    .expect("a query has its answer"),
                "{context}: the required ranges differ from the committed ranges"
            );
        }
    }
}

#[test]
fn the_corpus_is_exactly_what_the_exporter_writes() {
    let cases = golden_cases(database());
    let rendered: Vec<(String, String)> = cases
        .iter()
        .map(|case| {
            let text = render(engine(), case)
                .unwrap_or_else(|error| panic!("`{}` cannot be evaluated: {error}", case.id));
            (case.file_name(), text)
        })
        .collect();
    let mut committed_names: Vec<String> = committed()
        .into_iter()
        .map(|(path, _)| {
            path.file_name()
                .expect("a golden has a file name")
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    let mut expected_names: Vec<String> = rendered.iter().map(|(name, _)| name.clone()).collect();
    committed_names.sort();
    expected_names.sort();
    assert_eq!(
        committed_names, expected_names,
        "the committed corpus is not the case list; run `cargo run -p opportune-testkit --bin export_goldens`"
    );
    let current_stamp = format!(r#"\"engine\":\"{}\""#, env!("CARGO_PKG_VERSION"));
    for (name, text) in rendered {
        let path = corpus_dir().join(&name);
        let on_disk = fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("{} is unreadable: {error}", path.display()));
        assert!(
            text.contains(&current_stamp),
            "{} rendered without current version stamp `{current_stamp}`",
            path.display()
        );
        assert_eq!(
            golden_file_for_current_engine(&on_disk),
            text,
            "{} is stale; re-run the golden exporter",
            path.display()
        );
    }
}

#[test]
fn the_corpus_covers_every_use_case_rule_kind_and_representative_zone() {
    let corpus = committed();
    let use_cases: Vec<&str> = corpus
        .iter()
        .map(|(_, golden)| golden.use_case.as_str())
        .collect();
    for use_case in USE_CASES {
        assert!(
            use_cases.contains(&use_case),
            "the corpus carries no golden for {use_case}"
        );
    }
    for kind in RULE_KINDS {
        assert!(
            use_cases.contains(&kind),
            "the corpus carries no golden for the {kind} rule kind"
        );
    }
    // Spelled out in `REPRESENTATIVE_ZONES` rather than read from the generator,
    // so trimming the corpus's zone coverage is red here.
    let zones: Vec<&str> = corpus
        .iter()
        .map(|(_, golden)| golden.zone.as_str())
        .collect();
    for zone in REPRESENTATIVE_ZONES {
        assert!(
            zones.contains(&zone),
            "the corpus carries no golden in {zone}, so cross-language parity has no {zone} \
             representative"
        );
    }

    let find = |use_case: &str| -> &GoldenFile {
        &corpus
            .iter()
            .find(|(_, golden)| golden.use_case == use_case)
            .unwrap_or_else(|| panic!("the corpus carries no golden for {use_case}"))
            .1
    };
    let u22 = find("U22");
    assert!(
        u22.proposal.is_some() && u22.slot_check.is_some(),
        "U22 confirms a proposed booking"
    );
    // The runner compares the committed check byte for byte, so a corpus whose
    // first offer stopped confirming would be regenerated into agreement and go
    // on passing. The answer a caller reads has to be the confirming one.
    let check: serde_json::Value = serde_json::from_str(
        u22.slot_check
            .as_deref()
            .expect("U22 commits its slot check"),
    )
    .expect("the committed slot check is JSON");
    assert_eq!(
        check["valid"],
        serde_json::Value::Bool(true),
        "the offer U22's corpus hands back must confirm; a committed rejection is a \
         regenerated regression, not an answer"
    );
    // A cap counts booked occurrences and a rest rule counts neither, so the two
    // cases pin both answers of the ranges query.
    for (use_case, occurrences) in [("cap", true), ("rest", false)] {
        let golden = find(use_case);
        assert!(
            golden.ranges_query.is_some() && golden.required_ranges.is_some(),
            "{use_case} asks for the ranges the search needs"
        );
        let ranges: serde_json::Value = serde_json::from_str(
            golden
                .required_ranges
                .as_deref()
                .expect("the case's ranges are committed"),
        )
        .expect("the committed ranges are JSON");
        assert_eq!(
            !ranges["occurrences"].is_null(),
            occurrences,
            "the {use_case} case's occurrence range"
        );
    }
}

/// A reached limit truncates the answer and says so; it never fails the request, and
/// the corpus has to keep carrying a case where it does.
///
/// The runner compares the committed response byte for byte, so a case whose limit
/// stopped being reached would be regenerated into agreement and go on passing
/// while the corpus no longer pinned truncation at all.
#[test]
fn the_corpus_pins_both_truncations() {
    let corpus = committed();
    let by_id = |id: &str| -> serde_json::Value {
        let response = &corpus
            .iter()
            .find(|(_, golden)| golden.id == id)
            .unwrap_or_else(|| panic!("the corpus carries no `{id}` golden"))
            .1
            .response;
        serde_json::from_str(response).expect("a committed response is JSON")
    };

    let offers = by_id("truncated-offers");
    assert_eq!(
        offers["truncated"],
        serde_json::Value::Bool(true),
        "the offer limit was reached, so the answer says it was truncated"
    );
    assert_eq!(
        offers["offers"].as_array().map(Vec::len),
        Some(2),
        "the answer carries exactly the offers the limit allows"
    );

    let explanations = by_id("truncated-explanations");
    assert_eq!(
        explanations["excluded_truncated"],
        serde_json::Value::Bool(true),
        "the explanation limit was reached, so the exclusion list says it was truncated"
    );
    assert_eq!(
        explanations["excluded"].as_array().map(Vec::len),
        Some(1),
        "the exclusion list carries exactly the one exclusion the limit allows"
    );
}

#[test]
fn the_corpus_pins_the_bundled_time_zone_database() {
    for (path, golden) in committed() {
        let response: serde_json::Value =
            serde_json::from_str(&golden.response).expect("a committed response is JSON");
        assert_eq!(
            response["meta"]["tzdb"].as_str(),
            Some(database().version()),
            "{} was written for another time zone database; re-run the golden exporter",
            path.display()
        );
    }
}
