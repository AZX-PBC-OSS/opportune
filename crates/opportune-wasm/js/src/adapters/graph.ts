/**
 * Microsoft Graph / Entra Outlook payload conversions.
 *
 * Call the provider like this, or the conversions see the wrong data:
 *
 * - Read `calendarView`, not `/events`: recurrence expansion is the caller's job,
 *   and an unexpanded series is reported as an `unavailable` outcome.
 * - Send `Prefer: outlook.timezone="UTC"`. Graph `dateTimeTimeZone` values carry no
 *   offset, so a payload stamped in another zone is read in that zone's local time
 *   — an all-day event is read from its civil dates in the participant's zone
 *   whatever the payload stamps on it.
 * - `$expand` the marker properties with {@link EXTENDED_PROPERTY_EXPAND}, or the
 *   caller's own tags and event keys are absent.
 * - Chunk `getSchedule` to at most 20 schedules and 62 days per call.
 *
 * A per-calendar failure the provider reported (`scheduleInformation.error`) is an
 * `unavailable` outcome, never an empty block list, because an empty list would
 * read as "completely free".
 */

import { adapterOptions, binding, isEngine, type Engine } from "../engine.js";
import { rejection } from "../errors.js";
import { argument, asPlainRecord, documentText, payloadText, readDocument, typeName } from "../json.js";
import { translated } from "../wasm.js";
import type { CalendarOutcome, GraphWorkingHoursOutcome, Selector } from "../generated/types.js";

/**
 * The marker names and the Graph extended-property strings.
 *
 * They are re-exported from `../constants.js`, which imports nothing: a caller that
 * reads {@link EXTENDED_PROPERTY_EXPAND} to build one Graph request must not
 * instantiate the engine's 1.5 MB WebAssembly module to do it.
 */
export {
  EXTENDED_PROPERTY_EVENT_KEY_ID,
  EXTENDED_PROPERTY_EXPAND,
  EXTENDED_PROPERTY_NAMESPACE_GUID,
  EXTENDED_PROPERTY_TAGS_ID,
  MARKER_EVENT_KEY,
  MARKER_TAGS,
} from "../constants.js";

/**
 * Converts a Graph `calendarView` response into blocks or `unavailable`.
 *
 * `participantZone` is the mailbox owner's IANA zone, which all-day events are
 * resolved in. `options.categoryMap` maps a category name to a tag path — an unmapped
 * category becomes `category/<slug>`, and one with an empty slug is dropped — and
 * `options.engine` supplies the time zone database, defaulting to the shared engine so
 * a payload and the request it feeds are read in one database.
 *
 * @param options the conversion's optional settings; absent means the defaults.
 *
 * @throws OpportuneError when the participant zone is blank or holds no UTF-8
 *   spelling, when `options` is not an object or `categoryMap` is an `Engine` or
 *   holds no UTF-8 spelling, or when the payload has no JSON or UTF-8 spelling. A
 *   payload this adapter cannot read is an `unavailable` outcome, never an error:
 *   an errored mailbox must not read as a free one.
 */
export function calendarViewToBlocks(
  payload: string | object,
  participantZone: string,
  options: { categoryMap?: Readonly<Record<string, string>> | null; engine?: Engine } = {},
): CalendarOutcome {
  const payloadJson = payloadText(payload);
  const zone = argument(participantZone, "the participant zone");
  const { categoryMap, engine } = adapterOptions(options, "calendarViewToBlocks");
  const categoryMapJson = categoryMap == null ? null : categoryMapText(categoryMap);
  const answer = translated(() =>
    binding(engine).graph_calendar_view_to_blocks_json(payloadJson, zone, categoryMapJson),
  );
  return readDocument<CalendarOutcome>(answer);
}

/**
 * The category map as the JSON document the module reads.
 *
 * An `Engine` is an object too, and JavaScript writes one as `{}`: a caller that
 * hands the engine where the map goes would convert every event as if no category
 * were mapped, which is a silent answer for the wrong document. `{ engine }` is
 * where one belongs, so it is named here. The check is {@link isEngine} rather than
 * `instanceof`, so an `Engine` built by a second copy of the package — a monorepo
 * with two versions, a store that did not dedupe — is caught as one instead of
 * being written as an empty map.
 *
 * A `Map`, a `Set` or an instance of some other class is an object as well, and
 * `JSON.stringify` writes no members for any of them: a caller that handed one
 * reaches the engine as an empty map rather than as the mistake it is. Only a plain
 * mapping — an object literal, one read back from a document, one built with
 * `Object.create(null)` — is written; every other object is refused with its type
 * named.
 *
 * @throws OpportuneError with code `invalid_type` when `map` is an `Engine` or is
 *   not a plain mapping, and with the code {@link documentText} answers for a map
 *   with no JSON spelling.
 */
function categoryMapText(map: Readonly<Record<string, string>>): string {
  if (isEngine(map)) {
    throw rejection(
      "invalid_type",
      "`categoryMap` maps category names to tag paths, not an Engine: " +
        "pass the engine as `{ engine }` beside the map",
    );
  }
  if (asPlainRecord(map) === undefined) {
    throw rejection(
      "invalid_type",
      `\`categoryMap\` maps category names to tag paths, not a value of type ${typeName(map)}`,
    );
  }
  return documentText(map, "the category map");
}

/**
 * Converts a Graph `getSchedule` response into outcomes keyed by schedule ID.
 *
 * A schedule the provider reported an error for is an `unavailable` outcome; the
 * healthy schedules in the same batch are still converted. Blocks from free or busy
 * data carry no event key and are never all-day.
 *
 * @param options.engine the engine whose database resolves the payload's zone
 *   names, defaulting to the shared engine.
 *
 * @throws OpportuneError with code `payload_invalid` when the document is not a
 *   `getSchedule` response this adapter can read, `invalid_type` when `options` is
 *   not an object, and `invalid_json` when the payload has no JSON or UTF-8
 *   spelling.
 */
export function getScheduleToBlocks(
  payload: string | object,
  options: { engine?: Engine } = {},
): Record<string, CalendarOutcome> {
  const payloadJson = payloadText(payload);
  const { engine } = adapterOptions(options, "getScheduleToBlocks");
  const answer = translated(() =>
    binding(engine).graph_get_schedule_to_blocks_json(payloadJson),
  );
  return readDocument<Record<string, CalendarOutcome>>(answer);
}

/**
 * Converts one mailbox's Graph `workingHours` into a `window` rule, or reports why
 * it could not.
 *
 * A custom Graph time zone is an `unavailable` outcome rather than an exception, so
 * a caller falls back to its own window without catching one; the blocks converted
 * from the same schedule response are unaffected.
 *
 * @param options.ruleId the ID the resulting rule takes, defaulting to
 *   `working_hours`.
 * @param options.who the participants the rule constrains, defaulting to every host.
 * @param options.when the bookings the rule applies to, defaulting to all of them.
 * @param options.engine the engine whose database resolves the payload's zone
 *   names, defaulting to the shared engine.
 *
 * @throws OpportuneError with code `argument_not_utf8` when the rule ID holds an
 *   unpaired surrogate, `invalid_type` when `options` or the rule ID is not the
 *   type the signature names, and `invalid_json` when the payload has no JSON or
 *   UTF-8 spelling.
 */
export function workingHoursToRule(
  payload: string | object,
  options: { ruleId?: string; who?: Selector; when?: Selector; engine?: Engine } = {},
): GraphWorkingHoursOutcome {
  const payloadJson = payloadText(payload);
  const { ruleId: named, who, when, engine } = adapterOptions(options, "workingHoursToRule");
  const ruleId = argument(named ?? "working_hours", "the rule ID");
  const answer = translated(() =>
    binding(engine).graph_working_hours_to_rule_json(payloadJson, ruleId),
  );
  const outcome = readDocument<GraphWorkingHoursOutcome>(answer);
  if (outcome.status !== "available" || (who === undefined && when === undefined)) {
    return outcome;
  }
  // A field the caller left absent keeps the rule's own default, so only the
  // selectors that were named are replaced.
  return {
    ...outcome,
    rule: {
      ...outcome.rule,
      ...(who === undefined ? {} : { who }),
      ...(when === undefined ? {} : { when }),
    },
  };
}
