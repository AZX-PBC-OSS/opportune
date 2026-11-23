/**
 * The wire surface's rejections and the counts a 32-bit build must read as a
 * 64-bit one does.
 *
 * A JavaScript caller reaches the engine by handing it documents, so what it can
 * get wrong is the document's shape, the rule inside it, the provider payload, or a
 * count too large for the pointer width this build compiles to. Each of those has
 * one answer: an `OpportuneError` carrying the wire layer's own `code`, the pointer
 * of the field at fault in `path`, and — when the request is invalid rather than
 * unreadable — the findings in `errors`. Nothing here may surface as a raw
 * `TypeError` or as a plausible empty answer.
 *
 * The review's first focus is the last group: `limit: 4294967296` is a request
 * naming four billion offers, which a 32-bit `usize` cannot hold. Reading the wire
 * value as `u64` and saturating into the domain's `usize` is what makes the same
 * document mean the same thing in a browser and on a 64-bit server.
 *
 * Run against the built package, so build it first:
 *
 * ```sh
 * npm --prefix crates/opportune-wasm/js test
 * ```
 */

import assert from "node:assert/strict";
import test from "node:test";

import {
  Engine,
  OpportuneError,
  defaultLimits,
  httpStatusForError,
  limitsWith,
  requiredRanges,
  requiredRangesJson,
  rest,
  upgradeRule,
} from "../dist/index.js";
import { calendarViewToBlocks, getScheduleToBlocks, workingHoursToRule } from "../dist/adapters/graph.js";
import { freeBusyToBlocks } from "../dist/adapters/google.js";

/** One bookable half hour, so every request below has an answer once it is valid. */
const REQUEST = {
  now: "2027-06-14T08:00Z",
  goal: {
    zone: "UTC",
    search_window: ["2027-06-14T08:00Z", "2027-06-14T12:00Z"],
    parts: [{ id: "main", duration: "30m", requirement: { all: [{ person: "ana" }] } }],
  },
  participants: [{ id: "ana", zone: "UTC", calendar: [] }],
};

/** The engine's JSON answer to `document`. */
function answerJson(engine, document) {
  return engine.findAvailabilityJson(document);
}

/** The value `call` threw, failing when it returned instead. */
function thrown(call) {
  try {
    call();
  } catch (error) {
    return error;
  }
  assert.fail("expected the call to reject");
}

/** `call` rejected with `OpportuneError` whose `code` and `path` are `code` and `path`. */
function assertRejection(call, code, path = null) {
  const error = thrown(call);
  assert.ok(
    error instanceof OpportuneError,
    `expected OpportuneError, got ${error?.constructor?.name}: ${error?.message}`,
  );
  assert.equal(error.code, code);
  assert.equal(error.path, path);
  assert.deepEqual(error.errors, [], "a document that cannot be read has no findings to list");
  assert.equal(typeof error.message, "string");
  assert.notEqual(error.message, "", "the rejection says what it could not read");
  return error;
}

test("a document the wire layer cannot read is rejected with the wire layer's code", () => {
  const engine = new Engine();

  // A value with no JSON spelling never reaches the parser, so this layer names it.
  const circular = { ...REQUEST, goal: { ...REQUEST.goal } };
  circular.goal.self = circular;
  assertRejection(() => answerJson(engine, circular), "invalid_json");

  // A member the document does not have is a shape failure with the field named.
  assertRejection(
    () => answerJson(engine, { ...REQUEST, goal: { ...REQUEST.goal, parts: "one" } }),
    "invalid_type",
    "/goal/parts",
  );
  assertRejection(
    () => answerJson(engine, { ...REQUEST, now: undefined }),
    "missing_field",
  );
  assertRejection(
    () => answerJson(engine, { ...REQUEST, goal: { ...REQUEST.goal, extra: true } }),
    "unknown_field",
    "/goal",
  );
  assertRejection(
    () => answerJson(engine, { ...REQUEST, rules: [{ id: "r", kind: "nope", params: {} }] }),
    "unknown_variant",
    "/rules/0/kind",
  );
});

test("a rule the document spells wrongly is rejected at the field, not at the request", () => {
  // The pointer is what a caller acts on: `/rules/0` names the rule to look at,
  // where a pointer at the whole request would leave the caller to find it. A
  // rule's params are read behind the `kind` discriminator, so the wire layer
  // points at the rule and not at the weekly range inside it — only a mis-spelled
  // `kind` points deeper, as the `unknown_variant` case above does.
  assertRejection(
    () =>
      answerJson(new Engine(), {
        ...REQUEST,
        rules: [{ id: "hours", kind: "window", params: { zone: "UTC", weekly: { mon: "09:00-17:00" } } }],
      }),
    "invalid_type",
    "/rules/0",
  );
});

test("an invalid request reports its findings, with the pointer of each", () => {
  const engine = new Engine();

  // Structural validity is `validate`'s separate answer; a search refuses it.
  const diagnostics = engine.validate({
    ...REQUEST,
    rules: [
      { id: "hours", kind: "window", params: { zone: "UTC", weekly: { mon: ["09:00-17:00"] } } },
      { id: "hours", kind: "window", params: { zone: "UTC", weekly: { mon: ["09:00-17:00"] } } },
    ],
  });
  assert.deepEqual(diagnostics.warnings, []);
  assert.deepEqual(
    diagnostics.errors.map((finding) => [finding.code, finding.path]),
    [["rule_id_duplicate", "/rules/1/id"]],
  );

  const rejected = thrown(() =>
    answerJson(engine, {
      ...REQUEST,
      rules: [
        { id: "hours", kind: "window", params: { zone: "UTC", weekly: { mon: ["09:00-17:00"] } } },
        { id: "hours", kind: "window", params: { zone: "UTC", weekly: { mon: ["09:00-17:00"] } } },
      ],
    }),
  );
  assert.ok(rejected instanceof OpportuneError);
  assert.equal(rejected.code, "invalid_request");
  assert.equal(rejected.path, null, "the request is at fault, not one document field");
  assert.deepEqual(
    rejected.errors.map((finding) => [finding.code, finding.path]),
    [["rule_id_duplicate", "/rules/1/id"]],
  );

  // A participant naming a zone no database knows is the same kind of answer.
  const unknownZone = thrown(() =>
    answerJson(engine, { ...REQUEST, participants: [{ id: "ana", zone: "Mars/Olympus", calendar: [] }] }),
  );
  assert.ok(unknownZone instanceof OpportuneError);
  assert.equal(unknownZone.code, "invalid_request");
  assert.deepEqual(
    unknownZone.errors.map((finding) => [finding.code, finding.path]),
    [["zone_unknown", "/participants/0/zone"]],
  );
});

test("a provider payload that cannot be read is an unavailable calendar or a payload error", () => {
  // An unreadable `calendarView` is a mailbox that is unknown, never one that is
  // empty: an empty block list would read as completely free.
  for (const payload of ["{", "[]", JSON.stringify({ value: [{}] })]) {
    const outcome = calendarViewToBlocks(payload, "UTC");
    assert.equal(outcome.status, "unavailable", payload);
    assert.equal(outcome.code, "payload_invalid", payload);
  }

  // A payload that is not the whole response this adapter reads is an error, because
  // there is nothing per-calendar to answer for.
  assertRejection(() => getScheduleToBlocks("{"), "payload_invalid");
  assertRejection(() => freeBusyToBlocks("{"), "payload_invalid");
});

test("a working calendar that cannot be resolved is an outcome, not a throw", () => {
  // Graph's custom form is not an IANA or Windows name, so the caller falls back to
  // its own window rather than catching an exception. The E2E suite proves the
  // blocks from the same response are unaffected.
  const outcome = workingHoursToRule(
    JSON.stringify({
      daysOfWeek: ["monday"],
      startTime: "09:00:00.0000000",
      endTime: "17:00:00.0000000",
      timeZone: { "@odata.type": "#microsoft.graph.customTimeZone", bias: 480 },
    }),
  );
  assert.equal(outcome.status, "unavailable");
  assert.equal(outcome.code, "custom_time_zone");
  assert.equal(typeof outcome.message, "string");
});

test("a limit past four billion is the count it names, not a parse failure", () => {
  const engine = new Engine();

  // `usize` cannot hold four billion on `wasm32`, so the wire value is read as a
  // `u64` and saturates into the domain: the engine still sees a limit no offer
  // count can reach, which is what a 64-bit server sees too.
  const unbounded = JSON.parse(answerJson(engine, { ...REQUEST, limit: 4294967296 }));
  assert.equal(unbounded.truncated, false, "a limit nothing reaches truncates nothing");
  assert.equal(unbounded.offers.length, 8, "every candidate start in the four-hour window");

  // The same document without the limit answers the same offers, which is what
  // "the limit is effectively absent" means.
  const withoutLimit = JSON.parse(answerJson(engine, REQUEST));
  assert.deepEqual(unbounded.offers, withoutLimit.offers);

  // A limit that is reachable truncates, so the huge value is not simply ignored.
  const capped = JSON.parse(answerJson(engine, { ...REQUEST, limit: 2 }));
  assert.equal(capped.offers.length, 2);
  assert.equal(capped.truncated, true);
});

test("an explain limit past four billion is the configured-limits finding", () => {
  // On both targets the document parses and validation is what refuses it, with the
  // finding a caller can act on. The message counts a 32-bit `usize::MAX` in this
  // build and the requested count on a 64-bit one, so the finding — not its wording
  // — is the contract.
  const error = thrown(() =>
    answerJson(new Engine(), { ...REQUEST, explain: { limit: 4294967296 } }),
  );
  assert.ok(error instanceof OpportuneError);
  assert.equal(error.code, "invalid_request");
  assert.equal(error.path, null);
  assert.deepEqual(
    error.errors.map((finding) => [finding.code, finding.path]),
    [["explain_limit", "/explain/limit"]],
  );

  const diagnostics = new Engine().validate({ ...REQUEST, explain: { limit: 4294967296 } });
  assert.deepEqual(
    diagnostics.errors.map((finding) => [finding.code, finding.path]),
    [["explain_limit", "/explain/limit"]],
  );
});

test("a configured limit past four billion configures the same engine on every target", () => {
  // Every wire limit is a count, so each one reads as a `u64` and saturates into the
  // domain rather than failing the document that sets it.
  const pastFourBillion = {
    max_participants: 4294967296,
    max_blocks_per_participant: 4294967296,
    max_search_window_days: 92,
    max_candidate_starts: 4294967296,
    max_explain_limit: 4294967296,
  };
  const engine = new Engine(pastFourBillion);
  assert.equal(typeof engine.tzdbVersion, "string");
  const answered = JSON.parse(answerJson(engine, { ...REQUEST, explain: { limit: 5000 } }));
  assert.equal(answered.offers.length, 8, "an explain limit the engine accepts changes no offer");

  // The window length is a `u32` rather than a count and survives unchanged.
  assert.equal(limitsWith({ max_search_window_days: 120 }).max_search_window_days, 120);
  assert.equal(defaultLimits().max_search_window_days, 92, "the override starts from the defaults");

  // A limit the document cannot mean is still rejected, reclassified as
  // `invalid_limits` (500) with its field pointer and underlying wire cause kept.
  const negative = assertRejection(
    () => new Engine({ max_participants: -1 }),
    "invalid_limits",
    "/max_participants",
  );
  assert.equal(httpStatusForError(negative), 500);
  assert.equal(negative.cause?.code, "invalid_value");

  const unknown = assertRejection(() => new Engine({ nope: 1 }), "invalid_limits");
  assert.equal(httpStatusForError(unknown), 500);
  assert.equal(unknown.cause?.code, "unknown_field");
});

/**
 * A request whose every defaulted field is written as `null`.
 *
 * A binding that fills every key in — as several JSON mappings do — produces this
 * document, and each `null` has to mean the default the specification states rather
 * than a shape failure.
 */
const EVERY_DEFAULT_NULL = {
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
      calendar: [{ span: ["2027-03-09T15:00Z", "2027-03-09T16:00Z"], status: null, all_day: null, tags: null }],
    },
  ],
  // A window with no hours shares none, which is a defaulted window rather than an
  // unreadable rule.
  rules: [{ id: "w", kind: "window", who: null, when: null, params: { zone: "UTC", weekly: null, dates: null } }],
  occurrences: null,
  exclude_event_keys: null,
  calendars: { org: { zone: "UTC", working_days: "mon-fri", holidays: null } },
  weights: null,
};

test("an explicit null on a defaulted field is the default the specification states", () => {
  const engine = new Engine();

  const answer = JSON.parse(answerJson(engine, EVERY_DEFAULT_NULL));
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

test("a whole request is a ranges query, and a stray field is still unknown", () => {
  // A `RangesQuery` is a `Request` without the six fields the ranges do not read, and
  // the wire layer reads a query with `deny_unknown_fields`. Passing the request a
  // caller already holds therefore used to compile — both types have `now`, `goal`,
  // `participants` and `rules` — and crash on the first engine call with
  // `unknown_field`. The six fields are dropped here, and only those six: what the
  // answer says for the request is what it says for the query beneath it, and a
  // misspelled field is still a query the wire does not read.
  const engine = new Engine();
  const query = { now: REQUEST.now, goal: REQUEST.goal, participants: REQUEST.participants };
  const request = {
    ...query,
    occurrences: [],
    exclude_event_keys: ["evt_1"],
    calendars: { org: { zone: "UTC", working_days: "mon-fri" } },
    weights: { ana: 1 },
    limit: 5,
    explain: { limit: 3 },
  };

  assert.equal(engine.requiredRangesJson(request), engine.requiredRangesJson(query));
  assert.deepEqual(engine.requiredRanges(request), engine.requiredRanges(query));
  assert.equal(requiredRangesJson(request), engine.requiredRangesJson(query));
  assert.deepEqual(requiredRanges(request), engine.requiredRanges(query));

  // The six fields are the whole of the difference, so anything else the caller added
  // is sent as it stands rather than dropped with them.
  assertRejection(() => engine.requiredRangesJson({ ...query, bogus: 1 }), "unknown_field");
  assertRejection(() => requiredRangesJson({ ...request, bogus: 1 }), "unknown_field");
});

test("the whole-request form keeps the query's own fields, not only the six it drops", () => {
  // Dropping the six request-only fields is only half the projection: the fields a
  // query does read have to reach the wire unchanged. Two of them move the answer —
  // a `rest` rule widens the calendar range to the days it reads, and a participant's
  // own zone moves the hull to their local days — so a projection that dropped either
  // would answer the narrower document's ranges. That is the dangerous direction: a
  // caller fetches too little, an overnight absence or a busy morning outside the hull
  // reads as free, and the search offers a start nobody is available for.
  const engine = new Engine();
  const query = { now: REQUEST.now, goal: REQUEST.goal, participants: REQUEST.participants };
  const rested = { ...query, rules: [rest()] };
  const remote = {
    ...query,
    participants: [{ id: "ana", zone: "Pacific/Kiritimati", calendar: [] }],
  };
  for (const [what, moved] of [
    ["a rest rule", rested],
    ["a participant's own zone", remote],
  ]) {
    assert.notEqual(
      engine.requiredRangesJson(moved),
      engine.requiredRangesJson(query),
      `${what} does not move the ranges, so nothing here proves it reaches the wire`,
    );
  }

  // The request a caller already holds therefore answers what both query documents
  // answer, not the six-fields-dropped remainder of them.
  const beneath = { ...rested, participants: remote.participants };
  const request = {
    ...beneath,
    occurrences: [],
    exclude_event_keys: ["evt_1"],
    calendars: { org: { zone: "UTC", working_days: "mon-fri" } },
    weights: { ana: 1 },
    limit: 5,
    explain: { limit: 3 },
  };
  assert.equal(engine.requiredRangesJson(request), engine.requiredRangesJson(beneath));
  assert.deepEqual(requiredRanges(request), engine.requiredRanges(beneath));
  assert.notEqual(
    engine.requiredRangesJson(request),
    engine.requiredRangesJson(query),
    "the request's ranges have to be the widened query's, not a document with its rules and zones gone",
  );
});

test("a member the projection does not name reaches the wire, `__proto__` included", () => {
  // A route hands `requiredRanges` the body its JSON parser produced, and `JSON.parse`
  // makes `__proto__` a member of the object it returns. Carrying the query across by
  // assignment gave that name to the new object's prototype instead, so the member
  // vanished: the wire never saw a field it rejects every other unknown field with, and
  // the call answered for a narrower document than the caller sent.
  const engine = new Engine();
  const body = JSON.parse(
    `{"now":${JSON.stringify(REQUEST.now)},"goal":${JSON.stringify(REQUEST.goal)},` +
      `"participants":${JSON.stringify(REQUEST.participants)},"__proto__":{"stray":true}}`,
  );
  assert.ok(Object.hasOwn(body, "__proto__"), "the parsed body carries the member as its own");
  assert.ok(!Object.hasOwn({}, "stray"), "the fixture wrote no prototype");

  const error = assertRejection(() => engine.requiredRangesJson(body), "unknown_field");
  assert.match(error.message, /__proto__/, "the rejection names the member it refused");
  assertRejection(() => requiredRangesJson(body), "unknown_field");
});

test("a query member that cannot be read is a structured rejection", () => {
  // The request-to-query projection reads each member before JSON serialization. A
  // getter that throws is still a document with no JSON spelling, exactly as it is
  // for every other engine entry point, rather than a WASM failure blamed on the module.
  const engine = new Engine();
  const failure = new TypeError("the stored rules could not be read");
  const query = { ...REQUEST };
  Object.defineProperty(query, "rules", {
    enumerable: true,
    get() {
      throw failure;
    },
  });

  for (const call of [
    () => engine.requiredRangesJson(query),
    () => requiredRangesJson(query),
  ]) {
    const error = assertRejection(call, "invalid_json");
    assert.equal(error.cause, failure, "the projection keeps the read failure as its cause");
  }
});

test("a participant past the zone-period cap fails closed on every entry point", () => {
  // The list a schedule is resolved from is bounded like every other collection a
  // request carries, and every entry point answers the cap before resolving it.
  const engine = new Engine(limitsWith({ max_zone_periods_per_participant: 1 }));
  const crowded = {
    ...REQUEST,
    participants: [
      {
        id: "ana",
        zone: "UTC",
        calendar: [],
        zone_periods: [
          { from: "2027-06-14T00:00Z", until: "2027-06-14T01:00Z", zone: "UTC" },
          { from: "2027-06-14T02:00Z", until: "2027-06-14T03:00Z", zone: "UTC" },
        ],
      },
    ],
  };

  assert.deepEqual(
    engine.validate(crowded).errors.map((finding) => [finding.code, finding.path]),
    [["zone_periods_limit", "/participants/0/zone_periods"]],
  );

  for (const call of [
    () => engine.findAvailability(crowded),
    () => engine.checkSlot(crowded, { events: [] }),
    () => engine.requiredRanges(crowded),
  ]) {
    const error = thrown(call);
    assert.ok(error instanceof OpportuneError);
    assert.equal(error.code, "invalid_request");
    assert.deepEqual(
      error.errors.map((finding) => [finding.code, finding.path]),
      [["zone_periods_limit", "/participants/0/zone_periods"]],
    );
  }

  // The cap bounds the list rather than the request's shape: a participant exactly
  // at it is answered.
  const within = {
    ...crowded,
    participants: [{ ...crowded.participants[0], zone_periods: crowded.participants[0].zone_periods.slice(0, 1) }],
  };
  assert.ok(engine.findAvailability(within).offers.length > 0, "a participant at the cap is bookable");
});

test("a stored rule with no JSON spelling or an unknown version says which it is", () => {
  assertRejection(() => upgradeRule("{not json", 1), "invalid_json");
  assertRejection(
    () => upgradeRule('{"id":"x","kind":"window","params":{"zone":"UTC","weekly":{}}}', 99),
    "unsupported_schema_version",
  );
});
