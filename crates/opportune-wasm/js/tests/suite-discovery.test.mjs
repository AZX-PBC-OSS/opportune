/**
 * One suite, two runners, and the names both of them find.
 *
 * Each lane calls its runner over this directory rather than naming files: `node --test
 * tests/` through `npm test`, and `bun test crates/opportune-wasm/js/tests/`. The two
 * runners do not agree on every name. Measured on Node 26.13.0 and Bun 1.4.2, Node
 * discovers `*.test.*`, `*-test.*` and `*_test.*`; Bun discovers `*.test.*`, `*_test.*`,
 * `*.spec.*` and `*_spec.*` but not `*-test.*` — its own "tests need `.test`, `_test_`,
 * `.spec` or `_spec_`" note understates what it matches, since `name_test.mjs` runs under
 * both.
 *
 * A case file named for one runner therefore runs under that one alone —
 * `extra.spec.mjs` under Bun, `extra-test.mjs` under Node — and neither lane fails for
 * it. The gate would report green while the case it was added for went untested on the
 * other declared runtime. A name neither runner matches is the same silence from the
 * other side: it runs nowhere. So this directory holds case files only, every one of
 * them named for both runners, and a module that declares no cases — a helper, a
 * fixture loader — belongs in a subdirectory, whose names neither runner matches
 * either.
 *
 * The reported counts are not comparable across the runners, so this gate does not
 * compare them: Bun executes nested `t.test` subtests but reports only the parent, and
 * `web_loader.test.mjs` therefore reports five fewer tests under Bun while running the
 * same cases. A failing subtest still fails its parent under both runners. What matters
 * for a case that is present at all is its name, which is what this gate reads.
 *
 * The walk below is the suite root only. A case file moved into a subdirectory would not
 * be read here, which is why the directory stays flat and helpers live one level down.
 */

import assert from "node:assert/strict";
import { readdirSync } from "node:fs";
import test from "node:test";

/** The directory each lane points its runner at. */
const SUITE = import.meta.dirname;

test("every file in the suite is discovered by both runners", () => {
  // The two naming shapes both runners agree on: `name.test.mjs` and `name_test.mjs`.
  // A `name.spec.mjs` file is Bun-only and a `name-test.mjs` file is Node-only.
  const bothRunners = /(?:\.test|_test)\.[a-z]+$/;
  const invisible = readdirSync(SUITE, { withFileTypes: true })
    .filter((entry) => entry.isFile())
    .map((entry) => entry.name)
    .filter((name) => !bothRunners.test(name));
  assert.deepEqual(
    invisible,
    [],
    "these files are at the top level of the suite directory and are not named for " +
      "both runners, so at most one of them runs the cases inside: rename a case file " +
      "to <name>.test.mjs, and move a module that declares no cases into a subdirectory",
  );
});
