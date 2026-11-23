/**
 * The projection a public booking route serves: the bookable times, and nothing
 * about how they were found.
 *
 * A {@link Response} answers an operator. Beside the offers it carries the request's
 * warnings, every candidate start that was not offered and why, and, for each offer,
 * who could fill every term and who was chosen — the host's calendars, the policies
 * that shaped them and the other attendees. A public booking link shows a visitor the
 * times it can book, so a route serving one returns a projection of the answer
 * instead of the answer itself.
 *
 * The projection is opt-in and costs an operator route nothing: `Response` and
 * `SlotCheck` keep every field, and a route that serves an operator returns them
 * unchanged.
 *
 * ```ts
 * app.post("/offers", (request) => publicResponse(engine.findAvailability(request)));
 * ```
 *
 * Every field the projection keeps is one a visitor acts on — the offers, the dates
 * they fall on, whether the list was cut short, and the booking's validity — so a
 * visitor reads the same document shape an operator does, minus what is not theirs.
 * The Python package's `public_response`/`public_slot_check` draw the same line, so a
 * service that serves both surfaces answers the same document.
 *
 * The helpers read a document the engine wrote, so what reaches them is an answer in
 * a JavaScript caller's hands rather than a type-checked one: a route body parsed
 * back from a queue, a lookup that missed and answered `null`. Every field the
 * projection copies is checked as it is read, so such a document leaves as this
 * package's {@link OpportuneError} rather than as a bare `TypeError` from indexing it
 * — and never as an `undefined` written into the public document where a field
 * belongs. What lies inside a well-shaped value is otherwise the module's own
 * guarantee, as it is for every document the engine wrote.
 *
 * Every export here is pure TypeScript: the projections read the answer they are
 * given and import nothing that runs, so `opportune/presets` can carry them.
 */

import { rejection } from "./errors.js";
import { asRecord, typeName } from "./json.js";
import type { DateCount, Meta, Response, SlotCheck } from "./generated/types.js";
import type { OfferFlag } from "./vocabulary.js";

/** One goal part's time in an offer, without who fills it. */
export interface PublicOfferEvent {
  /** The booking's end. */
  end: string;
  /** The goal part's ID. */
  part: string;
  /** The booking's start. */
  start: string;
}

/** A bookable time: its parts, and what the visitor should know about it. */
export interface PublicOffer {
  /** One event per goal part. */
  events: PublicOfferEvent[];
  /** What the visitor should know about the offer, such as a host with no calendar. */
  flags: OfferFlag[];
}

/** The offers a public booking link serves, without the operator's own detail. */
export interface PublicResponse {
  /** Every civil date the search window touches, with its offer count. */
  dates: DateCount[];
  /** The versions the answer was produced from. */
  meta: Meta;
  /** The bookable times, in start order. */
  offers: PublicOffer[];
  /**
   * Whether the offer limit cut the list short, so a visitor knows to narrow the
   * search rather than read the list as every time available.
   */
  truncated: boolean;
}

/** The judgement of one proposed booking, without why it failed. */
export interface PublicSlotCheck {
  /** What the visitor should know that does not make it invalid. */
  flags: OfferFlag[];
  /** The versions the answer was produced from. */
  meta: Meta;
  /** Whether the booking can be made as proposed. */
  valid: boolean;
}

/**
 * `response` with the operator-only fields left behind.
 *
 * The warnings the request produced, the excluded starts and their reasons, and each
 * offer's eligible and assigned hosts are the operator's view of who and what shaped
 * the answer; a public booking link serves the offers themselves.
 *
 * @throws OpportuneError with code `invalid_type` when `response` is not the document
 *   `findAvailability` answers with: no engine answer writes a field of another type,
 *   and one written as `undefined` would corrupt a visitor's booking silently rather
 *   than fail.
 */
export function publicResponse(response: Response): PublicResponse {
  const document = documentOf(
    response,
    "publicResponse",
    "a response, an object with `dates`, `meta`, `offers` and `truncated`",
  );
  return {
    dates: field(
      document,
      "dates",
      isListOf<unknown>,
      "a response's `dates` as a list",
      "publicResponse",
    ).map((entry, index) => publicDateCount(entry, index)),
    meta: publicMeta(
      field(
        document,
        "meta",
        isObjectOf<Record<string, unknown>>,
        "a response's `meta` as an object",
        "publicResponse",
      ),
      "publicResponse",
    ),
    offers: field(
      document,
      "offers",
      isListOf<unknown>,
      "a response's `offers` as a list",
      "publicResponse",
    ).map((offer, index) => publicOffer(offer, index)),
    truncated: field(
      document,
      "truncated",
      isBoolean,
      "a response's `truncated` as a boolean",
      "publicResponse",
    ),
  };
}

/**
 * `check` with the operator-only fields left behind.
 *
 * Why a booking fails names the rules, the other participants' events and the reserve
 * the operator's own booking needs, so a public confirmation serves the verdict
 * itself: whether the booking can be made, and the flags that do not make it invalid.
 *
 * @throws OpportuneError with code `invalid_type` when `check` is not the document
 *   `checkSlot` answers with.
 */
export function publicSlotCheck(check: SlotCheck): PublicSlotCheck {
  const document = documentOf(
    check,
    "publicSlotCheck",
    "a slot check, an object with `flags`, `meta` and `valid`",
  );
  return {
    flags: [
      ...field(
        document,
        "flags",
        isListOf<OfferFlag>,
        "a slot check's `flags` as a list",
        "publicSlotCheck",
      ),
    ],
    meta: publicMeta(
      field(
        document,
        "meta",
        isObjectOf<Record<string, unknown>>,
        "a slot check's `meta` as an object",
        "publicSlotCheck",
      ),
      "publicSlotCheck",
    ),
    valid: field(
      document,
      "valid",
      isBoolean,
      "a slot check's `valid` as a boolean",
      "publicSlotCheck",
    ),
  };
}

/**
 * The public fields of `meta`, each read and checked as it is copied.
 *
 * A `Meta` is three required fields, so the projection reads all three rather than
 * copying the members it finds: a `Response` read back from a cache or a queue
 * whose `meta` lost a key would otherwise leave as a document typed `Meta` with an
 * `undefined` where the type promises a string, and a route that prints
 * `meta.tzdb` would serve `undefined` to a visitor instead of failing where the
 * projection was applied. Extra members are dropped, so neither a later field of
 * the operator's answer nor a caller's own leaks into the public document.
 */
function publicMeta(meta: Record<string, unknown>, call: string): Meta {
  return {
    engine: field(meta, "engine", isText, "`meta.engine` as a string", call),
    schema: field(meta, "schema", isInteger, "`meta.schema` as a whole number", call),
    tzdb: field(meta, "tzdb", isText, "`meta.tzdb` as a string", call),
  };
}

/** Copies the public fields of one `DateCount` entry. */
function publicDateCount(value: unknown, index: number): DateCount {
  const entry = documentOf(
    value,
    "publicResponse",
    `date entry ${index}, an object with \`date\` and \`offers\``,
  );
  return {
    date: field(entry, "date", isText, `date entry ${index}'s \`date\` as a string`, "publicResponse"),
    offers: field(
      entry,
      "offers",
      isNumber,
      `date entry ${index}'s \`offers\` as a number`,
      "publicResponse",
    ),
  };
}

/** The offer `value` is, or the rejection an offer that is not one raises. */
function publicOffer(value: unknown, index: number): PublicOffer {
  const where = `offer ${index}`;
  const call = "publicResponse";
  const offer = documentOf(value, call, `${where}, an object with \`events\` and \`flags\` lists`);
  return {
    events: field(
      offer,
      "events",
      isListOf<unknown>,
      `${where}'s \`events\` as a list`,
      call,
    ).map((event, position) => publicEvent(event, where, position)),
    flags: [...field(offer, "flags", isListOf<OfferFlag>, `${where}'s \`flags\` as a list`, call)],
  };
}

/** The event `value` is, or the rejection an event that is not one raises. */
function publicEvent(value: unknown, offer: string, position: number): PublicOfferEvent {
  const where = `event ${position} of ${offer}`;
  const call = "publicResponse";
  const event = documentOf(value, call, `${where}, an object with \`part\`, \`start\` and \`end\``);
  return {
    end: field(event, "end", isText, `${where}'s \`end\` as an instant`, call),
    part: field(event, "part", isText, `${where}'s \`part\` as an ID`, call),
    start: field(event, "start", isText, `${where}'s \`start\` as an instant`, call),
  };
}

/**
 * The field `name` of `document`, or the rejection a field that is not `holds` raises.
 *
 * Every field a projection copies is read through here, so a document missing one, or
 * carrying one of another type, is one rejection shape naming what was read and what
 * it was — rather than an `undefined` or a foreign value written into the public
 * document. `expected` is the phrase the message uses for it.
 *
 * `holds` checks the shape, not the fields inside: what a list's entries or an
 * object's members hold is the engine's own guarantee, so a caller reads a generated
 * document type there, as it does from `readDocument` for every engine answer.
 */
function field<Held>(
  document: Record<string, unknown>,
  name: string,
  holds: (value: unknown) => value is Held,
  expected: string,
  call: string,
): Held {
  const value = document[name];
  if (!holds(value)) {
    throw rejection(
      "invalid_type",
      `\`${call}\` reads ${expected}, not \`${typeName(value)}\`: ` +
        "it is a field of a document the engine wrote",
    );
  }
  return value;
}

/** `value` as the document `what` names, or the rejection one that is not raises. */
function documentOf(
  value: unknown,
  call: string,
  what: string,
): Record<string, unknown> {
  const document = asRecord(value);
  if (document === undefined) {
    throw rejection(
      "invalid_type",
      `\`${call}\` reads ${what}, not \`${typeName(value)}\`: ` +
        "it is a document the engine wrote",
    );
  }
  return document;
}

/** Whether `value` is a list; its entries are the engine's own. */
function isListOf<Entry>(value: unknown): value is Entry[] {
  return Array.isArray(value);
}

/** Whether `value` is an object; its members are the engine's own. */
function isObjectOf<Shape>(value: unknown): value is Shape {
  return asRecord(value) !== undefined;
}

/** Whether `value` is a boolean. */
function isBoolean(value: unknown): value is boolean {
  return typeof value === "boolean";
}

/** Whether `value` is a number. */
function isNumber(value: unknown): value is number {
  return typeof value === "number";
}

/**
 * Whether `value` is a whole, finite number.
 *
 * A schema version is a count, so `1.5`, `NaN` and `Infinity` are not versions: a
 * document read back from a JSON column that lost the field, or a queue that wrote
 * a float, has to be the rejection rather than a `meta.schema` a visitor's route
 * prints as `1.5`.
 */
function isInteger(value: unknown): value is number {
  return typeof value === "number" && Number.isInteger(value);
}

/** Whether `value` is text. */
function isText(value: unknown): value is string {
  return typeof value === "string";
}
