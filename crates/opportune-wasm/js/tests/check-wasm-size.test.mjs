/**
 * The size gate's own contract: green inside the committed budget, red on
 * everything it cannot measure or cannot match.
 *
 * `scripts/check-wasm-size.mjs` is the only thing standing between a release
 * `.wasm` that has grown — or one that is not the module the build wrote — and a
 * consumer who downloads it, so every branch that decides "within budget" has to be
 * exercised rather than trusted: the module over its ceiling, the published copy that
 * drifted from the build output, the source the budget names but cannot be read, a
 * budget document with no ceiling in it, and a path that holds something that is not
 * a WebAssembly module at all. A gate that reports a byte count for a file that is not
 * the thing it was built to measure is worse than no gate, because its green is the only
 * signal anyone reads.
 *
 * Each case runs the real script in a throwaway package root beside its own budget, its
 * own build output and its own published copy, so the committed
 * `wasm-size-budget.json` is read for its shape and its measured paths without the test
 * depending on `pkg/` or `dist/` having been built first.
 *
 * ```sh
 * npm --prefix crates/opportune-wasm/js test
 * ```
 */

import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { randomBytes } from "node:crypto";
import { copyFileSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import test from "node:test";

/** The gate this suite exercises, in the package it belongs to. */
const PACKAGE = join(import.meta.dirname, "..");
const SCRIPT = join(PACKAGE, "scripts", "check-wasm-size.mjs");

/** The committed budget: the paths and the ceiling the release is measured by. */
const BUDGET = JSON.parse(readFileSync(join(PACKAGE, "wasm-size-budget.json"), "utf8"));

/** The four bytes every WebAssembly module begins with. */
const WASM_MAGIC = Buffer.from([0x00, 0x61, 0x73, 0x6d]);

/** A module-shaped artefact of `size` bytes: the magic, then incompressible filler. */
function moduleOf(size) {
  return Buffer.concat([WASM_MAGIC, randomBytes(Math.max(0, size - WASM_MAGIC.length))]);
}

/** Writes `module` at `path` under `root`, creating the directories it needs. */
function write(root, path, module) {
  if (typeof path !== "string" || path === "") {
    // A budget that states no path is the case under test: it has nothing to write to.
    return;
  }
  const file = join(root, path);
  mkdirSync(dirname(file), { recursive: true });
  writeFileSync(file, module);
}

/**
 * A package root holding the gate beside the budget and the two files it reads.
 *
 * `module` is written at the budget's published path and at its source path, which is
 * what a build writes; `source` replaces the source copy alone (`null` leaves it out),
 * which is how a published tree drifts from a build output that is not there. Either is
 * left out when it is `undefined`. `budget` replaces the committed document when given.
 */
function sandbox({ module, source = module, budget = BUDGET } = {}) {
  const root = mkdtempSync(join(tmpdir(), "opportune-size-"));
  mkdirSync(join(root, "scripts"));
  copyFileSync(SCRIPT, join(root, "scripts", "check-wasm-size.mjs"));
  writeFileSync(join(root, "wasm-size-budget.json"), JSON.stringify(budget));
  if (source !== undefined && source !== null) {
    write(root, budget.source, source);
  }
  if (module !== undefined) {
    write(root, budget.artifact, module);
  }
  return root;
}

/** The gate's exit status and output, run in `root` exactly as the hook runs it. */
function gate(root) {
  const run = spawnSync(process.execPath, [join(root, "scripts", "check-wasm-size.mjs")], {
    encoding: "utf8",
  });
  assert.equal(run.error, undefined, `the gate did not start: ${run.error?.message}`);
  return { status: run.status, stdout: run.stdout, stderr: run.stderr };
}

/** Runs `call` against a fresh sandbox root, removing the root afterwards. */
function inSandbox(spec, call) {
  const root = sandbox(spec);
  try {
    return call(root);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
}

test("a module inside the committed budget passes and is reported", () => {
  inSandbox({ module: moduleOf(1024) }, (root) => {
    const { status, stdout } = gate(root);
    assert.equal(status, 0, stdout);
    assert.match(stdout, new RegExp(BUDGET.artifact.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")));
    assert.match(stdout, /gzipped at level 9/, "the gate says what it measured");
    assert.match(
      stdout,
      new RegExp(`is the bytes of ${BUDGET.source.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")}`),
      "the gate says the published copy is the build's own bytes",
    );
  });
});

test("an artefact past its ceiling fails and names the overflow", () => {
  // Random filler does not compress, so the gzipped size is the raw size: a
  // 64 KiB module cannot slip under a 1 KiB ceiling however forgiving gzip is.
  inSandbox({ module: moduleOf(64 * 1024), budget: { ...BUDGET, budgetBytes: 1024 } }, (root) => {
    const { status, stderr } = gate(root);
    assert.equal(status, 1, "an over-budget module is a failed gate");
    assert.match(stderr, /over its 1024-byte budget/);
  });
});

test("a published copy that is not the build's bytes fails closed", () => {
  // The published module is the build output copied into `dist/`, so a difference
  // means the gate measured one module and the tarball would ship another — a stale
  // `dist/`, or a build interrupted between the copy's two steps. Both are inside the
  // budget and instantiable, so only this comparison can tell them apart.
  inSandbox({ module: moduleOf(1024), source: moduleOf(2048) }, (root) => {
    const { status, stderr } = gate(root);
    assert.equal(status, 1, "a published copy that drifted from the build is a failed gate");
    assert.match(stderr, /is not the module/);
    assert.match(stderr, /npm run build/, "the failure names the step that copies them");
  });
});

test("a source the build wrote but cannot be read fails closed", () => {
  // Without the build output there is nothing to compare the published copy against,
  // and a gate that skipped the comparison would pass on whichever module `dist/`
  // happened to hold.
  inSandbox({ module: moduleOf(1024), source: null }, (root) => {
    const { status, stderr } = gate(root);
    assert.equal(status, 1);
    assert.match(stderr, new RegExp(`${BUDGET.source} cannot be read`));
    assert.match(stderr, /wasm-pack build/);
  });
});

test("an artefact that is not a WebAssembly module fails closed", () => {
  // The path the budget names holding a file that is not the module is the case a
  // byte count cannot tell apart from a very small release: an interrupted build
  // leaves a zero-length file where the module goes, and a stale placeholder or an
  // error page saved to the same path looks the same. Weighing those as "within
  // budget" would certify an artefact no consumer can instantiate.
  for (const [label, module] of [
    ["an empty file", Buffer.alloc(0)],
    ["text", Buffer.from("<html><body>404 Not Found</body></html>\n")],
  ]) {
    inSandbox({ module }, (root) => {
      const { status, stderr } = gate(root);
      assert.equal(status, 1, `${label} is not a module, so it cannot pass the gate`);
      assert.match(stderr, /not a WebAssembly module/);
      assert.match(stderr, /wasm-pack build/, "the failure names the build that writes it");
    });
  }
});

test("a missing artefact fails closed and names the build", () => {
  // The build output is there and the published copy is not: the case a checkout that
  // only ran `wasm-pack` leaves behind, and the one this gate exists for — a published
  // tree with no module in it must not be reported as "nothing to measure".
  inSandbox({ source: moduleOf(1024) }, (root) => {
    const { status, stderr } = gate(root);
    assert.equal(status, 1);
    assert.match(stderr, new RegExp(`${BUDGET.artifact} cannot be read`));
    assert.match(stderr, /wasm-pack build/);
  });
  // And with neither file present, the first one the gate looks for is the one it
  // names, so the failure is the same one a fresh clone meets.
  inSandbox({}, (root) => {
    const { status, stderr } = gate(root);
    assert.equal(status, 1);
    assert.match(stderr, /cannot be read/);
    assert.match(stderr, /wasm-pack build/);
  });
});

test("a budget with no ceiling or no paths fails closed rather than defaulting", () => {
  // A budget document that lost its ceiling is the one edit that would silently
  // turn the gate off, so it is read as a failure rather than as "no limit"; a budget
  // that lost a path would measure nothing or compare nothing.
  const { budgetBytes, ...withoutCeiling } = BUDGET;
  inSandbox({ module: moduleOf(1024), budget: withoutCeiling }, (root) => {
    const { status, stderr } = gate(root);
    assert.equal(status, 1);
    assert.match(stderr, /budgetBytes/);
  });
  for (const field of ["artifact", "source"]) {
    const { [field]: _dropped, ...withoutPath } = BUDGET;
    inSandbox({ module: moduleOf(1024), budget: withoutPath }, (root) => {
      const { status, stderr } = gate(root);
      assert.equal(status, 1, `a budget without \`${field}\` cannot pass`);
      assert.match(stderr, new RegExp(field));
    });
  }
});
