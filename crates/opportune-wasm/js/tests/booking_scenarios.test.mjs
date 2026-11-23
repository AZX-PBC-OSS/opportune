/**
 * The booking-product scenarios a server builds on, through the published package.
 *
 * The domain semantics behind these are pinned in the Rust suite: travel and
 * clearance composing in `crates/opportune/tests/engine_a1.rs`, caps and the tags
 * that make one apply in `engine_cap.rs`, pools, weights and attendees in
 * `engine_a2.rs`. Nothing here re-proves those. What only this file can prove is
 * the surface a Node or Bun server actually touches: the wire spelling of each
 * scenario, the presets and sugar that build it, and the answer's own JSON shape —
 * the reserve spans a caller writes to a provider calendar, the reason that names
 * the provider's event key, the histogram a booking page renders.
 *
 * A domain test cannot catch a preset that emits a rule the wire rejects, a
 * duration spelling that reads as a day, or a reserve span that arrives with a
 * different shape than the caller writes to Google or Graph. These can.
 *
 * Run against the built package, so build it first:
 *
 * ```sh
 * npm --prefix crates/opportune-wasm/js test
 * ```
 */

import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import test from "node:test";

import {
  Engine,
  OpportuneError,
  atLeast,
  buffers,
  businessHours,
  capPerDay,
  horizon,
  minNotice,
  offerAssignedHosts,
  offerEnd,
  offerStart,
  offerToProposal,
  person,
  rest,
  upgradeCalendar,
  upgradeGoal,
  upgradeRule,
} from "../dist/index.js";
import * as google from "../dist/adapters/google.js";
import * as graph from "../dist/adapters/graph.js";

/** The recorded provider payloads beside the adapter crate. */
const FIXTURES = join(import.meta.dirname, "../../../opportune-adapters/tests/fixtures");

/** One calendar block: a span, and whatever the caller knows about it. */
function block(span, extra = {}) {
  return { span, status: "busy", ...extra };
}

/** The starts of every offer, as the wire spells them. */
function starts(response) {
  return response.offers.map(offerStart);
}

/** The offer counts of the dates that have any, as `date=count`. */
function daysWithOffers(response) {
  return response.dates
    .filter((entry) => entry.offers > 0)
    .map((entry) => `${entry.date}=${entry.offers}`);
}

/** The reserve spans of an offer's first event, as `kind participant start..end`. */
function holds(offer) {
  return offer.events[0].reserve.map(
    (reserve) => `${reserve.kind} ${reserve.participant} ${reserve.span[0]}..${reserve.span[1]}`,
  );
}

/**
 * A field engineer's day: a job already on the calendar that needs driving time
 * after it, and a new appointment that needs driving time to and from it.
 *
 * The window is 08:00-18:00 in New York, which in March 2027 is 13:00Z-23:00Z.
 */
function fieldEngineerRequest(overrides = {}) {
  const part = {
    id: "visit",
    duration: "90m",
    tags: ["job/plumbing"],
    requirement: { all: [person("bob")] },
    travel: { before: "25m", after: "25m" },
    ...overrides.part,
  };
  return {
    now: "2027-03-06T12:00Z",
    goal: {
      zone: "America/New_York",
      search_window: ["2027-03-08T00:00Z", "2027-03-11T00:00Z"],
      grid: { step: "15m", zone: "America/New_York" },
      parts: [part],
    },
    participants: [
      {
        id: "bob",
        role: "host",
        zone: "America/New_York",
        calendar: [
          block(["2027-03-08T18:00Z", "2027-03-08T20:00Z"], {
            tags: ["job/plumbing"],
            event_key: "job-101",
            travel_after: "20m",
          }),
          block(["2027-03-09T14:00Z", "2027-03-09T16:00Z"], {
            tags: ["job/plumbing"],
            event_key: "job-102",
            travel_before: "30m",
          }),
        ],
        ...overrides.bob,
      },
      { id: "joe", role: "attendee", zone: "America/New_York", calendar: [] },
    ],
    rules: [businessHours("mon-sat", "08:00-18:00"), buffers("10m", "10m"), minNotice("2h")],
    limit: 6,
    explain: { limit: 30 },
  };
}

test("travel to the new appointment and home from the last one both hold time", () => {
  const response = new Engine().findAvailability(fieldEngineerRequest());

  // The window opens at 13:00Z, and 25 minutes of travel must fit inside it, so
  // the first start on the 15-minute grid is 13:30Z rather than 13:00Z.
  assert.equal(offerStart(response.offers[0]), "2027-03-08T13:30Z");
  assert.equal(offerEnd(response.offers[0]), "2027-03-08T15:00Z");

  // The travel comes back as spans to hold, for the engineer and for the customer:
  // this is what a server writes to each calendar, and nothing else reports it.
  assert.deepEqual(holds(response.offers[0]), [
    "travel bob 2027-03-08T13:05Z..2027-03-08T13:30Z",
    "travel bob 2027-03-08T15:00Z..2027-03-08T15:25Z",
    "travel joe 2027-03-08T13:05Z..2027-03-08T13:30Z",
    "travel joe 2027-03-08T15:00Z..2027-03-08T15:25Z",
  ]);

  // Without travel anywhere, the same day offers nine more starts and begins at
  // the window edge, which is what makes the travel above measurable.
  const noTravel = new Engine().findAvailability(
    fieldEngineerRequest({
      part: { travel: null },
      bob: {
        calendar: [
          block(["2027-03-08T18:00Z", "2027-03-08T20:00Z"], { event_key: "job-101" }),
          block(["2027-03-09T14:00Z", "2027-03-09T16:00Z"], { event_key: "job-102" }),
        ],
      },
    }),
  );
  assert.equal(offerStart(noTravel.offers[0]), "2027-03-08T13:00Z");
  assert.deepEqual(daysWithOffers(noTravel), [
    "2027-03-08=20",
    "2027-03-09=22",
    "2027-03-10=35",
  ]);
  assert.deepEqual(daysWithOffers(response), [
    "2027-03-08=11",
    "2027-03-09=18",
    "2027-03-10=31",
  ]);
});

test("a start too soon after the last job is refused naming that job", () => {
  const engine = new Engine();
  const request = fieldEngineerRequest();
  const response = engine.findAvailability(request);

  // The offer the engine made confirms, carrying the same holds: a server books
  // what it showed, and the check agrees with the search.
  const chosen = offerToProposal(response.offers[0]);
  const confirmed = engine.checkSlot(request, chosen);
  assert.equal(confirmed.valid, true);
  assert.deepEqual(
    confirmed.reserve.map(
      (reserve) => `${reserve.kind} ${reserve.participant} ${reserve.span[0]}..${reserve.span[1]}`,
    ),
    holds(response.offers[0]),
  );

  // The last job ends at 20:00Z and needs 20 minutes of driving after it, so a
  // visit starting 20:15Z is refused — and the reason names the provider's own
  // event key, which is what lets a server show the clash to a person.
  const tooSoon = {
    events: [{ part: "visit", start: "2027-03-08T20:15Z", assignment: [["bob"]] }],
  };
  const refused = engine.checkSlot(request, tooSoon);
  assert.equal(refused.valid, false);
  assert.deepEqual(
    refused.reasons.map((reason) => [reason.code, reason.participant, reason.event_key]),
    [["blocked", "bob", "job-101"]],
  );
});

test("a day split into two ranges offers neither range's edges nor the gap", () => {
  const request = {
    now: "2027-03-06T12:00Z",
    goal: {
      zone: "America/New_York",
      search_window: ["2027-03-08T00:00Z", "2027-03-20T00:00Z"],
      parts: [{ id: "main", duration: "30m", requirement: { all: [person("ana")] } }],
    },
    participants: [{ id: "ana", role: "host", zone: "America/New_York", calendar: [] }],
    rules: [businessHours("mon-fri", ["09:00-12:00", "13:00-17:30"])],
    limit: 40,
  };
  const response = new Engine().findAvailability(request);

  // A solid 09:00-17:00 day offers sixteen 30-minute starts; splitting out an
  // hour of lunch leaves fifteen, and nothing starts inside the gap.
  assert.deepEqual(daysWithOffers(response).slice(0, 3), [
    "2027-03-08=15",
    "2027-03-09=15",
    "2027-03-10=15",
  ]);
  const lunch = starts(response).filter(
    (start) => start >= "2027-03-08T16:45Z" && start <= "2027-03-08T17:30Z",
  );
  assert.deepEqual(lunch, [], "12:00-13:00 in New York is 17:00Z-18:00Z");
});

test("hours that differ per weekday hold on each of them", () => {
  const request = {
    now: "2027-03-06T12:00Z",
    goal: {
      zone: "America/New_York",
      search_window: ["2027-03-08T00:00Z", "2027-03-15T00:00Z"],
      parts: [{ id: "main", duration: "30m", requirement: { all: [person("ana")] } }],
    },
    participants: [{ id: "ana", role: "host", zone: "America/New_York", calendar: [] }],
    rules: [
      {
        id: "hours",
        kind: "window",
        params: {
          zone: "participant",
          weekly: {
            "mon-thu": ["08:30-12:00", "13:00-18:00"],
            fri: ["09:00-15:00"],
            sat: ["10:00-12:00"],
          },
        },
      },
    ],
    limit: 80,
  };
  const response = new Engine().findAvailability(request);

  // Monday 8 March through Thursday 11 March are the long split days, Friday the
  // short one, Saturday the morning only, and Sunday nothing: 2027-03-14 is absent
  // from the list because a date with no offers still appears in `dates` as zero.
  assert.deepEqual(daysWithOffers(response), [
    "2027-03-08=17",
    "2027-03-09=17",
    "2027-03-10=17",
    "2027-03-11=17",
    "2027-03-12=12",
    "2027-03-13=4",
  ]);
  assert.equal(
    response.dates.find((entry) => entry.date === "2027-03-14")?.offers,
    0,
    "Sunday is not in the weekly map at all",
  );
});

test("one host configuration answers a 30, 45 and 60 minute public event type", () => {
  const durations = ["30m", "45m", "60m"];
  const counts = [];
  for (const duration of durations) {
    const response = new Engine().findAvailability({
      now: "2027-03-05T12:00Z",
      goal: {
        zone: "America/New_York",
        search_window: ["2027-03-08T00:00Z", "2027-03-20T00:00Z"],
        grid: { step: "15m", zone: "America/New_York" },
        parts: [{
          id: "main",
          duration,
          requirement: { all: [atLeast(1, { role: "host" })] },
        }],
      },
      participants: [
        { id: "ana", role: "host", zone: "America/New_York", calendar: [] },
        { id: "invitee", role: "attendee", zone: "Europe/Berlin", calendar: [] },
      ],
      rules: [
        businessHours(),
        buffers("10m", "10m"),
        minNotice("4h"),
        horizon({ days: 30 }),
        rest(),
      ],
      limit: 20,
    });
    counts.push(response.dates.find((entry) => entry.date === "2027-03-08").offers);
    // The invitee is an attendee: never assigned, and a supplied empty calendar
    // means known-free rather than unknown, so no offer is flagged.
    assert.deepEqual(response.offers[0].flags, []);
    assert.deepEqual(offerAssignedHosts(response.offers[0]), ["ana"]);
  }

  // A longer booking loses the starts its own length no longer fits: the same
  // configuration, three durations, and the day's offer count falls by one each
  // time. This is what a public page with duration options renders.
  assert.deepEqual(counts, [31, 30, 29]);
});

test("a required room is assigned from the pool and a busy one is passed over", () => {
  const team = [
    { id: "sales-1", role: "host", zone: "America/New_York", calendar: [], tags: ["sales"] },
    { id: "se-1", role: "host", zone: "America/New_York", calendar: [], tags: ["sales-engineer"] },
    { id: "se-2", role: "host", zone: "America/New_York", calendar: [], tags: ["sales-engineer"] },
    {
      id: "room-a",
      role: "host",
      kind: "resource",
      zone: "America/New_York",
      tags: ["room"],
      calendar: [block(["2027-03-08T14:00Z", "2027-03-08T18:00Z"], { event_key: "room-a-0" })],
    },
    { id: "room-b", role: "host", kind: "resource", zone: "America/New_York", tags: ["room"], calendar: [] },
  ];
  const request = {
    now: "2027-03-06T12:00Z",
    goal: {
      zone: "America/New_York",
      search_window: ["2027-03-08T00:00Z", "2027-03-10T00:00Z"],
      parts: [{
        id: "demo",
        duration: "60m",
        tags: ["demo"],
        requirement: {
          all: [
            atLeast(1, { tag: "sales-engineer" }),
            person("sales-1"),
            atLeast(1, { tag: "room" }),
          ],
        },
      }],
    },
    participants: team,
    rules: [businessHours(), buffers("10m", "10m")],
    limit: 4,
  };
  const response = new Engine().findAvailability(request);

  // Three terms, three lists: the engineer pool could take it, the named sales
  // person must, and the free room is chosen because the other one is busy.
  const event = response.offers[0].events[0];
  assert.deepEqual(event.eligible, [["se-1", "se-2"], ["sales-1"], ["room-b"]]);
  assert.deepEqual(event.assignment, [["se-1"], ["sales-1"], ["room-b"]]);
  assert.deepEqual(offerAssignedHosts(response.offers[0]), ["se-1", "sales-1", "room-b"]);

  // Weighting the pool moves the engineer chosen and leaves the room alone, which
  // is how a caller steers a rotation without restating the requirement.
  const weighted = new Engine().findAvailability({
    ...request,
    weights: { "se-1": 2, "se-2": 8 },
  });
  assert.deepEqual(weighted.offers[0].events[0].assignment, [["se-2"], ["sales-1"], ["room-b"]]);
});

test("an optional room is reported eligible and never assigned", () => {
  const team = [
    { id: "sales-1", role: "host", zone: "America/New_York", calendar: [], tags: ["sales"] },
    { id: "se-1", role: "host", zone: "America/New_York", calendar: [], tags: ["sales-engineer"] },
    { id: "room-b", role: "host", kind: "resource", zone: "America/New_York", tags: ["room"], calendar: [] },
  ];
  const makeRequest = (roomTerms, weights = undefined) => ({
    now: "2027-03-06T12:00Z",
    goal: {
      zone: "America/New_York",
      search_window: ["2027-03-08T00:00Z", "2027-03-10T00:00Z"],
      parts: [{
        id: "demo",
        duration: "60m",
        tags: ["demo"],
        requirement: {
          all: [atLeast(1, { tag: "sales-engineer" }), person("sales-1"), ...roomTerms],
        },
      }],
    },
    participants: team,
    rules: [businessHours(), buffers("10m", "10m")],
    weights,
    limit: 40,
  });

  const optional = new Engine().findAvailability(makeRequest([atLeast(0, { tag: "room" })]));
  const absent = new Engine().findAvailability(makeRequest([]));

  // A zero-count term contributes an empty assignment list while still reporting
  // which rooms are eligible at that start, and offers the exact same start times
  // as omitting the room term. Weights do not auto-assign a zero-count term: the
  // caller inspects `eligible[2]` to see which rooms are free and confirms a
  // chosen room against `at_least: 1`.
  assert.deepEqual(optional.offers[0].events[0].eligible, [["se-1"], ["sales-1"], ["room-b"]]);
  assert.deepEqual(optional.offers[0].events[0].assignment, [["se-1"], ["sales-1"], []]);
  assert.deepEqual(starts(optional), starts(absent));

  const weightedOptional = new Engine().findAvailability(
    makeRequest([atLeast(0, { tag: "room" })], { "room-b": 10 }),
  );
  assert.deepEqual(weightedOptional.offers[0].events[0].assignment, [["se-1"], ["sales-1"], []]);
  assert.deepEqual(starts(weightedOptional), starts(absent));
});

test("a room that must only be free is an attendee, and needs no clearance", () => {
  const roomCalendar = [block(["2027-03-08T14:00Z", "2027-03-08T18:00Z"], { event_key: "room-b-0" })];
  const request = (room) => ({
    now: "2027-03-06T12:00Z",
    goal: {
      zone: "America/New_York",
      search_window: ["2027-03-08T00:00Z", "2027-03-10T00:00Z"],
      parts: [{
        id: "demo",
        duration: "60m",
        tags: ["demo"],
        requirement: {
          all: [atLeast(1, { tag: "sales-engineer" }), person("sales-1"), ...room.term],
        },
      }],
    },
    participants: [
      { id: "sales-1", role: "host", zone: "America/New_York", calendar: [], tags: ["sales"] },
      { id: "se-1", role: "host", zone: "America/New_York", calendar: [], tags: ["sales-engineer"] },
      room.participant,
    ],
    rules: [businessHours(), buffers("10m", "10m")],
    limit: 40,
  });

  const asHost = new Engine().findAvailability(
    request({
      term: [atLeast(1, { tag: "room" })],
      participant: { id: "room-b", role: "host", kind: "resource", zone: "America/New_York", tags: ["room"], calendar: roomCalendar },
    }),
  );
  const asAttendee = new Engine().findAvailability(
    request({
      term: [],
      participant: { id: "room-b", role: "attendee", kind: "resource", zone: "America/New_York", tags: ["room"], calendar: roomCalendar },
    }),
  );

  // Both keep the room's busy afternoon out of the offers, and neither assigns a
  // room the caller did not ask to have assigned.
  for (const response of [asHost, asAttendee]) {
    assert.deepEqual(
      starts(response).filter((start) => start >= "2027-03-08T14:00Z" && start < "2027-03-08T17:00Z"),
      [],
      "the room's busy afternoon offers nothing",
    );
  }
  assert.deepEqual(offerAssignedHosts(asAttendee.offers[0]), ["se-1", "sales-1"]);

  // They are not the same query. Clearance is a host rule, so an assigned room
  // also needs ten minutes clear of the block that ends at 18:00Z and its first
  // start is 18:30Z; an attendee only has to be free, so 18:00Z already works.
  // A caller choosing between the two patterns is choosing that difference.
  assert.equal(offerStart(asHost.offers[0]), "2027-03-08T18:30Z");
  assert.equal(offerStart(asAttendee.offers[0]), "2027-03-08T18:00Z");
  assert.equal(
    asAttendee.dates.find((entry) => entry.date === "2027-03-08").offers,
    asHost.dates.find((entry) => entry.date === "2027-03-08").offers + 1,
  );
});

test("anyone from a tagged pool is offered inside conference hours only", () => {
  // Twenty-four delegates, every third one busy on the first afternoon.
  const delegates = Array.from({ length: 24 }, (_, index) => {
    const id = `gtm-${String(index).padStart(2, "0")}`;
    return {
      id,
      role: "host",
      zone: "America/New_York",
      tags: ["gtm"],
      calendar:
        index % 3 === 0
          ? [block(["2027-03-09T14:00Z", "2027-03-09T20:00Z"], { event_key: `${id}-busy` })]
          : [],
    };
  });
  const request = {
    now: "2027-03-06T12:00Z",
    goal: {
      zone: "America/New_York",
      search_window: ["2027-03-09T00:00Z", "2027-03-12T00:00Z"],
      parts: [{
        id: "booth",
        duration: "20m",
        tags: ["conference"],
        requirement: { all: [atLeast(1, { tag: "gtm" })] },
      }],
    },
    participants: delegates,
    rules: [
      {
        id: "conference_hours",
        kind: "window",
        who: { tag: "gtm" },
        params: {
          zone: "America/New_York",
          dates: {
            "2027-03-09": ["09:00-17:00"],
            "2027-03-10": ["09:00-17:00"],
            "2027-03-11": ["09:00-15:00"],
          },
        },
      },
      buffers("5m", "5m"),
    ],
    limit: 200,
  };
  const response = new Engine().findAvailability(request);

  assert.equal(response.offers.length, 66);
  assert.equal(response.truncated, false, "the whole conference is offered, not a page of it");
  assert.deepEqual(daysWithOffers(response), [
    "2027-03-09=24",
    "2027-03-10=24",
    "2027-03-11=18",
  ]);

  // Every slot is one delegate, inside the hours stated for that day, and never
  // one of the delegates busy that afternoon.
  for (const offer of response.offers) {
    const assigned = offerAssignedHosts(offer);
    assert.equal(assigned.length, 1, `one delegate takes ${offerStart(offer)}`);
    assert.ok(assigned[0].startsWith("gtm-"), "the pool is selected by its tag");
    const start = offerStart(offer);
    assert.ok(start >= "2027-03-09T14:00Z", "conference hours, in New York, start at 09:00");
    if (start.startsWith("2027-03-09") && start >= "2027-03-09T14:00Z" && start < "2027-03-09T19:45Z") {
      for (const index of [0, 3, 6, 9, 12, 15, 18, 21]) {
        assert.notEqual(assigned[0], `gtm-${String(index).padStart(2, "0")}`,
          "a delegate busy that afternoon does not take it");
      }
    }
  }
});

test("a cap closes the day for a tagged event type and only for it", () => {
  const engine = new Engine();
  const request = (partTags) => ({
    now: "2027-03-06T12:00Z",
    goal: {
      zone: "America/New_York",
      search_window: ["2027-03-08T00:00Z", "2027-03-13T00:00Z"],
      parts: [{ id: "main", duration: "45m", tags: partTags, requirement: { all: [person("ana")] } }],
    },
    participants: [{
      id: "ana",
      role: "host",
      zone: "America/New_York",
      calendar: [
        block(["2027-03-08T14:00Z", "2027-03-08T14:45Z"], { tags: ["interview"], event_key: "int-1" }),
        block(["2027-03-08T15:00Z", "2027-03-08T15:45Z"], { tags: ["interview"], event_key: "int-2" }),
      ],
    }],
    rules: [businessHours(), capPerDay({ tag: "interview" }, 2, { per: "each" })],
    limit: 60,
    explain: { limit: 500 },
  });

  // Two interviews are already booked and the type allows two a day, so the whole
  // day goes: no offer on 8 March, and every excluded start there says why, with
  // the usage and the bucket a support page would show.
  const tagged = engine.findAvailability(request(["interview"]));
  assert.deepEqual(daysWithOffers(tagged), [
    "2027-03-09=15",
    "2027-03-10=15",
    "2027-03-11=15",
    "2027-03-12=15",
  ]);
  const capped = tagged.excluded.flatMap((entry) => entry.reasons)
    .filter((reason) => reason.code === "cap_reached");
  assert.ok(capped.length > 0, "the day is closed by the cap, not by the calendar");
  assert.deepEqual(capped[0].params, { bucket: "2027-03-08", limit: 2, used: 2 });
  assert.equal(capped[0].rule, "cap_per_day");

  // The same request with the part untagged is not limited at all, because a cap
  // applies only to a booking whose part tags match what it counts. A server that
  // forgets to tag the part gets a cap that silently never fires, so the
  // difference is pinned here rather than left to the domain suite.
  const untagged = engine.findAvailability(request([]));
  assert.equal(untagged.dates.find((entry) => entry.date === "2027-03-08").offers, 11);
  assert.deepEqual(
    untagged.excluded.flatMap((entry) => entry.reasons).filter((reason) => reason.code === "cap_reached"),
    [],
  );
});

test("minimum notice moves the first offer, and a day is not a duration", () => {
  const request = (notice) => ({
    now: "2027-03-08T14:00Z",
    goal: {
      zone: "America/New_York",
      search_window: ["2027-03-08T00:00Z", "2027-03-10T00:00Z"],
      parts: [{ id: "main", duration: "30m", requirement: { all: [person("ana")] } }],
    },
    participants: [{ id: "ana", role: "host", zone: "America/New_York", calendar: [] }],
    rules: [businessHours(), minNotice(notice)],
    limit: 2,
  });
  const engine = new Engine();

  // Notice is what stops a booking landing fifteen minutes from now: the first
  // offer is exactly `now` plus the notice, aligned to the grid, and the day's
  // count falls as the notice grows.
  const expected = [
    ["0m", "2027-03-08T14:00Z", 16],
    ["2h", "2027-03-08T16:00Z", 12],
    ["4h", "2027-03-08T18:00Z", 8],
    ["24h", "2027-03-09T14:00Z", 0],
  ];
  for (const [notice, first, dayOne] of expected) {
    const response = engine.findAvailability(request(notice));
    assert.equal(offerStart(response.offers[0]), first, `notice ${notice}`);
    assert.equal(
      response.dates.find((entry) => entry.date === "2027-03-08").offers,
      dayOne,
      `notice ${notice} leaves ${dayOne} starts on the day it is asked`,
    );
  }

  // A day of notice is the spelling a caller reaches for and the one the wire
  // refuses, because a day is not an exact duration across a transition. The
  // rejection names the field, so the caller writes `24h` or a horizon in days.
  let thrown;
  try {
    engine.findAvailability(request("1d"));
  } catch (error) {
    thrown = error;
  }
  assert.ok(thrown instanceof OpportuneError, "a day-spelled notice is a rejection");
  assert.equal(thrown.code, "duration_contains_days");
  assert.equal(thrown.path, "/rules/1/params/min");
  assert.match(thrown.message, /a day is not an exact duration/);
});

test("field-service travel composes exact minutes with clearance and exempts the customer at home", () => {
  // Bob finishes job-101 at 20:00Z with 20m travel_after (occupied until 20:20Z).
  // A 10m clearance buffer plus 25m drive to Joe's house needs 55m after 20:00Z,
  // so 20:54Z fails clearance and 20:55Z is the first exact minute that passes.
  // Joe is at home, so `by_participant` sets Joe's travel to 0m and only Bob
  // receives travel holds to write onto his calendar.
  const engine = new Engine();
  const request = {
    now: "2027-03-06T12:00Z",
    goal: {
      zone: "America/New_York",
      search_window: ["2027-03-08T18:00Z", "2027-03-08T23:00Z"],
      candidates: ["2027-03-08T20:54Z", "2027-03-08T20:55Z"],
      parts: [{
        id: "visit",
        duration: "60m",
        tags: ["job/plumbing"],
        requirement: { all: [person("bob")] },
        travel: {
          before: "25m",
          after: "25m",
          by_participant: { joe: { before: "0m", after: "0m" } },
        },
      }],
    },
    participants: [
      {
        id: "bob",
        role: "host",
        zone: "America/New_York",
        calendar: [
          block(["2027-03-08T18:00Z", "2027-03-08T20:00Z"], {
            tags: ["job/plumbing"],
            event_key: "job-101",
            travel_after: "20m",
          }),
        ],
      },
      { id: "joe", role: "attendee", zone: "America/New_York", calendar: [] },
    ],
    rules: [businessHours("mon-sat", "08:00-18:00"), buffers("10m", "10m")],
    explain: { limit: 10 },
  };

  const response = engine.findAvailability(request);
  assert.deepEqual(starts(response), ["2027-03-08T20:55Z"]);
  assert.deepEqual(holds(response.offers[0]), [
    "travel bob 2027-03-08T20:30Z..2027-03-08T20:55Z",
    "travel bob 2027-03-08T21:55Z..2027-03-08T22:20Z",
  ]);
  assert.equal(response.excluded.length, 1);
  assert.equal(response.excluded[0].start, "2027-03-08T20:54Z");
  assert.deepEqual(
    response.excluded[0].reasons.map((r) => [r.code, r.participant, r.rule]),
    [["clearance", "bob", "buffers"]],
  );
});

test("single-query optional resource tiering distinguishes slots with and without a free room", () => {
  // With `at_least: 0` on the room term, one `findAvailability` call returns every
  // slot where the required team is free AND reports in `eligible[2]` which rooms
  // are free at each slot — letting a picker badge "with room" vs "required only".
  // When confirming a booking that includes a chosen room, `checkSlot` validates
  // against the required-room (`at_least: 1`) shape; passing a room into an
  // `at_least: 0` term is rejected loudly with `proposal_host_count`.
  const engine = new Engine();
  const team = [
    { id: "sales-1", role: "host", zone: "America/New_York", calendar: [], tags: ["sales"] },
    { id: "se-1", role: "host", zone: "America/New_York", calendar: [], tags: ["sales-engineer"] },
    {
      id: "room-b",
      role: "host",
      kind: "resource",
      zone: "America/New_York",
      tags: ["room"],
      calendar: [block(["2027-03-08T14:00Z", "2027-03-08T18:00Z"], { event_key: "room-b-busy" })],
    },
  ];
  const makeRequest = (roomCount) => ({
    now: "2027-03-06T12:00Z",
    goal: {
      zone: "America/New_York",
      search_window: ["2027-03-08T14:00Z", "2027-03-08T20:00Z"],
      parts: [{
        id: "demo",
        duration: "60m",
        tags: ["demo"],
        requirement: {
          all: [
            atLeast(1, { tag: "sales-engineer" }),
            person("sales-1"),
            atLeast(roomCount, { tag: "room" }),
          ],
        },
      }],
    },
    participants: team,
    rules: [businessHours(), buffers("10m", "10m")],
  });

  const optionalRequest = makeRequest(0);
  const response = engine.findAvailability(optionalRequest);

  const withoutRoom = response.offers.filter((o) => o.events[0].eligible[2].length === 0);
  const withRoom = response.offers.filter((o) => o.events[0].eligible[2].length > 0);
  assert.ok(withoutRoom.length > 0, "slots during the room's busy afternoon are still offered");
  assert.ok(withRoom.length > 0, "slots after the room clears report room-b as eligible");
  assert.deepEqual(withRoom[0].events[0].eligible[2], ["room-b"]);
  assert.equal(offerStart(withRoom[0]), "2027-03-08T18:30Z");

  const withRoomProposal = {
    events: [{ part: "demo", start: offerStart(withRoom[0]), assignment: [["se-1"], ["sales-1"], ["room-b"]] }],
  };
  assert.throws(
    () => engine.checkSlot(optionalRequest, withRoomProposal),
    (error) => error instanceof OpportuneError && error.code === "proposal_host_count" && error.path === "/events/0/assignment/2",
  );
  const confirmedWithRoom = engine.checkSlot(makeRequest(1), withRoomProposal);
  assert.equal(confirmedWithRoom.valid, true);
});

test("a proposal whose travel runs past the supported range is rejected with its pointer", () => {
  // `2100-01-01T00:00Z` is the last instant the model names, so a booking may end
  // there but a hold that reaches past it cannot be written to a calendar.
  const engine = new Engine();
  const request = {
    now: "2099-12-31T22:00Z",
    goal: {
      zone: "UTC",
      search_window: ["2099-12-31T22:00Z", "2100-01-01T00:00Z"],
      parts: [{
        duration: "30m",
        requirement: { all: [{ person: "ana" }] },
        travel: { before: "0m", after: "1h" },
      }],
    },
    participants: [{ id: "ana", zone: "UTC", calendar: [] }],
  };

  assert.throws(
    () => engine.checkSlot(request, {
      events: [{ part: "main", start: "2099-12-31T23:30Z", assignment: [["ana"]] }],
    }),
    (error) =>
      error instanceof OpportuneError &&
      error.code === "proposal_out_of_range" &&
      error.path === "/events/0/start" &&
      error.message.includes("ana"),
  );

  // The same start is withheld from the search rather than offered as a booking
  // the caller could not persist, and no rule the caller wrote explains it.
  const response = engine.findAvailability({
    ...request,
    goal: { ...request.goal, candidates: ["2099-12-31T23:30Z"] },
    explain: { limit: 10 },
  });
  assert.deepEqual(starts(response), []);
  assert.deepEqual(response.excluded, []);
});

test("provider event IDs and event keys survive adapter conversion into conflict reasons", () => {
  const engine = new Engine();
  const googlePayload = readFileSync(join(FIXTURES, "google/events_list_ana.json"), "utf8");
  const graphPayload = readFileSync(join(FIXTURES, "graph/calendar_view_bo.json"), "utf8");

  const anaOutcome = google.eventsListToBlocks(googlePayload, "America/New_York");
  const boOutcome = graph.calendarViewToBlocks(graphPayload, "America/New_York", {
    categoryMap: { Interview: "interview" },
  });
  assert.equal(anaOutcome.status, "available");
  assert.equal(boOutcome.status, "available");

  // Every converted entry retains its provider event_id alongside the wire block's
  // event_key so the host application can correlate conflicts back to Graph/Google.
  assert.ok(anaOutcome.blocks.every((entry) => typeof entry.event_id === "string" && entry.event_id.length > 0));
  assert.ok(boOutcome.blocks.every((entry) => typeof entry.event_id === "string" && entry.event_id.length > 0));
  const sharedKey = boOutcome.blocks[0].block.event_key;
  assert.ok(typeof sharedKey === "string" && sharedKey.length > 0);

  const request = {
    now: "2027-03-08T12:00Z",
    goal: {
      zone: "America/New_York",
      search_window: ["2027-03-09T14:00Z", "2027-03-09T18:00Z"],
      parts: [{ id: "panel", duration: "60m", tags: ["interview"], requirement: { all: [person("ana"), person("bo")] } }],
    },
    participants: [
      { id: "ana", role: "host", zone: "America/New_York", calendar: anaOutcome.blocks.map((b) => b.block) },
      { id: "bo", role: "host", zone: "America/New_York", calendar: boOutcome.blocks.map((b) => b.block) },
    ],
    rules: [businessHours()],
  };

  // Checking the exact hour the shared interview occupies returns both hosts'
  // conflicts carrying the preserved event_key and category tags.
  const clash = engine.checkSlot(request, {
    events: [{ part: "panel", start: "2027-03-09T15:00Z", assignment: [["ana"], ["bo"]] }],
  });
  assert.equal(clash.valid, false);
  const boReason = clash.reasons.find((r) => r.participant === "bo");
  assert.ok(boReason !== undefined);
  assert.equal(boReason.code, "blocked");
  assert.equal(boReason.event_key, sharedKey);
  assert.ok(boReason.tags.includes("interview"));
});

test("last-second checkSlot catches a fresh calendar clash and validates off-grid overrides", () => {
  const engine = new Engine();
  const baseRequest = {
    now: "2027-03-08T12:00Z",
    goal: {
      zone: "America/New_York",
      search_window: ["2027-03-08T14:00Z", "2027-03-08T18:00Z"],
      grid: { step: "30m", zone: "America/New_York" },
      parts: [{ id: "consult", duration: "30m", requirement: { all: [person("ana")] } }],
    },
    participants: [{ id: "ana", role: "host", zone: "America/New_York", calendar: [] }],
    rules: [businessHours(), buffers("10m", "10m"), minNotice("2h")],
  };

  // 1. Slot picker lists offers; user picks the first slot.
  const listed = engine.findAvailability(baseRequest);
  const chosen = offerToProposal(listed.offers[0]);
  assert.equal(chosen.events[0].part, "consult");
  assert.equal(engine.checkSlot(baseRequest, chosen).valid, true);

  // 2. Last-second race: another booking lands on Ana's calendar before write.
  const afterRace = {
    ...baseRequest,
    participants: [{
      ...baseRequest.participants[0],
      calendar: [block([offerStart(listed.offers[0]), offerEnd(listed.offers[0])], {
        event_key: "booking-race-42",
        tags: ["consult"],
      })],
    }],
  };
  const raceCheck = engine.checkSlot(afterRace, chosen);
  assert.equal(raceCheck.valid, false);
  assert.deepEqual(
    raceCheck.reasons.map((r) => [r.code, r.participant, r.event_key, r.tags]),
    [["blocked", "ana", "booking-race-42", ["consult"]]],
  );

  // 3. Internal scheduling override: checkSlot validates an off-grid start
  // (14:05Z) and a start outside the search window (next day) against all rules.
  const offGrid = { events: [{ part: "consult", start: "2027-03-08T14:05Z", assignment: [["ana"]] }] };
  assert.equal(engine.checkSlot(baseRequest, offGrid).valid, true);
  const outsideWindow = { events: [{ part: "consult", start: "2027-03-09T14:05Z", assignment: [["ana"]] }] };
  assert.equal(engine.checkSlot(baseRequest, outsideWindow).valid, true);
});

test("stored rules, goals and calendars round-trip through upgrade entry points for document stores", () => {
  const engine = new Engine();
  const originalRules = [
    businessHours("mon-fri", ["09:00-12:00", "13:00-17:00"]),
    buffers("10m", "15m"),
    minNotice("2h"),
    capPerDay({ tag: "interview" }, 2, { per: "each" }),
  ];
  const originalGoal = {
    zone: "America/New_York",
    search_window: ["2027-03-08T00:00Z", "2027-03-10T00:00Z"],
    parts: [{
      id: "interview",
      duration: "45m",
      tags: ["interview"],
      requirement: { all: [person("ana")] },
    }],
  };
  const originalCalendar = {
    zone: "America/New_York",
    working_days: "mon-fri",
    holidays: ["2027-03-09"],
  };

  // Persist as JSON (e.g. Postgres JSONB or CosmosDB), upgrade on read, and
  // confirm the engine produces byte-identical availability output.
  const reloadedRules = originalRules.map((rule) => JSON.parse(upgradeRule(JSON.stringify(rule), 1)));
  const reloadedGoal = JSON.parse(upgradeGoal(JSON.stringify(originalGoal), 1));
  const reloadedCalendar = JSON.parse(upgradeCalendar(JSON.stringify(originalCalendar), 1));

  const before = {
    now: "2027-03-06T12:00Z",
    goal: originalGoal,
    participants: [{ id: "ana", role: "host", zone: "America/New_York", calendar: [] }],
    rules: originalRules,
    calendars: { "us-ny": originalCalendar },
  };
  const after = {
    now: "2027-03-06T12:00Z",
    goal: reloadedGoal,
    participants: [{ id: "ana", role: "host", zone: "America/New_York", calendar: [] }],
    rules: reloadedRules,
    calendars: { "us-ny": reloadedCalendar },
  };
  assert.equal(engine.findAvailabilityJson(before), engine.findAvailabilityJson(after));
});
