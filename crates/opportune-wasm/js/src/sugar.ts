/**
 * Single-part sugar: shortcuts that produce the general types.
 *
 * Nothing here is a parallel type. {@link singleGoal} builds a goal with one part,
 * {@link person} and {@link atLeast} build the requirement terms the engine matches
 * on, and the `offer*` accessors read a returned offer and rewrite it as the
 * proposal `checkSlot` takes. The one-person, one-meeting case is about ten lines.
 *
 * The `offer*` accessors read a document the engine wrote, so what reaches them is
 * an offer in a JavaScript caller's hands rather than a type-checked one: a lookup
 * that missed and answered `null`, an offer read from a queue and never parsed. Each
 * answers a shape it cannot read — an offer that is not an object, `events` or `flags`
 * that is not a list, an entry of `events` that is not an event, an event whose own
 * list is missing — with an {@link OpportuneError} naming the field at fault, so a
 * handler for that class catches every failure of this package, never a bare
 * `TypeError` from indexing `null`.
 *
 * What lies *inside* a well-shaped offer is otherwise the module's own guarantee, as
 * it is for every document `readDocument` accepts: the accessors do not re-validate an
 * offer the engine wrote. The two instants are the exception, because a function
 * declared to return a `string` that answered `undefined` would corrupt a caller's
 * booking silently rather than fail.
 */

import { rejection } from "./errors.js";
import { asRecord, optionsOf, typeName } from "./json.js";
import type {
  AtLeastTerm,
  Goal,
  Grid,
  Offer,
  OfferEvent,
  PersonTerm,
  Proposal,
  Requirement,
  ReserveSpan,
  Selector,
  Span,
  Travel,
} from "./generated/types.js";

/**
 * What {@link singleGoal} accepts: the goal's own fields and the part's.
 *
 * Every optional field an `undefined` or a `null` leaves is the engine's own
 * default, the reading `Engine`'s `limits` argument takes: a caller holding a
 * variable that may be nothing writes the value it holds, and a `null` means "the
 * default" rather than a field the engine would refuse.
 */
export interface SingleGoalOptions {
  /** The event zone. */
  zone: string;
  /** The range of starts to consider. */
  searchWindow: Span;
  /** The zone offers are grouped by date in, or absent for the event zone. */
  displayZone?: string | null;
  /** The grid candidate starts are drawn from, or absent for the default grid. */
  grid?: Grid | null;
  /** Caller-supplied candidate starts, instead of a grid: one instant or a list. */
  candidates?: string | readonly string[] | null;
  /** The part's ID, or absent for `main`. */
  partId?: string | null;
  /** What is being booked: one tag or a list of tags. */
  tags?: string | readonly string[] | null;
  /** Travel the new booking needs, or absent for none. */
  travel?: Travel | null;
}

/**
 * A goal with one part: the shortcut for the common single-meeting case.
 *
 * It builds exactly what a goal with one part builds, so nothing is lost by using
 * it. The part takes the engine's default ID (`main`) and the offers are displayed
 * in the goal's own zone; `partId`, `tags` and `travel` reach the part, and
 * `displayZone`, `grid` and `candidates` the goal, so a real booking link or a
 * provider's candidate list needs no second construction step.
 *
 * @throws OpportuneError with code `invalid_type` when `options` is not an object,
 *   or `invalid_argument` when `tags` or `candidates` is neither a string nor a list.
 */
export function singleGoal(
  duration: string,
  requirement: Requirement,
  options: SingleGoalOptions,
): Goal {
  const { zone, searchWindow, displayZone, grid, candidates, partId, tags, travel } = optionsOf(
    options,
    "singleGoal",
  );
  return {
    ...(candidates == null ? {} : { candidates: oneOrMore(candidates, "candidates") }),
    ...(displayZone == null ? {} : { display_zone: displayZone }),
    ...(grid == null ? {} : { grid }),
    parts: [
      {
        duration,
        requirement,
        ...(partId == null ? {} : { id: partId }),
        ...(tags == null ? {} : { tags: oneOrMore(tags, "tags") }),
        ...(travel == null ? {} : { travel }),
      },
    ],
    search_window: searchWindow,
    zone,
  };
}

/** `stated` as the list of strings it spells, rejecting mappings or non-lists. */
function oneOrMore(stated: string | readonly string[], where: string): string[] {
  if (typeof stated === "string") {
    return [stated];
  }
  if (Array.isArray(stated)) {
    return [...stated];
  }
  throw rejection(
    "invalid_argument",
    `\`${where}\` is one value or a list of them, not a value of type ${typeName(stated)}`,
  );
}

/** A requirement term naming one participant. */
export function person(id: string): PersonTerm {
  return { person: id };
}

/** A requirement term satisfied by any `n` participants `of` selects. */
export function atLeast(n: number, of: Selector): AtLeastTerm {
  return { at_least: n, of };
}

/**
 * The earliest start the offer books, as an RFC 3339 instant.
 *
 * @throws OpportuneError with code `invalid_argument` when the offer books no event,
 *   or its first event names no start, which no engine answer does.
 */
export function offerStart(offer: Offer): string {
  return instant(offer, "offerStart", "start", 0);
}

/**
 * The latest end the offer books, as an RFC 3339 instant.
 *
 * @throws OpportuneError with code `invalid_argument` when the offer books no event,
 *   or its last event names no end, which no engine answer does.
 */
export function offerEnd(offer: Offer): string {
  return instant(offer, "offerEnd", "end", -1);
}

/**
 * The participants assigned to the offer, in term order.
 *
 * @throws OpportuneError with code `invalid_type` when `offer` is not a valid offer
 *   object or an event's `assignment` is not a list.
 */
export function offerAssignedHosts(offer: Offer): string[] {
  return events(offer, "offerAssignedHosts").flatMap((event, index) =>
    list(event.assignment, "offerAssignedHosts", "assignment", index).flat(),
  );
}

/**
 * Whether the offer rests on a host whose calendar was never supplied.
 *
 * @throws OpportuneError with code `invalid_type` when `offer` is not a valid offer
 *   object or its `flags` is not a list.
 */
export function offerCalendarUnknown(offer: Offer): boolean {
  return flags(offer, "offerCalendarUnknown").includes("calendar_unknown");
}

/**
 * The spans a confirmed offer would hold on its participants' calendars.
 *
 * Travel is part of the booking the caller must write, so an offer's reserve is
 * what to put on each participant's calendar beside the event itself; the spans
 * come in the order the offer's events do.
 *
 * @throws OpportuneError with code `invalid_type` when `offer` is not a valid offer
 *   object or an event's `reserve` is not a list.
 */
export function offerReserve(offer: Offer): ReserveSpan[] {
  return events(offer, "offerReserve").flatMap((event, index) =>
    list(event.reserve, "offerReserve", "reserve", index),
  );
}

/**
 * The offer as the proposal `checkSlot` confirms.
 *
 * The result is a document, not a verdict: a field the offer lacks is dropped when the
 * proposal is written, and the engine answers with the pointer of the field it needs.
 *
 * @throws OpportuneError with code `invalid_type` when `offer` is not a valid offer
 *   object or its `events` is not a list of event objects.
 */
export function offerToProposal(offer: Offer): Proposal {
  return {
    events: events(offer, "offerToProposal").map((event) => ({
      part: event.part,
      start: event.start,
      assignment: event.assignment,
    })),
  };
}

/**
 * The events `offer` books, or the rejection an offer that is not one raises.
 *
 * @param what the accessor reading the offer, named in the message.
 * @throws OpportuneError with code `invalid_type` when `offer` is not an object, its
 *   `events` is not a list, or an entry of that list is not an event — a hole a
 *   parsed document keeps where an event belongs, which reading a field off would
 *   report as a `TypeError` from this package's own code.
 */
function events(offer: Offer, what: string): OfferEvent[] {
  const held = offerRecord(offer, what)["events"];
  if (!Array.isArray(held)) {
    throw rejection(
      "invalid_type",
      `\`${what}\` reads an offer's \`events\` as a list, not \`${typeName(held)}\`: ` +
        "an offer is a document the engine wrote",
    );
  }
  return held.map((entry, index) => {
    if (asRecord(entry) === undefined) {
      throw rejection(
        "invalid_type",
        `\`${what}\` reads an offer's \`events\`, and entry ${index} is \`${typeName(entry)}\` ` +
          "rather than an event: an offer is a document the engine wrote",
      );
    }
    return entry as OfferEvent;
  });
}

/**
 * The flags `offer` carries, or the rejection an offer that is not one raises.
 *
 * @throws OpportuneError with code `invalid_type` when `offer` is not an object or
 *   its `flags` is not a list.
 */
function flags(offer: Offer, what: string): string[] {
  const held = offerRecord(offer, what)["flags"];
  if (!Array.isArray(held)) {
    throw rejection(
      "invalid_type",
      `\`${what}\` reads an offer's \`flags\` as a list, not \`${typeName(held)}\`: ` +
        "an offer is a document the engine wrote",
    );
  }
  return held as string[];
}

/**
 * `offer` as the record its fields are read from.
 *
 * @throws OpportuneError with code `invalid_type` when `offer` is not an object.
 */
function offerRecord(offer: Offer, what: string): Record<string, unknown> {
  const document = asRecord(offer);
  if (document === undefined) {
    throw rejection(
      "invalid_type",
      `\`${what}\` reads an offer, an object with \`events\` and \`flags\` lists, not \`${typeName(offer)}\``,
    );
  }
  return document;
}

/**
 * The instant the `field` of the event at `index` names.
 *
 * @param index which event, counted from the start, or from the end when negative.
 * @throws OpportuneError with code `invalid_argument` when the offer books no event at
 *   `index`, or that event names no instant: no engine answer produces either, and a
 *   `string` function that answered `undefined` would corrupt a caller's booking
 *   silently rather than fail.
 */
function instant(offer: Offer, what: string, field: "start" | "end", index: number): string {
  const booked = events(offer, what);
  const event = index < 0 ? booked.at(index) : booked[index];
  const value = event?.[field];
  if (typeof value !== "string") {
    const which = index < 0 ? "last" : "first";
    throw rejection(
      "invalid_argument",
      `\`${what}\` reads the ${which} event's \`${field}\`, and the offer books none there`,
    );
  }
  return value;
}

/**
 * `value` as the list `field` names, or the rejection an event that is not one raises.
 *
 * @throws OpportuneError with code `invalid_type` when `value` is not a list, naming
 *   the event's position and the field, which is where a hand-written or truncated
 *   offer goes wrong.
 */
function list<T>(value: readonly T[] | undefined, what: string, field: string, index: number): readonly T[] {
  if (!Array.isArray(value)) {
    throw rejection(
      "invalid_type",
      `\`${what}\` reads event ${index}'s \`${field}\` as a list, not \`${typeName(value)}\`: ` +
        "an offer is a document the engine wrote",
    );
  }
  return value;
}

