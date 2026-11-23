/**
 * The values the wire states as open sets: the strings a document or an answer
 * carries that a later release may add to.
 *
 * Each is an "open union": the values this package writes today, followed by
 * `(string & {})`. A caller that switches on one gets completion for every literal
 * and a compile error for a case it forgot, while a value from a newer engine — or a
 * provider's own status — is still assignable, so an older package keeps reading a
 * newer document. A closed union would make every caller's exhaustive check a claim
 * about the future.
 *
 * Nothing here imports a value or a module that runs — `Status` is a generated type,
 * which erases — so `opportune/presets` stays pure TypeScript.
 */

import type { Status } from "./generated/types.js";

/**
 * A stable code a rejection carries, in an `OpportuneError`'s `code`.
 *
 * The literals are the codes this package and the wire's shape reader raise: a
 * document this binding could not write, an argument of the wrong type, and the
 * fields the wire layer refuses. The engine's own domain codes —
 * `window_ranges_overlap`, `weekday_invalid`, `requirement_unfillable` and the rest
 * of its diagnostic vocabulary — arrive through the open tail, as does any code a
 * later release adds, so a handler that names today's codes still receives
 * tomorrow's.
 *
 * Match on the code, never on the message: the text may be reworded.
 */
export type OpportuneErrorCode =
  | "argument_not_utf8"
  | "duplicate_field"
  | "engine_disposed"
  | "invalid_argument"
  | "invalid_json"
  | "invalid_length"
  | "invalid_limits"
  | "invalid_request"
  | "invalid_shape"
  | "invalid_type"
  | "invalid_value"
  | "missing_field"
  | "payload_invalid"
  | "serialization_failed"
  | "tzdb_bundled_unavailable"
  | "unknown_field"
  | "unknown_variant"
  | "unsupported_runtime"
  | "unsupported_schema_version"
  | "wasm_error"
  | "wasm_unavailable"
  | (string & {});

/**
 * How a block on a participant's calendar affects availability.
 *
 * A block's `status` is read as an open string by the wire layer — a provider may
 * report a value this crate has never seen, and an unrecognised one reads as `busy`,
 * the safe reading — so this union completes the three the engine interprets and
 * still accepts the rest. {@link isKnownStatus} is the runtime half: it tells a
 * caller whether the value is one the engine gave a meaning to.
 */
export type BlockStatus = Status | (string & {});

/**
 * Something a caller should know about an offer or a slot check that does not make
 * it invalid.
 *
 * `calendar_unknown` reports that an assigned host has no calendar, so their
 * availability is unknown and they are treated as free within their rules. The open
 * tail keeps a flag from a later release assignable.
 */
export type OfferFlag = "calendar_unknown" | (string & {});

/**
 * The statuses the engine interprets, keyed by the generated union rather than listed
 * beside it: `Record<Status, true>` is a compile error here when the schema grows a
 * status this table lacks, and when it drops one this table still names. A list typed
 * `readonly Status[]` pins only that each name is a status, so the fourth variant
 * would leave {@link isKnownStatus} answering `false` for one the engine interprets
 * with every gate green.
 */
const KNOWN_STATUSES = { free: true, tentative: true, busy: true } satisfies Record<Status, true>;

/**
 * Whether `value` is a status the engine interprets.
 *
 * A caller that needs to act on `free` in particular cannot switch on a value the
 * engine would have read as `busy`: this narrows the open {@link BlockStatus} to the
 * three the engine gives a meaning to, and anything else is `busy`'s safe reading.
 *
 * ```ts
 * if (isKnownStatus(block.status)) {
 *   console.log(block.status);   // `free`, `tentative` or `busy`
 * }
 * ```
 */
export function isKnownStatus(value: unknown): value is Status {
  return typeof value === "string" && Object.hasOwn(KNOWN_STATUSES, value);
}

