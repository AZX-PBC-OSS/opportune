/**
 * Recorded provider payloads through the adapters into a real search, in Node and Bun.
 *
 * Six of the ten recorded payloads are the ones the Rust adapter suite drives end to
 * end (`crates/opportune-adapters/tests/graph_engine_test.rs` and
 * `google_engine_test.rs`) and the Python binding replays against the same engine
 * (`crates/opportune-py/tests/test_adapter_e2e.py`): the same payloads convert, the
 * same request goes to the engine, and the same offers, exclusions and `checkSlot`
 * answer come back. A conversion that drops, widens or mistags a block shows up as
 * wrong availability rather than as a green adapter unit test.
 *
 * The other four are conversion fixtures, and their expectations here are the Rust
 * adapter unit tests' (`graph_adapter_test.rs`, `google_adapter_test.rs`): the
 * `getSchedule` batch, which Python's end-to-end suite also drives but the Rust
 * engine test does not, and the DST and the two tenant payloads, which no Python test
 * reads at all. Those three are driven through a built package only here, so a
 * divergence in them has no cross-language twin to compare against.
 *
 * What only a search can show, and what this suite therefore adds: the shared event
 * key a pooled cap dedupes on, the `status/focus` policy ignoring a block that would
 * otherwise be booked, an errored mailbox that must never read as a free one, and the
 * working-hours conversion failing alone when the zone cannot be resolved.
 *
 * The all-day DST days that are 23 and 25 hours long, and every tenant status,
 * category and event type, are the unit tests' own expectations re-driven through
 * the module the package loads.
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
  MARKER_EVENT_KEY,
  MARKER_TAGS,
  businessHours,
  capPerDay,
  checkSlot,
  offerCalendarUnknown,
  offerStart,
  offerToProposal,
  person,
  rest,
} from "../dist/index.js";
import * as graph from "../dist/adapters/graph.js";
import * as google from "../dist/adapters/google.js";

/** The recorded payloads, beside the crate that consumes them. */
const FIXTURES = join(import.meta.dirname, "../../../opportune-adapters/tests/fixtures");

/** One recorded provider payload, as its JSON text. */
function payload(name) {
  return readFileSync(join(FIXTURES, name), "utf8");
}

/** The category map the Rust end-to-end scenario passes to the Graph adapter. */
const CATEGORIES = { Interview: "interview" };

/** The blocks of an outcome that converted, failing when it did not. */
function blocks(outcome) {
  assert.equal(outcome.status, "available", JSON.stringify(outcome));
  assert.deepEqual(outcome.diagnostics, []);
  return outcome.blocks;
}

/** One calendar's converted blocks, without the provider event IDs. */
function plain(entries) {
  return entries.map((entry) => entry.block);
}

/** The converted entries for the event `eventId`. */
function blockFor(entries, eventId) {
  const entry = entries.find((candidate) => candidate.event_id === eventId);
  assert.ok(entry !== undefined, `no block for \`${eventId}\``);
  return entry.block;
}

/** The offer starts, in the order the engine returned them. */
function starts(response) {
  return response.offers.map(offerStart);
}

/** The starts inside `[first, until)`. */
function startsBetween(response, first, until) {
  return starts(response).filter((start) => start >= first && start < until);
}

/** The reasons the response gives for the exclusions inside `[first, until)`. */
function reasonsBetween(response, first, until) {
  return response.excluded
    .filter((excluded) => excluded.start >= first && excluded.start < until)
    .flatMap((excluded) =>
      excluded.reasons.map((reason) => [reason.participant, reason.code, reason.rule]),
    );
}

/** Whether `checkSlot` confirms the offer the search found at `start`. */
function confirmed(engine, request, start) {
  const booked = engine.findAvailability(request).offers.find((offer) => offerStart(offer) === start);
  assert.ok(booked !== undefined, `the search offered nothing at ${start}`);
  return checkSlot(request, offerToProposal(booked)).valid;
}

/** Both recorded Graph mailboxes, converted. */
function mailboxes() {
  return {
    ana: blocks(graph.calendarViewToBlocks(payload("graph/calendar_view_ana.json"), "America/New_York", { categoryMap: CATEGORIES })),
    bo: blocks(graph.calendarViewToBlocks(payload("graph/calendar_view_bo.json"), "Europe/London", { categoryMap: CATEGORIES })),
  };
}

/** The mailbox's working hours as the rule the Graph conversion produces. */
function graphWorkingHours() {
  const outcome = graph.workingHoursToRule(payload("graph/working_hours.json"), {
    who: { ids: ["ana"] },
    when: { tag: "interview" },
  });
  assert.equal(outcome.status, "available", JSON.stringify(outcome));
  assert.deepEqual(
    outcome.rule.who,
    { ids: ["ana"] },
    "the adapter stores the `who` the caller scoped it to",
  );
  assert.deepEqual(
    outcome.rule.when,
    { tag: "interview" },
    "the adapter stores the `when` the caller scoped it to",
  );
  return outcome.rule;
}

/** The interview the two recorded mailboxes are booked against. */
function graphRequest(workingHours, ana, bo) {
  return {
    now: "2027-03-08T00:00Z",
    goal: {
      zone: "UTC",
      search_window: ["2027-03-08T00:00Z", "2027-03-13T00:00Z"],
      parts: [
        { duration: "1h", tags: ["interview"], requirement: { all: [person("ana"), person("bo")] } },
      ],
    },
    participants: [
      { id: "ana", zone: "America/New_York", calendar: ana },
      { id: "bo", zone: "Europe/London", calendar: bo },
    ],
    rules: [
      workingHours,
      rest(),
      capPerDay({ tag: "interview" }, 2, { per: "pooled", zone: "event", ruleId: "interviews_per_day" }),
    ],
    explain: { limit: 1000 },
  };
}

/** All three recorded Google calendars, converted. */
function calendars() {
  const ana = blocks(google.eventsListToBlocks(payload("google/events_list_ana.json"), "America/New_York"));
  const bo = blocks(google.eventsListToBlocks(payload("google/events_list_bo.json"), "Asia/Tokyo"));
  const freeBusy = google.freeBusyToBlocks(payload("google/free_busy.json"));
  const room = blocks(freeBusy["room-4@example.com"]);
  return { ana, bo, room: plain(room) };
}

/** The interview the three recorded calendars are booked against. */
function googleRequest(ana, bo, room) {
  return {
    now: "2027-03-08T00:00Z",
    goal: {
      zone: "UTC",
      search_window: ["2027-03-08T00:00Z", "2027-03-13T00:00Z"],
      parts: [
        {
          duration: "1h",
          tags: ["interview"],
          requirement: { all: [person("ana"), person("bo"), person("room-4")] },
        },
      ],
    },
    participants: [
      { id: "ana", zone: "America/New_York", calendar: ana },
      { id: "bo", zone: "Asia/Tokyo", calendar: bo },
      { id: "room-4", zone: "UTC", calendar: room },
    ],
    rules: [
      // Google has no working-hours API, so Ana's working day is a caller window.
      { ...businessHours("mon-fri", "09:00-17:00"), who: { ids: ["ana"] } },
      // Focus time blocks by default; this caller's policy does not treat it as
      // occupied time.
      {
        id: "focus_policy",
        kind: "status_policy",
        params: { entries: [{ match: { tag: "status/focus" }, effect: "ignore" }] },
      },
      capPerDay({ tag: "interview" }, 2, { per: "pooled", zone: "event", ruleId: "interviews_per_day" }),
    ],
    explain: { limit: 1000 },
  };
}

test("the two recorded mailboxes share one event key per shared event", () => {
  const { ana, bo } = mailboxes();

  const shared = blockFor(ana, "AAMkAGI2THAAA-ana-interview-0900");
  assert.equal(shared.event_key, "040000008200E00074C5B7101A82E00800000000INTERVIEW0900");
  assert.equal(
    blockFor(bo, "AAMkAGI2THBBB-interview-0900").event_key,
    shared.event_key,
    "the same event in two mailboxes shares one key, which is what a pooled cap dedupes on",
  );

  const dayOff = blockFor(ana, "AAMkAGI2THAAA-ana-pto-0311");
  assert.equal(dayOff.all_day, true);
  assert.deepEqual(dayOff.span, ["2027-03-11T05:00Z", "2027-03-12T05:00Z"],
    "an all-day event resolves from its civil date in the participant's zone");
});

test("recorded mailboxes become the offers the rules describe", () => {
  const { ana, bo } = mailboxes();
  const engine = new Engine();
  const request = graphRequest(graphWorkingHours(), plain(ana), plain(bo));

  assert.deepEqual(engine.validate(request).errors, []);
  const response = engine.findAvailability(request);

  assert.ok(response.offers.length > 0, "the recorded mailboxes leave bookable time");
  for (const offer of response.offers) {
    assert.ok(offerStart(offer) >= "2027-03-08T14:00Z", `the offer at ${offerStart(offer)} is early`);
    assert.ok(
      offer.events.at(-1).end <= "2027-03-12T22:00Z",
      `the offer at ${offerStart(offer)} runs past Ana's working hours`,
    );
  }

  assert.ok(starts(response).includes("2027-03-09T14:00Z"),
    "the shared interview counts once for the pooled cap, leaving room on 9 March");
  assert.deepEqual(startsBetween(response, "2027-03-10T00:00Z", "2027-03-10T22:00Z"), [],
    "two interviews plus a booking pass the cap of two on 10 March");
  assert.ok(
    reasonsBetween(response, "2027-03-10T00:00Z", "2027-03-11T00:00Z").some(
      ([participant, code, rule]) => participant === "ana" && code === "cap_reached" && rule === "interviews_per_day",
    ),
    "the exclusions on 10 March name the cap",
  );
  assert.deepEqual(startsBetween(response, "2027-03-11T00:00Z", "2027-03-12T05:00Z"), [],
    "the all-day block occupies 11 March, so no offer starts inside it");

  assert.equal(confirmed(engine, request, "2027-03-09T14:00Z"), true,
    "the offer the search produced is one `checkSlot` confirms");
});

test("a timed block triggers the rest rule that an all-day block does not", () => {
  const { ana, bo } = mailboxes();
  const workingHours = graphWorkingHours();
  const engine = new Engine();

  const offered = (blocks) =>
    starts(engine.findAvailability(graphRequest(workingHours, blocks, plain(bo)))).includes(
      "2027-03-12T14:00Z",
    );

  assert.equal(offered(plain(ana)), true,
    "an all-day block is not rest activity, so 09:00 on 12 March is offered");
  const timed = plain(ana).map((entry) => (entry.all_day ? { ...entry, all_day: false } : entry));
  assert.equal(offered(timed), false,
    "the same block reported as timed is rest activity, so the start it protects is not offered");
});

test("the recorded DST days convert 23 and 25 hours long", () => {
  // Both all-day transition days and a timed event inside each transition hour,
  // stamped in the Windows zone a tenant reports.
  const entries = blocks(
    graph.calendarViewToBlocks(payload("graph/all_day_dst.json"), "America/New_York"),
  );
  assert.equal(entries.length, 4);

  const spring = blockFor(entries, "evt-spring-forward");
  assert.equal(spring.all_day, true);
  assert.deepEqual(spring.span, ["2027-03-14T05:00Z", "2027-03-15T04:00Z"]);
  assert.equal(minutes(spring.span), 23 * 60, "the spring-forward day is 23 hours long");

  const fall = blockFor(entries, "evt-fall-back");
  assert.equal(fall.all_day, true);
  assert.deepEqual(fall.span, ["2027-11-07T04:00Z", "2027-11-08T05:00Z"]);
  assert.equal(minutes(fall.span), 25 * 60, "the fall-back day is 25 hours long");

  // A local time in the repeated hour takes the earlier instant, and one in the
  // skipped hour moves past the gap.
  assert.deepEqual(blockFor(entries, "evt-repeated-hour").span, [
    "2027-11-07T05:30Z",
    "2027-11-07T05:45Z",
  ]);
  assert.deepEqual(blockFor(entries, "evt-skipped-hour").span, [
    "2027-03-14T07:30Z",
    "2027-03-14T07:45Z",
  ]);
});

/** The whole minutes between the two instants of `span`. */
function minutes(span) {
  return (Date.parse(span[1]) - Date.parse(span[0])) / 60_000;
}

test("the recorded tenant payload converts every Graph status and category", () => {
  // The remaining committed fixture: one tenant's statuses, Windows zone spelling,
  // fractional seconds and category-expanded tags, which the unit suite proves in
  // Rust and this one proves through the module the package loads.
  const outcome = graph.calendarViewToBlocks(payload("graph/calendar_view_tenant.json"), "America/New_York");
  assert.equal(outcome.status, "available", JSON.stringify(outcome));
  const entries = outcome.blocks;
  assert.equal(entries.length, 9, "three of the twelve events do not occupy time");
  assert.ok(entries.every((entry) => entry.event_id !== null));
  assert.ok(entries.every((entry) => entry.block.all_day === false));

  const blockOf = (eventId) => blockFor(entries, eventId);
  assert.equal(blockOf("evt-free").status, "free");
  assert.equal(blockOf("evt-tentative").status, "tentative");
  assert.deepEqual(blockOf("evt-oof").tags, ["status/oof"]);
  assert.equal(blockOf("evt-elsewhere").status, "free");
  assert.deepEqual(blockOf("evt-elsewhere").tags, ["status/working-elsewhere"]);
  assert.deepEqual(blockOf("evt-unknown").tags, ["status/unknown"]);
  assert.deepEqual(blockOf("evt-draft").tags, ["status/draft"]);
  // 09:00 Pacific Standard Time is 17:00Z, and a fractional end rounds outward.
  assert.deepEqual(blockOf("evt-windows-zone").span, ["2027-03-09T17:00Z", "2027-03-09T18:00Z"]);
  assert.deepEqual(blockOf("evt-fractional").span, ["2027-03-10T10:30Z", "2027-03-10T11:01Z"]);
  assert.deepEqual(blockOf("evt-categories").tags, [
    "category/interview",
    "category/1-1-sync",
    "category/client-demo-q3",
    "team/sales",
    "interview/platform",
  ]);
  assert.equal(blockOf("evt-categories").event_key, "booking-42");

  assert.deepEqual(
    outcome.diagnostics.map((finding) => finding.code),
    ["zero_length_event", "marker_tag_invalid", "category_tag_unmappable"],
    "a dropped event says so, so does a marker tag that is not a tag path, and so does a " +
      "category whose name has no ASCII slug",
  );
});

test("the recorded tenant payload converts every Google status and event type", () => {
  const outcome = google.eventsListToBlocks(payload("google/events_list_tenant.json"), "America/New_York");
  assert.equal(outcome.status, "available", JSON.stringify(outcome));
  const entries = outcome.blocks;
  assert.equal(entries.length, 12, "three of the fifteen events do not occupy time");
  assert.ok(entries.every((entry) => entry.event_id !== null));

  const blockOf = (eventId) => blockFor(entries, eventId);
  assert.equal(blockOf("evt-focus").status, "busy");
  assert.deepEqual(blockOf("evt-focus").tags, ["status/focus"]);
  assert.equal(blockOf("evt-working-location").status, "free");
  assert.deepEqual(blockOf("evt-working-location").tags, ["status/working-location"]);
  assert.equal(blockOf("evt-working-location").all_day, true);
  assert.deepEqual(blockOf("evt-ooo").tags, ["status/oof"]);
  assert.deepEqual(blockOf("evt-birthday").tags, ["status/birthday"]);
  assert.deepEqual(blockOf("evt-from-gmail").tags, ["status/from-gmail"]);
  assert.equal(blockOf("evt-default").status, "busy");
  assert.deepEqual(blockOf("evt-default").tags, []);
  assert.equal(blockOf("evt-needs-action").status, "tentative");
  assert.deepEqual(blockOf("evt-needs-action").tags, ["status/needs-action"]);
  assert.equal(blockOf("evt-tentative").status, "tentative");
  assert.deepEqual(blockOf("evt-unspecified-end").span, ["2027-03-11T14:00Z", "2027-03-11T15:00Z"]);
  assert.deepEqual(blockOf("evt-all-day").span, ["2027-03-11T05:00Z", "2027-03-12T05:00Z"]);
  assert.deepEqual(blockOf("evt-all-day").tags, ["time-off"]);
  assert.equal(blockOf("evt-recurring").event_key, "series-platform@example.com@2027-03-09T15:00:00Z");
  assert.equal(blockOf("evt-markers").event_key, "booking-42");
  assert.deepEqual(blockOf("evt-markers").tags, ["team/sales", "interview/platform", "private/note"]);

  assert.deepEqual(
    outcome.diagnostics.map((finding) => finding.code),
    ["zero_length_event", "end_time_unspecified", "marker_tag_invalid"],
  );
});

test("an errored mailbox is an unknown calendar rather than a free one", () => {
  // The whole path is the caller's: the adapter answers `unavailable` for the
  // mailbox, the caller passes that host with no calendar, and every offer resting
  // on it is flagged so an application can chase the calendar before booking.
  const outcomes = graph.getScheduleToBlocks(payload("graph/get_schedule.json"));

  const room = outcomes["room-vienna@example.com"];
  assert.equal(room.status, "unavailable", JSON.stringify(room));
  assert.match(room.message, /not resolvable/, "the provider's own cause is kept");

  // The mailbox's zero-length reminder is dropped with a finding; its blocks are
  // still the ones the search reads.
  const ana = outcomes["ana@example.com"];
  assert.equal(ana.status, "available");
  assert.deepEqual(
    ana.diagnostics.map((finding) => finding.code),
    ["zero_length_event"],
  );

  const request = {
    now: "2027-03-09T08:00Z",
    goal: {
      zone: "UTC",
      search_window: ["2027-03-09T14:00Z", "2027-03-09T17:00Z"],
      parts: [
        {
          duration: "30m",
          requirement: { all: [person("ana@example.com"), person("room-vienna@example.com")] },
        },
      ],
    },
    participants: [
      { id: "ana@example.com", zone: "UTC", calendar: plain(ana.blocks) },
      { id: "room-vienna@example.com", zone: "UTC", calendar: null },
    ],
  };

  const response = new Engine().findAvailability(request);

  assert.ok(response.offers.length > 0, "the healthy mailbox leaves bookable time");
  assert.ok(response.offers.every(offerCalendarUnknown),
    "an offer that rests on the unknown calendar says so");
  assert.deepEqual(startsBetween(response, "2027-03-09T15:00Z", "2027-03-09T16:00Z"), [],
    "the healthy mailbox's busy hour still blocks");
});

test("a custom Graph time zone fails only the working hours", () => {
  const outcomes = graph.getScheduleToBlocks(payload("graph/get_schedule.json"));
  // The batch answer carries a mailbox whose working hours name a custom zone; its
  // blocks convert, and the rule that cannot be resolved is reported on its own.
  assert.equal(blocks(outcomes["bo@example.com"]).length, 0);
  const custom = outcomes["bo@example.com"];
  assert.equal(custom.status, "available", "the mailbox's blocks are unaffected");

  const unavailable = graph.workingHoursToRule(
    JSON.stringify({
      daysOfWeek: ["monday"],
      startTime: "09:00:00.0000000",
      endTime: "17:00:00.0000000",
      timeZone: { "@odata.type": "#microsoft.graph.customTimeZone", bias: 60, name: "Customized Time Zone" },
    }),
  );
  assert.equal(unavailable.status, "unavailable");
  assert.equal(unavailable.code, "custom_time_zone");

  const healthy = graph.workingHoursToRule(payload("graph/working_hours.json"));
  assert.equal(healthy.status, "available");
  assert.deepEqual(healthy.rule.params.weekly.mon, ["09:00-17:00"]);
  assert.equal(healthy.rule.params.weekly.sat, undefined, "Saturday is not a working day");
  assert.deepEqual(Object.keys(healthy.rule.params.weekly).sort(), ["fri", "mon", "thu", "tue", "wed"]);
});

test("the two recorded calendars share one key per recurring instance", () => {
  const { ana, bo, room } = calendars();

  const shared = blockFor(ana, "ana-shared-interview-0900");
  assert.equal(shared.event_key, "series-interview@example.com@2027-03-09T15:00:00Z");
  assert.equal(blockFor(bo, "bo-shared-interview-0900").event_key, shared.event_key,
    "the two attendees received the instance in different offsets and derived one key");
  assert.equal(blockFor(ana, "ana-shared-interview-1000").event_key,
    "series-interview@example.com@2027-03-10T15:00:00Z", "a distinct instance derives a distinct key");
  assert.equal(blockFor(bo, "bo-shared-interview-1000").event_key,
    "series-interview@example.com@2027-03-10T15:00:00Z");

  const focus = blockFor(ana, "ana-focus-0312");
  assert.equal(focus.status, "busy");
  assert.deepEqual(focus.tags, ["status/focus"]);
  const dayOff = blockFor(ana, "ana-pto-0311");
  assert.equal(dayOff.all_day, true);
  assert.deepEqual(dayOff.span, ["2027-03-11T05:00Z", "2027-03-12T05:00Z"]);

  assert.equal(room.length, 2);
  assert.ok(room.every((entry) => entry.event_key === undefined && entry.status === "busy"),
    "free/busy ranges carry no event identity");
});

test("recorded calendars become the offers the rules describe", () => {
  const { ana, bo, room } = calendars();
  const engine = new Engine();
  const request = googleRequest(plain(ana), plain(bo), room);

  assert.deepEqual(engine.validate(request).errors, []);
  const response = engine.findAvailability(request);

  assert.ok(response.offers.length > 0, "the recorded calendars leave bookable time");
  for (const offer of response.offers) {
    assert.ok(offerStart(offer) >= "2027-03-08T14:00Z", `the offer at ${offerStart(offer)} is early`);
    assert.ok(
      offer.events.at(-1).end <= "2027-03-12T22:00Z",
      `the offer at ${offerStart(offer)} runs past Ana's working hours`,
    );
  }

  const offered = starts(response);
  assert.ok(offered.includes("2027-03-09T14:00Z"),
    "the shared interview counts once for the pooled cap");
  assert.ok(!offered.includes("2027-03-09T16:00Z"), "the room's free/busy range blocks that start");
  assert.ok(offered.includes("2027-03-09T17:00Z"), "the room is free again after its busy range");
  assert.deepEqual(startsBetween(response, "2027-03-10T00:00Z", "2027-03-11T00:00Z"), [],
    "two interviews plus a booking pass the cap of two on 10 March");
  assert.ok(
    reasonsBetween(response, "2027-03-10T00:00Z", "2027-03-11T00:00Z").some(
      ([participant, code, rule]) => participant === "ana" && code === "cap_reached" && rule === "interviews_per_day",
    ),
    "the exclusions on 10 March name the cap",
  );
  assert.deepEqual(startsBetween(response, "2027-03-11T00:00Z", "2027-03-12T05:00Z"), [],
    "the all-day day off occupies 11 March, so no offer starts inside it");
  assert.ok(offered.includes("2027-03-12T14:00Z"),
    "a `status/focus` block the policy ignores does not block");

  assert.equal(confirmed(engine, request, "2027-03-09T14:00Z"), true,
    "the offer the search produced is one `checkSlot` confirms");
});

test("focus time blocks when the policy does not ignore it", () => {
  const { ana, bo, room } = calendars();
  const engine = new Engine();
  const request = googleRequest(plain(ana), plain(bo), room);
  const withoutPolicy = { ...request, rules: request.rules.filter((rule) => rule.id !== "focus_policy") };

  const offered = starts(engine.findAvailability(withoutPolicy));

  assert.ok(!offered.includes("2027-03-12T14:00Z"),
    "the focus block covers 14:00-16:00Z on 12 March once nothing ignores it");
  assert.ok(offered.includes("2027-03-12T16:00Z"));
});

test("the marker constants are the package's own, from every entry point", () => {
  // An application reads the markers to $expand a provider's extended properties,
  // so the root export and both adapter submodules have to name the same fields.
  assert.equal(MARKER_TAGS, "opportune.tags");
  assert.equal(MARKER_EVENT_KEY, "opportune.event_key");
  assert.equal(graph.MARKER_TAGS, MARKER_TAGS);
  assert.equal(google.MARKER_TAGS, MARKER_TAGS);
  assert.equal(graph.MARKER_EVENT_KEY, MARKER_EVENT_KEY);
  assert.equal(google.MARKER_EVENT_KEY, MARKER_EVENT_KEY);

  // The Graph IDs are built from the permanent property set and the marker names,
  // which is what makes the two halves agree without a second source of truth.
  assert.match(graph.EXTENDED_PROPERTY_TAGS_ID, new RegExp(`Name ${MARKER_TAGS}$`));
  assert.match(graph.EXTENDED_PROPERTY_EVENT_KEY_ID, new RegExp(`Name ${MARKER_EVENT_KEY}$`));
  assert.ok(
    graph.EXTENDED_PROPERTY_TAGS_ID.includes(braceNamespace()),
    "the ID is in the namespace GUID the package exports",
  );
  assert.ok(graph.EXTENDED_PROPERTY_EXPAND.includes(graph.EXTENDED_PROPERTY_TAGS_ID));
  assert.ok(graph.EXTENDED_PROPERTY_EXPAND.includes(graph.EXTENDED_PROPERTY_EVENT_KEY_ID));
});

/** The Graph extended-property set's GUID, in the braces Graph's IDs use. */
function braceNamespace() {
  return `{${graph.EXTENDED_PROPERTY_NAMESPACE_GUID}}`;
}
