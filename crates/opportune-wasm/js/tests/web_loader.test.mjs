/**
 * The browser/Web Worker loader's state machine, against a real HTTP response.
 *
 * `dist/wasm-web.js` is what the `browser` and `worker` conditions resolve, and its
 * promises are all observable from Node: the generated glue's `init` accepts a URL and
 * fetches it, so a loopback server can answer once, answer 404 once, and count the
 * requests the loader actually made. That is the whole contract the page and Worker
 * harnesses depend on, tested here without a browser so a regression is reported by the
 * fast suite rather than only by Playwright.
 *
 * The loopback checks are one sequential case because the loader's state is module state:
 * pre-init checks run before anything loads, failure precedes the successful retry, and
 * that retry leaves the module loaded. The cases that need a fresh public module graph run
 * in a separate process.
 *
 * ```sh
 * make test-wasm
 * ```
 */

import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { createServer } from "node:http";
import { readFile } from "node:fs/promises";
import { join } from "node:path";
import test from "node:test";
import { pathToFileURL } from "node:url";

import { wasmModule, init, initSync } from "../dist/wasm-web.js";

/** The package root, and the browser loader file the `browser`/`worker` conditions name. */
const PACKAGE = join(import.meta.dirname, "..");
const LOADER = pathToFileURL(join(PACKAGE, "dist", "wasm-web.js")).href;

/** The built module the server answers with, and the path it answers on. */
const WASM = join(PACKAGE, "dist", "pkg", "opportune_wasm_bg.wasm");

/**
 * The Node binary that drives the glue-boundary probe.
 *
 * Bun's loader has no `module.registerHooks`, so the probe runs on Node proper even under
 * the Bun lane — the browser-loader source it imports is the same file either way.
 */
const NODE = process.versions.bun ? "node" : process.execPath;

/** A loopback server that answers the module once per request and 404s everything else. */
async function server() {
  const bytes = await readFile(WASM);
  let requests = 0;
  const instance = createServer((request, response) => {
    if (request.url === "/opportune_wasm_bg.wasm") {
      requests += 1;
      response.writeHead(200, { "content-type": "application/wasm" });
      response.end(bytes);
      return;
    }
    response.writeHead(404, { "content-type": "text/plain" });
    response.end("not found");
  });
  await new Promise((resolve) => {
    instance.listen(0, "127.0.0.1", resolve);
  });
  const { port } = instance.address();
  return {
    origin: `http://127.0.0.1:${String(port)}`,
    requests: () => requests,
    close: () => new Promise((resolve) => instance.close(resolve)),
  };
}

/** Runs `source` against the package's public name in a fresh runtime and conditions. */
function publicGraph(source, conditions = []) {
  const run = spawnSync(
    process.execPath,
    [...conditions.map((condition) => `--conditions=${condition}`), "--input-type=module", "--eval", source],
    { cwd: PACKAGE, encoding: "utf8", timeout: 10_000 },
  );
  assert.equal(run.error, undefined, `the public-graph process did not finish: ${run.error?.message}`);
  assert.equal(run.status, 0, `the public graph failed:\n${run.stdout}${run.stderr}`);
}

test("initSync(bytes) initializes the public browser graph before any async call", () => {
  publicGraph(`
    import assert from "node:assert/strict";
    import { readFileSync } from "node:fs";
    import init, { initSync } from "opportune/web";
    import { Engine } from "opportune";
    import { calendarViewToBlocks } from "opportune/adapters/graph";

    assert.throws(() => new Engine(), { code: "wasm_unavailable" });
    const bytes = readFileSync(new URL("./dist/pkg/opportune_wasm_bg.wasm", import.meta.url));
    initSync({ module: bytes });
    await init("/a-repeat-init-must-not-fetch-this.wasm");
    initSync();

    using engine = new Engine();
    assert.match(engine.tzdbVersion, /^\\d{4}[a-z]$/);
    assert.equal(calendarViewToBlocks({ value: [] }, "UTC").status, "available");
  `, ["browser"]);
});

test("opportune/web resolves a promised URL before fetching it", () => {
  publicGraph(`
    import assert from "node:assert/strict";
    import { readFileSync } from "node:fs";
    import init from "opportune/web";
    import { Engine } from "opportune";

    const bytes = readFileSync(new URL("./dist/pkg/opportune_wasm_bg.wasm", import.meta.url));
    const url = \`data:application/wasm;base64,\${bytes.toString("base64")}\`;
    await init(Promise.resolve(url));

    using engine = new Engine();
    assert.match(engine.tzdbVersion, /^\\d{4}[a-z]$/);
  `, ["browser"]);
});

test("an init() input no loader consumes cannot become an unhandled rejection", () => {
  // `init()` takes `InitInput | Promise<InitInput>`, so the natural async spelling is
  // `await init(fetch(url))`. A loader that already has its module never reads that
  // argument — the documented no-op — but a promise nobody attaches a handler to still
  // reports its rejection to the host, and Node ends the process on one. The caller
  // cannot defend itself: `init()` resolves, so its `try`/`catch` catches nothing, and
  // the exit happens after the engine call below has already succeeded. Each child here
  // exits non-zero if the input was dropped rather than released.
  for (const conditions of [["browser"], []]) {
    publicGraph(`
      import assert from "node:assert/strict";
      import init, { initSync } from "opportune/web";
      import { Engine } from "opportune";
      ${
        conditions.length === 0
          ? ""
          : `
      import { readFileSync } from "node:fs";
      initSync({
        module: readFileSync(new URL("./dist/pkg/opportune_wasm_bg.wasm", import.meta.url)),
      });`
      }

      await init(Promise.reject(new Error("a fetch this loader did not need")));

      using engine = new Engine();
      assert.match(engine.tzdbVersion, /^\\d{4}[a-z]$/);
    `, conditions);
  }
});

test("a second concurrent init() releases the input the in-flight load owns", () => {
  // The third path that never reads its argument: `pending ??= instantiate(input)` means a
  // caller arriving while a load is in flight contributes nothing, and its own promised
  // input is nobody's. `init()` assigns `pending` synchronously — `instantiate` runs to its
  // first `await` before `init()` returns — so the second call below is deterministically
  // the in-flight case with no I/O to time, and the child exits non-zero if that argument
  // was dropped rather than released. That one load is shared is pinned by the loopback
  // case below, which counts the requests the loader actually made.
  publicGraph(`
    import assert from "node:assert/strict";
    import { readFileSync } from "node:fs";
    import init from "opportune/web";
    import { Engine } from "opportune";

    const bytes = readFileSync(new URL("./dist/pkg/opportune_wasm_bg.wasm", import.meta.url));
    const loading = init(\`data:application/wasm;base64,\${bytes.toString("base64")}\`);
    await init(Promise.reject(new Error("the in-flight load owns this one")));
    await loading;

    using engine = new Engine();
    assert.match(engine.tzdbVersion, /^\\d{4}[a-z]$/);
  `, ["browser"]);
});

test("a rejecting init() input still fails the load that needed it", () => {
  // The other half of the contract: releasing an input a loader does not consume must
  // not swallow one it does. Before any load, the promised input is what the module
  // comes from, so its rejection has to surface as the structured failure, with the
  // caller's own error as the cause.
  publicGraph(`
    import assert from "node:assert/strict";
    import init from "opportune/web";

    const failure = new Error("the .wasm URL does not resolve");
    await assert.rejects(init(Promise.reject(failure)), (thrown) => {
      assert.equal(thrown.name, "OpportuneError");
      assert.equal(thrown.code, "wasm_unavailable");
      assert.equal(thrown.cause, failure, "the caller's own rejection is the cause");
      return true;
    });
  `, ["browser"]);
});

test("opportune/web and the root share initialization under default runtime conditions", () => {
  publicGraph(`
    import assert from "node:assert/strict";
    import { readFileSync } from "node:fs";
    import { initSync } from "opportune/web";
    import { Engine } from "opportune";

    const NativeModule = WebAssembly.Module;
    let compilations = 0;
    WebAssembly.Module = class extends NativeModule {
      constructor(bytes) {
        super(bytes);
        compilations += 1;
      }
    };
    const bytes = readFileSync(new URL("./dist/pkg/opportune_wasm_bg.wasm", import.meta.url));
    initSync({ module: bytes });
    assert.equal(compilations, 1, "the explicit initializer did not compile the supplied bytes");

    using engine = new Engine();
    assert.match(engine.tzdbVersion, /^\\d{4}[a-z]$/);
    assert.equal(compilations, 1, "the root initialized a second Wasm module behind opportune/web");
  `);
});

test("opportune/web's init shares the Node loader instead of fetching a URL", () => {
  publicGraph(`
    import assert from "node:assert/strict";
    import init, { initSync } from "opportune/web";
    import { Engine } from "opportune";

    // Under Node's default conditions, \`#wasm\` resolves to the packaged loader for
    // both \`opportune/web\` and \`opportune\`, so the web initializer is that loader's
    // explicit no-op rather than the browser loader's fetch. A URL argument must not be
    // fetched: there is no HTTP server on the path this case passes.
    await init("/a-node-init-must-not-fetch-this.wasm");
    initSync();
    using engine = new Engine();
    assert.match(engine.tzdbVersion, /^\\d{4}[a-z]$/);
  `);
});

test("the web loader loads once, retries after failure and never double-instantiates", async (t) => {
  const http = await server();
  t.after(() => http.close());

  await t.test("an engine call before init() names opportune/web", () => {
    assert.throws(
      () => wasmModule(),
      (thrown) => {
        assert.equal(thrown.name, "OpportuneError");
        assert.equal(thrown.code, "wasm_unavailable");
        assert.match(thrown.message, /opportune\/web/);
        assert.match(thrown.message, /init\(/);
        assert.match(thrown.message, /initSync/);
        return true;
      },
    );
  });

  await t.test("initSync() without a module refuses with guidance", () => {
    assert.throws(
      () => initSync(),
      (thrown) => {
        assert.equal(thrown.code, "wasm_unavailable");
        assert.match(thrown.message, /initSync\(\{ module \}\)/);
        assert.match(thrown.message, /init\(url\)/);
        return true;
      },
    );
  });

  await t.test("a failed load keeps its cause and leaves the loader retryable", async () => {
    await assert.rejects(
      init(`${http.origin}/missing.wasm`),
      (thrown) => {
        assert.equal(thrown.code, "wasm_unavailable");
        assert.ok(thrown.cause instanceof Error, "the runtime's own failure is the cause");
        return true;
      },
    );
    assert.throws(() => wasmModule(), { code: "wasm_unavailable" });
  });

  await t.test("concurrent init() calls share one load; a pending load refuses initSync()", async () => {
    const first = init(`${http.origin}/opportune_wasm_bg.wasm`);
    const second = init(`${http.origin}/opportune_wasm_bg.wasm`);
    assert.throws(
      () => initSync({ module: new Uint8Array() }),
      (thrown) => {
        assert.equal(thrown.code, "wasm_unavailable");
        assert.match(thrown.message, /already loading/);
        return true;
      },
    );
    await Promise.all([first, second]);
    assert.equal(http.requests(), 1, "two concurrent init() calls fetched the module twice");
  });

  await t.test("repeat init() and initSync() after success are no-ops", async () => {
    await init(`${http.origin}/opportune_wasm_bg.wasm`);
    initSync();
    initSync({ module: new Uint8Array() });
    assert.equal(http.requests(), 1, "a repeat init() fetched the module again");
    const module = wasmModule();
    const engine = new module.WasmEngine();
    try {
      assert.match(engine.tzdb_version(), /^\d{4}[a-z]$/);
    } finally {
      engine.free();
    }
  });
});

/**
 * Drives the shipped browser loader against glue built from an older surface.
 *
 * `registerHooks` replaces only `#wasm-glue`, the host boundary the loader imports; the
 * loader, its validation and its error translation are the published files. The stub
 * models wasm-bindgen's own early return: it instantiates once and then keeps the module
 * it produced, so a retry re-enters the loader but not the compiler — which is what makes
 * `instantiated` a real assertion about the second attempt.
 *
 * `entry` selects the loader initialiser under test; both share the same stub because
 * what is under test is the loader's refusal, not the glue's two spellings.
 */
function staleGlueProbe(entry) {
  const source = `
    import assert from "node:assert/strict";
    import { registerHooks } from "node:module";

    globalThis.__instantiated = 0;

    const glue = \`
      let wasm;
      let loading;
      export function initSync(options) {
        if (wasm !== undefined) return wasm;
        globalThis.__instantiated += 1;
        wasm = { stale: true };
        return wasm;
      }
      export default function init(input) {
        if (wasm !== undefined) return Promise.resolve(wasm);
        loading ??= Promise.resolve().then(() => {
          globalThis.__instantiated += 1;
          wasm = { stale: true };
          return wasm;
        });
        return loading;
      }
    \`;

    registerHooks({
      resolve(specifier, context, nextResolve) {
        if (specifier === "#wasm-glue") {
          return { url: "data:text/javascript," + encodeURIComponent(glue), shortCircuit: true };
        }
        return nextResolve(specifier, context);
      },
    });

    const loader = await import(${JSON.stringify(LOADER)});

    assert.throws(
      () => loader.wasmModule(),
      (thrown) => {
        assert.equal(thrown.code, "wasm_unavailable");
        assert.match(thrown.message, /not loaded/);
        return true;
      },
      "an unloaded browser realm answered an engine call before init",
    );

    const invoke = ${JSON.stringify(entry)} === "initSync"
      ? () => loader.initSync({ module: {} })
      : () => loader.init("https://example.test/opportune_wasm_bg.wasm");

    const messages = [];
    for (let attempt = 1; attempt <= 2; attempt += 1) {
      await assert.rejects(
        async () => { await invoke(); },
        (thrown) => {
          assert.equal(thrown.name, "OpportuneError", "attempt " + attempt);
          assert.equal(thrown.code, "wasm_unavailable", "attempt " + attempt);
          assert.equal(thrown.path, null, "attempt " + attempt);
          assert.deepEqual(thrown.errors, [], "attempt " + attempt);
          assert.match(thrown.message, /does not export WasmEngine/, "attempt " + attempt);
          assert.match(thrown.message, /older surface/, "attempt " + attempt);
          assert.match(thrown.message, /npm install opportune/, "attempt " + attempt);
          assert.match(thrown.message, /make wasm/, "attempt " + attempt);
          messages.push(thrown.message);
          return true;
        },
        "attempt " + attempt + " did not report the stale glue",
      );
      assert.throws(
        () => loader.wasmModule(),
        { code: "wasm_unavailable" },
        "attempt " + attempt + " cached the refused glue as loaded",
      );
    }

    assert.equal(messages[0], messages[1], "the two attempts reported different errors");
    assert.equal(
      globalThis.__instantiated,
      1,
      "the refused glue was instantiated more than once, so the retry did not take wasm-bindgen's early return",
    );
  `;

  const run = spawnSync(NODE, ["--input-type=module", "--eval", source], {
    cwd: PACKAGE,
    encoding: "utf8",
    timeout: 10_000,
  });
  const output = `${run.stdout ?? ""}${run.stderr ?? ""}`;
  assert.equal(run.error, undefined, `the browser-loader probe did not finish: ${run.error?.message}\n${output}`);
  assert.equal(run.status, 0, `the browser-loader probe failed:\n${output}`);
}

test("the browser loader refuses stale glue from initSync and retries it consistently", () => {
  staleGlueProbe("initSync");
});

test("the browser loader refuses stale glue from async init and retries it consistently", () => {
  staleGlueProbe("init");
});
