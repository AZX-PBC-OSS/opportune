import { exports } from "cloudflare:workers";
import { describe, expect, it } from "vitest";

import { VERSION } from "opportune";

import golden from "../../../../opportune-testkit/goldens/u1-europe-london.json" with { type: "json" };
import calendarView from "../../../../opportune-adapters/tests/fixtures/graph/calendar_view_ana.json" with { type: "json" };
import eventsList from "../../../../opportune-adapters/tests/fixtures/google/events_list_ana.json" with { type: "json" };

/** The Worker in `src/worker.ts`, addressed the way a request reaches it. */
function request(path: string, init?: RequestInit): Promise<Response> {
  return exports.default.fetch(new URL(path, "https://example.com"), init);
}

/** A POST with `document` as the JSON body. */
function post(path: string, document: unknown): Promise<Response> {
  return request(path, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(document),
  });
}

/**
 * The golden's answer with the recorded engine version replaced by this build's, so a
 * release bump is a version change rather than a false byte diff. The replacement is a
 * callback so the version is inserted literally: `String.replace` interprets `$`
 * sequences in a replacement string, and the version is data.
 */
function withCurrentEngine(expected: string): string {
  const recorded = (JSON.parse(expected) as { meta: { engine: string } }).meta.engine;
  return expected.replace(
    `"engine":${JSON.stringify(recorded)}`,
    () => `"engine":${JSON.stringify(VERSION)}`,
  );
}

/** The golden's response text, whatever shape the imported fixture holds it in. */
const goldenResponse =
  typeof golden.response === "string" ? golden.response : JSON.stringify(golden.response);

describe("opportune in Cloudflare Workers (workerd)", () => {
  it("reuses one isolate-scoped Engine across requests", async () => {
    const first = (await (await request("/meta")).json()) as {
      version: string;
      tzdbVersion: string;
      maxSearchWindowDays: number;
      requestCount: number;
    };
    const second = (await (await request("/meta")).json()) as {
      requestCount: number;
      tzdbVersion: string;
    };

    expect(first.version).toBe(VERSION);
    expect(first.tzdbVersion).toMatch(/^\d{4}[a-z]$/);
    expect(first.maxSearchWindowDays).toBeGreaterThan(0);
    expect(second.requestCount).toBe(first.requestCount + 1);
    expect(second.tzdbVersion).toBe(first.tzdbVersion);
  });

  it("answers the committed Rust golden byte for byte", async () => {
    const response = await post("/availability", golden.request);
    expect(response.status).toBe(200);
    const { engineJson, freeJson } = (await response.json()) as {
      engineJson: string;
      freeJson: string;
    };

    expect(engineJson).toBe(freeJson);
    expect(engineJson).toBe(withCurrentEngine(goldenResponse));
  });

  it("runs the Graph and Google adapters", async () => {
    const response = await post("/adapters", {
      calendarView,
      eventsList,
      zone: "America/New_York",
    });
    expect(response.status).toBe(200);
    const { graph, google } = (await response.json()) as {
      graph: { status: string; blocks?: unknown[] };
      google: { status: string; blocks?: unknown[] };
    };

    expect(graph.status).toBe("available");
    expect(graph.blocks?.length ?? 0).toBeGreaterThan(0);
    expect(google.status).toBe("available");
    expect(google.blocks?.length ?? 0).toBeGreaterThan(0);
  });

  it("answers inside a `using` block and releases through Symbol.dispose", async () => {
    const response = await post("/using", golden.request);
    expect(response.status).toBe(200);
    const { engineJson, disposed, code } = (await response.json()) as {
      engineJson: string;
      disposed: boolean;
      code?: string;
    };

    expect(engineJson).toBe(withCurrentEngine(goldenResponse));
    expect(disposed).toBe(true);
    expect(code).toBe("engine_disposed");
  });

  it("runs validate, requiredRanges, checkSlot and the preset builders", async () => {
    const response = await post("/api", golden.request);
    expect(response.status).toBe(200);
    const { diagnostics, requiredRanges, slotCheck, presets } = (await response.json()) as {
      diagnostics: string;
      requiredRanges: string;
      slotCheck: string;
      presets: {
        businessHours: { kind: string; params: { weekly: Record<string, string[]> } };
        person: { person: string };
        singleGoal: {
          parts: { duration: string; requirement: { all: { person: string }[] } }[];
          search_window: string[];
          zone: string;
        };
      };
    };

    const parsedDiagnostics = JSON.parse(diagnostics) as {
      errors: unknown[];
      warnings: unknown[];
    };
    expect(parsedDiagnostics.errors).toEqual([]);
    expect(Array.isArray(parsedDiagnostics.warnings)).toBe(true);

    const parsedRanges = JSON.parse(requiredRanges) as { calendar: string[] };
    expect(parsedRanges.calendar).toHaveLength(2);
    expect(parsedRanges.calendar[0]).toMatch(/Z$/);

    const parsedCheck = JSON.parse(slotCheck) as { reasons: unknown[]; valid: boolean };
    expect(parsedCheck.valid).toBe(true);
    expect(parsedCheck.reasons).toEqual([]);

    expect(presets.businessHours.kind).toBe("window");
    expect(presets.businessHours.params.weekly).toEqual({ "mon-fri": ["09:00-17:00"] });
    expect(presets.person).toEqual({ person: "rich" });
    expect(presets.singleGoal.zone).toBe("Europe/London");
    expect(presets.singleGoal.parts[0]?.duration).toBe("30m");
    expect(presets.singleGoal.parts[0]?.requirement.all).toEqual([{ person: "rich" }]);
  });

  it("reports a structured zone_unknown diagnostic", async () => {
    const document = structuredClone(golden.request);
    document.goal.display_zone = "Mars/Olympus_Mons";
    const response = await post("/unknown-zone", document);
    const error = (await response.json()) as {
      code: string;
      path: string | null;
      errors: { code: string; path: string | null }[];
    };

    expect(error.code).toBe("invalid_request");
    expect(error.errors).toContainEqual(
      expect.objectContaining({ code: "zone_unknown", path: "/goal/display_zone" }),
    );
  });

  it("raises engine_disposed after an explicit free", async () => {
    const response = await request("/dispose");
    expect(await response.json()).toEqual({ disposed: true, code: "engine_disposed" });
  });
});
