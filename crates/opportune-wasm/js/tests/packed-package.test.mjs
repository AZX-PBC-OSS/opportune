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
 * - `tsc` compiles real consumer projects (no DOM for the root, DOM and WebWorker for
 *   `opportune/web`, and a Node project with no DOM or WebWorker lib at all) from the
 *   installed tarball, so the declarations a consumer receives are what is checked.
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

/**
 * The longest a package-manager, compiler or consumer child may take.
 *
 * Every child here is local — `npm pack`, an offline install with no runtime dependency,
 * `tsc`, the validators, and a consumer that builds one engine — so this is a bound that
 * keeps a wedged child from holding the lane open, not a deadline the happy path meets.
 * It is also the only bound that can fire: `spawnSync` blocks the event loop, so a
 * `timeout` on the test itself cannot interrupt it.
 */
const CHILD_TIMEOUT_MS = 120_000;

/** The four bytes every WebAssembly module begins with. */
const WASM_MAGIC = "\0asm";

/** A child's status, signal, spawn error and streams, as a failure a reader can act on. */
function report(run) {
  return [
    `status: ${String(run.status)}`,
    `signal: ${String(run.signal)}`,
    `error: ${run.error?.message ?? "none"}`,
    `stdout: ${JSON.stringify(run.stdout ?? "")}`,
    `stderr: ${JSON.stringify(run.stderr ?? "")}`,
  ].join("\n");
}

/**
 * Every subpath the manifest declares, as the specifier a consumer imports, the file the
 * manifest says it resolves to, and the path the test packs.
 *
 * Derived from the manifest so a subpath added later cannot escape the consumers below:
 * the executable surface guard fails until a line says what the new subpath is for, and
 * the resolution loop imports and resolves every entry.
 */
const EXPORTS = Object.entries(MANIFEST.exports).map(([subpath, target]) => {
  const path = (typeof target === "string" ? target : target.default).replace(/^\.\//, "");
  return {
    subpath,
    specifier: subpath === "." ? MANIFEST.name : `${MANIFEST.name}/${subpath.replace(/^\.\//, "")}`,
    target: path,
  };
});

/** The subpaths a consumer executes as modules, as opposed to JSON documents and the binary. */
const EXECUTABLE_EXPORTS = EXPORTS.filter((entry) => entry.target.endsWith(".js"));

/** The JSON subpaths, which a consumer reads with an import attribute. */
const JSON_EXPORTS = EXPORTS.filter((entry) => entry.target.endsWith(".json"));

/** The one module binary, which a bundler turns into a URL rather than a module. */
const [WASM_ASSET] = EXPORTS.filter((entry) => entry.target.endsWith(".wasm"));

assert.ok(EXPORTS.length > 0, "the manifest declares no export for a consumer to reach");
assert.equal(
  new Set(EXPORTS.map((entry) => entry.specifier)).size,
  EXPORTS.length,
  "two declared subpaths share one specifier",
);
assert.notDeepEqual(EXECUTABLE_EXPORTS, [], "the manifest declares no executable subpath");
assert.notDeepEqual(JSON_EXPORTS, [], "the manifest declares no JSON subpath");
assert.ok(WASM_ASSET !== undefined, "the manifest declares no module binary subpath");

/**
 * What a consumer reads from each executable subpath.
 *
 * A new `.js` export fails the guard below until a line here names what a consumer gets
 * from it, so the ESM consumer cannot silently skip a subpath the manifest added.
 */
const SURFACE = {
  ".": ["Engine", "VERSION", "defaultLimits", "findAvailabilityJson", "OpportuneError"],
  "./web": ["init", "initSync", "default"],
  "./presets": ["businessHours", "singleGoal", "person", "MARKER_TAGS", "OpportuneError"],
  "./adapters/graph": ["calendarViewToBlocks", "MARKER_TAGS", "EXTENDED_PROPERTY_EXPAND"],
  "./adapters/google": ["eventsListToBlocks", "MARKER_TAGS"],
};
assert.deepEqual(
  EXECUTABLE_EXPORTS.map((entry) => entry.subpath).sort(),
  Object.keys(SURFACE).sort(),
  "an executable subpath has no consumer surface check: add it to SURFACE and to the consumer",
);

/** Every path the tarball holds, as `npm pack` lists it. */
function packedFiles() {
  const run = spawnSync("npm", ["pack", "--dry-run", "--json", "--ignore-scripts"], {
    cwd: PACKAGE,
    encoding: "utf8",
    timeout: CHILD_TIMEOUT_MS,
  });
  assert.equal(run.error, undefined, `npm pack did not finish:\n${report(run)}`);
  assert.equal(run.signal, null, `npm pack was killed:\n${report(run)}`);
  assert.equal(run.status, 0, `npm pack failed:\n${report(run)}`);
  const [listing] = JSON.parse(run.stdout);
  return listing.files.map((entry) => entry.path);
}

/** The packlist, computed once for the cases below. */
const FILES = packedFiles();

/** A local binary this package's devDependencies installed. */
function binary(name) {
  return join(PACKAGE, "node_modules", ".bin", name);
}

/** Runs `command` in `cwd`, bounded, and fails with what the child actually reported. */
function succeeds(command, args, cwd, what) {
  const run = spawnSync(command, args, { cwd, encoding: "utf8", timeout: CHILD_TIMEOUT_MS });
  assert.equal(run.error, undefined, `${what} did not finish:\n${report(run)}`);
  assert.equal(run.signal, null, `${what} was killed:\n${report(run)}`);
  assert.equal(run.status, 0, `${what} failed:\n${report(run)}`);
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
    { cwd: PACKAGE, encoding: "utf8", timeout: CHILD_TIMEOUT_MS },
  );
  assert.equal(packed.error, undefined, `npm pack did not finish:\n${report(packed)}`);
  assert.equal(packed.signal, null, `npm pack was killed:\n${report(packed)}`);
  assert.equal(packed.status, 0, `npm pack failed:\n${report(packed)}`);
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
    { encoding: "utf8", timeout: CHILD_TIMEOUT_MS },
  );
  assert.equal(installed.error, undefined, `npm install did not finish:\n${report(installed)}`);
  assert.equal(installed.signal, null, `npm install was killed:\n${report(installed)}`);
  assert.equal(installed.status, 0, `npm install of the packed tarball failed:\n${report(installed)}`);
}

/** Compiles the consumer project in `dir` with the package's own TypeScript. */
function typechecks(dir) {
  // The npm-installed `tsc` shim carries a `node` shebang, so this is the same compiler
  // whether the suite runs under Node or Bun. `process.execPath` would be Bun in the Bun
  // lane, which is not the toolchain this compile is checking.
  const run = spawnSync(binary("tsc"), ["-p", join(dir, "tsconfig.json")], {
    cwd: dir,
    encoding: "utf8",
    timeout: CHILD_TIMEOUT_MS,
  });
  // A compile that never ran, was killed, or crashed, is not a consumer whose declarations
  // compiled. Checking the status here is what keeps an empty-output failure from reading
  // as a clean compile, and `report` keeps the status, signal and both streams together.
  assert.equal(run.error, undefined, `tsc did not finish:\n${report(run)}`);
  assert.equal(run.signal, null, `tsc was killed:\n${report(run)}`);
  assert.equal(run.status, 0, `tsc failed:\n${report(run)}`);
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
   * The web entry names `Response` and `URL`, which live in the DOM lib, and it accepts
   * a compiled module, which it has to do without naming the `WebAssembly` namespace
   * (see the Node case below). The subpath exists so those names stay out of the root
   * declarations; a Web consumer is the program that has them, and this is the compile
   * that proves the subpath exports the two initialisers with their input types.
   *
   * `declare const compiled` is the platform's own type, not a cast: a cast would answer
   * `{}` for any value, and would prove nothing about a real `WebAssembly.Module` being
   * accepted by the published union.
   */
  const dir = mkdtempSync(join(tmpdir(), "opportune-web-declarations-"));
  t.after(() => rmSync(dir, { recursive: true, force: true }));
  installTarball(dir);

  writeFileSync(
    join(dir, "consumer.ts"),
    [
      'import init, { initSync, type InitInput, type SyncInitInput } from "opportune/web";',
      "",
      "declare const compiled: WebAssembly.Module;",
      "const input: InitInput = new URL(\"/opportune_wasm_bg.wasm\", globalThis.location.href);",
      "const module: SyncInitInput = compiled;",
      "",
      "export async function start(): Promise<void> {",
      "  await init(input);",
      "  initSync({ module });",
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

test("`opportune/web` compiles for a Web Worker consumer", (t) => {
  /**
   * A module Web Worker is the other program the subpath exists for: `WebWorker` instead
   * of `DOM`, and no `@types` at all. A `Response` (what `fetch` answers with) and the
   * platform's `WebAssembly.Module` both have to be assignable there, so a declaration
   * that named only the DOM spellings of those globals would pass the case above and
   * fail here.
   */
  const dir = mkdtempSync(join(tmpdir(), "opportune-web-worker-declarations-"));
  t.after(() => rmSync(dir, { recursive: true, force: true }));
  installTarball(dir);

  writeFileSync(
    join(dir, "consumer.ts"),
    [
      'import init, { initSync, type InitInput, type SyncInitInput } from "opportune/web";',
      "",
      "declare const compiled: WebAssembly.Module;",
      "declare const response: Response;",
      "const module: SyncInitInput = compiled;",
      "const input: InitInput = response;",
      "",
      "export async function start(): Promise<void> {",
      "  await init(input);",
      "  initSync({ module });",
      "}",
      "",
    ].join("\n"),
  );
  writeFileSync(
    join(dir, "tsconfig.json"),
    `${JSON.stringify(projectOptions(["ES2024", "WebWorker"]), null, 2)}\n`,
  );

  const diagnostics = typechecks(dir);
  assert.equal(
    diagnostics,
    "",
    "`opportune/web` does not compile for a Web Worker consumer with " +
      "`lib: [\"ES2024\", \"WebWorker\"]`",
  );
});

test("`opportune/web` compiles for a Node consumer with no DOM or WebWorker lib", (t) => {
  /**
   * An isomorphic module imports the initialiser on both sides: `init()` is a no-op on
   * Node, so one file can call it unconditionally and the server's project — `@types/node`
   * and `lib: ["ES2024"]`, the same lib the root declarations compile under — has to be
   * able to read the declaration. Only the `WebAssembly` namespace is missing there, so
   * this is the compile that keeps the published union from requiring a lib the server
   * does not have, while the bytes a Node caller actually passes stay assignable.
   */
  const dir = mkdtempSync(join(tmpdir(), "opportune-web-node-declarations-"));
  t.after(() => rmSync(dir, { recursive: true, force: true }));
  installTarball(dir);

  writeFileSync(
    join(dir, "consumer.ts"),
    [
      'import { readFileSync } from "node:fs";',
      "",
      'import init, { initSync, type InitInput, type SyncInitInput } from "opportune/web";',
      "",
      "const bytes: SyncInitInput = readFileSync(\"/opportune_wasm_bg.wasm\");",
      "const asset: InitInput = new URL(\"https://example.test/opportune_wasm_bg.wasm\");",
      "const promised: InitInput = Promise.resolve(asset);",
      "",
      "export async function start(): Promise<void> {",
      "  await init(promised);",
      "  initSync({ module: bytes });",
      "}",
      "",
    ].join("\n"),
  );
  // The package's own pinned `@types/node`, so the consumer needs no install and the
  // compile is the same one the package's `tsconfig.check.json` makes.
  const options = projectOptions(["ES2024"]);
  options.compilerOptions.types = ["node"];
  options.compilerOptions.typeRoots = [join(PACKAGE, "node_modules", "@types")];
  writeFileSync(
    join(dir, "tsconfig.json"),
    `${JSON.stringify(options, null, 2)}\n`,
  );

  const diagnostics = typechecks(dir);
  assert.equal(
    diagnostics,
    "",
    "`opportune/web` does not compile for a Node consumer whose lib is `ES2024` with " +
      "`@types/node` and `skipLibCheck: false`",
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
      `const { readFileSync } = require("node:fs");`,
      `const presets = require(${JSON.stringify(`${MANIFEST.name}/presets`)});`,
      `const graph = require(${JSON.stringify(`${MANIFEST.name}/adapters/graph`)});`,
      `const google = require(${JSON.stringify(`${MANIFEST.name}/adapters/google`)});`,
      `const web = require(${JSON.stringify(`${MANIFEST.name}/web`)});`,
      `const manifest = require(${JSON.stringify(`${MANIFEST.name}/package.json`)});`,
      `const schema = require(${JSON.stringify(`${MANIFEST.name}/schema.json`)});`,
      // The eighth export, and the one the browser documentation tells every bundler
      // consumer to import: `import wasmUrl from "opportune/opportune_wasm_bg.wasm?url"`
      // resolves through this same entry. Resolved, not required — a consumer's bundler
      // turns it into a URL, and `require()` of a binary would throw for the wrong reason.
      `const wasmAsset = require.resolve(${JSON.stringify(`${MANIFEST.name}/opportune_wasm_bg.wasm`)});`,
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
      '  wasmMagic: readFileSync(wasmAsset).subarray(0, 4).toString("latin1"),',
      "  tzdb,",
      "  limit,",
      "}));",
    ].join("\n"),
  );
  const run = spawnSync(process.execPath, [consumer], {
    cwd: dir,
    encoding: "utf8",
    timeout: CHILD_TIMEOUT_MS,
  });
  assert.equal(run.error, undefined, `the CommonJS consumer did not finish:\n${report(run)}`);
  assert.equal(run.signal, null, `the CommonJS consumer was killed:\n${report(run)}`);
  assert.equal(run.status, 0, `require() of the installed package failed:\n${report(run)}`);
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
  // An unmapped or mistyped asset subpath fails a consumer's build with
  // `ERR_PACKAGE_PATH_NOT_EXPORTED` while every test here stays green, because the
  // tarball-content cases check the file, not the entry that reaches it. Reading the
  // WebAssembly magic pins both ends: the documented entry resolves, and it resolves to
  // the module binary rather than to something a bundler would serve as a URL and hand
  // to `init(...)` as a document that is not WebAssembly.
  assert.equal(required.wasmMagic, WASM_MAGIC, "the asset subpath did not resolve to the module binary");
  // The same script built an engine, read the module's own time zone database version
  // and released it, all without a tick: a build whose glue needed the asynchronous
  // initialiser would fail here rather than in a consumer's server.
  assert.match(required.tzdb, /^\d{4}[a-z]$/, "the CommonJS consumer read a time zone database version");
  assert.equal(required.limit, 92, "the CommonJS consumer read the engine's default limits");
});

test("an ESM consumer imports every declared subpath and resolves each through import.meta.resolve", (t) => {
  // The CommonJS case above proves `require(esm)`; this is the other half a consumer has:
  // a native `import` through the installed package's `exports` map, the JSON entries read
  // with their import attributes, and `import.meta.resolve` for every declared subpath —
  // the asset included, resolved to the module binary itself rather than to something a
  // bundler would hand `init()` as a document that is not WebAssembly. Both lists are
  // derived from the manifest, so a subpath added to `exports` reaches this case without
  // anyone remembering to name it here.
  const dir = mkdtempSync(join(tmpdir(), "opportune-esm-"));
  t.after(() => rmSync(dir, { recursive: true, force: true }));
  installTarball(dir);

  const consumer = join(dir, "consumer.mjs");
  writeFileSync(
    consumer,
    [
      'import assert from "node:assert/strict";',
      'import { readFileSync } from "node:fs";',
      'import { fileURLToPath } from "node:url";',
      "",
      `const name = ${JSON.stringify(MANIFEST.name)};`,
      `const version = ${JSON.stringify(MANIFEST.version)};`,
      `const magic = ${JSON.stringify(WASM_MAGIC)};`,
      `const declared = ${JSON.stringify(EXPORTS)};`,
      `const json = ${JSON.stringify(JSON_EXPORTS.map((entry) => entry.specifier))};`,
      `const surface = ${JSON.stringify(Object.entries(SURFACE))};`,
      "",
      "// Every executable subpath is imported, and the binding a consumer reads is there.",
      "for (const [subpath, bindings] of surface) {",
      '  const specifier = subpath === "." ? name : `${name}/${subpath.slice(2)}`;',
      "  const module = await import(specifier);",
      "  for (const binding of bindings) {",
      "    assert.ok(binding in module, `${specifier} exports no ${binding}`);",
      "  }",
      "}",
      "",
      "// Every declared subpath resolves, and to the file the manifest names.",
      "for (const { specifier, target } of declared) {",
      "  const resolved = import.meta.resolve(specifier);",
      '  assert.ok(resolved.startsWith("file:"), `${specifier} resolved to ${resolved}`);',
      "  const path = fileURLToPath(resolved);",
      '  assert.ok(path.endsWith("/" + target), `${specifier} resolved to ${path}, not the declared ${target}`);',
      '  assert.ok(readFileSync(path).length > 0, `${specifier} resolved to an empty file`);',
      "}",
      "",
      "// The JSON subpaths a consumer validates or reads metadata through, with the import",
      "// attribute Node and Bun both require for a JSON module.",
      "const documents = {};",
      "for (const specifier of json) {",
      '  documents[specifier] = (await import(specifier, { with: { type: "json" } })).default;',
      "}",
      "",
      "const root = await import(name);",
      'const web = await import(name + "/web");',
      'const presets = await import(name + "/presets");',
      'const graph = await import(name + "/adapters/graph");',
      'const google = await import(name + "/adapters/google");',
      "",
      "assert.equal(root.VERSION, version);",
      'assert.equal(web.default, web.init, "opportune/web\'s default export is init");',
      'assert.equal(presets.businessHours().kind, "window");',
      'assert.deepEqual(presets.person("rich"), { person: "rich" });',
      'assert.equal(graph.calendarViewToBlocks({ value: [] }, "UTC").status, "available");',
      'assert.deepEqual(google.eventsListToBlocks({ items: [] }, "UTC"), {',
      '  status: "available",',
      "  blocks: [],",
      "  diagnostics: [],",
      "});",
      "",
      "// The engine really runs through the ESM entry: the module the Node loader requires",
      "// lazily is the same one the CJS consumer built above.",
      "const engine = new root.Engine();",
      "try {",
      "  assert.match(engine.tzdbVersion, /^\\d{4}[a-z]$/);",
      "  assert.equal(root.defaultLimits().max_search_window_days, 92);",
      "} finally {",
      "  engine.free();",
      "}",
      "",
      'const manifest = documents[name + "/package.json"];',
      'const schema = documents[name + "/schema.json"];',
      "assert.equal(manifest.version, version);",
      'assert.equal(schema.$schema, "https://json-schema.org/draft/2020-12/schema");',
      'assert.ok(schema.$defs?.Request !== undefined, "the schema carries the Request definition");',
      "",
      'const wasm = fileURLToPath(import.meta.resolve(name + "/opportune_wasm_bg.wasm"));',
      'assert.equal(readFileSync(wasm).subarray(0, magic.length).toString("latin1"), magic);',
      "",
      "console.log(JSON.stringify({",
      "  subpaths: declared.length,",
      "  executable: surface.length,",
      "  json: json.length,",
      '  runtime: process.versions.bun ? "bun" : "node",',
      "}));",
    ].join("\n"),
  );

  const run = spawnSync(process.execPath, [consumer], {
    cwd: dir,
    encoding: "utf8",
    timeout: CHILD_TIMEOUT_MS,
  });
  assert.equal(run.error, undefined, `the ESM consumer did not finish:\n${report(run)}`);
  assert.equal(run.signal, null, `the ESM consumer was killed:\n${report(run)}`);
  assert.equal(run.status, 0, `import() of the installed package failed:\n${report(run)}`);
  const imported = JSON.parse(run.stdout);
  assert.equal(imported.subpaths, EXPORTS.length, "the consumer resolved fewer subpaths than the manifest declares");
  assert.equal(imported.executable, EXECUTABLE_EXPORTS.length);
  assert.equal(imported.json, JSON_EXPORTS.length);
  // The spawned consumer is whichever runtime runs the suite, so the Bun lane proves this
  // case on Bun's loader rather than silently re-running it on Node.
  assert.equal(
    imported.runtime,
    process.versions.bun ? "bun" : "node",
    "the ESM consumer did not execute under the runtime that drove the suite",
  );
});
