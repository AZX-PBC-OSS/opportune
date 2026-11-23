/**
 * The bridge to the `wasm-pack` Node build in `pkg/`.
 *
 * The module wasm-pack writes is CommonJS and its declarations live beside it, so
 * the package loads it with `createRequire` instead of a static `import`: a static
 * import of the compiled module would pull a file outside the compiler's `rootDir`
 * into the program, and there would be nothing for it to type against but
 * wasm-bindgen's generated `.d.ts`. `npm run build` copies the module into
 * `dist/pkg/` under a `.cjs` name (see `bin/bundle-module.mjs`), so what this file
 * loads is part of the published tree. What is declared here is the subset of
 * exports this package calls, and it is checked by name at load time, so a `pkg/`
 * built from an older Rust surface stops the caller with the export it is missing
 * rather than with "`undefined` is not a function" several frames deep.
 *
 * Everything a call can throw is translated here, once: a rejection the module
 * documented arrives as its structured payload and is rebuilt as an
 * {@link OpportuneError} carrying the same four fields, so an application answers
 * its own caller with the pointer into the document it sent.
 *
 * The module is loaded on first use, not when this package is imported: importing
 * `opportune` to reach a preset builder, a sugar helper or a generated type must
 * not pay for instantiating a WebAssembly module — and must not fail because one
 * was never built. A caller that never touches the engine never loads it; the
 * first call that does loads it once and keeps it.
 */

import { createRequire } from "node:module";

import { OpportuneError, reason, rejection } from "./errors.js";
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

/** The exports this package calls, as the crate exports them. */
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

const require = createRequire(import.meta.url);

/** Where the build puts the compiled module, seen from this file in `dist/`. */
const MODULE_PATH = "./pkg/opportune_wasm.cjs";

/**
 * What a caller does about a module that did not load.
 *
 * The message reaches an application's operator, not the repository's maintainers: a
 * consumer of the published package reinstalls it, and someone working from a
 * checkout rebuilds the artefact there. The monorepo's own `wasm-pack` invocation
 * names neither of those places, so it is not what this failure says.
 */
const REPAIR =
  "Reinstall the package (`npm install opportune`) to restore it, or rebuild a source " +
  "checkout from its root with `make wasm`.";

/**
 * What a caller does when the module could not be *resolved*, rather than loaded.
 *
 * A bundler that inlines this package into one output file moves `dist/index.js`
 * away from the `dist/pkg/` pair beside it, so `require` resolves next to the bundle
 * and finds nothing: the file is present in `node_modules` and absent from the
 * build's own tree. Reinstalling and rebuilding both succeed and change nothing, so
 * this is the one case where the advice above is the wrong advice — and it is the
 * case a deployment meets first, since a serverless function is a bundle.
 */
const BUNDLED =
  "If a bundler produced this build, keep `opportune` external so its `dist/pkg/` " +
  "directory travels beside the module: `serverExternalPackages: [\"opportune\"]` in " +
  "`next.config.*`, or `external: [\"opportune\"]` in esbuild, Vite and Bun.";

/**
 * The repair guidance for a load failure, with the bundler note when `cause` is a
 * resolution failure.
 *
 * `require` reports a module that is not beside the loader in the runtime's own
 * words: Node sets `code: "MODULE_NOT_FOUND"`, Bun and the bundlers word it as
 * `Cannot find module`/`Cannot find package` without one. Both are read, so the
 * hint reaches the consumer whichever runtime bundled them.
 */
function repair(cause?: unknown): string {
  return resolutionFailure(cause) ? `${REPAIR} ${BUNDLED}` : REPAIR;
}

/** Whether `cause` is the runtime saying it could not resolve the module at all. */
function resolutionFailure(cause: unknown): boolean {
  const code =
    typeof cause === "object" && cause !== null
      ? (cause as { code?: unknown }).code
      : undefined;
  if (code === "ERR_MODULE_NOT_FOUND" || code === "MODULE_NOT_FOUND") {
    return true;
  }
  return /\bCannot find (?:module|package)\b/.test(reason(cause));
}

/** The free functions the package calls, checked by name when the module loads. */
const FUNCTIONS: readonly Exclude<keyof WasmModule, "WasmEngine">[] = [
  "version",
  "default_limits_json",
  "upgrade_rule_json",
  "upgrade_calendar_json",
  "upgrade_goal_json",
  "marker_tags",
  "marker_event_key",
  "graph_extended_property_namespace_guid",
  "graph_extended_property_tags_id",
  "graph_extended_property_event_key_id",
  "graph_extended_property_expand",
] as const;

/** The engine's own entry points, checked by name when the module loads. */
const METHODS: readonly (keyof WasmEngine)[] = [
  "tzdb_version",
  "find_availability_json",
  "check_slot_json",
  "validate_json",
  "required_ranges_json",
  "graph_calendar_view_to_blocks_json",
  "graph_get_schedule_to_blocks_json",
  "graph_working_hours_to_rule_json",
  "google_events_list_to_blocks_json",
  "google_free_busy_to_blocks_json",
  "free",
] as const;

/**
 * The compiled module, loaded once on first use.
 *
 * Loading is lazy and single: the module instance and its memory are the
 * resource every engine answers from, and there is one of each per process.
 *
 * @throws OpportuneError with code `wasm_unavailable` when the package was not
 *   built or its module cannot be loaded, or was built from a Rust surface that
 *   does not export what this package calls — the message names what the load
 *   reported or the missing export, and what a caller does about it.
 */
function load(): WasmModule {
  let loaded: unknown;
  try {
    loaded = require(MODULE_PATH);
  } catch (cause) {
    // `require` fails both for a file that is absent and for one that is present
    // but cannot be instantiated — a build interrupted while writing the `.wasm`
    // beside the loader — so the message reports what the load said rather than
    // claiming the module is missing: the second case sends whoever is paged
    // looking for a file that is already there. Only its first line, because a
    // Node module error carries a require stack below it and the whole failure is
    // kept as the cause.
    throw rejection(
      "wasm_unavailable",
      `the opportune WASM module at ${MODULE_PATH} beside this package did not load: ` +
        `${reason(cause).replace(/\n.*/s, "")}. ${repair(cause)}`,
      cause,
    );
  }
  const members = asRecord(loaded);
  if (members === undefined) {
    throw rejection(
      "wasm_unavailable",
      `the opportune WASM module at ${MODULE_PATH} did not load as a module object. ${REPAIR}`,
    );
  }
  const missing = ["WasmEngine", ...FUNCTIONS].filter(
    (name) => typeof members[name] !== "function",
  );
  if (missing.length > 0) {
    throw rejection(
      "wasm_unavailable",
      `the opportune WASM module at ${MODULE_PATH} does not export ${missing.join(", ")}: ` +
        `it was built from an older surface. ${REPAIR}`,
    );
  }
  const engineClass = members["WasmEngine"];
  const prototype = typeof engineClass === "function" ? asRecord(Reflect.get(engineClass, "prototype")) : undefined;
  const absent = METHODS.filter((name) => typeof prototype?.[name] !== "function");
  if (absent.length > 0) {
    throw rejection(
      "wasm_unavailable",
      `the opportune WASM module at ${MODULE_PATH} has no engine method ${absent.join(", ")}: ` +
        `it was built from an older surface. ${REPAIR}`,
    );
  }
  // Every export this package calls is present and callable, which is what the
  // declared signatures describe: the module is compiled from this repository's own
  // Rust crate, so its types cannot drift from it without the build changing.
  return loaded as WasmModule;
}

/** The compiled module, loading it on the first call. */
let loaded: WasmModule | undefined;

/**
 * The compiled module, loading it if this is the first call.
 *
 * Every WASM-backed entry point reaches the module through here, so a package
 * that is imported but never used loads nothing, and one whose module cannot be
 * loaded fails on the call that needs it rather than on `import`.
 *
 * @throws OpportuneError with code `wasm_unavailable`; see {@link load}.
 */
export function wasmModule(): WasmModule {
  loaded ??= load();
  return loaded;
}

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
