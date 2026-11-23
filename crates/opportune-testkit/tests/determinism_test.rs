//! The two-process determinism check: the whole scenario suite runs in two
//! separate OS processes and the two canonical renderings must be byte-identical.
//!
//! Two calls in one process would share every internal cache, allocator state and
//! iteration order, so they could agree while the engine is still
//! order-dependent. Each process here builds its own database and engine from
//! nothing but the bundled data and renders the suite with
//! [`opportune_testkit::canonical`], so a difference means the answer itself
//! depends on something outside the request.
//!
//! The report also has to prove it covers the suite: a report that quietly lost
//! the scenarios would compare two empty strings and pass.

use std::process::{Command, Output};

use opportune_testkit::scenarios::RULE_KINDS;

/// The use cases the assembled suite must cover.
const USE_CASES: [&str; 22] = [
    "U1", "U2", "U3", "U4", "U5", "U6", "U7", "U8", "U9", "U10", "U11", "U12", "U13", "U14", "U15",
    "U16", "U17", "U18", "U19", "U20", "U21", "U22",
];

fn run_suite() -> String {
    let output: Output = Command::new(env!("CARGO_BIN_EXE_scenario_dump"))
        .output()
        .expect("the scenario dump binary runs");
    assert!(
        output.status.success(),
        "the scenario dump binary failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("the report is UTF-8")
}

#[test]
fn the_scenario_suite_is_identical_in_two_separate_processes() {
    let first = run_suite();
    let second = run_suite();
    assert_eq!(
        first, second,
        "two processes rendered the scenario suite differently"
    );
}

#[test]
fn the_compared_report_covers_every_use_case_and_rule_kind() {
    let report = run_suite();
    for use_case in USE_CASES {
        assert!(
            report.contains(&format!("({use_case})")),
            "{use_case} is missing from the compared report"
        );
    }
    let header = report
        .lines()
        .find(|line| line.starts_with("rule-kinds "))
        .expect("the report names the rule kinds it covers");
    for kind in RULE_KINDS {
        assert!(
            header.split_whitespace().skip(1).any(|named| named == kind),
            "{kind} is missing from the compared report"
        );
    }
    // A report of headers alone would compare equal and prove nothing.
    assert!(
        report.lines().count() > 1_000,
        "the compared report has {} lines, too few to be the whole suite",
        report.lines().count()
    );
}
