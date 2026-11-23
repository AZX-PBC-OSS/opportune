# opportune

Pure-Rust calendar availability and scheduling engine: compute bookable times across participants' calendars, working hours, buffers, notice windows, rest rules, travel holds, and workload caps, and validate proposed bookings at confirmation time.

- **Deterministic and pure:** `now` is an explicit input, with no ambient clock, network, or filesystem I/O.
- **Explicit time zone data:** uses the bundled IANA time zone database (`bundled-tzdb`, enabled by default) or a caller-supplied `TZif` directory — never the host OS clock or system zoneinfo.
- **All seven rule kinds:** `window`, `blackout`, `clearance`, `lead_time`, `status_policy`, `rest`, and `cap`, plus multi-participant weighted matching and structured exclusion reasons.
- **Safe host scoping:** `Rule::and_who` intersects a selector into a rule's `who`, so `rule.and_who(Selector::kind(ParticipantKind::Person))` keeps the default `Role::Host` clause and reaches human hosts alone — neither rooms nor attendees. `Rule::except_who` exempts one, as in `rule.except_who(Selector::ids(["ceo"]))`; `with_who` replaces `who` outright and would also judge attendees.

## Install

```sh
cargo add opportune
```

Disable the bundled time zone database if you supply your own `TZif` directory:

```sh
cargo add opportune --no-default-features
```

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
    for offer in &response.offers {
        let start = offer.start().ok_or("an offer has an event")?;
        println!("{start} — {}", offer.assigned_hosts().join(", "));
    }
    Ok(())
}
```

## Companion Crates

- [`opportune-json`](https://crates.io/crates/opportune-json): JSON wire types, RFC 6901 pointer diagnostics, Draft 2020-12 JSON Schema export, and versioned upgrades.
- [`opportune-adapters`](https://crates.io/crates/opportune-adapters): Microsoft Graph and Google Calendar payload conversions.

## Documentation

- [Documentation site](https://azx-pbc-oss.github.io/opportune/)
- [API reference](https://azx-pbc-oss.github.io/opportune/api/)
- [Worked recipes](https://azx-pbc-oss.github.io/opportune/recipes/)
- [Source repository](https://github.com/AZX-PBC-OSS/opportune)

## License

Dual-licensed under [MIT](https://github.com/AZX-PBC-OSS/opportune/blob/main/LICENSE-MIT) or [Apache-2.0](https://github.com/AZX-PBC-OSS/opportune/blob/main/LICENSE-APACHE), at your option.
