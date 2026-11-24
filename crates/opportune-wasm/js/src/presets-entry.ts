/**
 * `opportune/presets`: the rule, goal and offer builders, the one error a caller
 * catches, the public projections, the package's own constants, and the generated
 * types.
 *
 * Every export here is pure TypeScript — builders that are functions of their
 * arguments, the class those builders raise, projections that read an answer they are
 * handed, constants stated as strings, and the generated types, which erase. Nothing
 * in this module graph imports the compiled module or `node:module`, so a browser
 * bundle or a client component can import it and call it with no
 * WebAssembly instantiation and no Node built-in. The engine itself is not here: it
 * runs on Node, Bun, Cloudflare Workers, browsers and Web Workers, and a browser or
 * Web Worker reaches it through `opportune/web` after `init()`.
 *
 * The error class and the document types are here for the same reason the builders
 * are: a bundle that builds a request, or reads one back from its own storage, needs
 * to recognise the failure a builder raises and to name the document it holds — and
 * importing `opportune` for either would resolve a module that is not initialised. The
 * constants are here because stamping a tag or asking a provider for an extended
 * property is a string a client-side or Edge bundle reads, with no engine behind it.
 *
 * ```ts
 * import { businessHours, capPerDay, MARKER_TAGS, person, singleGoal } from "opportune/presets";
 *
 * const rules = [businessHours("mon-fri", "09:00-17:00"), capPerDay({ tag: "interview" }, 2)];
 * ```
 */

export { OpportuneError, httpStatusForError } from "./errors.js";
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
export {
  publicResponse,
  publicSlotCheck,
  type PublicOffer,
  type PublicOfferEvent,
  type PublicResponse,
  type PublicSlotCheck,
} from "./public.js";
export {
  isKnownStatus,
  type BlockStatus,
  type OfferFlag,
  type OpportuneErrorCode,
} from "./vocabulary.js";

/**
 * The engine's version and the provider marker names.
 *
 * A bundle that stamps its own tags or event keys, or asks Graph for the extended
 * properties that carry them, needs these strings and nothing else: they are stated
 * in `constants.ts`, which imports nothing, so reading one here instantiates no
 * WebAssembly and resolves no `node:` module.
 */
export {
  EXTENDED_PROPERTY_EVENT_KEY_ID,
  EXTENDED_PROPERTY_EXPAND,
  EXTENDED_PROPERTY_NAMESPACE_GUID,
  EXTENDED_PROPERTY_TAGS_ID,
  MARKER_EVENT_KEY,
  MARKER_TAGS,
  VERSION,
} from "./constants.js";

export type * from "./generated/types.js";
