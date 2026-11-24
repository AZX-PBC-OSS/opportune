/**
 * What the package's engine code needs from the compiled module.
 *
 * `wasm-pack` builds this crate for two targets from one artefact: the `nodejs` target
 * emits a CommonJS glue that reads the module beside itself and instantiates it as it
 * loads, and the `web` target an ES module a runtime initialises explicitly. `npm run
 * build` copies both glues and the one `.wasm` into `dist/pkg/`, and
 * `package.json#imports` selects the loader through `#wasm`. This module holds what
 * everything above that seam shares: the shape of the exports the engine code calls, the
 * repair guidance a load failure carries, and the translation of everything a call can
 * throw into one error.
 *
 * Nothing here names a runtime or a loader: no `node:` import, no `fetch`, no `.wasm`
 * path, no initialiser. That is what lets each runtime's generated glue differ behind
 * `#wasm` while one copy of the contract and the translation sits below every
 * engine-backed entry point, including the adapters.
 */

import { OpportuneError, reason } from "./errors.js";
import { asRecord } from "./json.js";
import type { Diagnostic } from "./generated/types.js";

/**
 * The engine inside the compiled module: the bundled time zone database and the
 * request limits, with the JSON entry points as its methods.
 *
 * `free` is the explicit release the module gives its native resource, and
 * {@link WasmEngine.free} calling it twice is why an owner must remember whether it
 * has: `Engine` records the release instead of forwarding a second one.
 */
export interface WasmEngine {
  tzdb_version(): string;
  find_availability_json(requestJson: string): string;
  check_slot_json(requestJson: string, proposalJson: string): string;
  validate_json(requestJson: string): string;
  required_ranges_json(queryJson: string): string;
  graph_calendar_view_to_blocks_json(
    payloadJson: string,
    participantZone: string,
    categoryMapJson: string | null,
  ): string;
  graph_get_schedule_to_blocks_json(payloadJson: string): string;
  graph_working_hours_to_rule_json(payloadJson: string, ruleId: string): string;
  google_events_list_to_blocks_json(payloadJson: string, participantZone: string): string;
  google_free_busy_to_blocks_json(payloadJson: string): string;
  free(): void;
}

/**
 * The exports this package calls, as both `wasm-pack` targets export them.
 *
 * The target decides how the module is loaded, not what it exports: the engine class and
 * the free functions are the same in the `nodejs` glue and the `web` one, which
 * additionally exports the initialisers a runtime that loads explicitly calls. Those
 * belong to the loaders that call them, so they are not part of what the engine code
 * above `#wasm` depends on.
 */
export interface WasmModule {
  readonly WasmEngine: new (limitsJson?: string | null) => WasmEngine;
  readonly version: () => string;
  readonly default_limits_json: () => string;
  readonly upgrade_rule_json: (ruleJson: string, fromVersion: number) => string;
  readonly upgrade_calendar_json: (calendarJson: string, fromVersion: number) => string;
  readonly upgrade_goal_json: (goalJson: string, fromVersion: number) => string;
  readonly marker_tags: () => string;
  readonly marker_event_key: () => string;
  readonly graph_extended_property_namespace_guid: () => string;
  readonly graph_extended_property_tags_id: () => string;
  readonly graph_extended_property_event_key_id: () => string;
  readonly graph_extended_property_expand: () => string;
}

/**
 * What a caller does about a module that did not load.
 *
 * The message reaches an application's operator, not the repository's maintainers: a
 * consumer of the published package reinstalls it, and someone working from a
 * checkout rebuilds the artefact there. The monorepo's own `wasm-pack` invocation
 * names neither of those places, so it is not what this failure says.
 */
export const REPAIR =
  "Reinstall the package (`npm install opportune`) to restore it, or rebuild a source " +
  "checkout from its root with `make wasm`.";

/**
 * Runs one call into the module, raising whatever it throws as an
 * {@link OpportuneError}.
 *
 * The module throws its rejections as a string holding the structured payload; a
 * throw that is not one of those is a failure of the module itself and is reported
 * as `wasm_error` rather than escaping as whatever type the runtime chose.
 */
export function translated<T>(call: () => T): T {
  try {
    return call();
  } catch (thrown) {
    throw failure(thrown);
  }
}

/** The {@link OpportuneError} for a value a call into the module threw. */
function failure(thrown: unknown): OpportuneError {
  if (thrown instanceof OpportuneError) {
    return thrown;
  }
  if (typeof thrown === "string") {
    const payload = fromPayload(thrown);
    if (payload !== undefined) {
      return payload;
    }
  }
  return new OpportuneError("wasm_error", `the opportune WASM module failed: ${reason(thrown)}`, {
    cause: thrown,
  });
}

/**
 * The error the module's thrown payload describes, or `undefined` when `text` is
 * not that payload.
 *
 * The payload always carries all four fields — `code`, `message`, `path` and
 * `errors` — with `path` and each finding's `path` `null` when no field is at
 * fault. Text that parses to anything else did not come from the module's own
 * rejection path.
 */
function fromPayload(text: string): OpportuneError | undefined {
  let parsed: unknown;
  try {
    parsed = JSON.parse(text);
  } catch {
    // A thrown string that is not JSON did not come from the module's rejection
    // path, so it is reported as a module failure rather than as a rejection.
    return undefined;
  }
  const payload = asRecord(parsed);
  if (payload === undefined) {
    return undefined;
  }
  const code = payload["code"];
  const message = payload["message"];
  const path = payload["path"];
  const errors = findings(payload["errors"]);
  if (
    typeof code !== "string" ||
    typeof message !== "string" ||
    (path !== null && path !== undefined && typeof path !== "string") ||
    errors === undefined
  ) {
    return undefined;
  }
  return new OpportuneError(code, message, { path: path ?? null, errors, cause: text });
}

/** The findings `value` holds, or `undefined` when it is not a finding list. */
function findings(value: unknown): Diagnostic[] | undefined {
  if (!Array.isArray(value)) {
    return undefined;
  }
  const out: Diagnostic[] = [];
  for (const entry of value) {
    const finding = asRecord(entry);
    if (finding === undefined) {
      return undefined;
    }
    const { code, message, path } = finding;
    if (typeof code !== "string" || typeof message !== "string") {
      return undefined;
    }
    if (path !== null && path !== undefined && typeof path !== "string") {
      return undefined;
    }
    out.push({ code, message, path: path ?? null });
  }
  return out;
}
