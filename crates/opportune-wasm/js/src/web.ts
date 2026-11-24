/**
 * `opportune/web`: explicit WASM initialization for browsers and generic Web Workers.
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
  return initialize(input);
}

/** Synchronously initializes the active runtime's shared loader. */
export function initSync(options?: { module: SyncInitInput }): void {
  initializeSync(options);
}

export { init as default };
