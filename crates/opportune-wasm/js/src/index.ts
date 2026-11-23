/**
 * The typed public API over the `opportune` engine, for Node and Bun.
 *
 * ```ts
 * import { businessHours, findAvailability, person, singleGoal } from "opportune";
 *
 * const response = findAvailability({
 *   now: "2027-06-14T07:00Z",
 *   goal: singleGoal("30m", { all: [person("rich")] }, {
 *     zone: "Europe/London",
 *     searchWindow: ["2027-06-13T23:00Z", "2027-06-18T23:00Z"],
 *   }),
 *   participants: [{ id: "rich", zone: "Europe/London", calendar: [] }],
 *   rules: [businessHours()],
 * });
 * ```
 *
 * Build an {@link Engine} and reuse it when you need custom limits; the free
 * functions answer from one shared engine, built on first use. Every document the
 * engine reads and writes is a generated type re-exported here from the exported
 * JSON Schema, and every failure is an {@link OpportuneError}.
 *
 * Importing this module loads no WebAssembly: the module is loaded by the first call
 * that needs it, so a server that imports the package for a preset builder never
 * instantiates one. The version and marker constants it exports are stated in
 * `constants.ts`, which imports nothing, and the suite pins them against the module
 * the package was built beside.
 *
 * The provider conversions live in `opportune/adapters/graph` and
 * `opportune/adapters/google`. The preset builders alone, for a bundle that must not
 * reach for a module at all, are `opportune/presets`.
 */

export { OpportuneError, httpStatusForError } from "./errors.js";
export {
  Engine,
  checkSlot,
  checkSlotJson,
  defaultLimits,
  findAvailability,
  findAvailabilityJson,
  limitsWith,
  requiredRanges,
  requiredRangesJson,
  upgradeCalendar,
  upgradeGoal,
  upgradeRule,
  validate,
  validateJson,
} from "./engine.js";
export {
  andWho,
  availableOnly,
  buffers,
  businessHours,
  capPerDay,
  capPerMonth,
  capPerWeek,
  datedHours,
  exceptWho,
  horizon,
  minNotice,
  rest,
  statusPolicy,
  unless,
  weeklyHours,
  type CapOptions,
  type HorizonMax,
  type ScopedOptions,
  type WeeklyHoursOptions,
} from "./presets.js";
export {
  atLeast,
  offerAssignedHosts,
  offerCalendarUnknown,
  offerEnd,
  offerReserve,
  offerStart,
  offerToProposal,
  person,
  singleGoal,
  type SingleGoalOptions,
} from "./sugar.js";

/**
 * The offers a public booking route serves, without the operator's own detail.
 *
 * ```ts
 * app.post("/offers", (request) => publicResponse(findAvailability(request)));
 * ```
 */
export {
  publicResponse,
  publicSlotCheck,
  type PublicOffer,
  type PublicOfferEvent,
  type PublicResponse,
  type PublicSlotCheck,
} from "./public.js";

/**
 * The open vocabularies the wire states: a block's status, an offer's flags and the
 * codes a rejection carries.
 */
export {
  isKnownStatus,
  type BlockStatus,
  type OfferFlag,
  type OpportuneErrorCode,
} from "./vocabulary.js";

export type * from "./generated/types.js";

/**
 * The engine's version and the provider marker names.
 *
 * Defined in `constants.ts` without loading WebAssembly so importing the
 * package for metadata or marker constants never instantiates the module. The
 * test suite verifies each constant matches the compiled WASM module and
 * package manifest.
 */
export {
  MARKER_EVENT_KEY,
  MARKER_TAGS,
  VERSION,
} from "./constants.js";
