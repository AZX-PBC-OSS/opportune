/**
 * Google Calendar payload conversions.
 *
 * Call the provider like this, or the conversions see the wrong data:
 *
 * - Query `events.list` with `singleEvents=true`: an unexpanded series is reported
 *   as an `unavailable` outcome.
 * - Never set `maxAttendees`. Google may then omit the user's own attendee entry,
 *   which is what `self` status and `attendeesOmitted` are read from.
 * - Batch `freeBusy.query` within Google's per-call calendar limit.
 *
 * A per-calendar failure the provider reported (`calendars[id].errors`) is an
 * `unavailable` outcome, never an empty block list, because an empty list would
 * read as "completely free". Blocks from free/busy data carry no event key and are
 * never all-day.
 */

import { adapterOptions, binding, type Engine } from "../engine.js";
import { argument, payloadText, readDocument } from "../json.js";
import { translated } from "../wasm.js";
import type { CalendarOutcome } from "../generated/types.js";

/**
 * The marker names, re-exported from `../constants.js` so reading one loads no
 * WebAssembly.
 */
export { MARKER_EVENT_KEY, MARKER_TAGS } from "../constants.js";

/**
 * Converts a Google `events.list` response into blocks or `unavailable`.
 *
 * `participantZone` is the calendar owner's IANA zone, read for all-day events when
 * the response names no `timeZone` of its own. `options.engine` supplies the time
 * zone database and defaults to the shared engine, so a payload and the request it
 * feeds are read in one database.
 *
 * @param options the conversion's optional settings; absent means the defaults.
 *
 * @throws OpportuneError when the participant zone is blank or holds no UTF-8
 *   spelling, when `options` is not an object, or when the payload has no JSON or
 *   UTF-8 spelling. A payload this adapter cannot read is an `unavailable`
 *   outcome, never an error: an errored calendar must not read as a free one.
 */
export function eventsListToBlocks(
  payload: string | object,
  participantZone: string,
  options: { engine?: Engine } = {},
): CalendarOutcome {
  const payloadJson = payloadText(payload);
  const zone = argument(participantZone, "the participant zone");
  const { engine } = adapterOptions(options, "eventsListToBlocks");
  const answer = translated(() =>
    binding(engine).google_events_list_to_blocks_json(payloadJson, zone),
  );
  return readDocument<CalendarOutcome>(answer);
}

/**
 * Converts a Google `freeBusy.query` response into outcomes keyed by calendar ID.
 *
 * These ranges carry their own UTC offsets and name no zone, so the conversion
 * reads no database: `engine` only names which engine answers.
 *
 * @param options.engine the engine that answers, defaulting to the shared engine.
 *
 * @throws OpportuneError with code `payload_invalid` when the document is not a
 *   `freeBusy.query` response this adapter can read, `invalid_type` when `options`
 *   is not an object, and `invalid_json` when the payload has no JSON or UTF-8
 *   spelling. A calendar the provider reported an error for is an `unavailable`
 *   outcome inside the object; the healthy calendars are still converted.
 */
export function freeBusyToBlocks(
  payload: string | object,
  options: { engine?: Engine } = {},
): Record<string, CalendarOutcome> {
  const payloadJson = payloadText(payload);
  const { engine } = adapterOptions(options, "freeBusyToBlocks");
  const answer = translated(() => binding(engine).google_free_busy_to_blocks_json(payloadJson));
  return readDocument<Record<string, CalendarOutcome>>(answer);
}
