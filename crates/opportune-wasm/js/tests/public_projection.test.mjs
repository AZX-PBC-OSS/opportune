/**
 * The projection a public booking route serves, and what it leaves behind.
 *
 * A `Response` answers an operator: beside the offers it carries the request's
 * warnings, every candidate start that was not offered and why, and, for each offer,
 * who could fill every term and who was chosen. A public booking link must not serve
 * any of that — a visitor reads the times it can book, not the host's calendars and
 * the policies that shaped them. `publicResponse`/`publicSlotCheck` are the opt-in
 * projection, and this suite is the one that proves the operator's half is gone from
 * the document a route returns: every field below is asserted against a real engine
 * answer that *does* carry it, so a projection that silently passed the answer
 * through would fail here rather than in a visitor's browser.
 *
 * The projection is also reachable from `opportune/presets`, which is the subpath a
 * bundle that may load no WebAssembly can import: the two are the same functions, so
 * a route and a client component project the same document.
 *
 * Run against the built package:
 *
 * ```sh
 * make wasm && node --test crates/opportune-wasm/js/tests/public_projection.test.mjs
 * ```
 */

import assert from "node:assert/strict";
import test from "node:test";

import {
  OpportuneError,
  capPerDay,
  checkSlot,
  findAvailability,
  isKnownStatus,
  offerToProposal,
  person,
  publicResponse,
  publicSlotCheck,
  singleGoal,
  weeklyHours,
} from "../dist/index.js";
import {
  OpportuneError as PresetError,
  publicResponse as presetPublicResponse,
  publicSlotCheck as presetPublicSlotCheck,
} from "../dist/presets-entry.js";

/**
 * A request whose answer carries every operator-only field at once.
 *
 * `ana` has no calendar at all, so the offer carries the `calendar_unknown` flag; the
 * two-hour duration against a 09:00–17:00 window leaves candidate starts that cannot
 * fit, so `excluded` and `excluded_truncated` are populated; the offer limit is one so
 * `truncated` is the answer; and the cap names a tag no part is tagged with, which is
 * the validation warning the operator sees and a visitor must not.
 */
const REQUEST = {
  now: "2027-06-14T00:00Z",
  goal: singleGoal("2h", { all: [person("ana")] }, {
    zone: "UTC",
    searchWindow: ["2027-06-14T00:00Z", "2027-06-14T23:00Z"],
  }),
  participants: [{ id: "ana", zone: "UTC" }],
  rules: [
    weeklyHours({ mon: "09:00-17:00" }, { zone: "UTC" }),
    capPerDay({ tag: "missing" }, 2, { when: true }),
  ],
  limit: 1,
  explain: { limit: 2 },
};

/** The operator answer the projection is applied to. */
const ANSWER = findAvailability(REQUEST);

/** `value` as the JSON document it is, so a projection cannot hide behind `undefined`. */
const json = (value) => JSON.parse(JSON.stringify(value));

/** The rejection `call` raises, as the package's error. */
function rejection(call) {
  try {
    call();
  } catch (error) {
    assert.ok(
      error instanceof OpportuneError,
      `expected OpportuneError, got ${error?.constructor?.name}: ${error?.message}`,
    );
    return error;
  }
  return assert.fail("expected the call to reject");
}

test("the engine's own answer carries the fields a visitor must not see", () => {
  // The control the cases below are measured against: without it a projection that
  // dropped nothing would pass every assertion that only reads the projected document.
  assert.ok(ANSWER.offers.length > 0, "the request offers starts");
  assert.ok(ANSWER.truncated, "the offer limit cut the list short");
  assert.ok(ANSWER.excluded.length > 0, "the answer explains starts it did not offer");
  assert.ok(ANSWER.excluded_truncated, "the explanation limit cut the exclusion list short");
  assert.ok(
    ANSWER.warnings.some((warning) => warning.code === "count_matches_no_part"),
    "the answer carries the cap's validation warning",
  );
  const [event] = ANSWER.offers[0].events;
  assert.deepEqual(
    Object.keys(event).sort(),
    ["assignment", "eligible", "end", "part", "reserve", "start"],
    "an operator's event carries who could fill it, who was chosen and the travel to hold",
  );
  assert.deepEqual(ANSWER.offers[0].flags, ["calendar_unknown"]);
});

test("the public response keeps the offers, the dates and the truncation, and nothing else", () => {
  const publicAnswer = json(publicResponse(ANSWER));

  assert.deepEqual(Object.keys(publicAnswer).sort(), ["dates", "meta", "offers", "truncated"]);
  assert.deepEqual(publicAnswer.dates, json(ANSWER.dates), "the dates are the visitor's own view");
  assert.deepEqual(publicAnswer.meta, json(ANSWER.meta));
  assert.equal(publicAnswer.truncated, true, "the cut-short list is the visitor's to know about");
  assert.equal(publicAnswer.offers.length, ANSWER.offers.length);

  // Each offer is its parts' times and the flags — the visitor's answer — and the
  // host's eligibility, assignment and reserve travel stay behind.
  assert.deepEqual(Object.keys(publicAnswer.offers[0]).sort(), ["events", "flags"]);
  assert.deepEqual(publicAnswer.offers[0].events[0], {
    end: ANSWER.offers[0].events[0].end,
    part: ANSWER.offers[0].events[0].part,
    start: ANSWER.offers[0].events[0].start,
  });
  assert.deepEqual(publicAnswer.offers[0].flags, ["calendar_unknown"]);

  // The projection is a document of its own rather than the answer with keys deleted,
  // so a field the operator's answer grows later does not reach a visitor by default.
  const operatorOnly = ["excluded", "excluded_truncated", "warnings"];
  for (const field of operatorOnly) {
    assert.ok(!(field in publicAnswer), `the public response carries \`${field}\``);
  }
  for (const event of publicAnswer.offers.flatMap((offer) => offer.events)) {
    assert.deepEqual(Object.keys(event).sort(), ["end", "part", "start"]);
  }
});

test("the public slot check serves the verdict, not why it failed", () => {
  // A booking that does not fit: the operator's answer carries the reasons, the
  // reserve travel to hold and the request's warnings, and a public confirmation
  // serves whether the time can be booked.
  const proposal = offerToProposal(ANSWER.offers[0]);
  const outside = {
    ...proposal,
    events: proposal.events.map((event) => ({ ...event, start: "2027-06-14T06:00Z" })),
  };

  for (const check of [checkSlot(REQUEST, proposal), checkSlot(REQUEST, outside)]) {
    const publicCheck = json(publicSlotCheck(check));
    assert.deepEqual(Object.keys(publicCheck).sort(), ["flags", "meta", "valid"]);
    assert.equal(publicCheck.valid, check.valid);
    assert.deepEqual(publicCheck.flags, json(check.flags));
    assert.deepEqual(publicCheck.meta, json(check.meta));
    for (const field of ["reasons", "reserve", "warnings"]) {
      assert.ok(!(field in publicCheck), `the public slot check carries \`${field}\``);
    }
  }
  // The control: the rejected booking's own answer does carry the reason, so dropping
  // it above is the projection rather than an answer that never had one.
  assert.ok(checkSlot(REQUEST, outside).reasons.length > 0);
});

test("the projections are the ones `opportune/presets` exports", () => {
  // A route and a client component reach the same functions, so the document a route
  // serves and the one a bundle projects are one shape rather than two look-alikes.
  assert.equal(presetPublicResponse, publicResponse);
  assert.equal(presetPublicSlotCheck, publicSlotCheck);
  assert.equal(PresetError, OpportuneError);
  assert.deepEqual(json(presetPublicResponse(ANSWER)), json(publicResponse(ANSWER)));
});

/** The versions a projection reads, so a case below can be about another field. */
const META = { engine: "0.1.0", schema: 1, tzdb: "2026a" };

test("a document the projection cannot read is the package's rejection", () => {
  // A route body parsed back from a queue, a lookup that missed: the projection is a
  // public function, so a shape it cannot read has to leave as the error a handler
  // already catches rather than a `TypeError` from `.map` on `undefined` — and never
  // as an `undefined` written into the public document where a field belongs. Every
  // field the projection copies is checked, so the cases below are one per field.
  for (const value of [null, undefined, "a response", [], 7]) {
    const error = rejection(() => publicResponse(value));
    assert.equal(error.code, "invalid_type", String(value));
    assert.equal(error.path, null);
  }
  const unreadable = [
    {},
    { offers: [] },
    { dates: [], meta: META, offers: [], truncated: "no" },
    { dates: "none", meta: META, offers: [], truncated: false },
    { dates: [], meta: "none", offers: [], truncated: false },
    { dates: [], meta: META, offers: null, truncated: false },
    { dates: [], meta: META, offers: [null], truncated: false },
    { dates: [], meta: META, offers: [{ events: [] }], truncated: false },
    { dates: [], meta: META, offers: [{ flags: [] }], truncated: false },
    { dates: [], meta: META, offers: [{ events: [null], flags: [] }], truncated: false },
    { dates: [], meta: META, offers: [{ events: [{}], flags: [] }], truncated: false },
    { dates: [], meta: META, offers: [{ events: [{ part: "main" }], flags: [] }], truncated: false },
    // `meta` is three required fields, not "whatever the document carries": a
    // `Response` read back from a JSON column that lost one, or from a queue that
    // wrote a float, must not project into a document typed `Meta` with `undefined`
    // where the type promises a string.
    { dates: [], meta: {}, offers: [], truncated: false },
    { dates: [], meta: { ...META, engine: null }, offers: [], truncated: false },
    { dates: [], meta: { ...META, tzdb: 7 }, offers: [], truncated: false },
    { dates: [], meta: { ...META, schema: "1" }, offers: [], truncated: false },
    { dates: [], meta: { ...META, schema: 1.5 }, offers: [], truncated: false },
    { dates: [], meta: { ...META, schema: Number.NaN }, offers: [], truncated: false },
    { dates: [], meta: { ...META, schema: Number.POSITIVE_INFINITY }, offers: [], truncated: false },
  ];
  for (const value of unreadable) {
    const error = rejection(() => publicResponse(value));
    assert.equal(error.code, "invalid_type", JSON.stringify(value));
    assert.equal(error.path, null, JSON.stringify(value));
  }
  // The missing `meta` field is named, not reported as "the document has no meta".
  assert.match(
    rejection(() => publicResponse({ dates: [], meta: {}, offers: [], truncated: false })).message,
    /`meta\.engine`/,
  );
  assert.match(
    rejection(() =>
      publicResponse({ dates: [], meta: { ...META, tzdb: 7 }, offers: [], truncated: false }),
    ).message,
    /`meta\.tzdb`/,
  );

  // The message names the field it could not read, which is what makes the rejection
  // actionable at a route that was handed the wrong document.
  assert.match(rejection(() => publicResponse({ offers: [] })).message, /`dates`/);
  assert.match(
    rejection(() =>
      publicResponse({ dates: [], meta: META, offers: "none", truncated: false }),
    ).message,
    /`offers`/,
  );
  assert.match(
    rejection(() =>
      publicResponse({
        dates: [],
        meta: META,
        offers: [{ events: [{}], flags: [] }],
        truncated: false,
      }),
    ).message,
    /`end`/,
  );

  // The guard is a shape check, not a demand that the answer came from a call: a
  // document of the right shape is projected as it stands.
  assert.deepEqual(
    publicResponse({
      dates: [{ date: "2027-06-14", offers: 0 }],
      meta: { engine: "0.1.0", schema: 1, tzdb: "2026a" },
      offers: [{ events: [{ part: "main", start: "2027-06-14T09:00Z", end: "2027-06-14T09:30Z" }], flags: [] }],
      truncated: false,
    }),
    {
      dates: [{ date: "2027-06-14", offers: 0 }],
      meta: { engine: "0.1.0", schema: 1, tzdb: "2026a" },
      offers: [{ events: [{ part: "main", start: "2027-06-14T09:00Z", end: "2027-06-14T09:30Z" }], flags: [] }],
      truncated: false,
    },
  );

  for (const value of [
    null,
    undefined,
    [],
    { flags: null },
    { flags: [], meta: META, valid: "yes" },
    { flags: [], valid: true },
    { flags: [], meta: {}, valid: true },
    { flags: [], meta: { ...META, schema: null }, valid: true },
    { meta: META, valid: true },
  ]) {
    const error = rejection(() => publicSlotCheck(value));
    assert.equal(error.code, "invalid_type", JSON.stringify(value));
  }
  assert.deepEqual(publicSlotCheck({ flags: ["calendar_unknown"], meta: META, valid: true }), {
    flags: ["calendar_unknown"],
    meta: META,
    valid: true,
  });

  // Shallow-copy and projection guarantees: mutating the public output does not
  // mutate the source answer, and extra properties on `meta` or `dates` entries
  // are stripped rather than leaked.
  const withExtras = {
    dates: [{ date: "2027-06-14", offers: 1, internal: "drop" }],
    meta: { engine: "0.1.0", schema: 1, tzdb: "2026a", internal: "drop" },
    offers: [{ events: [{ part: "main", start: "2027-06-14T09:00Z", end: "2027-06-14T09:30Z" }], flags: ["calendar_unknown"] }],
    truncated: false,
  };
  const projected = publicResponse(withExtras);
  assert.deepEqual(projected.meta, { engine: "0.1.0", schema: 1, tzdb: "2026a" });
  assert.deepEqual(projected.dates, [{ date: "2027-06-14", offers: 1 }]);
  projected.offers[0].flags.push("mutated");
  assert.deepEqual(withExtras.offers[0].flags, ["calendar_unknown"]);

  const checkSource = {
    flags: ["calendar_unknown"],
    meta: { engine: "0.1.0", schema: 1, tzdb: "2026a", secret: "drop" },
    valid: true,
  };
  const projectedCheck = publicSlotCheck(checkSource);
  assert.deepEqual(projectedCheck.meta, { engine: "0.1.0", schema: 1, tzdb: "2026a" });
  projectedCheck.flags.push("mutated");
  assert.deepEqual(checkSource.flags, ["calendar_unknown"]);
});

test("a status the engine does not interpret is not one it named", () => {
  // The open union completes the three the engine gives a meaning to and still accepts
  // whatever a provider reports; the predicate is how a caller tells them apart, because
  // an unrecognised status reads as `busy`.
  for (const known of ["free", "tentative", "busy"]) {
    assert.ok(isKnownStatus(known), known);
  }
  const throwingObject = {
    toString() {
      throw new Error("should not be coerced");
    },
  };
  for (const other of ["", "Free", "out_of_office", "workingElsewhere", null, undefined, 42, throwingObject]) {
    assert.ok(!isKnownStatus(other), String(typeof other === "object" && other !== null ? "object" : other));
  }
});
