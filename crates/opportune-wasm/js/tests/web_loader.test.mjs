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
 * The tests are one sequential case because the loader's state is module state: the
 * pre-init checks have to run before anything loads, the failed load has to come before
 * the retry that succeeds, and the retry leaves the module loaded for the rest.
 *
 * ```sh
 * make test-wasm
 * ```
 */

import assert from "node:assert/strict";
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
