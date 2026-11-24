/**
 * The adoption bugs a Node/Bun service meets on its way in, pinned one per case.
 *
 * Each of these was a way for a correct-looking integration to be wrong without an
 * error: a cached answer projected into a document typed `Meta` with an `undefined`
 * where a string belongs, an options object that is a `Map` read as "no options" and
 * so as every default, a provider payload the server itself fetched answered `422`
 * as though the visitor had sent it, a `null` from a variable that may be nothing
 * refused where the same `null` is accepted everywhere else, a preset's rejection
 * naming a rule ID instead of the function the caller called, a bundled deployment
 * told to reinstall a package that is installed, and an `Engine` from a second copy
 * of the package refused by the first copy's adapters.
 *
 * The module-graph case runs the real loader against a copy of `dist/`, because the
 * one thing that cannot be simulated is which file `require` resolves beside.
 *
 * Run against the built package:
 *
 * ```sh
 * make wasm && node --test crates/opportune-wasm/js/tests/adoption_fixes.test.mjs
 * ```
 */

import assert from "node:assert/strict";
import { cpSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import { pathToFileURL } from "node:url";

import {
  Engine,
  OpportuneError,
  businessHours,
  capPerDay,
  capPerMonth,
  capPerWeek,
  httpStatusForError,
  limitsWith,
  publicResponse,
  publicSlotCheck,
  singleGoal,
  weeklyHours,
} from "../dist/index.js";
import { calendarViewToBlocks } from "../dist/adapters/graph.js";

/** The package root this suite belongs to, the built tree, and the glue the loader resolves. */
const PACKAGE = join(import.meta.dirname, "..");
const DIST = join(PACKAGE, "dist");
const GLUE = join("pkg", "opportune_wasm.cjs");
const WASM = join("pkg", "opportune_wasm_bg.wasm");

/** The `meta` every engine answer carries, and the projection now demands. */
const META = { engine: "0.1.0", schema: 1, tzdb: "2026a" };

/** One Graph `calendarView` event, as `@microsoft/microsoft-graph-types` declares it. */
const CALENDAR_VIEW = {
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
};

/**
 * The rejection `call` raises, as the package's error.
 *
 * Its shape is checked rather than `instanceof`, because two cases below import a
 * copy of the package from a temporary directory: that copy's class is not this
 * copy's class, and what a caller catches is the shape either way.
 */
function thrown(call) {
  try {
    call();
  } catch (error) {
    assert.equal(
      error?.name,
      "OpportuneError",
      `expected OpportuneError, got ${error?.constructor?.name}: ${error?.message}`,
    );
    assert.equal(typeof error.code, "string", "the rejection carries a code");
    return error;
  }
  return assert.fail("expected the call to reject");
}

/**
 * A copy of `dist/` in a temporary directory, after `damage` ran on it.
 *
 * The copy's own modules are what the cases import, so its loader resolves beside
 * the copy rather than the checked-in tree — which is the only way to reach the
 * failures a deployment sees. The manifest travels with it because the loader is
 * selected by `package.json#imports`, which a bare `dist/` copy has no `#wasm` for.
 */
async function damaged(damage, check) {
  const dir = mkdtempSync(join(tmpdir(), "opportune-adoption-"));
  try {
    cpSync(DIST, join(dir, "dist"), { recursive: true });
    cpSync(join(PACKAGE, "package.json"), join(dir, "package.json"));
    damage(join(dir, "dist"));
    return await check((name) => import(pathToFileURL(join(dir, "dist", name)).href));
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
}

test("a cached answer whose `meta` lost a field is a rejection, not an undefined", () => {
  // A `Response` read back from a JSON column, a queue, or an HTTP cache is a
  // document someone else wrote. `Meta` is three required fields, so the projection
  // reads all three: dropping one used to copy the rest and cast the result, which
  // left a visitor's route printing `undefined` for `meta.tzdb` under a type that
  // promises a string.
  const answer = {
    dates: [{ date: "2027-06-14", offers: 1 }],
    meta: { engine: "0.1.0", schema: 1 },
    offers: [
      {
        events: [{ part: "main", start: "2027-06-14T09:00Z", end: "2027-06-14T09:30Z" }],
        flags: [],
      },
    ],
    truncated: false,
  };
  const missing = thrown(() => publicResponse(answer));
  assert.equal(missing.code, "invalid_type");
  assert.equal(missing.path, null);
  assert.match(missing.message, /`meta\.tzdb`/);

  // The same for the other two fields, and for a `schema` that is not a whole
  // number: a float or a `NaN` is not a version.
  for (const [meta, field] of [
    [{ schema: 1, tzdb: "2026a" }, "meta.engine"],
    [{ engine: "0.1.0", tzdb: "2026a" }, "meta.schema"],
    [{ engine: "0.1.0", schema: 1.5, tzdb: "2026a" }, "meta.schema"],
    [{ engine: "0.1.0", schema: Number.NaN, tzdb: "2026a" }, "meta.schema"],
    [{ engine: "0.1.0", schema: 1, tzdb: 7 }, "meta.tzdb"],
  ]) {
    const error = thrown(() => publicSlotCheck({ flags: [], meta, valid: true }));
    assert.equal(error.code, "invalid_type", field);
    assert.match(error.message, new RegExp(`\`${field}\``), field);
  }

  // The control: the document with all three projects unchanged, so the rejections
  // above are the missing field rather than a projection that refuses everything.
  assert.deepEqual(publicResponse({ ...answer, meta: META }).meta, META);
  assert.deepEqual(publicSlotCheck({ flags: [], meta: META, valid: true }).meta, META);
});

test("options that are not plain mappings are refused rather than read as defaults", () => {
  // `asRecord` accepts any object, so a `Map`, a `Set` and a `Date` used to pass as
  // an options object and be read as one with every field absent. The caller who
  // wrote `new Map([["max_search_window_days", 5]])` then ran with the limits it
  // asked to tighten, silently; these are the rejections that say so.
  const notOptions = [
    [new Map([["max_search_window_days", 5]]), "a map"],
    [new Set(["max_search_window_days"]), "a set"],
    [new Date(0), "a date"],
    [Object.create({ zone: "UTC" }), "an instance"],
  ];
  for (const [value, named] of notOptions) {
    const limits = thrown(() => limitsWith(value));
    assert.equal(limits.code, "invalid_type");
    assert.match(limits.message, /limitsWith/);
    assert.ok(
      limits.message.includes(named),
      `the message names the value as ${named}: ${limits.message}`,
    );

    const preset = thrown(() => businessHours("mon-fri", "09:00-17:00", value));
    assert.equal(preset.code, "invalid_type");
    assert.match(preset.message, /businessHours/);

    const goal = thrown(() => singleGoal("30m", { all: [] }, value));
    assert.equal(goal.code, "invalid_type");
    assert.match(goal.message, /singleGoal/);
  }

  // A null-prototype mapping is still a mapping — Node's `querystring.parse` and a
  // document built with `Object.create(null)` are how a caller keeps a key such as
  // `__proto__` — so it stays accepted, and the same holds for a frozen literal.
  assert.ok(Object.keys(limitsWith(Object.create(null))).length > 0);
  assert.equal(businessHours("mon-fri", "09:00-17:00", Object.freeze({ zone: "UTC" })).params.zone, "UTC");
});

test("a provider payload the application fetched is a server failure, not a 422", () => {
  // The route asked Graph or Google for the payload; the visitor did not send it. A
  // document the adapter cannot read is the deployment's integration failing, which
  // is the line the Python binding draws, so HTTP middleware answers `500`.
  assert.equal(httpStatusForError(new OpportuneError("payload_invalid", "not a payload")), 500);
  // The codes that are still the caller's own document keep answering `422`, so the
  // classification above is one code rather than "everything the adapters raise".
  assert.equal(httpStatusForError(new OpportuneError("invalid_request", "bad")), 422);
  assert.equal(httpStatusForError(new OpportuneError("invalid_json", "bad")), 422);
  assert.equal(httpStatusForError(new OpportuneError("invalid_type", "bad")), 422);
});

/** The module that states the codes, read as source because a union is not a value. */
const VOCABULARY = join(import.meta.dirname, "..", "src", "vocabulary.ts");

/**
 * Every code `OpportuneErrorCode` states, and the status the package answers with.
 *
 * `500` is the server's own failure — the deployment, the compiled module, the
 * engine's configuration, or a provider payload the application fetched. `422` is a
 * document the caller sent, with the pointer saying which field. The map is
 * exhaustive by construction: the case below compares its keys against the union's
 * own literals, so a code added to `vocabulary.ts` without a status here fails
 * instead of silently defaulting.
 */
const HTTP_STATUS = {
  // The server or deployment: nothing a caller's document can fix.
  argument_not_utf8: 500,
  engine_disposed: 500,
  invalid_argument: 500,
  invalid_limits: 500,
  payload_invalid: 500,
  serialization_failed: 500,
  tzdb_bundled_unavailable: 500,
  unsupported_runtime: 500,
  unsupported_schema_version: 500,
  wasm_error: 500,
  wasm_unavailable: 500,
  // The caller's own document: the wire layer refused it, and `path` names where.
  duplicate_field: 422,
  invalid_json: 422,
  invalid_length: 422,
  invalid_request: 422,
  invalid_shape: 422,
  invalid_type: 422,
  invalid_value: 422,
  missing_field: 422,
  unknown_field: 422,
  unknown_variant: 422,
};

/**
 * The `tzdb_*` deployment failures the classification also answers `500`.
 *
 * These are not literals of `OpportuneErrorCode`: the union states the one the
 * compiled module can raise (`tzdb_bundled_unavailable`) and the rest are the native
 * engine's, which arrive through the open `(string & {})` tail when a service runs
 * the Rust or Python engine beside this one. They are pinned so the server set does
 * not quietly lose them.
 */
const SERVER_ONLY = [
  "tzdb_directory_unusable",
  "tzdb_not_configured",
  "tzdb_version_missing",
  "tzdb_version_without_directory",
];

/**
 * The literals `vocabulary.ts` states for `OpportuneErrorCode`.
 *
 * Read from the source rather than imported, because the union ends in
 * `(string & {})`: no value enumerates it, and the point of the case is that the
 * list is the declaration's rather than a copy of it.
 */
function declaredCodes() {
  const source = readFileSync(VOCABULARY, "utf8");
  const start = source.indexOf("export type OpportuneErrorCode =");
  assert.notEqual(start, -1, "vocabulary.ts declares `OpportuneErrorCode`");
  const end = source.indexOf(";", start);
  assert.notEqual(end, -1, "the `OpportuneErrorCode` union ends with a semicolon");
  return [...source.slice(start, end).matchAll(/^\s*\|\s*"([a-z0-9_]+)"/gm)].map(
    ([, code]) => code,
  );
}

test("every error code the package states answers the status it is classified as", () => {
  // The classification is what HTTP middleware branches on, so a code that moved
  // between the two answers is a client-facing behaviour change: a client told `500`
  // retries, one told `422` learns its document was wrong. Every code the package can
  // carry is pinned rather than the handful a reviewer remembers.
  const declared = declaredCodes();
  assert.notDeepEqual(declared, [], "the union's literals were read from `vocabulary.ts`");
  assert.deepEqual(
    [...declared].sort(),
    Object.keys(HTTP_STATUS).sort(),
    "`OpportuneErrorCode` states a code this case does not classify, or classifies one it no longer states",
  );
  for (const [code, status] of Object.entries(HTTP_STATUS)) {
    assert.equal(httpStatusForError(new OpportuneError(code, "a rejection")), status, code);
  }
  for (const code of SERVER_ONLY) {
    assert.equal(httpStatusForError(new OpportuneError(code, "a rejection")), 500, code);
  }
  // The open tail: a code a later release adds answers `422`, the caller's document by
  // default, which is what lets an older package read a newer answer without casting.
  assert.equal(
    httpStatusForError(new OpportuneError("a_code_from_a_later_release", "a rejection")),
    422,
  );
  // A value that is not an error at all is nobody's document, so it is the server's.
  for (const value of [null, undefined, {}, "invalid_request"]) {
    assert.equal(httpStatusForError(value), 500, JSON.stringify(value) ?? "undefined");
  }
});

test("`null` on an optional builder field is that field's default", () => {
  // `Engine(limits)` documents `null` as "the no-argument a caller reaches for", and
  // the adapters take `categoryMap: null`. A variable that may be nothing is the
  // same variable here, so a `null` is the default rather than the rejection every
  // other value of that type raises.
  assert.deepEqual(weeklyHours({ mon: "09:00-17:00" }, { dates: null, zone: null }), {
    id: "weekly_hours",
    kind: "window",
    params: { zone: "participant", weekly: { mon: ["09:00-17:00"] } },
  });
  assert.deepEqual(
    weeklyHours({ mon: "09:00-17:00" }, { dates: null }).params.dates,
    undefined,
    "a null override is an absent one, not an empty mapping",
  );

  const goal = singleGoal("30m", { all: [] }, {
    zone: "UTC",
    searchWindow: ["2027-06-14T08:00Z", "2027-06-14T18:00Z"],
    candidates: null,
    displayZone: null,
    grid: null,
    partId: null,
    tags: null,
    travel: null,
  });
  assert.deepEqual(goal, {
    parts: [{ duration: "30m", requirement: { all: [] } }],
    search_window: ["2027-06-14T08:00Z", "2027-06-14T18:00Z"],
    zone: "UTC",
  });

  // The control: a value that is not `null` and not absent still reaches the
  // document, so the cases above are the `null` rather than a builder that dropped
  // every optional field.
  assert.deepEqual(singleGoal("30m", { all: [] }, {
    zone: "UTC",
    searchWindow: ["2027-06-14T08:00Z", "2027-06-14T18:00Z"],
    partId: "main",
    tags: "interview",
  }).parts, [{ id: "main", duration: "30m", requirement: { all: [] }, tags: ["interview"] }]);
});

test("a cap's options rejection names the function the caller called", () => {
  // The message used to name the rule ID the preset would have written
  // (`cap_per_day`), which is a wire spelling no caller typed: the function in the
  // stack is the one they can look up.
  for (const [call, name] of [
    [() => capPerDay({ tag: "x" }, 2, null), "capPerDay"],
    [() => capPerWeek({ tag: "x" }, 2, "nope"), "capPerWeek"],
    [() => capPerMonth({ tag: "x" }, 2, new Map()), "capPerMonth"],
  ]) {
    const error = thrown(call);
    assert.equal(error.code, "invalid_type");
    assert.match(error.message, new RegExp(`^${name} options are an object`));
    assert.doesNotMatch(error.message, /cap_per_(?:day|week|month)/, error.message);
  }
  // The pooled-cap rejection is about the rule's own ID, so that one still names it.
  const pooled = thrown(() => capPerDay({ tag: "x" }, 2, { per: "pooled", ruleId: "interview_cap" }));
  assert.match(pooled.message, /interview_cap/);
});

test("an Engine from a second copy of the package is recognised, not refused", () => {
  // Two copies in one `node_modules` tree — a monorepo with two versions, a store
  // that did not dedupe — do not share a constructor, so `instanceof` is false
  // across them and `{ engine }` used to be rejected as `invalid_type` while it
  // already *is* an engine. The brand comes from the global symbol registry, which
  // both copies agree on, so the adapter answers from the engine it was handed.
  return damaged(
    () => {},
    async (load) => {
      const copy = await load("index.js");
      const foreign = new copy.Engine();
      assert.ok(foreign instanceof copy.Engine);
      assert.ok(
        foreign instanceof Engine === false,
        "the copy's Engine is not this copy's class, so the case is the cross-copy one",
      );

      const outcome = calendarViewToBlocks(CALENDAR_VIEW, "UTC", { engine: foreign });
      assert.equal(outcome.status, "available");
      assert.equal(outcome.blocks.length, 1);

      // The category map is a document, and an `Engine` handed there is the other
      // silent answer: `JSON.stringify` writes it as `{}`, so the conversion would
      // proceed with no category mapped. The same check catches a foreign one, so
      // `instanceof` is not the only thing standing between a caller and that.
      const refusedMap = thrown(() =>
        calendarViewToBlocks(CALENDAR_VIEW, "UTC", { categoryMap: foreign }),
      );
      assert.equal(refusedMap.code, "invalid_type");
      assert.match(refusedMap.message, /categoryMap/);

      // Releasing it through the other copy's `free` disposes it for both: the
      // handle is one property, not two registries that could disagree.
      foreign.free();
      const disposed = thrown(() => calendarViewToBlocks(CALENDAR_VIEW, "UTC", { engine: foreign }));
      assert.equal(disposed.code, "engine_disposed");
      assert.match(disposed.message, /freed/);

      // A duck-typed object that merely looks like an engine carries no brand, so
      // the check stayed a brand rather than "anything with a free method".
      const impostor = { [Symbol.for("opportune.engine")]: false, free() {} };
      const refused = thrown(() => calendarViewToBlocks(CALENDAR_VIEW, "UTC", { engine: impostor }));
      assert.equal(refused.code, "invalid_type");
      assert.match(refused.message, /must be an Engine/);
    },
  );
});

test("a module the bundler did not copy says how to keep the package external", async () => {
  // The module is present in `node_modules` and the bundler wrote one output file,
  // so `require` resolves beside the bundle and finds nothing: reinstalling and
  // rebuilding both succeed and change nothing. That is the failure a serverless
  // deployment meets first, and the one the reinstall advice cannot fix.
  await damaged(
    (dir) => rmSync(join(dir, GLUE)),
    async (load) => {
      const pkg = await load("index.js");
      const error = thrown(() => new pkg.Engine());
      assert.equal(error.code, "wasm_unavailable");
      assert.match(error.message, /Cannot find module/, "the runtime's own words are kept");
      assert.match(error.message, /npm install opportune/, "the reinstall advice stays");
      assert.match(error.message, /make wasm/, "the source-checkout advice stays");
      assert.match(error.message, /serverExternalPackages/, "Next.js is named");
      assert.match(error.message, /external: \["opportune"\]/, "esbuild, Vite and Bun are named");
    },
  );

  // The other load failure — a file that is present and is not a module — keeps the
  // reinstall advice alone: nothing was missing, so no bundler could have moved it.
  await damaged(
    (dir) => writeFileSync(join(dir, WASM), "not a wasm module"),
    async (load) => {
      const pkg = await load("index.js");
      const error = thrown(() => new pkg.Engine());
      assert.equal(error.code, "wasm_unavailable");
      assert.match(error.message, /npm install opportune/);
      assert.doesNotMatch(error.message, /serverExternalPackages/, error.message);
    },
  );
});
