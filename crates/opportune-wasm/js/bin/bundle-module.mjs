/**
 * Copies the wasm-pack output into `dist/`, where the package's loader reads it.
 *
 * wasm-pack writes its module to `pkg/` with a `.gitignore` holding `*`, and npm
 * honours a nested `.gitignore` when it packs, so a manifest that listed `pkg/`
 * would ship no module at all. The build copies the two files the loader needs into
 * `dist/pkg/`, inside the one directory the manifest publishes — and names the
 * module `.cjs`, because this package is `"type": "module"` and the extension is
 * what tells Node to read it as the CommonJS that wasm-pack wrote. The module finds
 * its own `.wasm` through `__dirname`, so the pair stays together.
 */

import { copyFile, mkdir } from "node:fs/promises";

/** The module wasm-pack writes, and the file it is published under. */
const MODULE = ["opportune_wasm.js", "opportune_wasm.cjs"];

/** The compiled module the copied code instantiates. */
const WASM = "opportune_wasm_bg.wasm";

const source = new URL("../pkg/", import.meta.url);
const target = new URL("../dist/pkg/", import.meta.url);

try {
  await mkdir(target, { recursive: true });
  await copyFile(new URL(MODULE[0], source), new URL(MODULE[1], target));
  await copyFile(new URL(WASM, source), new URL(WASM, target));
} catch (cause) {
  console.error(
    "bundle-module: the WASM module is missing from pkg/. Build it first with " +
      "`wasm-pack build crates/opportune-wasm --target nodejs --out-dir js/pkg --release`: " +
      cause.message,
  );
  process.exit(1);
}
