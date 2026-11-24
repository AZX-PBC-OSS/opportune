# Recipes

Nine worked scenarios, from a one-person booking link to a traveling field
engineer, a provider-fed enterprise deployment, and the three-tier policies,
team-and-room requests and weighted panel reschedules an enterprise adopter
assembles from them. Each one is a complete request
and the answer it produces, so it can be pasted into a service and changed one
field at a time. Every block is self-contained: the small `utc` helper that builds
a UTC instant with `Instant::from_utc` is repeated in each recipe rather than
shared, so a block never depends on another block's preamble.

The Rust blocks are the files under
`crates/opportune/tests/snippets/`, compiled and run on every change:
`crates/opportune/tests/docs_snippets.rs` executes them against the live crate
and fails if this page stops quoting them, and
`crates/opportune-adapters/tests/docs_snippets.rs` compiles and runs the provider
recipe, which needs the adapters crate — the check that this page still quotes it
is the core test's. The Python and TypeScript blocks are the same scenarios on
the other two surfaces, and they are checked too: every `python` and `ts` fence on
this page is classified for `make check-docs-snippets`, and the Python and
TypeScript lanes type-check each block and run it against the installed package.
Each block uses only APIs the [API reference](api.md) and
[WASM and TypeScript](wasm.md) pages document.

Two spellings are worth knowing before reading the TypeScript blocks. The
generated document types use the wire's `snake_case` field names
(`search_window`, `working_days`, `event_key`), because they are generated from
the JSON Schema; the preset and sugar helpers take `camelCase` options
(`searchWindow`, `zone`, `ruleId`). Python is `snake_case` throughout, and its
generated models are aliased so `Meta.schema_` is the field while `schema` is the
wire key.

The nine scenarios are followed by [production blueprints](#production-blueprints)
for the decisions every booking service has to make outside the engine: committing
a slot atomically, weighting a round-robin pool, counting a shared event once,
running one engine per limit profile, and the v1 product-model limits below.

## Before you build: v1 product model

- **One part per goal.** `Goal.parts` is an array for forward compatibility, but
  v1 requires exactly one part. A larger list is rejected with `part_count`;
  `Goal::single` and `Goal::single_part` build the supported shape.
- **`status_policy` is first-match-wins across rules.** A three-tier
  org → team → host application must pass the most specific policy first, for
  example `[host, team, org]`; otherwise the broad policy's first matching entry
  decides and the narrower tier never gets a say.
- **`Selector::kind(Person)` includes attendees.** To scope a rule to human
  *hosts* and away from both resource rooms and invitees, use
  `Selector::all([Selector::role(Role::Host), Selector::kind(ParticipantKind::Person)])`.
- **`calendar_unknown` is host-only.** The engine flags offers whose assigned host
  had no calendar. Attendee and invitee calendar fetches are the application's to
  track and surface.
- **Group classes and webinars are application objects.** A booking that admits N
  attendees is not a `cap`; model fixed seats as `Resource` participants (each
  booked once) or manage seat capacity in the product layer.

## A personal 30-minute booking link

One host, one meeting type, a week of weekdays, ten-minute buffers and four
hours' notice. The answer is the booking page: `Response.dates` is every date the
window touches with its offer count, and `Response.offers` is every bookable
start with who could take it.

### Rust

```rust
// A personal 30-minute booking link, and the booking page it fills.

use std::collections::BTreeMap;

use opportune::Engine;
use opportune::model::{Goal, Participant, Request, Requirement, Span, Term};
use opportune::presets;
use opportune::time::{
    Date, Duration, Instant, RuleZone, TimeOfDay, WallEnd, Weekday, Weekdays, ZoneName,
};

/// The UTC instant of a civil date and time, for readable request data.
fn utc(date: (i16, i8, i8), clock: (u8, u8)) -> Instant {
    Instant::from_utc(
        Date::new(date.0, date.1, date.2).expect("a valid civil date"),
        TimeOfDay::new(clock.0, clock.1).expect("a valid time of day"),
    )
    .expect("UTC has no transitions")
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let london = ZoneName::new("Europe/London")?;
    let now = utc((2027, 6, 14), (7, 0)); // Monday, 08:00 in London
    let window = Span::new(utc((2027, 6, 14), (0, 0)), utc((2027, 6, 19), (0, 0)))?;
    let mut request = Request::new(
        now,
        Goal::single(
            london.clone(),
            window,
            Duration::from_minutes(30),
            Requirement::new(vec![Term::person("rich")]),
        ),
    );
    request
        .participants
        .push(Participant::new("rich", london).with_calendar(vec![]));
    request.rules.push(presets::business_hours(
        RuleZone::Participant,
        Weekdays::range(Weekday::Monday, Weekday::Friday),
        TimeOfDay::new(9, 0)?,
        WallEnd::Time(TimeOfDay::new(17, 0)?),
    )?);
    request
        .rules
        .push(presets::buffers(Duration::from_minutes(10), Duration::from_minutes(10)));
    request
        .rules
        .push(presets::min_notice(Duration::from_hours_minutes(4, 0)));

    let engine = Engine::builder().build()?;
    let response = engine.find_availability(&request)?;

    // A booking page: every date the window touches gets a row, and each offer is
    // listed under its date in the display zone.
    let display = engine
        .time_zone_database()
        .zone(request.goal.display_zone.as_str())?;
    let mut page: BTreeMap<String, Vec<String>> = response
        .dates
        .iter()
        .map(|count| (count.date.to_string(), Vec::new()))
        .collect();
    for offer in &response.offers {
        let start = offer.start().ok_or("an offer has an event")?;
        page.entry(display.zoned(start).date().to_string())
            .or_default()
            .push(start.to_string());
    }

    // Four hours of notice from Monday 08:00 opens the week at 12:00 London.
    assert_eq!(page["2027-06-14"].first().map(String::as_str), Some("2027-06-14T11:00Z"));
    assert_eq!(page["2027-06-14"].len(), 10);
    assert_eq!(page["2027-06-15"].len(), 16);
    // The window reaches into Saturday, so the page lists it with nothing to book.
    assert!(page["2027-06-19"].is_empty());
    Ok(())
}
```

`calendar=[]` rather than an omitted calendar is deliberate: an absent calendar
means "unknown", and every offer resting on that host carries the
`calendar_unknown` flag. A host known to be free has an empty one.

### Python

<!-- snippet: personal-link-python -->
```python
from datetime import datetime
from zoneinfo import ZoneInfo

import opportune

request = opportune.Request(
    now="2027-06-14T07:00Z",
    goal=opportune.single_goal(
        "30m",
        opportune.Requirement(all=[opportune.person("rich")]),
        zone="Europe/London",
        search_window=("2027-06-14T00:00Z", "2027-06-19T00:00Z"),
    ),
    participants=[opportune.Participant(id="rich", zone="Europe/London", calendar=[])],
    rules=[
        opportune.presets.business_hours("mon-fri", "09:00-17:00"),
        opportune.presets.buffers("10m", "10m"),
        opportune.presets.min_notice("4h"),
    ],
)

response = opportune.find_availability(request)

display = ZoneInfo(request.goal.display_zone or request.goal.zone)
page: dict[str, list[str]] = {count.date: [] for count in response.dates}
for offer in response.offers:
    start = opportune.offer_start(offer)
    date = datetime.fromisoformat(start).astimezone(display).date().isoformat()
    page[date].append(start)

assert page["2027-06-14"][0] == "2027-06-14T11:00Z"
assert len(page["2027-06-14"]) == 10
```

In a FastAPI route the same code path is one line — see the
[lifespan and async entry points](api.md#fastapi-a-lifespan-engine-and-the-async-entry-points)
in the API reference — and the answer can be handed on as the engine's own text
with `find_availability_json`.

### TypeScript

<!-- snippet: personal-link-ts -->
```ts
import {
  Engine, businessHours, buffers, minNotice, offerStart, person, singleGoal,
} from "opportune";
import type { Request } from "opportune";

const request: Request = {
  now: "2027-06-14T07:00Z",
  goal: singleGoal("30m", { all: [person("rich")] }, {
    zone: "Europe/London",
    searchWindow: ["2027-06-14T00:00Z", "2027-06-19T00:00Z"],
  }),
  participants: [{ id: "rich", zone: "Europe/London", calendar: [] }],
  rules: [businessHours(), buffers(), minNotice("4h")],
};

using engine = new Engine();
const response = engine.findAvailability(request);

// `Response.dates` is keyed in the display zone, which is the event zone unless
// the goal names another one.
const dayOf = (instant: string) =>
  new Intl.DateTimeFormat("en-CA", {
    timeZone: request.goal.display_zone ?? request.goal.zone,
  }).format(new Date(instant));
const page = new Map<string, string[]>(response.dates.map((count) => [count.date, []]));
for (const offer of response.offers) {
  const start = offerStart(offer);
  page.get(dayOf(start))?.push(start);
}

const monday = page.get("2027-06-14");
if (monday === undefined || monday.length !== 10 || monday[0] !== "2027-06-14T11:00Z") {
  throw new Error("the booking page does not contain Monday's ten expected starts");
}
console.log(monday);
```

A route that only forwards the answer should return `engine.findAvailabilityJson(request)`
instead: the engine's own bytes, with no parse and no second write.

## Fair interview panels with workload caps

One interview slot, several interviewers, and three load limits at once: a pooled
weekly cap on the whole pool, a per-interviewer daily cap, and a weekly minutes
cap per interviewer. The terms select any one interviewer tagged `interviewer`,
which is what makes the request a panel rather than a named-person booking.

Fairness between interviewers is a weighting decision rather than a cap:
`Request.weights` prefers a host when the assignment is chosen, and a dispatcher
that books one interview at a time updates the weights between searches. The caps
here are what stop any one interviewer being over-booked within a week.

### Rust

```rust
// An interview panel under a pooled weekly cap, a per-interviewer
// daily cap, and a weekly minutes cap.

use opportune::Engine;
use opportune::model::{
    Block, CapPer, Explain, Goal, Participant, Part, Request, Requirement, Span, Term,
};
use opportune::presets;
use opportune::reasons::{ReasonCode, ReasonParams};
use opportune::tags::{Selector, Tag};
use opportune::time::{
    Date, Duration, Instant, RuleZone, TimeOfDay, WallEnd, Weekday, Weekdays, ZoneName,
};

fn tag(path: &str) -> Tag {
    Tag::new(path).expect("a valid tag path")
}

fn utc(date: (i16, i8, i8), clock: (u8, u8)) -> Instant {
    Instant::from_utc(
        Date::new(date.0, date.1, date.2).expect("a valid civil date"),
        TimeOfDay::new(clock.0, clock.1).expect("a valid time of day"),
    )
    .expect("UTC has no transitions")
}

fn span(start: Instant, end: Instant) -> Span {
    Span::new(start, end).expect("a non-empty span")
}

fn booked(start: Instant, end: Instant, key: &str) -> Block {
    let mut block = Block::new(span(start, end));
    block.tags = vec![tag("interview")];
    block.event_key = Some(key.to_owned());
    block
}

fn interviewer(id: &str, calendar: Vec<Block>) -> Participant {
    let mut host = Participant::new(id, ZoneName::new("UTC").expect("UTC is a zone name"));
    host.tags = vec![tag("interviewer")];
    host.calendar = Some(calendar);
    host
}

/// The three interviewers and what is already on their calendars this week.
fn panel() -> Vec<Participant> {
    vec![
        interviewer(
            "ana",
            vec![
                booked(utc((2027, 6, 14), (9, 0)), utc((2027, 6, 14), (10, 0)), "ana-1"),
                booked(utc((2027, 6, 14), (10, 0)), utc((2027, 6, 14), (11, 0)), "ana-2"),
            ],
        ),
        interviewer(
            "bo",
            vec![
                booked(utc((2027, 6, 14), (9, 0)), utc((2027, 6, 14), (10, 0)), "bo-1"),
                booked(utc((2027, 6, 15), (9, 0)), utc((2027, 6, 15), (12, 0)), "bo-2"),
            ],
        ),
        interviewer("cy", Vec::new()),
    ]
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let utc_zone = ZoneName::new("UTC")?;
    let part = Part {
        id: "interview".to_owned(),
        tags: vec![tag("interview")],
        ..Part::new(
            Duration::from_minutes(60),
            Requirement::new(vec![Term::at_least(1, Selector::tag(tag("interviewer")))]),
        )
    };

    let mut request = Request::new(
        utc((2027, 6, 13), (12, 0)),
        Goal::new(
            utc_zone,
            span(utc((2027, 6, 14), (0, 0)), utc((2027, 6, 15), (0, 0))),
            vec![part],
        ),
    );
    request.participants = panel();
    request.rules = vec![
        presets::business_hours(
            RuleZone::Participant,
            Weekdays::range(Weekday::Monday, Weekday::Friday),
            TimeOfDay::new(9, 0)?,
            WallEnd::Time(TimeOfDay::new(17, 0)?),
        )?,
        presets::cap_per_day(Selector::tag(tag("interview")), 2),
        presets::cap_per_week(
            Selector::tag(tag("interview")),
            8,
            CapPer::Pooled,
            RuleZone::Event,
        ),
        presets::cap_minutes_per_week(
            Selector::tag(tag("interview")),
            240,
            CapPer::Each,
            RuleZone::Participant,
        ),
    ];
    request.limit = Some(50);

    let engine = Engine::builder().build()?;
    let response = engine.find_availability(&request)?;

    // Ana is at her daily limit and Bo at his weekly minutes, so every Monday slot
    // goes to Cy, the only interviewer with room left.
    assert_eq!(response.offers.len(), 15);
    assert!(response.offers.iter().all(|offer| {
        offer.events[0].eligible == vec![vec!["cy".to_owned()]]
            && offer.events[0].assignment == vec![vec!["cy".to_owned()]]
    }));
    assert_eq!(
        response
            .dates
            .iter()
            .find(|count| count.date == Date::new(2027, 6, 14).expect("a date"))
            .map(|count| count.offers),
        Some(15)
    );

    // Lowering the pooled weekly cap to the four interviews already booked closes
    // the pool for the week, and the explanation names that cap and its bucket.
    let mut pooled = request.clone();
    pooled.rules.retain(|rule| rule.id != "cap_per_week");
    pooled.rules.push(presets::cap_per_week(
        Selector::tag(tag("interview")),
        4,
        CapPer::Pooled,
        RuleZone::Event,
    ));
    pooled.explain = Some(Explain { limit: 10 });
    pooled.goal.search_window = span(utc((2027, 6, 14), (9, 0)), utc((2027, 6, 14), (11, 0)));
    let closed = engine.find_availability(&pooled)?;
    assert!(closed.offers.is_empty());
    let entry = closed
        .excluded
        .iter()
        .find(|entry| entry.start == utc((2027, 6, 14), (9, 0)))
        .expect("the first business-hours start is excluded");
    let reason = entry
        .reasons
        .iter()
        .find(|reason| reason.rule.as_deref() == Some("cap_per_week"))
        .expect("the pooled cap is one of the reasons");
    assert_eq!(reason.code, ReasonCode::CapReached);
    assert_eq!(reason.participant.as_deref(), Some("cy"));
    assert_eq!(
        reason.params,
        Some(ReasonParams::CapReached {
            used: 4,
            limit: 4,
            bucket: "2027-06-14".to_owned(),
        })
    );
    Ok(())
}
```

A pooled cap counts an event once however many participants' calendars hold it,
which is why the existing interviews carry `event_key`s: two blocks sharing a key
are one event. The reason's `params` reports the bucket's usage before the
booking's contribution, so a UI can say "4 of 4 interviews this week".

### Python

<!-- snippet: panel-python -->
```python
import opportune

goal = opportune.single_goal(
    "60m",
    opportune.Requirement(all=[opportune.at_least(1, {"tag": "interviewer"})]),
    zone="UTC",
    search_window=("2027-06-14T00:00Z", "2027-06-15T00:00Z"),
    part_id="interview",
    tags=["interview"],
)


def booked(start: str, end: str, key: str) -> opportune.Block:
    return opportune.Block(span=(start, end), tags=["interview"], event_key=key)


def interviewer(id_: str, calendar: list[opportune.Block]) -> opportune.Participant:
    return opportune.Participant(id=id_, zone="UTC", tags=["interviewer"], calendar=calendar)


request = opportune.Request(
    now="2027-06-13T12:00Z",
    goal=goal,
    participants=[
        interviewer("ana", [booked("2027-06-14T09:00Z", "2027-06-14T10:00Z", "ana-1"),
                            booked("2027-06-14T10:00Z", "2027-06-14T11:00Z", "ana-2")]),
        interviewer("bo", [booked("2027-06-14T09:00Z", "2027-06-14T10:00Z", "bo-1"),
                           booked("2027-06-15T09:00Z", "2027-06-15T12:00Z", "bo-2")]),
        interviewer("cy", []),
    ],
    rules=[
        # The ordinary week in one rule, so a split shift or a holiday would be one
        # entry rather than several `business_hours` rules that intersect to nothing.
        opportune.presets.weekly_hours({"mon-fri": "09:00-17:00"}),
        opportune.presets.cap_per_day({"tag": "interview"}, 2, rule_id="panel_day"),
        opportune.presets.cap_per_week(
            {"tag": "interview"}, 8, per="pooled", zone="event", rule_id="panel_week"
        ),
        # No `cap_minutes_per_week` helper: the week cap measures minutes instead.
        opportune.presets.cap_per_week(
            {"tag": "interview"}, 240, measure="minutes", rule_id="panel_minutes"
        ),
    ],
    limit=50,
)

response = opportune.find_availability(request)
assert response.offers
assert all(opportune.offer_assigned_hosts(offer) == ["cy"] for offer in response.offers)

# The pooled cap, and the bucket that explains it.
tight = opportune.with_updates(
    request,
    explain=opportune.Explain(limit=10),
    goal=opportune.with_updates(goal, search_window=("2027-06-14T09:00Z", "2027-06-14T11:00Z")),
    rules=[
        *[rule for rule in request.rules or [] if rule.id != "panel_week"],
        opportune.presets.cap_per_week(
            {"tag": "interview"}, 4, per="pooled", zone="event", rule_id="panel_week"
        ),
    ],
)
closed = opportune.find_availability(tight)
entry = next(
    item for item in closed.excluded or [] if item.start == "2027-06-14T09:00Z"
)
reason = next(item for item in entry.reasons or [] if item.rule == "panel_week")
params = reason.params
assert isinstance(params, opportune.ReasonParams)
assert (reason.code, reason.participant, params.used, params.limit) == (
    "cap_reached", "cy", 4, 4
)
```

### TypeScript

<!-- snippet: panel-ts -->
```ts
import {
  Engine, atLeast, capPerDay, capPerWeek, offerAssignedHosts, singleGoal, weeklyHours,
} from "opportune";
import type { Block, Participant, Request } from "opportune";

const requirement = { all: [atLeast(1, { tag: "interviewer" })] };
const goal = singleGoal("60m", requirement, {
  zone: "UTC",
  searchWindow: ["2027-06-14T00:00Z", "2027-06-15T00:00Z"],
  partId: "interview",
  tags: ["interview"],
});

const booked = (start: string, end: string, key: string): Block => ({
  span: [start, end], tags: ["interview"], event_key: key,
});
const interviewer = (id: string, calendar: Block[]): Participant => ({
  id, zone: "UTC", tags: ["interviewer"], calendar,
});

const request: Request = {
  now: "2027-06-13T12:00Z",
  goal,
  participants: [
    interviewer("ana", [
      booked("2027-06-14T09:00Z", "2027-06-14T10:00Z", "ana-1"),
      booked("2027-06-14T10:00Z", "2027-06-14T11:00Z", "ana-2"),
    ]),
    interviewer("bo", [
      booked("2027-06-14T09:00Z", "2027-06-14T10:00Z", "bo-1"),
      booked("2027-06-15T09:00Z", "2027-06-15T12:00Z", "bo-2"),
    ]),
    interviewer("cy", []),
  ],
  rules: [
    // The ordinary week in one rule: a split shift or a closed holiday is one
    // entry instead of several `businessHours` rules that would intersect away.
    weeklyHours({ "mon-fri": "09:00-17:00" }),
    capPerDay({ tag: "interview" }, 2, { ruleId: "panel_day" }),
    capPerWeek({ tag: "interview" }, 8, { per: "pooled", zone: "event", ruleId: "panel_week" }),
    capPerWeek({ tag: "interview" }, 240, { measure: "minutes", ruleId: "panel_minutes" }),
  ],
  limit: 50,
};

using engine = new Engine();
const offers = engine.findAvailability(request).offers;
if (offers.length !== 15) throw new Error(`expected 15 offers, got ${offers.length}`);
for (const offer of offers) {
  const hosts = offerAssignedHosts(offer);
  if (hosts.length !== 1 || hosts[0] !== "cy") {
    throw new Error(`expected only Cy, got ${hosts.join(", ")}`);
  }
}
console.log("all 15 offers assign only Cy");
```

## Rest, working-day horizons, and minimum notice

A late fundraiser protects the following morning, the horizon is a number of
*working* days on a named calendar rather than a duration, and a minimum notice
blocks the earliest starts. The three rules compose: each is an independent
constraint, and the first offer has to clear all of them.

### Rust

```rust
// Evening-to-morning rest, a working-day horizon, and a minimum notice.

use std::collections::BTreeMap;

use opportune::Engine;
use opportune::model::{
    Block, CalendarSpec, Goal, LeadTimeMax, Participant, Request, Requirement, Span, Term,
};
use opportune::presets;
use opportune::time::{
    Date, Duration, Instant, RuleZone, TimeOfDay, WallEnd, Weekday, Weekdays, WorkingDays, ZoneName,
};

fn utc(date: (i16, i8, i8), clock: (u8, u8)) -> Instant {
    Instant::from_utc(
        Date::new(date.0, date.1, date.2).expect("a valid civil date"),
        TimeOfDay::new(clock.0, clock.1).expect("a valid time of day"),
    )
    .expect("UTC has no transitions")
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let new_york = ZoneName::new("America/New_York")?;
    // Sunday 18:00 in New York, with a fundraiser that runs to 21:00.
    let now = utc((2027, 6, 13), (22, 0));
    let mut fundraiser = Block::new(Span::new(
        utc((2027, 6, 13), (23, 0)),
        utc((2027, 6, 14), (1, 0)),
    )?);
    fundraiser.event_key = Some("fundraiser".to_owned());

    let window = Span::new(utc((2027, 6, 14), (4, 0)), utc((2027, 6, 19), (4, 0)))?;
    let mut request = Request::new(
        now,
        Goal::single(
            new_york.clone(),
            window,
            Duration::from_minutes(30),
            Requirement::new(vec![Term::person("ana")]),
        ),
    );
    request
        .participants
        .push(Participant::new("ana", new_york.clone()).with_calendar(vec![fundraiser]));
    request.rules = vec![
        presets::business_hours(
            RuleZone::Participant,
            Weekdays::range(Weekday::Monday, Weekday::Friday),
            TimeOfDay::new(9, 0)?,
            WallEnd::Time(TimeOfDay::new(17, 0)?),
        )?,
        presets::rest(TimeOfDay::new(18, 0)?, TimeOfDay::new(10, 0)?),
        presets::min_notice(Duration::from_hours_minutes(16, 0)),
        presets::horizon(LeadTimeMax::WorkingDays {
            count: 4,
            calendar: "org".to_owned(),
        }),
    ];
    request.calendars = BTreeMap::from([(
        "org".to_owned(),
        CalendarSpec {
            zone: new_york.clone(),
            working_days: WorkingDays::weekdays(),
            holidays: Vec::new(),
        },
    )]);

    let engine = Engine::builder().build()?;
    let response = engine.find_availability(&request)?;

    // The late fundraiser protects Monday before 10:00, and sixteen hours of
    // notice from Sunday 18:00 lands at the same minute.
    let first = response.offers.first().and_then(opportune::Offer::start);
    assert_eq!(first, Some(utc((2027, 6, 14), (14, 0))));
    let monday = response
        .dates
        .iter()
        .find(|count| count.date == Date::new(2027, 6, 14).expect("a date"))
        .map(|count| count.offers);
    assert_eq!(monday, Some(14));

    // Four working days after Sunday is Thursday, so Friday stays empty even
    // though the search window reaches it.
    let friday = response
        .dates
        .iter()
        .find(|count| count.date == Date::new(2027, 6, 18).expect("a date"))
        .map(|count| count.offers);
    assert_eq!(friday, Some(0));
    let last = response.offers.last().and_then(opportune::Offer::start);
    let display = engine
        .time_zone_database()
        .zone(request.goal.display_zone.as_str())?;
    assert_eq!(
        last.map(|start| display.zoned(start).date()),
        Some(Date::new(2027, 6, 17).expect("a date"))
    );
    Ok(())
}
```

The calendar is what the horizon counts: a weekday calendar makes the weekend not
count, and a date in `CalendarSpec.holidays` is skipped the same way. A holiday
that should also *close* availability needs a `blackout` (or its own dated
window), because the calendar only answers which days count as working days.

### Python

<!-- snippet: rest-python -->
```python
import opportune

request = opportune.Request(
    now="2027-06-13T22:00Z",
    goal=opportune.single_goal(
        "30m",
        opportune.Requirement(all=[opportune.person("ana")]),
        zone="America/New_York",
        search_window=("2027-06-14T04:00Z", "2027-06-19T04:00Z"),
    ),
    participants=[
        opportune.Participant(
            id="ana",
            zone="America/New_York",
            calendar=[
                opportune.Block(
                    span=("2027-06-13T23:00Z", "2027-06-14T01:00Z"),
                    event_key="fundraiser",
                )
            ],
        )
    ],
    rules=[
        opportune.presets.business_hours(),
        opportune.presets.rest("18:00", "10:00"),
        opportune.presets.min_notice("16h"),
        # The typed form takes the wire names: `working_days`, not `count`.
        opportune.presets.horizon(
            opportune.WorkingDaysMax(calendar="org", working_days=4)
        ),
    ],
    calendars={"org": opportune.CalendarSpec(zone="America/New_York", working_days="mon-fri")},
)

response = opportune.find_availability(request)
assert opportune.offer_start(response.offers[0]) == "2027-06-14T14:00Z"
assert next(d.offers for d in response.dates if d.date == "2027-06-18") == 0
```

### TypeScript

<!-- snippet: rest-ts -->
```ts
import {
  Engine, businessHours, horizon, minNotice, offerStart, person, rest, singleGoal,
} from "opportune";
import type { Request } from "opportune";

const request: Request = {
  now: "2027-06-13T22:00Z",
  goal: singleGoal("30m", { all: [person("ana")] }, {
    zone: "America/New_York",
    searchWindow: ["2027-06-14T04:00Z", "2027-06-19T04:00Z"],
  }),
  participants: [{
    id: "ana",
    zone: "America/New_York",
    calendar: [{ span: ["2027-06-13T23:00Z", "2027-06-14T01:00Z"], event_key: "fundraiser" }],
  }],
  rules: [
    businessHours(),
    rest("18:00", "10:00"),
    minNotice("16h"),
    // A horizon is a wire document here, so its field is `working_days`.
    horizon({ calendar: "org", working_days: 4 }),
  ],
  calendars: { org: { zone: "America/New_York", working_days: "mon-fri" } },
};

using engine = new Engine();
const response = engine.findAvailability(request);
const [first] = response.offers;
if (first === undefined) throw new Error("the request offers no start");
const firstStart = offerStart(first);
if (firstStart !== "2027-06-14T14:00Z") throw new Error(`unexpected first start: ${firstStart}`);
console.log(firstStart);
```

## A conference booth with traveling staff

A booth open only while the conference runs: `dated_hours` makes the event days
the whole window, so a date no key covers is closed. One delegate is traveling,
and their `zone_periods` move the zone their own working hours are read in to the
venue's for the trip.

### Rust

```rust
// A conference booth that is open only on the event's dated hours, with a
// delegate whose zone follows them to the venue.

use opportune::Engine;
use opportune::model::{
    Goal, Participant, Part, Request, Requirement, Span, Term, ZonePeriod,
};
use opportune::presets;
use opportune::tags::{Selector, Tag};
use opportune::time::{
    Date, DateKey, DateRange, Duration, Instant, RuleZone, TimeOfDay, WallEnd, WallRange, Weekday,
    Weekdays, ZoneName,
};

fn tag(path: &str) -> Tag {
    Tag::new(path).expect("a valid tag path")
}

fn utc(date: (i16, i8, i8), clock: (u8, u8)) -> Instant {
    Instant::from_utc(
        Date::new(date.0, date.1, date.2).expect("a valid civil date"),
        TimeOfDay::new(clock.0, clock.1).expect("a valid time of day"),
    )
    .expect("UTC has no transitions")
}

fn booth_day(day: i8) -> Result<DateKey, Box<dyn std::error::Error>> {
    Ok(DateKey::new(
        DateRange::single(Date::new(2027, 6, day)?),
        vec![WallRange::new(
            TimeOfDay::new(10, 0)?,
            WallEnd::Time(TimeOfDay::new(16, 0)?),
        )?],
    )?)
}

fn delegate(id: &str, home: &str) -> Result<Participant, Box<dyn std::error::Error>> {
    let mut host = Participant::new(id, ZoneName::new(home)?);
    host.tags = vec![tag("gtm")];
    host.calendar = Some(Vec::new());
    Ok(host)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let venue = ZoneName::new("America/New_York")?;
    let mut london = delegate("gtm-london", "Europe/London")?;
    // The London delegate is at the venue for the conference, so their own hours
    // are read in the venue's clock for those days.
    london.zone_periods = vec![ZonePeriod::new(
        venue.clone(),
        utc((2027, 6, 13), (0, 0)),
        utc((2027, 6, 17), (0, 0)),
    )?];

    let part = Part {
        id: "booth".to_owned(),
        tags: vec![tag("conference")],
        ..Part::new(
            Duration::from_minutes(20),
            Requirement::new(vec![Term::at_least(1, Selector::tag(tag("gtm")))]),
        )
    };

    let mut request = Request::new(
        utc((2027, 6, 1), (12, 0)),
        Goal::new(
            venue.clone(),
            Span::new(utc((2027, 6, 14), (0, 0)), utc((2027, 6, 17), (4, 0)))?,
            vec![part],
        ),
    );
    request.participants = vec![london, delegate("gtm-berlin", "Europe/Berlin")?];
    request.rules = vec![
        presets::dated_hours(
            RuleZone::Named(venue),
            vec![booth_day(14)?, booth_day(15)?, booth_day(16)?],
        )?
        .with_id("booth_hours"),
        presets::business_hours(
            RuleZone::Participant,
            Weekdays::range(Weekday::Monday, Weekday::Friday),
            TimeOfDay::new(9, 0)?,
            WallEnd::Time(TimeOfDay::new(17, 0)?),
        )?,
        presets::buffers(Duration::from_minutes(5), Duration::from_minutes(5)),
    ];

    let engine = Engine::builder().build()?;
    let response = engine.find_availability(&request)?;

    // Eighteen twenty-minute slots on each of the three booth days.
    assert_eq!(response.offers.len(), 54);
    for day in [14, 15, 16] {
        assert_eq!(
            response
                .dates
                .iter()
                .find(|count| count.date == Date::new(2027, 6, day).expect("a date"))
                .map(|count| count.offers),
            Some(18)
        );
    }
    let eligible_at = |hour: u8| -> Vec<Vec<String>> {
        response
            .offers
            .iter()
            .find(|offer| offer.start() == Some(utc((2027, 6, 14), (hour, 0))))
            .expect("the slot is offered")
            .events[0]
            .eligible
            .clone()
    };
    // At 10:00 New York both delegates are inside their own working hours; at
    // 15:00 only the one whose zone period moved them to the venue is.
    assert_eq!(
        eligible_at(14),
        vec![vec!["gtm-berlin".to_owned(), "gtm-london".to_owned()]]
    );
    assert_eq!(eligible_at(19), vec![vec!["gtm-london".to_owned()]]);

    // Without the zone period the London delegate keeps London hours, which end
    // at 12:00 in New York, and the afternoon slots disappear.
    let mut home_only = request.clone();
    home_only.participants[0].zone_periods.clear();
    let shortened = engine.find_availability(&home_only)?;
    assert_eq!(shortened.offers.len(), 18);
    Ok(())
}
```

`dated_hours` takes `DateKey` values: `booth_day` builds one from the `DateRange`
the key covers and the wall ranges the day is open, and a date no key covers is
closed. The rule's zone is `RuleZone::Named` here, so the same hours keep their
meaning if the venue's zone has a daylight-saving change during the event. The
Python and TypeScript blocks below spell the same keys as wire strings
(`{"2027-06-14": "10:00-16:00"}`), which is the form `DateKey` reads.

### Python

<!-- snippet: booth-python -->
```python
import opportune

london = opportune.Participant(
    id="gtm-london",
    zone="Europe/London",
    tags=["gtm"],
    calendar=[],
    zone_periods=[
        opportune.ZonePeriod(
            zone="America/New_York",
            from_="2027-06-13T00:00Z",
            until="2027-06-17T00:00Z",
        )
    ],
)
berlin = opportune.Participant(id="gtm-berlin", zone="Europe/Berlin", tags=["gtm"], calendar=[])

booth_days = {"2027-06-14": "10:00-16:00", "2027-06-15": "10:00-16:00",
              "2027-06-16": "10:00-16:00"}
request = opportune.Request(
    now="2027-06-01T12:00Z",
    goal=opportune.single_goal(
        "20m",
        opportune.Requirement(all=[opportune.at_least(1, {"tag": "gtm"})]),
        zone="America/New_York",
        search_window=("2027-06-14T00:00Z", "2027-06-17T04:00Z"),
    ),
    participants=[london, berlin],
    rules=[
        opportune.presets.dated_hours(booth_days, zone="America/New_York",
                                      rule_id="booth_hours"),
        opportune.presets.business_hours(),
        opportune.presets.buffers("5m", "5m"),
    ],
)

response = opportune.find_availability(request)
assert len(response.offers) == 54
assert [d.offers for d in response.dates if d.date == "2027-06-14"] == [18]
```

### TypeScript

<!-- snippet: booth-ts -->
```ts
import { Engine, atLeast, buffers, businessHours, datedHours, singleGoal } from "opportune";
import type { Request } from "opportune";

const request: Request = {
  now: "2027-06-01T12:00Z",
  goal: singleGoal("20m", { all: [atLeast(1, { tag: "gtm" })] }, {
    zone: "America/New_York",
    searchWindow: ["2027-06-14T00:00Z", "2027-06-17T04:00Z"],
  }),
  participants: [
    {
      id: "gtm-london", zone: "Europe/London", tags: ["gtm"], calendar: [],
      zone_periods: [{
        zone: "America/New_York",
        from: "2027-06-13T00:00Z",
        until: "2027-06-17T00:00Z",
      }],
    },
    { id: "gtm-berlin", zone: "Europe/Berlin", tags: ["gtm"], calendar: [] },
  ],
  rules: [
    datedHours(
      { "2027-06-14": "10:00-16:00", "2027-06-15": "10:00-16:00", "2027-06-16": "10:00-16:00" },
      { zone: "America/New_York", ruleId: "booth_hours" },
    ),
    businessHours(),
    buffers("5m", "5m"),
  ],
};

using engine = new Engine();
const offers = engine.findAvailability(request).offers;
if (offers.length !== 54) throw new Error(`expected 54 offers, got ${offers.length}`);
console.log(offers.length);
```

## Travel holds and confirm-time checking

An in-person visit needs travel either side, which the engine returns as reserve
spans for the caller to hold on the calendar. The same request then judges the
proposal at confirmation time, so the booking that is written is the one the
search offered.

### Rust

```rust
// A field-service visit with travel holds, confirmed at booking time.

use opportune::Engine;
use opportune::model::{
    Block, Goal, Participant, Part, Proposal, ProposalEvent, Request, Requirement, Span, Term,
    Travel,
};
use opportune::presets;
use opportune::reasons::ReasonCode;
use opportune::tags::Tag;
use opportune::time::{
    Date, Duration, Instant, RuleZone, TimeOfDay, WallEnd, Weekday, Weekdays, ZoneName,
};

fn utc(date: (i16, i8, i8), clock: (u8, u8)) -> Instant {
    Instant::from_utc(
        Date::new(date.0, date.1, date.2).expect("a valid civil date"),
        TimeOfDay::new(clock.0, clock.1).expect("a valid time of day"),
    )
    .expect("UTC has no transitions")
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let new_york = ZoneName::new("America/New_York")?;
    let mut prior_job = Block::new(Span::new(
        utc((2027, 3, 8), (18, 0)),
        utc((2027, 3, 8), (20, 0)),
    )?);
    prior_job.tags = vec![Tag::new("job/plumbing")?];
    prior_job.event_key = Some("job-101".to_owned());
    prior_job.travel_after = Duration::from_minutes(20);

    // The visit needs travel either side. The customer is at home, so their own
    // travel is zero even though the engineer's is not.
    let visit = Part {
        id: "visit".to_owned(),
        tags: vec![Tag::new("job/plumbing")?],
        travel: Travel::new(Duration::from_minutes(25), Duration::from_minutes(25))
            .with_participant("joe", Duration::ZERO, Duration::ZERO),
        ..Part::new(
            Duration::from_minutes(60),
            Requirement::new(vec![Term::person("bob")]),
        )
    };

    let mut goal = Goal::new(
        new_york.clone(),
        Span::new(utc((2027, 3, 8), (18, 0)), utc((2027, 3, 8), (23, 0)))?,
        vec![visit],
    );
    goal.candidates = Some(vec![utc((2027, 3, 8), (20, 54)), utc((2027, 3, 8), (20, 55))]);

    let mut request = Request::new(utc((2027, 3, 6), (12, 0)), goal);
    request.participants = vec![
        Participant::new("bob", new_york.clone()).with_calendar(vec![prior_job]),
        Participant::attendee("joe", new_york).with_calendar(vec![]),
    ];
    request.rules = vec![
        presets::business_hours(
            RuleZone::Participant,
            Weekdays::range(Weekday::Monday, Weekday::Saturday),
            TimeOfDay::new(8, 0)?,
            WallEnd::Time(TimeOfDay::new(18, 0)?),
        )?,
        presets::buffers(Duration::from_minutes(10), Duration::from_minutes(10)),
    ];

    let engine = Engine::builder().build()?;
    let response = engine.find_availability(&request)?;

    // Only the later candidate clears the prior job's travel and the buffer.
    let offer = response.offers.first().ok_or("the visit is bookable")?;
    assert_eq!(offer.start(), Some(utc((2027, 3, 8), (20, 55))));
    let holds: Vec<(&str, Instant, Instant)> = offer
        .reserve()
        .into_iter()
        .map(|reserve| (reserve.participant.as_str(), reserve.span.start(), reserve.span.end()))
        .collect();
    assert_eq!(
        holds,
        vec![
            ("bob", utc((2027, 3, 8), (20, 30)), utc((2027, 3, 8), (20, 55))),
            ("bob", utc((2027, 3, 8), (21, 55)), utc((2027, 3, 8), (22, 20))),
        ]
    );

    // Confirming it at booking time preserves the holds.
    let confirmed = engine.check_slot(&request, &offer.to_proposal())?;
    assert!(confirmed.valid);
    assert_eq!(confirmed.reserve.iter().collect::<Vec<_>>(), offer.reserve());

    // A start inside the prior job's travel is refused, naming the event.
    let too_soon = Proposal {
        events: vec![ProposalEvent {
            part: "visit".to_owned(),
            start: utc((2027, 3, 8), (20, 15)),
            assignment: vec![vec!["bob".to_owned()]],
        }],
    };
    let refused = engine.check_slot(&request, &too_soon)?;
    assert!(!refused.valid);
    let reason = refused.reasons.first().ok_or("the slot fails for a reason")?;
    assert_eq!(reason.code, ReasonCode::Blocked);
    assert_eq!(reason.event_key.as_deref(), Some("job-101"));
    Ok(())
}
```

`Offer::to_proposal` is the bridge between the two calls: the click that books a
slot hands the offer straight back, and `check_slot` answers every failing reason
with `valid: false` rather than one error. A malformed proposal — an unknown
part, the wrong number of terms or hosts — is a typed error instead; see
[`check_slot` and proposals](api.md#check_slot-and-proposals).

### Python

<!-- snippet: travel-python -->
```python
import opportune

goal = opportune.single_goal(
    "60m",
    opportune.Requirement(all=[opportune.person("bob")]),
    zone="America/New_York",
    search_window=("2027-03-08T18:00Z", "2027-03-08T23:00Z"),
    part_id="visit",
    tags=["job/plumbing"],
    travel=opportune.Travel(
        before="25m",
        after="25m",
        by_participant={"joe": opportune.ParticipantTravel(before="0m", after="0m")},
    ),
    candidates=["2027-03-08T20:54Z", "2027-03-08T20:55Z"],
)
request = opportune.Request(
    now="2027-03-06T12:00Z",
    goal=goal,
    participants=[
        opportune.Participant(
            id="bob",
            zone="America/New_York",
            calendar=[
                opportune.Block(
                    span=("2027-03-08T18:00Z", "2027-03-08T20:00Z"),
                    tags=["job/plumbing"],
                    event_key="job-101",
                    travel_after="20m",
                )
            ],
        ),
        opportune.Participant(id="joe", role="attendee", zone="America/New_York", calendar=[]),
    ],
    rules=[
        opportune.presets.business_hours("mon-sat", "08:00-18:00"),
        opportune.presets.buffers("10m", "10m"),
    ],
)

response = opportune.find_availability(request)
offer = response.offers[0]
assert opportune.offer_start(offer) == "2027-03-08T20:55Z"
assert [(hold.participant, hold.span) for hold in opportune.offer_reserve(offer)] == [
    ("bob", ("2027-03-08T20:30Z", "2027-03-08T20:55Z")),
    ("bob", ("2027-03-08T21:55Z", "2027-03-08T22:20Z")),
]

confirmed = opportune.check_slot(request, opportune.offer_to_proposal(offer))
assert confirmed.valid

too_soon = opportune.Proposal(
    events=[
        opportune.ProposalEvent(
            part="visit", start="2027-03-08T20:15Z", assignment=[["bob"]]
        )
    ]
)
refused = opportune.check_slot(request, too_soon)
assert not refused.valid
assert (refused.reasons[0].code, refused.reasons[0].event_key) == ("blocked", "job-101")
```

### TypeScript

<!-- snippet: travel-ts -->
```ts
import {
  Engine, buffers, businessHours, offerReserve, offerStart, offerToProposal,
  person, singleGoal,
} from "opportune";
import type { Request } from "opportune";

const request: Request = {
  now: "2027-03-06T12:00Z",
  goal: singleGoal("60m", { all: [person("bob")] }, {
    zone: "America/New_York",
    searchWindow: ["2027-03-08T18:00Z", "2027-03-08T23:00Z"],
    partId: "visit",
    tags: ["job/plumbing"],
    travel: { before: "25m", after: "25m", by_participant: { joe: { before: "0m", after: "0m" } } },
    candidates: ["2027-03-08T20:54Z", "2027-03-08T20:55Z"],
  }),
  participants: [
    {
      id: "bob",
      zone: "America/New_York",
      calendar: [{
        span: ["2027-03-08T18:00Z", "2027-03-08T20:00Z"],
        tags: ["job/plumbing"],
        event_key: "job-101",
        travel_after: "20m",
      }],
    },
    { id: "joe", role: "attendee", zone: "America/New_York", calendar: [] },
  ],
  rules: [businessHours("mon-sat", "08:00-18:00"), buffers("10m", "10m")],
};

using engine = new Engine();
const [offer] = engine.findAvailability(request).offers;
if (offer === undefined) throw new Error("the visit is bookable");
const start = offerStart(offer);
const holds = offerReserve(offer);
if (start !== "2027-03-08T20:55Z" || holds.length !== 2) {
  throw new Error("the visit does not carry its expected start and two travel holds");
}
console.log(start, holds);

const confirmed = engine.checkSlot(request, offerToProposal(offer));
if (!confirmed.valid) throw new Error("the offered visit no longer confirms");
console.log(confirmed.valid);
```

## A three-tier scheduling policy

One person, three tiers of policy over the same week. The global tier is the
ordinary week, a daily cap on interviews and evening-to-morning rest; the
`interview` group narrows the hours to Monday–Thursday 10:00–16:00 and carries its
own weekly count, which the block tightens to the three interviews already booked
to show the week closing; the `interview/frontend` event type adds 48 hours'
notice and a cap of one a day. A `vip` booking is *loosened* out of the global
and group windows and out of the global daily cap with `Rule::unless`, and given
its own wider hours — so the exception is wider than the rule it escapes rather
than an intersection with it that would leave nothing. The exception is from the
cap's judgment, not from its bucket: a `vip` interview already on a calendar
still carries the `interview` tag the daily cap counts, so it still spends the
budget an ordinary interview is measured against.

`unless` loosens *when* a rule applies. To narrow *who* it judges, use the
participant-side twins instead of replacing `who` outright: `Rule::and_who`
(`presets.and_who`, `andWho`) intersects a selector into the rule's `who`, and
`Rule::except_who` (`presets.except_who`, `exceptWho`) is the same call with
`not(selector)`. Because the intersection keeps the default `role: "host"`
clause, `and_who(Selector::kind(ParticipantKind::Person))` reaches human hosts
alone — neither resource rooms nor attendees — and `except_who(Selector::ids(["ceo"]))`
exempts the CEO from an otherwise universal rule. Setting `who: {"kind": "person"}`
on a builder would replace the host clause and start judging attendees.

### Rust

```rust
// A three-tier scheduling policy: the week's global hours, caps and rest; the
// interview group's narrower window and cap; and a frontend tier with its own
// notice and cap. The `vip` tier is *loosened* out of the broader tiers with
// `Rule::unless` rather than intersected with them, so the exception is wider
// than the rule it escapes instead of narrower.

use opportune::Engine;
use opportune::model::{
    Block, CapPer, Explain, Goal, Part, Participant, Request, Requirement, Span, Term,
};
use opportune::presets;
use opportune::reasons::ReasonCode;
use opportune::tags::{Selector, Tag};
use opportune::time::{
    Date, Duration, Instant, RuleZone, TimeOfDay, WallEnd, WallRange, Weekday, Weekdays, WeeklyKey,
    ZoneName,
};

fn tag(path: &str) -> Tag {
    Tag::new(path).expect("a valid tag path")
}

fn utc(date: (i16, i8, i8), clock: (u8, u8)) -> Instant {
    Instant::from_utc(
        Date::new(date.0, date.1, date.2).expect("a valid civil date"),
        TimeOfDay::new(clock.0, clock.1).expect("a valid time of day"),
    )
    .expect("UTC has no transitions")
}

fn wall(from: (u8, u8), until: (u8, u8)) -> WallRange {
    WallRange::new(
        TimeOfDay::new(from.0, from.1).expect("a valid time of day"),
        WallEnd::Time(TimeOfDay::new(until.0, until.1).expect("a valid time of day")),
    )
    .expect("a non-empty wall range")
}

fn vip() -> Selector {
    Selector::tag(tag("vip"))
}

/// An event on 14-18 June 2027, over `[from, until)` UTC, carrying `tags` and
/// identified by `key`.
fn booked(day: i8, from: (u8, u8), until: (u8, u8), tags: &[&str], key: &str) -> Block {
    let mut block = Block::new(
        Span::new(utc((2027, 6, day), from), utc((2027, 6, day), until))
            .expect("a non-empty span"),
    );
    block.tags = tags.iter().map(|path| tag(path)).collect();
    block.event_key = Some(key.to_owned());
    block
}

/// The one participant, with the week already in their calendar.
fn ana() -> Participant {
    Participant::new("ana", ZoneName::new("UTC").expect("UTC is a zone name")).with_calendar(vec![
        booked(14, (17, 30), (19, 30), &["meeting"], "mon_late"),
        booked(15, (9, 0), (10, 0), &["interview"], "tue_1"),
        booked(15, (10, 0), (11, 0), &["interview"], "tue_2"),
        booked(16, (9, 0), (10, 0), &["interview/frontend"], "wed_front"),
    ])
}

/// The 30-minute request for a booking tagged `tags`, offered only at
/// `candidates`, under all three tiers.
fn request(
    candidates: Vec<Instant>,
    tags: &[&str],
) -> Result<Request, Box<dyn std::error::Error>> {
    let mut part = Part::new(
        Duration::from_minutes(30),
        Requirement::new(vec![Term::person("ana")]),
    );
    part.tags = tags.iter().map(|path| tag(path)).collect();
    let goal = Goal::new(
        ZoneName::new("UTC")?,
        Span::new(utc((2027, 6, 14), (0, 0)), utc((2027, 6, 19), (0, 0)))?,
        vec![part],
    )
    .with_candidates(candidates);

    // The global tier: the ordinary week, a daily cap on interviews, and rest
    // after a late evening.
    let week = presets::weekly_hours(
        RuleZone::Event,
        vec![WeeklyKey::new(
            Weekdays::range(Weekday::Monday, Weekday::Friday),
            vec![wall((7, 0), (19, 0))],
        )?],
        Vec::new(),
    )?
    .with_id("week_hours")
    .unless(vip());
    let daily_limit = presets::cap_per_day(Selector::tag(tag("interview")), 2)
        .with_id("daily_limit")
        .unless(vip());
    let rest = presets::rest(TimeOfDay::new(18, 0)?, TimeOfDay::new(8, 0)?);

    // The event-group tier: interviews meet Monday to Thursday, 10:00 to 16:00,
    // and the group carries its own weekly count.
    let interview_hours = presets::business_hours(
        RuleZone::Event,
        Weekdays::range(Weekday::Monday, Weekday::Thursday),
        TimeOfDay::new(10, 0)?,
        WallEnd::Time(TimeOfDay::new(16, 0)?),
    )?
    .with_id("interview_hours")
    .with_when(Selector::tag(tag("interview")))
    .unless(vip());
    let interview_week = presets::cap_per_week(
        Selector::tag(tag("interview")),
        5,
        CapPer::Each,
        RuleZone::Participant,
    )
    .with_id("interview_week")
    .with_when(Selector::tag(tag("interview")));

    // The event-type tier: a frontend interview is the only kind that needs 48
    // hours' notice, and the only kind ana takes one of a day.
    let frontend = Selector::tag(tag("interview/frontend"));
    let frontend_day = presets::cap_per_day(Selector::tag(tag("interview/frontend")), 1)
        .with_id("frontend_day")
        .with_when(frontend.clone());
    let frontend_notice = presets::min_notice(Duration::from_hours_minutes(48, 0))
        .with_id("frontend_notice")
        .with_when(frontend);

    // The exception's own tier: the hours a vip booking is read in. Because the
    // two broader windows except `vip`, these are the only hours it gets.
    let vip_hours = presets::business_hours(
        RuleZone::Event,
        Weekdays::range(Weekday::Monday, Weekday::Friday),
        TimeOfDay::new(7, 0)?,
        WallEnd::Time(TimeOfDay::new(20, 0)?),
    )?
    .with_id("vip_hours")
    .with_when(vip());

    let mut request = Request::new(utc((2027, 6, 14), (6, 0)), goal);
    request.participants = vec![ana()];
    request.rules = vec![
        week,
        interview_hours,
        interview_week,
        daily_limit,
        frontend_day,
        frontend_notice,
        rest,
        vip_hours,
    ];
    request.explain = Some(Explain::new(100));
    Ok(request)
}

/// The code and rule of every reason `excluded` gives for `start`.
fn reasons_at(
    excluded: &[opportune::Excluded],
    start: Instant,
) -> Vec<(ReasonCode, Option<String>)> {
    excluded
        .iter()
        .filter(|entry| entry.start == start)
        .flat_map(|entry| &entry.reasons)
        .map(|reason| (reason.code, reason.rule.clone()))
        .collect()
}

/// The group's weekly count is the tier's own cap. Tightened to the three
/// interviews ana already has this week it closes the week, and the reason names
/// that cap rather than a window.
fn week_cap_closes_the_week(
    engine: &Engine,
    candidates: Vec<Instant>,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut tightened = request(candidates, &["interview"])?;
    tightened.rules.retain(|rule| rule.id != "interview_week");
    tightened.rules.push(
        presets::cap_per_week(
            Selector::tag(tag("interview")),
            3,
            CapPer::Each,
            RuleZone::Participant,
        )
        .with_id("interview_week")
        .with_when(Selector::tag(tag("interview"))),
    );
    let week_full = engine.find_availability(&tightened)?;
    assert!(week_full.offers.is_empty());
    assert_eq!(
        reasons_at(&week_full.excluded, utc((2027, 6, 14), (10, 30))),
        [(ReasonCode::CapReached, Some("interview_week".to_owned()))]
    );
    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let engine = Engine::builder().build()?;
    let candidates = vec![
        utc((2027, 6, 14), (10, 30)), // Monday, inside both windows
        utc((2027, 6, 14), (16, 30)), // Monday, after the group window closes
        utc((2027, 6, 15), (7, 30)),  // Tuesday, before the group window opens
        utc((2027, 6, 15), (13, 0)),  // Tuesday, at the global daily cap
        utc((2027, 6, 16), (13, 0)),  // Wednesday, at the frontend daily cap
        utc((2027, 6, 17), (11, 0)),  // Thursday, inside both windows
        utc((2027, 6, 18), (11, 0)),  // Friday, a day the group tier closes
    ];
    let starts = |offers: &[opportune::Offer]| -> Vec<Instant> {
        offers
            .iter()
            .filter_map(opportune::Offer::start)
            .collect()
    };

    // An ordinary interview sees the intersection of the global and group tiers
    // and is limited by the global daily cap, so the Friday start the global hours
    // allow is closed and Tuesday is full.
    let ordinary = engine.find_availability(&request(candidates.clone(), &["interview"])?)?;
    assert_eq!(
        starts(&ordinary.offers),
        [
            utc((2027, 6, 14), (10, 30)),
            utc((2027, 6, 16), (13, 0)),
            utc((2027, 6, 17), (11, 0)),
        ]
    );
    assert_eq!(
        reasons_at(&ordinary.excluded, utc((2027, 6, 14), (16, 30))),
        [(
            ReasonCode::OutsideWindow,
            Some("interview_hours".to_owned())
        )]
    );
    assert_eq!(
        reasons_at(&ordinary.excluded, utc((2027, 6, 15), (13, 0))),
        [(ReasonCode::CapReached, Some("daily_limit".to_owned()))]
    );
    assert_eq!(
        reasons_at(&ordinary.excluded, utc((2027, 6, 18), (11, 0))),
        [(
            ReasonCode::OutsideWindow,
            Some("interview_hours".to_owned())
        )]
    );

    // The group's weekly count is the tier's own cap.
    week_cap_closes_the_week(&engine, candidates.clone())?;

    // A vip booking is excepted from both windows and from the daily cap, so it
    // gets the wider vip hours — Friday included, which is the loosening that
    // matters: intersecting the two tiers would have left Friday closed.
    let vip_booking = engine.find_availability(&request(candidates, &["interview", "vip"])?)?;
    assert_eq!(
        starts(&vip_booking.offers),
        [
            utc((2027, 6, 14), (10, 30)),
            utc((2027, 6, 14), (16, 30)),
            utc((2027, 6, 15), (13, 0)),
            utc((2027, 6, 16), (13, 0)),
            utc((2027, 6, 17), (11, 0)),
            utc((2027, 6, 18), (11, 0)),
        ]
    );
    // The excepted tiers are only the windows and the daily cap: rest still
    // protects Tuesday's early morning after Monday's late meeting.
    assert_eq!(
        reasons_at(&vip_booking.excluded, utc((2027, 6, 15), (7, 30))),
        [(ReasonCode::Rest, Some("rest".to_owned()))]
    );

    // A frontend interview inherits every tier and adds its own: 48 hours' notice
    // and at most one a day.
    let frontend_candidates = vec![
        utc((2027, 6, 14), (10, 30)),
        utc((2027, 6, 16), (13, 0)),
        utc((2027, 6, 17), (11, 0)),
        utc((2027, 6, 18), (11, 0)),
    ];
    let frontend = engine.find_availability(&request(
        frontend_candidates,
        &["interview", "interview/frontend"],
    )?)?;
    assert_eq!(
        starts(&frontend.offers),
        [utc((2027, 6, 17), (11, 0))]
    );
    assert_eq!(
        reasons_at(&frontend.excluded, utc((2027, 6, 14), (10, 30))),
        [(
            ReasonCode::LeadTimeMin,
            Some("frontend_notice".to_owned())
        )]
    );
    assert_eq!(
        reasons_at(&frontend.excluded, utc((2027, 6, 16), (13, 0))),
        [(ReasonCode::CapReached, Some("frontend_day".to_owned()))]
    );
    Ok(())
}
```

### Python

<!-- snippet: layered-policy-python -->
```python
import opportune

vip = {"tag": "vip"}
frontend = {"tag": "interview/frontend"}

ana = opportune.Participant(
    id="ana",
    zone="UTC",
    calendar=[
        opportune.Block(
            span=("2027-06-14T17:30Z", "2027-06-14T19:30Z"),
            tags=["meeting"],
            event_key="mon_late",
        ),
        opportune.Block(
            span=("2027-06-15T09:00Z", "2027-06-15T10:00Z"),
            tags=["interview"],
            event_key="tue_1",
        ),
        opportune.Block(
            span=("2027-06-15T10:00Z", "2027-06-15T11:00Z"),
            tags=["interview"],
            event_key="tue_2",
        ),
        opportune.Block(
            span=("2027-06-16T09:00Z", "2027-06-16T10:00Z"),
            tags=["interview/frontend"],
            event_key="wed_front",
        ),
    ],
)

# The global tier, the event-group tier and the event-type tier, with `vip`
# excepted from the two broad windows and from the daily cap.
rules = [
    opportune.unless(
        opportune.presets.weekly_hours(
            {"mon-fri": "07:00-19:00"}, zone="event", rule_id="week_hours"
        ),
        vip,
    ),
    opportune.unless(
        opportune.presets.business_hours(
            "mon-thu", "10:00-16:00", zone="event", rule_id="interview_hours",
            when={"tag": "interview"},
        ),
        vip,
    ),
    opportune.presets.cap_per_week(
        {"tag": "interview"}, 5, rule_id="interview_week", when={"tag": "interview"}
    ),
    opportune.unless(
        opportune.presets.cap_per_day({"tag": "interview"}, 2, rule_id="daily_limit"),
        vip,
    ),
    opportune.presets.cap_per_day(frontend, 1, rule_id="frontend_day", when=frontend),
    opportune.presets.min_notice("48h", rule_id="frontend_notice", when=frontend),
    opportune.presets.rest("18:00", "08:00"),
    opportune.presets.business_hours(
        "mon-fri", "07:00-20:00", zone="event", rule_id="vip_hours", when=vip
    ),
]


def request(tags: list[str], candidates: list[str]) -> opportune.Request:
    return opportune.Request(
        now="2027-06-14T06:00Z",
        goal=opportune.single_goal(
            "30m",
            opportune.Requirement(all=[opportune.person("ana")]),
            zone="UTC",
            search_window=("2027-06-14T00:00Z", "2027-06-19T00:00Z"),
            candidates=candidates,
            tags=tags,
        ),
        participants=[ana],
        rules=rules,
        explain=opportune.Explain(limit=100),
    )


def starts(response: opportune.Response) -> list[str]:
    return [opportune.offer_start(offer) for offer in response.offers]


def reasons(response: opportune.Response, start: str) -> list[tuple[str, str | None]]:
    entry = next(item for item in response.excluded or [] if item.start == start)
    return [(reason.code, reason.rule) for reason in entry.reasons or []]


candidates = [
    "2027-06-14T10:30Z",
    "2027-06-14T16:30Z",
    "2027-06-15T07:30Z",
    "2027-06-15T13:00Z",
    "2027-06-16T13:00Z",
    "2027-06-17T11:00Z",
    "2027-06-18T11:00Z",
]

# An ordinary interview: the intersection of both windows, under the daily cap.
ordinary = opportune.find_availability(request(["interview"], candidates))
assert starts(ordinary) == [
    "2027-06-14T10:30Z",
    "2027-06-16T13:00Z",
    "2027-06-17T11:00Z",
]
assert reasons(ordinary, "2027-06-14T16:30Z") == [("outside_window", "interview_hours")]
assert reasons(ordinary, "2027-06-15T13:00Z") == [("cap_reached", "daily_limit")]
assert reasons(ordinary, "2027-06-18T11:00Z") == [("outside_window", "interview_hours")]

# The group's weekly count is the tier's own cap. Tightened to the three
# interviews ana already has this week it closes the week.
tightened = opportune.with_updates(
    request(["interview"], candidates),
    rules=[
        *[rule for rule in rules if rule.id != "interview_week"],
        opportune.presets.cap_per_week(
            {"tag": "interview"}, 3, rule_id="interview_week", when={"tag": "interview"}
        ),
    ],
)
week_full = opportune.find_availability(tightened)
assert week_full.offers == []
assert reasons(week_full, "2027-06-14T10:30Z") == [("cap_reached", "interview_week")]

# The vip booking escapes both windows and the daily cap; rest still applies.
vip_booking = opportune.find_availability(request(["interview", "vip"], candidates))
assert starts(vip_booking) == [
    "2027-06-14T10:30Z",
    "2027-06-14T16:30Z",
    "2027-06-15T13:00Z",
    "2027-06-16T13:00Z",
    "2027-06-17T11:00Z",
    "2027-06-18T11:00Z",
]
assert reasons(vip_booking, "2027-06-15T07:30Z") == [("rest", "rest")]

# The frontend tier adds 48 hours' notice and a cap of one a day.
frontend_booking = opportune.find_availability(
    request(["interview", "interview/frontend"], candidates[:6])
)
assert starts(frontend_booking) == ["2027-06-17T11:00Z"]
assert reasons(frontend_booking, "2027-06-14T10:30Z") == [
    ("lead_time_min", "frontend_notice")
]
assert reasons(frontend_booking, "2027-06-16T13:00Z") == [
    ("cap_reached", "frontend_day")
]
```

### TypeScript

<!-- snippet: layered-policy-ts -->
```ts
import {
  Engine, businessHours, capPerDay, capPerWeek, minNotice, offerStart, person, rest,
  singleGoal, unless, weeklyHours,
} from "opportune";
import type { Participant, Request, Response } from "opportune";

const vip = { tag: "vip" };
const frontend = { tag: "interview/frontend" };

const ana: Participant = {
  id: "ana",
  zone: "UTC",
  calendar: [
    { span: ["2027-06-14T17:30Z", "2027-06-14T19:30Z"], tags: ["meeting"], event_key: "mon_late" },
    { span: ["2027-06-15T09:00Z", "2027-06-15T10:00Z"], tags: ["interview"], event_key: "tue_1" },
    { span: ["2027-06-15T10:00Z", "2027-06-15T11:00Z"], tags: ["interview"], event_key: "tue_2" },
    {
      span: ["2027-06-16T09:00Z", "2027-06-16T10:00Z"],
      tags: ["interview/frontend"],
      event_key: "wed_front",
    },
  ],
};

// The global tier, the event-group tier and the event-type tier, with `vip`
// excepted from the two broad windows and from the daily cap.
const rules = [
  unless(weeklyHours({ "mon-fri": "07:00-19:00" }, { zone: "event", ruleId: "week_hours" }), vip),
  unless(
    businessHours("mon-thu", "10:00-16:00", {
      zone: "event", ruleId: "interview_hours", when: { tag: "interview" },
    }),
    vip,
  ),
  capPerWeek({ tag: "interview" }, 5, { ruleId: "interview_week", when: { tag: "interview" } }),
  unless(capPerDay({ tag: "interview" }, 2, { ruleId: "daily_limit" }), vip),
  capPerDay(frontend, 1, { ruleId: "frontend_day", when: frontend }),
  minNotice("48h", { ruleId: "frontend_notice", when: frontend }),
  rest("18:00", "08:00"),
  businessHours("mon-fri", "07:00-20:00", {
    zone: "event", ruleId: "vip_hours", when: vip,
  }),
];

const request = (tags: string[], candidates: string[]): Request => ({
  now: "2027-06-14T06:00Z",
  goal: singleGoal("30m", { all: [person("ana")] }, {
    zone: "UTC",
    searchWindow: ["2027-06-14T00:00Z", "2027-06-19T00:00Z"],
    candidates,
    tags,
  }),
  participants: [ana],
  rules,
  explain: { limit: 100 },
});

const starts = (response: Response): string[] => response.offers.map((offer) => offerStart(offer));
const reasons = (response: Response, start: string): string[] =>
  (response.excluded.find((entry) => entry.start === start)?.reasons ?? []).map(
    (reason) => `${reason.code}:${reason.rule ?? ""}`,
  );

const candidates = [
  "2027-06-14T10:30Z",
  "2027-06-14T16:30Z",
  "2027-06-15T07:30Z",
  "2027-06-15T13:00Z",
  "2027-06-16T13:00Z",
  "2027-06-17T11:00Z",
  "2027-06-18T11:00Z",
];

using engine = new Engine();

// An ordinary interview: the intersection of both windows, under the daily cap.
const ordinary = engine.findAvailability(request(["interview"], candidates));
const expected = ["2027-06-14T10:30Z", "2027-06-16T13:00Z", "2027-06-17T11:00Z"];
if (starts(ordinary).join() !== expected.join()) {
  throw new Error(`the ordinary interview offered ${starts(ordinary).join()}`);
}
if (reasons(ordinary, "2027-06-14T16:30Z").join() !== "outside_window:interview_hours") {
  throw new Error("the group window did not close Monday afternoon");
}
if (reasons(ordinary, "2027-06-15T13:00Z").join() !== "cap_reached:daily_limit") {
  throw new Error("the daily cap did not close Tuesday");
}

// The group's weekly count is the tier's own cap. Tightened to the three
// interviews ana already has this week it closes the week.
const weekFull = engine.findAvailability({
  ...request(["interview"], candidates),
  rules: [
    ...rules.filter((rule) => rule.id !== "interview_week"),
    capPerWeek({ tag: "interview" }, 3, {
      ruleId: "interview_week",
      when: { tag: "interview" },
    }),
  ],
});
if (weekFull.offers.length !== 0) {
  throw new Error(`the closed week still offered ${starts(weekFull).join()}`);
}
if (reasons(weekFull, "2027-06-14T10:30Z").join() !== "cap_reached:interview_week") {
  throw new Error("the group's weekly cap did not close the week");
}

// The vip booking escapes both windows and the daily cap; rest still applies.
const vipBooking = engine.findAvailability(request(["interview", "vip"], candidates));
if (starts(vipBooking).length !== 6) {
  throw new Error(`the vip booking offered ${starts(vipBooking).join()}`);
}
if (reasons(vipBooking, "2027-06-15T07:30Z").join() !== "rest:rest") {
  throw new Error("rest did not protect Tuesday's early morning");
}

// The frontend tier adds 48 hours' notice and a cap of one a day.
const frontendBooking = engine.findAvailability(
  request(["interview", "interview/frontend"], candidates.slice(0, 6)),
);
if (starts(frontendBooking).join() !== "2027-06-17T11:00Z") {
  throw new Error(`the frontend interview offered ${starts(frontendBooking).join()}`);
}
if (reasons(frontendBooking, "2027-06-14T10:30Z").join() !== "lead_time_min:frontend_notice") {
  throw new Error("the frontend tier did not require two days' notice");
}
if (reasons(frontendBooking, "2027-06-16T13:00Z").join() !== "cap_reached:frontend_day") {
  throw new Error("the frontend tier did not cap the day");
}
console.log(starts(frontendBooking), starts(vipBooking).length);
```

## An internal team and a conference room

A meeting that needs two people and a conference room. The room is a
`ParticipantKind::Resource` participant, so every rule that is a statement about
humans — hours, rest, the daily cap, and the policy that ignores tentative holds —
is scoped with `Selector::kind(ParticipantKind::Person)` and never reaches it. The
room's own tentative hold therefore keeps blocking, which is the double-booking
the unscoped default `who` would have hidden. Room choice is tiering rather than a
flag: the first request asks for both rooms, and the second asks for one when the
first has no answer.

### Rust

```rust
// An internal team plus a conference room. The room is a `ParticipantKind::Resource`
// participant, so the human-only rules — hours, rest and the daily cap — are scoped
// with `Selector::kind(ParticipantKind::Person)` and never reach it, while its own
// tentative hold keeps blocking. The second request is the same booking with one
// room instead of two: optional-room tiering is a second request, not a flag.

use opportune::Engine;
use opportune::model::{
    Block, BlockStatus, Explain, Goal, Part, Participant, ParticipantKind, Request, Requirement,
    Rule, Span, StatusEffect, StatusPolicyEntry, Term,
};
use opportune::presets;
use opportune::reasons::ReasonCode;
use opportune::tags::{Selector, Tag};
use opportune::time::{
    Date, Duration, Instant, RuleZone, TimeOfDay, WallEnd, Weekday, Weekdays, ZoneName,
};

fn tag(path: &str) -> Tag {
    Tag::new(path).expect("a valid tag path")
}

fn utc(date: (i16, i8, i8), clock: (u8, u8)) -> Instant {
    Instant::from_utc(
        Date::new(date.0, date.1, date.2).expect("a valid civil date"),
        TimeOfDay::new(clock.0, clock.1).expect("a valid time of day"),
    )
    .expect("UTC has no transitions")
}

/// A person participant, tagged, with the given calendar.
fn person(id: &str, tags: &[&str], blocks: Vec<Block>) -> Participant {
    let mut participant =
        Participant::new(id, ZoneName::new("UTC").expect("UTC is a zone name")).with_calendar(blocks);
    participant.tags = tags.iter().map(|path| tag(path)).collect();
    participant
}

fn room(id: &str, blocks: Vec<Block>) -> Participant {
    person(id, &["kind/room"], blocks).with_kind(ParticipantKind::Resource)
}

/// The block from `[from, until)` UTC on Monday 14 June 2027, with `status`.
fn held(from: (u8, u8), until: (u8, u8), status: BlockStatus) -> Block {
    let mut block = Block::new(
        Span::new(utc((2027, 6, 14), from), utc((2027, 6, 14), until)).expect("a non-empty span"),
    );
    block.status = status;
    block
}

fn people() -> Selector {
    Selector::kind(ParticipantKind::Person)
}

/// The rules an internal meeting runs under: the team's hours, rest and daily cap
/// are statements about people, and the room's own holds are not.
fn rules(scope_status_policy: bool) -> Result<Vec<Rule>, Box<dyn std::error::Error>> {
    let status_policy =
        presets::status_policy(vec![StatusPolicyEntry::status(
            BlockStatus::Tentative,
            StatusEffect::Ignore,
        )])
        .with_id("ignore_tentative");
    Ok(vec![
        presets::business_hours(
            RuleZone::Participant,
            Weekdays::range(Weekday::Monday, Weekday::Friday),
            TimeOfDay::new(9, 0)?,
            WallEnd::Time(TimeOfDay::new(17, 0)?),
        )?
        .with_who(people()),
        presets::rest(TimeOfDay::new(18, 0)?, TimeOfDay::new(8, 0)?).with_who(people()),
        presets::cap_per_day(Selector::tag(tag("meeting")), 3).with_who(people()),
        if scope_status_policy {
            status_policy.with_who(people())
        } else {
            status_policy
        },
    ])
}

/// The 60-minute team meeting at `candidates` that needs `rooms` conference rooms
/// as well as ana and bo.
fn request(
    candidates: Vec<Instant>,
    rooms: u32,
    scope_status_policy: bool,
) -> Result<Request, Box<dyn std::error::Error>> {
    let requirement = Requirement::new(vec![
        Term::person("ana"),
        Term::person("bo"),
        Term::at_least(rooms, Selector::tag(tag("kind/room"))),
    ]);
    let mut part = Part::new(Duration::from_minutes(60), requirement);
    part.tags = vec![tag("meeting")];
    let goal = Goal::new(
        ZoneName::new("UTC")?,
        Span::new(utc((2027, 6, 14), (0, 0)), utc((2027, 6, 15), (0, 0)))?,
        vec![part],
    )
    .with_candidates(candidates);

    let mut request = Request::new(utc((2027, 6, 14), (8, 0)), goal);
    request.participants = vec![
        person("ana", &["team"], vec![]),
        person("bo", &["team"], vec![]),
        // Held tentatively from 10:00 to 11:00: taken, whatever a person's own
        // tentative invite means.
        room("room-4", vec![held((10, 0), (11, 0), BlockStatus::Tentative)]),
        // Booked solid all day, so the two-room tier has nothing to offer.
        room("room-9", vec![held((8, 0), (18, 0), BlockStatus::Busy)]),
    ];
    request.rules = rules(scope_status_policy)?;
    request.explain = Some(Explain::new(100));
    Ok(request)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let engine = Engine::builder().build()?;
    let candidates = vec![
        utc((2027, 6, 14), (9, 0)),
        utc((2027, 6, 14), (10, 0)),
        utc((2027, 6, 14), (11, 0)),
    ];
    let starts = |offers: &[opportune::Offer]| -> Vec<Instant> {
        offers
            .iter()
            .filter_map(opportune::Offer::start)
            .collect()
    };

    // Tier one: both rooms. room-9 is booked all day, so the request has no
    // answer at all — the trigger a caller uses to fall back to one room.
    let both = engine.find_availability(&request(candidates.clone(), 2, true)?)?;
    assert!(both.offers.is_empty());

    // Tier two: one room. The tentative hold at 10:00 still blocks, because the
    // policy that ignores tentative holds is scoped to people.
    let one = engine.find_availability(&request(candidates.clone(), 1, true)?)?;
    assert_eq!(
        starts(&one.offers),
        [utc((2027, 6, 14), (9, 0)), utc((2027, 6, 14), (11, 0))]
    );
    let ten = one
        .excluded
        .iter()
        .find(|excluded| excluded.start == utc((2027, 6, 14), (10, 0)))
        .ok_or("the held hour is excluded")?;
    let blocked = ten
        .reasons
        .iter()
        .find(|reason| reason.code == ReasonCode::Blocked)
        .ok_or("the room's hold is the reason")?;
    assert_eq!(blocked.participant.as_deref(), Some("room-4"));

    // The scoping is what protects the room. Unscoped, the same policy reads the
    // room's hold as free and offers the hour a second meeting would double-book,
    // and `validate` warns about exactly that.
    let unscoped_request = request(candidates.clone(), 1, false)?;
    let unscoped = engine.find_availability(&unscoped_request)?;
    assert_eq!(
        starts(&unscoped.offers),
        [
            utc((2027, 6, 14), (9, 0)),
            utc((2027, 6, 14), (10, 0)),
            utc((2027, 6, 14), (11, 0)),
        ]
    );
    let warning = engine
        .validate(&unscoped_request)
        .warnings
        .into_iter()
        .find(|warning| warning.code() == "status_policy_ignores_resource")
        .ok_or("the unscoped policy is reported")?;
    assert_eq!(warning.path(), Some("/rules/3"));

    // Both remaining offers fill the room term with room-4: room-9 is booked all
    // day, so the room the requirement lands on is the one whose hold closed 10:00.
    let assigned = one
        .offers
        .iter()
        .flat_map(opportune::Offer::assigned_hosts)
        .collect::<Vec<_>>();
    assert_eq!(assigned, ["ana", "bo", "room-4", "ana", "bo", "room-4"]);
    Ok(())
}
```

### Python

<!-- snippet: team-room-python -->
```python
import opportune

people = opportune.KindSelector(kind="person")


def participant(
    id_: str, tags: list[str], calendar: list[opportune.Block]
) -> opportune.Participant:
    return opportune.Participant(id=id_, zone="UTC", tags=tags, calendar=calendar)


def room(id_: str, calendar: list[opportune.Block]) -> opportune.Participant:
    return opportune.Participant(
        id=id_, kind="resource", zone="UTC", tags=["kind/room"], calendar=calendar
    )


def held(start: str, end: str, status: str) -> opportune.Block:
    return opportune.Block(span=(start, end), status=status)


def rules(scoped: bool) -> list[opportune.Rule]:
    # One policy, two scopes: every host by default, or people only.
    policy = opportune.StatusPolicyRule(
        id="ignore_tentative",
        kind="status_policy",
        params=opportune.StatusPolicyParams(
            entries=[opportune.StatusPolicyEntry(status="tentative", effect="ignore")]
        ),
        who=people if scoped else None,
    )
    return [
        opportune.presets.business_hours("mon-fri", "09:00-17:00", who=people),
        opportune.presets.rest("18:00", "08:00", who=people),
        opportune.presets.cap_per_day({"tag": "meeting"}, 3, who=people),
        policy,
    ]


def request(rooms: int, scoped: bool) -> opportune.Request:
    return opportune.Request(
        now="2027-06-14T08:00Z",
        goal=opportune.single_goal(
            "60m",
            opportune.Requirement(
                all=[
                    opportune.person("ana"),
                    opportune.person("bo"),
                    opportune.at_least(rooms, {"tag": "kind/room"}),
                ]
            ),
            zone="UTC",
            search_window=("2027-06-14T00:00Z", "2027-06-15T00:00Z"),
            candidates=["2027-06-14T09:00Z", "2027-06-14T10:00Z", "2027-06-14T11:00Z"],
            part_id="meeting",
            tags=["meeting"],
        ),
        participants=[
            participant("ana", ["team"], []),
            participant("bo", ["team"], []),
            room("room-4", [held("2027-06-14T10:00Z", "2027-06-14T11:00Z", "tentative")]),
            room("room-9", [held("2027-06-14T08:00Z", "2027-06-14T18:00Z", "busy")]),
        ],
        rules=rules(scoped),
        explain=opportune.Explain(limit=100),
    )


def starts(response: opportune.Response) -> list[str]:
    return [opportune.offer_start(offer) for offer in response.offers]


# Both rooms, then one: the second request is what a caller falls back to.
assert starts(opportune.find_availability(request(2, True))) == []
one = opportune.find_availability(request(1, True))
assert starts(one) == ["2027-06-14T09:00Z", "2027-06-14T11:00Z"]
assert opportune.offer_assigned_hosts(one.offers[0]) == ["ana", "bo", "room-4"]

ten = next(entry for entry in one.excluded or [] if entry.start == "2027-06-14T10:00Z")
blocked = next(reason for reason in ten.reasons or [] if reason.code == "blocked")
assert blocked.participant == "room-4"

# Unscoped, the same policy drops the room's hold and the validator names the rule.
unscoped = request(1, False)
assert len(opportune.find_availability(unscoped).offers) == 3
warning = next(
    finding
    for finding in opportune.validate(unscoped).warnings
    if finding.code == "status_policy_ignores_resource"
)
assert warning.path == "/rules/3"
```

In a FastAPI route the same request is what the caller's JSON body validates into,
`public_response` strips the operator fields before the room list leaves the server,
and `find_availability_async` keeps the event loop free while the search runs — see
[the FastAPI entry points](api.md#fastapi-a-lifespan-engine-and-the-async-entry-points).

### TypeScript

<!-- snippet: team-room-ts -->
```ts
import {
  Engine, atLeast, businessHours, capPerDay, offerAssignedHosts, offerStart, person, rest,
  singleGoal,
} from "opportune";
import type { Block, Offer, Participant, Request, Rule, StatusPolicyRule } from "opportune";

const people = { kind: "person" } as const;

const participant = (id: string, tags: string[], calendar: Block[]): Participant => ({
  id, zone: "UTC", tags, calendar,
});

const room = (id: string, calendar: Block[]): Participant => ({
  id, kind: "resource", zone: "UTC", tags: ["kind/room"], calendar,
});

const held = (start: string, end: string, status: "tentative" | "busy"): Block => ({
  span: [start, end], status,
});

const rules = (scoped: boolean): Rule[] => {
  // One policy, two scopes: every host by default, or people only.
  const policy: StatusPolicyRule = {
    id: "ignore_tentative",
    kind: "status_policy",
    params: { entries: [{ status: "tentative", effect: "ignore" }] },
    ...(scoped ? { who: people } : {}),
  };
  return [
    businessHours("mon-fri", "09:00-17:00", { who: people }),
    rest("18:00", "08:00", { who: people }),
    capPerDay({ tag: "meeting" }, 3, { who: people }),
    policy,
  ];
};

const request = (rooms: number, scoped: boolean): Request => ({
  now: "2027-06-14T08:00Z",
  goal: singleGoal(
    "60m",
    {
      all: [
        person("ana"),
        person("bo"),
        atLeast(rooms, { tag: "kind/room" }),
      ],
    },
    {
      zone: "UTC",
      searchWindow: ["2027-06-14T00:00Z", "2027-06-15T00:00Z"],
      candidates: ["2027-06-14T09:00Z", "2027-06-14T10:00Z", "2027-06-14T11:00Z"],
      partId: "meeting",
      tags: ["meeting"],
    },
  ),
  participants: [
    participant("ana", ["team"], []),
    participant("bo", ["team"], []),
    room("room-4", [held("2027-06-14T10:00Z", "2027-06-14T11:00Z", "tentative")]),
    room("room-9", [held("2027-06-14T08:00Z", "2027-06-14T18:00Z", "busy")]),
  ],
  rules: rules(scoped),
  explain: { limit: 100 },
});

using engine = new Engine();
const starts = (offers: Offer[]): string[] => offers.map((offer) => offerStart(offer));

// Both rooms, then one: the second request is what a caller falls back to.
if (starts(engine.findAvailability(request(2, true)).offers).length !== 0) {
  throw new Error("the two-room request was expected to have no answer");
}
const one = engine.findAvailability(request(1, true));
const startsOne = starts(one.offers);
if (startsOne.join() !== "2027-06-14T09:00Z,2027-06-14T11:00Z") {
  throw new Error(`the one-room request offered ${startsOne.join()}`);
}
const assigned = one.offers.flatMap((offer) => offerAssignedHosts(offer));
if (assigned.join() !== "ana,bo,room-4,ana,bo,room-4") {
  throw new Error(`the room was not assigned: ${assigned.join()}`);
}
const ten = one.excluded.find((entry) => entry.start === "2027-06-14T10:00Z");
const blocked = ten?.reasons.find((reason) => reason.code === "blocked");
if (blocked?.participant !== "room-4") {
  throw new Error("the room's tentative hold did not block 10:00");
}

// Unscoped, the same policy drops the room's hold and the validator names the rule.
const unscoped = request(1, false);
if (engine.findAvailability(unscoped).offers.length !== 3) {
  throw new Error("the unscoped policy did not offer the held hour");
}
const warning = engine
  .validate(unscoped)
  .warnings.find((finding) => finding.code === "status_policy_ignores_resource");
if (warning?.path !== "/rules/3") {
  throw new Error("the unscoped policy was not reported");
}
console.log(startsOne, warning.path);
```

## A weighted panel and rescheduling at capacity

A panel interview: a named hiring manager plus any two of three engineers tagged
`eng/frontend`, weighted so the assignment prefers who the dispatcher wants, for a
candidate whose `available_only` hours are the only time they can be booked. The
interview is then moved to another slot with `Request::exclude_event_keys`. Naming
the event being moved takes that event out of the daily cap it was consuming, so the
interviewer who was already at their limit can keep the panel — the proposal that
keeps them is refused without the exclusion and confirms with it.

### Rust

```rust
// A true panel plus a reschedule: a hiring manager and two of three engineers,
// weighted so the assignment prefers who the dispatcher wants, for a candidate
// whose stated availability is the only time they can be booked. The interview is
// then moved with `Request::exclude_event_keys`, which takes the booking being
// moved out of the cap it was consuming — so the interviewer who is already at
// their daily cap can keep the panel.

use std::collections::BTreeMap;

use opportune::Engine;
use opportune::model::{
    Block, Explain, Goal, Part, Participant, Proposal, ProposalEvent, Request, Requirement, Span,
    Term,
};
use opportune::presets;
use opportune::reasons::ReasonCode;
use opportune::tags::{Selector, Tag};
use opportune::time::{Date, Duration, Instant, RuleZone, TimeOfDay, WallEnd, Weekday, Weekdays, ZoneName};

fn tag(path: &str) -> Tag {
    Tag::new(path).expect("a valid tag path")
}

fn utc(date: (i16, i8, i8), clock: (u8, u8)) -> Instant {
    Instant::from_utc(
        Date::new(date.0, date.1, date.2).expect("a valid civil date"),
        TimeOfDay::new(clock.0, clock.1).expect("a valid time of day"),
    )
    .expect("UTC has no transitions")
}

/// An interview already on the calendar, over `[from, until)` UTC on Tuesday
/// 15 June 2027, identified by `key`.
fn booked(from: (u8, u8), until: (u8, u8), key: &str) -> Block {
    let mut block = Block::new(
        Span::new(utc((2027, 6, 15), from), utc((2027, 6, 15), until)).expect("a non-empty span"),
    );
    block.tags = vec![tag("interview")];
    block.event_key = Some(key.to_owned());
    block
}

fn host(id: &str, tags: &[&str], blocks: Vec<Block>) -> Participant {
    let mut participant = Participant::new(id, ZoneName::new("UTC").expect("UTC is a zone name"))
        .with_calendar(blocks);
    participant.tags = tags.iter().map(|path| tag(path)).collect();
    participant
}

/// The one-hour panel interview at `candidates`, with `exclude` naming the event
/// being moved — empty for a fresh booking.
fn request(
    candidates: Vec<Instant>,
    exclude: &[&str],
) -> Result<Request, Box<dyn std::error::Error>> {
    let requirement = Requirement::new(vec![
        Term::person("dana"),
        Term::at_least(2, Selector::tag(tag("eng/frontend"))),
    ]);
    let part = Part {
        id: "panel".to_owned(),
        tags: vec![tag("interview")],
        ..Part::new(Duration::from_minutes(60), requirement)
    };
    let goal = Goal::new(
        ZoneName::new("UTC")?,
        Span::new(utc((2027, 6, 15), (8, 0)), utc((2027, 6, 15), (18, 0)))?,
        vec![part],
    )
    .with_candidates(candidates);

    // The candidate states the only hours they can attend; `available_only` hands
    // back the attendee and the window that holds them to it.
    let (candidate, availability) = presets::available_only(
        "candidate",
        &[Span::new(
            utc((2027, 6, 15), (11, 0)),
            utc((2027, 6, 15), (16, 0)),
        )?],
    )?;

    let mut request = Request::new(utc((2027, 6, 15), (8, 0)), goal);
    request.participants = vec![
        host("dana", &["hiring_manager"], vec![]),
        host("ana", &["eng/frontend"], vec![]),
        host("bo", &["eng/frontend"], vec![]),
        // Ana and cy are the interviewers the dispatcher wants on this panel.
        host(
            "cy",
            &["eng/frontend"],
            vec![booked((9, 0), (10, 0), "evt_prev"), booked((13, 0), (14, 0), "evt_move")],
        ),
        candidate,
    ];
    request.weights = BTreeMap::from([
        ("ana".to_owned(), 3),
        ("bo".to_owned(), 1),
        ("cy".to_owned(), 2),
    ]);
    request.rules = vec![
        presets::business_hours(
            RuleZone::Participant,
            Weekdays::range(Weekday::Monday, Weekday::Friday),
            TimeOfDay::new(9, 0)?,
            WallEnd::Time(TimeOfDay::new(17, 0)?),
        )?,
        presets::cap_per_day(Selector::tag(tag("interview")), 2).with_id("daily_limit"),
        availability,
    ];
    request.explain = Some(Explain::new(100));
    request.exclude_event_keys = exclude.iter().map(|key| (*key).to_owned()).collect();
    Ok(request)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let engine = Engine::builder().build()?;
    let candidates = vec![utc((2027, 6, 15), (10, 0)), utc((2027, 6, 15), (11, 0))];
    let starts = |offers: &[opportune::Offer]| -> Vec<Instant> {
        offers
            .iter()
            .filter_map(opportune::Offer::start)
            .collect()
    };

    // Before the reschedule: cy is at their daily cap of two, counting the very
    // interview being moved, so the panel is filled by the next two engineers and
    // the candidate's unavailable hour is excluded.
    let at_capacity = request(candidates.clone(), &[])?;
    let before = engine.find_availability(&at_capacity)?;
    assert_eq!(starts(&before.offers), [utc((2027, 6, 15), (11, 0))]);
    assert_eq!(before.offers[0].events[0].eligible[0], ["dana"]);
    assert_eq!(before.offers[0].events[0].eligible[1], ["ana", "bo"]);
    assert_eq!(before.offers[0].assigned_hosts(), ["dana", "ana", "bo"]);
    let unavailable = before
        .excluded
        .iter()
        .find(|excluded| excluded.start == utc((2027, 6, 15), (10, 0)))
        .ok_or("the candidate's unavailable hour is excluded")?;
    assert_eq!(
        (
            unavailable.reasons[0].code,
            unavailable.reasons[0].rule.as_deref()
        ),
        (
            ReasonCode::OutsideWindow,
            Some("available_only:candidate")
        )
    );

    // A proposal that keeps cy — the interviewer who already knows the candidate —
    // is refused while the moved interview still counts.
    let keep_cy = Proposal {
        events: vec![ProposalEvent {
            part: "panel".to_owned(),
            start: utc((2027, 6, 15), (11, 0)),
            assignment: vec![
                vec!["dana".to_owned()],
                vec!["ana".to_owned(), "cy".to_owned()],
            ],
        }],
    };
    let refused = engine.check_slot(&at_capacity, &keep_cy)?;
    assert!(!refused.valid);
    let capped = refused
        .reasons
        .iter()
        .find(|reason| reason.code == ReasonCode::CapReached)
        .ok_or("the cap is the reason")?;
    assert_eq!(capped.participant.as_deref(), Some("cy"));
    assert_eq!(capped.rule.as_deref(), Some("daily_limit"));

    // Naming the event the caller is moving takes it out of the cap. cy is under
    // the limit again, so the weights choose ana and cy, and the moved slot
    // confirms.
    let moving = request(candidates, &["evt_move"])?;
    let after = engine.find_availability(&moving)?;
    assert_eq!(starts(&after.offers), [utc((2027, 6, 15), (11, 0))]);
    assert_eq!(after.offers[0].events[0].eligible[1], ["ana", "bo", "cy"]);
    assert_eq!(after.offers[0].assigned_hosts(), ["dana", "ana", "cy"]);
    let confirmed = engine.check_slot(&moving, &after.offers[0].to_proposal())?;
    assert!(confirmed.valid);
    assert!(confirmed.reasons.is_empty());
    Ok(())
}
```

### Python

<!-- snippet: panel-reschedule-python -->
```python
import opportune

candidate, availability = opportune.presets.available_only(
    "candidate", [("2027-06-15T11:00Z", "2027-06-15T16:00Z")]
)


def booked(start: str, end: str, key: str) -> opportune.Block:
    return opportune.Block(span=(start, end), tags=["interview"], event_key=key)


def host(id_: str, tags: list[str], calendar: list[opportune.Block]) -> opportune.Participant:
    return opportune.Participant(id=id_, zone="UTC", tags=tags, calendar=calendar)


def request(exclude: list[str]) -> opportune.Request:
    return opportune.Request(
        now="2027-06-15T08:00Z",
        goal=opportune.single_goal(
            "60m",
            opportune.Requirement(
                all=[opportune.person("dana"), opportune.at_least(2, {"tag": "eng/frontend"})]
            ),
            zone="UTC",
            search_window=("2027-06-15T08:00Z", "2027-06-15T18:00Z"),
            candidates=["2027-06-15T10:00Z", "2027-06-15T11:00Z"],
            part_id="panel",
            tags=["interview"],
        ),
        participants=[
            host("dana", ["hiring_manager"], []),
            host("ana", ["eng/frontend"], []),
            host("bo", ["eng/frontend"], []),
            host(
                "cy",
                ["eng/frontend"],
                [
                    booked("2027-06-15T09:00Z", "2027-06-15T10:00Z", "evt_prev"),
                    booked("2027-06-15T13:00Z", "2027-06-15T14:00Z", "evt_move"),
                ],
            ),
            candidate,
        ],
        weights={"ana": 3, "bo": 1, "cy": 2},
        rules=[
            opportune.presets.business_hours("mon-fri", "09:00-17:00"),
            opportune.presets.cap_per_day({"tag": "interview"}, 2, rule_id="daily_limit"),
            availability,
        ],
        explain=opportune.Explain(limit=100),
        exclude_event_keys=exclude,
    )


def starts(response: opportune.Response) -> list[str]:
    return [opportune.offer_start(offer) for offer in response.offers]


before = opportune.find_availability(request([]))
assert starts(before) == ["2027-06-15T11:00Z"]
assert before.offers[0].events[0].eligible[1] == ["ana", "bo"]
assert opportune.offer_assigned_hosts(before.offers[0]) == ["dana", "ana", "bo"]

# The candidate's unavailable hour, which is the only reason 10:00 is out.
unavailable = next(
    entry for entry in before.excluded or [] if entry.start == "2027-06-15T10:00Z"
)
assert (unavailable.reasons[0].code, unavailable.reasons[0].rule) == (
    "outside_window",
    "available_only:candidate",
)

# Keeping cy is refused while the interview being moved still counts.
keep_cy = opportune.Proposal(
    events=[
        opportune.ProposalEvent(
            part="panel",
            start="2027-06-15T11:00Z",
            assignment=[["dana"], ["ana", "cy"]],
        )
    ]
)
refused = opportune.check_slot(request([]), keep_cy)
assert not refused.valid
capped = next(reason for reason in refused.reasons or [] if reason.code == "cap_reached")
assert (capped.participant, capped.rule) == ("cy", "daily_limit")

# Naming the event being moved frees the cap, the weights choose ana and cy, and
# the moved slot confirms.
after = opportune.find_availability(request(["evt_move"]))
assert starts(after) == ["2027-06-15T11:00Z"]
assert after.offers[0].events[0].eligible[1] == ["ana", "bo", "cy"]
assert opportune.offer_assigned_hosts(after.offers[0]) == ["dana", "ana", "cy"]
confirmed = opportune.check_slot(request(["evt_move"]), opportune.offer_to_proposal(after.offers[0]))
assert confirmed.valid
```

### TypeScript

<!-- snippet: panel-reschedule-ts -->
```ts
import {
  Engine, atLeast, availableOnly, businessHours, capPerDay, offerAssignedHosts, offerStart,
  offerToProposal, person, singleGoal,
} from "opportune";
import type { Block, Offer, Participant, Request } from "opportune";

const [candidate, availability] = availableOnly("candidate", [
  ["2027-06-15T11:00Z", "2027-06-15T16:00Z"],
]);

const booked = (start: string, end: string, key: string): Block => ({
  span: [start, end], tags: ["interview"], event_key: key,
});

const host = (id: string, tags: string[], calendar: Block[]): Participant => ({
  id, zone: "UTC", tags, calendar,
});

const request = (exclude: string[]): Request => ({
  now: "2027-06-15T08:00Z",
  goal: singleGoal(
    "60m",
    { all: [person("dana"), atLeast(2, { tag: "eng/frontend" })] },
    {
      zone: "UTC",
      searchWindow: ["2027-06-15T08:00Z", "2027-06-15T18:00Z"],
      candidates: ["2027-06-15T10:00Z", "2027-06-15T11:00Z"],
      partId: "panel",
      tags: ["interview"],
    },
  ),
  participants: [
    host("dana", ["hiring_manager"], []),
    host("ana", ["eng/frontend"], []),
    host("bo", ["eng/frontend"], []),
    host("cy", ["eng/frontend"], [
      booked("2027-06-15T09:00Z", "2027-06-15T10:00Z", "evt_prev"),
      booked("2027-06-15T13:00Z", "2027-06-15T14:00Z", "evt_move"),
    ]),
    candidate,
  ],
  weights: { ana: 3, bo: 1, cy: 2 },
  rules: [
    businessHours("mon-fri", "09:00-17:00"),
    capPerDay({ tag: "interview" }, 2, { ruleId: "daily_limit" }),
    availability,
  ],
  explain: { limit: 100 },
  exclude_event_keys: exclude,
});

const starts = (offers: Offer[]): string[] => offers.map((offer) => offerStart(offer));

using engine = new Engine();
const before = engine.findAvailability(request([]));
if (starts(before.offers).join() !== "2027-06-15T11:00Z") {
  throw new Error(`the panel was offered ${starts(before.offers).join()}`);
}
const [beforeOffer] = before.offers;
if (beforeOffer === undefined) throw new Error("the panel has no offer");
if (beforeOffer.events[0]?.eligible[1]?.join() !== "ana,bo") {
  throw new Error("cy was expected to be at their daily cap");
}
if (offerAssignedHosts(beforeOffer).join() !== "dana,ana,bo") {
  throw new Error("the panel was not filled by the two free engineers");
}

// Keeping cy is refused while the interview being moved still counts.
const keepCy = {
  events: [{
    part: "panel",
    start: "2027-06-15T11:00Z",
    assignment: [["dana"], ["ana", "cy"]],
  }],
};
const refused = engine.checkSlot(request([]), keepCy);
const capped = refused.reasons.find((reason) => reason.code === "cap_reached");
if (refused.valid || capped?.participant !== "cy" || capped.rule !== "daily_limit") {
  throw new Error("the cap was expected to refuse the proposal that keeps cy");
}

// Naming the event being moved frees the cap, the weights choose ana and cy, and
// the moved slot confirms.
const after = engine.findAvailability(request(["evt_move"]));
const [moved] = after.offers;
if (moved === undefined) throw new Error("the moved panel has no offer");
if (moved.events[0]?.eligible[1]?.join() !== "ana,bo,cy") {
  throw new Error("the excluded interviewer was not eligible again");
}
if (offerAssignedHosts(moved).join() !== "dana,ana,cy") {
  throw new Error("the weights did not choose ana and cy");
}
const confirmed = engine.checkSlot(request(["evt_move"]), offerToProposal(moved));
if (!confirmed.valid) throw new Error("the moved slot no longer confirms");
console.log(starts(after.offers), confirmed.valid);
```

## Graph and Google provider payloads

Fetching is the application's job; the adapters are pure conversions of a
recorded payload into blocks and rules. Most importantly, they fail closed: a
calendar the provider errored for becomes `Unavailable`, and the caller passes
that participant with no calendar so the engine flags their offers
`calendar_unknown` instead of treating them as free.

Graph needs `calendarView` rather than `/events`, the `$expand` clause from
`EXTENDED_PROPERTY_EXPAND` for the marker properties, and `Prefer:
outlook.timezone="UTC"`. Follow `@odata.nextLink` for every page: a single page is
rejected rather than read as a free calendar, and an occurrence's `originalStart`
keeps its event key distinct from the rest of the series. Google needs
`events.list` with `singleEvents=true` and no `maxAttendees`, and every page: a
payload that still carries `nextPageToken` is rejected for the same reason. Both
are documented under [caller requirements](api.md#caller-requirements).

Each language block below carries minimal inline sample payloads (`calendarView`,
`workingHours`, and `events.list`) so you can run it standalone and swap in your
own fetched responses.

### Rust

```rust
// A Graph `calendarView` and a Google `events.list` converted into participants,
// with a calendar the provider could not answer for left unknown rather than free.

use std::collections::BTreeMap;

use opportune::Engine;
use opportune::model::{Goal, Participant, Request, Requirement, Span, Term};
use opportune::presets;
use opportune::tags::{Selector, Tag};
use opportune::time::{
    Date, Duration, Instant, RuleZone, TimeOfDay, WallEnd, Weekdays, ZoneName,
};
use opportune_adapters::graph;
use opportune_adapters::google;
use opportune_adapters::CalendarOutcome;

const CALENDAR_VIEW: &str = r#"{
  "value": [
    {
      "id": "AAMk-interview",
      "isAllDay": false,
      "showAs": "busy",
      "categories": ["Interview"],
      "start": { "dateTime": "2027-06-14T09:00:00.0000000", "timeZone": "UTC" },
      "end": { "dateTime": "2027-06-14T10:00:00.0000000", "timeZone": "UTC" }
    }
  ]
}"#;

const WORKING_HOURS: &str = r#"{
  "daysOfWeek": ["monday", "tuesday", "wednesday", "thursday", "friday"],
  "startTime": "09:00:00.0000000",
  "endTime": "17:00:00.0000000",
  "timeZone": { "name": "America/New_York" }
}"#;

const EVENTS_LIST: &str = r#"{
  "kind": "calendar#events",
  "timeZone": "America/New_York",
  "items": [
    {
      "id": "evt-2",
      "status": "confirmed",
      "start": { "dateTime": "2027-06-14T10:00:00-04:00", "timeZone": "America/New_York" },
      "end": { "dateTime": "2027-06-14T11:00:00-04:00", "timeZone": "America/New_York" }
    }
  ]
}"#;

// A page is not the response: the events after `nextPageToken` are missing from
// the document, so converting the page alone would read them as free time.
const FIRST_PAGE: &str = r#"{
  "kind": "calendar#events",
  "timeZone": "America/New_York",
  "nextPageToken": "CJDUzdKmzZMDFOiPnNAGGAU=",
  "items": []
}"#;

fn utc(date: (i16, i8, i8), clock: (u8, u8)) -> Instant {
    Instant::from_utc(
        Date::new(date.0, date.1, date.2).expect("a valid civil date"),
        TimeOfDay::new(clock.0, clock.1).expect("a valid time of day"),
    )
    .expect("UTC has no transitions")
}

/// The participant a conversion describes: their blocks when the provider
/// answered, and no calendar at all when it did not. An empty calendar would read
/// as completely free, which is the one thing an errored mailbox does not mean.
fn participant(id: &str, zone: &ZoneName, outcome: CalendarOutcome) -> Participant {
    match outcome {
        CalendarOutcome::Available { blocks, .. } => Participant::new(id, zone.clone())
            .with_calendar(blocks.into_iter().map(|entry| entry.block).collect()),
        CalendarOutcome::Unavailable { .. } => Participant::new(id, zone.clone()),
    }
}

fn working_hours_window(
    engine: &Engine,
    payload: &str,
    zone: &ZoneName,
) -> Result<opportune::model::Rule, Box<dyn std::error::Error>> {
    let rule = match graph::working_hours_to_rule(
        engine.time_zone_database(),
        payload,
        "working_hours",
    ) {
        Ok(rule) => rule,
        Err(error) => {
            assert_eq!(error.code(), "custom_time_zone");
            presets::business_hours(
                RuleZone::Named(zone.clone()),
                Weekdays::weekdays(),
                TimeOfDay::new(9, 0)?,
                WallEnd::Time(TimeOfDay::new(17, 0)?),
            )?
        }
    };
    Ok(rule.with_who(Selector::ids(["ana"])))
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let engine = Engine::builder().build()?;
    let database = engine.time_zone_database();
    let new_york = ZoneName::new("America/New_York")?;

    // The query that asks Graph for the markers carries this clause in `$expand`;
    // the response is what the adapter reads.
    assert!(graph::EXTENDED_PROPERTY_EXPAND.contains("opportune.event_key"));

    let categories = BTreeMap::from([("Interview".to_owned(), Tag::new("interview")?)]);
    let view = graph::calendar_view_to_blocks(database, CALENDAR_VIEW, &new_york, Some(&categories));
    let ana = participant("ana", &new_york, view);
    let blocks = ana.calendar.as_deref().ok_or("the calendar view converts")?;
    assert_eq!(blocks.len(), 1);
    assert_eq!(blocks[0].tags, vec![Tag::new("interview")?]);

    // A mailbox Graph could not answer for keeps its participant, without a
    // calendar: its offers carry `calendar_unknown` instead of reading as free.
    let broken = graph::calendar_view_to_blocks(database, r#"{"value":"not a list"}"#, &new_york, None);
    assert!(matches!(
        broken,
        CalendarOutcome::Unavailable {
            code: "payload_invalid",
            ..
        }
    ));
    assert!(participant("bo", &new_york, broken).calendar.is_none());

    // The mailbox's own working hours, scoped to the mailbox's owner, with the
    // caller's window as the fallback for a custom Graph time zone the adapter
    // does not resolve.
    let window = working_hours_window(&engine, WORKING_HOURS, &new_york)?;
    assert_eq!(window.id, "working_hours");
    assert_eq!(window.who, Selector::ids(["ana"]));

    let events = google::events_list_to_blocks(database, EVENTS_LIST, &new_york);
    let cy = participant("cy", &new_york, events);
    assert_eq!(cy.blocks().len(), 1);
    let page = google::events_list_to_blocks(database, FIRST_PAGE, &new_york);
    assert!(matches!(
        page,
        CalendarOutcome::Unavailable {
            code: "payload_invalid",
            ..
        }
    ));

    let mut request = Request::new(
        utc((2027, 6, 14), (0, 0)),
        Goal::new(
            new_york,
            Span::new(utc((2027, 6, 14), (0, 0)), utc((2027, 6, 15), (0, 0)))?,
            vec![opportune::Part::new(
                Duration::from_minutes(30),
                Requirement::new(vec![Term::person("ana")]),
            )],
        ),
    );
    request.participants = vec![ana, cy];
    request.rules = vec![window];
    assert!(!engine.find_availability(&request)?.offers.is_empty());
    Ok(())
}
```

### Python

<!-- snippet: providers-python -->
```python
import opportune
from opportune.adapters import google, graph

# Minimal sample provider responses (str, bytes, or parsed dict all work).
calendar_view_payload = {
    "value": [
        {
            "id": "AAMk-interview",
            "isAllDay": False,
            "showAs": "busy",
            "categories": ["Interview"],
            "start": {"dateTime": "2027-06-14T09:00:00.0000000", "timeZone": "UTC"},
            "end": {"dateTime": "2027-06-14T10:00:00.0000000", "timeZone": "UTC"},
        }
    ]
}
working_hours_payload = {
    "daysOfWeek": ["monday", "tuesday", "wednesday", "thursday", "friday"],
    "startTime": "09:00:00.0000000",
    "endTime": "17:00:00.0000000",
    "timeZone": {"name": "America/New_York"},
}
events_list_payload = {
    "kind": "calendar#events",
    "timeZone": "America/New_York",
    "items": [
        {
            "id": "evt-2",
            "status": "confirmed",
            "start": {"dateTime": "2027-06-14T10:00:00-04:00", "timeZone": "America/New_York"},
            "end": {"dateTime": "2027-06-14T11:00:00-04:00", "timeZone": "America/New_York"},
        }
    ],
}

view = graph.calendar_view_to_blocks(
    calendar_view_payload, "America/New_York", {"Interview": "interview"}
)
calendar = (
    [entry.block for entry in view.blocks] if view.status == "available" else None
)

hours = graph.working_hours_to_rule(working_hours_payload, who={"ids": ["ana"]})
rule = (
    hours.rule
    if hours.status == "available"
    else opportune.presets.business_hours(who={"ids": ["ana"]})
)
assert rule.who == opportune.IdsSelector(ids=["ana"])

events = google.events_list_to_blocks(events_list_payload, "America/New_York")
events_calendar = (
    [entry.block for entry in events.blocks] if events.status == "available" else None
)

request = opportune.Request(
    now="2027-06-14T00:00Z",
    goal=opportune.single_goal(
        "30m",
        opportune.Requirement(all=[opportune.person("ana")]),
        zone="America/New_York",
        search_window=("2027-06-14T00:00Z", "2027-06-15T00:00Z"),
    ),
    participants=[
        # `calendar=None` is "unknown", which is why an unavailable outcome maps to
        # an omitted calendar rather than to `[]`.
        opportune.Participant(id="ana", zone="America/New_York", calendar=calendar),
        opportune.Participant(id="cy", zone="America/New_York", calendar=events_calendar),
    ],
    rules=[rule],
)
response = opportune.find_availability(request)
assert response.offers   # the converted calendars leave bookable time
```

### TypeScript

<!-- snippet: providers-ts -->
```ts
import { Engine, businessHours, singleGoal, person } from "opportune";
import * as graph from "opportune/adapters/graph";
import * as google from "opportune/adapters/google";

// Minimal sample provider responses (JSON strings or parsed objects both work).
const calendarViewPayload = {
  value: [
    {
      id: "AAMk-interview",
      isAllDay: false,
      showAs: "busy",
      categories: ["Interview"],
      start: { dateTime: "2027-06-14T09:00:00.0000000", timeZone: "UTC" },
      end: { dateTime: "2027-06-14T10:00:00.0000000", timeZone: "UTC" },
    },
  ],
};
const workingHoursPayload = {
  daysOfWeek: ["monday", "tuesday", "wednesday", "thursday", "friday"],
  startTime: "09:00:00.0000000",
  endTime: "17:00:00.0000000",
  timeZone: { name: "America/New_York" },
};
const eventsListPayload = {
  kind: "calendar#events",
  timeZone: "America/New_York",
  items: [
    {
      id: "evt-2",
      status: "confirmed",
      start: { dateTime: "2027-06-14T10:00:00-04:00", timeZone: "America/New_York" },
      end: { dateTime: "2027-06-14T11:00:00-04:00", timeZone: "America/New_York" },
    },
  ],
};

const view = graph.calendarViewToBlocks(calendarViewPayload, "America/New_York", {
  categoryMap: { Interview: "interview" },
});
const calendar = view.status === "available" ? view.blocks.map((entry) => entry.block) : null;

const hours = graph.workingHoursToRule(workingHoursPayload, { who: { ids: ["ana"] } });
const rule = hours.status === "available"
  ? hours.rule
  : businessHours("mon-fri", "09:00-17:00", { who: { ids: ["ana"] } });
if (JSON.stringify(rule.who) !== JSON.stringify({ ids: ["ana"] })) {
  throw new Error("the working-hours rule is not scoped to Ana");
}

const events = google.eventsListToBlocks(eventsListPayload, "America/New_York");
const eventsCalendar =
  events.status === "available" ? events.blocks.map((entry) => entry.block) : null;

const request = {
  now: "2027-06-14T00:00Z",
  goal: singleGoal("30m", { all: [person("ana")] }, {
    zone: "America/New_York",
    searchWindow: ["2027-06-14T00:00Z", "2027-06-15T00:00Z"],
  }),
  participants: [
    // An omitted calendar is "unknown"; `calendar: []` would claim it is free.
    { id: "ana", zone: "America/New_York", ...(calendar === null ? {} : { calendar }) },
    { id: "cy", zone: "America/New_York", ...(eventsCalendar === null ? {} : { calendar: eventsCalendar }) },
  ],
  rules: [rule],
};

using engine = new Engine();
const offers = engine.findAvailability(request).offers;
if (offers.length === 0) throw new Error("the converted calendars leave no bookable time");
console.log(offers.length);
```

Both adapter submodules accept the payload as JSON text or as a parsed object;
the options object is the third argument for `calendarViewToBlocks` and
`eventsListToBlocks`, and the second for the batch conversions. The signatures,
the browser, module Web Worker, and adapter runtime limits and the `opportune/presets`
subpath are in [WASM and TypeScript](wasm.md#graph-and-google-adapters).

## Production blueprints

These are application-side patterns. The engine stays pure; the transaction,
dispatch loop and tenant lifecycle belong to your service.

### Committing a booking atomically

`check_slot` is a fresh check, not a lock. Re-read the participant's blocks inside
the transaction, let `check_slot` judge the proposal, then persist the event and
its travel holds before committing. Lock participants in **ID order** so two
concurrent bookings cannot deadlock.

```sql
BEGIN;
-- 1. Serialize writers for every participant the booking needs, in ID order.
SELECT id FROM participants WHERE id = ANY($participant_ids) ORDER BY id FOR UPDATE;

-- 2. Re-read the calendar rows and occurrences the request was built from.
SELECT event_key, span, tags FROM calendar_blocks
 WHERE participant_id = ANY($participant_ids) AND span && $fetch_hull;

-- 3. Rebuild the Request, then call check_slot(request, proposal) in the service.
--    A malformed proposal is an application error; valid = false is a conflict.
-- 4. Insert the booking with a unique (participant_id, span) constraint as the
--    last line of defence, and write the offer's reserve spans as holds.
INSERT INTO bookings (event_key, participant_id, span) VALUES (...);
INSERT INTO holds (participant_id, span, kind) VALUES (...);
COMMIT;
```

Use the same `event_key` on the booking and every hold, so a later reschedule can
pass it in `exclude_event_keys`. Return `409` for `valid: false` after the
re-read, not a `200` with an empty answer, so the client knows to pick another
slot.

### Weighting a round-robin pool

`Request.weights` is an `i32` map; **higher wins**, omitted is `0`, and ties break
by participant ID. That is enough for round-robin without a dispatcher service:

- `weight = -recent_bookings[host]` prefers the least-booked host.
- `weight = tier * 10_000 - recent_bookings[host]` prefers a tier first and
  recent load second; keep the result inside `i32` or clamp it.
- For a custom tie-break, read `Offer.events[0].eligible` (IDs sorted, never
  weight-ranked) and pick the host your product wants, then send that choice back
  through `Offer::to_proposal` / `offerToProposal` and `check_slot`.

Rebuild weights between searches. A cap is a hard ceiling; a weight is a
preference, and neither replaces the other.

### Counting a shared event once

A panel interview or group session appears on several participants' calendars.
Pass it once as an `Occurrence` so a pooled cap does not count it once per
calendar:

```text
Occurrence::new("interview-42", span, ["ana", "bo", "cy"])
    .with_tags(vec![Tag::new("interview")?])
request.occurrences = vec![occurrence];
```

Every block with `event_key == "interview-42"` is then the same event: the
occurrence is authoritative for the span and participants, and its tags are what a
cap's `count` selector sees. `required_ranges` returns the `occurrences` hull
beside the calendar hull; fetch cap-counted blocks over the union of the two.

### One engine per limit profile

Limits live on `Engine`, not on a request. Reuse one engine for every tenant that
shares the same defaults, and build one per distinct limit profile (free, pro,
enterprise) rather than one per tenant or per request. The bundled time zone
database is read-only and the engine holds no per-request mutable state, so one
engine serves concurrent calls. The Python bindings release the GIL around each
call.

In Node, an engine call is synchronous. One engine per process is fine for normal
traffic; when a search is large enough to block the event loop, run that call in a
`worker_threads` worker (or a process per core) and keep the main loop serving
requests. Python offers `*_async` for the same reason; TypeScript does not.

## What the recipes share

- **`now` is an argument, never a clock.** Every request names it, and the engine
  reads no system time, so a stored request and an engine give the same answer
  wherever they run. Persist `now` beside the request if the answer is stored.
- **Limits are per engine.** The defaults cover ten caps: participants, blocks
  per participant, zone periods per participant, rules, occurrences, requirement
  slots and terms, the search window, candidate starts, `explain.limit` and the
  longest booking, travel or clearance.
  Raise one with `Engine::builder().limits(...)`, `opportune.limits_with(...)` or
  `limitsWith({...})`; a request over a cap is rejected with a stable code, not
  silently truncated.
- **Operator diagnostics are not public availability data.** `explain` is opt-in and
  bounded by `explain.limit` (reporting `excluded_truncated` when it stopped early),
  and `Response.warnings`, `Offer.events[].eligible`/`assignment`/`reserve`, and
  `SlotCheck.reasons`/`warnings` also carry internal host IDs, rule IDs, or calendar
  details. Force `explain` off and strip operator-only fields on public booking
  routes — `opportune.public_response` / `public_slot_check` and the npm
  `publicResponse` / `publicSlotCheck` do it for you, and the
  [privacy note](api.md#reasons-and-explain) lists the fields.
- **Failures are structured.** Every engine, adapter, and preset surface answers a
  rejection with a code, a message, and where the document is wrong:
  `Error::code()`/`Error::path()` in Rust, `OpportuneError` in Python (with
  `pydantic.ValidationError` for direct model construction and `with_updates`),
  `OpportuneError` in TypeScript, and one `ErrorEnvelope` body from a FastAPI
  handler.
