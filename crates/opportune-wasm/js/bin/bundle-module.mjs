/**
 * Copies the built module, and the loader `wasm-pack` generated for each runtime
 * target, into `dist/`.
 *
 * wasm-pack writes its output to `pkg/` for the `web` target and `pkg-node/` for the
 * `nodejs` target, each with a `.gitignore` holding `*`, and npm honours a nested
 * `.gitignore` when it packs, so a manifest that listed either directory would ship no
 * module at all. The build copies what is published into `dist/pkg/`, inside the one
 * directory the manifest publishes.
 *
 * One module, two glues. Both targets compile the same crate and emit the same bytes —
 * the build fails here if they ever do not, because the one published `.wasm` is what
 * both loaders read — so the target's `opportune_wasm.js` is the whole of the
 * difference: `web` writes an ES module, `nodejs` a CommonJS one. The ESM glue keeps
 * its own name and extension, which is what makes the runtime parse it as ESM beside
 * this `"type": "module"` manifest; the CommonJS glue is published as `.cjs`, because
 * the extension is the only thing that says how to parse a file this package would
 * otherwise load as ESM.
 */

import { copyFile, mkdir, readFile } from "node:fs/promises";

/** The glue both targets name their output, and the module it loads. */
const GLUE = "opportune_wasm.js";
const WASM = "opportune_wasm_bg.wasm";

/** The CommonJS glue's published name; see the header. */
const NODE_GLUE = "opportune_wasm.cjs";

/** The source-checkout build, for a failure that means the tree was never built. */
const BUILD = "make wasm";

const web = new URL(`../pkg/${GLUE}`, import.meta.url);
const node = new URL(`../pkg-node/${GLUE}`, import.meta.url);
const wasm = new URL(`../pkg/${WASM}`, import.meta.url);
const fromNode = new URL(`../pkg-node/${WASM}`, import.meta.url);
const target = new URL("../dist/pkg/", import.meta.url);

try {
  // The `nodejs` target's glue instantiates whatever `.wasm` sits beside it, so a
  // module that differed from the one published here would be a loader reading bytes
  // this build did not produce. Both targets compile one crate, so this holds; the
  // check is what keeps it true if a future target stops emitting identical bytes.
  const [published, built] = await Promise.all([readFile(wasm), readFile(fromNode)]);
  if (!published.equals(built)) {
    console.error(
      "bundle-module: `wasm-pack` built different modules for the `web` and `nodejs` targets, " +
        "and the package publishes one `.wasm` for both loaders to read. " +
        `Rebuild with \`${BUILD}\`.`,
    );
    process.exit(1);
  }
  await mkdir(target, { recursive: true });
  await copyFile(web, new URL(GLUE, target));
  await copyFile(node, new URL(NODE_GLUE, target));
  await copyFile(wasm, new URL(WASM, target));
} catch (cause) {
  console.error(
    `bundle-module: the WASM module is missing from pkg/ or pkg-node/: ${cause.message}. ` +
      `Build it first with \`${BUILD}\`.`,
  );
  process.exit(1);
}
