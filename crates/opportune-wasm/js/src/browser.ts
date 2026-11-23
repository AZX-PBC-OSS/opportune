/**
 * What a browser, an Edge runtime or a browser-targeting bundler gets for
 * `opportune`, `opportune/adapters/graph` and `opportune/adapters/google`.
 *
 * The engine is a `wasm-pack --target nodejs` build: its loader instantiates the
 * module through `node:module` and the compiled glue reads the `.wasm` beside it
 * with Node's filesystem. Neither exists in a browser, so resolving this package for
 * one there would fail later, at the first call, with a bare module-resolution or
 * `require` error from inside the loader — several frames away from the import that
 * caused it, and only once a user's request had reached it.
 *
 * This module is that failure, moved to the import and made legible: the `browser`
 * condition of the manifest resolves here instead, and importing it raises one
 * structured {@link OpportuneError} that names the runtimes the package supports and
 * what a bundle can use instead. A build that resolves this module fails at build or
 * import time, unmistakably, rather than shipping a bundle that throws for its users.
 *
 * The engine-backed subpaths do not share one export list, so this module declares
 * every name all three of them export. A bundler resolves a named import against the
 * module record it reads, and a module record built from a bare `throw` alone carries
 * no bindings: the bundle would fail with "export not found" — a resolution error
 * naming a symbol, which says nothing about the runtime — instead of reaching the
 * rejection below. Exported bindings are hoisted into the record before evaluation
 * throws, so the names are there to resolve and nothing in this module can run.
 *
 * `opportune/presets` is the exception: it is pure TypeScript with no WASM and no
 * `node:module`, so the manifest resolves it here too, for every runtime including a
 * browser.
 */

import { rejection } from "./errors.js";

/** The rejection every engine-backed subpath raises in a runtime it cannot run on. */
function unsupported(): never {
  throw rejection(
    "unsupported_runtime",
    "the opportune engine runs on Node or Bun, not in a browser, a service worker or an Edge " +
      "runtime: the compiled module loads its time zone database through `node:module` and " +
      "reads the `.wasm` beside it from the filesystem. Call the engine from a Node or Bun " +
      "route handler — in a Next.js App Router project one that has " +
      "`serverExternalPackages: [\"opportune\"]` in `next.config.*`, which " +
      "`export const runtime = \"nodejs\"` alone does not arrange — and import " +
      "`opportune/presets` where a bundle needs the rule, goal and offer builders, the error " +
      "class they raise, or the generated document types: they are pure TypeScript and load " +
      "no WebAssembly.",
  );
}

/** Every name `opportune`, `opportune/adapters/graph` and `opportune/adapters/google` export. */
export {
  unsupported as EXTENDED_PROPERTY_EVENT_KEY_ID,
  unsupported as EXTENDED_PROPERTY_EXPAND,
  unsupported as EXTENDED_PROPERTY_NAMESPACE_GUID,
  unsupported as EXTENDED_PROPERTY_TAGS_ID,
  unsupported as Engine,
  unsupported as MARKER_EVENT_KEY,
  unsupported as MARKER_TAGS,
  unsupported as OpportuneError,
  unsupported as VERSION,
  unsupported as andWho,
  unsupported as atLeast,
  unsupported as availableOnly,
  unsupported as buffers,
  unsupported as businessHours,
  unsupported as calendarViewToBlocks,
  unsupported as capPerDay,
  unsupported as capPerMonth,
  unsupported as capPerWeek,
  unsupported as checkSlot,
  unsupported as checkSlotJson,
  unsupported as datedHours,
  unsupported as defaultLimits,
  unsupported as eventsListToBlocks,
  unsupported as exceptWho,
  unsupported as findAvailability,
  unsupported as findAvailabilityJson,
  unsupported as freeBusyToBlocks,
  unsupported as getScheduleToBlocks,
  unsupported as horizon,
  unsupported as httpStatusForError,
  unsupported as isKnownStatus,
  unsupported as limitsWith,
  unsupported as minNotice,
  unsupported as offerAssignedHosts,
  unsupported as offerCalendarUnknown,
  unsupported as offerEnd,
  unsupported as offerReserve,
  unsupported as offerStart,
  unsupported as offerToProposal,
  unsupported as person,
  unsupported as publicResponse,
  unsupported as publicSlotCheck,
  unsupported as requiredRanges,
  unsupported as requiredRangesJson,
  unsupported as rest,
  unsupported as singleGoal,
  unsupported as statusPolicy,
  unsupported as unless,
  unsupported as upgradeCalendar,
  unsupported as upgradeGoal,
  unsupported as upgradeRule,
  unsupported as validate,
  unsupported as validateJson,
  unsupported as weeklyHours,
  unsupported as workingHoursToRule,
};

// The module's whole purpose: any engine-backed subpath resolved for a browser fails
// here, at the import, before a caller can reach a loader that cannot work.
unsupported();
