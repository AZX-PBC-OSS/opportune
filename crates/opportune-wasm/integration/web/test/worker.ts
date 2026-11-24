import { Engine, OpportuneError, findAvailabilityJson } from "opportune";
import { init } from "opportune/web";

self.onmessage = async (event: MessageEvent<{ wasmUrl: string; request: unknown }>) => {
  try {
    const { wasmUrl, request } = event.data;
    await init(wasmUrl);
    using engine = new Engine();
    const doc = request as Parameters<Engine["findAvailabilityJson"]>[0];
    self.postMessage({
      ok: true,
      engineJson: engine.findAvailabilityJson(doc),
      freeJson: findAvailabilityJson(doc),
      tzdbVersion: engine.tzdbVersion,
    });
  } catch (error) {
    self.postMessage({
      ok: false,
      code: error instanceof OpportuneError ? error.code : "unknown",
      message: error instanceof Error ? error.message : String(error),
    });
  }
};
