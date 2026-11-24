import { describe, expect, it } from "vitest";

import {
  Engine,
  OpportuneError,
  VERSION,
  checkSlot,
  defaultLimits,
  findAvailability,
  findAvailabilityJson,
  offerToProposal,
  requiredRanges,
  validate,
  type Request as OpportuneRequest,
} from "opportune";
import { calendarViewToBlocks } from "opportune/adapters/graph";
import { eventsListToBlocks } from "opportune/adapters/google";
import { businessHours, person, singleGoal } from "opportune/presets";
import { init } from "opportune/web";
import wasmUrl from "opportune/opportune_wasm_bg.wasm?url";

import golden from "../../../../opportune-testkit/goldens/u1-europe-london.json" with { type: "json" };
import calendarView from "../../../../opportune-adapters/tests/fixtures/graph/calendar_view_ana.json" with { type: "json" };
import eventsList from "../../../../opportune-adapters/tests/fixtures/google/events_list_ana.json" with { type: "json" };

/**
 * The golden's answer with the recorded engine version replaced by this build's, so a
 * release bump is a version change rather than a false byte diff in the browser lane.
 * The replacement is a callback so the version is inserted literally: `String.replace`
 * interprets `$` sequences in a replacement string, and the version is data.
 */
function withCurrentEngine(expected: string): string {
  const recorded = (JSON.parse(expected) as { meta: { engine: string } }).meta.engine;
  return expected.replace(
    `"engine":${JSON.stringify(recorded)}`,
    () => `"engine":${JSON.stringify(VERSION)}`,
  );
}

// The fixture's JSON types arrays as `string[]`; the wire's own type keeps the search
// window a two-element span. The cast is the boundary between a checked-in document and
// the type the engine declares, and the engine re-validates it at run time.
const goldenRequest = golden.request as unknown as OpportuneRequest;
const goldenResponse =
  typeof golden.response === "string" ? golden.response : JSON.stringify(golden.response);
const expectedGolden = withCurrentEngine(goldenResponse);

/** What the Worker answers: one result, or the failure the engine raised. */
type WorkerReply =
  | {
      ok: true;
      engineJson: string;
      freeJson: string;
      tzdbVersion: string;
      disposedCode: string;
    }
  | { ok: false; code: string; message: string };

/**
 * Sends one request to the module Worker and resolves with its reply. A message the page
 * cannot deserialize is a test failure, not a hang, so `onmessageerror` rejects.
 */
function ask(worker: Worker, request: unknown): Promise<WorkerReply> {
  return new Promise((resolve, reject) => {
    worker.onmessage = (event: MessageEvent<WorkerReply>) => resolve(event.data);
    worker.onmessageerror = () =>
      reject(new Error("the page could not deserialize the Web Worker's reply"));
    worker.onerror = (event) => reject(new Error(event.message || "the Web Worker failed"));
    worker.postMessage({ wasmUrl: new URL(wasmUrl, window.location.href).href, request });
  });
}

/**
 * Every test starts by awaiting `init(wasmUrl)`, so none depends on a sibling having
 * warmed the realm: the suite passes under `--sequence.shuffle` and `--retry=1`.
 */
describe("opportune in browsers and Web Workers", () => {
  it("matches the committed Rust golden byte for byte", async () => {
    await init(wasmUrl);
    const engine = new Engine();
    try {
      expect(VERSION).toMatch(/^\d+\.\d+\.\d+/);
      expect(engine.tzdbVersion).toMatch(/^\d{4}[a-z]$/);
      expect(defaultLimits().max_search_window_days).toBeGreaterThan(0);

      const engineJson = engine.findAvailabilityJson(goldenRequest);
      expect(engineJson).toBe(findAvailabilityJson(goldenRequest));
      expect(engineJson).toBe(expectedGolden);
    } finally {
      engine.free();
    }
  });

  it("validates, reports required ranges, checks a golden offer and builds presets", async () => {
    await init(wasmUrl);

    const diagnostics = validate(goldenRequest);
    expect(diagnostics.errors).toEqual([]);
    expect(Array.isArray(diagnostics.warnings)).toBe(true);

    const ranges = requiredRanges(goldenRequest);
    expect(ranges.calendar).toHaveLength(2);
    expect(ranges.calendar[0]).toMatch(/Z$/);

    const offer = findAvailability(goldenRequest).offers.at(0);
    expect(offer).toBeDefined();
    if (offer === undefined) {
      throw new Error("the golden request produced no offer to check");
    }
    const check = checkSlot(goldenRequest, offerToProposal(offer));
    expect(check.valid).toBe(true);
    expect(check.reasons).toEqual([]);

    const hours = businessHours();
    expect(hours.kind).toBe("window");
    expect(hours.params.weekly).toEqual({ "mon-fri": ["09:00-17:00"] });
    expect(person("rich")).toEqual({ person: "rich" });
    const goal = singleGoal(
      "30m",
      { all: [person("rich")] },
      { zone: "Europe/London", searchWindow: ["2027-06-13T23:00Z", "2027-06-18T23:00Z"] },
    );
    expect(goal.zone).toBe("Europe/London");
    expect(goal.parts[0]?.duration).toBe("30m");
    expect(goal.parts[0]?.requirement).toEqual({ all: [{ person: "rich" }] });
  });

  it("runs the Graph and Google adapters", async () => {
    await init(wasmUrl);

    const graph = calendarViewToBlocks(calendarView, "America/New_York");
    const google = eventsListToBlocks(eventsList, "America/New_York");
    expect(graph.status).toBe("available");
    expect(google.status).toBe("available");
    if (graph.status !== "available" || google.status !== "available") {
      throw new Error("the fixtures are both convertible calendars");
    }
    expect(graph.blocks.length).toBeGreaterThan(0);
    expect(google.blocks.length).toBeGreaterThan(0);
  });

  it("reports zone_unknown and engine_disposed through OpportuneError", async () => {
    await init(wasmUrl);

    const badZone = structuredClone(golden.request);
    badZone.goal.display_zone = "Mars/Olympus_Mons";
    try {
      findAvailabilityJson(badZone as unknown as OpportuneRequest);
      expect.unreachable("an unknown zone must fail");
    } catch (error) {
      expect(error).toBeInstanceOf(OpportuneError);
      const err = error as OpportuneError;
      expect(err.code).toBe("invalid_request");
      expect(
        err.errors.some((e) => e.code === "zone_unknown" && e.path === "/goal/display_zone"),
      ).toBe(true);
    }

    const engine = new Engine();
    engine.free();
    try {
      engine.findAvailabilityJson({} as OpportuneRequest);
      expect.unreachable("a freed engine must fail");
    } catch (error) {
      expect(error).toBeInstanceOf(OpportuneError);
      expect((error as OpportuneError).code).toBe("engine_disposed");
    }
  });

  it("executes inside a real module Web Worker", async () => {
    const worker = new Worker("/.vitest/worker.js", { type: "module" });
    try {
      const answer = await ask(worker, golden.request);
      expect(answer.ok, answer.ok ? undefined : answer.message).toBe(true);
      if (!answer.ok) {
        throw new Error(answer.message);
      }
      expect(answer.engineJson).toBe(answer.freeJson);
      expect(answer.engineJson).toBe(expectedGolden);
      expect(answer.tzdbVersion).toMatch(/^\d{4}[a-z]$/);
      expect(answer.disposedCode).toBe("engine_disposed");

      const badBody = structuredClone(golden.request);
      badBody.goal.display_zone = "Mars/Olympus_Mons";
      const rejected = await ask(worker, badBody);
      expect(rejected.ok).toBe(false);
      if (rejected.ok) {
        throw new Error("an unknown zone must fail inside the Worker too");
      }
      expect(rejected.code).toBe("invalid_request");
      expect(rejected.message).toContain("/goal/display_zone");
    } finally {
      worker.terminate();
    }
  });
});
