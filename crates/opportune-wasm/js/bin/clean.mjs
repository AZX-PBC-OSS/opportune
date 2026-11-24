/**
 * Removes the build output directory before the build writes it again.
 *
 * The steps after it only ever write: `tsc` emits the files `src/` compiles to and
 * `bundle-module` copies the module it is handed, so a file left in `dist/` by an
 * older build — a renamed module's `.cjs` glue, a stub a later build stopped
 * emitting — survives both and packs into the tarball, with every gate green on a
 * tree the source no longer describes. Deleting the directory first is the only
 * step that makes "the build emits everything `dist/` holds" true by construction.
 *
 * ```sh
 * node bin/clean.mjs && tsc -p tsconfig.build.json && node bin/bundle-module.mjs
 * ```
 */

import { rmSync } from "node:fs";

rmSync(new URL("../dist/", import.meta.url), { recursive: true, force: true });
