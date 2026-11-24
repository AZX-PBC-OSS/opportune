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

import { wasmModule, init, initSync } from "../dist/wasm-web.js";

/** The built module the server answers with, and the path it answers on. */
const WASM = join(import.meta.dirname, "..", "dist", "pkg", "opportune_wasm_bg.wasm");

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
    { cwd: join(import.meta.dirname, ".."), encoding: "utf8", timeout: 10_000 },
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
