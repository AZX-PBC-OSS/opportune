/**
 * The documentation snippet gate's TypeScript lane, and the red-team case that
 * proves it bites.
 *
 * `make check-docs-snippets` classifies every `python` and `ts` fence in README.md
 * and docs/, and each runtime's lane checks its own language against the
 * package a reader installs; `crates/opportune-py/bin/doc_snippets.py` is the one
 * driver, and `make test-wasm` runs the TypeScript lane over the committed pages.
 * This file is the red-team half: it runs the same lane over a copy with one
 * snippet made stale — a zone that resolves in no database, which compiles and
 * fails only when the block runs, so the case proves the execution half rather
 * than only `tsc`.
 *
 * The copy links the repository's `crates/` tree so the recorded provider fixtures
 * and the built `dist/` are the real ones; the driver needs `python3`, which the
 * runner images and this repository's other gates already require.
 *
 * ```sh
 * node --test crates/opportune-wasm/js/tests/doc-snippets.test.mjs
 * ```
 */

import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { copyFileSync, cpSync, mkdirSync, mkdtempSync, readFileSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";

/** This suite sits at `crates/opportune-wasm/js/tests`, so the root is four levels up. */
const ROOT = join(import.meta.dirname, "..", "..", "..", "..");
const DRIVER = join(ROOT, "crates/opportune-py/bin/doc_snippets.py");

/** Run the real driver over `root`, capturing both streams. */
function gate(root, ...arguments_) {
  return spawnSync("python3", [DRIVER, "--root", root, ...arguments_], { encoding: "utf8" });
}

test("a stale TypeScript snippet fails the TypeScript lane", () => {
  const root = mkdtempSync(join(tmpdir(), "opportune-doc-snippets-"));
  try {
    cpSync(join(ROOT, "docs"), join(root, "docs"), { recursive: true });
    copyFileSync(join(ROOT, "README.md"), join(root, "README.md"));
    symlinkSync(join(ROOT, "crates"), join(root, "crates"), "dir");
    const page = join(root, "docs/wasm.md");
    const text = readFileSync(page, "utf8");
    // Type-valid and semantically stale: `singleGoal` accepts any zone string, so
    // only running the block can notice that no database resolves this one.
    const stale = text.replace('zone: "Europe/London",', 'zone: "Mars/Olympus",');
    assert.notEqual(stale, text, "docs/wasm.md no longer carries the zone this case mutates");
    writeFileSync(page, stale);
    const result = gate(root, "--typescript");
    const output = `${result.stdout}${result.stderr}`;
    assert.match(result.stdout ?? "", /classified fences/);
    assert.notEqual(result.status, 0, "the TypeScript lane accepted a stale snippet");
    // The execution half failed on the intended snippet, not classification or tsc.
    assert.match(output, /`wasm-ts-quickstart` failed with status/);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test("a TypeScript snippet that stops type-checking fails the TypeScript lane", () => {
  const root = mkdtempSync(join(tmpdir(), "opportune-doc-snippets-"));
  try {
    cpSync(join(ROOT, "docs"), join(root, "docs"), { recursive: true });
    copyFileSync(join(ROOT, "README.md"), join(root, "README.md"));
    symlinkSync(join(ROOT, "crates"), join(root, "crates"), "dir");
    const page = join(root, "docs/wasm.md");
    const text = readFileSync(page, "utf8");
    // Type-only rot: the value still runs, so only the strict compile catches it.
    const stale = text.replace(
      "const limits = limitsWith({ max_search_window_days: 120 });",
      'const limits: string = limitsWith({ max_search_window_days: 120 });',
    );
    assert.notEqual(stale, text, "docs/wasm.md no longer carries the limits block this case mutates");
    writeFileSync(page, stale);
    const result = gate(root, "--typescript");
    const output = `${result.stdout}${result.stderr}`;
    assert.notEqual(result.status, 0, "the TypeScript lane accepted a snippet tsc rejects");
    assert.match(output, /tsc over the TypeScript snippets failed/);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test("a fence tagged with the language's full name is classified", () => {
  const root = mkdtempSync(join(tmpdir(), "opportune-doc-snippets-"));
  try {
    cpSync(join(ROOT, "docs"), join(root, "docs"), { recursive: true });
    copyFileSync(join(ROOT, "README.md"), join(root, "README.md"));
    symlinkSync(join(ROOT, "crates"), join(root, "crates"), "dir");
    const page = join(root, "docs/wasm.md");
    const text = readFileSync(page, "utf8");
    // `typescript` is the same language as `ts`: classification is the behavior
    // under test, so this case does not need a full compiler/execution run.
    const retagged = text.replace(
      "<!-- snippet: wasm-ts-quickstart -->\n```ts\n",
      "<!-- snippet: wasm-ts-quickstart -->\n```typescript\n",
    );
    assert.notEqual(retagged, text, "docs/wasm.md no longer opens u1 with a bare `ts` fence");
    writeFileSync(page, retagged);
    const result = gate(root);
    assert.equal(result.status, 0, result.stderr);
    assert.match(result.stdout ?? "", /classified fences/);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test("a stale TypeScript result assertion fails the TypeScript lane", () => {
  const root = mkdtempSync(join(tmpdir(), "opportune-doc-snippets-"));
  try {
    cpSync(join(ROOT, "docs"), join(root, "docs"), { recursive: true });
    copyFileSync(join(ROOT, "README.md"), join(root, "README.md"));
    symlinkSync(join(ROOT, "crates"), join(root, "crates"), "dir");
    const page = join(root, "docs/recipes.md");
    const text = readFileSync(page, "utf8");
    // The expected offer count is executable, so changing the documented answer
    // fails even when the request and all of its types remain valid.
    const stale = text.replace(
      'if (offers.length !== 54) throw new Error(`expected 54 offers, got ${offers.length}`);',
      'if (offers.length !== 55) throw new Error(`expected 55 offers, got ${offers.length}`);',
    );
    assert.notEqual(stale, text, "docs/recipes.md no longer carries the booth assertion");
    writeFileSync(page, stale);
    const result = gate(root, "--typescript");
    const output = `${result.stdout}${result.stderr}`;
    assert.notEqual(result.status, 0, "the TypeScript lane accepted a stale result assertion");
    assert.match(output, /`booth-ts` failed with status/);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test("a TypeScript snippet cannot end the lane with success", () => {
  const root = mkdtempSync(join(tmpdir(), "opportune-doc-snippets-"));
  try {
    cpSync(join(ROOT, "docs"), join(root, "docs"), { recursive: true });
    copyFileSync(join(ROOT, "README.md"), join(root, "README.md"));
    symlinkSync(join(ROOT, "crates"), join(root, "crates"), "dir");
    const page = join(root, "docs/wasm.md");
    const text = readFileSync(page, "utf8");
    // A script-style example may end with `process.exit(0)`: the process would
    // report success, so the lane has to notice the block never returned.
    const anchor = "console.log(text);                   // and the same answer as the engine's own bytes";
    const stale = text.replace(anchor, `${anchor}\nprocess.exit(0);`);
    assert.notEqual(stale, text, "docs/wasm.md no longer carries the line this case mutates");
    writeFileSync(page, stale);
    const result = gate(root, "--typescript");
    const output = `${result.stdout}${result.stderr}`;
    assert.notEqual(result.status, 0, "a snippet that ended the process with zero passed");
    assert.match(output, /`wasm-ts-quickstart` ended the process/);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test("the npm package's README is one of the pages the gate reads", () => {
  // The published README is the page a consumer copies first, so its TypeScript blocks
  // are gated beside the repository's own. A page the driver does not read at all is
  // silent: classification counts only the pages it globs, so the manifest naming this
  // page is not enough to prove its fences are checked. Removing a marker from a copy
  // is the case that does: the block is either read and reported as unclassified, or
  // not read at all and the gate passes.
  const root = mkdtempSync(join(tmpdir(), "opportune-doc-snippets-"));
  try {
    cpSync(join(ROOT, "docs"), join(root, "docs"), { recursive: true });
    copyFileSync(join(ROOT, "README.md"), join(root, "README.md"));
    // Only what classification reads: the recorded provider payloads the manifest
    // names, and a copy of the page itself, which this case mutates.
    mkdirSync(join(root, "crates/opportune-wasm/js"), { recursive: true });
    symlinkSync(
      join(ROOT, "crates/opportune-adapters"),
      join(root, "crates/opportune-adapters"),
      "dir",
    );
    const page = join(root, "crates/opportune-wasm/js/README.md");
    copyFileSync(join(ROOT, "crates/opportune-wasm/js/README.md"), page);
    const text = readFileSync(page, "utf8");
    const stale = text.replace("<!-- snippet: npm-ts-quickstart -->\n", "");
    assert.notEqual(stale, text, "the npm README no longer carries the marker this case mutates");
    writeFileSync(page, stale);
    const result = gate(root);
    const output = `${result.stdout}${result.stderr}`;
    assert.notEqual(result.status, 0, "a `.ts` block in the published README went unchecked");
    assert.match(
      output,
      /crates\/opportune-wasm\/js\/README\.md:\d+: a `ts` block has no `<!-- snippet: id -->`/,
    );
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});
