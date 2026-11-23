//! Writes the committed JSON golden corpus under `crates/opportune-testkit/goldens/`.
//!
//! Run it from the workspace root with
//! `cargo run -p opportune-testkit --bin export_goldens`. Every file is written
//! from the bundled time zone database and the scenario suite alone, so a re-run
//! on an unchanged tree produces no diff, and a time zone database bump changes
//! every response until the corpus is regenerated deliberately.
//!
//! A file the corpus no longer names is reported rather than deleted: the
//! exporter owns what it writes, not whatever else sits in the directory. Every
//! golden is rendered before the directory is touched, so a run that fails leaves
//! the committed corpus exactly as it was.

use std::collections::BTreeSet;
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use opportune::Engine;
use opportune::time::TimeZoneDatabase;
use opportune_testkit::json_goldens::{golden_cases, render};

fn main() -> ExitCode {
    let database = TimeZoneDatabase::bundled().expect("the bundled database is present");
    let engine = Engine::builder()
        .build()
        .expect("the bundled database builds an engine");
    let directory = Path::new(env!("CARGO_MANIFEST_DIR")).join("goldens");

    let rendered: Vec<(String, String)> = golden_cases(&database)
        .iter()
        .map(|case| {
            let text = render(&engine, case).unwrap_or_else(|error| {
                panic!("golden `{}` cannot be evaluated: {error}", case.id)
            });
            (case.file_name(), text)
        })
        .collect();

    let names: BTreeSet<&str> = rendered.iter().map(|(name, _)| name.as_str()).collect();
    let stray = stray_files(&directory, &names);
    if !stray.is_empty() {
        for path in &stray {
            eprintln!("{} is not a golden case", path.display());
        }
        eprintln!(
            "nothing was written: the corpus is exactly the cases `golden_cases` lists, so delete \
             the {} file(s) above and run the exporter again",
            stray.len()
        );
        return ExitCode::FAILURE;
    }

    fs::create_dir_all(&directory).expect("the goldens directory is creatable");
    for (name, text) in &rendered {
        fs::write(directory.join(name), text).expect("a golden file is writable");
    }
    println!(
        "wrote {} goldens to {}",
        rendered.len(),
        directory.display()
    );
    ExitCode::SUCCESS
}

/// The `.json` files in `directory` that the corpus does not name.
fn stray_files(directory: &Path, written: &BTreeSet<&str>) -> Vec<PathBuf> {
    if !directory.is_dir() {
        return Vec::new();
    }
    let mut stray: Vec<PathBuf> = fs::read_dir(directory)
        .expect("the goldens directory is readable")
        .map(|entry| entry.expect("the directory entry is readable").path())
        .filter(|path| path.extension() == Some(OsStr::new("json")))
        .filter(|path| {
            path.file_name()
                .and_then(OsStr::to_str)
                .is_none_or(|name| !written.contains(name))
        })
        .collect();
    stray.sort();
    stray
}
