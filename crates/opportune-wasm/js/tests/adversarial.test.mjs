/**
 * The argument checks the JavaScript boundary makes before the module is called.
 *
 * A JavaScript consumer is not type-checked, so an argument that is not the shape
 * the declaration asks for reaches this layer whatever the types say. Every such
 * argument must leave as `OpportuneError` with a stable `code` — never as a bare
 * `TypeError` from `String.prototype.isWellFormed`, from `for…of` over a
 * non-iterable, or from indexing `null`, and never as a silent rewrite of what the
 * caller handed over. These cases pin that for every public entry point that reads
 * an argument itself: the presets that take a list or an options object, the two
 * provider adapters that take a zone name or a rule ID, and the document a caller
 * already holds as text.
 *
 * The engine's own release is here for the same reason: a freed `Engine` is what an
 * argument check has to *report* rather than reach, and the release that frees it —
 * `using` in `docs/wasm.md`'s first example, or `free()` — has to actually happen in
 * whichever runtime runs it.
 *
 * Two things the package promises about its own module graph are here too, because
 * both are observable only as a rejection a caller can catch: the values that would
 * otherwise be rewritten silently on their way into JSON, and the surface the
 * browser/Web Worker entry exposes so a caller can initialise the module. The
 * `opportune/presets` subpath is the mirror image —
 * it must load no WebAssembly at all, which `module_load_failure.test.mjs` proves from
 * a package whose module is absent.
 *
 * Run against the built package, so build it first:
 *
 * ```sh
 * npm --prefix crates/opportune-wasm/js run build &&
 *   node --test crates/opportune-wasm/js/tests/adversarial.test.mjs
 * ```
 */

import assert from "node:assert/strict";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { pathToFileURL } from "node:url";
import test from "node:test";

import {
  Engine,
  MARKER_EVENT_KEY,
  MARKER_TAGS,
  OpportuneError,
  VERSION,
  availableOnly,
  buffers,
  businessHours,
  capPerDay,
  capPerWeek,
  checkSlot,
  findAvailability,
  horizon,
  limitsWith,
  minNotice,
  offerAssignedHosts,
  offerCalendarUnknown,
  offerEnd,
  offerReserve,
  offerStart,
  offerToProposal,
  person,
  rest,
  singleGoal,
  upgradeCalendar,
  upgradeGoal,
  upgradeRule,
} from "../dist/index.js";
import {
  calendarViewToBlocks,
  EXTENDED_PROPERTY_EVENT_KEY_ID,
  EXTENDED_PROPERTY_EXPAND,
  EXTENDED_PROPERTY_NAMESPACE_GUID,
  EXTENDED_PROPERTY_TAGS_ID,
  getScheduleToBlocks,
  workingHoursToRule,
} from "../dist/adapters/graph.js";
import { eventsListToBlocks, freeBusyToBlocks } from "../dist/adapters/google.js";
import { OpportuneError as PresetError, businessHours as presetBusinessHours } from "../dist/presets-entry.js";
import { wasmModule } from "#wasm";
import { binding } from "../dist/engine.js";

/** An empty Graph `calendarView` — the conversion reads every payload as a record. */
const CALENDAR_VIEW = JSON.stringify({ value: [] });

/** An empty Google `events.list`. */
const EVENTS_LIST = JSON.stringify({ items: [] });

/** One busy Graph event, the shape `calendar_view_ana.json` carries. */
const ONE_GRAPH_EVENT = JSON.stringify({
  value: [
    {
      id: "ana-interview-0900",
      subject: "Interview: front-end round",
      isAllDay: false,
      showAs: "busy",
      start: { dateTime: "2027-06-14T09:00:00.0000000", timeZone: "UTC" },
      end: { dateTime: "2027-06-14T10:00:00.0000000", timeZone: "UTC" },
    },
  ],
});

/** One Google event, the shape `events_list_ana.json` carries. */
const ONE_GOOGLE_EVENT = JSON.stringify({
  items: [
    {
      id: "ana-interview-0900",
      status: "confirmed",
      start: { dateTime: "2027-06-14T09:00:00Z" },
      end: { dateTime: "2027-06-14T10:00:00Z" },
    },
  ],
});

/** One `getSchedule` mailbox, the shape `get_schedule.json` carries. */
const ONE_SCHEDULE = JSON.stringify({
  value: [
    {
      scheduleId: "ana@example.com",
      scheduleItems: [
        {
          status: "busy",
          start: { dateTime: "2027-06-14T09:00:00.0000000", timeZone: "UTC" },
          end: { dateTime: "2027-06-14T10:00:00.0000000", timeZone: "UTC" },
        },
      ],
    },
  ],
});

/** One `freeBusy` calendar, the shape `free_busy.json` carries. */
const ONE_FREE_BUSY = JSON.stringify({
  calendars: {
    "ana@example.com": {
      busy: [{ start: "2027-06-14T09:00:00Z", end: "2027-06-14T10:00:00Z" }],
    },
  },
});

/** One mailbox's `workingHours`, the shape `working_hours.json` carries. */
const WORKING_HOURS = JSON.stringify({
  daysOfWeek: ["monday"],
  startTime: "09:00:00.0000000",
  endTime: "17:00:00.0000000",
  timeZone: { name: "UTC" },
});

/** A stored blackout rule, as the text an owner would read out of storage. */
const STORED_RULE =
  '{"id":"freeze","kind":"blackout","params":{"zone":"event",' +
  '"spans":[["2027-06-14T10:00","2027-06-14T11:00"]]}}';

/** An unpaired surrogate: the character `TextEncoder` replaces with U+FFFD. */
const LONE_SURROGATE = "\ud800";

/**
 * The value `call` threw.
 *
 * `assert.throws` would do, but the point of these cases is the *type* of the
 * thrown value: a raw `TypeError` is not a rejection a caller's handler for
 * `OpportuneError` can answer, whatever its message says.
 */
function thrown(call) {
  try {
    call();
  } catch (error) {
    return error;
  }
  assert.fail("expected the call to reject");
}

/** `call` rejected with `OpportuneError` carrying `code`, naming `what`. */
function assertRejection(call, code, what) {
  const error = thrown(call);
  assert.ok(
    error instanceof OpportuneError,
    `expected OpportuneError, got ${error?.constructor?.name}: ${error?.message}`,
  );
  assert.equal(error.code, code);
  assert.equal(error.path, null);
  assert.deepEqual(error.errors, []);
  assert.match(error.message, new RegExp(what, "i"));
  return error;
}

test("a stated-availability list that is not a list is a rejection, not a TypeError", () => {
  // `availableOnly("ana")` and `availableOnly("ana", null)` both reach `for…of`
  // with nothing to iterate, which is the shape a caller gets from an ignored
  // lookup rather than from a typo.
  assertRejection(() => availableOnly("ana"), "invalid_argument", "spans");
  assertRejection(() => availableOnly("ana", null), "invalid_argument", "spans");
});

test("a stated span that is not a pair is a rejection, not a TypeError", () => {
  // A `null` entry is what a JSON API response with a hole in it produces; the
  // pair check has to answer before the instant parser indexes it.
  assertRejection(() => availableOnly("ana", [null]), "invalid_argument", "span");
  assertRejection(
    () => availableOnly("ana", [["2027-06-14T09:00Z"]]),
    "invalid_argument",
    "instant",
  );
  assertRejection(() => availableOnly("ana", [[7, 9]]), "invalid_argument", "instant");
  // Three entries are not a `Span` either: the wire pair has two, and the extra one
  // used to be dropped without a word.
  assertRejection(
    () => availableOnly("ana", [["2027-06-14T09:00Z", "2027-06-14T10:00Z", "x"]]),
    "invalid_argument",
    "span",
  );
});

test("a stated span that is not aligned to a whole minute is a rejection", () => {
  // `Date.parse` keeps no digit below a millisecond, so a `.000001` fraction
  // arrives already aligned and would be read as the whole minute below it. Python's
  // `_utc_minute` rejects a non-zero sub-minute part down to a microsecond; this
  // rejects one down to the ninth fraction digit the wire spelling allows, so no
  // endpoint is ever truncated silently.
  const end = "2027-06-14T12:00Z";
  for (const start of [
    "2027-06-14T09:00:30Z",
    "2027-06-14T09:00:00.5Z",
    "2027-06-14T09:00:00.001Z",
    "2027-06-14T09:00:00.000001Z",
    "2027-06-14T09:00:00.000000001Z",
  ]) {
    assertRejection(() => availableOnly("ana", [[start, end]]), "invalid_argument", "whole minute");
  }
  // Seconds that add nothing to the minute are not a restriction: the wire spelling
  // allows them, and Python accepts every one of these.
  for (const start of [
    "2027-06-14T09:00Z",
    "2027-06-14T09:00:00Z",
    "2027-06-14T09:00:00.000Z",
    "2027-06-14T09:00:00.000000000Z",
  ]) {
    const [, rule] = availableOnly("ana", [[start, end]]);
    assert.deepEqual(rule.params.dates, { "2027-06-14": ["09:00-12:00"] }, start);
  }
});

test("a working-hours list that is not a list is a rejection, not a TypeError", () => {
  // `hours` is `string | readonly string[]`; a caller reading both from one
  // variable hands this layer whatever it holds.
  assertRejection(() => businessHours("mon-fri", null), "invalid_argument", "hours");
  assertRejection(() => businessHours("mon-fri", 5), "invalid_argument", "hours");
  // Non-string entries in `hours` — including values whose implicit stringification
  // throws (`Symbol`, null-prototype object) or spoofs a range via `toString` — must
  // reject as `invalid_argument`, never leak a `TypeError` or embed an object.
  assertRejection(() => businessHours("mon-fri", [Symbol("x")]), "invalid_argument", "hours");
  assertRejection(
    () => businessHours("mon-fri", [Object.create(null)]),
    "invalid_argument",
    "hours",
  );
  assertRejection(
    () => businessHours("mon-fri", [{ toString: () => "09:00-17:00" }]),
    "invalid_argument",
    "hours",
  );
});

test("a day key that is not a string is rejected rather than written as a key", () => {
  // Without the check the object becomes the weekday key `[object Object]`, which
  // the engine then rejects with a pointer into a rule the caller never wrote.
  assertRejection(() => businessHours({}), "invalid_argument", "days");
});

test("a non-document argument that is not a string is a rejection, not a TypeError", () => {
  // The two zone arguments and the rule ID are not JSON documents, so nothing
  // downstream can report a wrong type for them: the check is this layer's.
  assertRejection(
    () => calendarViewToBlocks(CALENDAR_VIEW),
    "invalid_type",
    "participant zone",
  );
  assertRejection(
    () => calendarViewToBlocks(CALENDAR_VIEW, null),
    "invalid_type",
    "participant zone",
  );
  assertRejection(() => eventsListToBlocks(EVENTS_LIST, 5), "invalid_type", "participant zone");
  assertRejection(
    () => workingHoursToRule(CALENDAR_VIEW, { ruleId: 7 }),
    "invalid_type",
    "rule id",
  );
  // A null-prototype object (such as Node's `querystring.parse` output) throws on
  // `String(value)`; error formatting must never turn a rejection into a `TypeError`.
  assertRejection(
    () => calendarViewToBlocks(CALENDAR_VIEW, Object.create(null)),
    "invalid_type",
    "participant zone",
  );
  assertRejection(
    () => availableOnly("ana", [[Object.create(null), "2027-06-14T10:00Z"]]),
    "invalid_argument",
    "instant",
  );
  assertRejection(
    () => capPerDay({ tag: "x" }, 2, { per: "pooled", ruleId: Symbol("cap") }),
    "invalid_argument",
    "Symbol\\(cap\\)",
  );
  assertRejection(
    () => availableOnly("ana", [["9999-12-31T23:00-05:00", "9999-12-31T23:30-05:00"]]),
    "invalid_argument",
    "1975-01-01T00:00Z\\.\\.=2100-01-01T00:00Z",
  );
});

test("an options argument that is not an object is a rejection, not a TypeError", () => {
  // `fn(a, b, null)` is the JS idiom for "defaults" (`JSON.stringify(value, null, 2)`
  // taught it), so it is what a caller with nothing to configure writes. Reading a
  // field off it must answer as a rejection, not from the property access; arrays
  // are not options objects either.
  const span = [["2027-06-14T09:00Z", "2027-06-14T10:00Z"]];
  const cases = [
    ["businessHours(null)", () => businessHours("mon-fri", "09:00-17:00", null)],
    ["businessHours([])", () => businessHours("mon-fri", "09:00-17:00", [])],
    ["buffers", () => buffers("10m", "10m", null)],
    ["minNotice", () => minNotice("4h", null)],
    ["horizon", () => horizon({ days: 3 }, null)],
    ["rest", () => rest("18:00", "10:00", null)],
    ["capPerDay", () => capPerDay({ tag: "x" }, 2, null)],
    ["capPerWeek", () => capPerWeek({ tag: "x" }, 2, null)],
    ["availableOnly", () => availableOnly("ana", span, null)],
    ["singleGoal", () => singleGoal("30m", { all: [] }, null)],
    ["calendarViewToBlocks", () => calendarViewToBlocks(CALENDAR_VIEW, "UTC", null)],
    ["getScheduleToBlocks", () => getScheduleToBlocks(ONE_SCHEDULE, null)],
    ["eventsListToBlocks", () => eventsListToBlocks(EVENTS_LIST, "UTC", null)],
    ["freeBusyToBlocks", () => freeBusyToBlocks(ONE_FREE_BUSY, null)],
    ["workingHoursToRule", () => workingHoursToRule(CALENDAR_VIEW, null)],
    ["limitsWith(null)", () => limitsWith(null)],
    ["limitsWith([])", () => limitsWith([])],
    ["limitsWith(5)", () => limitsWith(5)],
  ];
  for (const [name, call] of cases) {
    const error = thrown(call);
    assert.ok(
      error instanceof OpportuneError,
      `${name}: expected OpportuneError, got ${error?.constructor?.name}: ${error?.message}`,
    );
    assert.equal(error.code, "invalid_type", name);
  }
});

test("the argument checks leave a valid call unchanged", () => {
  // The guards above are shape checks, not a restriction: a frozen array of
  // readonly tuples still converts to the dated ranges it names.
  const spans = Object.freeze([
    Object.freeze(["2027-06-14T09:00Z", "2027-06-14T11:30Z"]),
    Object.freeze(["2027-06-14T10:00Z", "2027-06-15T02:00Z"]),
  ]);
  const [, rule] = availableOnly("ana", spans);
  assert.deepEqual(rule.params.dates, {
    "2027-06-14": ["09:00-24:00"],
    "2027-06-15": ["00:00-02:00"],
  });
  assert.deepEqual(businessHours("mon-fri", ["09:00-12:00", "13:00-17:00"]).params.weekly, {
    "mon-fri": ["09:00-12:00", "13:00-17:00"],
  });
});

/**
 * `text` with `from` replaced by an unpaired surrogate.
 *
 * The generated glue encodes every string argument with `TextEncoder`, which
 * replaces an unpaired surrogate with U+FFFD rather than failing. Without a check
 * before the call, the module reads a document that differs from the one the caller
 * holds and answers for the difference: an event ID that no longer names the
 * provider's event, an outcome keyed by a mailbox no lookup can reproduce, a rule
 * rewritten under an ID its owner never gave it. Nothing downstream can tell by the
 * time the bytes arrive, so the answer has to be a rejection here.
 */
function mangled(text, from) {
  const mangled = text.replace(from, LONE_SURROGATE);
  assert.notEqual(mangled, text, `the case substituted nothing for ${from}`);
  assert.equal(mangled.isWellFormed(), false, "the case is not the shape it claims");
  return mangled;
}

test("a document held as text with an unpaired surrogate is a rejection, not a rewrite", () => {
  // `invalid_json` is the wire layer's own code and the one the Python binding
  // answers for the same argument (`crates/opportune-py/src/error.rs::document`):
  // the text is a document, so it is the document that cannot be read.
  const cases = [
    ["calendarViewToBlocks", () =>
      calendarViewToBlocks(mangled(ONE_GRAPH_EVENT, '"ana-interview-0900"'), "UTC")],
    ["eventsListToBlocks", () =>
      eventsListToBlocks(mangled(ONE_GOOGLE_EVENT, '"ana-interview-0900"'), "UTC")],
    ["getScheduleToBlocks", () =>
      getScheduleToBlocks(mangled(ONE_SCHEDULE, '"ana@example.com"'))],
    ["freeBusyToBlocks", () => freeBusyToBlocks(mangled(ONE_FREE_BUSY, '"ana@example.com"'))],
    ["workingHoursToRule", () => workingHoursToRule(mangled(WORKING_HOURS, '"monday"'))],
    ["upgradeRule", () => upgradeRule(mangled(STORED_RULE, '"freeze"'), 1)],
  ];
  for (const [name, call] of cases) {
    const error = thrown(call);
    assert.ok(
      error instanceof OpportuneError,
      `${name}: expected OpportuneError, got ${error?.constructor?.name}: ${error?.message}`,
    );
    assert.equal(error.code, "invalid_json", name);
    assert.equal(error.path, null, name);
    assert.match(error.message, /not UTF-8 text/, name);
  }
});

test("a document argument that is not a string is a caller error, not a module failure", () => {
  // Left to the call, `undefined` reaches `passStringToWasm0` and throws a
  // `TypeError` from inside the generated glue, which `translated` can only report
  // as `wasm_error` — blaming the module for the caller's argument.
  assertRejection(() => upgradeRule(undefined, 1), "invalid_type", "rule");
  assertRejection(() => upgradeRule(null, 1), "invalid_type", "rule");
  assertRejection(() => calendarViewToBlocks(undefined, "UTC"), "invalid_json", "payload");
});

test("an invalid schema version is rejected instead of coerced to a u32", () => {
  // `wasm-bindgen` passes `fromVersion` into a WebAssembly `i32`, which coerces
  // `1.5`, `2**32 + 1`, `"1"` and `true` to `1` and wraps `-1` to `4294967295`.
  for (const bad of [1.5, NaN, Infinity, "1", true, undefined, null]) {
    assertRejection(() => upgradeRule(STORED_RULE, bad), "invalid_type", "schema version");
  }
  for (const outOfRange of [-1, 2 ** 32, 2 ** 32 + 1]) {
    assertRejection(
      () => upgradeRule(STORED_RULE, outOfRange),
      "invalid_value",
      "schema version",
    );
  }
  assertRejection(() => upgradeCalendar("{}", 1.5), "invalid_type", "schema version");
  assertRejection(() => upgradeGoal("{}", -1), "invalid_value", "schema version");
});

test("a non-Engine argument reports invalid_type rather than engine_disposed", () => {
  assertRejection(
    () => calendarViewToBlocks(CALENDAR_VIEW, "UTC", { engine: {} }),
    "invalid_type",
    "engine",
  );
  assertRejection(
    () => eventsListToBlocks(EVENTS_LIST, "UTC", { engine: 5 }),
    "invalid_type",
    "engine",
  );
  // A freed `Engine` still reports `engine_disposed`.
  const engine = new Engine();
  engine.free();
  assertRejection(
    () => calendarViewToBlocks(CALENDAR_VIEW, "UTC", { engine }),
    "engine_disposed",
    "freed",
  );
});

test("a `using` block releases the engine when it ends, and releasing twice frees nothing", () => {
  // `docs/wasm.md` opens with `using engine = new Engine()` and
  // `examples/personal-link.ts` compiles it, so the strict check proves `Engine` is
  // `Disposable` — but a type is not a release. Until this case nothing ran the
  // idiom: a `[Symbol.dispose]` that stops calling `free()`, or a runtime whose
  // `using` does not call it at block end, leaves every gate green while a long-lived
  // process leaks one native engine per request, which is the failure
  // `src/engine.ts`'s own JSDoc warns about. It runs under Node and under Bun, so the
  // lane declaring both runtimes is the one that proves the release happens in both.
  let released;
  {
    using engine = new Engine();
    assert.ok(engine.tzdbVersion.length > 0, "the engine answers from inside the block");
    released = engine;
  }
  // The block ended, so the handle is gone: the same report the explicit `free()`
  // above produces, from the release a `using` block performed instead.
  assertRejection(
    () => calendarViewToBlocks(CALENDAR_VIEW, "UTC", { engine: released }),
    "engine_disposed",
    "freed",
  );
  // `free()` is idempotent, so an owner that releases in a `finally` as well as with
  // `using` performs no second free and reaches no freed pointer.
  released.free();
  released.free();
  assertRejection(() => released.tzdbVersion, "engine_disposed", "freed");
});

test("a top-level `using` releases the engine when the module finishes evaluating", async () => {
  // The documentation tells a reader to keep `using` inside a function or a request
  // handler, because at ES module top level it disposes as soon as module evaluation
  // completes — before any exported handler runs. That sentence is the whole reason a
  // module-scope engine has to be `const engine = new Engine()`, so it is pinned by
  // running it: a module whose only statement after the declaration exports the engine
  // hands back a disposed handle, and one that exported a handler would find it freed
  // the first time the handler ran. The module has to be a real file — the runner
  // decides when evaluation ends — so it is written into a scratch directory inside
  // this package, where `opportune` resolves through the package's own name.
  const dir = mkdtempSync(join(import.meta.dirname, ".top-level-using-"));
  try {
    const module = join(dir, "engine.mjs");
    writeFileSync(
      module,
      [
        'import { Engine } from "opportune";',
        "using engine = new Engine();",
        "export const topLevel = engine;",
        "",
      ].join("\n"),
    );
    const imported = await import(pathToFileURL(module).href);
    assert.ok(
      imported.topLevel instanceof Engine,
      "the module exported the engine it built",
    );
    assertRejection(
      () => imported.topLevel.tzdbVersion,
      "engine_disposed",
      "freed",
    );
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

test("a release that fails still leaves the engine disposed", () => {
  // `Engine.free()` removes the handle entry in a `finally`, and this is the case that
  // needs it: the release fails, and the entry has to be gone anyway or the next call
  // reaches a handle that was destroyed under it. No public caller can make the release
  // fail — `free()` is the only code that calls it — so the failure is staged through
  // the package's own seam: release the native handle the engine holds, then ask the
  // engine to release it again. Whatever the release answers, it is this package's
  // error class, and the engine answers as disposed rather than reaching the handle.
  const engine = new Engine();
  binding(engine).free();
  try {
    engine.free();
  } catch (error) {
    assert.ok(
      error instanceof OpportuneError,
      `a failed release surfaced as ${error?.constructor?.name}: ${error?.message}`,
    );
  }
  assertRejection(() => engine.tzdbVersion, "engine_disposed", "freed");
  // Releasing again is a no-op rather than a second release of the same handle.
  engine.free();
});

test("a translated module rejection preserves the thrown payload as Error.cause", () => {
  const error = thrown(() => upgradeRule(STORED_RULE, 99));
  assert.ok(error instanceof OpportuneError);
  assert.equal(error.code, "unsupported_schema_version");
  assert.equal(typeof error.cause, "string");
  assert.equal(JSON.parse(error.cause).code, "unsupported_schema_version");
});

test("the UTF-8 check leaves a readable document unchanged", () => {
  // The guard is a shape check, not a restriction: the same documents without the
  // surrogate still convert, and still carry the provider's own IDs and keys.
  assert.equal(calendarViewToBlocks(ONE_GRAPH_EVENT, "UTC").blocks[0]?.event_id,
    "ana-interview-0900");
  assert.equal(eventsListToBlocks(ONE_GOOGLE_EVENT, "UTC").blocks[0]?.event_id,
    "ana-interview-0900");
  assert.deepEqual(Object.keys(getScheduleToBlocks(ONE_SCHEDULE)), ["ana@example.com"]);
  assert.deepEqual(Object.keys(freeBusyToBlocks(ONE_FREE_BUSY)), ["ana@example.com"]);
  assert.equal(workingHoursToRule(WORKING_HOURS).status, "available");
  assert.match(upgradeRule(STORED_RULE, 1), /"id":"freeze"/);
  // A document held as an *object* was never at risk: `JSON.stringify` escapes an
  // unpaired surrogate, and the wire parser rejects the escape. Pinned so the text
  // guard is not "fixed" by making the object path lossy too.
  const participant = { id: LONE_SURROGATE, zone: "UTC", calendar: [] };
  assert.match(JSON.stringify(participant), /\\ud800/);
});

/** A request the shared engine answers; the cases below change one field of it. */
const REQUEST = {
  now: "2027-06-14T07:00Z",
  goal: singleGoal("30m", { all: [person("rich")] }, {
    zone: "Europe/London",
    searchWindow: ["2027-06-13T23:00Z", "2027-06-18T23:00Z"],
  }),
  participants: [{ id: "rich", zone: "Europe/London", calendar: [] }],
  rules: [businessHours()],
};

test("a non-finite number is a rejection, never the `null` JSON writes", () => {
  // `JSON.stringify` spells `Infinity`, `-Infinity` and `NaN` as `null`, and `null` is
  // what a request means by "the engine's default": `limit: Infinity` would reach the
  // engine as "no limit" and answer an unbounded search for a caller who asked for a
  // bounded one. The number cannot cross, so the document cannot be written.
  for (const value of [Infinity, -Infinity, NaN]) {
    assertRejection(
      () => findAvailability({ ...REQUEST, limit: value }),
      "invalid_json",
      String(value),
    );
  }
  // Anywhere in the document, not only at its top level: the replacer sees every
  // member JSON writes.
  assertRejection(
    () => findAvailability({
      ...REQUEST,
      participants: [{ id: "rich", zone: "Europe/London", calendar: [{ from: Infinity }] }],
    }),
    "invalid_json",
    "from",
  );
  // The same guard covers the documents the other entry points write.
  assertRejection(
    () => checkSlot({ ...REQUEST, now: "2027-06-14T07:00Z" }, { events: [{ start: NaN }] }),
    "invalid_json",
    "start",
  );
  const badLimits = assertRejection(
    () => new Engine(limitsWith({ max_candidates: NaN })),
    "invalid_limits",
    "max_candidates",
  );
  assert.equal(badLimits.cause?.code, "invalid_json");
  assertRejection(
    () => calendarViewToBlocks({ value: [{ subject: "x", start: { dateTime: Infinity } }] }, "UTC"),
    "invalid_json",
    "dateTime",
  );
});

test("limits JSON would write as `{}` are refused, not answered with the defaults", () => {
  // Every field of a limits document is optional, so `{}` is a document the wire reads
  // as "all defaults". A `Map`, a `Set`, an instance of a class and an object whose
  // fields are on its prototype carry no own enumerable member, so `JSON.stringify`
  // writes exactly that — and the engine would run with none of the limits the caller
  // asked for. The plain object is the control: with the same field on the document
  // itself, the one participant over the bound *is* refused.
  const request = {
    ...REQUEST,
    goal: singleGoal("30m", { all: [person("rich"), person("ana")] }, {
      zone: "Europe/London",
      searchWindow: ["2027-06-13T23:00Z", "2027-06-18T23:00Z"],
    }),
    participants: [
      { id: "rich", zone: "Europe/London", calendar: [] },
      { id: "ana", zone: "Europe/London", calendar: [] },
    ],
  };
  using bounded = new Engine({ max_participants: 1 });
  const limited = thrown(() => bounded.findAvailability(request));
  assert.ok(limited instanceof OpportuneError, `the bounded engine answered: ${limited?.message}`);
  assert.equal(limited.code, "invalid_request");
  assert.ok(
    limited.errors.some((finding) => finding.code === "participants_limit"),
    `the plain limits document did not bound the request: ${JSON.stringify(limited.errors)}`,
  );

  const refused = [
    ["a Map", new Map([["max_participants", 1]]), "map"],
    ["a Set", new Set(["max_participants"]), "set"],
    ["an instance of a class", new (class Limits {})(), "limits"],
    ["a prototype-carried field", Object.create({ max_participants: 1 }), "object"],
  ];
  for (const [what, limits, named] of refused) {
    // `invalid_limits` is the code `Engine` documents, and it is the existing
    // configuration translation — not a new one — that turns the shape refusal into it.
    const error = assertRejection(
      () => new Engine(limits),
      "invalid_limits",
      `not a value of type ${named}`,
    );
    assert.equal(error.cause?.code, "invalid_type", what);
  }

  // The two "no argument" spellings keep meaning the engine's own defaults.
  using fromNull = new Engine(null);
  using fromUndefined = new Engine(undefined);
  assert.match(fromNull.tzdbVersion, /^\d{4}[a-z]$/);
  assert.match(fromUndefined.tzdbVersion, /^\d{4}[a-z]$/);
});

test("an offer the accessors cannot read is a rejection, not a TypeError", () => {
  // A lookup that missed, an offer read from a queue and never parsed: the accessors
  // are how an application reads the engine's answer, so a shape they cannot read has
  // to leave as the error the application already handles — never as a `TypeError`
  // from indexing `null`, and never as an `undefined` returned as if it were a start.
  const accessors = [offerStart, offerEnd, offerAssignedHosts, offerCalendarUnknown, offerReserve, offerToProposal];
  for (const access of accessors) {
    for (const [label, offer] of [
      ["null", null],
      ["undefined", undefined],
      ["a string", "an offer"],
      ["a list", []],
    ]) {
      const error = thrown(() => access(offer));
      assert.ok(
        error instanceof OpportuneError,
        `${access.name}(${label}): expected OpportuneError, got ${error?.constructor?.name}`,
      );
      assert.equal(error.code, "invalid_type", `${access.name}(${label})`);
    }
  }
  // A record that is not an offer, or whose `events` is not a list, is the same answer
  // for every accessor: every one of them reads `events`, so the field is named and
  // nothing is read off it.
  for (const offer of [{}, { events: null }, { events: "none" }, { events: 7 }]) {
    for (const access of accessors) {
      const label = `${access.name}(${JSON.stringify(offer)})`;
      const error = thrown(() => access(offer));
      assert.ok(error instanceof OpportuneError, label);
      assert.equal(error.code, "invalid_type", label);
    }
  }
  // `flags` is read by one accessor only, so the guard for it is where it is read: the
  // others answer from `events`, which these offers have.
  for (const offer of [{ events: [] }, { events: [], flags: null }]) {
    const error = thrown(() => offerCalendarUnknown(offer));
    assert.ok(error instanceof OpportuneError, "offerCalendarUnknown(no flags)");
    assert.equal(error.code, "invalid_type", "offerCalendarUnknown(no flags)");
  }
  // An offer that books nothing is the one shape with its own code: no engine answer
  // books an eventless offer, so it is an argument this layer rejects rather than a
  // document the wire would. An event that names no instant is the same answer: a
  // `string` accessor must not hand `undefined` back as if it were the start.
  for (const offer of [{ events: [], flags: [] }, { events: [{}], flags: [] }]) {
    assertRejection(() => offerStart(offer), "invalid_argument", "`start`");
    assertRejection(() => offerEnd(offer), "invalid_argument", "`end`");
  }
  // The lists inside an event are the next level down, and a bare `TypeError` from
  // `.flat()` on `undefined` is not a failure a caller's handler can answer.
  for (const [access, field] of [
    [offerAssignedHosts, "assignment"],
    [offerReserve, "reserve"],
  ]) {
    const error = thrown(() => access({ events: [{}], flags: [] }));
    assert.ok(error instanceof OpportuneError, `${access.name}: got ${error?.constructor?.name}`);
    assert.equal(error.code, "invalid_type", access.name);
    assert.match(error.message, new RegExp(field), access.name);
  }
  // A hole in the list is the shape a hand-written or foreign document holds: a
  // caller mapping a lookup that missed keeps the `null` where the event belongs, and
  // reading a field off it is a bare `TypeError` from this package's own code — the
  // very failure the accessors exist to answer, one level in from the list they check.
  for (const offer of [{ events: [null], flags: [] }, { events: [undefined], flags: [] }]) {
    for (const access of [
      offerStart,
      offerEnd,
      offerAssignedHosts,
      offerReserve,
      offerToProposal,
    ]) {
      const error = thrown(() => access(offer));
      assert.ok(
        error instanceof OpportuneError,
        `${access.name}(${JSON.stringify(offer)}): expected OpportuneError, got ` +
          `${error?.constructor?.name}: ${error?.message}`,
      );
      assert.equal(error.code, "invalid_type", `${access.name}(${JSON.stringify(offer)})`);
    }
  }
});

test("an Engine handed to an adapter where a document belongs is a rejection", () => {
  // The category map is a document of tag paths, and an `Engine` is an object too:
  // `JSON.stringify` writes it as `{}`, so a caller that passed one positionally —
  // where it used to go — would convert every event as if no category were mapped.
  // The options object is the same trap from the other side: an `Engine` is an object,
  // so without the check it would read as "every option absent" and silently answer
  // from the shared engine.
  const engine = new Engine();
  try {
    assertRejection(
      () => calendarViewToBlocks(CALENDAR_VIEW, "UTC", { categoryMap: engine }),
      "invalid_type",
      "categoryMap",
    );
    assertRejection(
      () => calendarViewToBlocks(CALENDAR_VIEW, "UTC", engine),
      "invalid_type",
      "calendarViewToBlocks",
    );
    for (const [name, call] of [
      ["getScheduleToBlocks", () => getScheduleToBlocks(ONE_SCHEDULE, engine)],
      ["workingHoursToRule", () => workingHoursToRule(WORKING_HOURS, engine)],
      ["eventsListToBlocks", () => eventsListToBlocks(EVENTS_LIST, "UTC", engine)],
      ["freeBusyToBlocks", () => freeBusyToBlocks(ONE_FREE_BUSY, engine)],
    ]) {
      assertRejection(call, "invalid_type", name);
    }
    // The object the engine does belong in is read as one, on every adapter.
    assert.equal(calendarViewToBlocks(ONE_GRAPH_EVENT, "UTC", { engine }).blocks.length, 1);
    assert.equal(eventsListToBlocks(ONE_GOOGLE_EVENT, "UTC", { engine }).blocks.length, 1);
    assert.deepEqual(Object.keys(getScheduleToBlocks(ONE_SCHEDULE, { engine })), ["ana@example.com"]);
    assert.deepEqual(Object.keys(freeBusyToBlocks(ONE_FREE_BUSY, { engine })), ["ana@example.com"]);
    assert.equal(workingHoursToRule(WORKING_HOURS, { engine }).status, "available");
  } finally {
    engine.free();
  }
});

test("the browser entry exposes the explicit initialisers instead of a stub", async () => {
  // A browser or Web Worker bundle resolves the manifest's `browser`/`worker`
  // condition to the shared public entry, whose `#wasm` loader is `wasm-web.js`. What
  // the entry exposes is the one step a caller has to take: `init()` and `initSync()`,
  // with `init` as the default. The loader's own pre-init rejection and retry state
  // machine are exercised by `web_loader.test.mjs`, which owns that module state in its
  // own process: under Bun every test file shares one module registry, so calling the
  // in-process web loader here would race that suite's initialisation.
  const web = await import("../dist/web.js");
  assert.equal(typeof web.init, "function", "opportune/web exports init");
  assert.equal(typeof web.initSync, "function", "opportune/web exports initSync");
  assert.equal(web.default, web.init, "opportune/web's default export is init");
});

test("the error class is one class across the entry and the pure subpath", () => {
  // `opportune/presets` exports `OpportuneError` so a bundle can catch what a builder
  // raises, and `opportune` exports it so a Node caller can catch what the engine
  // raises. An isomorphic codebase uses one import on both sides, and its `catch`
  // reaches the engine's failures only while the two specifiers name one class object:
  // a second copy would satisfy nothing but its own `instanceof`, and every `catch`
  // written against the subpath's class would let the engine's error through as a raw
  // exception. The preset's own rejection is the same check from the other side.
  assert.equal(
    PresetError,
    OpportuneError,
    "`opportune/presets` exports a different class than `opportune`",
  );
  const engineRaised = (() => {
    try {
      findAvailability({});
    } catch (error) {
      return error;
    }
    return assert.fail("a request with no goal was answered");
  })();
  assert.equal(engineRaised.code, "missing_field");
  assert.ok(
    engineRaised instanceof PresetError,
    `an engine rejection is not a \`opportune/presets\` ${String(engineRaised?.constructor?.name)}`,
  );
  const presetRaised = (() => {
    try {
      presetBusinessHours("mon-fri", null);
    } catch (error) {
      return error;
    }
    return assert.fail("a wall-clock range of `null` built a rule");
  })();
  assert.ok(
    presetRaised instanceof OpportuneError,
    "a builder's rejection is not a `opportune` OpportuneError",
  );
});

test("the constants the package states without the module are the module's own values", () => {
  // `constants.ts` states them so that importing the package — or an adapter, for the
  // `$expand` value — loads no WebAssembly, which makes each a second copy of a value
  // only the compiled module knows. These are the comparisons that keep the copies
  // honest, in the suite both runtimes run: every marker and extended-property string
  // the adapters hand a caller is pinned here, not only the two the package entry
  // re-exports. `#wasm` is the loader the package itself resolves, so this compares
  // the constants against the module the engine these tests call actually loaded.
  const wasm = wasmModule();
  assert.equal(VERSION, wasm.version());
  assert.equal(MARKER_TAGS, wasm.marker_tags());
  assert.equal(MARKER_EVENT_KEY, wasm.marker_event_key());
  assert.equal(EXTENDED_PROPERTY_NAMESPACE_GUID, wasm.graph_extended_property_namespace_guid());
  assert.equal(EXTENDED_PROPERTY_TAGS_ID, wasm.graph_extended_property_tags_id());
  assert.equal(EXTENDED_PROPERTY_EVENT_KEY_ID, wasm.graph_extended_property_event_key_id());
  assert.equal(EXTENDED_PROPERTY_EXPAND, wasm.graph_extended_property_expand());
});
