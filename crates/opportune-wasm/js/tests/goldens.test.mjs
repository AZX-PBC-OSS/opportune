/**
 * Every committed Rust golden, answered by the built package byte for byte.
 *
 * The corpus under `crates/opportune-testkit/goldens` is written by the Rust
 * testkit, and each file stores its answers — `response`, `slot_check`,
 * `required_ranges` — as the exact text `opportune_json::to_json` wrote. This
 * suite replays every file through the published package's JSON entry points and
 * compares the answer with that text unchanged: no parse, no re-serialise, so a
 * difference in key order, number spelling or separators is a failure rather than
 * a detail the comparison would normalise away.
 *
 * A golden is the Rust engine's own answer to the same request the testkit ran, so
 * this is parity against Rust rather than Node against itself. The truncation
 * goldens are in the corpus for the same reason: a limit that cuts the answer must
 * cut it in the same place with the same flags.
 *
 * The package is driven through its public entry points, from `dist/`, so the
 * bytes compared here are the ones a consumer receives. Build it first:
 *
 * ```sh
 * npm --prefix crates/opportune-wasm/js test
 * ```
 */

import assert from "node:assert/strict";
import { readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";
import test from "node:test";

import {
  Engine,
  VERSION,
  checkSlotJson,
  findAvailabilityJson,
  requiredRangesJson,
  validateJson,
} from "../dist/index.js";

/** The committed corpus, beside the crate that writes it. */
const GOLDENS = join(import.meta.dirname, "../../../opportune-testkit/goldens");

/** Every use case and rule kind the corpus is expected to carry. */
const USE_CASES = Array.from({ length: 22 }, (_, index) => `U${index + 1}`);
const RULE_KINDS = ["window", "blackout", "clearance", "lead_time", "status_policy", "rest", "cap"];

/**
 * The corpus, read once: each golden's file name beside its document.
 *
 * A golden the suite cannot name is a golden nothing verifies, so the directory is
 * the list rather than a hand-kept array.
 */
const CORPUS = readdirSync(GOLDENS)
  .filter((name) => name.endsWith(".json"))
  .sort()
  .map((name) => ({
    name,
    golden: JSON.parse(readFileSync(join(GOLDENS, name), "utf8")),
  }));

/**
 * The engine every golden is answered by: one build, one bundled database, one
 * request limit set, which is what makes the corpus a comparison rather than a
 * series of separate runs.
 */
const engine = new Engine();

/** The first offset at which two texts differ, or `-1` when they are equal. */
function firstDifference(actual, expected) {
  const length = Math.min(actual.length, expected.length);
  for (let index = 0; index < length; index += 1) {
    if (actual[index] !== expected[index]) {
      return index;
    }
  }
  return actual.length === expected.length ? -1 : length;
}

/** The text around `offset`, so a failure names the byte it is about. */
function excerpt(text, offset) {
  const from = Math.max(0, offset - 40);
  return `${JSON.stringify(text.slice(from, offset + 40))} (from offset ${from})`;
}

/** `actual` is the same text as `expected`, byte for byte, or the test fails. */
function assertSameBytes(actual, expected, what) {
  const offset = firstDifference(actual, expected);
  if (offset === -1) {
    return;
  }
  assert.fail(
    `${what} differs at offset ${offset} of ${expected.length}:\n` +
      `expected ${excerpt(expected, offset)}\n` +
      `received ${excerpt(actual, offset)}`,
  );
}

/**
 * Substitutes the current package `VERSION` for the golden's recorded `meta.engine`
 * so release bumps do not break byte-for-byte golden replay while `meta.engine` is
 * still verified explicitly against `VERSION`.
 */
function forCurrentEngine(goldenJson) {
  const recorded = JSON.parse(goldenJson).meta?.engine;
  assert.equal(typeof recorded, "string", "golden answer must carry string meta.engine");
  const needle = `"engine":${JSON.stringify(recorded)}`;
  const parts = goldenJson.split(needle);
  assert.equal(parts.length, 2, `expected exactly one ${needle} in golden answer`);
  return `${parts[0]}"engine":${JSON.stringify(VERSION)}${parts[1]}`;
}

for (const { name, golden } of CORPUS) {
  test(`${name}: the response is the golden's own bytes`, () => {
    const actual = engine.findAvailabilityJson(golden.request);
    assert.equal(
      JSON.parse(actual).meta.engine,
      VERSION,
      `${name}: response meta.engine must match package VERSION`,
    );
    assertSameBytes(actual, forCurrentEngine(golden.response), "the response");
  });
}

for (const { name, golden } of CORPUS.filter(({ golden }) => golden.slot_check !== undefined)) {
  test(`${name}: the slot check is the golden's own bytes`, () => {
    const actual = engine.checkSlotJson(golden.request, golden.proposal);
    assert.equal(
      JSON.parse(actual).meta.engine,
      VERSION,
      `${name}: slot_check meta.engine must match package VERSION`,
    );
    assertSameBytes(actual, forCurrentEngine(golden.slot_check), "the slot check");
  });
}

for (const { name, golden } of CORPUS.filter(({ golden }) => golden.required_ranges !== undefined)) {
  test(`${name}: the required ranges are the golden's own bytes`, () => {
    assertSameBytes(
      engine.requiredRangesJson(golden.ranges_query),
      golden.required_ranges,
      "the required ranges",
    );
  });
}

test("the free JSON functions answer the text the engine they share answers", () => {
  // The four `*Json` entry points exist as `Engine` methods and as free functions over
  // the shared engine, which is what a script or a route that takes the text calls
  // without building an engine. The pair has to be the same answer, not merely the same
  // shape: the free function hands on the module's own bytes for the request, so a
  // free function that parsed and re-serialised would show up here as different text
  // than the engine's.
  const ask = CORPUS.find(({ golden }) => golden.required_ranges !== undefined);
  const confirm = CORPUS.find(({ golden }) => golden.slot_check !== undefined);
  assert.ok(ask !== undefined && confirm !== undefined, "the corpus carries both an ask and a confirm");

  assertSameBytes(
    findAvailabilityJson(ask.golden.request),
    engine.findAvailabilityJson(ask.golden.request),
    "the free response",
  );
  assertSameBytes(
    checkSlotJson(confirm.golden.request, confirm.golden.proposal),
    engine.checkSlotJson(confirm.golden.request, confirm.golden.proposal),
    "the free slot check",
  );
  assertSameBytes(
    validateJson(ask.golden.request),
    engine.validateJson(ask.golden.request),
    "the free diagnostics",
  );
  // A ranges query is the document the free function takes, and it has to answer the
  // same bytes as the engine's own entry point rather than a re-serialised copy.
  assertSameBytes(
    requiredRangesJson(ask.golden.ranges_query),
    engine.requiredRangesJson(ask.golden.ranges_query),
    "the free required ranges",
  );
});

test("the corpus still covers every use case, rule kind and extra document", () => {
  // A corpus that shrank would leave this suite green with less to prove, so the
  // cases the initiative names are checked for rather than assumed.
  const covered = new Set(CORPUS.map(({ golden }) => golden.use_case));
  const missing = [...USE_CASES, ...RULE_KINDS].filter((useCase) => !covered.has(useCase));
  assert.deepEqual(missing, [], `the corpus no longer covers ${missing.join(", ")}`);

  assert.ok(
    CORPUS.some(({ golden }) => golden.slot_check !== undefined),
    "no golden carries a proposal to confirm",
  );
  assert.ok(
    CORPUS.some(({ golden }) => golden.required_ranges !== undefined),
    "no golden carries a required-ranges query",
  );
});

test("the truncation goldens reach this suite and their answers really truncate", () => {
  // The limit is what the truncation cases pin, and a corpus that lost them would
  // still pass every remaining byte comparison.
  const truncated = CORPUS.flatMap(({ name, golden }) => {
    const response = JSON.parse(golden.response);
    return response.truncated || response.excluded_truncated ? [{ name, response }] : [];
  });
  assert.ok(truncated.length >= 2, "the offers and the explanations each need a truncating golden");
  assert.ok(
    truncated.some(({ response }) => response.truncated === true),
    "no golden truncates its offers",
  );
  assert.ok(
    truncated.some(({ response }) => response.excluded_truncated === true),
    "no golden truncates its explanations",
  );
});
