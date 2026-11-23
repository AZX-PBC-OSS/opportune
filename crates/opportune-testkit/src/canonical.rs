//! A canonical text rendering of the breadth scenario suite's answers.
//!
//! Two separate processes each run the suite through the real engine and render
//! the answers with this module; the determinism check compares the two
//! renderings byte for byte. Every field an answer is made of is rendered in a
//! fixed order, so a difference between two runs means the engine answered
//! differently rather than that the same answer was described differently.
//!
//! The rendering is deliberately hand-written: the core crate has no serialiser,
//! and adding one to it for a test's sake would put a wire format where none
//! belongs.

use opportune::Engine;
use opportune::model::{OfferFlag, RangesQuery, Request, ReserveSpan, Response, Span};
use opportune::reasons::Excluded;
use opportune::time::TimeZoneDatabase;

use crate::scenarios::{RULE_KINDS, Scenario, a1_scenarios, rule_kind_scenarios, zone_matrix};
use crate::scenarios_a2::{MultiScenario, a2_scenarios};

/// Appends one line, which cannot fail on a `String`.
macro_rules! emit {
    ($out:expr, $($arg:tt)*) => {
        <String as std::fmt::Write>::write_fmt(
            $out,
            format_args!("{}\n", format_args!($($arg)*)),
        )
        .expect("writing to a String cannot fail")
    };
}

/// Renders every breadth scenario — the A1 use cases, the rule kinds and the A2
/// multi-participant cases — in every zone of the matrix, through `engine`.
///
/// The compact exact-list scenarios ([`crate::scenarios::exact_scenarios`]) are
/// not rendered here: they take no generated input, and the A1 test compares
/// their whole answers with `assert_eq!`, which pins more than this rendering
/// would.
///
/// The two header lines name what the body covers: the use cases the suite's
/// scenarios name, and the rule kinds it carries fixtures for. A run that
/// silently lost a scenario therefore changes the header too, rather than
/// quietly comparing a smaller suite.
#[must_use]
pub fn scenario_suite_report(db: &TimeZoneDatabase, engine: &Engine) -> String {
    let single: Vec<Vec<Scenario>> = zone_matrix()
        .iter()
        .map(|zone| a1_scenarios(db, zone))
        .collect();
    let kinds: Vec<Vec<Scenario>> = zone_matrix()
        .iter()
        .map(|zone| rule_kind_scenarios(db, zone))
        .collect();
    let multi: Vec<Vec<MultiScenario>> = zone_matrix()
        .iter()
        .map(|zone| a2_scenarios(db, zone))
        .collect();

    let mut out = String::new();
    emit!(
        &mut out,
        "{}",
        header(single.iter().flatten(), multi.iter().flatten())
    );
    for scenarios in [&single, &kinds] {
        for (zone, scenarios) in zone_matrix().iter().zip(scenarios) {
            for scenario in scenarios {
                emit!(
                    &mut out,
                    "== scenario {} ({}) in {zone}",
                    scenario.id,
                    scenario.use_case
                );
                render_request(&mut out, engine, &scenario.request);
            }
        }
    }
    for (zone, scenarios) in zone_matrix().iter().zip(&multi) {
        for scenario in scenarios {
            emit!(
                &mut out,
                "== scenario {} ({}) in {zone}",
                scenario.id,
                scenario.use_case
            );
            render_request(&mut out, engine, &scenario.request);
        }
    }
    out
}

/// The two header lines: the use cases the suite's scenarios name, and the rule
/// kinds it carries fixtures for.
fn header<'a>(
    single: impl Iterator<Item = &'a Scenario>,
    multi: impl Iterator<Item = &'a MultiScenario>,
) -> String {
    let mut use_cases: Vec<&str> = single.map(|scenario| scenario.use_case).collect();
    use_cases.extend(multi.map(|scenario| scenario.use_case));
    use_cases.sort_unstable();
    use_cases.dedup();
    format!(
        "use-cases {}\nrule-kinds {}",
        use_cases.join(" "),
        RULE_KINDS.join(" ")
    )
}

/// One request through every entry point a caller uses on it.
fn render_request(out: &mut String, engine: &Engine, request: &Request) {
    let diagnostics = engine.validate(request);
    emit!(
        out,
        "validation {} errors={} warnings={}",
        if diagnostics.is_valid() {
            "valid"
        } else {
            "invalid"
        },
        diagnostics.errors.len(),
        diagnostics.warnings.len()
    );
    for diagnostic in diagnostics.errors.iter().chain(&diagnostics.warnings) {
        emit!(
            out,
            "  diagnostic {} at {} {:?}",
            diagnostic.code(),
            diagnostic.path().unwrap_or("-"),
            diagnostic.message()
        );
    }

    match engine.find_availability(request) {
        Ok(response) => render_response(out, engine, request, &response),
        Err(error) => emit!(out, "search failed {}", error.code()),
    }
    render_ranges(out, engine, request);
}

/// A search's answer, and every offer handed back for confirmation.
fn render_response(out: &mut String, engine: &Engine, request: &Request, response: &Response) {
    emit!(
        out,
        "response offers={} dates={} truncated={} excluded={} excluded_truncated={}",
        response.offers.len(),
        response.dates.len(),
        response.truncated,
        response.excluded.len(),
        response.excluded_truncated
    );
    for date in &response.dates {
        emit!(out, "  date {} offers={}", date.date, date.offers);
    }
    for warning in &response.warnings {
        emit!(out, "  warning {}", warning.code());
    }
    for offer in &response.offers {
        for event in &offer.events {
            emit!(
                out,
                "  offer {}..{} part={} eligible={} assignment={} reserve={}",
                event.start,
                event.end,
                event.part,
                lists(&event.eligible),
                lists(&event.assignment),
                reserves(&event.reserve)
            );
        }
        emit!(out, "  offer flags={}", flags(&offer.flags));

        // Handing an offer back is how a caller confirms it, so the confirmation
        // is part of the answer under test.
        match engine.check_slot(request, &offer.to_proposal()) {
            Ok(check) => {
                emit!(
                    out,
                    "  check valid={} flags={} reserve={}",
                    check.valid,
                    flags(&check.flags),
                    reserves(&check.reserve)
                );
                for reason in &check.reasons {
                    emit!(out, "    reason {reason}");
                }
            }
            Err(error) => emit!(out, "  check failed {}", error.code()),
        }
    }
    for entry in &response.excluded {
        render_exclusion(out, entry);
    }
}

fn render_exclusion(out: &mut String, entry: &Excluded) {
    emit!(out, "  excluded {} part={}", entry.start, entry.part);
    for reason in &entry.reasons {
        emit!(out, "    reason {reason}");
    }
}

/// The caller responsibility `required_ranges` answers: which calendar and
/// occurrence data to fetch.
fn render_ranges(out: &mut String, engine: &Engine, request: &Request) {
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
    match engine.required_ranges(&query) {
        Ok(ranges) => {
            let occurrences = ranges
                .occurrences
                .map_or_else(|| "none".to_owned(), |span| span_text(&span));
            emit!(
                out,
                "ranges calendar={} occurrences={occurrences}",
                span_text(&ranges.calendar)
            );
        }
        Err(error) => emit!(out, "ranges failed {}", error.code()),
    }
}

fn span_text(span: &Span) -> String {
    format!("{}..{}", span.start(), span.end())
}

/// One entry per requirement term, each term's IDs comma-separated.
fn lists(lists: &[Vec<String>]) -> String {
    lists
        .iter()
        .map(|list| list.join(","))
        .collect::<Vec<_>>()
        .join(";")
}

fn reserves(reserve: &[ReserveSpan]) -> String {
    reserve
        .iter()
        .map(|span| {
            format!(
                "{}:{}:{}",
                span.participant,
                span.kind,
                span_text(&span.span)
            )
        })
        .collect::<Vec<_>>()
        .join(";")
}

fn flags(flags: &[OfferFlag]) -> String {
    flags
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(",")
}
