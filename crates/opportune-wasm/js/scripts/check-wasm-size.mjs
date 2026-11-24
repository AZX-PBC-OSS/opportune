/**
 * The gzipped-size gate for the release `.wasm` a consumer downloads.
 *
 * Two files are involved and both are named in `wasm-size-budget.json`. `source` is
 * what `wasm-pack` writes into `pkg/` for the `web` target; `artifact` is the copy
 * inside `dist/`, which is the tree the manifest publishes and the one both generated
 * loaders instantiate from — so it is the copy whose bytes a consumer pays for, and the
 * copy this gate measures. wasm-pack writes `pkg/` with a `.gitignore` holding `*`,
 * which npm honours, so the published module reaches a consumer only through that copy;
 * a gate measuring `pkg/` would certify a file the tarball does not carry.
 *
 * Because the two are copies of one build, the gate also proves they are the same
 * bytes. A `dist/` left over from an earlier build, or a build interrupted between the
 * two steps, is otherwise a module measured and a different module shipped, and the
 * gate's green is the only signal anyone reads.
 *
 * The budget is what a consumer downloads: the `-Oz` optimised module, gzipped at the
 * highest level, which is what a server or CDN applies. The ceiling is committed in
 * `wasm-size-budget.json` rather than carried over from the previous run, so raising it
 * is a reviewable diff instead of a ratchet nothing remembers. `baselineBytes` beside it
 * is advice rather than a check: the gate prints it next to the measurement and never
 * compares the two, so growth that stays under the ceiling is visible in that printed
 * line and nowhere else.
 *
 * The gate fails closed: a missing artefact, a file that is not a WebAssembly module, a
 * missing or malformed budget, two files that differ, or a module over budget all exit
 * non-zero. A size check that passed because it could not find what it was measuring
 * would be worse than no check at all — and a zero-length or placeholder file at the
 * artefact's path weighs less than any budget while being nothing a consumer can
 * instantiate.
 *
 * ```sh
 * make wasm && node crates/opportune-wasm/js/scripts/check-wasm-size.mjs
 * ```
 */

import { readFileSync } from "node:fs";
import { join, resolve, sep } from "node:path";
import { gzipSync } from "node:zlib";

/** The package root: this script lives in `scripts/`. */
const ROOT = join(import.meta.dirname, "..");

/** The committed budget, seen from the package root. */
const BUDGET = "wasm-size-budget.json";

/** The gzip level the budget is recorded at: the highest. */
const LEVEL = 9;

/** The commands that write both files, in the order that makes them copies. */
const BUILD =
  "wasm-pack build crates/opportune-wasm --target web --out-dir js/pkg --release --locked && " +
  "wasm-pack build crates/opportune-wasm --target nodejs --out-dir js/pkg-node --release --locked && " +
  "npm --prefix crates/opportune-wasm/js run build";

/** The four bytes every WebAssembly module begins with: the wasm magic `\0asm`. */
const MAGIC = Buffer.from([0x00, 0x61, 0x73, 0x6d]);

/** `value` as a whole number of bytes, or the failure naming the field. */
function byteCount(value, field) {
  if (!Number.isSafeInteger(value) || value <= 0) {
    throw new Error(`${BUDGET}: \`${field}\` must be a positive whole number of bytes, not ${value}`);
  }
  return value;
}

/** `value` as the path the budget states, or the failure naming the field. */
function path(value, field) {
  if (typeof value !== "string" || value === "") {
    throw new Error(`${BUDGET}: \`${field}\` must name a file the gate reads`);
  }
  return value;
}

/**
 * `value` as the path the budget states, resolved under `directory` — or the failure
 * naming the field. A budget is a committed file a reviewer reads, and a path in it
 * decides which file is measured and therefore what the gate certifies: an absolute
 * path, a `..` segment or a directory prefix that only looks like `dist/` would let
 * the gate measure something this package neither builds nor ships. The comparison is
 * on the resolved path plus a separator, so `dist-other/` does not pass as `dist/`.
 */
function under(value, field, directory) {
  const base = resolve(ROOT, directory);
  const target = resolve(ROOT, path(value, field));
  if (!target.startsWith(base + sep)) {
    throw new Error(
      `${BUDGET}: \`${field}\` must name a file under \`${directory}/\`, not ${value}`,
    );
  }
  return value;
}

/**
 * The committed budget: the artefact it measures, the build output it is a copy of,
 * the recorded baseline and the ceiling.
 */
function budget() {
  let document;
  try {
    document = JSON.parse(readFileSync(join(ROOT, BUDGET), "utf8"));
  } catch (cause) {
    throw new Error(`${BUDGET} cannot be read: ${cause.message}`, { cause });
  }
  const artifact = under(document.artifact, "artifact", "dist");
  const source = under(document.source, "source", "pkg");
  // The gate proves the two are the same bytes, so one path standing for both makes
  // that comparison a tautology and leaves the copy it exists to check unread.
  if (artifact === source) {
    throw new Error(
      `${BUDGET}: \`artifact\` and \`source\` must name different files; ` +
        "they are the published copy and the build output it is a copy of",
    );
  }
  return {
    artifact,
    source,
    baselineBytes: byteCount(document.baselineBytes, "baselineBytes"),
    budgetBytes: byteCount(document.budgetBytes, "budgetBytes"),
  };
}

/** The module's bytes at `file`, or the failure naming the build that writes them. */
function moduleAt(file) {
  let module;
  try {
    module = readFileSync(join(ROOT, file));
  } catch (cause) {
    throw new Error(`${file} cannot be read: ${cause.message}. Build it with \`${BUILD}\`.`, {
      cause,
    });
  }
  if (!module.subarray(0, MAGIC.length).equals(MAGIC)) {
    throw new Error(
      `${file} is not a WebAssembly module: ${bytes(module.length)} with no ` +
        "`\\0asm` header, so there is no module to measure. " +
        `Build it with \`${BUILD}\`.`,
    );
  }
  // The magic is four bytes a truncated or corrupt file keeps, so the header alone
  // says nothing a consumer can instantiate. Validation is what the loading runtime
  // does with the bytes, so it is what decides "a module" here.
  if (!WebAssembly.validate(module)) {
    throw new Error(
      `${file} is not a WebAssembly module: ${bytes(module.length)} carrying the ` +
        "`\\0asm` header but bytes no runtime validates, so there is no module to " +
        `measure. Build it with \`${BUILD}\`.`,
    );
  }
  return module;
}

/**
 * The failure the two files raise when they are not one build's bytes, or `undefined`
 * when they are.
 */
function drift(artifact, source, published, built) {
  if (published.equals(built)) {
    return undefined;
  }
  return (
    `${artifact} is not the module ${source} holds: ${bytes(published.length)} published, ` +
    `${bytes(built.length)} built. \`npm run build\` copies the one to the other, so the ` +
    `published tree holds a module the release did not build. Rebuild both with \`${BUILD}\`.`
  );
}

/** The count as a byte size a person reads. */
function bytes(count) {
  return `${count} B (${(count / 1024).toFixed(1)} KiB)`;
}

try {
  const committed = budget();
  const built = moduleAt(committed.source);
  const published = moduleAt(committed.artifact);

  const difference = drift(committed.artifact, committed.source, published, built);
  if (difference !== undefined) {
    throw new Error(difference);
  }

  const gzipped = gzipSync(published, { level: LEVEL }).length;

  console.log(
    `${committed.artifact}: ${bytes(published.length)} raw, ${bytes(gzipped)} gzipped at level ${LEVEL}`,
  );
  console.log(
    `${BUDGET}: ${bytes(committed.budgetBytes)}, recorded baseline ${bytes(committed.baselineBytes)}`,
  );
  console.log(`${committed.artifact} is the bytes of ${committed.source}`);

  if (gzipped > committed.budgetBytes) {
    console.error(
      `the gzipped release module is ${gzipped - committed.budgetBytes} bytes over its ` +
        `${committed.budgetBytes}-byte budget. Shrink the module, or raise the budget in ` +
        `${BUDGET} if the growth is intended and reviewed.`,
    );
    process.exitCode = 1;
  }
} catch (cause) {
  console.error(cause.message);
  process.exitCode = 1;
}
