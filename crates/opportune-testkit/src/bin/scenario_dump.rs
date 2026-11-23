//! Prints the scenario suite's canonical answer, for the two-process determinism
//! check that compares two independent runs byte for byte.
//!
//! Every run builds its own bundled database and engine, then renders the whole
//! suite through [`opportune_testkit::canonical`]. Nothing is read from the host:
//! the database is the bundled one and `now` is a request field.

use opportune::Engine;
use opportune::time::TimeZoneDatabase;
use opportune_testkit::canonical::scenario_suite_report;

fn main() {
    let db = TimeZoneDatabase::bundled().expect("the bundled database is present");
    let engine = Engine::builder()
        .build()
        .expect("the bundled database builds an engine");
    print!("{}", scenario_suite_report(&db, &engine));
}
