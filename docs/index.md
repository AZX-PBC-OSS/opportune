# opportune

Calendar availability and scheduling calculations across constraints: determine which times can be offered given participants' calendars, working hours, and rules, and verify whether a proposed booking is valid at confirmation time.

`opportune` implements its scheduling semantics once in pure Rust and exposes that same engine across three native ecosystems:

- **Rust:** pure-Rust core crate (`opportune`), JSON wire layer (`opportune-json`), and Microsoft Graph / Google Calendar payload adapters (`opportune-adapters`).
- **Python:** typed `cp312-abi3` wheel (Python 3.12+) with generated Pydantic v2 models, GIL-releasing engine calls, async helpers, and provider adapters.
- **TypeScript / JavaScript:** zero-dependency npm package for Node 24+ and Bun 1.4.2+ backed by WebAssembly (`wasm32`), with generated TypeScript types, presets, and provider adapters.

## Capabilities

- **Pure and deterministic:** one request in, one response out, with `now` as an explicit argument and no ambient clock or I/O.
- **All seven rule kinds:** `window`, `blackout`, `clearance`, `lead_time`, `status_policy`, `rest`, and `cap`.
- **Multi-participant scheduling:** hosts and attendees, multi-term requirements with weighted matching, travel buffers, and reserve spans.
- **Full booking lifecycle:** offers, date counts, opt-in exclusion reasons (`explain`), confirm-time verification (`check_slot`), static validation (`validate`), and fetch-range planning (`required_ranges`).
- **Ergonomic presets and sugar:** concise builders for common single-host and multi-host scheduling flows.

## Quickstart

```rust
use opportune::Engine;
use opportune::model::{Goal, Participant, Request, Requirement, Span, Term};
use opportune::presets;
use opportune::time::{
    Date, Duration, Instant, RuleZone, TimeOfDay, Weekday, Weekdays, ZoneName,
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let zone = ZoneName::new("Europe/London")?;
    let now = Instant::from_utc(Date::new(2027, 6, 14)?, TimeOfDay::new(7, 0)?)?;
    let until = Instant::from_utc(Date::new(2027, 6, 19)?, TimeOfDay::new(0, 0)?)?;
    let mut request = Request::new(
        now,
        Goal::single(
            zone.clone(),
            Span::new(now, until)?,
            Duration::from_minutes(30),
            Requirement::new(vec![Term::person("rich")]),
        ),
    );
    request
        .participants
        .push(Participant::new("rich", zone).with_calendar(Vec::new()));
    request.rules.push(presets::business_hours(
        RuleZone::Participant,
        Weekdays::range(Weekday::Monday, Weekday::Friday),
        TimeOfDay::new(9, 0)?,
        TimeOfDay::new(17, 0)?,
    )?);
    request
        .rules
        .push(presets::buffers(Duration::from_minutes(10), Duration::from_minutes(10)));
    request
        .rules
        .push(presets::min_notice(Duration::from_hours_minutes(4, 0)));

    let response = Engine::builder().build()?.find_availability(&request)?;
    assert!(!response.offers.is_empty());
    Ok(())
}
```

The engine reads time zone data from the database bundled into the binary (`bundled-tzdb`, enabled by default) or from a caller-supplied directory of `TZif` files — never from the host OS, keeping results reproducible across machines.

## Install

### Rust

Add the core crate from crates.io, along with `opportune-json` (wire layer) and `opportune-adapters` (Graph/Google conversions) as needed:

```sh
cargo add opportune
cargo add opportune-json
cargo add opportune-adapters
```

### Python

Install the `cp312-abi3` wheel (Python 3.12+):

```sh
pip install opportune
# or: uv add opportune
```

<!-- snippet: index-python-version -->
```python
import opportune

print(opportune.__version__)
```

The compiled extension module `opportune._opportune` shares a single version source with the Rust workspace.

### Node and Bun

Install the npm package (Node 24+ or Bun 1.4.2+):

```sh
npm install opportune
```

See [WASM and TypeScript](wasm.md) for runtime support, adapter usage, browser/Edge presets (`opportune/presets`), and the bundle-size budget.

### Building from source

To work from a clone of the repository:

- **Rust:** run `cargo add --path ../opportune/crates/opportune` inside your consumer crate.
- **Python:** run `make install` (creates `.venv` and builds the extension via `uv` + `maturin`), or activate a virtualenv and run `maturin develop --release --manifest-path crates/opportune-py/Cargo.toml`.
- **Node/Bun:** run `make wasm` to build the release WASM module and `crates/opportune-wasm/js/dist/`.

## Where to go next

- [Recipes](recipes.md): nine worked scenarios — a personal booking link, interview panels with caps, rest and horizons, a conference booth, travel holds, provider adapters, a three-tier policy, a team plus a conference room, and a weighted panel rescheduled at capacity — in Rust, Python, and TypeScript — plus the production blueprints for booking commits, weighted dispatch, occurrences, and multi-tenant engines.
- [API reference](api.md): the Rust surface, JSON wire format and pointer errors, provider adapter contract, and Python/FastAPI guide.
- [WASM and TypeScript](wasm.md): the npm package, generated types, Graph and Google adapter submodules, and bundle-size budget.
- [Crate structure](crates.md): the six-crate Cargo workspace and crate boundaries.
- [Design principles](design.md): core architectural commitments and semantics.
- [Dependencies and licensing](dependencies.md): permissive-only dependency and license policy.

## Contributing & Security

Development setup, pre-PR gates, and commit rules are in [`CONTRIBUTING.md`](https://github.com/AZX-PBC-OSS/opportune/blob/main/CONTRIBUTING.md). To report security vulnerabilities privately, see [`SECURITY.md`](https://github.com/AZX-PBC-OSS/opportune/blob/main/SECURITY.md).
