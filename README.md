# opportune

Calendar availability and scheduling calculations across constraints, as a pure-Rust library with native Python and WebAssembly (Node, Bun, Cloudflare Workers, browsers, and module Web Workers) bindings.

`opportune` takes a scheduling request — `now`, a booking goal, participants with their calendars, and a rule list — and deterministically returns bookable offers, date counts, travel holds, and optional exclusion reasons. It supports all seven rule kinds (`window`, `blackout`, `clearance`, `lead_time`, `status_policy`, `rest`, `cap`), multi-participant weighted matching, confirm-time validation (`check_slot`), fetch-hull planning (`required_ranges`), a deterministic JSON wire format, and Microsoft Graph and Google Calendar payload adapters.

## Rust

Install the core crate (plus `opportune-json` for the wire format and `opportune-adapters` for Graph/Google conversions when needed):

```sh
cargo add opportune
```

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
    for offer in &response.offers {
        let start = offer.start().ok_or("an offer has an event")?;
        println!("{start} — {}", offer.assigned_hosts().join(", "));
    }
    Ok(())
}
```

`Engine::check_slot` confirms a proposed booking against the same rules and calendars, `Engine::validate` reports structural errors and warnings, and `Engine::required_ranges` computes which calendar and occurrence spans to fetch. The engine reads time zone data from the bundled database (`bundled-tzdb`, enabled by default) or from a caller-specified `TZif` directory — never from the host OS. See [`docs/api.md`](docs/api.md) for the full reference.

## Python

Install the `cp312-abi3` wheel (Python 3.12+; one wheel per platform bundles the compiled Rust engine):

```sh
pip install opportune
# or: uv add opportune
```

<!-- snippet: readme-python -->
```python
import opportune

request = opportune.Request(
    now="2027-06-14T07:00Z",
    goal=opportune.single_goal(
        "30m",
        opportune.Requirement(all=[opportune.person("rich")]),
        zone="Europe/London",
        search_window=("2027-06-13T23:00Z", "2027-06-18T23:00Z"),
    ),
    participants=[opportune.Participant(id="rich", zone="Europe/London", calendar=[])],
    rules=[
        opportune.presets.business_hours(),
        opportune.presets.buffers(),
        opportune.presets.min_notice(),
    ],
)

response = opportune.find_availability(request)
print(opportune.offer_start(response.offers[0]), opportune.offer_end(response.offers[0]))
```

Requests and responses are generated Pydantic v2 models that plug directly into FastAPI routes. Build and reuse one `opportune.Engine` — every call releases the GIL around Rust execution — and use `opportune.adapters.graph` / `opportune.adapters.google` to convert provider payloads. See [`docs/api.md`](docs/api.md#python) for the FastAPI lifespan and `*_async` pattern, stored-document upgrades, adapter helpers, and benchmarks.

## TypeScript / JavaScript

Install the `opportune` npm package (Node 24+ or Bun 1.4.2+; also Cloudflare Workers, browsers, and module Web Workers, selected by the `workerd`/`browser`/`worker` conditions on the package's private `#wasm` import — WebAssembly engine with bundled time zone data, generated TypeScript types, presets, and Graph/Google adapter submodules):

```sh
npm install opportune
```

<!-- snippet: readme-ts -->
```ts
import { Engine, businessHours, offerStart, person, singleGoal } from "opportune";

const request = {
  now: "2027-06-14T07:00Z",
  goal: singleGoal("30m", { all: [person("rich")] }, {
    zone: "Europe/London",
    searchWindow: ["2027-06-13T23:00Z", "2027-06-18T23:00Z"],
  }),
  participants: [{ id: "rich", zone: "Europe/London", calendar: [] }],
  rules: [businessHours()],
};

using engine = new Engine();
const [earliest] = engine.findAvailability(request).offers;
if (earliest === undefined) throw new Error("the request offers no start");
const start = offerStart(earliest);
if (start !== "2027-06-14T08:00Z") throw new Error(`unexpected earliest start: ${start}`);
console.log(start);
```

See [`docs/wasm.md`](docs/wasm.md) for runtime requirements, the JSON fast path, error handling, adapter usage, `opportune/presets` for browser and module Web Worker bundles, and the gzipped bundle-size budget.

## Documentation

Full documentation publishes to [azx-pbc-oss.github.io/opportune](https://azx-pbc-oss.github.io/opportune/):

- [`docs/recipes.md`](docs/recipes.md) — nine scenarios worked through in Rust, Python, and TypeScript, plus integration notes on occurrences, weighting, fetch planning, and engine lifecycle (start here for scheduling rules and integration patterns).
- [`docs/api.md`](docs/api.md) — Rust core, JSON wire format, provider adapters, and Python/FastAPI reference.
- [`docs/wasm.md`](docs/wasm.md) — npm package, TypeScript types, adapters, and WASM bundle-size budget.
- [`docs/design.md`](docs/design.md) — core design principles and semantics.
- [`docs/crates.md`](docs/crates.md) — workspace architecture and crate boundaries.
- [`docs/dependencies.md`](docs/dependencies.md) — dependency and license policy.

## Contributing & Security

- **Contributing:** See [`CONTRIBUTING.md`](CONTRIBUTING.md) for prerequisites, workspace layout, `make` targets, code generation, Conventional Commits rules, and release procedures.
- **Security:** See [`SECURITY.md`](SECURITY.md) to report vulnerabilities privately via GitHub Security Advisories.

## License

Dual-licensed under either the [MIT license](https://github.com/AZX-PBC-OSS/opportune/blob/main/LICENSE-MIT) or the [Apache-2.0 license](https://github.com/AZX-PBC-OSS/opportune/blob/main/LICENSE-APACHE), at your option — AZX, PBC. <oss@azx.io>
