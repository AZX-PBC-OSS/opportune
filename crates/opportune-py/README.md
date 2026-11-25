# opportune

Typed Python bindings for the [`opportune`](https://github.com/AZX-PBC-OSS/opportune) Rust scheduling engine: compute calendar availability across working hours, buffers, notice windows, rest rules, travel holds, and workload caps, and verify proposed bookings at confirmation time.

- **Native Rust engine:** packaged as `cp312-abi3` wheels (Python 3.12+) with the IANA time zone database bundled. Every engine call releases the GIL.
- **Pydantic v2 models:** generated from the wire JSON Schema, ready to declare directly on FastAPI routes and persist as JSON documents. Every `int32`/`uint32`/`uint64` field publishes its exact bounds and enforces them at validation.
- **Provider adapters included:** `opportune.adapters.graph` and `opportune.adapters.google` convert Microsoft Graph and Google Calendar payloads into domain blocks and rules.
- **Async & public-route helpers:** `*_async` methods offload CPU work with `asyncio.to_thread` (every one accepts a keyword-only `executor=` for a bounded `ThreadPoolExecutor`), module-level `*_json` and `*_json_async` forms return the engine's own text, and `public_response` / `public_slot_check` strip operator-only diagnostics for public booking endpoints.
- **Safe host scoping:** `presets.and_who(rule, {"kind": "person"})` narrows `who` without dropping the default `{"role": "host"}` clause, so working hours, rest and caps never start judging attendees, while `presets.except_who(rule, {"ids": ["ceo"]})` exempts named participants. Both are also exported as `opportune.and_who` / `opportune.except_who`.
- **Partial limits:** `opportune.Limits(max_participants=50)` overrides one cap; every omitted field reads as the engine default.

## Install

```sh
pip install opportune
```

or with `uv`:

```sh
uv add opportune
```

Requires Python 3.12+ and `pydantic>=2.14`.

## Quickstart

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

For web services, build one `opportune.Engine()` at startup (for example, in a FastAPI `lifespan` handler) and call `await engine.find_availability_async(request)`. Every `*_async` entry point takes an optional keyword-only `executor=`; pass a dedicated bounded `concurrent.futures.ThreadPoolExecutor` to keep CPU-bound searches off the event loop's shared default pool. `engine.find_availability_json_async(request)` returns the engine's own document text; the module-level `opportune.find_availability`, `check_slot`, `validate`, `required_ranges` and their `*_json`, `*_async` and `*_json_async` forms answer from one shared engine.

## Documentation

- [Python API reference](https://azx-pbc-oss.github.io/opportune/api/#python)
- [Worked recipes (Rust, Python, TypeScript)](https://azx-pbc-oss.github.io/opportune/recipes/)
- [Documentation site](https://azx-pbc-oss.github.io/opportune/)
- [Source repository](https://github.com/AZX-PBC-OSS/opportune)

## License

Dual-licensed under [MIT](https://github.com/AZX-PBC-OSS/opportune/blob/main/LICENSE-MIT) or [Apache-2.0](https://github.com/AZX-PBC-OSS/opportune/blob/main/LICENSE-APACHE), at your option.
