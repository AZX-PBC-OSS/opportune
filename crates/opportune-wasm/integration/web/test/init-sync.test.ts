import { describe, expect, it } from "vitest";

import { Engine, OpportuneError } from "opportune";
import { initSync } from "opportune/web";
import wasmUrl from "opportune/opportune_wasm_bg.wasm?url";

/**
 * `initSync({ module })` on its own cold realm, so a warm `init()` in another test
 * cannot make this pass without exercising the precompiled-module path. The bytes are
 * fetched and compiled with `WebAssembly.compile`, then handed over as the module the
 * caller precompiled.
 */
describe("opportune web initSync", () => {
  it("instantiates a precompiled WebAssembly.Module synchronously", async () => {
    // The realm has to be cold for the case to mean anything: a loader another test had
    // already warmed would turn `initSync` into its documented no-op and this would pass
    // without ever exercising the precompiled-module path. Assert it here, in this file,
    // rather than relying on the runner's per-file isolation to be the only evidence.
    let coldRealm: unknown;
    try {
      new Engine();
    } catch (error) {
      coldRealm = error;
    }
    expect(coldRealm).toBeInstanceOf(OpportuneError);
    expect((coldRealm as OpportuneError).code).toBe("wasm_unavailable");

    const response = await fetch(new URL(wasmUrl, window.location.href).href);
    expect(response.ok).toBe(true);
    const module = await WebAssembly.compile(await response.arrayBuffer());

    initSync({ module });

    const engine = new Engine();
    try {
      expect(engine.tzdbVersion).toMatch(/^\d{4}[a-z]$/);
    } finally {
      engine.free();
    }
  });
});
