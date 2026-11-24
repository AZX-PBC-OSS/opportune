import { describe, expect, it } from "vitest";

import { Engine, OpportuneError } from "opportune";
import { init, initSync } from "opportune/web";
import wasmUrl from "opportune/opportune_wasm_bg.wasm?url";

/**
 * Cold-realm loader failures, in one test so no other test in the file can warm the
 * realm first: Vitest Browser Mode gives each file a fresh module graph, and the
 * order of tests inside a file is shuffled too.
 *
 * The recovery half proves a failed `init()` is never cached — the retry below succeeds
 * and the later `Engine` answers — while concurrent calls share one load.
 */
describe("opportune web loader state", () => {
  it("rejects engine use and empty initSync before init, then recovers from a failed init", async () => {
    expect(() => new Engine()).toThrowError(OpportuneError);
    try {
      new Engine();
    } catch (error) {
      expect(error).toBeInstanceOf(OpportuneError);
      expect((error as OpportuneError).code).toBe("wasm_unavailable");
      expect((error as OpportuneError).message).toContain("opportune/web");
    }

    try {
      initSync();
      expect.unreachable("initSync() without { module } must fail in a browser");
    } catch (error) {
      expect(error).toBeInstanceOf(OpportuneError);
      expect((error as OpportuneError).code).toBe("wasm_unavailable");
    }

    await expect(init("/definitely-missing-opportune.wasm")).rejects.toMatchObject({
      code: "wasm_unavailable",
    });

    // Concurrent init() calls share one load and recover cleanly after the failed attempt above.
    const realFetch = globalThis.fetch.bind(globalThis);
    let fetches = 0;
    globalThis.fetch = (input: RequestInfo | URL, init?: RequestInit) => {
      if (String(input).includes(".wasm")) fetches += 1;
      return realFetch(input, init);
    };
    try {
      await Promise.all([init(wasmUrl), init(wasmUrl)]);
      // Repeat init / initSync after load are clean no-ops.
      await init(wasmUrl);
      initSync();
    } finally {
      globalThis.fetch = realFetch;
    }
    expect(fetches).toBe(1);

    const engine = new Engine();
    try {
      expect(engine.tzdbVersion).toMatch(/^\d{4}[a-z]$/);
    } finally {
      engine.free();
    }
  });
});
