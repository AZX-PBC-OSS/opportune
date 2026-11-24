/**
 * The `#wasm` entry Node and Bun get: the module `wasm-pack` generates, loaded on the
 * first engine call.
 *
 * `package.json#imports` maps `#wasm` to this module for the `node` condition and as the
 * default, which is the one place a runtime is chosen. `engine.ts` imports `#wasm` and
 * every engine-backed entry point — the adapters included — reaches the module through
 * that one import, so the public API above the seam is one implementation and each
 * runtime gets its own generated loader behind it.
 *
 * This file is not a loader. `wasm-pack --target nodejs` already writes one: a CommonJS
 * module that finds the `.wasm` beside itself with `__dirname`, reads it with `node:fs`,
 * and instantiates it with `new WebAssembly.Module` at load time. All this module does is
 * decide *when* that happens — `require` is deferred to the first engine call, so
 * importing `opportune` to reach a preset builder, a sugar helper or a generated type
 * pays for nothing and fails for nothing — and translate a load that did not work into
 * the package's own `wasm_unavailable` diagnostic instead of a bare `ENOENT` or
 * `CompileError` escaping from inside the generated code.
 *
 * The load is single for this copy of the package in the current runtime, and a load
 * that failed is not remembered: someone who repairs the install and calls again gets a
 * working engine rather than a package that has to be restarted.
 */

import { createRequire } from "node:module";

import { reason, rejection } from "./errors.js";
import { REPAIR, type WasmModule } from "./wasm.js";

const require = createRequire(import.meta.url);

/**
 * The loader `wasm-pack --target nodejs` writes, as the build publishes it.
 *
 * The `.cjs` extension is what makes it CommonJS: this package is `"type": "module"`,
 * so the extension is the only thing that says how to parse the file wasm-pack emitted.
 */
const GLUE = "./pkg/opportune_wasm.cjs";

/**
 * What a caller does when the generated loader could not be *resolved*, rather than run.
 *
 * A bundler that inlines this package into one output file moves `dist/index.js` away
 * from the `dist/pkg/` pair beside it, so `require` resolves next to the bundle and finds
 * nothing: the file is present in `node_modules` and absent from the build's own tree.
 * Reinstalling and rebuilding both succeed and change nothing, so this is the one case
 * where the advice above is the wrong advice — and it is the case a deployment meets
 * first, since a serverless function is a bundle.
 */
const BUNDLED =
  "If a bundler produced this build, keep `opportune` external so its `dist/pkg/` " +
  "directory travels beside the module: `serverExternalPackages: [\"opportune\"]` in " +
  "`next.config.*`, or `external: [\"opportune\"]` in esbuild, Vite and Bun.";

/**
 * The repair guidance for a load failure, with the bundler note when `cause` is a
 * resolution failure.
 *
 * `require` reports a module that is not beside the loader in the runtime's own words:
 * Node sets `code: "MODULE_NOT_FOUND"`, Bun and the bundlers word it as `Cannot find
 * module`/`Cannot find package` without one. Both are read, so the hint reaches the
 * consumer whichever runtime bundled them.
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

/** The generated module, loaded on the first call. */
let loaded: WasmModule | undefined;

/** Explicit initialization for isomorphic callers; Node/Bun load their packaged module. */
export async function init(_input?: unknown): Promise<void> {
  wasmModule();
}

/** Synchronous explicit initialization, sharing the root entry's lazy module. */
export function initSync(_options?: unknown): void {
  wasmModule();
}

/**
 * The generated module, loading it if this is the first call.
 *
 * @throws OpportuneError with code `wasm_unavailable` when the generated loader is
 *   absent or was built for another target, when the `.wasm` beside it cannot be read,
 *   or when it cannot be instantiated — the message names what failed and what a caller
 *   does about it, and keeps the runtime's own failure as the cause.
 */
export function wasmModule(): WasmModule {
  loaded ??= load();
  return loaded;
}

/** The generated module, deferred to and checked against the one export this package constructs. */
function load(): WasmModule {
  let glue: unknown;
  try {
    glue = require(GLUE);
  } catch (cause) {
    // Both of the generated loader's steps — resolving itself and reading the `.wasm`
    // beside it — fail here, so the message reports what the runtime said rather than
    // guessing which file is missing: only its first line, because a module error can
    // carry a require stack below it and the whole failure is kept as the cause.
    throw rejection(
      "wasm_unavailable",
      `the opportune WASM module at ${GLUE} beside this package did not load: ` +
        `${reason(cause).replace(/\n.*/s, "")}. ${repair(cause)}`,
      cause,
    );
  }
  // The one fact this package needs from the loaded module, and the only surface check
  // it makes: constructing the engine is what every call goes through, and a `pkg/` left
  // behind by an earlier build loads cleanly and then throws "not a constructor" from
  // inside the generated code. The engine's other calls are typechecked against the
  // interface in `wasm.ts`; if older glue lacks one of those members, its own call fails
  // and is translated rather than escaping as a raw `TypeError`.
  if (!exportsEngine(glue)) {
    // Unlike a thrown read or compile, this require completed and is cached. Evict the
    // rejected glue so repairing a stale installation and calling again can load it.
    delete require.cache[require.resolve(GLUE)];
    throw rejection(
      "wasm_unavailable",
      `the opportune WASM module at ${GLUE} does not export WasmEngine: it was built ` +
        `from an older surface. ${REPAIR}`,
    );
  }
  return glue as WasmModule;
}

/** Whether `glue` exports the engine class, which is what makes it this crate's module. */
function exportsEngine(glue: unknown): boolean {
  return (
    (typeof glue === "object" || typeof glue === "function") &&
    glue !== null &&
    typeof (glue as { WasmEngine?: unknown }).WasmEngine === "function"
  );
}
