/**
 * What the tarball the publish job uploads actually contains, and how it resolves.
 *
 * Every other test in this suite drives `dist/` by relative path, which is the tree
 * `npm publish` packs but not the way a consumer reaches it: npm decides what travels
 * from `files`, and a consumer resolves through the manifest's `exports`. A `files`
 * entry dropped in a review, or a subpath added to `exports` and not to the packed
 * tree, publishes a package that installs and then cannot be imported — and the
 * publish job would still be green, because it proves `dist/` rather than the
 * artefact. Both licence copies and the README are checked here for the same reason:
 * the lint gate compares them on disk, and this is what puts them in the tarball.
 *
 * The resolution case goes the whole way: it packs, installs the tarball into a
 * temporary project and imports every declared subpath by name from there, so the
 * resolver of whichever runtime runs the suite reads the installed `exports` map — the
 * one step no relative import performs, and the one a subpath renamed or repointed in
 * the map would break. A second consumer script `require`s the package from the same
 * install, because the CommonJS entry a Node application reaches for is the same
 * `exports` lookup under another loader.
 *
 * `npm pack --dry-run` computes the real packlist without writing anything, and the
 * resolution case writes only inside a temporary directory, so neither touches the
 * checkout and both run offline against the built package. Build it first:
 *
 * ```sh
 * make wasm && npm --prefix crates/opportune-wasm/js test
 * ```
 */

import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { mkdtempSync, readFileSync, readdirSync, rmSync, writeFileSync } from "node:fs";
import { createRequire } from "node:module";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";

/** The package root this suite belongs to. */
const PACKAGE = join(import.meta.dirname, "..");

/** The manifest `exports` and `files` are read from. */
const MANIFEST = JSON.parse(readFileSync(join(PACKAGE, "package.json"), "utf8"));

/**
 * The TypeScript compiler the package's own tooling pins.
 *
 * The consumer compile below uses the compiler's own API rather than spawning the
 * `tsc` script: the suite runs under Node and under Bun, and both execute this
 * JavaScript, so the lane that proves the declarations is the lane that declares the
 * runtime rather than whichever binary the script's shebang finds.
 */
const TS = createRequire(import.meta.url)("typescript");

/** The command that builds the tree the tarball packs. */
const BUILD = "make wasm";

/**
 * Every path the tarball holds, as `npm pack` lists it.
 *
 * `--json` keeps the report on stdout with npm's notices on stderr, and
 * `--ignore-scripts` makes the measurement independent of whatever lifecycle script
 * the manifest grows later.
 */
function packedFiles() {
  const run = spawnSync("npm", ["pack", "--dry-run", "--json", "--ignore-scripts"], {
    cwd: PACKAGE,
    encoding: "utf8",
  });
  assert.equal(run.error, undefined, `npm pack did not start: ${run.error?.message}`);
  assert.equal(run.status, 0, `npm pack failed: ${run.stderr}`);
  const [report] = JSON.parse(run.stdout);
  return report.files.map((entry) => entry.path);
}

/** `target` as it appears in the packlist: the `exports` map spells it `./…`. */
function packedPath(target) {
  return target.replace(/^\.\//, "");
}

/** `subpath` as a consumer spells it: `.` is the bare name, the rest are joined to it. */
function specifier(subpath) {
  return subpath === "." ? MANIFEST.name : `${MANIFEST.name}/${subpath.slice("./".length)}`;
}

/**
 * The conditions every `exports` entry has to declare.
 *
 * `types` is what a TypeScript consumer resolves before `default`, and an entry that
 * loses it still packs, installs, imports and runs: every typed caller gets "could not
 * find a declaration file" instead of the generated types. Nothing else in the
 * repository reads this map — `tsc` compiles `src/`, and the packlist case below walks
 * only the conditions that are present, so a dropped one leaves it green.
 */
const CONDITIONS = ["types", "default"];

/**
 * The subpaths the engine itself backs, and one binding each has to expose.
 *
 * These are the entries that also declare `browser` and `node`, because they are the
 * ones whose module only runs on one of the two. The binding below is exported by each
 * module and by no other, which is what makes the comparison with the resolver say
 * something; the others the documentation names are in `DOCUMENTED`.
 */
const ENGINE = {
  ".": "Engine",
  "./adapters/graph": "EXTENDED_PROPERTY_EXPAND",
  "./adapters/google": "freeBusyToBlocks",
};

/**
 * The subpaths the documentation tells a consumer to import, and one binding each has
 * to expose.
 *
 * This is the expectation the manifest cannot supply about itself. Comparing the
 * resolver's answer with the map it was resolved from proves only that the runtime
 * obeys the map, so a `default` repointed at some other file the tarball also carries
 * needs a name that lives outside it. `README.md` and `docs/wasm.md` are the pages that
 * name these four imports; `./presets` is the one a browser bundle may import, because
 * it reaches no WebAssembly.
 */
const DOCUMENTED = { ...ENGINE, "./presets": "businessHours" };

/**
 * The browser stub every engine-backed subpath resolves to under the `browser`
 * condition: one module, one explanation of the runtime the engine needs.
 */
const BROWSER = "./dist/browser.js";

/**
 * The conditions a bundler for a non-Node runtime applies, and the manifest entry
 * each has to reach the stub through.
 *
 * `browser` is the one every bundler sets. `edge-light` is the working group's
 * condition for a bundler target like Vercel's, and `worker` is Cloudflare's: a
 * runtime that names one of them gets the stub at the import rather than a bundle
 * that throws from the loader for its users. They are asserted separately because
 * each is a key a manifest edit can drop on its own — `browser` staying green while
 * `edge-light` falls through to `node`.
 */
const NON_NODE_CONDITIONS = ["browser", "edge-light", "worker"];

/**
 * The subpaths that name one file directly instead of a condition map.
 *
 * `./package.json` is the manifest itself, which a tool reads to learn the version
 * and the package's own metadata. `./schema.json` is the exported schema the
 * generated types derive from, so a consumer can validate a document with the same
 * Draft 2020-12 bundle the engine validates against. Neither names a condition
 * because neither has runtime variants: there is one manifest and one schema per
 * installed package.
 */
const PASSTHROUGH = new Set(["./package.json", "./schema.json"]);

/** The declared subpaths that resolve through a condition map. */
const MAPPED = Object.entries(MANIFEST.exports).filter(([, target]) => typeof target === "object");

/**
 * The consumer's own resolution, as the runtime running the suite performs it.
 *
 * Writes a script into an installed project and runs it there, so `import` reads the
 * installed manifest's `exports` map: the relative `../dist/…` imports every other
 * suite uses never consult it, so a subpath renamed or repointed in the map leaves all
 * of them green while every consumer resolves somewhere else. The script is spawned
 * with `process.execPath`, which is Node under `npm test` and Bun under `bun test`, so
 * each lane proves the installed-package path of the runtime that lane runs — and the
 * Bun lane is what makes the manifest's `engines.bun` more than a claim.
 *
 * @returns one `{ specifier, resolved, names }` per declared subpath.
 */
function resolvedByConsumer(dir) {
  const subpaths = MAPPED.map(([subpath]) => subpath);
  const consumer = join(dir, "consumer.mjs");
  writeFileSync(
    consumer,
    [
      `const subpaths = ${JSON.stringify(subpaths)};`,
      `const name = ${JSON.stringify(MANIFEST.name)};`,
      "const out = [];",
      "for (const subpath of subpaths) {",
      '  const specifier = subpath === "." ? name : `${name}/${subpath.slice(2)}`;',
      "  const resolved = import.meta.resolve(specifier);",
      "  const loaded = await import(specifier);",
      "  out.push({ specifier, resolved, names: Object.keys(loaded) });",
      "}",
      "console.log(JSON.stringify(out));",
    ].join("\n"),
  );
  const run = spawnSync(process.execPath, [consumer], { cwd: dir, encoding: "utf8" });
  assert.equal(
    run.status,
    0,
    `a consumer could not import the installed package: ${run.stderr.trim() || run.stdout.trim()}`,
  );
  return JSON.parse(run.stdout);
}

/**
 * The package as a CommonJS application reaches it, from the same installed tarball.
 *
 * A server written in CommonJS `require`s its dependencies, and that lookup reads the
 * same `exports` map through a loader that cannot run top-level `await`: a package that
 * needs one publishes a module CommonJS cannot load, and the failure only ever appears
 * in a consumer's application.
 *
 * @returns the version and one preset builder, as the required package reports them.
 */
function requiredByConsumer(dir) {
  const consumer = join(dir, "consumer.cjs");
  writeFileSync(
    consumer,
    [
      `const pkg = require(${JSON.stringify(MANIFEST.name)});`,
      `const presets = require(${JSON.stringify(`${MANIFEST.name}/presets`)});`,
      `const manifest = require(${JSON.stringify(`${MANIFEST.name}/package.json`)});`,
      "console.log(JSON.stringify({",
      "  version: pkg.VERSION,",
      "  rule: presets.businessHours().kind,",
      "  manifestVersion: manifest.version,",
      "}));",
    ].join("\n"),
  );
  const run = spawnSync(process.execPath, [consumer], { cwd: dir, encoding: "utf8" });
  assert.equal(
    run.status,
    0,
    `require() of the installed package failed: ${run.stderr.trim() || run.stdout.trim()}`,
  );
  return JSON.parse(run.stdout);
}

/**
 * The exported schema as an installed consumer reaches it.
 *
 * `opportune/schema.json` is the document the generated types derive from, so a
 * consumer that validates a request before sending it — with Ajv, or with a
 * provider's own schema validation — reads the same Draft 2020-12 bundle the
 * engine validates against instead of restating it. Both loaders are exercised:
 * the subpath is one file with no condition map, and a JSON import needs an import
 * attribute while `require` needs none. A manifest entry that dropped the subpath,
 * a `files` list that stopped packing it, or a document that lost the two unions
 * `Request` and `Response` name fails here rather than in the consumer.
 *
 * @returns the `$schema` URI and the shapes of the two document definitions, as
 *   each loader reports them.
 */
function schemaByConsumer(dir) {
  const consumer = join(dir, "schema.mjs");
  const specifier = JSON.stringify(`${MANIFEST.name}/schema.json`);
  writeFileSync(
    consumer,
    [
      'import { createRequire } from "node:module";',
      `import schema from ${specifier} with { type: "json" };`,
      `const required = createRequire(import.meta.url)(${specifier});`,
      "const shape = (document) => ({",
      "  $schema: document.$schema,",
      "  request: typeof document.$defs?.Request,",
      "  response: typeof document.$defs?.Response,",
      "});",
      "console.log(JSON.stringify({ imported: shape(schema), required: shape(required) }));",
    ].join("\n"),
  );
  const run = spawnSync(process.execPath, [consumer], { cwd: dir, encoding: "utf8" });
  assert.equal(
    run.status,
    0,
    `an installed consumer could not read the exported schema: ${run.stderr.trim() || run.stdout.trim()}`,
  );
  return JSON.parse(run.stdout);
}

/**
 * The package as a bundler for a non-Node runtime resolves it, from the same
 * installed tarball.
 *
 * Each of {@link NON_NODE_CONDITIONS} is a condition no other case in the suite ever
 * supplies: `import.meta.resolve` and every relative import use the default set.
 * Passing one to the runtime under test is what proves the manifest actually routes
 * those subpaths to the stub — an `exports` entry that lost its `browser` key, or one
 * pointing at a module that does not exist, still passes every engine case. The
 * script records the rejection's `name` and `code` for each subpath, and `"imported"`
 * for the ones that load.
 *
 * @returns one entry per declared subpath: `"imported"`, or the rejection's shape.
 */
function resolvedForCondition(dir, condition) {
  const consumer = join(dir, `consumer.${condition}.mjs`);
  writeFileSync(
    consumer,
    [
      `const subpaths = ${JSON.stringify(MAPPED.map(([subpath]) => subpath))};`,
      `const name = ${JSON.stringify(MANIFEST.name)};`,
      "const out = {};",
      "for (const subpath of subpaths) {",
      '  const specifier = subpath === "." ? name : `${name}/${subpath.slice(2)}`;',
      "  try {",
      "    await import(specifier);",
      '    out[specifier] = "imported";',
      "  } catch (error) {",
      "    out[specifier] = { name: error.name, code: error.code, message: error.message };",
      "  }",
      "}",
      "console.log(JSON.stringify(out));",
    ].join("\n"),
  );
  const run = spawnSync(process.execPath, [`--conditions=${condition}`, consumer], {
    cwd: dir,
    encoding: "utf8",
  });
  assert.equal(
    run.status,
    0,
    `a ${condition} bundle could not load the installed package: ` +
      `${run.stderr.trim() || run.stdout.trim()}`,
  );
  return JSON.parse(run.stdout);
}

/**
 * The runtime floors the documentation states, as `engines` has to spell them.
 *
 * This is the expectation the manifest cannot supply about itself, like `CONDITIONS`
 * and `DOCUMENTED` above: an `engines` entry dropped in a review still packs, installs
 * and imports, and nothing else in the repository reads it, so a supported runtime a
 * consumer cannot see from the manifest otherwise ships silently.
 */
const RUNTIMES = { bun: ">=1.4.2", node: ">=24" };

/** The packlist, computed once for the cases below. */
const FILES = packedFiles();

/**
 * The package packed into `dir` and installed there, as a consumer receives it.
 *
 * `--pack-destination` keeps the tarball out of the checkout, and the file is found in
 * the empty directory rather than parsed out of npm's stdout, whose shape is not the
 * assertion's subject. `--offline`: the package declares no runtime dependency, so the
 * install resolves nothing and needs no registry. `--ignore-scripts` on both, so the
 * install runs no lifecycle script of a dependency.
 */
function installTarball(dir) {
  const packed = spawnSync(
    "npm",
    ["pack", "--pack-destination", dir, "--ignore-scripts", "--silent"],
    { cwd: PACKAGE, encoding: "utf8" },
  );
  assert.equal(packed.status, 0, `npm pack failed: ${packed.stderr}`);
  const tarballs = readdirSync(dir).filter((entry) => entry.endsWith(".tgz"));
  assert.deepEqual(tarballs.length, 1, `npm pack wrote ${tarballs.join(", ") || "nothing"}`);

  const installed = spawnSync(
    "npm",
    [
      "install", "--prefix", dir, join(dir, tarballs[0]),
      "--offline", "--no-audit", "--no-fund", "--ignore-scripts", "--silent",
    ],
    { encoding: "utf8" },
  );
  assert.equal(installed.status, 0, `npm install of the packed tarball failed: ${installed.stderr}`);
}

/**
 * The consumer script the declarations are compiled against.
 *
 * It is what the documentation tells a consumer to write, and every line of it is a
 * declaration this package emits: the class, the generated request type, the error
 * class and a preset builder from the two subpaths, and `using` — which is the
 * declaration the whole case exists for.
 */
const CONSUMER = [
  'import { Engine, type Response } from "opportune";',
  'import { OpportuneError, businessHours, type Request } from "opportune/presets";',
  "",
  "const request: Request = {",
  '  now: "2027-06-14T07:00Z",',
  "  goal: {",
  '    zone: "Europe/London",',
  '    search_window: ["2027-06-13T23:00Z", "2027-06-18T23:00Z"],',
  '    parts: [{ duration: "30m", requirement: { all: [{ person: "rich" }] } }],',
  "  },",
  "  rules: [businessHours()],",
  "};",
  "",
  "export function book(): Response | string {",
  "  try {",
  "    using engine = new Engine();",
  "    return engine.findAvailability(request);",
  "  } catch (thrown) {",
  "    if (thrown instanceof OpportuneError) {",
  "      return thrown.code;",
  "    }",
  "    throw thrown;",
  "  }",
  "}",
  "",
].join("\n");

/** The page that states, to a consumer, which runtimes the package installs onto. */
const WASM_DOC = join(PACKAGE, "..", "..", "..", "docs", "wasm.md");

test("the manifest declares the runtime floors the documentation states", () => {
  assert.deepEqual(MANIFEST.engines, RUNTIMES);
});

test("the manifest marks the browser stub as the one module with side effects", () => {
  // A bundler tree-shakes a package whose manifest declares `sideEffects: false` — the
  // flag that makes `import { businessHours } from "opportune/presets"` reach only the
  // builder the caller uses. The stub cannot be shaken: its whole purpose is the
  // rejection it raises at import, and a bundler that dropped it would ship a bundle
  // that resolves no runtime check at all. Naming it — and nothing else — is therefore
  // the declaration that both halves hold: every other module here, the compiled glue
  // included, is loaded lazily by the call that needs it.
  assert.deepEqual(
    MANIFEST.sideEffects,
    [BROWSER],
    "the manifest declares side effects other than the browser stub, or none at all: a bundle " +
      "then either cannot tree-shake `opportune/presets` or drops the stub that explains the runtime",
  );
});

test("the documentation states the runtime floors the manifest declares", () => {
  // The sentence wraps in the page and moves as the prose is edited, so it is matched
  // with its whitespace flattened. Its `engines` clause is what makes it the sentence
  // promising the manifest says the same thing: the floor a consumer reads in the docs
  // and the floor their package manager enforces are one statement split across two
  // files, and either half can be edited without touching the other.
  const page = readFileSync(WASM_DOC, "utf8").replace(/\s+/g, " ");
  const stated = /Node (\d+) or newer, or Bun (\d+\.\d+\.\d+) or newer, as `engines` in the manifest states/.exec(
    page,
  );
  assert.ok(
    stated !== null,
    "docs/wasm.md no longer states the Node and Bun floors as `engines` in the manifest states them",
  );
  const [, node, bun] = stated;
  assert.equal(
    MANIFEST.engines.node,
    `>=${node}`,
    `docs/wasm.md tells a consumer Node ${node} or newer, the manifest a different floor`,
  );
  assert.equal(
    MANIFEST.engines.bun,
    `>=${bun}`,
    `docs/wasm.md tells a consumer Bun ${bun} or newer, the manifest a different floor`,
  );
});

test("every export the manifest declares is in the tarball", () => {
  const declared = Object.entries(MANIFEST.exports).flatMap(([subpath, target]) =>
    typeof target === "string"
      ? [[subpath, packedPath(target)]]
      : Object.entries(target).map(([field, path]) => [`${subpath} (${field})`, packedPath(path)]),
  );
  assert.ok(declared.length > 0, "the manifest declares no export to check");
  for (const [what, path] of declared) {
    assert.ok(
      FILES.includes(path),
      `${what} resolves to ${path}, which the tarball does not carry. Build it with \`${BUILD}\`.`,
    );
  }
});

test("the tarball carries the module the loader reads, both licence copies and the README", () => {
  // wasm-pack writes a `.gitignore` holding `*` beside its output and npm honours it,
  // so the module reaches a consumer only because `bin/bundle-module.mjs` copies the
  // pair into `dist/`. The names below are the loader's own path from `src/wasm.ts`
  // seen from `dist/`. npm's always-included patterns cover `README` but not the
  // `-MIT`/`-APACHE` licence names, so `files` is what puts the latter in the tarball.
  for (const path of [
    "package.json",
    "schema.json",
    "README.md",
    "LICENSE-MIT",
    "LICENSE-APACHE",
    "dist/pkg/opportune_wasm.cjs",
    "dist/pkg/opportune_wasm_bg.wasm",
  ]) {
    assert.ok(FILES.includes(path), `the tarball does not carry ${path}`);
  }
});

test("every export the manifest declares names both its types and its runtime file", () => {
  for (const [subpath, target] of MAPPED) {
    for (const condition of CONDITIONS) {
      assert.ok(
        typeof target[condition] === "string" && target[condition] !== "",
        `exports[${JSON.stringify(subpath)}] declares no \`${condition}\` condition`,
      );
    }
  }
  assert.deepEqual(
    [...PASSTHROUGH],
    Object.keys(MANIFEST.exports).filter((subpath) => typeof MANIFEST.exports[subpath] === "string"),
    "a subpath names one file directly without being declared as a passthrough this suite reads, " +
      "so nothing proves a consumer can reach it",
  );
});

test("the engine-backed exports name their runtime, and every non-Node runtime gets the stub", () => {
  // `node` and `default` have to be the same file — Node has no `browser` condition, so
  // a `default` pointing elsewhere would be the file the runtime least able to run it
  // loads — and every non-Node condition has to be the one stub that explains the
  // runtime, not a per-subpath copy of that explanation. The stub also has to come
  // before `node` in the map: a condition list is matched in declaration order, and
  // Node always includes `node`, so a `node` key above `edge-light` would answer a
  // request for the Edge runtime with the module that cannot run there.
  for (const subpath of Object.keys(ENGINE)) {
    const target = MANIFEST.exports[subpath];
    assert.ok(target !== undefined, `the manifest declares no ${subpath}`);
    for (const condition of NON_NODE_CONDITIONS) {
      assert.equal(
        target[condition],
        BROWSER,
        `${subpath} does not resolve to the browser stub under \`${condition}\``,
      );
    }
    assert.equal(target.node, target.default, `${subpath} resolves differently on Node and by default`);
    assert.notEqual(target.browser, target.node, `${subpath}'s browser condition loads the engine`);
    const order = Object.keys(target);
    for (const condition of NON_NODE_CONDITIONS) {
      assert.ok(
        order.indexOf(condition) < order.indexOf("node"),
        `${subpath} declares \`node\` before \`${condition}\`, so a request for that runtime loads ` +
          "the module that cannot run there",
      );
    }
  }
  const presets = MANIFEST.exports["./presets"];
  assert.equal(
    presets.browser,
    undefined,
    "./presets resolves to the browser stub, but a browser bundle is the one consumer it is for",
  );
});

test("an installed tarball resolves every declared subpath to the module behind it", (t) => {
  const dir = mkdtempSync(join(tmpdir(), "opportune-consumer-"));
  t.after(() => rmSync(dir, { recursive: true, force: true }));

  installTarball(dir);
  const resolved = resolvedByConsumer(dir);

  for (const [subpath, target] of MAPPED) {
    const name = specifier(subpath);
    const entry = resolved.find((candidate) => candidate.specifier === name);
    assert.ok(entry !== undefined, `${name} does not resolve for a consumer at all`);
    const declared = packedPath(target.default);
    assert.ok(
      entry.resolved.endsWith(`/${declared}`),
      `${name} resolved to ${entry.resolved}, not to the \`${declared}\` the manifest declares`,
    );
    assert.ok(
      entry.names.length > 0,
      `${name} resolved to a module exporting nothing. Build it with \`${BUILD}\`.`,
    );
  }

  for (const [subpath, binding] of Object.entries(DOCUMENTED)) {
    const name = specifier(subpath);
    const entry = resolved.find((candidate) => candidate.specifier === name);
    assert.ok(entry !== undefined, `the manifest declares no ${subpath} for a consumer to import`);
    assert.ok(
      entry.names.includes(binding),
      `${name} does not export ${binding}: it resolves to ${entry.resolved}, ` +
        `so the subpath the documentation names no longer reaches the module behind it`,
    );
  }

  const required = requiredByConsumer(dir);
  assert.equal(
    required.version,
    MANIFEST.version,
    "the package's `VERSION` is the manifest's `version`: bump `src/constants.ts`'s constant " +
      "when the release moves `package.json`",
  );
  assert.equal(
    required.manifestVersion,
    MANIFEST.version,
    "`require(\"opportune/package.json\")` answers a manifest that is not this package's: the " +
      "`./package.json` export is what a tool reads its version and metadata through",
  );
  assert.equal(required.rule, "window", "the CommonJS consumer builds a rule from `opportune/presets`");

  // The exported schema, as the same install serves it. `dist/` is derived from this
  // document, so a consumer validating its own request gets the same Draft 2020-12
  // bundle rather than a hand-written copy — and the two unions a request and an
  // answer are written against are in it by name.
  const schema = schemaByConsumer(dir);
  for (const [loader, shape] of Object.entries(schema)) {
    assert.equal(
      shape.$schema,
      "https://json-schema.org/draft/2020-12/schema",
      `the ${loader} loader read a schema that names another draft`,
    );
    assert.equal(
      shape.request,
      "object",
      `\`$defs.Request\` is not a definition of the exported schema (${loader})`,
    );
    assert.equal(
      shape.response,
      "object",
      `\`$defs.Response\` is not a definition of the exported schema (${loader})`,
    );
  }

  // The presets subpath carries the error class and the generated types, which is what
  // a bundle that builds a request — or answers the failure a builder raises — needs,
  // and what it cannot get from `opportune` without resolving a module that refuses its
  // runtime. A subpath that stopped re-exporting them would still pack, install and
  // import, so this is the assertion that keeps a browser consumer able to type its
  // documents and catch `OpportuneError`.
  const presets = resolved.find((candidate) => candidate.specifier === specifier("./presets"));
  assert.ok(
    presets?.names.includes("OpportuneError"),
    "`opportune/presets` does not export `OpportuneError`, so a browser consumer can catch " +
      "and type against nothing without importing the engine-backed entry",
  );
  // The same reason, for everything else a pure consumer reaches for: the constants a
  // bundle stamps onto a provider request, and the projections a public route serves.
  // Each is a value export, so a subpath that stopped re-exporting one still packs,
  // installs and imports — the name is simply absent from the one module list a browser
  // may load.
  const pure = [
    "VERSION",
    "MARKER_TAGS",
    "MARKER_EVENT_KEY",
    "EXTENDED_PROPERTY_NAMESPACE_GUID",
    "EXTENDED_PROPERTY_TAGS_ID",
    "EXTENDED_PROPERTY_EVENT_KEY_ID",
    "EXTENDED_PROPERTY_EXPAND",
    "httpStatusForError",
    "isKnownStatus",
    "publicResponse",
    "publicSlotCheck",
    "statusPolicy",
  ];
  const missing = pure.filter((binding) => !presets?.names.includes(binding));
  assert.deepEqual(
    missing,
    [],
    "`opportune/presets` does not export " +
      `${missing.join(", ")}: a bundle that may load only that subpath cannot reach it`,
  );

  // The same install, resolved for each non-Node runtime: the engine subpaths have to
  // refuse the runtime through the manifest rather than load a module that cannot work
  // there, and the pure presets subpath has to keep loading.
  for (const condition of NON_NODE_CONDITIONS) {
    const under = resolvedForCondition(dir, condition);
    for (const subpath of Object.keys(ENGINE)) {
      const name = specifier(subpath);
      const entry = under[name];
      assert.equal(
        entry?.name,
        "OpportuneError",
        `${name} under the ${condition} condition resolved to ${JSON.stringify(entry)}`,
      );
      assert.equal(entry.code, "unsupported_runtime", `${name} under the ${condition} condition`);
      assert.match(entry.message, /Node or Bun/);
    }
    assert.equal(
      under[specifier("./presets")],
      "imported",
      `\`opportune/presets\` must load under the ${condition} condition: it is the one a bundle can use`,
    );
  }

  // The stub resolves for three subpaths that export three different lists, and a
  // bundler that cannot resolve a named import fails the build with "export not found"
  // — a symbol error that says nothing about the runtime the stub exists to explain.
  // The map routes all three here, so the stub is only useful if it declares every
  // name the modules it replaces declare. Read from the installed tree, because that
  // is the copy a bundler parses.
  const stub = readFileSync(join(dir, "node_modules", MANIFEST.name, packedPath(BROWSER)), "utf8");
  const declared = new Set(
    [...stub.matchAll(/export\s*\{([^}]*)\}/g)].flatMap(([, names]) =>
      names.split(",").map((name) => name.trim().replace(/^\S+ as /, "")).filter(Boolean),
    ),
  );
  for (const subpath of Object.keys(ENGINE)) {
    const name = specifier(subpath);
    const exported = resolved.find((candidate) => candidate.specifier === name)?.names ?? [];
    const absent = exported.filter((binding) => !declared.has(binding));
    assert.deepEqual(
      absent,
      [],
      `${name} exports ${absent.join(", ")}, which ${BROWSER} does not: a browser bundle ` +
        "imports one of them and the build fails on the symbol instead of saying the runtime is wrong",
    );
  }
});

test("the installed declarations compile for a consumer with no disposable lib", (t) => {
  // `Engine` is `Disposable`, and `Symbol.dispose` is declared in the `esnext.disposable`
  // library rather than in `ES2024` — which is the lib a project starts from, and the
  // one a consumer's `tsconfig.json` states. This package's own compile cannot catch a
  // declaration that leans on a lib the consumer does not have: its `tsconfig.json`
  // includes `esnext.disposable`, so the emission is green and the consumer — with
  // `skipLibCheck: false` — is the first program to read the declaration as written.
  // The case compiles the installed `dist/` under exactly that project, so the emitted
  // declaration carries the `esnext.disposable` reference that makes `using` compile.
  const dir = mkdtempSync(join(tmpdir(), "opportune-declarations-"));
  t.after(() => rmSync(dir, { recursive: true, force: true }));
  installTarball(dir);

  writeFileSync(join(dir, "consumer.ts"), CONSUMER);
  writeFileSync(
    join(dir, "tsconfig.json"),
    `${JSON.stringify(
      {
        compilerOptions: {
          target: "ES2024",
          module: "nodenext",
          moduleResolution: "nodenext",
          lib: ["ES2024"],
          strict: true,
          skipLibCheck: false,
          noEmit: true,
        },
        files: ["consumer.ts"],
      },
      null,
      2,
    )}\n`,
  );

  // The project is read the way the compiler reads a consumer's: the tsconfig on disk,
  // resolved from the directory the package was installed into. The file name travels
  // with it because the compiler resolves the project's `typeRoots` — and every option
  // that defaults relative to the project rather than the process — from that path: a
  // program built from these options alone looks for `@types` beside the current
  // working directory, which here is this package's, whose own `@types/node` supplies
  // `Symbol.dispose` and hides the very thing the case checks for.
  const configFile = join(dir, "tsconfig.json");
  const config = TS.readConfigFile(configFile, TS.sys.readFile);
  assert.equal(config.error, undefined, "the fixture tsconfig.json does not parse");
  const project = TS.parseJsonConfigFileContent(config.config, TS.sys, dir, undefined, configFile);
  assert.deepEqual(
    project.errors.map((error) => TS.flattenDiagnosticMessageText(error.messageText, " ")),
    [],
    "the fixture tsconfig.json is not a project the compiler accepts",
  );

  const program = TS.createProgram(project.fileNames, project.options);
  // A bare project, which is the one the missing directive breaks. `@types/node` —
  // inherited from this repository's tooling, or installed by the consumer's own
  // application — declares `Symbol.dispose` itself, so a program that picked it up would
  // compile whether or not the declaration carries the reference. The case is only
  // evidence while the program is the project the lib alone describes.
  const inherited = program.getSourceFiles()
    .map((file) => file.fileName)
    .filter((name) => name.includes("/node_modules/@types/"));
  assert.deepEqual(
    inherited,
    [],
    "the fixture project inherited a type package, whose own libs would answer for the " +
      "declaration this case exists to check",
  );

  const diagnostics = TS.getPreEmitDiagnostics(program).map((diagnostic) => {
    const where = diagnostic.file === undefined
      ? ""
      : `${diagnostic.file.fileName}:${String(
          diagnostic.file.getLineAndCharacterOfPosition(diagnostic.start ?? 0).line + 1,
        )}: `;
    return `${where}${TS.flattenDiagnosticMessageText(diagnostic.messageText, " ")}`;
  });
  assert.deepEqual(
    diagnostics,
    [],
    "the packed declarations do not compile for a consumer whose lib is `ES2024` with " +
      "`skipLibCheck: false`: the reference directive `dist/engine.d.ts` needs is missing",
  );
});

test("the tarball carries no source, test or tooling tree", () => {
  const unpublished = ["src/", "tests/", "bin/", "scripts/", "examples/", "pkg/", "node_modules/"];
  const leaked = FILES.filter((path) => unpublished.some((dir) => path.startsWith(dir)));
  assert.deepEqual(leaked, [], "the package publishes its build output, not its sources");
});
