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

export {
  init,
  initSync,
  init as default,
  type InitInput,
  type SyncInitInput,
} from "./wasm-web.js";
