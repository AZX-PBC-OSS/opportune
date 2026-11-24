import { describe, expect, it } from "vitest";
import {
  Engine,
  OpportuneError,
  VERSION,
  defaultLimits,
  findAvailabilityJson,
} from "opportune";
import { calendarViewToBlocks } from "opportune/adapters/graph";
import { eventsListToBlocks } from "opportune/adapters/google";
import { init, initSync } from "opportune/web";
import wasmUrl from "opportune/opportune_wasm_bg.wasm?url";
import golden from "../../../../opportune-testkit/goldens/u1-europe-london.json" with { type: "json" };
import calendarView from "../../../../opportune-adapters/tests/fixtures/graph/calendar_view_ana.json" with { type: "json" };
import eventsList from "../../../../opportune-adapters/tests/fixtures/google/events_list_ana.json" with { type: "json" };

/**
 * The golden's answer with the recorded engine version replaced by this build's, so a
 * release bump is a version change rather than a false byte diff in the browser lane.
 */
function withCurrentEngine(expected: string): string {
  const recorded = (JSON.parse(expected) as { meta: { engine: string } }).meta.engine;
  return expected.replace(
    `"engine":${JSON.stringify(recorded)}`,
    `"engine":${JSON.stringify(VERSION)}`,
  );
}

describe("opportune in browsers and Web Workers", () => {
  it("rejects engine use and empty initSync before init, and recovers after a failed init", async () => {
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
  });

  it("matches the committed Rust golden byte-for-byte, runs adapters, and reports structured errors", async () => {
    await init(wasmUrl);
    const engine = new Engine();
    try {
      expect(VERSION).toMatch(/^\d+\.\d+\.\d+/);
      expect(engine.tzdbVersion.length).toBeGreaterThan(0);
      expect(defaultLimits().max_search_window_days).toBeGreaterThan(0);

      const req = golden.request as Parameters<Engine["findAvailabilityJson"]>[0];
      const engineJson = engine.findAvailabilityJson(req);
      const freeJson = findAvailabilityJson(req);
      const expected = withCurrentEngine(
        typeof golden.response === "string" ? golden.response : JSON.stringify(golden.response),
      );
      expect(engineJson).toBe(freeJson);
      expect(engineJson).toBe(expected);

      const graph = calendarViewToBlocks(calendarView, "America/New_York");
      const google = eventsListToBlocks(eventsList, "America/New_York");
      expect(graph.blocks.length).toBeGreaterThan(0);
      expect(google.blocks.length).toBeGreaterThan(0);

      const badZone = structuredClone(golden.request);
      badZone.goal.display_zone = "Mars/Olympus_Mons";
      try {
        engine.findAvailabilityJson(badZone as Parameters<Engine["findAvailabilityJson"]>[0]);
        expect.unreachable("unknown zone must fail");
      } catch (error) {
        expect(error).toBeInstanceOf(OpportuneError);
        const err = error as OpportuneError;
        expect(err.code).toBe("invalid_request");
        expect(err.errors.some((e) => e.code === "zone_unknown" && e.path === "/goal/display_zone")).toBe(true);
      }
    } finally {
      engine.free();
    }

    expect(() =>
      engine.findAvailabilityJson({} as Parameters<Engine["findAvailabilityJson"]>[0]),
    ).toThrowError(
      expect.objectContaining({ code: "engine_disposed" }),
    );
  });

  it("executes inside a real module Web Worker under the worker condition", async () => {
    const worker = new Worker(new URL("./worker.ts", import.meta.url), { type: "module" });
    try {
      const answer = await new Promise<{
        ok: boolean;
        engineJson?: string;
        freeJson?: string;
        tzdbVersion?: string;
        message?: string;
      }>((resolve, reject) => {
        worker.onmessage = (event) => resolve(event.data);
        worker.onerror = (event) => reject(new Error(event.message || "worker error"));
        worker.postMessage({
          wasmUrl: new URL(wasmUrl, window.location.href).href,
          request: golden.request,
        });
      });

      expect(answer.ok, answer.message).toBe(true);
      const expected = withCurrentEngine(
        typeof golden.response === "string" ? golden.response : JSON.stringify(golden.response),
      );
      expect(answer.engineJson).toBe(answer.freeJson);
      expect(answer.engineJson).toBe(expected);
      expect(answer.tzdbVersion?.length).toBeGreaterThan(0);
    } finally {
      worker.terminate();
    }
  });
});
