# opportune-json

JSON wire types, validating conversions, RFC 6901 JSON-pointer errors, Draft 2020-12 JSON Schema export, and versioned document upgrades for the [`opportune`](https://crates.io/crates/opportune) scheduling engine.

- **Separate wire and domain types:** parsing validates and converts JSON documents into `opportune` domain types without altering engine semantics.
- **Deterministic output:** `to_json` serializes ordered maps so identical values produce byte-identical JSON across platforms.
- **Actionable errors:** `JsonError` carries a stable `code()`, an RFC 6901 `path()` pointing to the offending field, and full validation diagnostics in `errors()`.
- **Schema & upgrades:** `schema()` (enabled via the default `schema` feature) exports one Draft 2020-12 JSON Schema bundle, `SCHEMA_VERSION` is the version this build writes, and `upgrade_rule`, `upgrade_calendar`, and `upgrade_goal` migrate stored documents to the current schema version. `From<TimeError> for JsonError` lets a handler use `?` on civil-time constructors.

## Install

```sh
cargo add opportune opportune-json
```

To exclude `schemars` in size-constrained builds:

```sh
cargo add opportune-json --no-default-features
```

## Quickstart

```rust
use opportune::Engine;
use opportune_json::{WireResponse, parse_request, to_json};

fn evaluate(document: &str) -> Result<String, Box<dyn std::error::Error>> {
    let request = parse_request(document)?;
    let engine = Engine::builder().build()?;
    let response = engine.find_availability(&request)?;
    Ok(to_json(&WireResponse::from(&response))?)
}
```

You can also run the full JSON-in, JSON-out pipeline in one call with `opportune_json::find_availability(&engine, request_json)`, `check_slot`, `validate`, and `required_ranges`; each takes the `&Engine` to run on.

## Documentation

- [JSON wire layer reference](https://azx-pbc-oss.github.io/opportune/api/#json-wire-layer-opportune-json)
- [Documentation site](https://azx-pbc-oss.github.io/opportune/)
- [Source repository](https://github.com/AZX-PBC-OSS/opportune)

## License

Dual-licensed under [MIT](https://github.com/AZX-PBC-OSS/opportune/blob/main/LICENSE-MIT) or [Apache-2.0](https://github.com/AZX-PBC-OSS/opportune/blob/main/LICENSE-APACHE), at your option.
