# opportune (npm)

Availability scheduling for Node, Bun, Cloudflare Workers, browsers and module Web Workers: the [`opportune`](https://github.com/AZX-PBC-OSS/opportune) engine — the same core exposed by the Rust and Python packages — compiled to WebAssembly and wrapped in a strict TypeScript surface. The IANA time zone database is bundled inside the module, so calls need no native toolchain and no `TZif` files on disk.

```sh
npm install opportune
```

Requires Node 24+ or Bun 1.4.2+, or a Cloudflare Worker, a browser or a module Web Worker. Node, Bun and Cloudflare Workers load the module themselves; a browser or module Web Worker calls `init()` from `opportune/web` first — see [Runtimes](#runtimes).

## Quickstart: one person, one meeting

<!-- snippet: npm-ts-quickstart -->
```ts
import { Engine, businessHours, buffers, minNotice, offerStart, person, singleGoal } from "opportune";

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
const [earliest] = engine.findAvailability(request).offers;
console.log(earliest === undefined ? "no offers" : offerStart(earliest));
```

`using` releases the engine's native handle when the block exits; without `using`, call `engine.free()`. Keep `using` inside a function or request handler — at ES module top level, `using` disposes as soon as module evaluation completes, before any exported handler runs. For a long-lived module-scope instance, use `const engine = new Engine()` or call the free functions (`findAvailability`, `checkSlot`, `validate`, `requiredRanges`), which share one engine automatically.

`findAvailabilityJson`, `checkSlotJson`, `validateJson`, and `requiredRangesJson` return the engine's canonical JSON string directly for routes that forward responses unchanged.

## Errors

Every failure throws `OpportuneError` with a stable `code`, a `message`, an RFC 6901 `path`, and `errors` (per-field validation diagnostics). Handlers catching `OpportuneError` never receive a bare `TypeError`:

<!-- snippet: npm-ts-errors -->
```ts
import { OpportuneError, findAvailability, person, singleGoal } from "opportune";

try {
  findAvailability({
    now: "2027-06-14T07:00Z",
    goal: singleGoal("30m", { all: [person("rich")] }, {
      zone: "Europe/London",
      searchWindow: ["2027-06-13T23:00Z", "2027-06-18T23:00Z"],
    }),
    participants: [{ id: "rich", zone: "Europe/London", calendar: [] }],
    rules: [],
    limit: Infinity,   // JSON serializes Infinity as `null`
  });
} catch (thrown) {
  if (thrown instanceof OpportuneError) {
    console.error(thrown.code, thrown.path, thrown.errors);   // invalid_json, null, []
  }
}
```

`httpStatusForError(error)` maps a rejection to the status a route should answer:
`422` for a caller-fixable document (`invalid_request`, `invalid_json`, wire
codes) and `500` for deployment/integration or package-argument failures
(`invalid_limits`, `payload_invalid`, `invalid_argument`, `argument_not_utf8`,
`unsupported_schema_version`, `serialization_failed`, `tzdb_*`, WASM codes). Use
it in Express's error middleware, Hono's `onError`, or a Next.js `NextResponse`.
`new Engine(limits)` throws `invalid_limits` for a limits document the engine
cannot read; `undefined` and `null` mean the engine defaults.

Three generated unions are open so a newer engine's value still assigns:
`BlockStatus` (`free` | `tentative` | `busy` | any provider string),
`OfferFlag` (`"calendar_unknown"` plus a future flag), and `OpportuneErrorCode`.
Use `isKnownStatus(value: unknown)` to narrow a provider status to the three the
engine interprets before acting on `free`. On generated documents a `null` optional field
means absent (the wire default applies); an options object itself must be an
object, so `null` as a builder's options argument is `invalid_type`.

## Presets, policies, and public answers

`opportune` and `opportune/presets` export the same builders so browser bundles and servers share one vocabulary:

- **`singleGoal(duration, requirement, options)`** accepts `tags` and `candidates` as one string or a `readonly string[]`; `partId`, `displayZone`, `grid` and `travel` are optional.
- **`statusPolicy(entries, options)`** builds a `status_policy` rule from generated entries or wire mappings. Policies are tried first-match across rules in request order, so pass most-specific first (`[host, team, org]`).
- **`unless(rule, selector)`** excludes a selector from the bookings a rule applies to (for example, `unless(businessHours(), { tag: "vip" })`), allowing a narrower tier to loosen a broader rule instead of intersecting with it.
- **`andWho(rule, selector)`** is the participant-side twin: it intersects `who` instead of replacing it, so `andWho(businessHours(), { kind: "person" })` keeps the default `role: "host"` clause and reaches human hosts alone — not attendees, not resource rooms. **`exceptWho(rule, selector)`** narrows by `not(selector)`, so `exceptWho(capPerDay({ tag: "interview" }, 2), { ids: ["ceo"] })` exempts the CEO while every other host stays subject.
- **`{ kind: "person" }`** scopes human-only rules (working hours, rest, caps, and `status_policy`) away from `kind: "resource"` conference rooms. It also matches attendees, so use `andWho` (or combine it with `{ role: "host" }`) when the rule is about human hosts only. `validate(request)` warns with `status_policy_ignores_resource` at `/rules/{index}` if an unscoped `ignore` policy reaches a room.
- **`exclude_event_keys`** on a request ignores events being rescheduled for blocking, rest, and cap counts so a booking can move even when a host is at their daily cap.
- **`all_day` blocks** block the day they cover without consuming a cap's meeting budget.
- **`publicResponse(response)` and `publicSlotCheck(check)`** strip operator-only fields (warnings, `excluded` reasons, and each offer's `eligible`, `assignment`, and `reserve`) for public booking routes. The full `Response` and `SlotCheck` remain unchanged for operator routes.

The engine is synchronous: a large search blocks the Node event loop and there is
no `*_async` binding. Run heavy calls in a `worker_threads` worker or a separate
process, and share one engine per worker rather than building one per request.

## Subpaths

- `opportune` — engine, presets, sugar helpers, and generated TypeScript types.
- `opportune/presets` — pure TypeScript rule/goal/offer builders, `OpportuneError`, `httpStatusForError`, `isKnownStatus`, the open unions, `publicResponse`/`publicSlotCheck`, `VERSION`, marker constants, and generated types. Loads no WebAssembly and no `node:` modules — safe for browser, module Web Worker, and client component bundles.
- `opportune/adapters/graph` — Microsoft Graph `calendarView`, `getSchedule`, and `workingHours` conversions.
- `opportune/adapters/google` — Google Calendar `events.list` and `freeBusy.query` conversions.
- `opportune/web` — explicit WASM initializer (`init`, `initSync`) for browsers and module Web Workers.
- `opportune/opportune_wasm_bg.wasm` — the compiled WebAssembly binary for bundlers or CDNs serving it to `init(...)`.
- `opportune/schema.json` — the Draft 2020-12 JSON Schema the generated types derive from, exported directly from the package for consumers that validate a document before sending it. Read it with `import schema from "opportune/schema.json" with { type: "json" }` or `require("opportune/schema.json")`; `make check-codegen-ts` drift-checks it against the Rust export.

Provider conversions return an `unavailable` outcome for errored calendars instead of an empty block list, so an unreachable mailbox never reads as free.

## Runtimes

The engine runs on **Node 24+**, **Bun 1.4.2+**, **Cloudflare Workers (`workerd`)**, **browsers**, and **module Web Workers** — all backed by one `.wasm` binary selected through `package.json#imports["#wasm"]`:

- **Node 24+ / Bun 1.4.2+ (`node`, `default`):** lazy synchronous load from disk on the first engine call (works with both ESM `import` and CommonJS `require("opportune")`).
- **Cloudflare Workers (`workerd`):** static `.wasm` import compiled by Wrangler (`CompiledWasm` rule in `wrangler.jsonc`) and instantiated lazily on the first engine call, with zero runtime `WebAssembly.compile`. Reuse a single module-scoped `Engine` across requests in the isolate.
- **Browsers and module Web Workers (`browser`, `worker`):** call `await init(wasmUrl)` (or `initSync({ module })`) from `opportune/web` once before creating an `Engine`, passing the `.wasm` asset URL (for example `import wasmUrl from "opportune/opportune_wasm_bg.wasm?url"` in Vite). Calling the engine before initialization throws `OpportuneError` with code `wasm_unavailable`.

Deno, the deprecated Next.js Edge runtime (`edge-light`), and browser engines beyond the Chromium, Firefox, and WebKit test matrix are untested and not claimed; see [`docs/wasm.md`](https://azx-pbc-oss.github.io/opportune/wasm/) for the full runtime contract and non-claims.

A Cloudflare Worker compiles the module in its bundle (Wrangler `CompiledWasm`), and one module-scoped `Engine` serves every request in the isolate:

<!-- snippet: npm-ts-cloudflare-worker -->
```ts
import { Engine } from "opportune";

const engine = new Engine();

export default {
  async fetch(request: Request): Promise<Response> {
    if (request.method !== "POST") {
      return new Response("POST a request document", { status: 405 });
    }
    return new Response(
      engine.findAvailabilityJson(
        (await request.json()) as Parameters<Engine["findAvailabilityJson"]>[0],
      ),
    );
  },
};
```

A browser or module Web Worker initializes the packaged module before its first engine call:

```js
import { Engine } from "opportune";
import { init } from "opportune/web";
// Vite: resolve the package's .wasm as an asset URL.
import wasmUrl from "opportune/opportune_wasm_bg.wasm?url";

await init(wasmUrl);

const engine = new Engine();
console.log(engine.tzdbVersion);
```

In a Next.js App Router project, run the engine on the Node.js runtime (Vercel's recommended default):

<!-- snippet: npm-ts-runtime -->
```ts
export const runtime = "nodejs";
```

and list `"opportune"` in `serverExternalPackages` in `next.config.mjs`:

```js
// next.config.mjs
export default { serverExternalPackages: ["opportune"] };
```

Import `opportune/presets` wherever a bundle only needs builders and types.

## Documentation

Full guide — requests, responses, limits, provider adapters, presets, and bundle-size budget — at [azx-pbc-oss.github.io/opportune/wasm/](https://azx-pbc-oss.github.io/opportune/wasm/) (source in [`docs/wasm.md`](https://github.com/AZX-PBC-OSS/opportune/blob/main/docs/wasm.md)).

## License

MIT OR Apache-2.0.
