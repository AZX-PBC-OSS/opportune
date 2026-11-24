/**
 * The `#wasm` loader Cloudflare Workers (`workerd`) resolves.
 *
 * Wrangler compiles static `.wasm` imports into a `WebAssembly.Module` at
 * bundle time, avoiding runtime `WebAssembly.compile` (which `workerd`
 * prohibits). Instantiation stays lazy on the first engine call, cached per
 * isolate, and never caches a failed load.
 */

import precompiled from "./pkg/opportune_wasm_bg.wasm";
import * as glue from "#wasm-glue";

import { reason, rejection } from "./errors.js";
import { exportsEngine, REPAIR, type WasmModule } from "./wasm.js";

let loaded: WasmModule | undefined;

/** Explicit initialization for isomorphic callers, using the Worker's precompiled module. */
export async function init(_input?: unknown): Promise<void> {
  wasmModule();
}

/** Synchronous explicit initialization, sharing the root entry's precompiled module. */
export function initSync(_options?: unknown): void {
  wasmModule();
}

/** Instantiates the statically bundled Worker WASM module on first use. */
export function wasmModule(): WasmModule {
  if (loaded !== undefined) {
    return loaded;
  }
  try {
    glue.initSync({ module: precompiled });
  } catch (cause) {
    throw rejection(
      "wasm_unavailable",
      `the opportune WASM module this Worker bundles did not instantiate: ${reason(cause)}. ${REPAIR}`,
      cause,
    );
  }
  if (!exportsEngine(glue)) {
    throw rejection(
      "wasm_unavailable",
      `the opportune WASM module this Worker bundles does not export WasmEngine: it was built ` +
        `from an older surface. ${REPAIR}`,
    );
  }
  loaded = glue;
  return loaded;
}
