/**
 * The type face of the ES module `wasm-pack --target web` generates, which `#wasm-glue`
 * resolves to at run time.
 *
 * That file exists only after a wasm build, so the compiler needs a declaration that does
 * not. Each member is derived from `WasmModule` — the one place the module's surface is
 * declared for the code above the seam — so the two cannot disagree: a member the
 * interface drops is an error here, and a member it adds is one this file gains. The
 * members are listed because an ES module's namespace has to be declared by name; a type
 * cannot stand in for one.
 *
 * This file is never emitted or published. It is read by `tsconfig.build.json` and
 * `tsconfig.check.json` through the same `paths` alias that gives them `#wasm`, and by
 * nothing at run time.
 */

import type { SyncInitInput } from "./wasm-web.js";
import type { WasmModule } from "./wasm.js";

export declare const WasmEngine: WasmModule["WasmEngine"];
export declare const version: WasmModule["version"];
export declare const default_limits_json: WasmModule["default_limits_json"];
export declare const upgrade_rule_json: WasmModule["upgrade_rule_json"];
export declare const upgrade_calendar_json: WasmModule["upgrade_calendar_json"];
export declare const upgrade_goal_json: WasmModule["upgrade_goal_json"];
export declare const marker_tags: WasmModule["marker_tags"];
export declare const marker_event_key: WasmModule["marker_event_key"];
export declare const graph_extended_property_namespace_guid: WasmModule["graph_extended_property_namespace_guid"];
export declare const graph_extended_property_tags_id: WasmModule["graph_extended_property_tags_id"];
export declare const graph_extended_property_event_key_id: WasmModule["graph_extended_property_event_key_id"];
export declare const graph_extended_property_expand: WasmModule["graph_extended_property_expand"];

/**
 * Instantiates the generated module from a module the bundle already compiled, or
 * from the raw bytes of one.
 *
 * `{ module }` is the form this package uses: passing the `WebAssembly.Module` itself
 * keeps `workerd` from compiling at run time, which it refuses outright. The type is
 * the loader's own {@link SyncInitInput}, the same union `src/wasm-web.ts` accepts, so
 * the seam cannot narrow one side without the other failing to compile.
 */
export declare function initSync(options: { module: SyncInitInput }): unknown;

/**
 * The asynchronous initialiser `wasm-pack` exports by default.
 *
 * Callers give it the value to load under `module_or_path`; with none, the generated
 * module fetches `opportune_wasm_bg.wasm` beside itself. The value's shape is the
 * loader's business (`wasm-web.ts` declares it), so the shared declaration leaves it
 * `unknown` rather than restating a Web-platform surface here.
 */
declare function init(options?: { module_or_path?: unknown }): Promise<unknown>;

export default init;
