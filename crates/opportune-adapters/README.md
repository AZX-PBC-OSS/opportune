# opportune-adapters

Microsoft Graph (Entra / Outlook) and Google Calendar payload conversions into [`opportune`](https://crates.io/crates/opportune) domain `Block`s and `Rule`s.

- **Pure translation layer:** no network, filesystem, or system clock access — identical results on native and `wasm32` targets.
- **Fail-closed outcomes:** every conversion returns a `CalendarOutcome` (`Available` or `Unavailable`). Provider-reported mailbox or calendar errors never collapse into an empty ("completely free") block list.
- **Pagination is part of the payload:** follow Graph `@odata.nextLink` and Google `nextPageToken` before converting; a single page is a `payload_invalid` outcome.
- **Shared marker convention:** reads `opportune.tags` and `opportune.event_key` from Graph single-value extended properties (`graph::EXTENDED_PROPERTY_EXPAND`) and Google `extendedProperties`. Graph occurrences add their `originalStart` slot (`<key>@<utc>`) so one series is not counted as one event.
- **Diagnostics, not failures:** a Graph category with no ASCII slug (for example a non-Latin name, when the category map has no entry) still converts its event; it makes no `category/...` tag and emits a non-fatal `category_tag_unmappable` diagnostic naming the event and the category, capped at 16 findings per event plus one summary naming how many more were omitted.

## Install

```sh
cargo add opportune opportune-adapters
```

### Feature Flags

| Feature | Default | Effect |
| --- | --- | --- |
| `graph` | on | `graph::calendar_view_to_blocks`, `get_schedule_to_blocks`, `working_hours_to_rule` |
| `google` | on | `google::events_list_to_blocks`, `free_busy_to_blocks` |
| `windows-zones` | on | Resolves Windows time zone names via CLDR data (`Unicode-DFS-2016`) |
| `bundled-tzdb` | on | Enables bundled-tzdb JSON adapter entry points |
| `json` | off | JSON envelope wrappers for language bindings; with `bundled-tzdb` the short `json::*` names, otherwise the `json::*_with_db` names take a caller database |
| `schema` | off | Adapter JSON Schema definitions |

## Quickstart

```rust
use opportune::time::{TimeZoneDatabase, ZoneName};
use opportune_adapters::{CalendarOutcome, graph};

fn convert_graph(payload: &str) -> Result<(), Box<dyn std::error::Error>> {
    let db = TimeZoneDatabase::bundled()?;
    let zone = ZoneName::new("America/New_York")?;

    match graph::calendar_view_to_blocks(&db, payload, &zone, None) {
        CalendarOutcome::Available { blocks, diagnostics } => {
            println!("{} blocks, {} diagnostics", blocks.len(), diagnostics.len());
        }
        CalendarOutcome::Unavailable { code, message } => {
            eprintln!("calendar unavailable ({code}): {message}");
        }
    }
    Ok(())
}
```

## Documentation

- [Provider adapters reference](https://azx-pbc-oss.github.io/opportune/api/#provider-adapters-opportune-adapters)
- [Provider payload recipe](https://azx-pbc-oss.github.io/opportune/recipes/#graph-and-google-provider-payloads)
- [Documentation site](https://azx-pbc-oss.github.io/opportune/)
- [Source repository](https://github.com/AZX-PBC-OSS/opportune)

## License

Dual-licensed under [MIT](https://github.com/AZX-PBC-OSS/opportune/blob/main/LICENSE-MIT) or [Apache-2.0](https://github.com/AZX-PBC-OSS/opportune/blob/main/LICENSE-APACHE), at your option.
