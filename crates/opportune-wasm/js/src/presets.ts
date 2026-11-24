/**
 * Presets that build the general rule types, adding no semantics of their own.
 *
 * Each preset generates its rule ID from its own name, so a rule always has an ID
 * to cite; `availableOnly` derives its ID from the attendee, so two attendees'
 * stated availability compose in one request. Rule IDs are unique in a request, so
 * two presets of the same kind need distinct `ruleId` values.
 *
 * Every preset returns the generated rule type the engine reads — nothing parallel
 * to it — so its output serializes to the wire format, stores as JSON, and plugs
 * straight into a request.
 *
 * Each preset's `options` is optional, and one that is not an object — `null`, a
 * number — is rejected as `invalid_type` rather than read as "no options": every
 * field in it is optional, so there is nothing a wrong value could mean.
 */

import { reason, rejection } from "./errors.js";
import { asPlainRecord, asRecord, optionsOf, typeName } from "./json.js";
import type {
  CapMeasure,
  CapPer,
  CapPeriod,
  CapRule,
  ClearanceRule,
  LeadTimeMax,
  LeadTimeRule,
  Participant,
  RestRule,
  Rule,
  Selector,
  Span,
  StatusPolicyEntry,
  StatusPolicyRule,
  WindowRule,
} from "./generated/types.js";

const WALL_RANGE = /^(?:[01][0-9]|2[0-3]):[0-5][0-9]-(?:(?:[01][0-9]|2[0-3]):[0-5][0-9]|24:00)$/;

/** A duration written in hours and minutes, such as `30m` or `1h30m`. */
const DURATION = /^(?:\d+h(?:\d+m)?|\d+m)$/;

/** A civil date written `YYYY-MM-DD`. */
const CIVIL_DATE = /^\d{4}-\d{2}-\d{2}$/;

/**
 * The wire spelling of an RFC 3339 instant: a date, a clock and an explicit
 * offset.
 *
 * The fields are bounded the way the wire layer bounds them — `HH:MM` with
 * optional `:SS` and up to nine fraction digits, `Z` or `±HH:MM` — so an instant
 * the preset accepts is one the engine's own parser reads rather than one it
 * rejects afterwards with a pointer into a document the caller never wrote. The
 * date and the seconds are captured because both are read from the text, not from
 * the instant `Date.parse` builds (see {@link utcMinute}).
 */
const INSTANT =
  /^(\d{4}-\d{2}-\d{2})[Tt](?:[01]\d|2[0-3]):[0-5]\d(?::([0-5]\d(?:\.\d{1,9})?))?(?:[Zz]|[+-](?:[01]\d|2[0-3]):[0-5]\d)$/;

/** Seconds that add nothing to the minute: absent, `00`, or `00` and zeros. */
const WHOLE_MINUTE = /^00(?:\.0+)?$/;

const MINUTE_MS = 60_000;
const DAY_MINUTES = 24 * 60;

/** Midnight UTC on 1970-01-01: the origin the whole-minute arithmetic counts from. */
const EPOCH_MS = Date.UTC(1970, 0, 1);

/**
 * The earliest and latest instants a stated availability may name, as `Date.parse`
 * reads them: the range every `Instant` in the engine holds.
 *
 * A span outside it has no dated window the engine could read, so it is refused here
 * rather than written as a window over a date the engine silently never matches.
 */
const SUPPORTED_FROM_MS = Date.UTC(1975, 0, 1);
const SUPPORTED_TO_MS = Date.UTC(2100, 0, 1);

/** The range above as a message names it, which is the engine's own wording. */
const SUPPORTED_RANGE = "1975-01-01T00:00Z..=2100-01-01T00:00Z";

/**
 * How much of a refused object the message echoes: enough to read its members back,
 * bounded so a caller-controlled value cannot make the message as large as itself.
 */
const MAX_SHOWN = 120;

/**
 * The most dated entries one stated availability may need: ten years of UTC days.
 *
 * A stated availability is a handful of hours an external attendee is reachable.
 * The ceiling is a guard, not a limit anyone should meet: the preset writes one
 * entry per UTC day of every span, so an availability without a ceiling turns one
 * bad argument into millions of entries written before the engine is ever called.
 */
const MAX_DATED_ENTRIES = 3660;

/**
 * The values `stated` names, as a list.
 *
 * A wall-clock argument has two shapes — one range, or a list of them — and this
 * is the one place that reads both. JavaScript callers are not type-checked, so a
 * value that is neither arrives as whatever a variable held — `null`, a number —
 * and a `for…of` over it is a raw `TypeError` rather than a rejection. `where`
 * names the argument in the message.
 */
function statedHours(stated: unknown, where: string): readonly string[] {
  if (typeof stated === "string") {
    return [stated];
  }
  try {
    return [...(stated as Iterable<string>)];
  } catch (cause) {
    throw rejection(
      "invalid_argument",
      `${where} is one wall-clock range such as \`09:00-17:00\`, or a list of them, ` +
        `not a value of type ${typeName(stated)}: ${reason(cause)}`,
      cause,
    );
  }
}

/**
 * The wall ranges `stated` names, rejected when one of them could never be a range.
 *
 * `where` names the argument in the message, so a caller reads which value of a
 * mapping was wrong rather than that some value was.
 */
function wallRanges(stated: unknown, where: string): string[] {
  const ranges = statedHours(stated, where);
  for (const range of ranges) {
    if (typeof range !== "string" || !WALL_RANGE.test(range)) {
      throw rejection(
        "invalid_argument",
        `${where} is one wall-clock range such as \`09:00-17:00\`, or a list of them, ` +
          `not \`${reason(range)}\``,
      );
    }
  }
  return [...ranges];
}

/**
 * What every preset that applies to hosts lets a caller override.
 *
 * `who` narrows the participants the rule constrains and `when` the bookings it
 * applies to; both default to the rule type's own selector, so a preset that names
 * neither applies to every host and every booking.
 */
export interface ScopedOptions {
  /** The rule's ID, defaulting to the preset's name. */
  ruleId?: string;
  /** Which participants the rule constrains, or every host by default. */
  who?: Selector;
  /** Which bookings it applies to, or all of them by default. */
  when?: Selector;
}

/**
 * The rule with `who` and `when` replaced where the caller named them.
 *
 * A field the caller left absent keeps the rule type's default, so spreading only
 * what is present is what makes `who`/`when` overrides rather than a replacement
 * of both selectors.
 */
function scoped<Rule extends { who?: Selector | null; when?: Selector | null }>(
  rule: Rule,
  who: Selector | undefined,
  when: Selector | undefined,
): Rule {
  if (who === undefined && when === undefined) {
    return rule;
  }
  return {
    ...rule,
    ...(who === undefined ? {} : { who }),
    ...(when === undefined ? {} : { when }),
  };
}

/**
 * A `window` rule open `hours` on `days`, read in `zone`.
 *
 * `days` is a wire day key such as `mon-fri` or `mon,wed`; `hours` is one wire wall
 * range such as `09:00-17:00`, or a list of them for a day split into several
 * ranges. `zone` is `participant`, `event` or an IANA name.
 *
 * @throws OpportuneError with code `invalid_argument` when `days` is not a string,
 *   when `hours` is neither a string nor a list of them, or when an entry of
 *   `hours` is not a wall range. The engine still decides what a range or a day key
 *   means; this catches the shapes that could never be one.
 */
export function businessHours(
  days = "mon-fri",
  hours: string | readonly string[] = "09:00-17:00",
  options: ScopedOptions & { zone?: string } = {},
): WindowRule {
  if (typeof days !== "string") {
    throw rejection(
      "invalid_argument",
      `\`days\` is a wire day key such as \`mon-fri\`, not a value of type ${typeName(days)}`,
    );
  }
  const ranges = wallRanges(hours, "`hours`");
  const { zone = "participant", ruleId = "business_hours", who, when } =
    optionsOf(options, "businessHours");
  return scoped<WindowRule>(
    {
      id: ruleId,
      kind: "window",
      params: { zone, weekly: { [days]: ranges } },
    },
    who,
    when,
  );
}

/**
 * A `window` rule open on `dates` only, read in `zone`.
 *
 * `dates` maps a wire date key — `2027-06-14` or a range such as
 * `2027-06-14..2027-06-16` — to the hours it has, so this is the conference booth
 * shape: open on the days the event runs, and on no other. The dates are the whole
 * window rather than an override on top of weekly hours: a date no key covers is
 * closed.
 *
 * ```ts
 * datedHours({ "2027-06-14": "10:00-16:00", "2027-06-15": ["10:00-12:00", "13:00-16:00"] });
 * ```
 *
 * @throws OpportuneError with code `invalid_argument` when `dates` is not a mapping
 *   of date keys to hours, or a value is not a wall range or a list of them.
 */
export function datedHours(
  dates: Readonly<Record<string, string | readonly string[]>>,
  options: ScopedOptions & { zone?: string } = {},
): WindowRule {
  const { zone = "event", ruleId = "dated_hours", who, when } = optionsOf(options, "datedHours");
  return scoped<WindowRule>(
    { id: ruleId, kind: "window", params: { zone, dates: hoursByKey(dates, "dates") } },
    who,
    when,
  );
}

/**
 * A `window` rule open on each weekday's own hours, read in `zone`.
 *
 * `weekly` maps a wire day key — `mon`, `mon,wed` or `mon-fri` — to the hours
 * those days share: one wall range such as `09:00-17:00`, or a list of them for a
 * day split around lunch. A day no key covers is closed, so this is one rule for
 * the ordinary week rather than one `businessHours` rule per day — and it is a
 * window, so it gets no clearance and never triggers rest.
 *
 * `dates` overrides the weekly hours for the dates it names, keyed like
 * {@link datedHours}; a date mapped to an empty list is closed, which is how a
 * holiday is written.
 *
 * ```ts
 * weeklyHours(
 *   { mon: "09:00-17:00", tue: ["09:00-12:00", "14:00-18:00"] },
 *   { dates: { "2027-12-25": [] } },
 * );
 * ```
 *
 * @throws OpportuneError with code `invalid_argument` when `weekly` or `dates` is
 *   not a mapping of keys to hours, or a value is not a wall range or a list of
 *   them. An overlapping pair of ranges is the engine's rejection, with the pointer
 *   of the key that holds it.
 */
export function weeklyHours(
  weekly: Readonly<Record<string, string | readonly string[]>>,
  options: WeeklyHoursOptions = {},
): WindowRule {
  const {
    zone: namedZone,
    dates,
    ruleId = "weekly_hours",
    who,
    when,
  } = optionsOf(options, "weeklyHours");
  return scoped<WindowRule>(
    {
      id: ruleId,
      kind: "window",
      params: {
        zone: namedZone ?? "participant",
        weekly: hoursByKey(weekly, "weekly"),
        ...(dates == null ? {} : { dates: hoursByKey(dates, "dates") }),
      },
    },
    who,
    when,
  );
}

/**
 * What {@link weeklyHours} lets a caller override beyond the scope.
 *
 * `zone` and `dates` each read an `undefined` or a `null` as the engine's own
 * default — the reading `Engine`'s `limits` argument takes — so a caller holding a
 * variable that may be nothing writes the value it holds instead of a document the
 * engine would refuse. `ruleId` keeps the presets' shared reading, where only an
 * absent value is the default.
 */
export interface WeeklyHoursOptions extends ScopedOptions {
  /** The zone the hours are read in, defaulting to the participant's own. */
  zone?: string | null;
  /** Hours that override the weekly ones, keyed by a date or date range. */
  dates?: Readonly<Record<string, string | readonly string[]>> | null;
}

/**
 * The hours each key of a window mapping states, or the rejection a value that is
 * not a mapping of them raises.
 *
 * A list of `[key, value]` pairs would read as a mapping here and at the wire, so
 * the shape is checked rather than coerced. A `Map` or a `Set` has no enumerable
 * members, so it would otherwise be read as a mapping of no keys at all — a window
 * closed everywhere rather than the mistake it is. So would any other object that is
 * not a plain mapping: a `Date`, a `RegExp` or an instance of a class carries no
 * enumerable members either, and reading one as a mapping closes every day of the
 * window instead of saying that the value was never a mapping. `where` names the
 * argument in the message.
 *
 * Every key the caller wrote is kept as it was. A mapping read from a document can
 * carry an own `__proto__` key, which assigning into the object would read as its
 * prototype and drop — a closed window rather than the key the engine rejects.
 */
function hoursByKey(stated: unknown, where: string): Record<string, string[]> {
  const mapping = asPlainRecord(stated);
  if (mapping === undefined) {
    throw rejection(
      "invalid_argument",
      `\`${where}\` is a mapping of keys to hours, ` +
        `not a value of type ${typeName(stated)}`,
    );
  }
  const hours: [string, string[]][] = Object.entries(mapping).map(([key, value]) => [
    key,
    wallRanges(value, `\`${where}[${key}]\``),
  ]);
  return Object.fromEntries(hours);
}

/** A `clearance` rule: the gaps a booking needs before and after it. */
export function buffers(
  before = "10m",
  after = "10m",
  options: ScopedOptions & { minGap?: string } = {},
): ClearanceRule {
  const { ruleId = "buffers", minGap = "0m", who, when } = optionsOf(options, "buffers");
  return scoped<ClearanceRule>(
    { id: ruleId, kind: "clearance", params: { before, after, min_gap: minGap } },
    who,
    when,
  );
}

/** A `lead_time` rule carrying only a minimum notice. */
export function minNotice(notice = "4h", options: ScopedOptions = {}): LeadTimeRule {
  const { ruleId = "min_notice", who, when } = optionsOf(options, "minNotice");
  return scoped<LeadTimeRule>(
    { id: ruleId, kind: "lead_time", params: { min: notice } },
    who,
    when,
  );
}

/**
 * A `lead_time` rule carrying only a latest start.
 *
 * `max` is one of the generated forms — a duration, a date, a number of days, or a
 * number of working days in a calendar — or the shorthand for the first three: a
 * duration or date string, such as `"4h"` or `"2027-07-01"`, or a calendar-day
 * count as a `number`.
 *
 * @throws OpportuneError with code `invalid_argument` when `max` is not one of the
 *   accepted forms: a string that is neither a duration nor a date in the engine's
 *   grammar, such as `"14d"`, an object that is not one of the generated forms, such
 *   as `{ hours: 4 }`, or no value at all. A day is 23 to 25 hours in a
 *   named zone, so it is never an exact duration. Write `{ days: 14 }` for
 *   calendar days, or `{ calendar: "org", working_days: 14 }` for working days.
 */
export function horizon(max: HorizonMax, options: ScopedOptions = {}): LeadTimeRule {
  const { ruleId = "horizon", who, when } = optionsOf(options, "horizon");
  return scoped<LeadTimeRule>(
    { id: ruleId, kind: "lead_time", params: { min: "0m", max: leadTimeMax(max) } },
    who,
    when,
  );
}

/** A latest start: the generated forms, or the shorthand for three of them. */
export type HorizonMax = LeadTimeMax | string | number;

/**
 * The typed lead-time maximum a shorthand names.
 *
 * A `string` is read as the duration or civil date it spells and a `number` as a
 * calendar-day count. A value that is not one of those shorthands and not a
 * generated form is the rejection, so the shorthand never turns a typo into a
 * document the engine answers with a pointer into a request the caller never wrote.
 * A generated form is one the schema would write: a single `duration`, `date` or
 * `days` field, or `calendar` with `working_days`, and nothing else. An object that
 * names a field the schema does not, or leaves one out, is not a form the engine
 * reads, so it is refused here rather than sent as a document whose `unknown_field`
 * pointer names a rule the caller never wrote.
 */
function leadTimeMax(max: HorizonMax): LeadTimeMax {
  // A caller's unset horizon arrives as `null` or `undefined`, and a rule whose
  // `max` is absent is not a horizon at all: it would silently enforce nothing.
  const stated: unknown = max;
  if (isDuration(stated)) {
    return { duration: stated };
  }
  if (isCivilDateValue(stated)) {
    return { date: stated };
  }
  if (isDayCount(stated)) {
    return { days: stated };
  }
  if (typeof stated === "string") {
    if (CIVIL_DATE.test(stated)) {
      throw rejection("invalid_argument", `\`${stated}\` is not a civil date`);
    }
    throw rejection("invalid_argument", notAMaximum(reason(stated)));
  }
  if (typeof stated === "number") {
    throw rejection("invalid_argument", `\`${stated}\` is not a whole number of calendar days`);
  }
  return generatedMaximum(stated) ?? rejectMaximum(stated);
}

/**
 * The generated form `stated` is, or `undefined` when it is not one.
 *
 * The accepted shapes are the schema's own: a `{ duration }`, `{ date }`, `{ days }`
 * or `{ calendar, working_days }` document, with the field the form names and none
 * besides.
 */
function generatedMaximum(stated: unknown): LeadTimeMax | undefined {
  const fields = asRecord(stated);
  if (fields === undefined) {
    return undefined;
  }
  const names = [...Object.keys(fields)].sort();
  if (names.length === 1) {
    const name = names[0] ?? "";
    const value = fields[name];
    if (name === "duration" && isDuration(value)) {
      return { duration: value };
    }
    if (name === "date" && isCivilDateValue(value)) {
      return { date: value };
    }
    if (name === "days" && isDayCount(value)) {
      return { days: value };
    }
  }
  if (names.length === 2 && names[0] === "calendar" && names[1] === "working_days") {
    const calendar: unknown = fields["calendar"];
    const workingDays: unknown = fields["working_days"];
    if (typeof calendar === "string" && isDayCount(workingDays)) {
      return { calendar, working_days: workingDays };
    }
  }
  return undefined;
}

/** The message every value that is no lead-time maximum raises, naming `shown`. */
function notAMaximum(shown: string): string {
  return (
    `\`${shown}\` is not a duration such as \`4h\` or \`1h30m\`, a date such as ` +
    "`2027-07-01`, a number of days, or `{ calendar, working_days }`"
  );
}

/** The rejection a value that is no lead-time maximum raises, naming `stated`. */
function rejectMaximum(stated: unknown): never {
  throw rejection("invalid_argument", notAMaximum(shownMaximum(stated)));
}

/**
 * `stated` as the message names it: the members a caller wrote when it is a plain
 * mapping — so they read back the fields the schema does not read — and the value or
 * its class otherwise.
 *
 * A member is shown by its own text rather than by `JSON.stringify` of the whole
 * object, because JSON drops an `undefined` member and writes `Infinity` as `null`: the
 * message would name a document the caller never wrote, which is the failure it exists
 * to avoid. An object that is not a mapping — a `Date`, a `Map`, an instance — is named
 * by its class, the one fact about it a caller recognises.
 */
function shownMaximum(stated: unknown): string {
  const document = asPlainRecord(stated);
  if (document === undefined) {
    return typeof stated === "object" && stated !== null ? typeName(stated) : reason(stated);
  }
  const members = Object.entries(document).map(
    ([name, value]) => `${name}: ${shownMember(value)}`,
  );
  const shown = `{${members.join(", ")}}`;
  return shown.length <= MAX_SHOWN ? shown : `${shown.slice(0, MAX_SHOWN)}…`;
}

/** One member of a refused object, as the message names it. */
function shownMember(value: unknown): string {
  if (typeof value === "string") {
    return JSON.stringify(value);
  }
  return typeof value === "object" && value !== null ? typeName(value) : reason(value);
}

/** Whether `value` is a duration written in hours and minutes. */
function isDuration(value: unknown): value is string {
  return typeof value === "string" && DURATION.test(value);
}

/** Whether `value` is a civil date the calendar has. */
function isCivilDateValue(value: unknown): value is string {
  return typeof value === "string" && CIVIL_DATE.test(value) && isCivilDate(value);
}

/** The largest unsigned 32-bit integer the wire format accepts for day counts. */
const MAX_U32 = 0xffff_ffff;

/** Whether `value` is a whole, non-negative `u32` count of days. */
function isDayCount(value: unknown): value is number {
  return typeof value === "number" && Number.isInteger(value) && value >= 0 && value <= MAX_U32;
}

/**
 * A `rest` rule: no booking before `notBefore` when anything runs at or after
 * `lateAfter` the day before.
 */
export function rest(
  lateAfter = "18:00",
  notBefore = "10:00",
  options: ScopedOptions = {},
): RestRule {
  const { ruleId = "rest", who, when } = optionsOf(options, "rest");
  return scoped<RestRule>(
    { id: ruleId, kind: "rest", params: { late_after: lateAfter, not_before: notBefore } },
    who,
    when,
  );
}

/** What a [`capPerDay`], [`capPerWeek`] or [`capPerMonth`] rule limits. */
export interface CapOptions extends ScopedOptions {
  /** Whether events or their minutes count, defaulting to the events. */
  measure?: CapMeasure;
  /**
   * Whether the limit applies to each participant or to the pool together,
   * defaulting to each.
   */
  per?: CapPer;
  /**
   * The zone the buckets are read in, defaulting to the participant's own.
   *
   * A pooled cap must name `event` or an IANA zone instead: it counts the
   * participants together, and each one's own zone would count the same event
   * against a different day.
   */
  zone?: string;
}

/**
 * A `cap` rule of at most `limit` per civil day.
 *
 * ```ts
 * capPerDay({ tag: "interview" }, 2, { per: "pooled", zone: "Europe/London" });
 * ```
 *
 * @throws OpportuneError with code `invalid_argument` when a pooled cap is left in
 *   the participant's own zone.
 */
export function capPerDay(count: Selector, limit: number, options: CapOptions = {}): CapRule {
  return cap("day", count, limit, "cap_per_day", "capPerDay", options);
}

/**
 * A `cap` rule of at most `limit` per week.
 *
 * A week starts on `weekStarts` and runs seven civil days. `measure`, `per` and
 * `zone` mean what they mean in {@link capPerDay}.
 *
 * @throws OpportuneError with code `invalid_argument` when a pooled cap is left in
 *   the participant's own zone.
 */
export function capPerWeek(
  count: Selector,
  limit: number,
  options: CapOptions & { weekStarts?: string } = {},
): CapRule {
  return cap("week", count, limit, "cap_per_week", "capPerWeek", options);
}

/**
 * A `cap` rule of at most `limit` per civil month.
 *
 * `measure`, `per` and `zone` mean what they mean in {@link capPerDay}.
 *
 * @throws OpportuneError with code `invalid_argument` when a pooled cap is left in
 *   the participant's own zone.
 */
export function capPerMonth(count: Selector, limit: number, options: CapOptions = {}): CapRule {
  return cap("month", count, limit, "cap_per_month", "capPerMonth", options);
}

function cap(
  period: CapPeriod,
  count: Selector,
  limit: number,
  defaultId: string,
  call: string,
  options: CapOptions & { weekStarts?: string },
): CapRule {
  const {
    per = "each",
    zone = "participant",
    measure = "count",
    weekStarts,
    ruleId,
    who,
    when,
  } = optionsOf(options, call);
  const id = ruleId ?? defaultId;
  if (per === "pooled" && zone === "participant") {
    throw rejection(
      "invalid_argument",
      `\`${reason(id)}\` pools its cap, so \`zone\` must be \`event\` or an IANA name — the zone ` +
        "the cap is counted in — not the default `participant`: a pooled cap counts the " +
        "participants together, and each one's own zone would count the same event " +
        "against a different day",
    );
  }
  return scoped<CapRule>(
    {
      id,
      kind: "cap",
      params: { count, limit, measure, per, period, zone, week_starts: weekStarts ?? "mon" },
    },
    who,
    when,
  );
}

/**
 * A `status_policy` rule: entries tried in order to decide which blocks block.
 *
 * Entries are evaluated in the order given across applicable policies, so a narrower
 * policy belongs before a broader one.
 *
 * ```ts
 * statusPolicy([{ status: "tentative", effect: "ignore" }], { who: { kind: "person" } });
 * ```
 *
 * @throws OpportuneError with code `invalid_argument` when `entries` is not a list of
 *   entry objects, or `invalid_type` when `options` is not an object.
 */
export function statusPolicy(
  entries: readonly StatusPolicyEntry[],
  options: ScopedOptions = {},
): StatusPolicyRule {
  if (typeof entries === "string" || asPlainRecord(entries) !== undefined) {
    throw rejection(
      "invalid_argument",
      `\`entries\` is a list of status-policy entries, not a value of type ${typeName(entries)}`,
    );
  }
  let stated: readonly unknown[];
  try {
    stated = [...(entries as Iterable<unknown>)];
  } catch (cause) {
    throw rejection(
      "invalid_argument",
      `\`entries\` is a list of status-policy entries, not a value of type ${typeName(entries)}: ${reason(cause)}`,
      cause,
    );
  }
  const normalized: StatusPolicyEntry[] = stated.map((entry, index) => {
    if (asPlainRecord(entry) === undefined) {
      throw rejection(
        "invalid_argument",
        `\`entries[${index}]\` is a status-policy entry object, not a value of type ${typeName(entry)}`,
      );
    }
    return entry as StatusPolicyEntry;
  });
  const { ruleId = "status_policy", who, when } = optionsOf(options, "statusPolicy");
  return scoped<StatusPolicyRule>(
    { id: ruleId, kind: "status_policy", params: { entries: normalized } },
    who,
    when,
  );
}

/**
 * The rule with `selector` excepted from the bookings it applies to.
 *
 * This is how a broad tier is loosened for a narrow one. A rule whose `when` is
 * unconditional becomes `{ not: selector }`; one that is already scoped keeps its
 * scope and adds the exception, as `{ all: [existing, { not: selector }] }`.
 *
 * Excluding the exception from the broad rule is what makes a hierarchy work: a
 * `window` rule intersected with a second window leaves only the hours the two
 * share, so an exception written as another window on the narrow tier could only
 * narrow the booking further. A tier that should apply to everything *but* the
 * exception excludes it here instead.
 *
 * ```ts
 * const hours = unless(businessHours("mon-fri", "09:00-17:00"), { tag: "vip" });
 * ```
 *
 * The rule type is preserved, so a `CapRule` stays a `CapRule` and its `kind`
 * stays readable without a guard.
 *
 * A value that is not a rule object — a list of them, a primitive, or an object
 * carrying no `kind` — is rejected rather than spread into one: the object a spread
 * would build has no `kind`, so the engine would reject it at a rule the caller
 * never wrote.
 *
 * @throws OpportuneError with code `invalid_argument` when `rule` is not a rule
 *   object. The selector is the engine's to judge, with the pointer of the field
 *   it rejected.
 */
export function unless<Shape extends Rule>(
  rule: Shape,
  selector: Selector,
): Shape {
  const record = asPlainRecord(rule);
  if (record === undefined || typeof record["kind"] !== "string") {
    throw rejection(
      "invalid_argument",
      `\`rule\` is a rule the presets build, not a value of type ${typeName(rule)}`,
    );
  }
  const excluded: Selector = { not: selector };
  const when = rule.when;
  return {
    ...rule,
    when:
      when === undefined || when === null || when === true ? excluded : { all: [when, excluded] },
  };
}

/**
 * `who` as the wire reads it: an omitted or `null` selector is every host.
 *
 * A rule the presets build leaves `who` off, and the wire format reads the absent
 * field as `{ role: "host" }` — not as `true`. Treating the omission as "everything"
 * would let one narrowing call drop the default host clause, and the rule would then
 * reach attendees and resource rooms as well. `true` is a value a caller wrote, so it
 * keeps meaning "everything" here.
 */
function whoOf(who: Selector | null | undefined): Selector {
  return who === undefined || who === null ? { role: "host" } : who;
}

/**
 * The `who` that selects what `existing` selects and `selector` too.
 *
 * An unconditional `who` becomes `selector` alone, an `all` gains the selector as one
 * more clause — so chained calls stay flat rather than nesting an `all` in an `all` —
 * and any other selector object becomes `all([existing, selector])`. A value that is
 * not an object at all is carried through untouched, for the engine to reject at the
 * field the caller wrote; a value the wire cannot carry is refused here, because the
 * engine never sees it.
 *
 * @throws OpportuneError with code `invalid_type` when either value is a function or a
 *   symbol, which JSON drops rather than writes.
 */
function narrowed(existing: Selector, selector: Selector): Selector {
  // A function and a symbol are the malformed values the engine can never judge:
  // `JSON.stringify` drops the object member holding one, so a `who` the wire reads as
  // absent becomes its own `{ role: "host" }` default and the engine answers a rule the
  // caller never wrote, with nothing failing — the narrowing beside it silently gone.
  // Every other malformed value survives serialization, as itself or as the `null` a
  // dropped list member becomes, so the engine rejects it at the field the caller wrote.
  for (const [what, value] of [
    ["who", existing],
    ["selector", selector],
  ] as const) {
    if (typeof value === "function" || typeof value === "symbol") {
      throw rejection(
        "invalid_type",
        `\`${what}\` is a selector the wire can carry, not a value of type ${typeName(value)}`,
      );
    }
  }
  if (existing === true) {
    return selector;
  }
  // `in` throws on the primitives, so the shape is confirmed before it is read —
  // and a list is turned away with them, because a list of selectors is not itself
  // one. JavaScript callers are not type-checked, so `who` is whatever the caller's
  // variable held; leaving such a value unwrapped lets the engine's structured
  // validation reject it at the `who` field the caller wrote, instead of this
  // preset throwing a raw `TypeError` first.
  if (typeof existing !== "object" || existing === null || Array.isArray(existing)) {
    return existing;
  }
  if ("all" in existing && Array.isArray(existing.all)) {
    return { all: [...existing.all, selector] };
  }
  return { all: [existing, selector] };
}

/**
 * The rule with its `who` narrowed to the participants `selector` also selects.
 *
 * This is the participant-side twin of {@link unless}: it **intersects** with the
 * rule's existing `who` instead of replacing it, so a rule that already applies to
 * every host keeps applying to hosts and only to those hosts the selector picks out.
 * `ScopedOptions`' `who` replaces the default outright, so
 * `businessHours("mon-fri", "09:00-17:00", { who: { kind: "person" } })` reaches every
 * person, attendees included; `andWho` keeps the `Role.Host` clause beside the new one
 * and reaches human hosts alone.
 *
 * ```ts
 * const hours = andWho(businessHours("mon-fri", "09:00-17:00"), { kind: "person" });
 * ```
 *
 * The rule type is preserved, so a `CapRule` stays a `CapRule` and its `kind` stays
 * readable without a guard.
 *
 * A value that is not a rule object — a list of them, a primitive, or an object
 * carrying no `kind` — is rejected rather than spread into one, for the reason
 * {@link unless} rejects it.
 *
 * @throws OpportuneError with code `invalid_argument` when `rule` is not a rule object,
 *   and `invalid_type` when `rule.who` or `selector` is a value the wire cannot carry.
 *   Every other selector is the engine's to judge, with the pointer of the field it
 *   rejected.
 */
export function andWho<Shape extends Rule>(rule: Shape, selector: Selector): Shape {
  const record = asPlainRecord(rule);
  if (record === undefined || typeof record["kind"] !== "string") {
    throw rejection(
      "invalid_argument",
      `\`rule\` is a rule the presets build, not a value of type ${typeName(rule)}`,
    );
  }
  return { ...rule, who: narrowed(whoOf(rule.who), selector) };
}

/**
 * The rule with the participants `selector` selects excepted from its `who`.
 *
 * This is the participant-side twin of {@link unless}, and it is written in terms of
 * {@link andWho}: it narrows `who` by `not(selector)`, so the rule keeps applying to
 * every host but the excepted ones and never reaches an attendee, because the default
 * host clause is still there.
 *
 * ```ts
 * const cap = exceptWho(capPerDay({ tag: "interview" }, 2), { ids: ["ceo"] });
 * ```
 *
 * @throws OpportuneError with code `invalid_argument` when `rule` is not a rule object,
 *   and `invalid_type` when `rule.who` is a value the wire cannot carry: both are
 *   {@link andWho}'s, which this calls. A `selector` the wire cannot carry reaches the
 *   engine inside `{ not: … }`, which rejects it as no selector.
 */
export function exceptWho<Shape extends Rule>(rule: Shape, selector: Selector): Shape {
  return andWho(rule, { not: selector });
}

/**
 * Stated availability for an external attendee: the attendee, and the `window`
 * rule that makes exactly those UTC instants available to them.
 *
 * ```ts
 * const [ana, rule] = availableOnly("ana", [["2027-06-14T09:00Z", "2027-06-14T11:30Z"]]);
 * ```
 *
 * The spans are coalesced first and then written as dated ranges split at UTC
 * midnight, with a span ending at midnight written as ending `24:00`. A window
 * rather than blocks is deliberate: windows get no clearance and never trigger
 * rest, so stated availability cannot be mistaken for a meeting, and an excluded
 * start explains itself as `outside_window` citing this rule's ID.
 *
 * The rule's ID is `available_only:{id}` unless `ruleId` names another, so two
 * attendees' availability compose in one request without colliding on an ID.
 *
 * @throws OpportuneError with code `invalid_argument` when `spans` is not a list of
 *   pairs, a span is not a pair of RFC 3339 instants, is not aligned to a whole
 *   minute, ends at or before it starts, names an instant outside
 *   `1975-01-01T00:00Z..=2100-01-01T00:00Z` — the range every instant the engine
 *   holds is in — or needs more than ten years of dated entries, one per UTC day of
 *   every span.
 */
export function availableOnly(
  id: string,
  spans: readonly Span[],
  options: { ruleId?: string } = {},
): readonly [Participant, WindowRule] {
  if (typeof id !== "string") {
    throw rejection(
      "invalid_argument",
      `\`id\` is a participant ID such as \`ana\`, not a value of type ${typeName(id)}`,
    );
  }
  const { ruleId = `available_only:${id}` } = optionsOf(options, "availableOnly");
  if (typeof ruleId !== "string") {
    throw rejection(
      "invalid_argument",
      `\`ruleId\` is a rule ID, not a value of type ${typeName(ruleId)}`,
    );
  }
  const participant: Participant = { id, role: "attendee", zone: "UTC" };
  const rule: WindowRule = {
    id: ruleId,
    kind: "window",
    params: { zone: "UTC", dates: utcDates(spans) },
    who: { ids: [id] },
  };
  return [participant, rule] as const;
}

/**
 * The pairs `spans` states, or the rejection a value that is not a list of them
 * raises.
 *
 * JavaScript callers are not type-checked, so `spans` is whatever the caller's
 * variable held: `null` and `undefined` reach a `for…of` as a raw `TypeError`
 * rather than as a rejection. The spread is the check the Python preset's
 * `list(spans)` makes, and it takes any iterable for the same reason.
 */
function statedSpans(spans: readonly Span[]): readonly Span[] {
  let stated: readonly unknown[];
  try {
    stated = [...spans];
  } catch (cause) {
    throw rejection(
      "invalid_argument",
      "`spans` is a sequence of RFC 3339 instant pairs, " +
        `not a value of type ${typeName(spans)}: ${reason(cause)}`,
      cause,
    );
  }
  return stated.map(statedSpan);
}

/**
 * The two instants `value` states, or the rejection a value that is not a pair
 * raises.
 *
 * A span with a hole in it — a `null` entry, one instant, a number where an
 * instant belongs — would otherwise be read as `undefined` instants or have its
 * extra entries dropped.
 */
function statedSpan(value: unknown): Span {
  if (!Array.isArray(value)) {
    throw rejection(
      "invalid_argument",
      "a stated span is a pair of RFC 3339 instants, " +
        `not a value of type ${typeName(value)}`,
    );
  }
  if (value.length !== 2) {
    throw rejection(
      "invalid_argument",
      "a stated span is a pair of RFC 3339 instants, " +
        `not ${String(value.length)} entries`,
    );
  }
  const [first, last] = value as unknown[];
  if (typeof first !== "string" || typeof last !== "string") {
    throw rejection(
      "invalid_argument",
      "a stated span is a pair of RFC 3339 instants, " +
        `not \`${reason(first)}..${reason(last)}\``,
    );
  }
  return [first, last];
}

/**
 * The spans as dated wall ranges in UTC, coalesced and split at midnight.
 *
 * Every endpoint is a whole minute counted from the epoch, so the date of each
 * midnight boundary is integer arithmetic rather than a float timestamp; the ceiling
 * on the dated entries bounds the day loop whatever the caller states, and
 * {@link utcMinute} bounds every endpoint to a year this writes.
 */
function utcDates(spans: readonly Span[]): Record<string, string[]> {
  const ranges: [number, number][] = [];
  for (const span of statedSpans(spans)) {
    const start = utcMinute(span[0]);
    const end = utcMinute(span[1]);
    if (end <= start) {
      throw rejection(
        "invalid_argument",
        `the span \`${span[0]}..${span[1]}\` must end after it starts`,
      );
    }
    ranges.push([start, end]);
  }

  const merged: [number, number][] = [];
  for (const [start, end] of [...ranges].sort((left, right) => left[0] - right[0])) {
    const last = merged[merged.length - 1];
    if (last !== undefined && start <= last[1]) {
      last[1] = Math.max(last[1], end);
    } else {
      merged.push([start, end]);
    }
  }

  // The ceiling bounds what the preset writes, so it counts the whole stated
  // availability: spans that each fit the ceiling can still add up to millions of
  // entries written before the engine is ever called. Two disjoint spans on one
  // date write two entries under one key, so this is entries, not keys.
  const entries = merged.reduce(
    (total, [start, end]) =>
      total + Math.floor((end - 1) / DAY_MINUTES) - Math.floor(start / DAY_MINUTES) + 1,
    0,
  );
  if (entries > MAX_DATED_ENTRIES) {
    throw rejection(
      "invalid_argument",
      `the stated spans need ${entries} dated entries, more than the ` +
        `${MAX_DATED_ENTRIES} this preset accepts: one is written per UTC day of every span`,
    );
  }

  const dates: Record<string, string[]> = {};
  for (const [start, end] of merged) {
    let cursor = start;
    while (cursor < end) {
      const day = Math.floor(cursor / DAY_MINUTES);
      const dayStart = day * DAY_MINUTES;
      const stop = Math.min(end, dayStart + DAY_MINUTES);
      const dayDate = new Date(EPOCH_MS + day * DAY_MINUTES * MINUTE_MS);
      const date = dayDate.toISOString().slice(0, 10);
      const wallRanges = dates[date] ?? [];
      wallRanges.push(`${clock(cursor - dayStart)}-${clock(stop - dayStart)}`);
      dates[date] = wallRanges;
      cursor = dayStart + DAY_MINUTES;
    }
  }
  return dates;
}

/**
 * The whole minute an RFC 3339 instant names.
 *
 * Every endpoint is one of the engine's own instants: a value outside
 * `1975-01-01T00:00Z..=2100-01-01T00:00Z` has no dated window the engine could read,
 * so it is refused here with the supported range named directly rather than
 * being written as a window the engine never matches or misreported as an
 * invalid RFC 3339 instant.
 */
function utcMinute(instant: string): number {
  const invalid = () =>
    rejection(
      "invalid_argument",
      `\`${instant}\` is not an RFC 3339 instant with an explicit offset`,
    );
  const match = INSTANT.exec(instant);
  const date = match?.[1];
  if (date === undefined) {
    throw invalid();
  }
  // The seconds are read from the text rather than from the parsed instant:
  // `Date.parse` keeps no digit below a millisecond, so `09:00:00.000001` would
  // arrive whole-minute aligned and be written as `09:00` — an endpoint truncated
  // with no word, where the Python preset rejects it.
  const seconds = match?.[2];
  if (seconds !== undefined && !WHOLE_MINUTE.test(seconds)) {
    throw rejection("invalid_argument", `\`${instant}\` is not aligned to a whole minute`);
  }
  const at = Date.parse(instant);
  if (Number.isNaN(at)) {
    throw invalid();
  }
  // The range is checked before the civil date is read back, so an instant in a year
  // `Date.UTC` cannot build is refused for the reason that is true of it.
  if (at < SUPPORTED_FROM_MS || at > SUPPORTED_TO_MS) {
    throw rejection(
      "invalid_argument",
      `\`${instant}\` is outside the instants this package accepts, ${SUPPORTED_RANGE}: ` +
        "a stated availability is written as dated windows, and every instant the engine holds " +
        "is in that range",
    );
  }
  if (!isCivilDate(date)) {
    throw invalid();
  }
  return at / MINUTE_MS;
}

/**
 * Whether `date` is a `YYYY-MM-DD` civil date that exists.
 *
 * `Date.parse` rolls an impossible date forward (2027-02-30 reads as 2027-03-02),
 * so the date is read back out of the instant it produced: a date that survives the
 * round trip is the one that was written. Years 0000 to 0099 are outside the range
 * this reads, because `Date.UTC` maps them into the twentieth century.
 */
function isCivilDate(date: string): boolean {
  const [year, month, day] = date.split("-").map(Number);
  if (year === undefined || month === undefined || day === undefined) {
    return false;
  }
  const at = new Date(Date.UTC(year, month - 1, day));
  return (
    at.getUTCFullYear() === year && at.getUTCMonth() === month - 1 && at.getUTCDate() === day
  );
}

/** A minute of the day as a wire wall time. */
function clock(minuteOfDay: number): string {
  const hours = String(Math.floor(minuteOfDay / 60)).padStart(2, "0");
  const minutes = String(minuteOfDay % 60).padStart(2, "0");
  return `${hours}:${minutes}`;
}
