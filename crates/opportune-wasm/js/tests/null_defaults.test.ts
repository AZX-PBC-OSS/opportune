/**
 * The document a binding that writes every key produces, under the package's
 * strict compile.
 *
 * A JSON mapping that fills every key in — several do — writes an explicit `null`
 * where a human would leave a field out, and a request read back from a stored JSON
 * document is the common case: the store returns what the mapper stored. So the
 * generated request type has to accept those nulls with no cast, and the engine
 * has to answer the document it already answers without them.
 *
 * The runtime half of that contract is `errors_and_wasm32.test.mjs`'s explicit-null
 * case, which writes the same document where no typechecker reads it. This is the
 * twin the package's strict compile covers: the document below is typed as
 * {@link Request}, so a generated type that stopped admitting `null` on a defaulted
 * field fails `npm run check` here rather than in a consumer's build. The
 * assertions are the twin's, so the two surfaces cannot describe different
 * documents.
 *
 * Both halves run against the built package, so the import below is the package's
 * published name: `node --test` and `bun test` resolve it through `dist/`, while
 * `npm run check` resolves it to `src/` (see `tsconfig.check.json`), which is what
 * lets the typechecker read this file without a wasm build.
 */

import assert from "node:assert/strict";
import test from "node:test";

import { Engine, type Request } from "opportune";

/**
 * A request with `null` on every field the specification defaults.
 *
 * Each null is the field's own default: an empty list or map, `busy`, not
 * all-day, every host, every booking, and no hours in a window.
 */
const EVERY_DEFAULT_NULL: Request = {
  now: "2027-03-09T15:00Z",
  goal: {
    parts: [
      {
        tags: null,
        duration: "30m",
        requirement: { all: [{ person: "ana" }] },
        travel: { before: "0m", after: "0m", by_participant: null },
      },
    ],
    search_window: ["2027-03-09T15:00Z", "2027-03-09T16:00Z"],
    zone: "UTC",
  },
  participants: [
    {
      id: "ana",
      zone: "UTC",
      tags: null,
      zone_periods: null,
      calendar: [
        { span: ["2027-03-09T15:00Z", "2027-03-09T16:00Z"], status: null, all_day: null, tags: null },
      ],
    },
  ],
  rules: [
    { id: "w", kind: "window", who: null, when: null, params: { zone: "UTC", weekly: null, dates: null } },
  ],
  occurrences: null,
  exclude_event_keys: null,
  calendars: { org: { zone: "UTC", working_days: "mon-fri", holidays: null } },
  weights: null,
};

test("a typed request carrying null on every defaulted field is the document the engine answers", () => {
  const engine = new Engine();

  const answer = engine.findAvailability(EVERY_DEFAULT_NULL);
  assert.deepEqual(answer.offers, [], "the nulled window shares no hours");
  assert.deepEqual(
    answer.dates,
    [{ date: "2027-03-09", offers: 0 }],
    "the request parsed, so the search ran over its own window",
  );
  assert.deepEqual(
    answer.warnings.map((warning) => [warning.code, warning.path]),
    [["no_candidate_in_window", "/goal/search_window"]],
  );

  // A list written as `null` is the empty list: a participant written that way is a
  // participant the goal no longer names, not a document that fails to read.
  const withoutLists = engine.validate({
    ...EVERY_DEFAULT_NULL,
    participants: null,
    rules: null,
    occurrences: null,
    calendars: null,
    weights: null,
  });
  assert.deepEqual(
    withoutLists.errors.map((finding) => [finding.code, finding.path]),
    [["participant_unknown", "/goal/parts/0/requirement/all/0"]],
  );

  // An occurrence's tag list is a defaulted field the request-level nulls above
  // cannot reach, because that request carries no occurrence at all.
  const occurred = engine.validate({
    ...EVERY_DEFAULT_NULL,
    occurrences: [
      {
        event_key: "e",
        span: ["2027-03-09T15:00Z", "2027-03-09T15:30Z"],
        tags: null,
        participants: ["ana"],
      },
    ],
  });
  assert.deepEqual(occurred.errors, [], "a null tag list on an occurrence is an empty one");
  assert.deepEqual(
    occurred.warnings.map((warning) => [warning.code, warning.path]),
    [["no_candidate_in_window", "/goal/search_window"]],
  );
});

test("an engine built with a null limits document is the engine built with none", () => {
  // `null` is the value a variable holds when a caller has nothing to configure, and
  // the parameter is optional, so `new Engine(null)` is the "no argument" a JavaScript
  // caller writes. It has to be the engine's own default limits — the document the
  // module would otherwise read as a request to build an engine from `null`.
  const defaulted = new Engine(null);
  const bare = new Engine();
  assert.equal(defaulted.tzdbVersion, bare.tzdbVersion, "the two engines are built from one database");
  assert.deepEqual(
    defaulted.findAvailability(EVERY_DEFAULT_NULL).offers,
    bare.findAvailability(EVERY_DEFAULT_NULL).offers,
    "a null limits document configures the same engine as no limits argument",
  );
});
