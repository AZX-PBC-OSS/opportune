/**
 * `opportune/web`: explicit WASM initialization for browsers and module Web Workers.
 *
 * ```ts
 * import { init } from "opportune/web";
 * import { Engine } from "opportune";
 *
 * await init("/opportune_wasm_bg.wasm");
 * const engine = new Engine();
 * ```
 */

import { init as initialize, initSync as initializeSync } from "#wasm";
import type { InitInput, SyncInitInput } from "./wasm-web.js";

export type { InitInput, SyncInitInput } from "./wasm-web.js";

/** Initializes the active runtime's shared loader before any engine call. */
export function init(input?: InitInput | Promise<InitInput>): Promise<void> {
  // A loader that already has its module never reads `input` — the documented no-op —
  // and a promise nobody reads still reports its own rejection to the host: Node ends
  // the process on one, and the caller's `try`/`catch` around this call cannot
  // intercept it, because this call resolves. Handling the argument here releases it
  // without consuming it, so the load that does need the input still awaits it and
  // still translates its rejection into `wasm_unavailable`.
  if (input !== undefined) {
    void Promise.resolve(input).catch(() => {});
  }
  return initialize(input);
}

/** Synchronously initializes the active runtime's shared loader. */
export function initSync(options?: { module: SyncInitInput }): void {
  initializeSync(options);
}

export { init as default };
