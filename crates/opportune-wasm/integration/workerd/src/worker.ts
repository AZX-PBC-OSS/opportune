import {
  Engine,
  OpportuneError,
  VERSION,
  checkSlotJson,
  defaultLimits,
  findAvailabilityJson,
  offerToProposal,
  requiredRangesJson,
  validateJson,
  type Request as OpportuneRequest,
} from "opportune";
import { calendarViewToBlocks } from "opportune/adapters/graph";
import { eventsListToBlocks } from "opportune/adapters/google";
import { businessHours, person, singleGoal } from "opportune/presets";

/** The isolate's engine: built once at module scope and reused by every request. */
const sharedEngine = new Engine();
let requestCount = 0;

/**
 * The Worker the Vitest integration drives over real requests.
 *
 * Each route is a small, production-shaped handler around the package's public API, so
 * the checks assert what a deployment would answer rather than internal module state.
 */
export default {
  async fetch(request: Request): Promise<Response> {
    const { pathname } = new URL(request.url);
    const body = request.method === "POST" ? await request.json() : undefined;

    if (pathname === "/meta") {
      requestCount += 1;
      return Response.json({
        version: VERSION,
        tzdbVersion: sharedEngine.tzdbVersion,
        maxSearchWindowDays: defaultLimits().max_search_window_days,
        requestCount,
      });
    }

    if (pathname === "/availability") {
      const document = body as OpportuneRequest;
      return Response.json({
        engineJson: sharedEngine.findAvailabilityJson(document),
        freeJson: findAvailabilityJson(document),
      });
    }

    if (pathname === "/adapters") {
      const { calendarView, eventsList, zone } = body as {
        calendarView: string | object;
        eventsList: string | object;
        zone: string;
      };
      return Response.json({
        graph: calendarViewToBlocks(calendarView, zone),
        google: eventsListToBlocks(eventsList, zone),
      });
    }

    if (pathname === "/using") {
      const document = body as OpportuneRequest;
      let engineJson: string;
      let released: Engine | undefined;
      {
        using engine = new Engine();
        engineJson = engine.findAvailabilityJson(document);
        released = engine;
      }
      if (released === undefined) {
        throw new Error("the `using` block never built an engine");
      }

      try {
        released.findAvailabilityJson(document);
        return Response.json({ engineJson, disposed: false });
      } catch (error) {
        if (error instanceof OpportuneError) {
          return Response.json({ engineJson, disposed: true, code: error.code });
        }
        throw error;
      }
    }

    if (pathname === "/api") {
      const document = body as OpportuneRequest;
      const answer = sharedEngine.findAvailability(document);
      const offer = answer.offers.at(0);
      if (offer === undefined) {
        throw new Error("the request produced no offer to check");
      }

      return Response.json({
        diagnostics: validateJson(document),
        requiredRanges: requiredRangesJson(document),
        slotCheck: checkSlotJson(document, offerToProposal(offer)),
        presets: {
          businessHours: businessHours(),
          person: person("rich"),
          singleGoal: singleGoal(
            "30m",
            { all: [person("rich")] },
            {
              zone: "Europe/London",
              searchWindow: ["2027-06-13T23:00Z", "2027-06-18T23:00Z"],
            },
          ),
        },
      });
    }

    if (pathname === "/unknown-zone") {
      try {
        const document = body as OpportuneRequest;
        return Response.json({ answered: sharedEngine.findAvailabilityJson(document) });
      } catch (error) {
        if (error instanceof OpportuneError) {
          return Response.json({ code: error.code, path: error.path, errors: error.errors });
        }
        throw error;
      }
    }

    if (pathname === "/dispose") {
      const disposable = new Engine();
      disposable.free();
      try {
        disposable.findAvailabilityJson({} as OpportuneRequest);
        return Response.json({ disposed: false });
      } catch (error) {
        if (error instanceof OpportuneError) {
          return Response.json({ disposed: true, code: error.code });
        }
        throw error;
      }
    }

    return new Response(null, { status: 404 });
  },
};
