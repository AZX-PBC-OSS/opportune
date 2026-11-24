import {
  Engine,
  OpportuneError,
  VERSION,
  defaultLimits,
  findAvailabilityJson,
} from "opportune";
import { calendarViewToBlocks } from "opportune/adapters/graph";
import { eventsListToBlocks } from "opportune/adapters/google";

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
      const document = body as Parameters<Engine["findAvailabilityJson"]>[0];
      return Response.json({
        engineJson: sharedEngine.findAvailabilityJson(document),
        freeJson: findAvailabilityJson(document),
      });
    }

    if (pathname === "/adapters") {
      const { calendarView, eventsList, zone } = body as {
        calendarView: unknown;
        eventsList: unknown;
        zone: string;
      };
      return Response.json({
        graph: calendarViewToBlocks(calendarView, zone),
        google: eventsListToBlocks(eventsList, zone),
      });
    }

    if (pathname === "/unknown-zone") {
      try {
        const document = body as Parameters<Engine["findAvailabilityJson"]>[0];
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
        disposable.findAvailabilityJson({} as Parameters<Engine["findAvailabilityJson"]>[0]);
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
