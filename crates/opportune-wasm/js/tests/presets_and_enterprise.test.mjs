/**
 * U1 and the enterprise scenarios, built with the package's own presets and sugar.
 *
 * Every request here is built from the public surface — the presets produce the
 * rules, the sugar produces the goal, terms and proposal — so a preset that emits a
 * rule the engine rejects, or a helper that drifts from the general shape, fails
 * here rather than in an application. The scenarios are the ones the Python suite
 * proves against the same engine (`crates/opportune-py/tests/test_presets_and_enterprise.py`),
 * so the two bindings run the same expectations on the same rules.
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
  andWho,
  atLeast,
  availableOnly,
  buffers,
  businessHours,
  capPerDay,
  capPerMonth,
  capPerWeek,
  checkSlot,
  datedHours,
  defaultLimits,
  exceptWho,
  findAvailability,
  horizon,
  httpStatusForError,
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
  statusPolicy,
  unless,
  validate,
  weeklyHours,
} from "../dist/index.js";
import { calendarViewToBlocks } from "../dist/adapters/graph.js";

/** One calendar block: a span, its tags, and the caller's event key. */
function block(span, tags = [], key = undefined) {
  const value = { span, tags };
  if (key !== undefined) {
    value.event_key = key;
  }
  return value;
}

/** The first failing reason per participant the response excluded at `start`. */
function reasonsAt(response, start) {
  const excluded = response.excluded.find((entry) => entry.start === start);
  assert.ok(excluded !== undefined, `the response excludes no start at ${start}`);
  return excluded.reasons.map((reason) => [reason.participant, reason.code, reason.rule]);
}

/** The value `call` threw, failing when it returned instead. */
function rejection(call) {
  try {
    call();
  } catch (thrown) {
    return thrown;
  }
  assert.fail("expected the call to reject");
}

/** Whether the reasons at `start` include the `[participant, code, rule]` triple. */
function hasReason(response, start, expected) {
  const [participant, code, rule] = expected;
  return reasonsAt(response, start).some(
    ([at, found, named]) => at === participant && found === code && named === rule,
  );
}

/** The offer starts, in the order the engine returned them. */
function starts(response) {
  return response.offers.map(offerStart);
}

test("U1: a personal thirty-minute link takes about ten lines", () => {
  const request = {
    now: "2027-06-14T07:00Z",
    goal: singleGoal("30m", { all: [person("rich")] }, {
      zone: "Europe/London",
      searchWindow: ["2027-06-13T23:00Z", "2027-06-18T23:00Z"],
    }),
    participants: [{ id: "rich", zone: "Europe/London", calendar: [] }],
    rules: [businessHours(), buffers(), minNotice()],
  };

  const response = findAvailability(request);

  assert.equal(offerStart(response.offers[0]), "2027-06-14T11:00Z", "four hours of notice");
  assert.equal(offerEnd(response.offers[0]), "2027-06-14T11:30Z");
  assert.deepEqual(response.offers[0].flags, [], "a supplied calendar is not unknown");
  assert.equal(response.dates[0].offers, 10);
});

test("U17: the booth window narrows the team day in the conference zone", () => {
  const request = {
    now: "2027-06-14T08:00Z",
    goal: {
      zone: "America/Anchorage",
      search_window: ["2027-06-14T16:00Z", "2027-06-15T02:00Z"],
      parts: [
        { duration: "30m", tags: ["booth"], requirement: { all: [person("team")] } },
      ],
    },
    participants: [
      {
        id: "team",
        zone: "UTC",
        calendar: [],
        zone_periods: [
          { from: "2027-06-13T08:00Z", until: "2027-06-15T08:00Z", zone: "America/Anchorage" },
        ],
      },
    ],
    rules: [
      { ...businessHours("mon-sun", "10:00-16:00", { zone: "event", ruleId: "booth" }), when: { tag: "booth" } },
      businessHours("mon-sun", "08:00-18:00", { ruleId: "hours" }),
    ],
    explain: { limit: 100 },
  };

  const response = findAvailability(request);

  assert.deepEqual(starts(response).slice(0, 2), ["2027-06-14T18:00Z", "2027-06-14T18:30Z"],
    "10:00 in Anchorage, the booth's opening, not the team's 08:00");
  assert.equal(response.offers.length, 12, "six booth hours of half-hour slots");
  assert.ok(
    hasReason(response, "2027-06-14T16:00Z", ["team", "outside_window", "booth"]),
    "the team's own day starts before the booth opens",
  );
});

test("U7/U11: the panel respects each interviewer's daily cap", () => {
  const booked = (host, day) => [
    block([`2027-06-${day}T00:15Z`, `2027-06-${day}T00:45Z`], ["interview"], `${host}-${day}-a`),
    block([`2027-06-${day}T00:45Z`, `2027-06-${day}T01:15Z`], ["interview"], `${host}-${day}-b`),
  ];
  const pool = ["ana", "bo", "cy"];
  const request = {
    now: "2027-06-13T18:15Z",
    goal: {
      zone: "Asia/Kathmandu",
      search_window: ["2027-06-14T03:15Z", "2027-06-14T06:15Z"],
      candidates: [
        "2027-06-14T03:15Z",
        "2027-06-14T03:45Z",
        "2027-06-14T04:15Z",
        "2027-06-14T04:45Z",
        "2027-06-14T05:15Z",
        "2027-06-14T05:45Z",
      ],
      parts: [
        { duration: "30m", tags: ["interview"], requirement: { all: [atLeast(1, { tag: "pool/interviewers" })] } },
      ],
    },
    participants: [
      ...pool.map((host) => ({
        id: host,
        zone: "Asia/Kathmandu",
        tags: ["pool/interviewers"],
        calendar: booked(host, "14"),
      })),
      {
        id: "dee",
        zone: "Asia/Kathmandu",
        tags: ["pool/interviewers"],
        calendar: [block(["2027-06-14T04:15Z", "2027-06-14T06:15Z"], [], "dee-block")],
      },
    ],
    rules: [
      businessHours("mon-sun", "09:00-18:00", { ruleId: "hours" }),
      capPerDay({ tag: "interview" }, 2, { ruleId: "each_day" }),
      capPerWeek({ tag: "interview" }, 8, { per: "pooled", zone: "event", ruleId: "pool_week" }),
    ],
    explain: { limit: 100 },
  };

  const response = findAvailability(request);
  assert.deepEqual(response.offers.map(offerAssignedHosts), [["dee"], ["dee"]],
    "ana, bo and cy are at two interviews each; only dee can take the panel");
  assert.deepEqual(
    new Set(
      reasonsAt(response, "2027-06-14T04:15Z")
        .filter(([, , rule]) => rule === "each_day")
        .map(([, code]) => code),
    ),
    new Set(["cap_reached"]),
  );

  // Two interviews each on both days exhausts the pool's eight-interview week.
  const overQuota = {
    ...request,
    participants: [
      ...pool.map((host) => ({
        id: host,
        zone: "Asia/Kathmandu",
        tags: ["pool/interviewers"],
        calendar: [...booked(host, "14"), ...booked(host, "15")],
      })),
      {
        id: "dee",
        zone: "Asia/Kathmandu",
        tags: ["pool/interviewers"],
        calendar: [block(["2027-06-14T04:15Z", "2027-06-14T06:15Z"], [], "dee-block")],
      },
    ],
  };

  const exhausted = findAvailability(overQuota);
  assert.deepEqual(exhausted.offers, [], "the pool has used its eight interviews for the week");
  assert.deepEqual(exhausted.warnings, [], "every counted block carries its own key");
  assert.ok(hasReason(exhausted, "2027-06-14T03:15Z", ["dee", "cap_reached", "pool_week"]));
});

test("U4/U22: weighted round robin offers confirm with check slot", () => {
  const request = {
    now: "2027-06-13T13:30Z",
    goal: {
      zone: "Australia/Lord_Howe",
      search_window: ["2027-06-13T22:30Z", "2027-06-14T01:30Z"],
      parts: [{ duration: "30m", requirement: { all: [atLeast(1, { tag: "team/sales" })] } }],
    },
    participants: [
      {
        id: "ana",
        zone: "Australia/Lord_Howe",
        tags: ["team/sales"],
        calendar: [block(["2027-06-13T23:30Z", "2027-06-14T00:30Z"])],
      },
      {
        id: "bo",
        zone: "Australia/Lord_Howe",
        tags: ["team/sales"],
        calendar: [block(["2027-06-14T00:00Z", "2027-06-14T00:30Z"])],
      },
      {
        id: "cy",
        zone: "Australia/Lord_Howe",
        tags: ["team/sales"],
        calendar: [
          block(["2027-06-13T22:30Z", "2027-06-13T23:30Z"]),
          block(["2027-06-14T00:00Z", "2027-06-14T00:30Z"]),
        ],
      },
    ],
    rules: [businessHours("mon-sun", "09:00-18:00", { ruleId: "hours" })],
    weights: { bo: 2, cy: 5 },
  };

  const response = findAvailability(request);
  assert.deepEqual(response.offers.map(offerAssignedHosts), [["bo"], ["bo"], ["cy"], ["cy"], ["cy"]],
    "the weights decide the rotation");
  assert.equal(offerCalendarUnknown(response.offers[0]), false);

  const proposal = offerToProposal(response.offers[0]);
  const checked = checkSlot(request, proposal);
  assert.equal(checked.valid, true, "a returned offer always confirms");
  assert.deepEqual(checked.reasons, []);

  const refused = checkSlot(request, {
    ...proposal,
    events: [{ ...proposal.events[0], start: "2027-06-14T00:00Z" }],
  });
  assert.equal(refused.valid, false, "an offered slot moved onto a busy block no longer confirms");
  assert.ok(refused.reasons.some((reason) => reason.code === "blocked"));
});

test("U14: a short horizon and notice bound the dates offered", () => {
  const request = {
    now: "2027-06-17T02:15Z",
    goal: singleGoal("30m", { all: [person("rich")] }, {
      zone: "Asia/Kathmandu",
      searchWindow: ["2027-06-16T18:15Z", "2027-06-23T18:15Z"],
    }),
    participants: [{ id: "rich", zone: "Asia/Kathmandu", calendar: [] }],
    rules: [
      minNotice("1h"),
      horizon({ working_days: 3, calendar: "org" }),
    ],
    calendars: { org: { zone: "Asia/Kathmandu", working_days: "mon,tue,wed,thu,fri" } },
  };

  const response = findAvailability(request);

  assert.equal(offerStart(response.offers[0]), "2027-06-17T03:15Z", "one hour of notice");
  assert.equal(
    response.dates.filter((date) => date.offers > 0).at(-1).date,
    "2027-06-22",
    "Friday, Monday and Tuesday are the three working days the horizon allows",
  );
  assert.equal(response.dates.at(-1).offers, 0, "the date after the horizon is listed but unbookable");
});

test("U15: rest protects the morning after a late finish", () => {
  const request = {
    now: "2027-06-13T13:30Z",
    goal: singleGoal("30m", { all: [person("ana")] }, {
      zone: "Australia/Lord_Howe",
      searchWindow: ["2027-06-14T19:30Z", "2027-06-15T01:30Z"],
    }),
    participants: [
      {
        id: "ana",
        zone: "Australia/Lord_Howe",
        calendar: [block(["2027-06-14T08:00Z", "2027-06-14T09:30Z"])],
      },
    ],
    rules: [businessHours("mon-sun", "06:00-18:00", { ruleId: "hours" }), rest()],
    explain: { limit: 100 },
  };

  const response = findAvailability(request);

  assert.deepEqual(starts(response), [
    "2027-06-14T23:30Z",
    "2027-06-15T00:00Z",
    "2027-06-15T00:30Z",
    "2027-06-15T01:00Z",
  ], "10:00 in Lord Howe, after the evening that ran past 18:00");
  assert.ok(hasReason(response, "2027-06-14T19:30Z", ["ana", "rest", "rest"]));
});

test("U20: the available-only preset limits the attendee to the stated spans", () => {
  const [candidate, availability] = availableOnly("cand", [
    ["2027-06-14T14:00Z", "2027-06-14T15:00Z"],
    ["2027-06-14T15:30Z", "2027-06-14T16:30Z"],
  ]);
  const request = {
    now: "2027-06-14T00:00Z",
    goal: singleGoal("30m", { all: [person("ana")] }, {
      zone: "UTC",
      searchWindow: ["2027-06-14T13:00Z", "2027-06-14T17:00Z"],
    }),
    participants: [{ id: "ana", zone: "UTC", calendar: [] }, candidate],
    rules: [availability],
    explain: { limit: 100 },
  };

  const response = findAvailability(request);

  assert.deepEqual(starts(response), [
    "2027-06-14T14:00Z",
    "2027-06-14T14:30Z",
    "2027-06-14T15:30Z",
    "2027-06-14T16:00Z",
  ]);
  assert.ok(hasReason(response, "2027-06-14T15:00Z", ["cand", "outside_window", "available_only:cand"]));
});

test("available only coalesces spans and splits them at UTC midnight", () => {
  const [candidate, availability] = availableOnly("cand", [
    ["2027-06-14T23:30Z", "2027-06-15T00:30Z"],
    ["2027-06-15T00:30Z", "2027-06-15T01:00Z"],
  ]);

  assert.deepEqual(availability.who, { ids: ["cand"] }, "the window binds only the attendee");
  assert.deepEqual(availability.params.dates, {
    "2027-06-14": ["23:30-24:00"],
    "2027-06-15": ["00:00-01:00"],
  }, "one coalesced span, split at UTC midnight, the first half ending 24:00");

  const request = {
    now: "2027-06-14T00:00Z",
    goal: singleGoal("30m", { all: [person("ana")] }, {
      zone: "UTC",
      searchWindow: ["2027-06-14T23:00Z", "2027-06-15T01:30Z"],
    }),
    participants: [{ id: "ana", zone: "UTC", calendar: [] }, candidate],
    rules: [availability],
  };

  assert.deepEqual(starts(findAvailability(request)), [
    "2027-06-14T23:30Z",
    "2027-06-15T00:00Z",
    "2027-06-15T00:30Z",
  ]);
});

test("available only ceilings the dated entries of the whole availability", () => {
  // One dated entry is written per UTC day of every span, so the ceiling counts the
  // whole availability rather than one span of it: ten years still models, a minute
  // more does not, and two spans that each fit add up to twice as many entries.
  const [, tenYears] = availableOnly("cand", [["2027-01-01T00:00Z", "2037-01-08T00:00Z"]]);
  assert.equal(
    Object.keys(tenYears.params.dates).length,
    3660,
    "ten years models into one key per UTC day",
  );

  const over = rejection(() =>
    availableOnly("cand", [["2027-01-01T00:00Z", "2037-01-08T00:01Z"]]),
  );
  assert.equal(over.code, "invalid_argument");
  assert.match(over.message, /3661 dated entries/);

  const doubled = rejection(() =>
    availableOnly("cand", [
      ["2027-01-01T00:00Z", "2037-01-08T00:00Z"],
      ["2038-01-01T00:00Z", "2048-01-09T00:00Z"],
    ]),
  );
  assert.equal(doubled.code, "invalid_argument");
  assert.match(doubled.message, /7320 dated entries/);
});

test("the ceiling counts the entries written, not the dates they land on", () => {
  // The ceiling bounds what the preset writes: two disjoint slots on one date are
  // two entries under one key. Counting distinct dates instead would let a caller
  // write an unbounded number of entries before the check, so the entries are what
  // is counted — and the same availability stated as whole-day spans is compact.
  const clock = (minute) =>
    `${String(Math.floor(minute / 60)).padStart(2, "0")}:${String(minute % 60).padStart(2, "0")}`;
  const slots = [];
  for (let day = 1; day <= 6; day += 1) {
    for (let minute = 0; minute < 1222; minute += 2) {
      slots.push([`2027-06-0${day}T${clock(minute)}Z`, `2027-06-0${day}T${clock(minute + 1)}Z`]);
    }
  }
  assert.equal(slots.length, 3666, "611 one-minute slots on each of six days");

  const manySlots = rejection(() => availableOnly("cand", slots));
  assert.equal(manySlots.code, "invalid_argument");
  assert.match(manySlots.message, /3666 dated entries/);

  const [, compact] = availableOnly(
    "cand",
    [1, 2, 3, 4, 5, 6].map((day) => [`2027-06-0${day}T00:00Z`, `2027-06-0${day + 1}T00:00Z`]),
  );
  assert.equal(
    Object.keys(compact.params.dates).length,
    6,
    "the same six days stated as spans is six keys",
  );
});

test("a stated span names an instant the engine holds", () => {
  // A window is written as dates, so an endpoint outside the instants the engine
  // holds has no window the engine could read: written anyway, the rule is silently
  // inert — every day of it unreachable — rather than the mistake it is. The range is
  // the engine's own, and the message names it, so a caller reading the rejection
  // knows what to correct instead of being told that a valid RFC 3339 instant is not
  // one.
  for (const instant of [
    "1974-12-31T23:59Z",
    "2100-01-01T00:01Z",
    "0000-01-01T00:00Z",
    "9999-12-31T23:00-05:00",
  ]) {
    const rejected = rejection(() => availableOnly("ana", [[instant, "2027-06-14T10:00Z"]]));
    assert.ok(rejected instanceof OpportuneError, instant);
    assert.equal(rejected.code, "invalid_argument", instant);
    assert.match(rejected.message, /1975-01-01T00:00Z\.\.=2100-01-01T00:00Z/, instant);
  }

  // The ends of the range are instants the engine holds, so a span touching either
  // is written as the dated window it states.
  assert.deepEqual(Object.entries(availableOnly("ana", [["1975-01-01T00:00Z", "1975-01-02T00:00Z"]])[1].params.dates), [
    ["1975-01-01", ["00:00-24:00"]],
  ]);
  assert.deepEqual(Object.entries(availableOnly("ana", [["2099-12-31T12:00Z", "2100-01-01T00:00Z"]])[1].params.dates), [
    ["2099-12-31", ["12:00-24:00"]],
  ]);
});

test("the buffers preset holds the gap after an existing meeting", () => {
  const request = {
    now: "2027-06-14T09:00Z",
    goal: {
      zone: "UTC",
      search_window: ["2027-06-14T10:00Z", "2027-06-14T11:30Z"],
      grid: { step: "10m" },
      parts: [{ duration: "20m", requirement: { all: [person("ana")] } }],
    },
    participants: [
      {
        id: "ana",
        zone: "UTC",
        calendar: [block(["2027-06-14T10:00Z", "2027-06-14T10:40Z"])],
      },
    ],
    rules: [buffers()],
    explain: { limit: 100 },
  };

  const response = findAvailability(request);

  assert.equal(offerStart(response.offers[0]), "2027-06-14T10:50Z", "ten minutes clear first");
  assert.ok(hasReason(response, "2027-06-14T10:40Z", ["ana", "clearance", "buffers"]));
});

/** A tour of 30-minute `sales/demo` bookings for one host, capped weekly in minutes. */
function demoRequest(booked) {
  return {
    now: "2027-06-14T08:00Z",
    goal: {
      zone: "UTC",
      search_window: ["2027-06-14T08:00Z", "2027-06-17T18:00Z"],
      parts: [{ duration: "30m", tags: ["sales/demo"], requirement: { all: [person("ana")] } }],
    },
    participants: [
      {
        id: "ana",
        zone: "UTC",
        calendar: booked.map((span, index) => block(span, ["sales/demo"], `demo/${index}`)),
      },
    ],
    rules: [capPerWeek({ tag: "sales/demo" }, 360, { measure: "minutes" })],
    explain: { limit: 100 },
  };
}

test("U13: a cap measured in minutes bounds the hours of demos", () => {
  const rule = capPerWeek({ tag: "sales/demo" }, 360, { measure: "minutes" });
  assert.deepEqual(
    [rule.params.measure, rule.params.limit, rule.params.period],
    ["minutes", 360, "week"],
  );

  const spent = [
    ["2027-06-14T09:00Z", "2027-06-14T12:00Z"],
    ["2027-06-15T09:00Z", "2027-06-15T12:00Z"],
  ];
  const request = demoRequest(spent);
  assert.deepEqual(validate(request).errors, [], "the engine reads the rule the preset built");

  const exhausted = findAvailability(request);
  assert.deepEqual(exhausted.offers, [], "the week's six hours are already booked");
  assert.ok(hasReason(exhausted, "2027-06-14T08:00Z", ["ana", "cap_reached", "cap_per_week"]));

  const room = findAvailability(
    demoRequest([spent[0], ["2027-06-15T09:00Z", "2027-06-15T11:00Z"]]),
  );
  assert.ok(room.offers.length > 0, "five hours booked leaves an hour of demo time to book");
});

test("a pooled cap demands the zone it is counted in", () => {
  const error = rejection(() => capPerWeek({ tag: "interview" }, 8, { per: "pooled" }));
  assert.ok(error instanceof OpportuneError);
  assert.equal(error.code, "invalid_argument");
  assert.match(error.message, /zone/);

  const pooled = capPerWeek({ tag: "interview" }, 8, { per: "pooled", zone: "event" });
  assert.deepEqual([pooled.params.per, pooled.params.zone], ["pooled", "event"]);

  const each = capPerDay({ tag: "interview" }, 2);
  assert.deepEqual([each.params.per, each.params.zone], ["each", "participant"],
    "the defaults stay the per-participant cap a single-host rule means");
});

test("business hours rejects an argument it could never mean", () => {
  // A pair of clock times is two values, not one range, so the engine would read a
  // rule that closes every day; the preset names the value instead.
  for (const [label, call, expected] of [
    ["two clock times", () => businessHours("mon-fri", ["09:00", "17:00"]), /09:00/],
    ["a list of day keys", () => businessHours(["mon", "tue"]), /days/],
    ["a null not-yet-set hours", () => businessHours("mon-fri", null), /`hours`/],
  ]) {
    const rejected = rejection(call);
    assert.ok(rejected instanceof OpportuneError, label);
    assert.equal(rejected.code, "invalid_argument", label);
    assert.match(rejected.message, expected, label);
  }

  const split = businessHours("mon-fri", ["09:00-12:00", "13:00-17:00", "23:30-24:00"]);
  assert.deepEqual(split.params.weekly, {
    "mon-fri": ["09:00-12:00", "13:00-17:00", "23:30-24:00"],
  });
});

test("the default limits are the engine's own and override cleanly", () => {
  const defaults = defaultLimits();
  assert.deepEqual(defaults, {
    max_participants: 500,
    max_blocks_per_participant: 10_000,
    max_search_window_days: 92,
    max_candidate_starts: 100_000,
    max_explain_limit: 10_000,
    max_rules: 500,
    max_occurrences: 50_000,
    max_requirement_slots: 64,
    max_duration_minutes: 10_080,
    max_zone_periods_per_participant: 1_000,
  });

  const overridden = limitsWith({ max_search_window_days: 120 });
  assert.equal(overridden.max_search_window_days, 120);
  assert.equal(overridden.max_participants, defaults.max_participants);

  // The override is a document the native engine parses, not only a value this
  // package carries: an engine built from it enforces the window it names.
  const narrow = new Engine(limitsWith({ max_search_window_days: 1 }));
  const outOfRange = rejection(() =>
    narrow.findAvailability({
      now: "2027-06-14T08:00Z",
      goal: singleGoal("30m", { all: [person("ana")] }, {
        zone: "UTC",
        searchWindow: ["2027-06-14T08:00Z", "2027-06-18T08:00Z"],
      }),
      participants: [{ id: "ana", zone: "UTC", calendar: [] }],
    }),
  );
  assert.ok(outOfRange instanceof OpportuneError);
  assert.deepEqual(
    outOfRange.errors.map((finding) => [finding.code, finding.path]),
    [["search_window_limit", "/goal/search_window"]],
  );

  for (const invalid of [{ max_participants: -1 }, { no_such_limit: 1 }]) {
    assert.throws(() => new Engine(limitsWith(invalid)), OpportuneError);
  }
});

test("an offer limit truncates the list and an explain limit the exclusions", () => {
  const request = {
    now: "2027-06-14T08:00Z",
    goal: singleGoal("30m", { all: [person("ana")] }, {
      zone: "UTC",
      searchWindow: ["2027-06-14T08:00Z", "2027-06-14T13:00Z"],
    }),
    participants: [{ id: "ana", zone: "UTC", calendar: [] }],
    rules: [businessHours("mon-sun", "09:00-12:00")],
    limit: 2,
  };

  const truncated = findAvailability(request);
  assert.equal(truncated.offers.length, 2);
  assert.equal(truncated.truncated, true);
  assert.equal(truncated.dates[0].offers, 6, "the date still reports every offer it had");

  const explained = findAvailability({ ...request, limit: null, explain: { limit: 1 } });
  assert.equal(explained.offers.length, 6);
  assert.equal(explained.excluded.length, 1);
  assert.equal(explained.excluded_truncated, true);
});

test("buffers min gap keeps two events apart", () => {
  const rule = buffers("0m", "0m", { minGap: "15m" });
  assert.equal(rule.params.min_gap, "15m");

  const request = {
    now: "2027-06-14T08:00Z",
    goal: {
      zone: "UTC",
      search_window: ["2027-06-14T10:30Z", "2027-06-14T12:00Z"],
      candidates: ["2027-06-14T11:00Z", "2027-06-14T11:15Z"],
      parts: [{ duration: "30m", requirement: { all: [person("ana")] } }],
    },
    participants: [
      {
        id: "ana",
        zone: "UTC",
        calendar: [block(["2027-06-14T10:05Z", "2027-06-14T11:00Z"])],
      },
    ],
    rules: [rule],
    explain: { limit: 100 },
  };

  assert.deepEqual(validate(request).errors, []);
  const response = findAvailability(request);

  assert.deepEqual(
    starts(response),
    ["2027-06-14T11:15Z"],
    "five minutes after the meeting is too close; exactly fifteen is enough",
  );
  assert.ok(hasReason(response, "2027-06-14T11:00Z", ["ana", "clearance", "buffers"]));
});

test("dated hours opens a booth only on the conference dates", () => {
  const request = {
    now: "2027-06-14T06:00Z",
    goal: {
      zone: "America/New_York",
      search_window: ["2027-06-14T04:00Z", "2027-06-17T04:00Z"],
      parts: [{ duration: "30m", requirement: { all: [person("team")] } }],
    },
    participants: [{ id: "team", zone: "America/New_York", calendar: [] }],
    rules: [
      businessHours("mon-fri", "08:00-18:00", { ruleId: "hours" }),
      datedHours(
        {
          "2027-06-14": "10:00-16:00",
          "2027-06-15": ["10:00-12:00", "13:00-16:00"],
          "2027-06-16": [],
        },
        { zone: "event" },
      ),
    ],
    explain: { limit: 100 },
  };

  const response = findAvailability(request);

  assert.deepEqual(
    response.dates.map((entry) => `${entry.date}:${entry.offers}`),
    ["2027-06-14:12", "2027-06-15:10", "2027-06-16:0"],
    "the booth's two days, and the conference date its own hours leave closed",
  );
  assert.equal(
    response.offers[0].events[0].start,
    "2027-06-14T14:00Z",
    "10:00 in New York, the booth's opening",
  );
  assert.ok(
    hasReason(response, "2027-06-15T16:00Z", ["team", "outside_window", "dated_hours"]),
    "12:00-13:00 is between the Tuesday sessions",
  );
  assert.ok(
    hasReason(response, "2027-06-16T14:00Z", ["team", "outside_window", "dated_hours"]),
    "a conference date with no hours is closed",
  );
});

test("who and when scope a preset rule", () => {
  const hoursWith = (who) => ({
    now: "2027-06-14T00:00Z",
    goal: {
      zone: "UTC",
      search_window: ["2027-06-14T00:00Z", "2027-06-15T00:00Z"],
      parts: [{ duration: "30m", requirement: { all: [person("ana")] } }],
    },
    participants: [
      { id: "ana", zone: "UTC", calendar: [] },
      { id: "bo", zone: "UTC", calendar: [] },
    ],
    rules: [businessHours("mon-sun", "09:00-17:00", who === undefined ? {} : { who })],
  });

  const everyone = findAvailability(hoursWith(undefined));
  assert.equal(everyone.offers.length, 16, "nine to five in half-hour starts");

  const onlyBo = findAvailability(hoursWith({ ids: ["bo"] }));
  assert.equal(
    onlyBo.offers.length,
    48,
    "ana is the only required host, and the rule constrains bo alone",
  );
  assert.equal(offerStart(onlyBo.offers[0]), "2027-06-14T00:00Z");

  const demoCapped = (when) => ({
    now: "2027-06-14T08:00Z",
    goal: {
      zone: "UTC",
      search_window: ["2027-06-14T08:00Z", "2027-06-15T18:00Z"],
      candidates: ["2027-06-14T14:00Z", "2027-06-15T14:00Z"],
      parts: [
        { duration: "30m", tags: ["sales/demo"], requirement: { all: [person("ana")] } },
      ],
    },
    participants: [
      {
        id: "ana",
        zone: "UTC",
        calendar: [
          block(["2027-06-14T09:00Z", "2027-06-14T09:30Z"], ["sales/demo"], "demo/1"),
        ],
      },
    ],
    rules: [capPerDay({ tag: "sales/demo" }, 1, { when })],
    explain: { limit: 100 },
  });

  const matched = findAvailability(demoCapped({ tag: "sales/demo" }));
  assert.deepEqual(starts(matched), ["2027-06-15T14:00Z"]);

  const unmatched = findAvailability(demoCapped({ tag: "other" }));
  assert.deepEqual(
    starts(unmatched),
    ["2027-06-14T14:00Z", "2027-06-15T14:00Z"],
    "a `when` the part does not match leaves the cap unapplied",
  );
});

test("unless composes the exception into an unconditional and a scoped rule", () => {
  const hours = businessHours("mon-fri", "09:00-17:00", { zone: "UTC" });
  const vip = { tag: "vip" };

  const unconditional = unless(hours, vip);
  assert.deepEqual(unconditional.when, { not: vip });
  assert.deepEqual(unconditional.who, hours.who, "`who` is untouched");
  assert.equal(unconditional.id, hours.id, "the ID is untouched");
  assert.deepEqual(unconditional.params, hours.params, "the kind is untouched");

  const interview = { tag: "interview" };
  const scoped = unless({ ...hours, when: interview }, vip);
  assert.deepEqual(
    scoped.when,
    { all: [interview, { not: vip }] },
    "a scoped rule keeps its scope and adds the exception",
  );

  const twice = unless(scoped, { tag: "board" });
  assert.deepEqual(
    twice.when,
    { all: [scoped.when, { not: { tag: "board" } }] },
    "chaining a second exception nests a new `all` rather than replacing the first",
  );

  // A rule whose `when` was left at the wire's `null` is unconditional too: the
  // field is nullable and the engine reads it as `true`.
  assert.deepEqual(unless({ ...hours, when: null }, vip).when, { not: vip });

  // `true` is the spelling a rule read back from storage carries: the wire writes
  // `when` on every rule, and `true` is what it writes for every booking.
  assert.deepEqual(unless({ ...hours, when: true }, vip).when, { not: vip });

  // A value that is not a rule object is rejected here rather than composed into
  // one: spreading a list of rules, or a primitive, would build an object with no
  // `kind` that the engine rejects at a rule the caller never wrote.
  for (const [notARule, named] of [
    [null, /null/],
    [[hours], /list/],
    ["business_hours", /string/],
  ]) {
    const rejected = rejection(() => unless(notARule, vip));
    assert.ok(rejected instanceof OpportuneError, String(named));
    assert.equal(rejected.code, "invalid_argument", String(named));
    assert.match(rejected.message, /rule/, String(named));
    assert.match(rejected.message, named);
  }
});

test("andWho narrows the wire's default host selector instead of replacing it", () => {
  const hours = businessHours("mon-fri", "09:00-17:00", { zone: "UTC" });
  // A preset leaves `who` off: the wire reads the omitted field as `{ role: "host" }`,
  // which is not `true`.
  assert.equal(hours.who, undefined, "the preset leaves `who` to the wire's default");

  const people = { kind: "person" };
  const narrowed = andWho(hours, people);
  assert.deepEqual(
    narrowed.who,
    { all: [{ role: "host" }, people] },
    "an omitted `who` reads as every host, so narrowing keeps the host clause beside the selector",
  );
  assert.equal(narrowed.kind, "window", "the kind is untouched");
  assert.equal(narrowed.id, hours.id, "the ID is untouched");
  assert.deepEqual(narrowed.params, hours.params, "the parameters are untouched");
  assert.equal(narrowed.when, hours.when, "`when` is untouched");

  // `null` is the same omission spelled out, which is what a rule read back from
  // storage may carry.
  assert.deepEqual(andWho({ ...hours, who: null }, people).who, {
    all: [{ role: "host" }, people],
  });

  // `true` is a value the caller wrote, so it keeps meaning every participant.
  assert.deepEqual(andWho({ ...hours, who: true }, people).who, people);

  // An existing `all` gains one clause rather than nesting, so a chain stays flat.
  assert.deepEqual(andWho(andWho(hours, people), { tag: "senior" }).who, {
    all: [{ role: "host" }, people, { tag: "senior" }],
  });

  // Anything else becomes an `all` of the two.
  assert.deepEqual(andWho({ ...hours, who: { ids: ["ana"] } }, people).who, {
    all: [{ ids: ["ana"] }, people],
  });

  // A value that is not a rule object is rejected by the same guard `unless` uses,
  // so spreading a list or a primitive never builds a rule with no `kind`.
  for (const [notARule, named] of [
    [null, /null/],
    [[hours], /list/],
    ["business_hours", /string/],
    [{ id: "x", params: {} }, /object/],
  ]) {
    const rejected = rejection(() => andWho(notARule, people));
    assert.ok(rejected instanceof OpportuneError, String(named));
    assert.equal(rejected.code, "invalid_argument", String(named));
    assert.match(rejected.message, /rule/, String(named));
    assert.match(rejected.message, named);
  }
});

test("andWho and exceptWho carry a malformed who to the engine instead of throwing", () => {
  const hours = businessHours("mon-fri", "09:00-17:00", { zone: "UTC" });
  const people = { kind: "person" };

  // A JavaScript caller is not type-checked, so `who` is whatever the caller's
  // variable held. The narrowing reads `who` with the `in` operator, which throws
  // a raw TypeError on every primitive, so the shape is confirmed before it is
  // read — and a value that is not a selector object is carried through
  // untouched, for the engine's structured validation to reject at the field the
  // caller wrote.
  for (const who of [0, false, "host", 1n, ["vip"]]) {
    for (const narrow of [andWho, exceptWho]) {
      const rule = narrow({ ...hours, who }, people);
      assert.deepEqual(rule.who, who, `${narrow.name} kept the who it was given`);
      assert.equal(rule.kind, "window", `${narrow.name} kept the rule's kind`);
      assert.equal(rule.id, hours.id, `${narrow.name} kept the rule's ID`);
    }
  }

  // A null-prototype object is a selector the caller wrote, not a malformed one:
  // it is read and wrapped like any other.
  const plain = Object.assign(Object.create(null), { ids: ["ana"] });
  assert.deepEqual(andWho({ ...hours, who: plain }, people).who, { all: [plain, people] });
  assert.deepEqual(exceptWho({ ...hours, who: plain }, people).who, {
    all: [plain, { not: people }],
  });

  // The carried value does reach structured validation: the engine rejects the
  // rule with an OpportuneError naming the selector it wanted, not a TypeError
  // from the preset. (BigInt is left to the preset layer above: it does not
  // survive JSON serialization to reach the engine at all.)
  const booking = (rules) => ({
    now: "2027-06-14T00:00Z",
    goal: singleGoal("30m", { all: [person("ana")] }, {
      zone: "UTC",
      searchWindow: ["2027-06-15T07:00Z", "2027-06-15T07:30Z"],
      candidates: ["2027-06-15T07:00Z"],
    }),
    participants: [{ id: "ana", zone: "UTC", calendar: [] }],
    rules,
  });
  const cases = [
    [0, "invalid_type"],
    [false, "invalid_value"],
    ["host", "invalid_type"],
    [["vip"], "invalid_type"],
  ];
  for (const [who, code] of cases) {
    const error = rejection(() => findAvailability(booking([{ ...rest("22:00", "08:00"), who }])));
    assert.ok(
      error instanceof OpportuneError,
      `who ${String(who)} reached the engine's structured validation, not a ${error?.constructor?.name}`,
    );
    assert.equal(error.code, code, `who ${String(who)}`);
    assert.match(error.message, /selector/, `who ${String(who)}`);
  }

  // A function or a symbol is the one malformed value the engine can never judge:
  // `JSON.stringify` drops the object member holding it, so the wire reads the absent
  // `who` as its own `{ role: "host" }` default and answers a rule the caller never
  // wrote — the narrowing silently gone, a resource room it was meant to keep out
  // still in, and nothing failing. Both entry points refuse it structurally instead,
  // whichever argument carries it.
  for (const unspellable of [() => true, Symbol("vip")]) {
    for (const narrow of [andWho, exceptWho]) {
      const rejected = rejection(() => narrow({ ...hours, who: unspellable }, people));
      assert.ok(
        rejected instanceof OpportuneError,
        `${narrow.name} refused a who the wire cannot carry, not a ${rejected?.constructor?.name}`,
      );
      assert.equal(rejected.code, "invalid_type", `${narrow.name} with an unspellable who`);
      assert.match(rejected.message, /`who`/, `${narrow.name} names the field it refused`);
    }
    // A rule whose own `who` is `true` returns the selector bare, so an unspellable one
    // would vanish the same way and leave the rule at the wire's default.
    const rejected = rejection(() => andWho({ ...hours, who: true }, unspellable));
    assert.ok(rejected instanceof OpportuneError, String(rejected?.constructor?.name));
    assert.equal(rejected.code, "invalid_type", "andWho with an unspellable selector");
    assert.match(rejected.message, /`selector`/, "andWho names the argument it refused");
  }

  // `exceptWho` wraps its selector before `andWho` reads it. A function inside `{ not: }`
  // is written by the wire as `{}` — an object naming no selector — so the wrapped value
  // is now refused at the preset, naming `selector`, rather than being carried to the
  // engine as an empty object. What it must never do is be silently accepted, which the
  // old spelling was one `JSON.stringify` away from.
  const wrapped = rejection(() => exceptWho({ ...hours, who: true }, () => true));
  assert.ok(wrapped instanceof OpportuneError, String(wrapped));
  assert.equal(wrapped.code, "invalid_type", "exceptWho's wrapped selector writes no selector");
  assert.match(wrapped.message, /`selector`/, "exceptWho names the selector it refused");
});

test("a preset refuses a who or when the wire writes as no selector", () => {
  // `who` and `when` are the callers' own selectors, and a rule the preset build leaves
  // them off is read by the engine as its default — every host, every booking. A value
  // the wire writes as no selector at all therefore has to be refused here, because the
  // same rule used to leave the preset with the default the caller meant to override: a
  // personal link that reached every host, or a cap that applied to every booking, with
  // nothing failing. `JSON.stringify` writes no member for a function or a symbol, and
  // writes `{}` for a `Map`, a `Set`, a `RegExp` or an empty object — and `null` for an
  // invalid `Date`, which the wire reads as the default name — so the spelling decides.
  const dropped = [
    ["a function", () => true],
    ["a symbol", Symbol("vip")],
    ["a Map", new Map([["ids", ["ana"]]])],
    ["a Set", new Set(["ana"])],
    ["a RegExp", /ana/],
    ["an empty object", {}],
    ["an invalid Date", new Date(Number.NaN)],
  ];
  for (const [label, unspellable] of dropped) {
    for (const [name, build, field] of [
      ["businessHours", () => businessHours("mon-fri", "09:00-17:00", { who: unspellable }), "who"],
      ["weeklyHours", () => weeklyHours({ mon: "09:00-17:00" }, { when: unspellable }), "when"],
      ["capPerDay", () => capPerDay({ tag: "interview" }, 2, { who: unspellable }), "who"],
      ["rest", () => rest("22:00", "08:00", { when: unspellable }), "when"],
    ]) {
      const rejected = rejection(build);
      assert.ok(
        rejected instanceof OpportuneError,
        `${name} refused ${label} as a ${field}, not a ${rejected?.constructor?.name}`,
      );
      assert.equal(rejected.code, "invalid_type", `${name} with ${label} as ${field}`);
      assert.match(rejected.message, new RegExp(`\`${field}\``), `${name} names the field`);
      assert.match(rejected.message, /wire can carry/, `${name} says why it refused the value`);
    }
  }

  // What the wire does carry is still written exactly as the caller gave it. A selector
  // object and the `null` that is the wire's own default reach the rule unchanged, an
  // absent one stays absent, and `true` stays the unconditional selector.
  const hours = businessHours("mon-fri", "09:00-17:00", { who: { ids: ["ana"] } });
  assert.deepEqual(hours.who, { ids: ["ana"] });
  assert.equal(businessHours("mon-fri", "09:00-17:00", { who: null }).who, null);
  assert.equal(businessHours("mon-fri", "09:00-17:00", { who: true }).who, true);
  assert.equal("who" in businessHours(), false, "an absent who is still absent");

  // The check reads the JSON spelling, not the keys: a class instance whose own fields
  // are the members the wire reads is a selector, a boxed boolean spells the `true` the
  // wire reads as unconditional, and an object with a `toJSON` is whatever that returns.
  class Ids {
    constructor() {
      this.ids = ["ana"];
    }
  }
  for (const [label, value, spelled] of [
    ["a class instance's own fields", new Ids(), { ids: ["ana"] }],
    ["a boxed true", new Boolean(true), true],
    ["an object with a toJSON", { toJSON: () => ({ tag: "vip" }) }, { tag: "vip" }],
  ]) {
    const rule = businessHours("mon-fri", "09:00-17:00", { who: value });
    assert.deepEqual(
      JSON.parse(JSON.stringify(rule)).who,
      spelled,
      `${label} is written as the selector it spells`,
    );
  }

  // A `Date` is not silently emptied: a valid one spells its instant, so the rule carries
  // it and the engine rejects it at the field the caller wrote, with the pointer.
  const asDate = businessHours("mon-fri", "09:00-17:00", {
    who: new Date("2027-06-14T00:00:00Z"),
  });
  assert.equal(
    JSON.parse(JSON.stringify(asDate)).who,
    "2027-06-14T00:00:00.000Z",
    "a valid Date is carried as its spelling rather than read as an empty selector",
  );
  const dated = rejection(() =>
    findAvailability({
      now: "2027-06-14T00:00Z",
      goal: singleGoal("30m", { all: [atLeast(1, { role: "host" })] }, {
        zone: "UTC",
        searchWindow: ["2027-06-15T09:00Z", "2027-06-15T11:00Z"],
      }),
      participants: [{ id: "ana", role: "host", zone: "UTC", calendar: [] }],
      rules: [asDate],
    }),
  );
  assert.equal(dated.code, "invalid_type", "the engine rejects the carried Date at its field");
  assert.match(dated.message, /selector/, "the engine names the selector it wanted");

  // The rule the caller wrote is what the engine evaluates, so a selector carried here
  // is visible in the answer: the rule constrains `ana` alone, and `tom` — a host in a
  // zone whose working day is over at this instant — stays an eligible candidate. A
  // `who` the wire dropped would leave the host default in place, and the same rule
  // would have excluded him.
  const request = {
    now: "2027-06-14T00:00Z",
    goal: singleGoal("30m", { all: [atLeast(1, { role: "host" })] }, {
      zone: "UTC",
      searchWindow: ["2027-06-15T09:00Z", "2027-06-15T11:00Z"],
      candidates: ["2027-06-15T09:00Z"],
    }),
    participants: [
      { id: "ana", role: "host", zone: "UTC", calendar: [] },
      { id: "tom", role: "host", zone: "Asia/Tokyo", calendar: [] },
    ],
    rules: [businessHours("mon-fri", "09:00-17:00", { who: { ids: ["ana"] } })],
  };
  const offered = findAvailability(request);
  assert.equal(offered.offers.length, 1, "the request offers its candidate start");
  assert.deepEqual(
    offered.offers[0].events[0].eligible,
    [["ana", "tom"]],
    "the who the caller wrote reached the engine, so the unconstrained host stays eligible",
  );
});

test("andWho keeps a host rule off a resource room and off an attendee", () => {
  // Everyone here finished late on Monday, so a `rest` rule's reach is exactly what
  // decides whether Tuesday's 07:00 start is offered: with the default `who` the rule
  // protects the night after any host's late meeting, and narrowing it to people must
  // leave a room's early morning alone without letting the attendee's late meeting in.
  const late = (id, extra) => ({
    id,
    zone: "UTC",
    calendar: [block(["2027-06-14T22:00Z", "2027-06-14T23:30Z"])],
    ...extra,
  });
  const free = (id) => ({ id, zone: "UTC", calendar: [] });
  const booking = (participants, rules) => ({
    now: "2027-06-14T00:00Z",
    goal: singleGoal("30m", { all: [person(participants[0].id)] }, {
      zone: "UTC",
      searchWindow: ["2027-06-15T07:00Z", "2027-06-15T07:30Z"],
      candidates: ["2027-06-15T07:00Z"],
    }),
    participants,
    rules,
    explain: { limit: 100 },
  });
  const night = () => rest("22:00", "08:00");
  const people = { kind: "person" };

  // A room is a host, so the default `who` reaches it and the rest rule holds the slot.
  const roomOnly = [late("room-4", { kind: "resource" })];
  assert.deepEqual(starts(findAvailability(booking(roomOnly, [night()]))), []);
  assert.deepEqual(
    starts(findAvailability(booking(roomOnly, [andWho(night(), people)]))),
    ["2027-06-15T07:00Z"],
    "`andWho(person)` keeps a resource room out of a host rule",
  );

  // A human host and an attendee who finished late: the default `who` never reaches
  // the attendee, so the start is offered either way — which is why replacing `who`
  // is the call that changes the answer.
  const pair = [free("ana"), late("tom", { role: "attendee" })];
  assert.deepEqual(starts(findAvailability(booking(pair, [night()]))), ["2027-06-15T07:00Z"]);
  assert.deepEqual(
    starts(findAvailability(booking(pair, [andWho(night(), people)]))),
    ["2027-06-15T07:00Z"],
    "`andWho(person)` kept the default `Role.Host` clause, so the attendee stays out",
  );
  assert.deepEqual(
    starts(findAvailability(booking(pair, [rest("22:00", "08:00", { who: people })]))),
    [],
    "the twin call that replaces `who` drops the host clause and reaches the attendee",
  );
});

test("exceptWho exempts the named host and leaves every other participant alone", () => {
  const cap = exceptWho(capPerDay({ tag: "meeting" }, 1), { ids: ["ceo"] });
  assert.deepEqual(
    cap.who,
    { all: [{ role: "host" }, { not: { ids: ["ceo"] } }] },
    "the exception is `not(selector)` intersected with the default host clause",
  );
  assert.equal(cap.kind, "cap", "the rule type is preserved");
  assert.equal(cap.id, "cap_per_day", "the ID is untouched");
  assert.deepEqual(
    exceptWho({ ...cap, who: undefined }, { ids: ["ceo"] }).who,
    { all: [{ role: "host" }, { not: { ids: ["ceo"] } }] },
    "an omitted `who` is still every host, never every participant",
  );

  const meeting = (id, extra = {}) => ({
    id,
    zone: "UTC",
    calendar: [block(["2027-06-14T09:00Z", "2027-06-14T09:30Z"], ["meeting"])],
    ...extra,
  });
  const free = (id) => ({ id, zone: "UTC", calendar: [] });
  const asks = (host, participants) => ({
    now: "2027-06-14T08:00Z",
    goal: singleGoal("30m", { all: [person(host)] }, {
      zone: "UTC",
      searchWindow: ["2027-06-14T14:00Z", "2027-06-14T14:30Z"],
      candidates: ["2027-06-14T14:00Z"],
      tags: ["meeting"],
    }),
    participants,
    rules: [exceptWho(capPerDay({ tag: "meeting" }, 1), { ids: ["ceo"] })],
    explain: { limit: 100 },
  });

  // An ordinary host already holds Monday's one meeting, so the cap reaches her.
  // `ceo` rides along because `who` names her, not because she is judged here.
  const ana = findAvailability(asks("ana", [meeting("ana"), free("ceo")]));
  assert.deepEqual(starts(ana), [], "the cap still binds a host it does not except");
  assert.ok(
    hasReason(ana, "2027-06-14T14:00Z", ["ana", "cap_reached", "cap_per_day"]),
    JSON.stringify(reasonsAt(ana, "2027-06-14T14:00Z")),
  );

  // The excepted host is the same shape, and the cap does not judge her.
  assert.deepEqual(
    starts(findAvailability(asks("ceo", [meeting("ceo")]))),
    ["2027-06-14T14:00Z"],
    "`exceptWho` narrows `who` instead of replacing it, so the exempt host fits",
  );

  // A free host carrying an attendee whose day is full: keeping the default host
  // clause is what leaves the attendee out of the cap's judgement.
  assert.deepEqual(
    starts(
      findAvailability(
        asks("ana", [free("ana"), meeting("tom", { role: "attendee" }), free("ceo")]),
      ),
    ),
    ["2027-06-14T14:00Z"],
    "the default `Role.Host` clause keeps an attendee out of the cap",
  );

  // Reaching people by replacing `who` instead drags the attendee in, which is the
  // footgun `exceptWho` exists to avoid.
  const withoutHostClause = exceptWho(
    capPerDay({ tag: "meeting" }, 1, { who: { kind: "person" } }),
    { ids: ["ceo"] },
  );
  assert.deepEqual(
    starts(
      findAvailability({
        ...asks("ana", [free("ana"), meeting("tom", { role: "attendee" }), free("ceo")]),
        rules: [withoutHostClause],
      }),
    ),
    [],
    "`who: { kind: person }` replaces the host clause, so the attendee is judged",
  );
});

test("the excepted tag is exempt from the global and group windows", () => {
  const rules = [
    unless(businessHours("mon-fri", "09:00-17:00", { zone: "UTC" }), { tag: "vip" }),
    unless(
      businessHours("mon-fri", "09:00-12:00", {
        zone: "UTC",
        ruleId: "interview_hours",
        when: { tag: "interview" },
      }),
      { tag: "vip" },
    ),
  ];
  const hours = (partTags) => ({
    now: "2027-06-14T00:00Z",
    goal: {
      zone: "UTC",
      search_window: ["2027-06-14T00:00Z", "2027-06-20T00:00Z"],
      candidates: ["2027-06-14T10:00Z", "2027-06-14T14:00Z", "2027-06-19T10:00Z"],
      parts: [{ duration: "30m", tags: partTags, requirement: { all: [person("ana")] } }],
    },
    participants: [{ id: "ana", zone: "UTC", calendar: [] }],
    rules,
    explain: { limit: 100 },
  });

  assert.deepEqual(
    starts(findAvailability(hours(["interview"]))),
    ["2027-06-14T10:00Z"],
    "an ordinary interview gets only the hours both tiers are open",
  );
  assert.deepEqual(starts(findAvailability(hours(["interview", "vip"]))), [
    "2027-06-14T10:00Z",
    "2027-06-14T14:00Z",
    "2027-06-19T10:00Z",
  ], "the excepted booking is exempt from both windows");
});

test("horizon accepts a duration, a date and a calendar day count", () => {
  assert.deepEqual(horizon("4h").params.max, { duration: "4h" });
  assert.deepEqual(horizon("1h30m").params.max, { duration: "1h30m" });
  assert.deepEqual(horizon("2027-07-01").params.max, { date: "2027-07-01" });
  assert.deepEqual(horizon(7).params.max, { days: 7 });
  assert.deepEqual(
    horizon({ working_days: 3, calendar: "org" }).params.max,
    { working_days: 3, calendar: "org" },
    "a typed form passes through unchanged",
  );

  for (const shorthand of ["14d", "soon", "2027-02-30", -1, 1.5]) {
    const rejected = rejection(() => horizon(shorthand));
    assert.ok(rejected instanceof OpportuneError, String(shorthand));
    assert.equal(rejected.code, "invalid_argument", String(shorthand));
    assert.match(rejected.message, new RegExp(String(shorthand).replace(".", "\\.")), String(shorthand));
  }

  for (const shorthand of ["14d", "soon"]) {
    const rejected = rejection(() => horizon(shorthand));
    assert.match(
      rejected.message,
      /is not a duration such as/,
      "the message says what the value is not, and names the working-days form",
    );
    assert.match(rejected.message, /\{ calendar, working_days \}/, rejected.message);
  }

  const request = {
    now: "2027-06-14T08:00Z",
    goal: singleGoal("30m", { all: [person("ana")] }, {
      zone: "UTC",
      searchWindow: ["2027-06-14T08:00Z", "2027-06-24T08:00Z"],
    }),
    participants: [{ id: "ana", zone: "UTC", calendar: [] }],
    rules: [horizon(2)],
  };

  const bounded = findAvailability(request);
  assert.equal(
    bounded.dates.filter((entry) => entry.offers > 0).at(-1).date,
    "2027-06-16",
    "two calendar days after today is the last date a start may fall on",
  );
});

test("horizon refuses a missing maximum instead of dropping the bound", () => {
  for (const absent of [null, undefined]) {
    const rejected = rejection(() => horizon(absent));
    assert.ok(rejected instanceof OpportuneError, String(absent));
    assert.equal(rejected.code, "invalid_argument", String(absent));
    assert.match(rejected.message, new RegExp(String(absent)), String(absent));
  }
});

test("horizon refuses an object that is no generated form", () => {
  // The four generated forms are the schema's own: a `duration`, a `date`, a `days`
  // count, or `calendar` with `working_days`. Any other object passes the type but
  // has no field the engine reads, so sent as it stands the rule would be rejected
  // with a pointer into a rule the caller never wrote — or, for an object whose own
  // shape happens to look like one, enforce a bound the caller meant differently.
  const bogus = [
    {},
    { duration: "14d" },
    { duration: "4h", days: 7 },
    { duration: "4h", extra: true },
    { date: "2027-02-30" },
    { days: "7" },
    { days: -1 },
    { calendar: "org" },
    { working_days: 3 },
    { calendar: "org", working_days: 3, extra: 1 },
    { days: 7, calendar: "org" },
    ["4h"],
    new Date(),
    true,
  ];
  for (const value of bogus) {
    const rejected = rejection(() => horizon(value));
    assert.ok(rejected instanceof OpportuneError, JSON.stringify(value));
    assert.equal(rejected.code, "invalid_argument", JSON.stringify(value));
    assert.match(rejected.message, /is not a duration such as/, JSON.stringify(value));
  }

  // The forms themselves still pass through unchanged, in either field order.
  assert.deepEqual(horizon({ calendar: "org", working_days: 3 }).params.max, {
    calendar: "org",
    working_days: 3,
  });
  assert.deepEqual(horizon({ duration: "4h" }).params.max, { duration: "4h" });
});

test("the four generated maximum forms are documents the engine reads", () => {
  // A caller may write a `lead_time` maximum as the generated document instead of the
  // shorthand, and the guard above copies rather than forwards it — so every one of
  // the four forms has to be recognised by shape, not only the two the shorthand
  // exercises. A branch that stopped matching `date` or `days` still leaves every
  // shorthand case green while refusing a document the schema states, which is a
  // generated value the caller cannot correct.
  for (const form of [
    { duration: "4h" },
    { date: "2027-07-01" },
    { days: 7 },
    { calendar: "org", working_days: 3 },
    { working_days: 3, calendar: "org" },
  ]) {
    assert.deepEqual(horizon(form).params.max, form, JSON.stringify(form));
    assert.notEqual(horizon(form).params.max, form, "the guard copies the form it accepts");
  }

  // The copy is the wire document, so the engine is what has to read it: a bound the
  // engine refused would be the preset handing back a rule no request can carry.
  const request = {
    now: "2027-06-14T00:00Z",
    goal: singleGoal("30m", { all: [person("ana")] }, {
      zone: "UTC",
      searchWindow: ["2027-06-14T00:00Z", "2027-06-20T00:00Z"],
    }),
    participants: [{ id: "ana", zone: "UTC", calendar: [] }],
    rules: [horizon({ date: "2027-07-01" })],
    calendars: { org: { zone: "UTC", working_days: "mon,tue,wed,thu,fri" } },
  };
  assert.deepEqual(validate(request).errors, [], "the engine reads the copied date document");
});

test("horizon names the object it refuses, not the JavaScript type it arrived as", () => {
  // The rejection exists so a caller reads back what they wrote where the schema reads
  // a form. `{ hours: 4 }` is the mistake this guard is for — `hours` is not a field
  // the wire names, and a message reading `[object Object]` sends its author to the
  // wrong place, which is the diagnostic the pointer into a rule the caller never
  // wrote was already giving them.
  const shown = (value) => rejection(() => horizon(value)).message;
  assert.match(shown({ hours: 4 }), /\{hours: 4\}/, "the message names the member that was written");
  assert.match(shown({ duration: "4h", extra: true }), /extra: true/);

  // A member is shown by the text it has, so the message never names a document the
  // caller did not write: `JSON.stringify` would drop the `undefined` that caused the
  // refusal and write `Infinity` as `null`, and an object that is not a mapping would
  // become `{}` — telling the author of a `Date` or a class instance that they passed
  // an empty object. Each of these is a value a caller has to read back verbatim.
  assert.match(shown({ days: "7" }), /\{days: "7"\}/, "a string and a number are not one member");
  assert.match(shown({ duration: "4h", days: undefined }), /days: undefined/);
  assert.match(shown({ days: Infinity }), /days: Infinity/);
  assert.match(shown(new Map([["duration", "4h"]])), /\bmap\b/);
  assert.match(shown(new (class Calendar {})()), /\bcalendar\b/);

  // The message is built from the caller's own value, so it must not become a second
  // failure or grow with it: a value with no text of its own is still this rejection,
  // and a large object is echoed only far enough to identify it.
  const circular = {};
  circular.self = circular;
  assert.equal(rejection(() => horizon(circular)).code, "invalid_argument");
  const huge = Object.fromEntries(
    Array.from({ length: 4000 }, (_, index) => [`field_${index}`, index]),
  );
  assert.ok(
    shown(huge).length < 300,
    `the message echoes the whole object (${shown(huge).length} characters)`,
  );
});

test("a stated-availability endpoint is an instant, not the date it is written with", () => {
  // The supported range is the engine's, and the engine holds instants: an endpoint
  // written in another offset is the instant it spells, so 1975-01-01T00:00Z written
  // as `1974-12-31T19:00-05:00` is inside the range and the civil date `1974-12-31`
  // it carries is not what the check reads. A minute before the range written with a
  // date inside it — `1975-01-01T00:59+01:00` — is outside, and refusing it by the
  // date on its face would write a window over an instant the engine cannot hold.
  const [, min] = availableOnly("cand", [
    ["1974-12-31T19:00-05:00", "1975-01-02T00:00Z"],
  ]);
  assert.deepEqual(Object.entries(min.params.dates), [["1975-01-01", ["00:00-24:00"]]]);
  assert.deepEqual(
    validate({
      now: "1975-01-01T00:00Z",
      goal: singleGoal("30m", { all: [person("cand")] }, {
        zone: "UTC",
        searchWindow: ["1975-01-01T00:00Z", "1975-01-02T00:00Z"],
      }),
      participants: [{ id: "cand", zone: "UTC", calendar: [] }],
      rules: [min],
    }).errors,
    [],
    "the engine reads the window at its own earliest instant",
  );

  const before = rejection(() =>
    availableOnly("cand", [["1975-01-01T00:59+01:00", "1975-01-02T00:00Z"]]),
  );
  assert.equal(before.code, "invalid_argument");
  assert.match(before.message, /1975-01-01T00:00Z\.\.=2100-01-01T00:00Z/);
});

test("a window mapping may be built without a prototype, as a caller's own dictionary", () => {
  // `Object.create(null)` is how a caller keeps keys a literal would swallow — the
  // `__proto__` a stored document carries is the case the preset's own comment names
  // — so a mapping with no prototype is a mapping and is read as written. A guard
  // that demanded `Object.prototype` exactly would refuse the dictionary that form
  // exists for and accept only the literal.
  const dictionary = Object.create(null);
  dictionary["2027-06-14"] = "10:00-16:00";
  dictionary["__proto__"] = "09:00-10:00";
  assert.deepEqual(Object.entries(datedHours(dictionary).params.dates), [
    ["2027-06-14", ["10:00-16:00"]],
    ["__proto__", ["09:00-10:00"]],
  ]);
  assert.deepEqual(Object.entries(weeklyHours(dictionary).params.weekly), [
    ["2027-06-14", ["10:00-16:00"]],
    ["__proto__", ["09:00-10:00"]],
  ]);
});

test("who and when scope the lead time and clearance presets", () => {
  const notice = (when) => {
    const request = {
      now: "2027-06-14T08:00Z",
      goal: {
        zone: "UTC",
        search_window: ["2027-06-14T08:00Z", "2027-06-14T18:00Z"],
        candidates: ["2027-06-14T09:00Z"],
        parts: [
          { duration: "30m", tags: ["demo"], requirement: { all: [person("ana")] } },
        ],
      },
      participants: [{ id: "ana", zone: "UTC", calendar: [] }],
      rules: [minNotice("4h", { when })],
    };
    return starts(findAvailability(request));
  };

  assert.deepEqual(notice({ tag: "demo" }), [], "the notice binds the matching part");
  assert.deepEqual(
    notice({ tag: "other" }),
    ["2027-06-14T09:00Z"],
    "a `when` the part does not match leaves the notice unapplied",
  );

  const minimumGap = (who) => {
    const request = {
      now: "2027-06-14T08:00Z",
      goal: {
        zone: "UTC",
        search_window: ["2027-06-14T08:00Z", "2027-06-14T18:00Z"],
        candidates: ["2027-06-14T10:00Z", "2027-06-14T11:30Z"],
        parts: [{ duration: "30m", requirement: { all: [person("ana")] } }],
      },
      participants: [
        {
          id: "ana",
          zone: "UTC",
          calendar: [block(["2027-06-14T11:00Z", "2027-06-14T11:30Z"], [], "m1")],
        },
        { id: "bo", zone: "UTC", calendar: [] },
      ],
      rules: [buffers("0m", "0m", { minGap: "45m", who })],
    };
    return starts(findAvailability(request));
  };

  assert.deepEqual(
    minimumGap({ ids: ["ana"] }),
    [],
    "thirty minutes before the meeting and none after are both under the gap",
  );
  assert.deepEqual(
    minimumGap({ ids: ["bo"] }),
    ["2027-06-14T10:00Z", "2027-06-14T11:30Z"],
    "ana's meeting does not constrain a rule scoped to bo",
  );
});

test("a monthly cap rolls at the calendar month", () => {
  const request = {
    now: "2027-06-14T08:00Z",
    goal: {
      zone: "UTC",
      search_window: ["2027-06-14T08:00Z", "2027-07-02T18:00Z"],
      candidates: ["2027-06-16T14:00Z", "2027-07-01T14:00Z"],
      parts: [
        { duration: "30m", tags: ["sales/demo"], requirement: { all: [person("ana")] } },
      ],
    },
    participants: [
      {
        id: "ana",
        zone: "UTC",
        calendar: [
          block(["2027-06-14T09:00Z", "2027-06-14T10:00Z"], ["sales/demo"], "demo/1"),
          block(["2027-06-15T09:00Z", "2027-06-15T10:00Z"], ["sales/demo"], "demo/2"),
        ],
      },
    ],
    rules: [capPerMonth({ tag: "sales/demo" }, 2)],
    explain: { limit: 100 },
  };

  assert.deepEqual(validate(request).errors, [], "the engine reads the rule the preset built");
  const response = findAvailability(request);

  assert.deepEqual(
    starts(response),
    ["2027-07-01T14:00Z"],
    "June's two demos exhaust the month; July's bucket is empty",
  );
  assert.ok(hasReason(response, "2027-06-16T14:00Z", ["ana", "cap_reached", "cap_per_month"]));
});

test("offer reserve returns the travel every event holds", () => {
  // A multi-part offer holds travel per part: reading only the first event's
  // reserve would pass every single-part fixture while dropping the holds a caller
  // writes for the later parts of one visit.
  const toTheVisit = {
    kind: "travel",
    participant: "bob",
    span: ["2027-03-08T20:30Z", "2027-03-08T20:55Z"],
  };
  const toTheFollowUp = {
    kind: "travel",
    participant: "ana",
    span: ["2027-03-08T21:00Z", "2027-03-08T21:20Z"],
  };
  const offer = {
    events: [
      {
        assignment: [["bob"]],
        eligible: [["bob"]],
        end: "2027-03-08T21:00Z",
        part: "visit",
        reserve: [toTheVisit],
        start: "2027-03-08T20:55Z",
      },
      {
        assignment: [["ana"]],
        eligible: [["ana"]],
        end: "2027-03-08T21:50Z",
        part: "follow_up",
        reserve: [toTheFollowUp],
        start: "2027-03-08T21:20Z",
      },
    ],
    flags: [],
  };

  assert.deepEqual(
    offerReserve(offer),
    [toTheVisit, toTheFollowUp],
    "every event's holds, in the order the offer's events are in",
  );
});

test("preset arguments the models would refuse are the packages error", () => {
  // A shape no rule could mean is rejected where it is written; a value the schema
  // answers with a pointer (a negative limit) is the engine's own `invalid_request`.
  for (const [call, named] of [
    [() => datedHours([["2027-06-14", "10:00-16:00"]]), /dates/],
    [() => datedHours(new Map([["2027-06-14", "10:00-16:00"]])), /map/],
    [() => datedHours(new Set(["2027-06-14"])), /set/],
    [() => datedHours({ "2027-06-14": "10:00" }), /dates\[2027-06-14\]/],
    // An object that is not a plain mapping has no enumerable keys of its own, so
    // reading it as one closed every day of the window instead of saying that the
    // value was never a mapping: a `Date`, a pattern and an instance are the three
    // shapes a caller reaches for by mistake.
    [() => datedHours(new Date()), /dates/],
    [() => datedHours(/a/), /dates/],
    [() => datedHours(new (class Calendar {})()), /dates/],
    [() => weeklyHours(Object.create({ mon: "09:00-17:00" })), /weekly/],
  ]) {
    const rejected = rejection(call);
    assert.ok(rejected instanceof OpportuneError, String(named));
    assert.equal(rejected.code, "invalid_argument", String(named));
    assert.match(rejected.message, named);
  }

  // The type the message names is the one the caller wrote: `object` would say
  // nothing about the `Date` it passed where a mapping belonged.
  assert.match(rejection(() => datedHours(new Date())).message, /\bdate\b/);
  assert.match(rejection(() => datedHours(new (class Calendar {})())).message, /calendar/);
});

/**
 * The U2 schedule every binding writes from the same inputs: per-weekday hours, a
 * split shift, a midnight-crossing range, a closed date and an override, scoped to
 * one attendee and one kind of booking.
 */
const U2_WEEKLY_HOURS = {
  id: "weekly_hours",
  kind: "window",
  who: { ids: ["ana"] },
  when: { tag: "interview" },
  params: {
    zone: "event",
    weekly: {
      mon: ["09:00-17:00"],
      tue: ["09:00-12:00", "14:00-18:00"],
      wed: ["09:00-17:00"],
      thu: ["10:00-16:00"],
      fri: ["22:00-02:00"],
    },
    dates: { "2027-06-15": ["10:00-12:00"], "2027-12-25": [] },
  },
};

test("weekly hours writes the document every binding reads", () => {
  const rule = weeklyHours(
    {
      mon: "09:00-17:00",
      tue: ["09:00-12:00", "14:00-18:00"],
      wed: "09:00-17:00",
      thu: "10:00-16:00",
      fri: "22:00-02:00",
    },
    {
      zone: "event",
      dates: { "2027-06-15": "10:00-12:00", "2027-12-25": [] },
      who: { ids: ["ana"] },
      when: { tag: "interview" },
    },
  );

  assert.deepEqual(rule, U2_WEEKLY_HOURS);
});

test("U2: weekly hours offers only the days and hours it names", () => {
  const request = {
    now: "2027-06-14T00:00Z",
    goal: singleGoal("30m", { all: [person("ana")] }, {
      zone: "UTC",
      searchWindow: ["2027-06-14T00:00Z", "2027-06-18T00:00Z"],
    }),
    participants: [{ id: "ana", zone: "UTC", calendar: [] }],
    rules: [
      weeklyHours(
        {
          mon: "09:00-17:00",
          tue: ["09:00-12:00", "14:00-18:00"],
          wed: "09:00-17:00",
          thu: "10:00-16:00",
        },
        // The Monday holiday is closed and Wednesday gets its own hours, both
        // overriding the weekday key.
        { dates: { "2027-06-14": [], "2027-06-16": ["13:00-15:00"] }, zone: "UTC" },
      ),
    ],
  };

  const offered = starts(findAvailability(request));

  assert.ok(!offered.includes("2027-06-14T09:00Z"), "the closed date overrides Monday's hours");
  assert.ok(offered.includes("2027-06-15T09:00Z"), "Tuesday's morning session");
  assert.ok(!offered.includes("2027-06-15T13:00Z"), "Tuesday's lunch is closed");
  assert.ok(offered.includes("2027-06-15T14:00Z"), "Tuesday's afternoon session");
  assert.ok(!offered.includes("2027-06-16T09:30Z"), "the date replaces Wednesday's shared hours");
  assert.ok(offered.includes("2027-06-16T13:00Z"), "the date's own hours");
  assert.ok(!offered.includes("2027-06-16T15:00Z"), "and nothing after them");
  assert.ok(!offered.includes("2027-06-17T09:30Z"), "Thursday starts at 10:00");
  assert.ok(offered.includes("2027-06-17T10:00Z"));
  assert.ok(!offered.includes("2027-06-17T16:00Z"), "a 30-minute booking cannot fit at 16:00");
  assert.ok(!offered.some((start) => start.startsWith("2027-06-18")), "Friday has no hours");
});

test("weekly hours rejects a shape it could never mean and leaves overlap to the engine", () => {
  const shape = (call, named) => {
    const rejected = rejection(call);
    assert.ok(rejected instanceof OpportuneError, String(named));
    assert.equal(rejected.code, "invalid_argument", String(named));
    assert.match(rejected.message, named);
  };
  shape(() => weeklyHours(["mon"]), /weekly/);
  shape(() => weeklyHours(new Map([["mon", "09:00-17:00"]])), /map/);
  shape(() => weeklyHours({ mon: "9-5" }), /weekly\[mon\]/);
  shape(() => weeklyHours({}, { dates: { "2027-06-14": "10:00" } }), /dates\[2027-06-14\]/);

  const overlapping = rejection(() =>
    findAvailability({
      now: "2027-06-14T00:00Z",
      goal: singleGoal("30m", { all: [person("ana")] }, {
        zone: "UTC",
        searchWindow: ["2027-06-14T00:00Z", "2027-06-15T00:00Z"],
      }),
      participants: [{ id: "ana", zone: "UTC", calendar: [] }],
      rules: [weeklyHours({ mon: ["09:00-12:00", "11:00-13:00"] }, { zone: "UTC" })],
    }),
  );
  assert.ok(overlapping instanceof OpportuneError);
  assert.equal(overlapping.code, "window_ranges_overlap");
  assert.equal(overlapping.path, "/rules/0/params/weekly/mon");
});

test("a window mapping keeps every key the caller wrote, for the engine to judge", () => {
  // A mapping read from a stored document carries an own `__proto__` key when the
  // document names one. Writing that key into the accumulator would set the
  // accumulator's prototype instead of a member, so the key would vanish: a rule
  // the engine rejects would arrive as a window closed everywhere, silently. Every
  // key the caller wrote has to arrive as written, for the engine to reject.
  const weekly = JSON.parse('{"__proto__": "09:00-17:00"}');
  assert.deepEqual(Object.entries(weeklyHours(weekly, { zone: "UTC" }).params.weekly), [
    ["__proto__", ["09:00-17:00"]],
  ]);
  assert.deepEqual(Object.entries(datedHours(weekly).params.dates), [
    ["__proto__", ["09:00-17:00"]],
  ]);

  const rejected = rejection(() =>
    validate({
      now: "2027-06-14T00:00Z",
      goal: singleGoal("30m", { all: [person("ana")] }, {
        zone: "UTC",
        searchWindow: ["2027-06-14T00:00Z", "2027-06-18T00:00Z"],
      }),
      participants: [{ id: "ana", zone: "UTC", calendar: [] }],
      rules: [weeklyHours(weekly, { zone: "UTC" })],
    }),
  );
  assert.ok(rejected instanceof OpportuneError, String(rejected));
  assert.equal(rejected.code, "weekday_invalid", "the key reaches the engine's own check");
  assert.equal(rejected.path, "/rules/0/params/weekly/__proto__");
});

test("available only for two attendees composes in one request", () => {
  // Each attendee states different availability, so the offers are the
  // intersection: either rule alone would offer a start the other forbids.
  const anaSpans = [["2027-06-14T10:00Z", "2027-06-14T11:00Z"]];
  const bobSpans = [["2027-06-14T10:30Z", "2027-06-14T11:30Z"]];
  const [ana, anaAvailability] = availableOnly("ana", anaSpans);
  const [bob, bobAvailability] = availableOnly("bob", bobSpans);
  assert.deepEqual(
    [anaAvailability.id, bobAvailability.id],
    ["available_only:ana", "available_only:bob"],
    "the attendee decides the rule ID",
  );

  const [, renamed] = availableOnly("cy", anaSpans, { ruleId: "cy_available" });
  assert.equal(renamed.id, "cy_available", "an explicit ID still wins");

  const request = {
    now: "2027-06-14T00:00Z",
    goal: singleGoal("30m", { all: [person("xx")] }, {
      zone: "UTC",
      searchWindow: ["2027-06-14T09:00Z", "2027-06-14T12:00Z"],
    }),
    participants: [{ id: "xx", zone: "UTC", calendar: [] }, ana, bob],
    rules: [anaAvailability, bobAvailability],
  };

  assert.deepEqual(
    validate(request).errors,
    [],
    "two attendees' availability is two rules, not one ID twice",
  );
  assert.deepEqual(starts(findAvailability(request)), [
    "2027-06-14T10:30Z",
  ], "both attendees' stated availability is in force, not just one of them");
});

test("single goal carries the fields a real booking link needs", () => {
  const travel = {
    before: "15m",
    after: "15m",
    by_participant: { ana: { before: "30m", after: "30m" } },
  };
  const goal = singleGoal("45m", { all: [person("ana")] }, {
    zone: "Europe/London",
    searchWindow: ["2027-06-14T08:00Z", "2027-06-15T08:00Z"],
    displayZone: "America/New_York",
    grid: { step: "15m" },
    partId: "interview",
    tags: ["interview/front-end"],
    travel,
  });

  assert.equal(goal.display_zone, "America/New_York");
  assert.deepEqual(goal.grid, { step: "15m" });
  assert.deepEqual(goal.parts, [{
    id: "interview",
    tags: ["interview/front-end"],
    duration: "45m",
    requirement: { all: [{ person: "ana" }] },
    travel,
  }]);

  const request = {
    now: "2027-06-14T08:00Z",
    goal,
    participants: [{ id: "ana", zone: "Europe/London", calendar: [] }],
  };
  assert.deepEqual(validate(request).errors, []);

  const exact = singleGoal("30m", { all: [person("ana")] }, {
    zone: "UTC",
    searchWindow: ["2027-06-14T08:00Z", "2027-06-15T08:00Z"],
    candidates: ["2027-06-14T09:00Z"],
  });
  assert.deepEqual(exact.candidates, ["2027-06-14T09:00Z"]);
  assert.deepEqual(validate({ ...request, goal: exact }).errors, []);

  const plain = singleGoal("30m", { all: [person("ana")] }, {
    zone: "UTC",
    searchWindow: ["2027-06-14T08:00Z", "2027-06-15T08:00Z"],
  });
  assert.deepEqual(plain, {
    parts: [{ duration: "30m", requirement: { all: [{ person: "ana" }] } }],
    search_window: ["2027-06-14T08:00Z", "2027-06-15T08:00Z"],
    zone: "UTC",
  }, "omitting the new fields keeps the shape the shortcut always built");

  // Single-string shorthand and invalid-type guards match Python's `single_goal`.
  const shorthand = singleGoal("30m", { all: [person("ana")] }, {
    zone: "UTC",
    searchWindow: ["2027-06-14T08:00Z", "2027-06-15T08:00Z"],
    tags: "interview",
    candidates: "2027-06-14T09:00Z",
  });
  assert.deepEqual(shorthand.parts[0].tags, ["interview"]);
  assert.deepEqual(shorthand.candidates, ["2027-06-14T09:00Z"]);
  assert.equal(
    rejection(() =>
      singleGoal("30m", { all: [person("ana")] }, {
        zone: "UTC",
        searchWindow: ["2027-06-14T08:00Z", "2027-06-15T08:00Z"],
        tags: { interview: true },
      }),
    ).code,
    "invalid_argument",
  );
});

test("an empty limits document is the engine's own defaults, not a limit of zero", () => {
  // Every cap is optional on the wire, so `{}` has to mean the engine defaults
  // rather than a cap of zero: a partial config must not refuse every request.
  // The same request answers identically from an engine built with `{}` and from
  // one built with no limits at all.
  const request = {
    now: "2027-06-14T08:00Z",
    goal: singleGoal("30m", { all: [person("ana")] }, {
      zone: "UTC",
      searchWindow: ["2027-06-14T09:00Z", "2027-06-14T11:00Z"],
    }),
    participants: [
      { id: "ana", zone: "UTC", calendar: [] },
      { id: "bo", zone: "UTC", calendar: [] },
    ],
  };

  using empty = new Engine({});
  using defaults = new Engine();
  assert.equal(empty.findAvailabilityJson(request), defaults.findAvailabilityJson(request));
  assert.ok(JSON.parse(empty.findAvailabilityJson(request)).offers.length > 0);

  // A cap that is stated still applies, with the rest defaulted.
  using crowded = new Engine({ max_participants: 1 });
  assert.throws(() => crowded.findAvailability(request), OpportuneError);
  assert.equal(defaultLimits().max_participants, 500);
});

test("limitsWith ignores explicit undefined overrides and engine construction classifies limits as 500", () => {
  const defaults = defaultLimits();
  const merged = limitsWith({ max_search_window_days: undefined, max_participants: 42 });
  assert.equal(merged.max_search_window_days, defaults.max_search_window_days);
  assert.equal(merged.max_participants, 42);
  using engine = new Engine(merged);
  assert.equal(typeof engine.tzdbVersion, "string");

  const badLimits = rejection(() => new Engine(limitsWith({ max_participants: -1 })));
  assert.ok(badLimits instanceof OpportuneError);
  assert.equal(badLimits.code, "invalid_limits");
  assert.equal(httpStatusForError(badLimits), 500);
  assert.equal(httpStatusForError(new OpportuneError("invalid_request", "bad")), 422);
  assert.equal(httpStatusForError(new OpportuneError("engine_disposed", "gone")), 500);
});

test("presets runtime guards cover unless, availableOnly id, horizon u32 ceiling, and categoryMap null", () => {
  for (const badRule of [{}, new Date(), new Engine(), { id: "x", params: {} }]) {
    const err = rejection(() => unless(badRule, { tag: "vip" }));
    assert.ok(err instanceof OpportuneError);
    assert.equal(err.code, "invalid_argument");
  }

  for (const badId of [null, undefined, 42]) {
    const err = rejection(() => availableOnly(badId, [["2027-06-14T09:00Z", "2027-06-14T10:00Z"]]));
    assert.ok(err instanceof OpportuneError);
    assert.equal(err.code, "invalid_argument");
    assert.match(err.message, /`id`/);
  }
  const badRuleId = rejection(() =>
    availableOnly("ana", [["2027-06-14T09:00Z", "2027-06-14T10:00Z"]], { ruleId: 5 }),
  );
  assert.equal(badRuleId.code, "invalid_argument");
  assert.match(badRuleId.message, /`ruleId`/);

  assert.deepEqual(horizon(0xffff_ffff).params.max, { days: 0xffff_ffff });
  assert.equal(rejection(() => horizon(0x1_0000_0000)).code, "invalid_argument");
  assert.equal(rejection(() => horizon({ days: 0x1_0000_0000 })).code, "invalid_argument");
  assert.equal(
    rejection(() => horizon({ calendar: "org", working_days: 0x1_0000_0000 })).code,
    "invalid_argument",
  );

  // `categoryMap: null` is treated identically to absent.
  const outcome = calendarViewToBlocks({ value: [] }, "UTC", { categoryMap: null });
  assert.equal(outcome.status, "available");
  assert.deepEqual(outcome.blocks, []);
});

test("statusPolicy preset, KindSelector scoping, and status_policy_ignores_resource warning", () => {
  const unscoped = statusPolicy([{ status: "tentative", effect: "ignore" }]);
  assert.equal(unscoped.kind, "status_policy");
  assert.equal(unscoped.id, "status_policy");

  for (const badEntries of [null, "tentative", { status: "tentative" }, [null]]) {
    assert.equal(rejection(() => statusPolicy(badEntries)).code, "invalid_argument");
  }

  const baseRequest = {
    now: "2027-06-14T08:00Z",
    goal: singleGoal("30m", {
      all: [person("ana"), atLeast(1, { kind: "resource" })],
    }, {
      zone: "UTC",
      searchWindow: ["2027-06-14T09:00Z", "2027-06-14T10:00Z"],
    }),
    participants: [
      {
        id: "ana",
        kind: "person",
        zone: "UTC",
        calendar: [
          { span: ["2027-06-14T09:00Z", "2027-06-14T09:30Z"], status: "tentative", tags: [] },
        ],
      },
      {
        id: "room-a",
        kind: "resource",
        zone: "UTC",
        calendar: [
          { span: ["2027-06-14T09:00Z", "2027-06-14T09:30Z"], status: "tentative", tags: [] },
        ],
      },
    ],
    rules: [unscoped],
  };

  // Unscoped policy warns that it ignores tentative holds on resources.
  const diagnostics = validate(baseRequest);
  assert.deepEqual(diagnostics.errors, []);
  assert.ok(
    diagnostics.warnings.some((w) => w.code === "status_policy_ignores_resource" && w.path === "/rules/0"),
    JSON.stringify(diagnostics.warnings),
  );

  // Scoping `who` to `{ kind: "person" }` clears the warning and keeps the room's hold blocking.
  const scoped = statusPolicy([{ status: "tentative", effect: "ignore" }], {
    who: { kind: "person" },
  });
  const safeRequest = { ...baseRequest, rules: [scoped] };
  assert.deepEqual(validate(safeRequest).warnings, []);
  assert.deepEqual(
    starts(findAvailability(safeRequest)),
    ["2027-06-14T09:30Z"],
    "09:00 stays blocked by room-a's tentative hold; 09:30 is open for both",
  );
});

test("exclude_event_keys rescheduling clears old slot, cap, and rest, and validates empty/limit", () => {
  const request = {
    now: "2027-06-14T08:00Z",
    goal: singleGoal("30m", { all: [person("ana")] }, {
      zone: "UTC",
      searchWindow: ["2027-06-14T09:00Z", "2027-06-14T11:00Z"],
      tags: "interview",
    }),
    participants: [
      {
        id: "ana",
        zone: "UTC",
        calendar: [
          block(["2027-06-14T09:00Z", "2027-06-14T09:30Z"], ["interview"], "evt-move"),
        ],
      },
    ],
    rules: [capPerDay({ tag: "interview" }, 1)],
  };

  assert.deepEqual(starts(findAvailability(request)), [], "daily cap of 1 is already used");

  const rescheduled = findAvailability({ ...request, exclude_event_keys: ["evt-move"] });
  assert.ok(
    starts(rescheduled).includes("2027-06-14T09:00Z"),
    "excluding the event being moved frees both its time and its cap slot",
  );
  assert.ok(starts(rescheduled).includes("2027-06-14T10:00Z"));

  const emptyKey = validate({ ...request, exclude_event_keys: ["  "] });
  assert.deepEqual(
    emptyKey.errors.map((e) => [e.code, e.path]),
    [["event_key_empty", "/exclude_event_keys/0"]],
  );

  using narrow = new Engine(limitsWith({ max_occurrences: 1 }));
  const tooMany = narrow.validate({ ...request, exclude_event_keys: ["k1", "k2"] });
  assert.deepEqual(
    tooMany.errors.map((e) => [e.code, e.path]),
    [["exclude_event_keys_limit", "/exclude_event_keys"]],
  );
});

test("all_day blocks do not count toward a cap while still blocking their span", () => {
  const request = {
    now: "2027-06-14T08:00Z",
    goal: singleGoal("30m", { all: [person("ana")] }, {
      zone: "UTC",
      searchWindow: ["2027-06-14T09:00Z", "2027-06-16T10:00Z"],
      candidates: ["2027-06-14T09:00Z", "2027-06-15T09:00Z"],
      tags: "interview",
    }),
    participants: [
      {
        id: "ana",
        zone: "UTC",
        calendar: [
          {
            span: ["2027-06-14T00:00Z", "2027-06-15T00:00Z"],
            status: "busy",
            all_day: true,
            tags: ["interview"],
            event_key: "ooo-monday",
          },
        ],
      },
    ],
    rules: [capPerWeek({ tag: "interview" }, 1)],
  };

  assert.deepEqual(
    starts(findAvailability(request)),
    ["2027-06-15T09:00Z"],
    "Monday is blocked by the all-day event, while Tuesday still has the full weekly cap of 1",
  );
});
