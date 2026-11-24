import { describe, expect, it } from "vitest";

import { Engine } from "opportune";
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
