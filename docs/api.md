# API reference

Reference for the Rust core (`opportune`), JSON wire layer (`opportune-json`),
provider adapters (`opportune-adapters`), and Python package (`opportune`).

## Rust

### `opportune::Engine`

```rust
pub struct Engine { /* holds the time zone database and the limits */ }

impl Engine {
    pub fn builder() -> EngineBuilder;
    pub fn find_availability(&self, request: &Request) -> Result<Response, Error>;
    pub fn check_slot(&self, request: &Request, proposal: &Proposal) -> Result<SlotCheck, Error>;
    pub fn validate(&self, request: &Request) -> Diagnostics;
    pub fn required_ranges(&self, query: &RangesQuery) -> Result<RequiredRanges, Error>;
    pub fn time_zone_database(&self) -> &TimeZoneDatabase;
    pub fn limits(&self) -> &Limits;
}
```

The engine holds the time zone database and the request limits; a request holds
neither, so it can be built, stored and parsed without either. Build one once
and reuse it. **Limits live on the engine, not on a request**, so reuse one
engine per distinct limit profile: a process-wide engine when every tenant shares
the defaults, or one engine per plan/tier that needs its own caps.

**The database is always an explicit choice, never the host's**, because an
answer that depends on the machine is not reproducible:

```rust
use opportune::Engine;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // The database compiled into the binary (the default with `bundled-tzdb`).
    let engine = Engine::builder().tzdb_bundled().build()?;
    assert!(!engine.time_zone_database().version().is_empty());

    // A directory of `TZif` files, labeled from its `+VERSION` file when it
    // has one, or from the label the caller passes when it does not.
    let engine = Engine::builder()
        .tzdb_dir("/usr/share/zoneinfo", Some("2027a"))
        .build()?;
    assert!(!engine.time_zone_database().version().is_empty());
    Ok(())
}
```

`Engine::builder().build()` uses the bundled database when the `bundled-tzdb`
feature is on. Without that feature there is no default engine and no host
fallback: `build()` fails with `tzdb_not_configured` until a directory is given.

### The four entry points

`find_availability` books forwards: every candidate start the participants'
calendars, rules, caps and rest allow, in start order, each with the hosts who
could take it, the travel to reserve, and the civil dates the search window
touches. `check_slot` judges one proposed booking against the same rules and
calendars at confirmation time, ignoring the grid and the search window, and
always reports every failing reason. `validate` reports the errors and warnings
a request has before any availability is computed. `required_ranges` answers the
caller's fetch responsibility, below.

With the default `bundled-tzdb` feature each is also a free function using a
default engine with the bundled database and the default limits:

```rust
pub fn find_availability(request: &Request) -> Result<Response, Error>;
pub fn check_slot(request: &Request, proposal: &Proposal) -> Result<SlotCheck, Error>;
pub fn validate(request: &Request) -> Diagnostics;
pub fn required_ranges(query: &RangesQuery) -> Result<RequiredRanges, Error>;
```

```rust
use opportune::{
    Engine,
    model::{Goal, Participant, Request, Requirement, Span, Term},
};
use opportune::time::{Date, Duration, Instant, TimeOfDay, ZoneName};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let zone = ZoneName::utc();
    let now = Instant::from_utc(Date::new(2027, 6, 14)?, TimeOfDay::new(0, 0)?)?;
    let until = Instant::from_utc(Date::new(2027, 6, 15)?, TimeOfDay::new(0, 0)?)?;
    let window = Span::new(now, until)?;
    let mut request = Request::new(
        now,
        Goal::single(
            zone.clone(),
            window,
            Duration::from_minutes(30),
            Requirement::new(vec![Term::person("ana")]),
        ),
    );
    request
        .participants
        .push(Participant::new("ana", zone).with_calendar(Vec::new()));

    let response = opportune::find_availability(&request)?;
    assert_eq!(response.offers.len(), 48);
    assert_eq!(
        response.meta.tzdb,
        Engine::builder().build()?.time_zone_database().version()
    );
    Ok(())
}
```

### The response shape

`Response` is the one answer `find_availability` returns, and every field is
additive within a schema version: `meta.schema` names the version (1 today), and
new fields, flags and reason codes arrive beside the old ones rather than
replacing them.

| Field | Type | What it is |
| --- | --- | --- |
| `offers` | `Vec<Offer>` | The bookable starts, in start order, cut at `Request::limit` (1,000 by default). |
| `offers[].events` | `Vec<OfferEvent>` | One event per goal part. Version 1 requires exactly one part, and `Offer::start`/`end` read its first/last event. |
| `offers[].events[].part` | `String` | The part's ID — `main` unless the caller names one. An excluded start names the same `part` in `excluded[]`; a `Reason` inside it does not carry one. |
| `offers[].events[].start`, `.end` | `Instant` | The booking's span. |
| `offers[].events[].eligible` | `Vec<Vec<String>>` | One list per requirement term: the hosts that can fill that term at this start, IDs sorted (never weight-ranked). An `at_least(0)` term still lists the hosts that *would* fill it here, while its matching `assignment` list stays empty. |
| `offers[].events[].assignment` | `Vec<Vec<String>>` | The hosts chosen by weight (ties broken lexicographically by ID), one list per term in the same order as `eligible`; each name is drawn from the matching `eligible` list. An `at_least(0)` term always has an empty assignment, and `check_slot` requires each term's exact declared count. |
| `offers[].events[].reserve` | `Vec<ReserveSpan>` | Travel to hold on a calendar: `{participant, kind, span}`, with `kind: travel` today. |
| `offers[].flags` | `Vec<OfferFlag>` | What the caller should know that does not make the offer invalid. `calendar_unknown` means an assigned host has no calendar, so their availability was assumed rather than checked. |
| `dates` | `Vec<DateCount>` | `{date, offers}` for every civil date the search window touches in the display zone, counted **before** `limit` truncates the offer list, so a date picker never reads a cut-off day as empty. A date the rules close has `offers: 0`. |
| `truncated` | `bool` | Whether `limit` cut `offers` short. |
| `excluded` | `Vec<Excluded>` | `{start, part, reasons}` for the starts that were not offered, in start order. Empty unless `Request::explain` is set. |
| `excluded_truncated` | `bool` | Whether `explain.limit` cut the exclusion list short. Always `false` without an explanation. |
| `warnings` | `Vec<Diagnostic>` | Request warnings that did not stop the search, such as a window no candidate's booking and travel fits inside. |
| `meta` | `Meta` | The versions the answer was produced from: `engine`, `schema` and `tzdb`. |

`check_slot` answers a different shape, documented under
[`check_slot` and proposals](#check_slot-and-proposals). A rendered booking page
is worked through in [the personal booking link recipe](recipes.md#a-personal-30-minute-booking-link).

### Product-model constraints in v1

These are deliberate v1 limits, not bugs to work around:

- **One part per goal.** `Goal.parts` is a list so a later version can book a
  multi-part itinerary, but v1 requires `parts.len() == 1`. `Goal::single` and
  `Goal::single_part` build that one part; a larger list is rejected with
  `part_count` by `validate`, `find_availability`, `check_slot` and
  `required_ranges`.
- **`status_policy` is first-match-wins across every applicable rule.** A
  three-tier org → team → host application must put the **most specific policy
  first** (`[host, team, org]`), because the first matching entry decides.
- **`Selector::kind(Person)` includes attendees.** To scope a human *host* rule
  away from both resource rooms and invitees, use
  `Selector::all([Selector::role(Role::Host), Selector::kind(ParticipantKind::Person)])`.
- **`calendar_unknown` is host-only.** The flag tells you an assigned host had no
  calendar. An attendee's calendar fetch is the application's to check; the engine
  never assigns an attendee.
- **Multi-seat group classes and webinars are product-layer objects.** One booking
  that admits N attendees is not a `cap`; model fixed seats as `Resource`
  participants (each booked once) or manage capacity in the application.

### `required_ranges` and the caller's fetch responsibility

The engine never fetches anything. `required_ranges` takes the request's `now`,
goal, rule list and participants and answers which calendar and occurrence data
the caller has to have before the search:

```rust
pub struct RequiredRanges {
    pub calendar: Span,          // UTC range every participant's calendar must cover
    pub occurrences: Option<Span>, // the occurrence range, `None` when no cap applies
}
```

Build a query with `RangesQuery::new(now, goal)`, then
`.with_participants(...)` and `.with_rules(...)`; the participants carry no
calendars, because the planner computes the range before any block is fetched.

`calendar` is the search window padded by the part's travel, clearance, and
duration, then widened to whole civil days with one day either side, read in
every zone a participant can be in: a block just outside the window that would
block the first offered start is included, and so is the neighboring day a
`rest` rule reads. (If existing calendar blocks carry their own
`travel_before`/`travel_after` longer than the one-day hull margin, fetch events
whose span plus travel overlaps `calendar`.) `occurrences` is present only when
a cap applies, and it covers every bucket the search window touches in the cap's
own zone — those days for a per-day cap, and the whole weeks or months they fall
in for the longer periods. **Because caps count both `occurrences` and matching
calendar blocks, fetch cap-counted calendar blocks over the union of `calendar`
and `occurrences` whenever `occurrences` is present.** (`RangesQuery.now` is
carried for wire symmetry with `Request` and does not clip the fetch hull.) Data
outside these ranges (subject to block-travel overlap) cannot change the answer.

`required_ranges` does not require a prior `validate`: a query whose goal is not
exactly one part is refused with `Error::PartCount`, and a query whose participant
count, rule count or per-participant zone-period count exceeds the configured
`Limits` is refused with `Error::InvalidRequest` carrying the same
`participants_limit`, `rules_limit` and `zone_periods_limit` findings the request
validator reports, before any schedule is read. It reads no calendar block and no
occurrence, matches no requirement and generates no candidate start, so the
collection and requirement caps on those have nothing to refuse here. The window
and duration caps are request-search bounds rather than fetch rejection rules:
padding by the booking's own duration and travel is what makes the fetch
sufficient, and resolving each zone still reads the offset transitions inside the
resulting hull. A query that names a very long booking, travel or window therefore
receives a correspondingly wide hull and pays its zone lookups; refusing those
values is the request validator's, not this call's.

### Rule kinds

Rules are typed data with fixed meaning, and the caller supplies the final list;
the engine combines them by fixed semantics. Every rule kind has a domain type
and builders, evaluation and combination, an independent reference model in the
test suite, fixtures that include and exclude, reason codes, and a contribution
to `validate` and `required_ranges`.

| Kind | Type | What it does |
| --- | --- | --- |
| `window` | `time::WindowSpec` | The times a booking may occupy, by weekday and by date, read in the participant's, the event's or a named zone. |
| `blackout` | `time::BlackoutSpec` | Local date-time spans inside which nothing may be booked. |
| `clearance` | `Clearance` | Gaps a booking needs before and after existing events, plus a minimum gap between two bookings. |
| `lead_time` | `LeadTime` | Per-participant minimum notice, as a duration, and maximum horizon, in durations, days, dates or working days. |
| `status_policy` | `StatusPolicy` | How a block's status and tags are treated — `block` or `ignore` — so a `free` block can be made to block and a `busy` one ignored. |
| `rest` | `Rest` | Protects the hours before a stated time after any activity that finished after a stated time the day before. A block with `all_day` set is never activity, so an out-of-office day protects no morning. |
| `cap` | `Cap` | Workload limits by count or minutes, per participant or pooled, over a day, week or month, keyed on the booking's start date label. A block with `all_day` set contributes no workload of its own to a cap — an out-of-office or holiday block is not meeting time — while still blocking the day it covers. Its tags are still the event's, so an all-day block sharing an `event_key` with an occurrence can bring that event into a cap's `count`. |

`RuleKind` and `CapPeriod` are `#[non_exhaustive]` in Rust, so `0.2+` can add a
rule kind or a cap period without a breaking change; a `match` outside the crate
needs a `_` arm. Every kind and period listed here keeps its meaning.

#### How rules of one kind combine

Every rule in the request that applies to a participant and to the booking is
applied; a later rule never replaces an earlier one. Combination is fixed per
kind, and is part of the public contract rather than an implementation detail:

| Kind | How several applicable rules combine |
| --- | --- |
| `window` | **Intersect.** A booking has to lie inside every applicable window, so two windows that both apply leave only the hours they share. A week with different hours per day is one window (`weekly_hours`), not one rule per day — several `business_hours` rules would intersect rather than each contributing its own day or range. |
| `blackout` | **Union.** A booking is blocked when any applicable blackout covers it. |
| `clearance` | **The field-wise maximum.** `before`, `after` and `min_gap` are each the largest value any applicable rule asks for, never a sum: ten minutes of buffer beside a five-minute buffer stays ten. |
| `lead_time` | **The strictest of every bound.** The largest `min` is the notice required, and of every `max` the one permitting the fewest starts wins, so the shortest allowed range results. |
| `status_policy` | **First match wins**, across every applicable policy in request order: the first entry that matches a block decides whether it blocks. |
| `rest` | **Independent.** Each rule is evaluated on its own `late_after`/`not_before` pair; one rule's late time is never paired with another's early time. |
| `cap` | **Independent and all binding.** A booking has to fit under every applicable cap's own bucket. A cap applies only when its `when` and its `count` both select the booking, so a cap measuring another kind of event does not constrain this one. |

A rule's `who` selects the participants it constrains (every host by default) and
its `when` selects the bookings it applies to by the part's tags (all of them by
default).

#### Layered policies (global, event-group, and event-type overrides)

Because rules of the same kind combine rather than overwrite, **tightening** at a
narrower tier composes automatically (for example, adding a Mon–Thu `10:00–16:00`
window with `when: tag("interview")` narrows a global Mon–Fri `09:00–17:00`
window). When a narrower tier needs to **loosen or replace** a broader rule (for
example, an executive event type that opens Saturday hours or uses a shorter
minimum notice):

- Exclude the narrower tag from the broader rule with `when: all([..., not(tag("vip"))])`
  (or `when: not(tag("vip"))` on a global rule), then supply the complete rule for
  `when: tag("vip")`.
- For **caps**, `when` controls whether the cap restricts the booking while `count`
  controls which events consume the bucket: negate `count` (`all([count, not(tag("vip"))])`)
  if the narrower event should stop consuming the broad cap, or negate only `when` if
  it should still count toward the broad cap without being limited by it.
- For **`status_policy`**, place more specific policies **before** broader ones,
  since first match wins in request order.

### Selectors

`who` and `when` are both `Selector` values, and every form is data:

| Form | Constructor | Matches |
| --- | --- | --- |
| All | `Selector::True` | Every participant and every booking. |
| Tag | `Selector::tag(tag)` | Items or participants carrying the tag or a descendant of it, so `tag("interview")` also matches `interview/frontend`. |
| IDs | `Selector::ids(["ana"])` | Participants with one of those IDs. Participant-only, like `Kind` and `Role`: a booking's tags never match it. |
| Role | `Selector::role(Role::Attendee)` | Participants with that role. |
| Kind | `Selector::kind(ParticipantKind::Person)` | Participants of that kind. |
| Any/All/Not | `Selector::any`, `::all`, `::not` | The boolean combination of the nested selectors. |

`Selector::kind` is how a rule about people is kept off a conference room: a
`ParticipantKind::Resource` participant is a host like any other, so a rule whose
`who` is left at the default reaches it. Scoping the human-only rules — the
`window` hours, `rest`, the `cap`, and a `status_policy` that ignores tentative
holds — with `Selector::kind(ParticipantKind::Person)` leaves the room's own holds
blocking; [the team-and-room recipe](recipes.md#an-internal-team-and-a-conference-room)
works the whole request. The same clause inside the selector of an `at_least` term
keeps a room out of a slot that was meant for a person:
`Term::at_least(1, Selector::all([Selector::tag(tag("eng")), Selector::kind(ParticipantKind::Person)]))`.
A `Kind` clause in an item position — a rule's `when`, a cap's `count`, or a
`status_policy` entry's tag selector — is rejected, because a booking carries no
kind. `ids`, `role` and `kind` are all participant-only: using any of them there
is the same `selector_not_for_items` error rather than a clause that silently
matches nothing (and, under a `not`, everything).

A `status_policy` whose entries ignore a status also reaches the rooms the request
carries, because the default `who` is every host — and an ignored tentative hold is
a double-booked room. `validate` therefore reports `status_policy_ignores_resource`
at `/rules/{index}` whenever a policy with an `ignore` entry selects a resource
participant, naming the room and suggesting `who: {"kind": "person"}`.

### Candidate starts and the default grid

A goal offers starts from its own `grid`, or from an explicit `candidates` list;
a request carrying both is rejected. A goal with neither uses the default grid:
a step of `min(part.duration, 30m)` — never less than one minute — anchored at
`00:00` in the goal's event `zone`. So a 15-minute booking is offered on quarter
hours and anything 30 minutes or longer on half hours, counted from local
midnight in the event zone, not the display zone or a participant's own zone.
`Goal::single` sets neither, so a booking link built with it gets that default.

### Rescheduling an existing booking

`Request::exclude_event_keys` names events the caller is moving. An excluded key
takes no part in the search: the block and occurrence carrying it neither blocks
time nor triggers `rest` nor consumes a cap, so a booking can be moved into a slot
its own old copy would otherwise have closed, and the interviewer already at
their daily limit stays eligible once the event being moved stops counting against
it. An entry that is empty or whitespace is rejected with `event_key_empty` at
`/exclude_event_keys/{i}`, and the list is bounded by the occurrence limits, so an
unbounded exclusion list fails closed rather than scanning the calendars per key.

```rust
let request = request.exclude_event_key("evt_move");   // or with_exclude_event_keys(vec![...])
```

The same field is on the wire, so `{"exclude_event_keys": ["evt_move"]}` is the
Python and TypeScript spelling. `required_ranges` does not read it, so the Python
`required_ranges(query)` and TypeScript `requiredRanges(query)` accept a whole
`Request` and project away the fields the query does not carry.

### Weights, occurrences and working calendars

`Request::weights` maps a participant ID to an `i32` used only when choosing
among eligible hosts. **A higher weight wins**; an omitted participant has weight
`0`; a tie breaks lexicographically by participant ID. The chosen assignment
maximizes the total weight, not each term greedily. Keep the value in `i32` range
and clamp an application-computed score before inserting it.

`Request::occurrences` is how a shared event is counted once for a pooled cap:
one `Occurrence` carries the event's `event_key`, `span`, `participants` and
`tags`, and every participant's calendar block with that key is treated as the
same event. Build it with `Occurrence::new(key, span, participants)` and attach
tags with `Occurrence::with_tags`; `required_ranges` reports the bucket hull the
caller must fetch as `occurrences`. See [the occurrence blueprint](recipes.md#counting-a-shared-event-once)
for the whole pattern.

`Request::calendars` names the `CalendarSpec`s a `LeadTimeMax::WorkingDays` horizon
reads: a zone, a `WorkingDays` set and holiday dates. The calendar answers which
days count as working days; it does not close availability by itself.

### Presets

`opportune::presets` builds the common rules into `Rule` values, generating each
rule's ID from its own name — except `available_only`, which derives it from the
attendee:

```rust
pub fn business_hours(zone, days, from, until) -> Result<Rule, WindowError>;
pub fn business_hours_ranges(zone, days, hours: Vec<WallRange>) -> Result<Rule, WindowError>;
pub fn dated_hours(zone: RuleZone, dates: Vec<DateKey>) -> Result<Rule, WindowError>;
pub fn weekly_hours(zone: RuleZone, weekly: Vec<WeeklyKey>, dates: Vec<DateKey>) -> Result<Rule, WindowError>;
pub fn blackout(zone: RuleZone, spans: Vec<BlackoutSpan>) -> Rule;
pub fn buffers(before: Duration, after: Duration) -> Rule;
pub fn buffers_with_gap(before: Duration, after: Duration, min_gap: Duration) -> Rule;
pub fn min_notice(notice: Duration) -> Rule;
pub fn horizon(max: LeadTimeMax) -> Rule;
pub fn rest(late_after: TimeOfDay, not_before: TimeOfDay) -> Rule;
pub fn cap_per_day(count: Selector, limit: u32) -> Rule;
pub fn cap_per_day_with(count: Selector, limit: u32, per: CapPer, zone: RuleZone) -> Rule;
pub fn cap_per_week(count: Selector, limit: u32, per: CapPer, zone: RuleZone) -> Rule;
pub fn cap_per_month(count: Selector, limit: u32, per: CapPer, zone: RuleZone) -> Rule;
pub fn cap_minutes_per_day(count: Selector, limit_minutes: u32, per: CapPer, zone: RuleZone) -> Rule;
pub fn cap_minutes_per_week(count: Selector, limit_minutes: u32, per: CapPer, zone: RuleZone) -> Rule;
pub fn cap_minutes_per_month(count: Selector, limit_minutes: u32, per: CapPer, zone: RuleZone) -> Rule;
pub fn status_policy(entries: Vec<StatusPolicyEntry>) -> Rule;
pub fn available_only(id, spans: &[Span]) -> Result<(Participant, Rule), WindowError>;
```

`business_hours` and `WallRange::new` take `until: impl Into<WallEnd>`, so a
`TimeOfDay` is enough for every end before midnight; write `WallEnd::Midnight` for
`24:00`.

Presets construct the standard `Rule` types directly, with no parallel semantics:

- **Unique IDs & scoping:** Rule IDs must be unique in a request. Use `Rule::with_id` to rename a second preset of the same kind, and `Rule::with_who` / `Rule::with_when` to scope applicability.
- **Loosening broader tiers (`Rule::unless`):** `Rule::unless` excludes a selector from a rule's `when` clause so a narrower tier can widen a global or group rule instead of intersecting with it. On a `cap`, `unless` exempts matching bookings from the cap's *judgment* only — the booked event still consumes the cap's `count` bucket if its tags match `count`. Narrow both `when` and `count` when an excepted event should also stop consuming the bucket.
- **Human-only hosts (`Rule::and_who` / `Rule::except_who`):** The participant-side twins of `unless` narrow `who` instead of replacing it. `and_who` intersects a selector into the rule's existing `who`, so `presets::business_hours(...).unwrap().and_who(Selector::kind(ParticipantKind::Person))` keeps the default `{"role": "host"}` clause beside the new one and reaches human hosts alone — neither resource rooms nor attendees. `except_who` is `and_who(Selector::not(...))`, so `.except_who(Selector::ids(["ceo"]))` exempts the CEO while every other host stays subject. Prefer these over `with_who(Selector::kind(...))`, which replaces `who` outright and therefore also judges attendees.
- **Scoping `presets::status_policy`:** Unscoped, a `status_policy` applies to every host including `Resource` conference rooms, so an `ignore` entry on tentative holds would drop room holds. Pass `.with_who(Selector::kind(ParticipantKind::Person))` for people-only policies.
- **Caps and `all_day` blocks:** A `cap` counts each event once per bucket and ignores workload from `all_day` blocks (such as out-of-office or holiday blocks) while those blocks still block the day. If an all-day block shares an `event_key` with an occurrence, its tags still apply to that event.
- **Multi-range weeks (`weekly_hours`):** Express a full week of split shifts and holiday overrides in one `weekly_hours` rule (each `WeeklyKey` names the days sharing its hours, omitted days are closed, and `DateKey` overrides or closes specific dates) instead of adding multiple `business_hours` rules that intersect.
- **Whole-day closures (`blackout`):** A `blackout` takes local date-time spans that union, so a holiday, maintenance window or company shutdown closes the time it covers without also becoming a `window` that every other rule intersects with.
- **External stated availability (`available_only`):** Names its rule `available_only:{id}` from the attendee so multiple attendees compose in one request (rename with `Rule::with_id` if needed). It rejects the preset call itself when a span is not a whole-minute RFC 3339 instant or lies outside `1975-01-01T00:00Z..=2100-01-01T00:00Z`, and rejects an availability exceeding `MAX_AVAILABLE_ONLY_DAYS` (3,660 dated entries).

### Single-part sugar

```rust
impl Goal  {
    pub fn single(zone, search_window, duration, requirement) -> Goal;
    pub fn single_part(zone, search_window, part: Part) -> Goal;
}
impl Offer {
    pub fn start(&self) -> Option<Instant>;
    pub fn end(&self) -> Option<Instant>;
    pub fn assigned_hosts(&self) -> Vec<&str>;
    pub fn reserve(&self) -> Vec<&ReserveSpan>;
    pub fn calendar_unknown(&self) -> bool;
    pub fn to_proposal(&self) -> Proposal;
}
impl Span { pub const EVERYTHING: Span; }   // [Instant::MIN, Instant::MAX)
impl ZoneName { pub fn utc() -> ZoneName; }
```

`Goal::single` is `Goal::new` with one `Part::new`; `Goal::single_part` is the
same shortcut when the caller already holds a `Part`. `Span::EVERYTHING` is the
widest legal search window, and `ZoneName::utc()` is the infallible `"UTC"`
spelling.

`Offer::start` and
`Offer::end` read the first and last event (the same event in version 1), and
`Offer::reserve` flattens the travel every event holds, which is what a caller
writes on each participant's calendar. `Offer::calendar_unknown` is true when an
assigned host has no calendar, so the offer's availability was assumed rather
than checked — the same fact the offer also carries as a `calendar_unknown` flag.
`Offer::to_proposal` builds the proposal
`check_slot` confirms, so an offer a caller was given can be handed straight back.

Request data is built with the types' own constructors: `Instant::from_utc` maps a
civil date and time to its UTC instant, `Instant::checked_add`/`checked_sub` move
one without leaving the supported range, and `Travel::new` with
`Travel::with_participant` writes travel without nesting maps by hand.

### Reasons and `explain`

```rust
pub struct Explain { pub limit: usize }        // Request::explain
pub struct Excluded { pub start: Instant, pub part: String, pub reasons: Vec<Reason> }
pub struct Reason {
    pub participant: Option<String>,   // `None` for a start-level reason
    pub code: ReasonCode,
    pub rule: Option<String>,
    pub event_key: Option<String>,
    pub tags: Vec<Tag>,
    pub params: Option<ReasonParams>,
}
```

With `Request::explain` set, `Response::excluded` lists the candidate starts that
were not offered, in start order, each with the first failing reason for every
participant the part's terms select and for every attendee it needs. Entries are
atomic: output stops before the first start that would take the total reason
count past `explain.limit`, setting `excluded_truncated`. Without `explain`,
`excluded` is empty and `excluded_truncated` is false.

**`explain`, `warnings`, `eligible`/`assignment`/`reserve`, and `SlotCheck.reasons`
contain operator-facing details, not public availability data.** A reason names the
participant whose calendar or rule blocked the start, the event key and tags of the
event that did it, and a cap's usage in its bucket; `Response::warnings` can name
participant IDs, rule IDs, and keyless-block counts; `OfferEvent::eligible`,
`assignment`, and `reserve` expose internal host IDs and travel holds; and
`SlotCheck::reasons` always reports failing participant details whether or not
`explain` was set. Serve these fields only to authenticated operators or support
tools (with a small `explain.limit`). On a public booking route, **force `explain`
unset** (ignoring any client-supplied `explain`) and project each offer to the public
fields your UI needs (such as start/end spans and `dates`). The Python package
ships that projection as `opportune.public_response` / `public_slot_check` (see
[the Python section](#engine-presets-and-sugar)) and the npm package as
`publicResponse` / `publicSlotCheck`; a route that cannot use one has to strip the
same fields itself.

`ReasonCode::ALL` is the registry, in the precedence order a participant's first
failing reason follows:

| Code | Meaning |
| --- | --- |
| `lead_time_min` | Starts before `now` plus the minimum notice. |
| `lead_time_max` | Starts past the latest start the horizon allows. |
| `outside_window` | The booking's span leaves a window that applies to it. |
| `blackout` | The booking's span overlaps a blackout. |
| `blocked` | The booking's span overlaps an event on the participant's calendar. |
| `travel` | The booking's span is clear, but its travel is not. |
| `clearance` | Too close to an existing event. |
| `rest` | Inside the rest window a late finish protects. |
| `cap_reached` | Would take a cap's bucket past its limit; `params` reports the usage. |
| `attendee_unavailable` | An attendee the booking needs is not available. |
| `term_unfillable` | No distinct hosts can fill every requirement term. |

Every reason prints as a plain English sentence for logs, so a caller can put one
in front of a person without re-deriving it from the code.

### `check_slot` and proposals

```rust
pub struct Proposal { pub events: Vec<ProposalEvent> }
pub struct ProposalEvent {
    pub part: String,
    pub start: Instant,
    pub assignment: Vec<Vec<String>>,   // one host list per requirement term
}
pub struct SlotCheck {
    pub valid: bool,
    pub reasons: Vec<Reason>,           // every failing reason, `explain` or not
    pub flags: Vec<OfferFlag>,
    pub reserve: Vec<ReserveSpan>,
    pub warnings: Vec<Diagnostic>,
    pub meta: Meta,
}
```

A proposal that does not fit the request is an `Error::MalformedProposal` rather
than an invalid slot; its stable `code()` is one of `proposal_part_unknown`,
`proposal_part_duplicated`, `proposal_part_missing`, `proposal_term_count`,
`proposal_host_count`, `proposal_host_unknown`, `proposal_host_attendee`,
`proposal_host_mismatch`, `proposal_host_duplicated` or `proposal_out_of_range`.
`proposal_out_of_range` names `/events/{index}/start`: the booking, or the travel
an assigned host or an attendee needs, reaches past `Instant::MIN` or
`Instant::MAX`, where the model has no `Span` to report. `find_availability`
withholds such a start from its offers and its exclusions instead, using the
request-wide travel envelope, so every offer it returns confirms.

`check_slot` is pure and stateless: it verifies a proposed booking against fresh
calendar and occurrence data (catching stale availability), but does not lock rows or
serialize concurrent writers. Pair it with a database transaction or unique constraint
on booked spans when committing a reservation. [The booking-commit blueprint](recipes.md#committing-a-booking-atomically)
works the transaction, ordered locking and write-back.

### Typed errors and diagnostics

```rust
pub enum Error { /* … */ }
impl Error {
    pub const fn code(&self) -> &'static str;  // stable across releases
    pub fn path(&self) -> Option<&str>;        // RFC 6901 pointer, when one field is at fault
}

pub struct Diagnostics { pub errors: Vec<Diagnostic>, pub warnings: Vec<Diagnostic> }
pub struct Diagnostic { /* … */ }
impl Diagnostic {
    pub const fn code(&self) -> &'static str;
    pub fn message(&self) -> &str;
    pub fn path(&self) -> Option<&str>;
}
```

`Error::code()` values are stable across releases:

- **Domain construction errors:** `span_empty`, `span_reversed`, `zone_period_empty`, `occurrence_without_participants`, `event_key_empty`, `status_policy_entry_empty`, `part_count`, plus civil-time, interval, window, blackout, tag, and time-zone error codes (`tzdb_not_configured`, `zone_unknown`, etc.).
- **Malformed proposal errors (`Error::MalformedProposal`):** `proposal_part_unknown`, `proposal_part_duplicated`, `proposal_part_missing`, `proposal_term_count`, `proposal_host_count`, `proposal_host_unknown`, `proposal_host_attendee`, `proposal_host_mismatch`, `proposal_host_duplicated`, and `proposal_out_of_range`.
- **Request validation (`invalid_request`):** carries every `Diagnostic` produced by `validate`. Empty IDs appear as findings inside `invalid_request` (`participant_id_empty` at `/participants/{i}/id`, `rule_id_empty` at `/rules/{i}/id`, `part_id_empty` at `/goal/parts/{i}/id`). A `selector_not_for_items` finding at the offending `when`/`count` path rejects an `ids`, `role` or `kind` clause used where only booking tags can match. Configured size and duration limits are checked first and short-circuit structural and requirement passes when exceeded.

A request that exceeds a configured cap reports the cap it exceeded: `participants_limit`,
`blocks_limit`, `zone_periods_limit`, `rules_limit`, `occurrences_limit`,
`requirement_slots_limit`, `requirement_terms_limit`, `candidates_limit`,
`search_window_limit`, `explain_limit`, `exclude_event_keys_limit`,
`part_duration_too_long`, `travel_duration_too_long`,
`block_travel_duration_too_long` and `clearance_duration_too_long`, each at the
pointer of the field that holds the excess.

The same `Diagnostic` shape carries the warnings, which never stop the search but
name a request that cannot mean what its author intended:

| Code | What it says |
| --- | --- |
| `selector_matches_no_participant` | A tag no participant carries, named in a rule's `who` or in an `at_least` term's selector, so that selector matches nobody — or, under a `not`, everybody. |
| `when_matches_no_part` | A rule's `when` selects no part of the goal. |
| `count_matches_no_part` | A `cap`'s `count` selects no part, so the cap can never be reached — usually a typo in the tag. |
| `block_missing_event_key` | A block a pooled cap counts carries no `event_key`, so two copies count twice. |
| `status_policy_ignores_resource` | A `status_policy` with an `ignore` entry reaches a resource participant, so a room's hold would read as free. |
| `rest_window_overlaps_day` | A `rest` rule whose `late_after` is at or before its `not_before`, which is the swapped-argument mistake. |
| `no_candidate_in_window` | No candidate's booking plus travel fits inside an applicable window. |
| `horizon_before_earliest_start` | A `lead_time` maximum permits no start after the minimum notice. |

### `opportune::Limits`

Configurable caps on request size: participants (`max_participants`), blocks per
participant (`max_blocks_per_participant`), zone periods per participant
(`max_zone_periods_per_participant`, 1,000 by default), rules (`max_rules`),
occurrences (`max_occurrences`), the host slots a requirement may ask for
(`max_requirement_slots`) and the terms it may name (the same
`max_requirement_slots`, counted as terms so a zero-count one cannot escape it),
the search window in days (`max_search_window_days`), candidate starts
(`max_candidate_starts`), the maximum `explain.limit` (`max_explain_limit`), and
the longest booking, travel or clearance in minutes (`max_duration_minutes`,
which also bounds a block's own `travel_before`/`travel_after`).
`Limits::default()` is the published default; a caller that needs larger inputs
supplies its own, and every one of the ten has a `with_` builder.

### `opportune::VERSION`

```rust
pub const VERSION: &str
```

The workspace version, compiled from the crate manifest. The Python module
re-exports it as `opportune.__version__`, so the two can never disagree.

### Feature flags

| Feature | Default | Effect |
| --- | --- | --- |
| `bundled-tzdb` | on | Compiles the time zone database into the crate and provides the free functions and `EngineBuilder::tzdb_bundled`. Turning it off leaves a caller-supplied database only. |

Full API documentation for the core crate is on
[docs.rs](https://docs.rs/opportune) once the crate is published. This site does
not carry rustdoc: CI builds it on every push with `RUSTDOCFLAGS="-D warnings"`,
so a broken intra-doc link or an invalid code block fails the build.

## JSON wire layer (`opportune-json`)

`opportune-json` is the wire layer every binding crosses: JSON text in, JSON text
out, and a valid document is converted into the core's domain types, so no
document can change what the engine means.

```rust
use opportune_json::{parse_request, to_json, WireResponse};

let request = parse_request(document)?;                    // JSON text -> opportune::Request
let answer = to_json(&WireResponse::from(&response))?;      // -> JSON text
```

`parse_request`, `parse_rule`, `parse_calendar`, `parse_goal`, `parse_proposal`,
`parse_ranges_query` and `parse_limits` read the documents a caller sends or
stores on its own; `to_json` writes any wire value back out deterministically,
with object maps ordered, so identical input gives identical bytes on every
platform. A service that already holds a wire value — a `WireRequest`
deserialized as a field of its own request body, say — converts it directly with
`Request::try_from(&wire_request)` and its siblings, with no JSON round trip and
the same field pointers a parse failure reports. The records a service caches
on their own convert the same way — `Participant::try_from(&wire_participant)`,
`Block::try_from(&wire_block)` and `Occurrence::try_from(&wire_occurrence)` — so a
calendar row read back from JSONB becomes the domain value without a second parse. The answer types
(`WireResponse`, `WireSlotCheck`, `WireDiagnostics`, `WireRequiredRanges`) read
back from JSON too, so a Rust HTTP client deserializes the engine's answer, and
an additive field from a newer engine does not fail the read.

### The wire format

| Value | Written as |
| --- | --- |
| instants | RFC 3339 with an explicit offset, minute-aligned: `"2027-03-09T15:00Z"` |
| durations | `"30m"`, `"1h30m"` — hours and minutes, never days |
| clock times and ranges | `"09:00"`, `"09:00-17:00"`, `"22:00-02:00"` |
| dates | `"2027-03-08"`, `"2027-03-08..2027-03-10"` |
| weekdays | `"mon-fri"`, `"mon,wed"` |
| selectors | `true`, `{"role": "host"}`, `{"kind": "person"}`, `{"tag": "interview"}`, `{"ids": ["ana"]}`, `{"not": …}`, `{"any": …}`, `{"all": …}` |

`now` rounds up to the next minute and a block's endpoints round outward; every
other instant must already be minute-aligned. Every **input** object rejects
unknown keys, and the documented defaults are applied while parsing: a participant
is a `person` and a `host`, a part's ID is `main`, a goal's display zone is its
event zone, a block is `busy` and not all-day, and optional lists default to
empty. Answer documents are the exception: the wire answer types accept additive
fields from a newer engine.

**A block's `status` is the one open string in the document.** The wire format
reads whatever string the provider sent, and any value outside `free`,
`tentative`, and `busy` defaults fail-closed to `busy`. The generated Pydantic
and TypeScript models advertise the same open string so provider vocabularies
(`workingElsewhere`, `oof`) are not rejected at the model boundary; the Pydantic
model normalizes unknown statuses to `busy` when validating a block, matching
the wire converter. An adapter maps provider-specific statuses to the three
canonical values before returning a block. A `status_policy` entry's `status`
stays a closed enum because it is caller configuration, so invalid values are
rejected immediately. An empty or whitespace-only `event_key` is also rejected
(`event_key_empty`) so unrelated blocks can never collide in cap counting.

### JSON-pointer errors

A rejected document is a `JsonError`. Its `code()` is stable
(`unknown_field`, `duration_contains_days`, `instant_not_aligned`,
`unsupported_schema_version`, …) and its `path()` is an RFC 6901 JSON pointer to
the field at fault — `"/participants/0/zone"`, `"/rules/2/params/min"` — so a
client can be told exactly where the document it sent has to change. A request
the engine's validation rejects carries every `Diagnostic` in `errors()`, so a
caller can report all of a document's problems at once rather than one per
attempt. `From<TimeError> for JsonError` keeps the time error's stable code and
message with an empty pointer, so an Axum/Tonic handler returning
`Result<_, JsonError>` can use `?` on `Instant::new`, `Date::new`,
`ZoneName::new` and the other civil-time constructors.

### Schema and upgrades

With the `schema` feature on, `schema()` returns the whole format as one Draft
2020-12 JSON Schema document whose `$defs` name and title every wire type.
`opportune_json::SCHEMA_VERSION` is the request/answer schema version this build
writes (`1` today), the same value `Response.meta.schema` reports. The Python
Pydantic models are generated (`make codegen`) from the bundle this and
`opportune-adapters` export together, so the parser and the typed models describe
one format instead of two.

Every **input** definition in the bundle is closed
(`additionalProperties: false`), because the wire reader rejects a key it does not
name. Every one of the 13 **response and output** definitions — `Response`,
`Offer`, `OfferEvent`, `ReserveSpan`, `DateCount`, `Excluded`, `Reason`,
`ReasonParams`, `Diagnostic`, `Diagnostics`, `Meta`, `SlotCheck` and
`RequiredRanges` — is left open, matching the answer types that tolerate additive
fields. The generated Python models follow that split: response models are
`extra="ignore"`, so a rolling deploy where an older client reads a newer
engine's answer does not reject a field the client has not learned yet, while
every input model stays `extra="forbid"` and refuses a key the engine does not
know.

`upgrade_rule`, `upgrade_calendar` and `upgrade_goal` take a stored document and
the schema version it was written with, validate it against that version's
meaning, and return the current version's canonical JSON. A version this build
does not read is rejected with `unsupported_schema_version` rather than
misread — see the Python section below for the Postgres-stored rule pattern.

## Provider adapters (`opportune-adapters`)

The adapters convert Microsoft Graph / Entra Outlook and Google Calendar payloads
into domain `Block`s and `Rule`s. They are a translation layer only: no network,
no filesystem, no system clock, so a recorded payload converts to the same blocks
on every platform, `wasm32` included.

| Feature | Default | What it adds |
| --- | --- | --- |
| `graph` | on | `graph::calendar_view_to_blocks`, `graph::get_schedule_to_blocks`, `graph::working_hours_to_rule` |
| `google` | on | `google::events_list_to_blocks`, `google::free_busy_to_blocks` |
| `windows-zones` | on | Resolving Windows zone names through CLDR data |
| `bundled-tzdb` | on | The `json` entry points that resolve zones against the database compiled into the binary; without it the `*_with_db` variants remain |
| `json` | off | The JSON envelopes and entry points a binding calls — `json::calendar_view_to_blocks`, `json::get_schedule_to_blocks`, `json::working_hours_to_rule`, `json::events_list_to_blocks`, `json::free_busy_to_blocks`, and the `*_with_db` twins of the zone-resolving four |
| `schema` | off | The adapter half of the JSON Schema bundle |

### The binding entry points

With the `json` feature on, each zone-resolving conversion exists twice: the short name uses
the compiled-in database (`bundled-tzdb`) and the `*_with_db` name takes a
`&TimeZoneDatabase`, exactly as the core's engine builder does:

```rust
pub fn calendar_view_to_blocks(payload_json: &str, participant_zone: &str, category_map_json: Option<&str>) -> Result<String, JsonError>;
pub fn get_schedule_to_blocks(payload_json: &str) -> Result<String, JsonError>;
pub fn working_hours_to_rule(payload_json: &str, rule_id: &str) -> Result<String, JsonError>;
pub fn events_list_to_blocks(payload_json: &str, participant_zone: &str) -> Result<String, JsonError>;
pub fn free_busy_to_blocks(payload_json: &str) -> Result<String, JsonError>;
```

The `*_with_db` forms are the same functions with that first argument supplied
by the caller; the Python, TypeScript and WASM bindings call them with their own
engine's database. `free_busy_to_blocks` is the exception: free/busy ranges carry
no zone names, so it needs no time zone database and has no `*_with_db` twin.

### The outcome contract

Every conversion answers one `CalendarOutcome` per calendar: `Available` with
`AnnotatedBlock`s (each block next to the provider's event ID) and per-event
`AdapterDiagnostic`s, or `Unavailable { code, message }`. An errored calendar is
never an empty block list, because an empty list reads as "completely free"; both
Graph (`scheduleInformation.error`) and Google (`calendars[id].errors`) report
per-calendar failures inside otherwise successful responses, and the adapters
check for them.

Free/busy sources carry less: Graph `getSchedule` items and Google free/busy
ranges have no event IDs, categories or all-day flag, so their blocks have no
event key and are never all-day. Rest rules and category-based caps therefore
need an event source (`calendarView`, `events.list`).

A per-event diagnostic never fails a calendar, and its code is stable. A Graph
category the caller's `categoryMap` does not name and whose text has no ASCII slug
— a non-Latin name such as `会議`, or `Встреча` — makes no `category/<slug>` tag
and emits `category_tag_unmappable`, naming the event and the category, so a
`category/...` policy can be seen not to match it. At most 16 such findings are
reported per event, with one summary diagnostic naming how many more were omitted,
so a hostile payload cannot flood the findings a caller carries.

### Markers

One convention on both providers: `opportune.tags` (space-separated tags) and
`opportune.event_key`. Google reads them from `extendedProperties.shared` and
`private`; Graph from single-value extended properties in opportune's own
property set, which the query must `$expand`:

```rust
use opportune_adapters::{MARKER_EVENT_KEY, MARKER_TAGS};
use opportune_adapters::graph::{
    EXTENDED_PROPERTY_EVENT_KEY_ID, EXTENDED_PROPERTY_EXPAND, EXTENDED_PROPERTY_NAMESPACE_GUID,
    EXTENDED_PROPERTY_TAGS_ID,
};
```

### Caller requirements

- **Graph:** read `calendarView`, not `/events` — an unexpanded series is an
  `unexpanded_series` outcome. Follow `@odata.nextLink` until it is absent and
  pass the concatenated `value` array as one payload: a single page is
  `payload_invalid`, because the events after it would read as free time. Send
  `Prefer: outlook.timezone="UTC"`. `$expand` the markers with
  `EXTENDED_PROPERTY_EXPAND`. Chunk `getSchedule` to at most 20 schedules and 62
  days per call. An occurrence or exception of a recurring series derives its
  event key from the shared `iCalUId` or marker plus its `originalStart` slot
  (`<key>@<utc>`), so a whole series is not counted as one event. When
  `getSchedule` omits `scheduleItems`, an all-free `availabilityView` is a free
  mailbox; busy slots with no items make that calendar `payload_invalid` instead
  of silently empty.
- **Google:** query `events.list` with `singleEvents=true`; never set
  `maxAttendees` (Google may then omit the user's own attendee entry, which is
  what `self` status and `attendeesOmitted` are read from); batch
  `freeBusy.query` within Google's per-call calendar limit, and follow
  `nextPageToken` for `events.list` before converting (a page is
  `payload_invalid`).
- **Both:** pass the participant's IANA zone — all-day events resolve in it.
  Graph additionally takes an optional category-to-tag map, since Graph events
  carry categories; an unmapped category becomes `category/<slug>`, and one whose
  name has no ASCII slug is dropped with a non-fatal `category_tag_unmappable`
  diagnostic (see [the outcome contract](#the-outcome-contract)). Google events
  carry no categories: their tags come from `eventType` and from the marker
  properties below, so `events_list_to_blocks` takes no category map.

Graph working hours become a `window` rule through
`graph::working_hours_to_rule`. A custom Graph time zone
(`#microsoft.graph.customTimeZone`) is a `WorkingHoursError::CustomTimeZone`, so
the caller falls back to its own window while the blocks from the same payload
are unaffected. Block conversion and working-hours conversion are separate
functions for exactly that reason.

## Python

### Installation

Install the `cp312-abi3` wheel (Python 3.12+; one wheel per platform bundles the
compiled Rust engine, generated Pydantic models, and provider adapters, with
`pydantic>=2.14` as the sole runtime dependency):

```sh
pip install opportune
# or: uv add opportune
```

To build from a repository checkout instead:

```console
make install   # create the project .venv and build the extension
make wheel     # build the sdist and wheel into dist/
```

The package ships `py.typed`, `_opportune.pyi`, and `_models.py`, so type
checkers inspect the full surface without importing the compiled binary.
`opportune.__version__` always matches `opportune::VERSION`.

### Quickstart: one person, one meeting

<!-- snippet: api-python-quickstart -->
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
print(opportune.offer_start(response.offers[0]))  # 2027-06-14T11:00Z: four hours of notice
print(opportune.offer_end(response.offers[0]))    # 2027-06-14T11:30Z
```

`calendar=[]` rather than an omitted calendar is deliberate: an absent calendar
means "unknown", and every offer resting on that host carries the
`calendar_unknown` flag. A host known to be free has an empty one.

### Engine, presets and sugar

```text
engine = opportune.Engine(tzdb_dir=None, tzdb_version=None, limits=None)
engine.find_availability(request) -> opportune.Response
engine.check_slot(request, proposal) -> opportune.SlotCheck
engine.validate(request) -> opportune.Diagnostics
engine.required_ranges(query: RangesQuery | Request) -> opportune.RequiredRanges
engine.find_availability_json(request) -> str
engine.check_slot_json(request, proposal) -> str
engine.validate_json(request) -> str
engine.required_ranges_json(query: RangesQuery | Request) -> str
engine.find_availability_async(request) -> opportune.Response
engine.check_slot_async(request, proposal) -> opportune.SlotCheck
engine.validate_async(request) -> opportune.Diagnostics
engine.required_ranges_async(query: RangesQuery | Request) -> opportune.RequiredRanges
engine.find_availability_json_async(request) -> str
engine.check_slot_json_async(request, proposal) -> str
engine.validate_json_async(request) -> str
engine.required_ranges_json_async(query: RangesQuery | Request) -> str
engine.tzdb_version -> str

# module-level forms, over one lazily built default engine
opportune.find_availability(request) -> opportune.Response
opportune.check_slot(request, proposal) -> opportune.SlotCheck
opportune.validate(request) -> opportune.Diagnostics
opportune.required_ranges(query: RangesQuery | Request) -> opportune.RequiredRanges
opportune.find_availability_json(request) -> str
opportune.check_slot_json(request, proposal) -> str
opportune.validate_json(request) -> str
opportune.required_ranges_json(query: RangesQuery | Request) -> str
opportune.find_availability_async(request) -> opportune.Response
opportune.check_slot_async(request, proposal) -> opportune.SlotCheck
opportune.validate_async(request) -> opportune.Diagnostics
opportune.required_ranges_async(query: RangesQuery | Request) -> opportune.RequiredRanges
opportune.find_availability_json_async(request) -> str
opportune.check_slot_json_async(request, proposal) -> str
opportune.validate_json_async(request) -> str
opportune.required_ranges_json_async(query: RangesQuery | Request) -> str
```

Build one engine and reuse it. The native engine is frozen and every call
releases the GIL around its Rust work, so threads sharing one instance run in
parallel rather than taking turns. The module-level `find_availability`,
`check_slot`, `validate` and `required_ranges` answer from one lazily built
default engine, and each has module-level `*_json` and `*_async` forms; the
`*_json_async` form is the document-returning call awaited off the loop.

`required_ranges` accepts a `RangesQuery`, a whole `Request`, or the mapping
either is read from, in Python as it does in TypeScript. A request is projected
onto the query's four fields, so the six request-only fields (`occurrences`,
`exclude_event_keys`, `calendars`, `weights`, `limit`, `explain`) are dropped
after validation rather than read by the fetch planner.

`Limits()` is valid with every field omitted: an omitted or `null` field means
the engine default, so `Engine(limits=Limits(max_participants=50))` overrides one
cap without restating the other nine. Every wire integer — `int32`, `uint32` and
`uint64`, including each `Request.weights` value — publishes its exact inclusive
`minimum`/`maximum` in the JSON Schema and enforces it during Pydantic
validation, before the document reaches the native reader.

The `*_async` methods await `asyncio.to_thread` around their synchronous twin, so
an `async def` route awaits one instead of holding the event loop for the search.
Every `Engine.*_async` method — and every module-level `*_async` and
`*_json_async` function — also accepts an optional keyword-only
`executor: concurrent.futures.Executor | None = None`. `None` (`asyncio.to_thread`)
uses the loop's shared default pool; pass a dedicated bounded
`ThreadPoolExecutor` to keep CPU-bound searches off that shared pool, so they
cannot queue behind another library's blocking work or starve it. The synchronous
methods remain the right call from a worker thread or a script.

Each query has a `*_json` form, on the engine and at module level, that returns
the engine's own document as text. Both forms hand the request to the engine as
JSON text; the typed form additionally parses the answer back into a model. The
GIL is released around the engine call itself, so a route that only forwards the
document can skip the parse and hand the engine's own bytes on; one that inspects
the answer uses the typed form. Both are CPU-bound in the middle, so an `async
def` route awaits the matching `*_async` entry point rather than calling either
one directly.

Requests and answers are the generated Pydantic v2 models, so a FastAPI route
declares them directly and a caller stores them with
`model_dump_json(by_alias=True, exclude_none=True)` — the wire spelling, ready for
Postgres `jsonb` — and reads them back with `Model.model_validate_json`. A dump
carries the same values the Rust serializer writes, in the schema's field order
rather than the serializer's declaration order, so compare a Python dump against
Rust's bytes by value and not by digest.

The limits and the models have validated helpers, so an application never
restates or hand-patches one:

<!-- snippet: api-python-limits -->
```python
import opportune

limits = opportune.default_limits()            # every field, read from Rust Limits::default()
limits = opportune.limits_with(max_search_window_days=120)
limits = opportune.Limits(max_participants=50) # omitted fields take the engine defaults
engine = opportune.Engine(limits=limits)

rule = opportune.presets.business_hours()
rule = opportune.with_updates(rule, id="ana_hours", who={"ids": ["ana"]})
```

`default_limits()` is the starting point the engine itself uses, so an override
changes one field rather than restating all ten. `Limits(...)` lets you omit every
field you do not need, and each omitted or `null` field reads as the engine
default. `limits_with` and `with_updates` validate the merged document:
`model_copy(update=...)` skips validation, so a field set through it can hold a
value the model would never have accepted. `with_updates` takes a field by either
spelling — `Meta`'s wire key is `schema`, its field name `schema_` — and replaces
the field rather than leaving both spellings in the document it validates, and
both helpers raise `pydantic.ValidationError` for a value the model rejects.

`opportune.presets` builds common rules as generated Pydantic models:

- **Available presets:** `business_hours`, `dated_hours`, `weekly_hours` (one rule for multi-range weeks and holiday overrides), `buffers` (supports `min_gap`), `min_notice`, `horizon` (accepts a duration string, date string, calendar-day count, or typed `LeadTimeMax` such as `WorkingDaysMax(calendar=..., working_days=...)`), `rest`, `cap_per_day`, `cap_per_week`, `cap_per_month`, `status_policy`, and `available_only` (whose `who` is the attendee and whose rule ID is `available_only:{id}`; spans outside `1975-01-01T00:00Z..=2100-01-01T00:00Z` and any non-whole-minute RFC 3339 instant are rejected immediately with `invalid_argument`).
- **Status policies (`presets.status_policy`):** Accepts generated `StatusPolicyEntry` models or their wire mappings. Entries are tried across applicable policies in request order, so put the most specific policy first; an unscoped `ignore` entry reaches resource rooms unless `who` is scoped with `{"kind": "person"}`.
- **Selectors (`opportune.SelectorInput`):** Every preset except `available_only` accepts `who` and `when` as a generated selector model, a wire mapping (`{"ids": ["ana"]}`, `{"kind": "person"}`), or `True`. Use `KindSelector(kind="person")` / `{"kind": "person"}` to keep people-only rules off `Participant(kind="resource")` conference rooms.
- **Loosening tiers (`opportune.unless` / `opportune.presets.unless`):** Returns a rule with a selector excluded from `when` so a narrower tier can loosen a broader rule instead of intersecting to empty. Passing `True` excludes every booking, causing `validate` to report `when_matches_no_part`.
- **Human-only hosts (`opportune.presets.and_who` / `except_who`):** The participant-side twins of `unless`, also exported as `opportune.and_who` / `opportune.except_who`. They narrow `who` by intersection rather than replacing it, so `presets.and_who(presets.business_hours(), {"kind": "person"})` keeps the default `{"role": "host"}` clause beside the kind clause and reaches human hosts alone — not attendees, not resource rooms. `presets.except_who(presets.cap_per_day({"tag": "interview"}, 2), {"ids": ["ceo"]})` exempts the CEO while every other host stays subject. Passing `who={"kind": "person"}` to a builder instead replaces the default outright and also matches attendees.
- **Rescheduling:** Pass `Request(exclude_event_keys=[...])` to ignore the event being moved during blocking, rest, and cap evaluation.
- **Shared events (`Request(occurrences=[...])`):** Build `Occurrence(event_key=..., span=..., participants=[...])` once for a multi-participant event so a pooled cap counts it once; `Occurrence.tags` (or `with_updates(occurrence, tags=[...])`) is what a cap's `count` selector sees.
- **Public route projections:** `opportune.public_response(response: Response | Mapping[str, object])` and `opportune.public_slot_check(check: SlotCheck | Mapping[str, object])` return `PublicResponse` / `PublicSlotCheck`, dropping operator-only fields (`warnings`, `excluded`, `excluded_truncated`, each offer's `eligible`/`assignment`/`reserve`, and a slot check's `reasons`, `reserve`, and `warnings`). Each helper reads the model or the document it was stored or received as — a mapping read back from a cache, queue, or JSON column — and rejects a malformed document with `OpportuneError` code `invalid_argument` naming the field, never a raw `AttributeError` or `pydantic.ValidationError`. The full `Response` and `SlotCheck` stay unchanged for operator routes; public booking routes must explicitly project through `public_response` / `public_slot_check` to avoid leaking internal host IDs, rules, or calendar details.
- **Single-part sugar (`opportune.sugar`):** `single_goal` (supports `display_zone`, `grid`, `candidates`, `part_id`, `tags`, and `travel`), `person`, `at_least`, and `offer_start`, `offer_end`, `offer_assigned_hosts`, `offer_calendar_unknown`, `offer_reserve`, and `offer_to_proposal` (accepting either an `Offer` model or a raw dict). Sugar builds the standard models with no parallel semantics.

### FastAPI: a lifespan engine and the async entry points

The engine's work is CPU-bound Rust rather than I/O, so an `async def` route that
calls it directly holds the event loop for the whole search. Build the engine once
in the lifespan handler, keep it on `app.state`, and await the engine's own
`*_async` entry point, which runs the call on a worker thread while the loop keeps
serving:

<!-- snippet: api-python-fastapi-lifespan -->
```python
from collections.abc import AsyncGenerator
from contextlib import asynccontextmanager

import opportune
from fastapi import FastAPI, Request, Response


@asynccontextmanager
async def lifespan(app: FastAPI) -> AsyncGenerator[None]:
    app.state.engine = opportune.Engine()
    yield


app = FastAPI(lifespan=lifespan)


@app.post("/availability", response_model_exclude_none=True)
async def availability(request: opportune.Request, http: Request) -> opportune.Response:
    engine: opportune.Engine = http.app.state.engine
    return await engine.find_availability_async(request)


@app.post("/availability/raw")
async def availability_raw(request: opportune.Request, http: Request) -> Response:
    engine: opportune.Engine = http.app.state.engine
    return Response(
        await engine.find_availability_json_async(request),
        media_type="application/json",
    )
```

A service that knows its own load passes an executor rather than sharing the
loop's default pool: build a bounded `ThreadPoolExecutor` once beside the engine
and `await engine.find_availability_async(request, executor=pool)`. Every
`*_async` and `*_json_async` entry point accepts it.

`response_model_exclude_none=True` is what keeps availability, slot-check and
validation routes byte-identical with their `*_json` twins. The engine's serializer
omits an absent optional (`event_key`, `params`, `rule`, `participant` on an excluded
reason); a typed route that returns the model writes `null` for it unless FastAPI is
told to omit `None`. `RequiredRanges.occurrences` follows the same rule: the
engine omits it when no cap applies, so a typed `/required-ranges` route with
`response_model_exclude_none=True` matches `required_ranges_json` byte-for-byte
with no special case — including on the days before a cap exists. The second route
hands the engine's own document on, so the body is exactly the bytes the engine
wrote — no Pydantic parse on the way out. It is the route to use when the caller
forwards the answer untouched; a route that reads fields from it uses the typed
one.

### FastAPI: reject oversized bodies before they are parsed

The engine's `Limits` bound a *parsed* request: participants, blocks, rules, the
search window. They cannot bound the work FastAPI does first, because
`await request.json()` has already read, buffered and decoded the whole body
before a `Request` model exists. A 200 MB body of unknown keys is rejected by the
engine's schema validation only after the server has paid for it. Cap the body at
the boundary instead:

- **At the proxy**, with nginx's `client_max_body_size` or Apache's
  `LimitRequestBody`. This is the only place a chunked body — which carries no
  `Content-Length` — is stopped before the application reads it.
- **In middleware**, as a cheap second line, rejecting a declared
  `Content-Length` above the cap with `413` before the route runs. A
  `Content-Length` check alone is not sufficient, because the header is optional
  and a client can lie; the proxy's streaming limit is what makes the cap real.

A request document is small: a realistic one is a few kilobytes, and the limit
that matters is hundreds of kilobytes, not megabytes. Pair the cap with the
engine's own `max_participants`, `max_blocks_per_participant` and `max_rules` so a
body that fits the cap still cannot ask for unbounded work.

### Stored rules: upgrades from Postgres

Rules a caller stores carry the schema version they were written with, and
`upgrade_rule`, `upgrade_calendar` and `upgrade_goal` rewrite them for this build.
They take and return JSON text, because the document is what was stored:

<!-- snippet: api-python-stored-rules -->
```python
import json
from typing import Any, cast

from pydantic import TypeAdapter

import opportune


db: Any = ...  # the application's asyncpg pool


async def load_rule(rule_id: str) -> dict[str, object]:
    row = await db.fetchrow(
        "SELECT document, schema_version FROM rules WHERE id = $1", rule_id
    )
    # Validates against the version the rule was written with and returns the
    # current version's canonical JSON; an unreadable version raises
    # OpportuneError with code `unsupported_schema_version`.
    upgraded = opportune.upgrade_rule(row["document"], row["schema_version"])
    TypeAdapter(opportune.Rule).validate_json(upgraded)
    return cast(dict[str, object], json.loads(upgraded))
```

Run the upgrade in the migration that reads the row, write the canonical form and
the new version back, and the row stops needing upgrading.

A `TypeAdapter` failure carries no HTTP body, so pass the document the row holds when
you build the envelope — `opportune.error_envelope_from_validation_error(refused,
document=json.loads(upgraded))` — and its findings' pointers resolve in that
document, exactly as a body's do. That is what reads a stored document as *itself*:
the package declares `Goal`, `Part` and `Requirement` as classes, and without the
document a key they spell like one of the unions' form tags (`Requirement`'s `all`)
is read as the tag and dropped.

### Calendar adapter helpers

`opportune.adapters.graph` and `opportune.adapters.google` take a payload as
`str`, `bytes`, or the `dict` an `httpx2.Response.json()` returns, and answer the
generated models:

<!-- snippet: api-python-adapter-helper -->
```python
import opportune
from opportune.adapters import graph

# A recorded `calendarView` response; a str, bytes or parsed dict all work.
outcome = graph.calendar_view_to_blocks(
    calendar_view_payload, "America/New_York", {"Interview": "interview"}
)
if isinstance(outcome, opportune.AvailableCalendarOutcome):
    participant = opportune.Participant(
        id="ana", zone="America/New_York", calendar=[entry.block for entry in outcome.blocks]
    )
```

`graph.get_schedule_to_blocks` and `google.free_busy_to_blocks` answer a dict of
outcomes keyed by schedule or calendar ID, `graph.working_hours_to_rule` answers a
`GraphWorkingHoursOutcome`, and `google.events_list_to_blocks` converts an
`events.list` response. All of them default to the shared engine and accept
`engine=` to use a caller's own.

The same five conversions are also methods on `Engine` —
`calendar_view_to_blocks`, `get_schedule_to_blocks`, `working_hours_to_rule`,
`events_list_to_blocks`, and `free_busy_to_blocks` — with the same parameters
(minus `engine=`), so `who=` and `when=` work identically either way. Prefer the
`Engine` methods when payloads and requests must resolve against the same time
zone database. The marker constants are `opportune.MARKER_TAGS` and
`opportune.MARKER_EVENT_KEY`; Graph extended-property constants live on
`opportune.adapters.graph` (`EXTENDED_PROPERTY_NAMESPACE_GUID`,
`EXTENDED_PROPERTY_TAGS_ID`, `EXTENDED_PROPERTY_EVENT_KEY_ID`,
`EXTENDED_PROPERTY_EXPAND`), and `opportune.adapters.google` re-exports both
marker names.

An `UnavailableCalendarOutcome` is never turned into an empty calendar: an empty
block list would read as "completely free", so a caller passes that host with no
calendar (its offers carry `calendar_unknown`) or leaves it out.

### Errors

Every engine, adapter, and preset rejection crosses the package boundary as
`opportune.OpportuneError` (direct Pydantic model construction, `with_updates`, and
`limits_with` raise `pydantic.ValidationError`, which `error_envelope_from_validation_error`
also translates): `.code` is the stable code, `.message` names the value that was
rejected, `.path` is the RFC 6901 JSON pointer of the field at fault (or `None`),
`.errors` holds the findings as generated `Diagnostic` models, and the native cause
stays chained as `__cause__`.

`opportune.ErrorEnvelope` provides a single 422 response body for both engine
rejections (`OpportuneError`) and FastAPI/Pydantic request validation failures
(`RequestValidationError`):

- Register both exception handlers below and declare `responses={422: {"model": opportune.ErrorEnvelope}}` on your routes so OpenAPI matches runtime responses.
- `opportune.http_status_for_error(error)` returns `422` when the failure is a caller-fixable document the engine or wire parser refused (`invalid_request`, `invalid_json`, and the wire/validation codes), and `500` when it is a server, deployment, integration or package-argument failure (`invalid_limits`, `payload_invalid`, `invalid_argument`, `argument_not_utf8`, `unsupported_schema_version`, `serialization_failed`, and the `tzdb_*` codes). Internal deployment failures are never surfaced as 422 client errors.
- Classification is by error code alone; if a route passes caller-supplied values directly into `upgrade_*`, presets, adapters, or a helper that raises `invalid_argument`, override that code (or `unsupported_schema_version` / `argument_not_utf8`) as appropriate for the endpoint, because the package classifies them as the server's own failures by default.

<!-- snippet: api-python-error-handlers -->
```python
# Register these handlers on the application from the lifespan section above.
# This block creates a fresh app so its registrations run standalone. Declare
# `responses={422: {"model": opportune.ErrorEnvelope}}` on the routes so the
# served OpenAPI document describes this body.
import opportune
from fastapi import FastAPI, Request
from fastapi.exceptions import RequestValidationError
from fastapi.responses import JSONResponse

app = FastAPI()


def envelope(answer: opportune.ErrorEnvelope) -> JSONResponse:
    return JSONResponse(status_code=422, content=answer.model_dump())


@app.exception_handler(opportune.OpportuneError)
async def rejected(_: Request, error: opportune.OpportuneError) -> JSONResponse:
    if opportune.http_status_for_error(error) == 422:
        return envelope(opportune.error_envelope_from_opportune_error(error))
    # The server's own failure — an unusable database, a stored document this build
    # cannot read — says nothing of its message or path to a client.
    return JSONResponse(
        status_code=500,
        content={"code": "internal_error", "message": "the request could not be served"},
    )


@app.exception_handler(RequestValidationError)
async def unreadable(_: Request, error: RequestValidationError) -> JSONResponse:
    return envelope(opportune.error_envelope_from_validation_error(error))
```

The envelope carries `code`, `message`, `path` and `errors`; each finding has its
own pointer. `error_envelope_from_validation_error` turns Pydantic's `loc` tuples
into the same RFC 6901 pointers the engine reports, so one resolver finds the
field a client has to change whichever layer rejected the request. A body's
pointer drops the parameter name FastAPI's own `body` location states and the tag
each union reports for the arm it picked, while a parameter outside the body keeps
the `query` or `path` segment naming where it arrived. The body need not be a
`Request`: a route that declares a `Rule`, `Selector`, `Term` or `LeadTimeMax`
directly — an "is this stored rule acceptable?" endpoint — is served pointers into
that document, a route taking two body parameters keeps each parameter's name
above the document it holds, and a body of the application's own model keeps the
keys it wrote. A body that is not JSON at all names no field: its finding carries
no path, as the engine's own `invalid_json` rejection does. `Selector` dispatch
reports a misspelled selector key (`{"who": {"idz": ["a"]}}`) as **one** finding at
that key rather than one per union arm, so the envelope stays readable.

### Measuring a Python call

`crates/opportune-py/tests/bench_e2e.py` benchmarks realistic multi-host
workloads: 50 participants, 14 days, and a 5-minute grid across both a
single-term pool and overlapping pools. It prints median and p95 timings split
across Pydantic serialization, the Rust call, and Pydantic deserialization,
alongside the public end-to-end call:

```console
uv run --locked --project crates/opportune-py python crates/opportune-py/tests/bench_e2e.py
```

The test suite runs the same workloads to check the answers and asserts nothing
about the clock, because a shared runner cannot promise a latency.

## Documentation sources

- This site: built with mkdocs from `docs/` on every push to `main`.
- Rust API docs: generated by `cargo doc` from every crate's `//!` and `///`
  comments — the crate-level docs are the API contract, and undocumented public
  items fail the lint gate (`missing_docs = "deny"`).
