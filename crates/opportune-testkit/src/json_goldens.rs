//! The committed JSON golden corpus: the scenario suite rendered through the
//! wire layer into one document per case.
//!
//! A golden is the whole conversation a caller has over JSON: the request
//! document, the answer the engine gives it, and — for the cases that exercise
//! them — the proposal and its slot check, and the ranges query and its answer.
//! Every answer is stored as the exact text the wire layer writes, so a reader in
//! another language compares bytes instead of parsing and re-serialising; the
//! documents a caller sends are stored as JSON values, because the whitespace a
//! caller chooses is its own business.
//!
//! [`golden_cases`] is the single list of cases. The exporter writes exactly
//! those files and the test suite compares the committed corpus against the same
//! list, so a case cannot be written without being verified, or verified without
//! being written.
//!
//! The cases span five zones rather than the whole matrix: London and Los Angeles
//! are northern-hemisphere DST zones with distinct transition schedules, Lord Howe
//! shifts by half an hour in the southern hemisphere, Kathmandu sits at a 45-minute
//! offset, and UTC pins the absolute answer. Every
//! case runs in every zone of the matrix in the round-trip and scenario tests in
//! this crate, so the corpus does not have to repeat that coverage in a second
//! language.

use opportune::Engine;
use opportune::model::{Explain, Proposal, RangesQuery, Request, Response};
use opportune::time::TimeZoneDatabase;
use opportune_json::{
    WireProposal, WireRangesQuery, WireRequest, WireRequiredRanges, WireResponse, WireSlotCheck,
    to_json,
};
use serde::{Deserialize, Serialize};

use crate::scenarios::{a1_scenarios, rule_kind_scenarios};
use crate::scenarios_a2::a2_scenarios;

/// The zones the corpus spans: a northern DST zone, a southern one, a half-hour
/// DST zone, a 45-minute offset, and UTC.
pub const ZONES: [&str; 5] = [
    "Europe/London",
    "America/Los_Angeles",
    "Australia/Lord_Howe",
    "Asia/Kathmandu",
    "UTC",
];

/// The documents a case carries beyond its search answer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Extra {
    /// The search answer alone.
    Search,
    /// The search answer, then its first offer handed back for confirmation.
    Confirm,
    /// The search answer, then the calendar and occurrence ranges the search
    /// needs before any calendar is fetched.
    Ranges,
}

/// One scenario the corpus carries, in the zone it is rendered for.
#[derive(Clone, Debug)]
pub struct GoldenCase {
    /// The scenario's stable identifier.
    pub id: String,
    /// The use case or rule kind the scenario pins.
    pub use_case: String,
    /// The zone the scenario's own clock is read in.
    pub zone: String,
    /// The request the scenario runs.
    pub request: Request,
    /// The documents the case carries beyond its search answer.
    pub extra: Extra,
}

impl GoldenCase {
    /// The file the case is committed as.
    #[must_use]
    pub fn file_name(&self) -> String {
        format!("{}-{}.json", self.id, zone_slug(&self.zone))
    }
}

/// One committed golden file.
///
/// Writing and reading share this shape, and `deny_unknown_fields` makes a key
/// neither side knows a load failure rather than a field nothing reads.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct GoldenFile {
    /// The scenario's stable identifier.
    pub id: String,
    /// The use case or rule kind the scenario pins.
    pub use_case: String,
    /// The zone the scenario's own clock is read in.
    pub zone: String,
    /// The request document, verbatim as a caller sends it.
    pub request: serde_json::Value,
    /// The search answer, as the exact text the wire layer writes.
    pub response: String,
    /// The proposal the case confirms, where it confirms one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proposal: Option<serde_json::Value>,
    /// The slot check's answer, as the exact text the wire layer writes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slot_check: Option<String>,
    /// The ranges query document, where the case asks for one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ranges_query: Option<serde_json::Value>,
    /// The ranges query's answer, as the exact text the wire layer writes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub required_ranges: Option<String>,
}

/// Every case the corpus carries.
///
/// The zones take turns rather than every case running in every zone: one golden
/// per case keeps the corpus reviewable, and the full matrix is covered by the
/// round-trip and scenario tests.
#[must_use]
pub fn golden_cases(db: &TimeZoneDatabase) -> Vec<GoldenCase> {
    let mut cases = Vec::new();
    for (index, zone) in ZONES.iter().enumerate() {
        for (position, case) in suite(db, zone).into_iter().enumerate() {
            if position % ZONES.len() == index {
                cases.push(case);
            }
        }
    }
    cases
}

/// Renders one case exactly as the corpus commits it.
///
/// # Errors
/// Returns the engine's error when the case's request cannot be evaluated, so a
/// fixture the engine rejects cannot be written out as a plausible-looking
/// golden.
///
/// # Panics
/// Panics when a fixture stops being the shape the corpus assumes: a confirming
/// case whose answer offers no start to hand back. Writing a golden without its
/// slot check would report success for a document the corpus never exercised.
pub fn render(engine: &Engine, case: &GoldenCase) -> Result<String, opportune::Error> {
    let response = engine.find_availability(&case.request)?;
    let (proposal, slot_check) = proposal_and_check(engine, case, &response)?;
    let (ranges_query, required_ranges) = ranges(engine, case)?;
    let file = GoldenFile {
        id: case.id.clone(),
        use_case: case.use_case.clone(),
        zone: case.zone.clone(),
        request: document(&WireRequest::from(&case.request)),
        response: wire(&WireResponse::from(&response)),
        proposal,
        slot_check,
        ranges_query,
        required_ranges,
    };
    let mut text = serde_json::to_string_pretty(&file).expect("a golden file serialises");
    text.push('\n');
    Ok(text)
}

/// The suite's cases for one zone, in the order the corpus lists them.
fn suite(db: &TimeZoneDatabase, zone: &str) -> Vec<GoldenCase> {
    let mut cases = Vec::new();
    let a1 = a1_scenarios(db, zone);
    // The base the two truncation cases are derived from: the first use case's
    // request has more candidate starts than either limit and excludes most of them.
    // It is captured here and appended last, so the positions every other case holds
    // in this list — and therefore the zone each one is rendered for — stay put.
    let truncation_base = a1
        .iter()
        .find(|scenario| scenario.use_case == "U1")
        .map(|scenario| (scenario.zone.clone(), scenario.request.clone()));
    for scenario in a1 {
        let extra = if scenario.use_case == "U22" {
            Extra::Confirm
        } else {
            Extra::Search
        };
        cases.push(GoldenCase {
            id: scenario.id.to_owned(),
            use_case: scenario.use_case.to_owned(),
            zone: scenario.zone,
            request: scenario.request,
            extra,
        });
    }
    for scenario in rule_kind_scenarios(db, zone) {
        let extra = match scenario.use_case {
            // A cap needs occurrence data and a rest rule reads the day either
            // side, so these two pin both answers of the ranges query.
            "cap" | "rest" => Extra::Ranges,
            _ => Extra::Search,
        };
        cases.push(GoldenCase {
            id: scenario.id.to_owned(),
            use_case: scenario.use_case.to_owned(),
            zone: scenario.zone,
            request: scenario.request,
            extra,
        });
    }
    for scenario in a2_scenarios(db, zone) {
        cases.push(GoldenCase {
            id: scenario.id.to_owned(),
            use_case: scenario.use_case.to_owned(),
            zone: scenario.zone,
            request: scenario.request,
            extra: Extra::Search,
        });
    }
    // A limit that is reached truncates the answer and sets `truncated`; it never
    // fails the request.
    if let Some((zone, base)) = truncation_base {
        for (id, limit, explain) in [
            ("truncated-offers", Some(2_usize), None),
            ("truncated-explanations", None, Some(1_usize)),
        ] {
            let mut request = base.clone();
            request.limit = limit;
            request.explain = explain.map(Explain::new);
            cases.push(GoldenCase {
                id: id.to_owned(),
                use_case: "truncation".to_owned(),
                zone: zone.clone(),
                request,
                extra: Extra::Search,
            });
        }
    }
    cases
}

/// The proposal a confirming case hands back, and the engine's answer to it.
fn proposal_and_check(
    engine: &Engine,
    case: &GoldenCase,
    response: &Response,
) -> Result<(Option<serde_json::Value>, Option<String>), opportune::Error> {
    if case.extra != Extra::Confirm {
        return Ok((None, None));
    }
    let offer = response
        .offers
        .first()
        .expect("a confirming case's fixture offers a start");
    let proposal: Proposal = offer.to_proposal();
    let check = engine.check_slot(&case.request, &proposal)?;
    Ok((
        Some(document(&WireProposal::from(&proposal))),
        Some(wire(&WireSlotCheck::from(&check))),
    ))
}

/// The ranges query a ranges case sends, and the engine's answer to it.
fn ranges(
    engine: &Engine,
    case: &GoldenCase,
) -> Result<(Option<serde_json::Value>, Option<String>), opportune::Error> {
    if case.extra != Extra::Ranges {
        return Ok((None, None));
    }
    let query = ranges_query(&case.request);
    let required = engine.required_ranges(&query)?;
    Ok((
        Some(document(&WireRangesQuery::from(&query))),
        Some(wire(&WireRequiredRanges::from(&required))),
    ))
}

/// The query a caller sends before fetching anything: the request's `now`, goal,
/// rules and participants, with the calendars left out — discovering the range to
/// fetch is the point of the query.
fn ranges_query(request: &Request) -> RangesQuery {
    let mut query = RangesQuery::new(request.now, request.goal.clone());
    query.rules.clone_from(&request.rules);
    query.participants = request
        .participants
        .iter()
        .cloned()
        .map(|mut participant| {
            participant.calendar = None;
            participant
        })
        .collect();
    query
}

/// The exact text the wire layer writes for a value.
fn wire<T: Serialize>(value: &T) -> String {
    to_json(value).expect("every wire value serialises to JSON")
}

/// The same document as a JSON value, so a file holds it as JSON rather than as
/// an escaped string.
fn document<T: Serialize>(value: &T) -> serde_json::Value {
    serde_json::to_value(value).expect("every wire value is a JSON document")
}

/// A zone name as a file-name segment: `Europe/London` becomes `europe-london`.
fn zone_slug(zone: &str) -> String {
    zone.to_ascii_lowercase().replace('/', "-")
}
