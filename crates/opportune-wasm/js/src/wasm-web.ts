/**
 * The `#wasm` loader browsers and generic Web Workers resolve.
 *
 * Neither runtime has a Node filesystem or a guaranteed static `.wasm` module
 * rule, so callers initialize once via `opportune/web` (`await init(...)` or
 * `initSync({ module })`) before constructing an `Engine` or calling engine
 * helpers. Concurrent `init()` calls share one in-flight load, failures are
 * never cached so retries work, and repeated calls after initialization are
 * no-ops.
 */

import initWasm, * as glue from "#wasm-glue";

import { reason, rejection } from "./errors.js";
import { exportsEngine, REPAIR, type WasmModule } from "./wasm.js";

/** Synchronous initialization input: a precompiled module or raw WASM bytes. */
export type SyncInitInput = WebAssembly.Module | ArrayBuffer | ArrayBufferView;

/** Async initialization input: URL, Request, Response, module, or raw WASM bytes. */
export type InitInput = string | URL | Request | Response | SyncInitInput;

const NOT_LOADED =
  "the opportune WASM module is not loaded. In a browser or Web Worker, initialize it once " +
  "via `await init(...)` from `opportune/web` with the `.wasm` URL the build serves (for " +
  "example `await init(\"/opportune_wasm_bg.wasm\")`, or a URL the bundler produced from " +
  "`opportune/opportune_wasm_bg.wasm`), or `initSync({ module })` with the module the build " +
  "compiled, before calling the engine.";

const NO_INPUT =
  "`initSync()` requires `{ module }` in a browser or Web Worker because there is no filesystem " +
  "to read the `.wasm` binary from. Pass `initSync({ module })` or call `await init(url)` from " +
  "`opportune/web`.";

const IN_FLIGHT =
  "an async `init()` is already loading the opportune WASM module. Await that `init()` instead " +
  "of calling `initSync()` concurrently.";

let loaded: WasmModule | undefined;
let pending: Promise<WasmModule> | undefined;

/** Returns the initialized WASM module or throws `wasm_unavailable` if `init()` has not run. */
export function wasmModule(): WasmModule {
  if (loaded === undefined) {
    throw rejection("wasm_unavailable", NOT_LOADED);
  }
  return loaded;
}

/**
 * Loads and instantiates the WASM module once per realm.
 *
 * Concurrent calls share the same in-flight promise. A failed load resets so
 * the caller can retry with a corrected URL or after a transient network error.
 */
export async function init(input?: InitInput | Promise<InitInput>): Promise<void> {
  if (loaded !== undefined) {
    return;
  }
  pending ??= instantiate(input)
    .then((module) => {
      // Set `loaded` before `pending` clears: a caller arriving between the two would
      // otherwise see neither and start a second instantiation.
      loaded = module;
      return module;
    })
    .finally(() => {
      pending = undefined;
    });
  await pending;
}

/**
 * Synchronously instantiates the WASM module from a precompiled `WebAssembly.Module`
 * or raw WASM bytes.
 */
export function initSync(options?: { module: SyncInitInput }): void {
  if (loaded !== undefined) {
    return;
  }
  if (pending !== undefined) {
    throw rejection("wasm_unavailable", IN_FLIGHT);
  }
  if (options === undefined) {
    throw rejection("wasm_unavailable", NO_INPUT);
  }
  try {
    glue.initSync(options);
  } catch (cause) {
    throw rejection(
      "wasm_unavailable",
      `the opportune WASM module did not instantiate: ${reason(cause)}. ${REPAIR}`,
      cause,
    );
  }
  loaded = validated(glue);
}

async function instantiate(input: InitInput | Promise<InitInput> | undefined): Promise<WasmModule> {
  try {
    const resolved = input === undefined ? undefined : await input;
    await initWasm(resolved === undefined ? undefined : { module_or_path: resolved });
  } catch (cause) {
    throw rejection(
      "wasm_unavailable",
      `the opportune WASM module did not load: ${reason(cause)}. Check that the \`.wasm\` URL ` +
        `resolves and is served with \`application/wasm\`. ${REPAIR}`,
      cause,
    );
  }
  return validated(glue);
}

function validated(candidate: WasmModule): WasmModule {
  if (!exportsEngine(candidate)) {
    throw rejection(
      "wasm_unavailable",
      `the opportune WASM module does not export WasmEngine: it was built from an older ` +
        `surface. ${REPAIR}`,
    );
  }
  return candidate;
}
