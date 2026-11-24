/**
 * What the tarball `npm publish` uploads contains, and whether the standard package
 * validators accept it.
 *
 * npm decides what ships from `files`, and a consumer resolves it through `exports` and
 * the emitted declarations. Rather than assert that map against a hand-written resolver,
 * this suite runs the tools that own those questions:
 *
 * - `publint` checks the manifest's `files`, `exports`, `main`, `types` and `sideEffects`
 *   against the packed tree;
 * - `@arethetypeswrong/cli` checks every entry point's runtime and declaration resolution
 *   for the packed tarball under Node's ESM/CJS and bundler rules;
 * - `tsc` compiles two real consumer projects (no DOM, and DOM for `opportune/web`) from
 *   the installed tarball, so the declarations a consumer receives are what is checked.
 *
 * The behavioural cases that tools cannot express stay here: a CommonJS consumer builds
 * an `Engine` synchronously, `opportune/schema.json` reads through both loaders, and the
 * packlist carries exactly one `.wasm` and no source or harness tree.
 *
 * Build the package first:
 *
 * ```sh
 * make wasm && npm --prefix crates/opportune-wasm/js test
 * ```
 */

import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { mkdtempSync, readFileSync, readdirSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";

/** The package root this suite belongs to. */
const PACKAGE = join(import.meta.dirname, "..");

/** The manifest `exports` and `files` are read from. */
const MANIFEST = JSON.parse(readFileSync(join(PACKAGE, "package.json"), "utf8"));

/** The command that builds the tree the tarball packs. */
const BUILD = "make wasm";

/** The runtime floors the documentation states, as `engines` has to spell them. */
const RUNTIMES = { bun: ">=1.4.2", node: ">=24" };

/** The page that states, to a consumer, which runtimes the package installs onto. */
const WASM_DOC = join(PACKAGE, "..", "..", "..", "docs", "wasm.md");

/** Every path the tarball holds, as `npm pack` lists it. */
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

/** The packlist, computed once for the cases below. */
const FILES = packedFiles();

/** A local binary this package's devDependencies installed. */
function binary(name) {
  return join(PACKAGE, "node_modules", ".bin", name);
}

/** Runs `command` in `cwd` and fails with its output when it does not exit 0. */
function succeeds(command, args, cwd, what) {
  const run = spawnSync(command, args, { cwd, encoding: "utf8" });
  assert.equal(run.error, undefined, `${what} did not start: ${run.error?.message}`);
  assert.equal(
    run.status,
    0,
    `${what} failed:\n${run.stdout ?? ""}${run.stderr ?? ""}`.trim(),
  );
}

/**
 * The package packed into `dir`, as the tarball `npm publish` uploads.
 *
 * `--pack-destination` keeps the tarball out of the checkout, and `--ignore-scripts`
 * keeps the pack free of lifecycle scripts. The single `.tgz` is returned so callers
 * can install it or hand the same artefact to a validator.
 */
function packTarball(dir) {
  const packed = spawnSync(
    "npm",
    ["pack", "--pack-destination", dir, "--ignore-scripts", "--silent"],
    { cwd: PACKAGE, encoding: "utf8" },
  );
  assert.equal(packed.error, undefined, `npm pack did not start: ${packed.error?.message}`);
  assert.equal(packed.status, 0, `npm pack failed: ${packed.stderr}`);
  const tarballs = readdirSync(dir).filter((entry) => entry.endsWith(".tgz"));
  assert.deepEqual(tarballs.length, 1, `npm pack wrote ${tarballs.join(", ") || "nothing"}`);
  return join(dir, tarballs[0]);
}

/**
 * The package packed into `dir` and installed there, as a consumer receives it.
 *
 * `--offline` is sound because the package declares no runtime dependency.
 * `--ignore-scripts` on both, so the install runs no lifecycle script.
 */
function installTarball(dir) {
  const tarball = packTarball(dir);
  const installed = spawnSync(
    "npm",
    [
      "install",
      "--prefix",
      dir,
      tarball,
      "--offline",
      "--no-audit",
      "--no-fund",
      "--ignore-scripts",
      "--silent",
    ],
    { encoding: "utf8" },
  );
  assert.equal(installed.status, 0, `npm install of the packed tarball failed: ${installed.stderr}`);
}

/** Compiles the consumer project in `dir` with the package's own TypeScript. */
function typechecks(dir) {
  // The npm-installed `tsc` shim carries a `node` shebang, so this is the same compiler
  // whether the suite runs under Node or Bun. `process.execPath` would be Bun in the Bun
  // lane, which is not the toolchain this compile is checking.
  const run = spawnSync(binary("tsc"), ["-p", join(dir, "tsconfig.json")], {
    cwd: dir,
    encoding: "utf8",
  });
  assert.equal(run.error, undefined, `tsc did not start: ${run.error?.message}`);
  return `${run.stdout ?? ""}${run.stderr ?? ""}`.trim();
}

/** The project options a consumer starts from: no DOM, no `@types` inherited. */
function projectOptions(lib) {
  return {
    compilerOptions: {
      target: "ES2024",
      module: "nodenext",
      moduleResolution: "nodenext",
      lib,
      strict: true,
      noEmit: true,
      skipLibCheck: false,
      types: [],
    },
    files: ["consumer.ts"],
  };
}

test("the manifest declares the runtime floors the documentation states", () => {
  assert.deepEqual(MANIFEST.engines, RUNTIMES);
});

test("the documentation states the runtime floors the manifest declares", () => {
  const page = readFileSync(WASM_DOC, "utf8").replace(/\s+/g, " ");
  const stated = /Node (\d+) or newer, or Bun (\d+\.\d+\.\d+) or newer, as `engines` in the manifest states/.exec(
    page,
  );
  assert.ok(
    stated !== null,
    "docs/wasm.md no longer states the Node and Bun floors as `engines` in the manifest states them",
  );
  const [, node, bun] = stated;
  assert.equal(MANIFEST.engines.node, `>=${node}`);
  assert.equal(MANIFEST.engines.bun, `>=${bun}`);
});

test("the manifest is tree-shakeable and publishes nothing it does not build", () => {
  // `sideEffects: false` is what lets a bundler drop `opportune/presets` down to the one
  // builder a caller imports. Every module in the package is pure: the loader instantiates
  // on the first engine call, and the web loader does so only when the application calls
  // `init()`. The packlist half is what `files` promises: build output, schema, README and
  // both licence copies, and none of the source, test, harness or install trees.
  assert.equal(MANIFEST.sideEffects, false, "the manifest no longer declares itself tree-shakeable");
  const unpublished = ["src/", "tests/", "bin/", "scripts/", "examples/", "pkg/", "node_modules/", "integration/"];
  assert.deepEqual(
    FILES.filter((path) => unpublished.some((dir) => path.startsWith(dir))),
    [],
    "the package publishes its build output, not its sources",
  );
});

test("the tarball carries the module, both generated loaders, the licences and the README", () => {
  for (const path of [
    "package.json",
    "schema.json",
    "README.md",
    "LICENSE-MIT",
    "LICENSE-APACHE",
    "dist/pkg/opportune_wasm.js",
    "dist/pkg/opportune_wasm.cjs",
    "dist/pkg/opportune_wasm_bg.wasm",
    "dist/wasm-node.js",
    "dist/wasm-workerd.js",
    "dist/wasm-web.js",
    "dist/web.js",
  ]) {
    assert.ok(FILES.includes(path), `the tarball does not carry ${path}. Build it with \`${BUILD}\`.`);
  }
});

test("the tarball carries one module binary and the loaders this build generated", () => {
  assert.deepEqual(
    FILES.filter((path) => path.endsWith(".wasm")),
    ["dist/pkg/opportune_wasm_bg.wasm"],
    "the tarball must carry exactly one WebAssembly binary",
  );
  assert.deepEqual(
    FILES.filter((path) => path.startsWith("dist/pkg/")).sort(),
    [
      "dist/pkg/opportune_wasm.cjs",
      "dist/pkg/opportune_wasm.js",
      "dist/pkg/opportune_wasm_bg.wasm",
    ],
    "`dist/pkg/` must hold the module and the two generated loaders",
  );
  assert.ok(
    !FILES.includes("dist/browser.js"),
    "the removed browser stub is still in the tarball; the build did not clean `dist/`",
  );
});

test("every export target the manifest declares is in the tarball", () => {
  const declared = Object.entries(MANIFEST.exports).flatMap(([subpath, target]) =>
    typeof target === "string"
      ? [[subpath, target.replace(/^\.\//, "")]]
      : Object.entries(target).map(([field, path]) => [
          `${subpath} (${field})`,
          path.replace(/^\.\//, ""),
        ]),
  );
  assert.ok(declared.length > 0, "the manifest declares no export to check");
  for (const [what, path] of declared) {
    assert.ok(FILES.includes(path), `${what} resolves to ${path}, which the tarball does not carry`);
  }
});

test("publint accepts the packed manifest", () => {
  succeeds(binary("publint"), ["--strict"], PACKAGE, "publint");
});

test("arethetypeswrong accepts every entry point's declarations", (t) => {
  // The tarball is packed into a temporary directory rather than the checkout, and the
  // validator is handed that `.tgz` directly: `--pack .` would write the tarball beside
  // the manifest and invoke `npm pack` without `--ignore-scripts`. The declared entry
  // points are derived from the manifest, minus the `.wasm` filename that appears inside
  // the web loader's guidance text and is not a JS entry point. The `cjs-resolves-to-esm`
  // rule is ignored on purpose — this package supports Node 24+ `require(esm)`, which is
  // proved by the CommonJS consumer case below.
  const dir = mkdtempSync(join(tmpdir(), "opportune-attw-"));
  t.after(() => rmSync(dir, { recursive: true, force: true }));
  const tarball = packTarball(dir);
  const entrypoints = Object.keys(MANIFEST.exports).filter((subpath) => !subpath.endsWith(".wasm"));
  assert.notDeepEqual(entrypoints, [], "the manifest declares no JS entry point to check");
  succeeds(
    binary("attw"),
    [
      tarball,
      "--profile",
      "node16",
      "--entrypoints",
      ...entrypoints,
      "--ignore-rules",
      "cjs-resolves-to-esm",
    ],
    PACKAGE,
    "attw",
  );
});

test("the installed declarations compile in a consumer with no DOM lib", (t) => {
  // `Engine` is `Disposable`, and `Symbol.dispose` lives in `esnext.disposable` rather
  // than in `ES2024`. The package's own compile includes that lib, so a declaration that
  // leaned on it without emitting the reference directive still compiles here and fails
  // only in a consumer's project — which this case is.
  const dir = mkdtempSync(join(tmpdir(), "opportune-declarations-"));
  t.after(() => rmSync(dir, { recursive: true, force: true }));
  installTarball(dir);

  writeFileSync(
    join(dir, "consumer.ts"),
    [
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
    ].join("\n"),
  );
  writeFileSync(
    join(dir, "tsconfig.json"),
    `${JSON.stringify(projectOptions(["ES2024"]), null, 2)}\n`,
  );

  const diagnostics = typechecks(dir);
  assert.equal(
    diagnostics,
    "",
    "the packed root declarations do not compile for a consumer whose lib is `ES2024` with " +
      "`skipLibCheck: false`",
  );
});

test("`opportune/web` compiles for a Web consumer with typed init and initSync", (t) => {
  /**
   * The web entry names `Response`, `URL` and `WebAssembly.Module`, which live in the DOM
   * lib. The subpath exists so those names stay out of the root declarations; a Web
   * consumer is the program that has them, and this is the compile that proves the
   * subpath exports the two initialisers with their input types.
   */
  const dir = mkdtempSync(join(tmpdir(), "opportune-web-declarations-"));
  t.after(() => rmSync(dir, { recursive: true, force: true }));
  installTarball(dir);

  writeFileSync(
    join(dir, "consumer.ts"),
    [
      'import init, { initSync, type InitInput, type SyncInitInput } from "opportune/web";',
      "",
      "const input: InitInput = new URL(\"/opportune_wasm_bg.wasm\", globalThis.location.href);",
      "const compiled: SyncInitInput = {} as WebAssembly.Module;",
      "",
      "export async function start(): Promise<void> {",
      "  await init(input);",
      "  initSync({ module: compiled });",
      "}",
      "",
    ].join("\n"),
  );
  writeFileSync(
    join(dir, "tsconfig.json"),
    `${JSON.stringify(projectOptions(["ES2024", "DOM"]), null, 2)}\n`,
  );

  const diagnostics = typechecks(dir);
  assert.equal(
    diagnostics,
    "",
    "`opportune/web` does not compile for a Web consumer with `lib: [\"ES2024\", \"DOM\"]`",
  );
});

test("a CommonJS consumer builds an engine synchronously and reads the schema", (t) => {
  const dir = mkdtempSync(join(tmpdir(), "opportune-cjs-"));
  t.after(() => rmSync(dir, { recursive: true, force: true }));
  installTarball(dir);

  const consumer = join(dir, "consumer.cjs");
  writeFileSync(
    consumer,
    [
      `const pkg = require(${JSON.stringify(MANIFEST.name)});`,
      `const presets = require(${JSON.stringify(`${MANIFEST.name}/presets`)});`,
      `const graph = require(${JSON.stringify(`${MANIFEST.name}/adapters/graph`)});`,
      `const google = require(${JSON.stringify(`${MANIFEST.name}/adapters/google`)});`,
      `const web = require(${JSON.stringify(`${MANIFEST.name}/web`)});`,
      `const manifest = require(${JSON.stringify(`${MANIFEST.name}/package.json`)});`,
      `const schema = require(${JSON.stringify(`${MANIFEST.name}/schema.json`)});`,
      "const engine = new pkg.Engine();",
      "const tzdb = engine.tzdbVersion;",
      "const limit = pkg.defaultLimits().max_search_window_days;",
      "engine.free();",
      "console.log(JSON.stringify({",
      "  version: pkg.VERSION,",
      "  rule: presets.businessHours().kind,",
      "  graphMarker: graph.MARKER_TAGS,",
      "  googleMarker: google.MARKER_TAGS,",
      "  graphExpand: graph.EXTENDED_PROPERTY_EXPAND,",
      "  webInit: typeof web.init,",
      "  webDefault: typeof web.default,",
      "  manifestVersion: manifest.version,",
      "  schema: schema.$schema,",
      "  tzdb,",
      "  limit,",
      "}));",
    ].join("\n"),
  );
  const run = spawnSync(process.execPath, [consumer], { cwd: dir, encoding: "utf8" });
  assert.equal(
    run.status,
    0,
    `require() of the installed package failed: ${run.stderr.trim() || run.stdout.trim()}`,
  );
  const required = JSON.parse(run.stdout);

  assert.equal(required.version, MANIFEST.version);
  assert.equal(required.manifestVersion, MANIFEST.version);
  assert.equal(required.rule, "window");
  // Every subpath waived under `cjs-resolves-to-esm` is genuinely reached by `require()`:
  // the adapters' constants and the web entry's initialisers are read from the CommonJS
  // namespace, so the waiver rests on a working `require(esm)` rather than an untested
  // claim about what a resolver would do.
  assert.equal(required.graphMarker, "opportune.tags");
  assert.equal(required.googleMarker, "opportune.tags");
  assert.match(required.graphExpand, /^singleValueExtendedProperties\(\$filter=/);
  assert.equal(required.webInit, "function");
  assert.equal(required.webDefault, "function");
  assert.equal(required.schema, "https://json-schema.org/draft/2020-12/schema");
  // The same script built an engine, read the module's own time zone database version
  // and released it, all without a tick: a build whose glue needed the asynchronous
  // initialiser would fail here rather than in a consumer's server.
  assert.match(required.tzdb, /^\d{4}[a-z]$/, "the CommonJS consumer read a time zone database version");
  assert.equal(required.limit, 92, "the CommonJS consumer read the engine's default limits");
});
