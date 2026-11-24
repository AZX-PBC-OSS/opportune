import { Engine, OpportuneError, findAvailabilityJson } from "opportune";
import { init } from "opportune/web";

/** What a page sends the Worker: the module URL and one engine request document. */
interface WorkerRequest {
  wasmUrl: string;
  request: unknown;
}

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
 * The slice of `DedicatedWorkerGlobalScope` this entry uses.
 *
 * The harness compiles with the DOM library because the page-side suite needs it, so the
 * WebWorker globals are not in the program. Naming the members here keeps this file
 * typed without adding a second global library that would conflict with DOM.
 */
interface WorkerScope {
  onmessage: ((event: MessageEvent<WorkerRequest>) => void) | null;
  postMessage(message: WorkerReply): void;
}

const scope = self as unknown as WorkerScope;

/**
 * A rejection's message with the pointers it carries. `OpportuneError.message` states the
 * code and the sentence; the field a request went wrong at lives on the finding, so the
 * page receives both rather than only the sentence.
 */
function failureMessage(error: OpportuneError): string {
  const pointers = [
    error.path,
    ...error.errors.map((finding) => finding.path),
  ].filter((path): path is string => path !== null);
  const unique = [...new Set(pointers)];
  return unique.length === 0 ? error.message : `${error.message} (at ${unique.join(", ")})`;
}

scope.onmessage = async (event) => {
  const { wasmUrl, request } = event.data;
  try {
    await init(wasmUrl);
    const doc = request as Parameters<Engine["findAvailabilityJson"]>[0];

    // Everything the reply reports is computed inside the block. The reply is posted
    // after the block exits, so `[Symbol.dispose]` has run before the page resolves.
    let engineJson: string;
    let freeJson: string;
    let tzdbVersion: string;
    let released: Engine | undefined;
    {
      using engine = new Engine();
      engineJson = engine.findAvailabilityJson(doc);
      freeJson = findAvailabilityJson(doc);
      tzdbVersion = engine.tzdbVersion;
      released = engine;
    }
    if (released === undefined) {
      throw new Error("the `using` block never built an engine");
    }

    // Probe the released engine so the page observes that dispose really ran; the
    // package answers `engine_disposed` rather than touching its released handle.
    let disposedCode = "not_disposed";
    try {
      released.findAvailabilityJson(doc);
    } catch (error) {
      if (error instanceof OpportuneError) {
        disposedCode = error.code;
      } else {
        throw error;
      }
    }

    scope.postMessage({ ok: true, engineJson, freeJson, tzdbVersion, disposedCode });
  } catch (error) {
    scope.postMessage({
      ok: false,
      code: error instanceof OpportuneError ? error.code : "unknown",
      message:
        error instanceof OpportuneError
          ? failureMessage(error)
          : error instanceof Error
            ? error.message
            : String(error),
    });
  }
};
