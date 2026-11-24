/**
 * Failure and retry behaviour of the real shipped workerd loader.
 *
 * Node's module hooks replace only the two host boundaries the Worker runtime owns:
 * the precompiled `.wasm` import and the generated wasm-bindgen glue. The loader and
 * its error translation are the actual files published in `dist/`. A child process
 * gives the module a cold cache under both the Node and Bun test runners — and the
 * child must be Node even under Bun, whose loader has no `registerHooks` to install
 * these hooks with.
 */

import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { join } from "node:path";
import { pathToFileURL } from "node:url";
import test from "node:test";

const PACKAGE = join(import.meta.dirname, "..");
const LOADER = pathToFileURL(join(PACKAGE, "dist", "wasm-workerd.js")).href;

/** Bun's loader cannot install hooks, so the probe always runs on Node proper. */
const NODE = process.versions.bun ? "node" : process.execPath;

test("the workerd loader keeps failures retryable and caches only success", () => {
  const run = spawnSync(
    NODE,
    ["--input-type=module", "--eval", `
      import assert from "node:assert/strict";
      import { registerHooks } from "node:module";

      const precompiled = { precompiled: true };
      const failure = new Error("instantiation refused");
      globalThis.__workerPrecompiled = precompiled;
      globalThis.__workerFailure = failure;
      globalThis.__workerAttempts = 0;
      globalThis.__workerInput = undefined;
      globalThis.__workerWithEngine = true;

      registerHooks({
        resolve(specifier, context, nextResolve) {
          if (specifier === "#wasm-glue") {
            const source = \`
              export function initSync({ module }) {
                globalThis.__workerAttempts += 1;
                globalThis.__workerInput = module;
                if (globalThis.__workerFailure !== undefined) {
                  throw globalThis.__workerFailure;
                }
              }
              \${globalThis.__workerWithEngine ? "export class WasmEngine {}" : ""}
            \`;
            return {
              url: "data:text/javascript," + encodeURIComponent(source),
              shortCircuit: true,
            };
          }
          if (specifier.endsWith(".wasm")) {
            return {
              url: "data:text/javascript," + encodeURIComponent(
                "export default globalThis.__workerPrecompiled;",
              ),
              shortCircuit: true,
            };
          }
          return nextResolve(specifier, context);
        },
      });

      const loader = await import(${JSON.stringify(LOADER)});
      assert.equal(globalThis.__workerAttempts, 0, "importing the loader instantiated WASM");

      assert.throws(
        () => loader.wasmModule(),
        (thrown) => {
          assert.equal(thrown.name, "OpportuneError");
          assert.equal(thrown.code, "wasm_unavailable");
          assert.equal(thrown.cause, failure);
          assert.match(thrown.message, /did not instantiate/);
          assert.match(thrown.message, /make wasm/);
          return true;
        },
      );
      assert.equal(globalThis.__workerAttempts, 1);

      globalThis.__workerFailure = undefined;
      const loaded = loader.wasmModule();
      assert.equal(globalThis.__workerAttempts, 2, "the failed attempt was cached");
      assert.equal(globalThis.__workerInput, precompiled);
      assert.equal(loader.wasmModule(), loaded);
      loader.initSync();
      await loader.init();
      assert.equal(globalThis.__workerAttempts, 2, "a successful load was instantiated twice");
    `],
    { cwd: PACKAGE, encoding: "utf8", timeout: 10_000 },
  );
  assert.equal(run.error, undefined, `the loader probe did not finish: ${run.error?.message}`);
  assert.equal(run.status, 0, `the workerd loader probe failed:\n${run.stdout}${run.stderr}`);
});

test("the workerd loader rejects stale glue and never half-caches it", () => {
  // The other published failure path: glue built from an older surface exports no
  // `WasmEngine`, instantiation itself succeeds, and the loader must refuse it with
  // the repairable error. The refusal cannot heal in-process — the glue is a static
  // import, so only a fresh isolate (after `npm install` or `make wasm`) gets one —
  // but neither may the bad module be cached as loaded: every attempt re-runs
  // instantiation and reports the same error, never a wedged half-state.
  const run = spawnSync(
    NODE,
    ["--input-type=module", "--eval", `
      import assert from "node:assert/strict";
      import { registerHooks } from "node:module";

      globalThis.__workerWithEngine = false;
      globalThis.__workerAttempts = 0;

      registerHooks({
        resolve(specifier, context, nextResolve) {
          if (specifier === "#wasm-glue") {
            const source = \`
              export function initSync() {
                globalThis.__workerAttempts += 1;
              }
            \`;
            return {
              url: "data:text/javascript," + encodeURIComponent(source),
              shortCircuit: true,
            };
          }
          if (specifier.endsWith(".wasm")) {
            return {
              url: "data:text/javascript," + encodeURIComponent("export default {};"),
              shortCircuit: true,
            };
          }
          return nextResolve(specifier, context);
        },
      });

      const loader = await import(${JSON.stringify(LOADER)});

      for (const attempt of [1, 2]) {
        assert.throws(
          () => loader.wasmModule(),
          (thrown) => {
            assert.equal(thrown.name, "OpportuneError");
            assert.equal(thrown.code, "wasm_unavailable");
            assert.match(thrown.message, /WasmEngine/);
            assert.match(thrown.message, /make wasm/);
            return true;
          },
          "attempt " + attempt + " did not report the stale-glue rejection",
        );
        assert.equal(globalThis.__workerAttempts, attempt, "each attempt re-ran instantiation");
      }
    `],
    { cwd: PACKAGE, encoding: "utf8", timeout: 10_000 },
  );
  assert.equal(run.error, undefined, `the stale-glue probe did not finish: ${run.error?.message}`);
  assert.equal(run.status, 0, `the stale-glue probe failed:\n${run.stdout}${run.stderr}`);
});
