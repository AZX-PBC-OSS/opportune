# WASM and TypeScript

`opportune` is also published as an npm package for Node 24+, Bun 1.4.2+, Cloudflare Workers (`workerd`), browsers, and generic Web Workers: the same Rust engine compiled to `wasm32-unknown-unknown` with `wasm-bindgen` and wrapped in a strict TypeScript API. The IANA time zone database is bundled inside the module, so calls need no native toolchain and no `TZif` files on disk.

The binding crosses into Rust via the JSON wire format. Request and response documents match the Rust and Python surfaces byte-for-byte, verified against the committed golden corpus in `crates/opportune-testkit/goldens/`. TypeScript definitions are generated directly from the exported JSON Schema (`make codegen-ts`), keeping types in sync with the wire format. The same Draft 2020-12 bundle ships with the package as `opportune/schema.json`, exported from `package.json`, so a consumer that validates a request before sending it — with Ajv, or a provider's own validation — reads the document the engine validates against. `make check-codegen-ts` fails if the shipped file drifts from `export_schema`.

Integer `u64` wire fields are parsed as `u64` on every target, saturating into `usize::MAX` on `wasm32` rather than failing to parse — so large `limit` or `explain.limit` values behave identically on 32-bit WebAssembly and 64-bit native hosts. `max_search_window_days` is a `u32` in the domain and rejects out-of-range values on all targets. Every integer field — `int32`, `uint32` and `uint64` alike — publishes its exact inclusive `minimum`/`maximum` in the JSON Schema the package reads, so a value outside it is an `OpportuneError` rather than a wrapped or silently clamped number.

## Installation

Install the package from npm:

```sh
npm install opportune
```

Requires Node 24 or newer, or Bun 1.4.2 or newer, as `engines` in the manifest states. Cloudflare Workers (`workerd`), browsers, and generic Web Workers are also supported through `package.json#imports["#wasm"]` and `opportune/web`, described below. Node 24 is the oldest release line supporting `using` and `Symbol.dispose`; CI tests the packed package on Node 24, Node 26, Bun 1.4.2, Cloudflare `workerd` (via `@cloudflare/vitest-plugin`), and Chromium/Firefox/WebKit plus a real module Web Worker (via Vitest Browser Mode + Playwright).

### Building from source

To build and pack the package from a repository checkout (the first four commands run from the repo root; the last two write outside it to avoid adding an untracked `node_modules/` or root `package-lock.json`):

```sh
wasm-pack build crates/opportune-wasm --target web --out-dir js/pkg --release
wasm-pack build crates/opportune-wasm --target nodejs --out-dir js/pkg-node --release
npm --prefix crates/opportune-wasm/js ci --ignore-scripts
npm --prefix crates/opportune-wasm/js run build
npm pack crates/opportune-wasm/js --pack-destination /tmp --ignore-scripts
npm install --ignore-scripts --prefix /tmp/opportune-consumer /tmp/opportune-*.tgz
```

One crate is built for two targets — `wasm-pack` emits byte-identical `.wasm` files, and the package publishes **one** `.wasm` binary with both generated glues (`opportune_wasm.cjs` for Node/Bun and `opportune_wasm.js` for Workers/browsers) so each runtime uses `wasm-bindgen`'s own loader without duplicating the binary. `make wasm` runs the first four steps.

### Server runtimes, Cloudflare Workers, Next.js, and browsers

- **Node 24+ and Bun 1.4.2+ (`node`, `default`):** the `#wasm` loader lazily requires the CommonJS glue on the first engine call, keeping `new Engine()` and `require("opportune")` 100% synchronous with zero top-level `await`.
- **Cloudflare Workers (`workerd`):** a Worker has no filesystem and `workerd` refuses runtime `WebAssembly.compile` / `new WebAssembly.Module(bytes)`. Under the `workerd` condition, `#wasm` statically imports `./pkg/opportune_wasm_bg.wasm` (precompiled at bundle time via a `CompiledWasm` rule in `wrangler.jsonc`) and instantiates it lazily via `initSync({ module })` on the first engine call. Build one module-scoped `Engine` per isolate and reuse it across requests. Verified in real `workerd` via `make test-wasm-workerd`.
- **Browsers and generic Web Workers (`browser`, `worker`):** call `await init(wasmUrl)` (or `initSync({ module })`) from `opportune/web` once before calling the engine, passing the `.wasm` asset URL (exported as `opportune/opportune_wasm_bg.wasm`, e.g., `import wasmUrl from "opportune/opportune_wasm_bg.wasm?url"` in Vite). Calling the engine before initialization throws `OpportuneError` (`code: "wasm_unavailable"`). Concurrent `init()` calls share one load, failures reset cleanly for retry, and repeat calls after initialization are no-ops. Verified in Chromium, Firefox, WebKit, and a real module Web Worker via `make test-wasm-web`.
- **Next.js App Router:** keep `opportune` on the Node.js runtime (not legacy Edge) and mark it external in `next.config.mjs`:

```js
// next.config.mjs
export default { serverExternalPackages: ["opportune"] };
```

- **Pure builders with zero WebAssembly (`opportune/presets`):** if a bundle only needs rule presets, `offer*` accessors, `publicResponse`/`publicSlotCheck`, `httpStatusForError`, `isKnownStatus`, marker constants, `OpportuneError`, or generated TypeScript types, import `opportune/presets`. It loads no WebAssembly and no `node:` modules.
- **Not claimed:** Deno and deprecated Next.js Edge (`edge-light`) are untested and not claimed.

## Quickstart: one person, one meeting

<!-- snippet: wasm-ts-quickstart -->
```ts
import {
  Engine, businessHours, buffers, minNotice, offerStart, person, singleGoal,
} from "opportune";

const request = {
  now: "2027-06-14T07:00Z",
  goal: singleGoal("30m", { all: [person("rich")] }, {
    zone: "Europe/London",
    searchWindow: ["2027-06-13T23:00Z", "2027-06-18T23:00Z"],
  }),
  participants: [{ id: "rich", zone: "Europe/London", calendar: [] }],
  rules: [businessHours(), buffers(), minNotice()],
};

using engine = new Engine();
// The answer twice: as the generated document types, and as the JSON text the
// module already wrote.
const response = engine.findAvailability(request);      // Response
const text = engine.findAvailabilityJson(request);      // the same answer, as JSON
const [earliest] = response.offers;
if (earliest === undefined) throw new Error("the request offers no start");
const start = offerStart(earliest);
if (start !== "2027-06-14T11:00Z") throw new Error(`unexpected earliest start: ${start}`);
console.log(start);                  // the earliest start, from the typed answer
console.log(text);                   // and the same answer as the engine's own bytes
```

`using` releases the engine's WASM handle when the block exits; without `using`, call `engine.free()` manually. **Keep `using` inside a function or request handler** — at ES module top level, `using` disposes as soon as module evaluation finishes, before any exported route handler runs (subsequent calls throw `engine_disposed`). For a long-lived module-scope instance (for example in a Next.js App Router route), declare `const engine = new Engine()` and reuse it across requests, or use the free functions (`findAvailability`, `checkSlot`, `validate`, `requiredRanges`), which share a single lazy engine automatically.

## Requests, answers, and errors

Every query exists in both typed and raw-JSON forms: `findAvailability` / `findAvailabilityJson`, `checkSlot` / `checkSlotJson`, `validate` / `validateJson`, and `requiredRanges` / `requiredRangesJson` (on `Engine` and as module-level free functions). Routes forwarding JSON responses directly should use the `*Json` variant to avoid parsing and re-serializing the output.

`requiredRanges` accepts either a `RangesQuery` or a full `Request`, projecting away the six fields not needed for fetch-range planning (`occurrences`, `exclude_event_keys`, `calendars`, `weights`, `limit`, and `explain`) while still validating unknown keys.

Every failure throws `OpportuneError` carrying `code`, `message`, RFC 6901 `path`, and per-field `errors`:

<!-- snippet: wasm-ts-errors -->
```ts
import { Engine, OpportuneError, person, singleGoal } from "opportune";

using engine = new Engine();
try {
  // A zone the engine cannot resolve: a structured rejection, never a TypeError.
  engine.findAvailability({
    now: "2027-06-14T07:00Z",
    goal: singleGoal("30m", { all: [person("rich")] }, {
      zone: "Mars/Olympus",
      searchWindow: ["2027-06-14T00:00Z", "2027-06-15T00:00Z"],
    }),
    participants: [{ id: "rich", zone: "Europe/London", calendar: [] }],
    rules: [],
  });
  throw new Error("the invalid zone was accepted");
} catch (thrown) {
  if (!(thrown instanceof OpportuneError)) throw thrown;
  if (thrown.code !== "invalid_request") throw thrown;
  if (!thrown.errors.some((finding) => finding.code === "zone_unknown")) {
    throw new Error("the rejection does not name the unknown zone");
  }
  console.error(thrown.code, thrown.path, thrown.errors);
}
```

Unpaired UTF-16 surrogates, circular objects, unknown zones, and invalid rules all throw `OpportuneError`, so handlers never encounter bare `TypeError`s.

### HTTP status, open unions, and known statuses

`httpStatusForError(error)` is the one mapping a server needs: it returns `422`
for a caller-fixable document the engine or wire parser refused
(`invalid_request`, `invalid_json`, wire codes), and `500` for a server,
deployment, integration or package-argument failure (`invalid_limits`,
`payload_invalid`, `invalid_argument`, `argument_not_utf8`,
`unsupported_schema_version`, `serialization_failed`, and the `tzdb_*` / WASM
codes). Express, Hono and Next.js all answer the same pair:

<!-- snippet: wasm-ts-http-errors -->
```ts
import { OpportuneError, httpStatusForError } from "opportune";

function refusal(thrown: unknown): {
  status: 422 | 500;
  body: { code: string; message: string };
} {
  if (thrown instanceof OpportuneError) {
    const status = httpStatusForError(thrown);
    return {
      status,
      body: status === 422
        ? { code: thrown.code, message: thrown.message }
        : { code: "internal_error", message: "the request could not be served" },
    };
  }
  return { status: 500, body: { code: "internal_error", message: "the request could not be served" } };
}

const rejected = refusal(new OpportuneError("invalid_json", "the request is not JSON"));
if (rejected.status !== 422 || rejected.body.code !== "invalid_json") {
  throw new Error("a malformed document was not answered 422");
}
console.log(rejected);

// Express:  app.use((error, _req, res, _next) => { const r = refusal(error); res.status(r.status).json(r.body); })
// Hono:     app.onError((error, c) => { const r = refusal(error); return c.json(r.body, r.status); })
// Next.js:  return NextResponse.json(r.body, { status: r.status })
```

`new Engine(limits)` throws `OpportuneError` with code `invalid_limits` when the
limits document is not one the engine reads; it is classified `500` and chains the
underlying rejection as `.cause`. `undefined` and `null` both mean "use the
defaults", so `new Engine(null)` is valid.

Three unions are deliberately open so a newer engine's value still assigns:

- `BlockStatus` is the three statuses the engine interprets (`free`, `tentative`,
  `busy`) plus any provider string. The wire reads an unknown status as `busy`, so
  use `isKnownStatus(value: unknown)` to narrow to the three before acting on
  `free`.
- `OfferFlag` is `"calendar_unknown"` plus a future flag.
- `OpportuneErrorCode` lists today's codes plus any later one; match on `code`,
  never on `message`.

On generated request and limits documents, `null` for an optional field means
"absent": the wire applies the documented default, and omission or `undefined`
behaves the same. That includes an optional field a builder copies into the
document, such as `displayZone` or `travel`. An options object itself must still
be an object — `null` as the third argument to a builder is `invalid_type`, not
"no options".

### Scaling a synchronous engine

The JavaScript engine call is synchronous. That is fine for ordinary traffic, but
a large search blocks the event loop, and unlike Python there is no `*_async`
binding. Run those calls in a `worker_threads` worker (or one process per core)
and keep the main loop serving; share one engine per worker rather than building
one per request.

Request documents share the exact wire format used by Rust and Python:

- `exclude_event_keys` excludes events being rescheduled from blocking, rest triggers, and cap counts.
- `occurrences` passes a shared event once (its `event_key`, `span`, `participants` and `tags`) so a pooled cap counts it once across calendars. See [counting a shared event once](recipes.md#counting-a-shared-event-once).
- `weights` steers host assignment among eligible candidates: higher `i32` wins, omitted is `0`, ties break by participant ID.
- `{ kind: "person" }` (`KindSelector`) scopes human-only rules away from `kind: "resource"` conference rooms; `validate` warns with `status_policy_ignores_resource` at `/rules/{index}` if an unscoped `ignore` policy applies to a resource.
- `all_day` blocks block the day they cover without counting toward a cap's meeting budget.

Product-model limits apply here too: v1 goals have exactly one part, `statusPolicy`
entries are first-match across policies in request order (most specific first),
`{ kind: "person" }` also matches attendees (combine it with `{ role: "host" }` to
scope host-only rules), `calendar_unknown` is host-only, and multi-seat group
classes belong in the application or as `Resource` seats rather than a `cap`. See
[product-model constraints](api.md#product-model-constraints-in-v1).

### Limits, upgrades, and constants

Customize request-size limits starting from the engine's defaults via `limitsWith` or `defaultLimits()`:

<!-- snippet: wasm-ts-limits -->
```ts
import { Engine, limitsWith } from "opportune";

const limits = limitsWith({ max_search_window_days: 120 });
const engine = new Engine(limits);
console.log(engine.tzdbVersion);   // the version a response's `meta.tzdb` reports
```

`defaultLimits()` covers all ten caps: participants, blocks per participant, zone periods per participant (`max_zone_periods_per_participant`), rules, occurrences, requirement slots/terms, search window days, candidate starts, `explain.limit`, and maximum duration. `new Engine(limits)` throws `OpportuneError` with code `invalid_limits` (classified `500`) when the document is not one the engine reads; a field set to `undefined` or `null` reads as the default, including every field of `limitsWith({...})`.

- `upgradeRule`, `upgradeCalendar`, and `upgradeGoal` migrate stored documents from earlier wire schema versions.
- `VERSION`, `MARKER_TAGS`, and `MARKER_EVENT_KEY` expose the engine version and extended-property marker names without instantiating WebAssembly.

## Graph and Google adapters

`opportune/adapters/graph` and `opportune/adapters/google` convert recorded provider payloads into domain blocks and rules with no network or clock access. Per-calendar provider errors — or unreadable single-calendar responses — return an `unavailable` outcome carrying an error code instead of an empty block list (which would falsely read as free time). Unreadable batch payloads (`getSchedule`, `freeBusy`) throw `OpportuneError` with code `payload_invalid`.

For Graph, follow `@odata.nextLink` and pass the concatenated `value` array: a single page is `payload_invalid` because the rest would read as free. Recurring occurrences derive distinct event keys from `originalStart` (`<key>@<utc>`), so a series is not counted once. A `getSchedule` entry with no `scheduleItems` is free only when its `availabilityView` is all free; busy slots without items make that calendar `payload_invalid`. For Google, follow `nextPageToken` before calling `eventsListToBlocks` for the same reason.

<!-- snippet: wasm-ts-adapters -->
```ts
import { businessHours } from "opportune";
import * as graph from "opportune/adapters/graph";

const outcome = graph.calendarViewToBlocks(calendarViewPayload, "Europe/London");
const calendar = outcome.status === "available"
  ? outcome.blocks.map((entry) => entry.block)
  : null;                       // unknown availability, not "free"

const hours = graph.workingHoursToRule(workingHoursPayload, { who: { ids: ["rich"] } });
const request = {
  /* … */
  participants: [
    { id: "rich", zone: "Europe/London", ...(calendar === null ? {} : { calendar }) },
  ],
  rules: hours.status === "available" ? [hours.rule] : [businessHours()],
};
if (request.participants.length !== 1 || request.rules.length !== 1) {
  throw new Error("the adapter request is incomplete");
}
console.log(request.participants.length, request.rules.length);
```

Both submodules accept payloads as JSON strings or parsed objects and re-export `MARKER_TAGS` and `MARKER_EVENT_KEY` (plus the four `EXTENDED_PROPERTY_*` constants on `opportune/adapters/graph`).

```text
calendarViewToBlocks(payload, participantZone, { categoryMap?, engine? })
eventsListToBlocks(payload, participantZone, { engine? })
getScheduleToBlocks(payload, { engine? })
freeBusyToBlocks(payload, { engine? })
workingHoursToRule(payload, { ruleId?, who?, when?, engine? })
```

Pass a custom `Engine` via `{ engine }` when you need zone resolution against a specific engine instance (passing an `Engine` as `categoryMap` throws `OpportuneError` with `invalid_type`).

## Presets and sugar

All builders are exported from both `opportune` and the pure-TypeScript `opportune/presets` subpath (alongside `OpportuneError`, `httpStatusForError`, `isKnownStatus`, the open unions `BlockStatus`/`OfferFlag`/`OpportuneErrorCode`, `publicResponse`/`publicSlotCheck`, constants, and generated types):

- **Goal & offer sugar:** `singleGoal` (`SingleGoalOptions`; supports `displayZone`, `grid`, `candidates`, `partId`, `tags`, and `travel`; `tags` and `candidates` each accept one `string` or a `readonly string[]`), `person`, `atLeast`, and the `offerStart`, `offerEnd`, `offerAssignedHosts`, `offerCalendarUnknown`, `offerReserve`, and `offerToProposal` accessors.
- **Rule presets:** `businessHours`, `datedHours`, `weeklyHours` (one rule for multi-range weeks and holiday overrides), `buffers` (supports `minGap`), `minNotice`, `horizon` (accepts a duration string, date string, calendar-day count, or typed `LeadTimeMax` such as `{ calendar: "org", working_days: 4 }`), `rest`, `capPerDay`, `capPerWeek`, `capPerMonth` (pooled caps require an explicit `zone`, e.g., `capPerWeek(selector, 8, { per: "pooled", zone: "event" })`), `statusPolicy` (try the most specific policy first), and `availableOnly`.
- **`statusPolicy(entries, options)`:** Builds a `status_policy` rule from generated `StatusPolicyEntry` objects or wire mappings. Entries are tried first-match across applicable policies in request order, so pass `[hostPolicy, teamPolicy, orgPolicy]` in a tiered product; an unscoped `ignore` entry reaches resource rooms unless `who` is scoped to people.
- **Scoping & layered overrides:** Every preset except `availableOnly` accepts `who` and `when` selectors. Use `unless(rule, selector)` to exclude a narrower tag from a broader rule (wrapping any existing `when` in `{ all: [existing, { not: selector }] }`) so wider exceptions do not intersect to empty — see [layered policies](api.md#layered-policies-global-event-group-and-event-type-overrides).
- **People rather than bookings:** `andWho(rule, selector)` is the participant-side twin of `unless` and intersects into `who` instead of replacing it, so `andWho(businessHours("mon-fri", "09:00-17:00"), { kind: "person" })` keeps the default `role: "host"` clause and reaches human hosts alone — not attendees, not resource rooms. `exceptWho(rule, selector)` narrows by `not(selector)`, so `exceptWho(capPerDay({ tag: "interview" }, 2), { ids: ["ceo"] })` exempts the CEO while every other host stays subject. Passing `who: { kind: "person" }` to a builder instead replaces the default outright and also matches attendees.
- **Public route projections:** `publicResponse(response)` and `publicSlotCheck(check)` strip operator-only fields (`warnings`, `excluded`, `excluded_truncated`, `eligible`, `assignment`, `reserve`, and `SlotCheck.reasons`) before serving answers to unauthenticated visitors.
- **Stated external availability:** `availableOnly` returns an `[attendee, rule]` tuple with rule ID `available_only:{id}` (bounded to the same 3,660-day ceiling as Rust and Python):

<!-- snippet: wasm-ts-available-only -->
```ts
import { availableOnly } from "opportune";

const [attendee, rule] = availableOnly("ana", [
  ["2027-06-14T09:00Z", "2027-06-14T11:30Z"],
]);
const request = {
  /* … */
  participants: [attendee],
  rules: [rule],
};
if (request.participants.length !== 1 || request.rules.length !== 1) {
  throw new Error("the availability-only request is incomplete");
}
console.log(request.participants.length, request.rules.length);
```

See [Presets in the API reference](api.md#presets) for complete semantics.

## Bundle size

Most of the WASM binary size comes from the bundled IANA time zone database and calendar adapters. Release builds are optimized with `-Oz` (`wasm-opt`), tracked against a committed gzipped size budget in `crates/opportune-wasm/js/wasm-size-budget.json`:

| Metric | Gzipped (level 9) |
| --- | --- |
| Recorded baseline (`default`, `windows-zones` on) | 608,415 B |
| Enforced CI ceiling (`budgetBytes`) | 640,000 B |

Run `make wasm-size` to print live raw and gzipped sizes beside both figures and fail if the module exceeds the ceiling.

Compiling without `windows-zones` drops the `Unicode-DFS-2016` CLDR dependency
(verified by `deny.minimal.toml`) and produces a smaller module. To weigh that trade
yourself, build both targets without the feature and run the size gate over the result:

```sh
wasm-pack build crates/opportune-wasm --target web --out-dir js/pkg --release \
  --no-default-features
wasm-pack build crates/opportune-wasm --target nodejs --out-dir js/pkg-node --release \
  --no-default-features
npm --prefix crates/opportune-wasm/js run build
node crates/opportune-wasm/js/scripts/check-wasm-size.mjs
```

Without `windows-zones`, calendars naming a Windows time zone return an `unavailable` outcome with `zone_unknown` instead of guessing an offset.

## Running the test suite

```sh
make wasm        # build the release WASM module and dist/
make wasm-size   # check the gzipped bundle-size budget
make test-wasm   # check the budget, run the Node test suite, and run TypeScript doc snippets
```

`make test-wasm` also compiles and executes every classified `ts` snippet across the documentation (`make test-docs-snippets-ts` runs that lane alone).

Run the identical suite under Bun without rewriting any tests:

```sh
bun test crates/opportune-wasm/js/tests/
```

Tooling is installed via `npm ci --ignore-scripts` from the committed `package-lock.json` (do not run `bun install` inside `crates/opportune-wasm/js`). CI runs the suite on Node 24, Node 26, and Bun, including packed-tarball install, subpath resolution, CommonJS entrypoints, and TypeScript declaration checks.
