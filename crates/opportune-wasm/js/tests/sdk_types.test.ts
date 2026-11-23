/**
 * The documents a consumer actually holds, under the package's strict compile.
 *
 * A provider payload in an application is the SDK's own value, not a hand-written
 * record: `googleapis` types it as `calendar_v3.Schema$Events` and
 * `@microsoft/microsoft-graph-types` as `CalendarView`/`ScheduleInformation`, and both
 * declare their fields in an interface with no index signature. An interface without
 * one is not assignable to `Record<string, unknown>`, so a payload parameter typed that
 * way rejected the one document every consumer passes — at compile time, in the
 * consumer's build, with the adapter as the culprit. The interfaces below are that
 * shape: fields and no index signature, so this file compiles only while the six
 * adapters accept them.
 *
 * The other half is what the package hands back. `AvailableWorkingHoursOutcome.rule`
 * is a `WindowRule` rather than the seven-way `Rule` union the schema used to advertise
 * — the conversion produces a `window` rule and nothing else — so a caller can read
 * `kind` off it without a guard in between. The annotations below are the assertion: a
 * rule widened back to the union fails `npm run check` here.
 *
 * Both halves run against the built package, so the imports are the published names:
 * `node --test` and `bun test` resolve them through `dist/`, while `npm run check`
 * resolves them to `src/` (see `tsconfig.check.json`). That is what lets the
 * typechecker read this file without a wasm build.
 */

import assert from "node:assert/strict";
import test from "node:test";

import {
  OpportuneError,
  httpStatusForError,
  isKnownStatus,
  requiredRanges,
  singleGoal,
  statusPolicy,
  type Block,
  type BlockStatus,
  type GraphWorkingHoursOutcome,
  type OfferFlag,
  type OpportuneErrorCode,
  type Participant,
  type PublicResponse,
  type PublicSlotCheck,
  type RangesQuery,
  type Request,
  type Status,
  type StatusPolicyRule,
  type WindowRule,
} from "opportune";
import {
  calendarViewToBlocks,
  getScheduleToBlocks,
  workingHoursToRule,
} from "opportune/adapters/graph";
import { eventsListToBlocks, freeBusyToBlocks } from "opportune/adapters/google";
import {
  andWho,
  businessHours,
  capPerDay,
  exceptWho,
  httpStatusForError as presetHttpStatusForError,
  statusPolicy as presetStatusPolicy,
  unless,
  type BlockStatus as PresetBlockStatus,
  type CapRule,
  type OfferFlag as PresetOfferFlag,
  type OpportuneErrorCode as PresetOpportuneErrorCode,
  type PublicResponse as PresetPublicResponse,
  type PublicSlotCheck as PresetPublicSlotCheck,
} from "opportune/presets";

/** A Graph `dateTimeTimeZone`, as the SDK declares it. */
interface GraphDateTimeTimeZone {
  readonly dateTime: string;
  readonly timeZone: string;
}

/** A Graph event as `calendarView` returns it: fields, and no index signature. */
interface GraphEvent {
  readonly id: string;
  readonly subject: string;
  readonly isAllDay: boolean;
  readonly showAs: string;
  readonly start: GraphDateTimeTimeZone;
  readonly end: GraphDateTimeTimeZone;
}

/** The `calendarView` response the SDK hands back. */
interface GraphCalendarView {
  readonly value: readonly GraphEvent[];
}

/** One `scheduleItems` entry of a `getSchedule` response. */
interface GraphScheduleItem {
  readonly status: string;
  readonly start: GraphDateTimeTimeZone;
  readonly end: GraphDateTimeTimeZone;
}

/** One mailbox of a `getSchedule` response. */
interface GraphScheduleInformation {
  readonly scheduleId: string;
  readonly scheduleItems: readonly GraphScheduleItem[];
}

/** The `getSchedule` response the SDK hands back. */
interface GraphGetSchedule {
  readonly value: readonly GraphScheduleInformation[];
}

/** One mailbox's `workingHours`, as Graph reports it. */
interface GraphWorkingHours {
  readonly daysOfWeek: readonly string[];
  readonly startTime: string;
  readonly endTime: string;
  readonly timeZone: { readonly name: string };
}

/** An instant of a Google event, as `calendar_v3.Schema$EventDateTime` declares it. */
interface GoogleEventDateTime {
  readonly dateTime: string;
}

/** One Google event as `events.list` returns it. */
interface GoogleEvent {
  readonly id: string;
  readonly status: string;
  readonly start: GoogleEventDateTime;
  readonly end: GoogleEventDateTime;
}

/** The `events.list` response the SDK hands back. */
interface GoogleEventsList {
  readonly items: readonly GoogleEvent[];
}

/** One calendar of a `freeBusy` response, with the provider's own busy ranges. */
interface GoogleFreeBusyCalendar {
  readonly busy: readonly { readonly start: string; readonly end: string }[];
  readonly errors?: readonly { readonly domain: string; readonly reason: string }[];
}

/** The `freeBusy.query` response the SDK hands back. */
interface GoogleFreeBusy {
  readonly calendars: Readonly<Record<string, GoogleFreeBusyCalendar>>;
}

const CALENDAR_VIEW: GraphCalendarView = {
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

const SCHEDULE: GraphGetSchedule = {
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
};

const WORKING_HOURS: GraphWorkingHours = {
  daysOfWeek: ["monday", "tuesday", "wednesday", "thursday", "friday"],
  startTime: "09:00:00.0000000",
  endTime: "17:00:00.0000000",
  timeZone: { name: "America/New_York" },
};

const EVENTS_LIST: GoogleEventsList = {
  items: [
    {
      id: "ana-interview-0900",
      status: "confirmed",
      start: { dateTime: "2027-06-14T09:00:00Z" },
      end: { dateTime: "2027-06-14T10:00:00Z" },
    },
  ],
};

const FREE_BUSY: GoogleFreeBusy = {
  calendars: {
    "ana@example.com": {
      busy: [{ start: "2027-06-14T09:00:00Z", end: "2027-06-14T10:00:00Z" }],
    },
  },
};

/** The fields a `Request` carries that a `RangesQuery` has no place for. */
type RequestOnlyField = Exclude<keyof Request, keyof RangesQuery>;

/**
 * The names `requiredRanges` drops when it is handed a whole request.
 *
 * This is a copy of a list the projection keeps private, so it cannot catch that list
 * changing; the runtime cases in `errors_and_wasm32.test.mjs` do, from both sides —
 * one of the six surviving into the document and one of the query's own fields being
 * dropped. What this catches is the generated schema moving underneath it: a `Request`
 * field added to the schema and not to the projection reaches the wire as
 * `unknown_field`, on a document the signature accepts, and a hand-written request in
 * any test of the current shape would not carry the new field to notice.
 */
const DROPPED = [
  "occurrences",
  "exclude_event_keys",
  "calendars",
  "weights",
  "limit",
  "explain",
] as const satisfies readonly RequestOnlyField[];

/** The blocks a conversion answered with, asserting it did convert. */
function blocks(outcome: ReturnType<typeof calendarViewToBlocks>): number {
  assert.equal(outcome.status, "available");
  if (outcome.status !== "available") {
    throw new Error("the calendar did not convert");
  }
  return outcome.blocks.length;
}

/** The rule the working-hours conversion answered with, asserting it converted. */
function rule(outcome: GraphWorkingHoursOutcome): WindowRule {
  assert.equal(outcome.status, "available");
  if (outcome.status !== "available") {
    throw new Error("the working hours did not convert");
  }
  return outcome.rule;
}

test("the five adapters read the SDK's own document shapes", () => {
  // Every value below is typed by an interface with no index signature, which is the
  // shape the SDK ships and the one the payload parameter used to refuse.
  assert.equal(blocks(calendarViewToBlocks(CALENDAR_VIEW, "UTC")), 1);
  assert.equal(blocks(eventsListToBlocks(EVENTS_LIST, "UTC")), 1);
  assert.deepEqual(Object.keys(getScheduleToBlocks(SCHEDULE)), ["ana@example.com"]);
  assert.deepEqual(Object.keys(freeBusyToBlocks(FREE_BUSY)), ["ana@example.com"]);
  assert.equal(workingHoursToRule(WORKING_HOURS).status, "available");

  // A payload still crosses as text, which is how an HTTP client that did not parse
  // the body holds it.
  assert.equal(blocks(calendarViewToBlocks(JSON.stringify(CALENDAR_VIEW), "UTC")), 1);

  const converted: WindowRule = rule(workingHoursToRule(WORKING_HOURS));
  assert.equal(converted.kind, "window", "the outcome's rule is the window the conversion built");
});

test("a ranges query takes the request the caller already holds", () => {
  // `Request` carries six fields a query has no place for. The signature accepts both
  // documents, so passing the request is one argument rather than a second document
  // rebuilt by hand — and the annotation below is what makes this the compile check
  // for it.
  const request: Request = {
    now: "2027-06-14T07:00Z",
    goal: {
      zone: "Europe/London",
      search_window: ["2027-06-13T23:00Z", "2027-06-18T23:00Z"],
      parts: [{ duration: "30m", requirement: { all: [{ person: "rich" }] } }],
    },
    participants: [{ id: "rich", zone: "Europe/London", calendar: [] }],
    occurrences: [],
    exclude_event_keys: ["evt_1"],
    calendars: { org: { zone: "Europe/London", working_days: "mon-fri" } },
    weights: { rich: 1 },
    limit: 5,
    explain: { limit: 3 },
  };

  const ranges = requiredRanges(request);
  assert.equal(ranges.calendar.length, 2, "the range to fetch is the request's own window");
  assert.equal(
    ranges.occurrences,
    undefined,
    "no calendar is stored, so the field is omitted rather than written as `null`",
  );

  // The projection names the six fields it drops, and `DROPPED` above is what makes a
  // seventh one fail this compile instead of the caller's first call: `uncovered` is
  // `{}` only while every request-only field the generated types declare is a name the
  // engine's projection removes.
  const uncovered: Record<Exclude<RequestOnlyField, (typeof DROPPED)[number]>, never> = {};
  assert.deepEqual(
    Object.keys(uncovered),
    [],
    "a `Request` field outside the six `requiredRanges` drops: a whole request now rejects it as `unknown_field`",
  );
});

/**
 * `unless` preserves the rule type it is given, so a caller builds a policy tier
 * in one call and still reads the rule's own fields without a guard.
 *
 * The annotation is the assertion: an `unless` that widened its argument to the
 * seven-way rule union fails this compile.
 */
test("unless keeps the rule type it is given", () => {
  const capped: CapRule = unless(capPerDay({ tag: "interview" }, 2), { tag: "vip" });
  assert.equal(capped.kind, "cap");
  assert.deepEqual(capped.when, { not: { tag: "vip" } });

  const policy: StatusPolicyRule = unless(
    statusPolicy([{ status: "tentative", effect: "ignore" }]),
    { tag: "vip" },
  );
  assert.equal(policy.kind, "status_policy");
  assert.equal(presetStatusPolicy, statusPolicy);

  // Compile-time check: non-rule documents such as Participant cannot be passed to `unless`.
  const participant: Participant = { id: "ana", zone: "UTC" };
  // @ts-expect-error Participant is not a Rule
  const rejectedCheck = () => unless(participant, { tag: "vip" });
  assert.throws(rejectedCheck, OpportuneError);
});

test("andWho and exceptWho keep the rule type and the default host clause", () => {
  // The annotation is the assertion: a twin that widened its argument to the
  // seven-way rule union fails this compile, and so does one that typed `who` as
  // anything narrower than the generated `Selector`.
  const capped: CapRule = andWho(capPerDay({ tag: "interview" }, 2), { kind: "person" });
  assert.equal(capped.kind, "cap");
  assert.deepEqual(
    capped.who,
    { all: [{ role: "host" }, { kind: "person" }] },
    "an omitted `who` is the wire's host default, not `true`: the selector narrows it",
  );

  const exempt: CapRule = exceptWho(capPerDay({ tag: "interview" }, 2), { ids: ["ceo"] });
  assert.equal(exempt.kind, "cap");
  assert.deepEqual(exempt.who, { all: [{ role: "host" }, { not: { ids: ["ceo"] } }] });

  const hours: WindowRule = andWho(businessHours(), { kind: "person" });
  assert.equal(hours.kind, "window");
  assert.deepEqual(hours.who, { all: [{ role: "host" }, { kind: "person" }] });

  const policy: StatusPolicyRule = exceptWho(
    statusPolicy([{ status: "tentative", effect: "ignore" }]),
    { kind: "resource" },
  );
  assert.equal(policy.kind, "status_policy");
  assert.equal(presetStatusPolicy, statusPolicy);

  // Compile-time check: a document that is not a rule cannot be narrowed either.
  const participant: Participant = { id: "ana", zone: "UTC" };
  // @ts-expect-error Participant is not a Rule
  const rejectedAnd = () => andWho(participant, { kind: "person" });
  // @ts-expect-error Participant is not a Rule
  const rejectedExcept = () => exceptWho(participant, { ids: ["ceo"] });
  assert.throws(rejectedAnd, OpportuneError);
  assert.throws(rejectedExcept, OpportuneError);
});

test("isKnownStatus narrows nullable Block.status and singleGoal accepts readonly arrays", () => {
  const block: Block = { span: ["2027-06-14T09:00Z", "2027-06-14T10:00Z"], status: "tentative" };
  let narrowed: Status | undefined;
  if (isKnownStatus(block.status)) {
    narrowed = block.status;
  }
  assert.equal(narrowed, "tentative");

  const readonlyTags = ["interview", "vip"] as const;
  const readonlyCandidates = ["2027-06-14T09:00Z"] as const;
  const goal = singleGoal("30m", { all: [{ person: "ana" }] }, {
    zone: "UTC",
    searchWindow: ["2027-06-14T08:00Z", "2027-06-14T18:00Z"],
    tags: readonlyTags,
    candidates: readonlyCandidates,
  });
  assert.deepEqual(goal.parts[0]?.tags, ["interview", "vip"]);
  assert.deepEqual(goal.candidates, ["2027-06-14T09:00Z"]);

  assert.equal(httpStatusForError, presetHttpStatusForError);
  assert.equal(httpStatusForError(new OpportuneError("invalid_request", "bad")), 422);
  assert.equal(httpStatusForError(new OpportuneError("invalid_limits", "bad")), 500);
});

/**
 * `true` when `A` and `B` are the same type, so one declaration serves both.
 *
 * Identity rather than mutual assignability: every union here ends in `| (string & {})`,
 * so any two of them — and a bare `string` — are assignable in both directions, and an
 * assignability check would call a narrower duplicate of `BlockStatus` the same
 * declaration. Two conditional types are interchangeable only when the types they test
 * are identical, which is the claim this makes.
 */
type Same<A, B> = (<T>() => T extends A ? 1 : 2) extends (<T>() => T extends B ? 1 : 2)
  ? true
  : false;

/**
 * The code an error carries, as the open union a handler switches on.
 *
 * The annotation is the assertion: `OpportuneError.code` typed as a plain `string` would
 * fail this compile, and a handler could not name a code without a cast.
 */
function codeOf(error: OpportuneError): OpportuneErrorCode {
  return error.code;
}

test("an error's code is one of the open codes", () => {
  assert.equal(codeOf(new OpportuneError("invalid_json", "the document is not JSON")), "invalid_json");
  // A domain code the engine raises is a value of the same type, which is what the open
  // tail is for: neither literal above is written by this binding.
  assert.equal(
    codeOf(new OpportuneError("window_ranges_overlap", "the ranges overlap")),
    "window_ranges_overlap",
  );
});

/**
 * The open unions and the public projections are one declaration from both subpaths.
 *
 * An isomorphic application types its answer from `opportune/presets` on the client and
 * from `opportune` on the route, so the two halves have to name the same document. Each
 * name below is imported from both modules and compared: a subpath that declared its
 * own copy fails `npm run check` here rather than in a consumer's build.
 *
 * The annotations are also the open-union contract: `"free-ish"` and `"a_code_from_a_`
 * `later_release"` are values no release here writes, and each still assigns, while
 * `isKnownStatus` is the runtime half that tells a caller which ones the engine gave a
 * meaning to.
 */
test("the open unions and the projections are one declaration from both subpaths", () => {
  const statuses: BlockStatus[] = ["free", "tentative", "busy", "free-ish"];
  const flags: OfferFlag[] = ["calendar_unknown", "a_flag_from_a_later_release"];
  const codes: OpportuneErrorCode[] = [
    "invalid_json",
    "engine_disposed",
    "window_ranges_overlap",
  ];
  const known: ("free" | "tentative" | "busy")[] = statuses.filter(
    (status): status is "free" | "tentative" | "busy" => isKnownStatus(status),
  );
  assert.deepEqual(known, ["free", "tentative", "busy"]);
  assert.deepEqual(flags.length, 2);
  assert.deepEqual(codes.length, 3);

  const sameStatus: Same<BlockStatus, PresetBlockStatus> = true;
  const sameFlag: Same<OfferFlag, PresetOfferFlag> = true;
  const sameCode: Same<OpportuneErrorCode, PresetOpportuneErrorCode> = true;
  const sameResponse: Same<PublicResponse, PresetPublicResponse> = true;
  const sameSlotCheck: Same<PublicSlotCheck, PresetPublicSlotCheck> = true;
  assert.ok(
    sameStatus && sameFlag && sameCode && sameResponse && sameSlotCheck,
    "a name the two subpaths export is declared twice",
  );
});
