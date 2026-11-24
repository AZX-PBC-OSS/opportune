/**
 * The package fails legibly when the module it was built beside cannot be loaded.
 *
 * `docs/wasm.md` tells a consumer to build the package from a checkout, and the
 * loader's own contract is that a caller who has not — or whose build was interrupted
 * while writing the `.wasm` — gets an `OpportuneError` naming what a consumer does
 * about it, not the raw `ENOENT` or `CompileError` the runtime raised underneath. That
 * is the failure a caller meets before any `try`/`catch` of theirs can help, so it is
 * the one place where "it works under Node" is not enough: the two runners word a bad
 * module differently (Node: "expected magic word 00 61 73 6d", Bun: "doesn't parse at
 * byte 0"), and only this package's translation makes them one legible failure with
 * the cause kept. A Bun consumer must not be the first to discover the difference.
 *
 * Importing the package is not one of those failures. The module loads on the first
 * call that needs it, so a package with no loadable module still imports — for its
 * version, its marker constants, its preset builders and its generated types — and
 * raises the structured failure on the first engine call, which is where a caller
 * can catch it. The presets subpath is the extreme case of the same rule: it loads no
 * module at all, so it builds rules out of a package that has none. Its purity has a
 * second half that neither the Node lane nor the Bun lane can see at runtime — both
 * provide every Node built-in, so a stray `node:` import in that graph would resolve
 * here and fail in the browser bundle the subpath exists for — and the last case here
 * reads the shipped graph itself to keep it that way.
 *
 * Each case copies the published `dist/` and the manifest beside it into a temporary
 * directory and damages the module there, so the checked-in build every other case
 * answers from is never touched. The manifest travels with the tree because the
 * loader is selected by `package.json#imports`: a copy without it has no `#wasm` to
 * resolve, which is a different failure from the one these cases are about.
 *
 * Build the package first:
 *
 * ```sh
 * make wasm && npm --prefix crates/opportune-wasm/js test
 * ```
 */

import assert from "node:assert/strict";
import { cpSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { isBuiltin } from "node:module";
import { dirname, join } from "node:path";
import test from "node:test";
import { pathToFileURL } from "node:url";

/** The package root this suite belongs to, and the built tree the tarball packs. */
const PACKAGE = join(import.meta.dirname, "..");
const DIST = join(PACKAGE, "dist");

/** The glue the Node and Bun loader resolves, and the module it instantiates. */
const GLUE = join("pkg", "opportune_wasm.cjs");
const MODULE = join("pkg", "opportune_wasm_bg.wasm");

/** What a caller does about a module that did not load: reinstall the published package. */
const REPAIR = /npm install opportune/;

/**
 * The other repair, for the caller whose package is a source checkout. The monorepo's
 * own `wasm-pack` invocation is not either of them, so a consumer is never sent to a
 * command that does not exist in their tree.
 */
const REBUILD = /make wasm/;

/**
 * A copy of `dist/` in a temporary directory, after `damage` runs on it.
 *
 * The copy's own modules are what the cases import, so its loader reads the copy's
 * module beside it rather than the checked-in one.
 */
async function damaged(damage, check) {
  const dir = mkdtempSync(join(tmpdir(), "opportune-unbuilt-"));
  try {
    cpSync(DIST, join(dir, "dist"), { recursive: true });
    cpSync(join(PACKAGE, "package.json"), join(dir, "package.json"));
    damage(join(dir, "dist"));
    return await check((name) => import(pathToFileURL(join(dir, "dist", name)).href), join(dir, "dist"));
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
}

/** The package with its compiled module removed, as a checkout nobody built. */
function unbuilt(dir) {
  rmSync(join(dir, MODULE));
}

/**
 * The shape and the repair guidance every module failure answers with, whatever the
 * runtime said underneath. `instanceof` is deliberately not used: this is the copy's
 * class, and what a caller catches is the shape, not identity with the checked-in
 * build's.
 */
function assertFailure(error, what) {
  assert.equal(error?.name, "OpportuneError", `${what} surfaced as: ${error?.stack ?? error}`);
  assert.equal(error.code, "wasm_unavailable", `${what} carried code ${String(error.code)}`);
  assert.equal(error.path, null, `${what}: a load failure is not a document field`);
  assert.deepEqual(error.errors, [], `${what}: a load failure carries no diagnostics`);
  assert.match(error.message, REPAIR, `${what}: the message names the consumer's repair`);
  assert.match(error.message, REBUILD, `${what}: the message names the source-checkout rebuild`);
}

/** A module the runtime could not load, whose own failure has to survive as the cause. */
function assertLegible(error, what) {
  assertFailure(error, what);
  assert.match(error.message, /did not load/, `${what}: the message must say what failed`);
  assert.ok(error.cause instanceof Error, `${what}: the module's own failure is kept as the cause`);
}

/** The error `call` threw, failing the case when it did not throw one. */
function thrown(call) {
  try {
    call();
  } catch (error) {
    return error;
  }
  assert.fail("the first engine call imported a package with no loadable module");
}

/**
 * The import specifiers `text` names, as written.
 *
 * Only module syntax is read: a statement line has to begin with `import`/`export` and
 * name its target after `from`, a side-effect import has to begin with `import`, or a
 * dynamic `import(…)` supplies one. Anything else in the file — an `export const` whose
 * value is a string, a specifier written inside a doc comment — is a value or a mention,
 * not an import, and the built files carry both.
 */
function specifiers(text) {
  const named = [];
  for (const match of text.matchAll(
    /^[ \t]*(?:import|export)\s[^\n]*?\bfrom\s*["']([^"']+)["']|^[ \t]*import\s*["']([^"']+)["']|import\(\s*["']([^"']+)["']\s*\)/gm,
  )) {
    named.push(match[1] ?? match[2] ?? match[3]);
  }
  return named;
}

/**
 * Every module `entry` reaches through relative imports, as `{ file, specifiers }`.
 *
 * The walk starts at the shipped file a consumer imports and follows only what it
 * actually reaches, so a module that lives in `dist/` but is not on this path — the
 * loader, the adapters, the web loader — is not counted as part of it.
 */
function graph(entry) {
  const reached = new Map();
  const pending = [entry];
  while (pending.length > 0) {
    const file = pending.pop();
    if (reached.has(file)) {
      continue;
    }
    const named = specifiers(readFileSync(file, "utf8"));
    reached.set(file, named);
    for (const specifier of named) {
      if (specifier.startsWith(".")) {
        pending.push(join(dirname(file), specifier));
      }
    }
  }
  return reached;
}

test("a package whose module was never built imports, and fails on the first engine call", async () => {
  await damaged(unbuilt, async (load) => {
    // Everything a bundle can use without the engine is still there: the import is a
    // module graph, not a call.
    const pkg = await load("index.js");
    assert.equal(typeof pkg.VERSION, "string", "the pure constants still answer");
    assert.equal(typeof pkg.businessHours, "function", "the presets still answer");
    assert.equal(typeof pkg.offerStart, "function", "the sugar still answers");

    const error = thrown(() => new pkg.Engine());
    assertLegible(error, "an absent module");
    assert.match(
      error.message,
      /opportune_wasm_bg\.wasm/,
      "the message names the artefact that is missing, not only the loader beside it",
    );
    // A package that never loads an engine never learns it has no module, so the
    // failure is the engine's to raise and not the import's.
    assert.equal(typeof pkg.defaultLimits, "function");
  });
});

test("a bundled copy that kept the glue but dropped the module names the bundler repair", async () => {
  // A bundler that copies `dist/pkg/opportune_wasm.cjs` but not the `.wasm` beside it
  // reaches the generated loader and fails there, so the runtime's own failure is an
  // `ENOENT` naming `opportune_wasm_bg.wasm` rather than one that could not be resolved.
  // Reinstalling and rebuilding both succeed and change nothing about that tree, so the
  // bundler note is the only repair a consumer can act on — and the runtime's cause is
  // still what a log shows underneath.
  await damaged(unbuilt, async (load) => {
    const pkg = await load("index.js");
    const error = thrown(() => new pkg.Engine());
    assertLegible(error, "a bundled copy without its module");
    assert.equal(
      error.cause?.code,
      "ENOENT",
      `the runtime's own failure is the cause, not ${String(error.cause?.code)}`,
    );
    assert.match(error.message, /opportune_wasm_bg\.wasm/, "the message names the artefact that is missing");
    assert.match(
      error.message,
      /serverExternalPackages/,
      "the message names the Next.js repair for a bundled copy",
    );
    assert.match(error.message, /`external: \[\s*"opportune"\s*\]`/, "the message names the bundler repair");
  });
});

test("an unrelated ENOENT keeps the plain repair rather than the bundler advice", async () => {
  // Only the glue's own missing artefact is a bundling failure. An `ENOENT` a consumer's
  // deployment meets for some other file is not repaired by keeping `opportune` external,
  // so the note must not be attached to the code alone.
  await damaged(
    (dir) =>
      writeFileSync(
        join(dir, GLUE),
        'const error = new Error("ENOENT: no such file or directory, open \'/srv/config.json\'");\n' +
          'error.code = "ENOENT";\n' +
          "throw error;\n",
      ),
    async (load) => {
      const pkg = await load("index.js");
      const error = thrown(() => new pkg.Engine());
      assertLegible(error, "an unrelated missing file");
      assert.doesNotMatch(
        error.message,
        /serverExternalPackages/,
        "a file this package does not ship is not a reason to keep it external",
      );
    },
  );
});

test("a module that is not WebAssembly fails the same way on first use, keeping the runtime's cause", async () => {
  await damaged(
    (dir) => writeFileSync(join(dir, MODULE), "not a wasm module"),
    async (load) => {
      const pkg = await load("index.js");
      const error = thrown(() => new pkg.Engine());
      assertLegible(error, "a truncated module");
      // The runners word this one differently, which is accepted: the cause is where a
      // reader learns what the runtime made of the bytes, and it has to survive.
      assert.equal(error.cause.constructor.name, "CompileError");
    },
  );
});

test("a module from an older surface names the export it lacks and the repair", async () => {
  // A `pkg/` left behind by an earlier build loads as a module object and exports the
  // wrong surface, which is the drift the loader's single export check exists for:
  // without it the call fails several frames deep with "not a constructor". The message
  // has to name the export, say the surface is old, and give the same repair the load
  // failures give — the wording a consumer acts on is one string.
  await damaged(
    (dir) => writeFileSync(join(dir, GLUE), "module.exports = {};\n"),
    async (load) => {
      const pkg = await load("index.js");
      const error = thrown(() => new pkg.Engine());
      assertFailure(error, "a module from an older surface");
      assert.match(error.message, /does not export WasmEngine/, "the message names the missing export");
      assert.match(error.message, /older surface/, "the message says why the export is missing");
      assert.equal(error.cause, undefined, "nothing underneath was raised, so nothing is attached");
    },
  );
});

test("a load that failed is retried, so a repaired install answers on the next call", async () => {
  // The module is cached only once it loaded. A package whose glue is absent fails on
  // the first engine call, and a caller who reinstalls or rebuilds — which is exactly
  // what the failure tells them to do — must get a working engine from the same
  // process: caching the failure would make a repaired package need a restart.
  await damaged(
    (dir) => rmSync(join(dir, GLUE)),
    async (load, dir) => {
      const pkg = await load("index.js");
      assertLegible(thrown(() => new pkg.Engine()), "a missing glue");

      cpSync(join(DIST, GLUE), join(dir, GLUE));
      using engine = new pkg.Engine();
      assert.equal(typeof engine.tzdbVersion, "string", "the repaired install answers");
    },
  );
});

test("stale glue is retried after repair rather than served from the require cache", async () => {
  // This failure happens after require succeeded, so unlike a missing or corrupt module
  // the runtime has a completed CommonJS evaluation it can cache. Retryability covers
  // this validation failure too: replacing the stale glue must not leave the process
  // permanently reading the old export object.
  await damaged(
    (dir) => writeFileSync(join(dir, GLUE), "module.exports = {};\n"),
    async (load, dir) => {
      const pkg = await load("index.js");
      assertFailure(thrown(() => new pkg.Engine()), "stale glue");

      cpSync(join(DIST, GLUE), join(dir, GLUE));
      using engine = new pkg.Engine();
      assert.equal(typeof engine.tzdbVersion, "string", "the repaired glue answers");
    },
  );
});

test("a module that failed inside the generated glue is retried after repair", async () => {
  // The generated CommonJS glue reads and compiles the binary during require. That is
  // a later failure than an absent glue, and runtimes must not retain its partial module
  // in the require cache: the package deliberately caches successes only, so a repaired
  // binary has to be reachable without restarting the process.
  await damaged(unbuilt, async (load, dir) => {
    const pkg = await load("index.js");
    assertLegible(thrown(() => new pkg.Engine()), "a missing module");

    cpSync(join(DIST, MODULE), join(dir, MODULE));
    using engine = new pkg.Engine();
    assert.equal(typeof engine.tzdbVersion, "string", "the repaired module answers");
  });
});

test("the presets subpath builds rules out of a package with no module at all", async () => {
  // The `opportune/presets` declaration is a promise: pure TypeScript, no WASM. A
  // package whose module is absent is the strongest proof of it — with an eager loader
  // this import would fail, and with one imported indirectly it would fail at the first
  // preset.
  await damaged(unbuilt, async (load) => {
    const presets = await load("presets-entry.js");
    const goal = presets.singleGoal("30m", { all: [presets.person("rich")] }, {
      zone: "Europe/London",
      searchWindow: ["2027-06-13T23:00Z", "2027-06-18T23:00Z"],
    });
    assert.deepEqual(goal.parts, [{ duration: "30m", requirement: { all: [{ person: "rich" }] } }]);
    assert.deepEqual(presets.businessHours("mon-fri", "09:00-17:00").params.weekly, {
      "mon-fri": ["09:00-17:00"],
    });
    assert.deepEqual(
      presets.weeklyHours({ mon: "09:00-17:00" }, { dates: { "2027-12-25": [] } }),
      {
        id: "weekly_hours",
        kind: "window",
        params: { zone: "participant", weekly: { mon: ["09:00-17:00"] }, dates: { "2027-12-25": [] } },
      },
      "the pure subpath builds the per-weekday rule",
    );
    assert.equal(presets.offerStart({ events: [{ start: "2027-06-14T09:00Z", end: "2027-06-14T10:00Z" }] }),
      "2027-06-14T09:00Z");

    // The constants a bundle stamps onto its own tags or asks a provider for are here
    // for the same reason: reading one must reach no module at all, which this package
    // — one whose module was never built — is the proof of.
    assert.equal(presets.MARKER_TAGS, "opportune.tags");
    assert.equal(presets.MARKER_EVENT_KEY, "opportune.event_key");
    assert.match(presets.EXTENDED_PROPERTY_EXPAND, /^singleValueExtendedProperties\(\$filter=/);
    assert.equal(typeof presets.VERSION, "string", "the version is stated, not computed");

    // A bundle that calls a builder has to answer the failure it raises, and the class
    // is here for exactly that: `instanceof` against the subpath's own export, with
    // nothing resolved from the engine-backed entry that refuses its runtime.
    const raised = (() => {
      try {
        presets.businessHours("mon-fri", null);
      } catch (error) {
        return error;
      }
      return assert.fail("a wall-clock range of `null` built a rule");
    })();
    assert.equal(typeof presets.OpportuneError, "function", "`opportune/presets` exports the error class");
    assert.ok(
      raised instanceof presets.OpportuneError,
      `a preset rejection is the subpath's own ${String(raised?.constructor?.name)}`,
    );
  });
});

test("the adapters' constants answer from a package whose module was never built", async () => {
  // The marker names and the Graph `$expand` value are strings a caller reads once to
  // build one provider request, and reading them used to instantiate the whole module:
  // an import of `opportune/adapters/graph` for `EXTENDED_PROPERTY_EXPAND` paid 1.5 MB
  // of WebAssembly — and, in a package whose module was never built, failed outright.
  // They are stated in a module that imports nothing, so this package answers them and
  // still fails legibly on the first conversion, which is the call that needs a module.
  await damaged(unbuilt, async (load) => {
    const graph = await load("adapters/graph.js");
    const google = await load("adapters/google.js");

    assert.equal(graph.MARKER_TAGS, "opportune.tags");
    assert.equal(graph.MARKER_EVENT_KEY, "opportune.event_key");
    assert.equal(google.MARKER_TAGS, graph.MARKER_TAGS);
    assert.equal(google.MARKER_EVENT_KEY, graph.MARKER_EVENT_KEY);
    assert.match(graph.EXTENDED_PROPERTY_NAMESPACE_GUID, /^[0-9a-f-]{36}$/);
    assert.match(graph.EXTENDED_PROPERTY_TAGS_ID, /Name opportune\.tags$/);
    assert.match(graph.EXTENDED_PROPERTY_EVENT_KEY_ID, /Name opportune\.event_key$/);
    assert.match(graph.EXTENDED_PROPERTY_EXPAND, /^singleValueExtendedProperties\(\$filter=/);

    // The loader is still on this module graph, and this is the call that needs it: a
    // constant read must not have been the thing that loaded — or failed to load — the
    // module, but a conversion still has to say why it cannot run.
    assertLegible(thrown(() => graph.calendarViewToBlocks({ value: [] }, "UTC")), "a conversion");
  });
});

test("the presets module graph names no Node built-in and no bare specifier", () => {
  // The `opportune/presets` promise has two halves: no WebAssembly (the case above
  // proves it from a package whose module is absent) and a graph a browser bundle can
  // resolve from the package alone. The second half is the one no lane can fail at
  // runtime: Node and Bun both provide every built-in, so a `node:` import added to
  // `sugar.ts` or `presets.ts` would resolve under `npm test`, under `bun test`, and
  // even under the packed consumer spawned with `--conditions=browser` — and break only
  // in the bundle the subpath exists for. The same is true of a bare specifier: `#wasm`
  // or a dependency name would be resolved by the loader's manifest in Node and by a
  // bundler's rules here, not by this subpath's own relative graph. The shipped graph is
  // what a bundler parses, so the shipped graph is what is read here.
  const presets = graph(join(DIST, "presets-entry.js"));
  const builtins = [...presets].flatMap(([file, named]) =>
    named
      .filter((specifier) => isBuiltin(specifier))
      .map((specifier) => `${file.slice(DIST.length + 1)} imports ${specifier}`),
  );
  assert.deepEqual(
    builtins,
    [],
    "`opportune/presets` reaches a Node built-in, so a browser bundle cannot load it:",
  );
  const bare = [...presets].flatMap(([file, named]) =>
    named
      .filter((specifier) => !specifier.startsWith(".") && !isBuiltin(specifier))
      .map((specifier) => `${file.slice(DIST.length + 1)} imports ${specifier}`),
  );
  assert.deepEqual(
    bare,
    [],
    "`opportune/presets` reaches a bare specifier, so a browser bundle must resolve it " +
      "from elsewhere rather than from the subpath's own relative graph:",
  );
  // The walk is not vacuous, and it is not reading the whole tree: the engine's own
  // loader is the one that names Node built-ins, and the same walk finds them there.
  const engine = graph(join(DIST, "wasm-node.js"));
  assert.ok(
    [...engine.values()].flat().includes("node:module"),
    "the walk finds the loader's own `node:module` import, so its silence about the presets means something",
  );
  for (const loader of ["wasm.js", "wasm-node.js"]) {
    assert.ok(
      !presets.has(join(DIST, loader)),
      "the presets graph reaches the loader, so it is no longer pure TypeScript",
    );
  }
});
