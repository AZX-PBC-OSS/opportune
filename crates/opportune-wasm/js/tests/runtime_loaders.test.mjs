/**
 * The shipped JavaScript module graph is one each runtime can load, and the loaders in it
 * are the ones their runtime's condition names.
 *
 * Node's `require` of an ES module — how a CommonJS server reaches this package — is
 * synchronous only while no module in that graph awaits at the top level: one top-level
 * `await` turns the whole graph into a promise, and `require` fails with
 * `ERR_REQUIRE_ASYNC_MODULE` in a
 * consumer's application rather than here. Bun is no help either, because the same
 * module graph is what each runtime gets.
 *
 * Nothing in the source has such an `await` today, and nothing stops one being added:
 * a `fetch` of the `.wasm` at module scope, an `await import()`, a top-level
 * `await using` all read as ordinary code and pass every other case in this suite, which
 * run from ESM imports that can await. So the gate is static and reads the artefacts
 * rather than the sources — what a bundler and a consumer actually parse — and it pins
 * the whole tree, not the handful of files a reviewer remembers. The generated glues are
 * part of that tree: the `web` one is an ES module beside the package's own, and the
 * `nodejs` one is CommonJS, which no top-level `await` can even be written in but which
 * is scanned all the same so the walk covers every file the tarball ships as JavaScript.
 *
 * The checks are AST walks rather than patterns: a top-level `await` is a position in a
 * tree, and a regex for `await` matches the identifier `awaitCount`, a string in a doc
 * comment, and a function body that is perfectly legal. `@babel/parser` reads the emitted
 * JavaScript without relying on the programmatic API TypeScript 7 no longer ships. The
 * same walkers pin the bundled-runtime loaders' static `.wasm` imports, their distance
 * from dynamic compilation, and their lazy instantiation — properties a comment
 * mentioning them would otherwise satisfy.
 *
 * Build the package first:
 *
 * ```sh
 * make wasm && npm --prefix crates/opportune-wasm/js test
 * ```
 */

import assert from "node:assert/strict";
import { existsSync, mkdtempSync, readFileSync, readdirSync, rmSync, writeFileSync } from "node:fs";
import { createRequire } from "node:module";
import { tmpdir } from "node:os";
import { join, relative } from "node:path";
import test from "node:test";
import { pathToFileURL } from "node:url";
import { forEachChild, isCall, isMember, nodeText, parseSource, walk } from "./support/ast.mjs";

/** The package root this suite belongs to, and the tree the tarball packs. */
const PACKAGE = join(import.meta.dirname, "..");
const DIST = join(PACKAGE, "dist");

/** The manifest whose `imports` map selects the loader each runtime gets. */
const MANIFEST = JSON.parse(readFileSync(join(PACKAGE, "package.json"), "utf8"));

const require = createRequire(import.meta.url);

/** Every JavaScript file `npm pack` would carry, as paths relative to `dist/`. */
function shipped() {
  const found = [];
  const walk = (dir) => {
    for (const entry of readdirSync(dir, { withFileTypes: true })) {
      const path = join(dir, entry.name);
      if (entry.isDirectory()) {
        walk(path);
      } else if (entry.name.endsWith(".js") || entry.name.endsWith(".cjs")) {
        found.push(relative(DIST, path));
      }
    }
  };
  walk(DIST);
  return found.sort();
}

/**
 * Where `file` has a site `isSite` accepts outside a function, as `file:line` for each.
 *
 * A function boundary is any position a later call could run from — a declaration, an
 * expression, an arrow, a method, a constructor — so anything inside one is the caller's
 * and says nothing about the module's own evaluation.
 */
function topLevelSites(file, isSite) {
  const content = readFileSync(file, "utf8");
  const source = parseSource(file, content);
  const sites = [];
  const site = (node) => {
    sites.push(relative(DIST, file) + ":" + String(node.loc.start.line));
  };
  const visit = (node) => {
    if (isSite(node, content)) {
      site(node);
      return;
    }
    const inner = [
      "FunctionDeclaration",
      "FunctionExpression",
      "ArrowFunctionExpression",
      "ObjectMethod",
      "ClassMethod",
      "ClassPrivateMethod",
    ].includes(node.type);
    if (inner) {
      if (node.computed && node.key !== undefined) {
        // A method's body is deferred, but its computed name is evaluated with the
        // surrounding module, so an await there is still top-level.
        visit(node.key);
      }
      return;
    }
    forEachChild(node, visit);
  };
  visit(source);
  return sites;
}

/**
 * An `await` the module itself performs: an expression, a `for await`, or `await using`.
 *
 * All three are awaits even though the last two are written as statement modifiers.
 */
function AWAIT(node) {
  return (
    node.type === "AwaitExpression" ||
    (node.type === "ForOfStatement" && node.await) ||
    (node.type === "VariableDeclaration" && node.kind === "await using")
  );
}

/** Where `file` awaits outside a function, as `file:line` for each site. */
function topLevelAwaits(file) {
  return topLevelSites(file, AWAIT);
}

/**
 * Every module `file` asks for, as its specifier text and whether it is a dynamic import.
 *
 * A static import is part of the module graph a runtime resolves before it runs anything;
 * a dynamic one is deferred to the call that makes it. That difference is the whole of
 * what makes a `.wasm` import safe on `workerd` and unsafe on Node, so it is recorded.
 */
function importedSpecifiers(file) {
  const source = parseSource(file, readFileSync(file, "utf8"));
  const found = [];
  walk(source, (node) => {
    if (["ImportDeclaration", "ExportNamedDeclaration", "ExportAllDeclaration"].includes(node.type)) {
      if (node.source?.type === "StringLiteral") {
        found.push({ specifier: node.source.value, dynamic: false });
      }
    } else if (node.type === "ImportExpression" && node.source.type === "StringLiteral") {
      found.push({ specifier: node.source.value, dynamic: true });
    } else if (node.type === "CallExpression" && node.callee.type === "Import") {
      const [first] = node.arguments;
      if (first?.type === "StringLiteral") {
        found.push({ specifier: first.value, dynamic: true });
      }
    }
  });
  return found;
}

/**
 * Every place `file` reaches into the global `WebAssembly` namespace, as `file:line`.
 *
 * A comment mentioning `WebAssembly.compile` is not a call to it, and the loader's own
 * documentation says the name several times, so the check reads the tree rather than the
 * text.
 */
function webAssemblyUses(file) {
  const content = readFileSync(file, "utf8");
  const source = parseSource(file, content);
  const found = [];
  walk(source, (node) => {
    if (isMember(node) && nodeText(node.object, content) === "WebAssembly") {
      found.push(`${relative(DIST, file)}:${String(node.loc.start.line)}: ${nodeText(node, content)}`);
    }
  });
  return found;
}

/**
 * Every place `file` compiles or constructs a module itself, as `file:line: expression`.
 *
 * A loader that reached for `WebAssembly.compile`, `WebAssembly.instantiate`, or
 * `new WebAssembly.Module(bytes)` would be the dynamic compilation both bundled runtimes
 * refuse, written as an ordinary call; a comment saying the same is neither.
 */
function compilingUses(file) {
  const content = readFileSync(file, "utf8");
  const source = parseSource(file, content);
  const found = [];
  const record = (node, text) => {
    found.push(`${relative(DIST, file)}:${String(node.loc.start.line)}: ${text}`);
  };
  walk(source, (node) => {
    if (isCall(node) && isMember(node.callee)) {
      const target = nodeText(node.callee, content).replaceAll("?.", ".");
      if (target === "WebAssembly.compile" || target === "WebAssembly.instantiate") {
        record(node, target);
      }
    }
    if (node.type === "NewExpression") {
      const target = nodeText(node.callee, content);
      if (target === "WebAssembly.Module" || target === "WebAssembly.Instance") {
        record(node, `new ${target}`);
      }
    }
  });
  return found;
}

test("the top-level await walk recognises statement forms as well as expressions", (t) => {
  const dir = mkdtempSync(join(tmpdir(), "opportune-await-"));
  t.after(() => rmSync(dir, { recursive: true, force: true }));
  const fixtures = {
    "for-await.js": "for await (const value of []) { void value; }\nexport {};\n",
    "await-using.js": "await using resource = null;\nexport {};\n",
    "computed-method.js": "const value = { [await Promise.resolve('method')]() {} };\nexport { value };\n",
  };
  const found = {};
  for (const [name, source] of Object.entries(fixtures)) {
    const file = join(dir, name);
    writeFileSync(file, source);
    found[name] = topLevelAwaits(file);
  }
  assert.deepEqual(
    found,
    Object.fromEntries(Object.keys(fixtures).map((name) => [name, [`${relative(DIST, join(dir, name))}:1`]])),
    "each form makes Node reject require(esm) with ERR_REQUIRE_ASYNC_MODULE",
  );
});

test("no shipped module awaits at the top level", () => {
  const files = shipped();
  // The gate is only evidence if it read the tree: the entry point, the loader and both
  // generated glues are the files whose format matters most, and a `dist/` that was
  // never built would otherwise pass with nothing scanned.
  for (const required of [
    "index.js",
    "wasm-node.js",
    "wasm-workerd.js",
    "wasm-web.js",
    "pkg/opportune_wasm.js",
    "pkg/opportune_wasm.cjs",
  ]) {
    assert.ok(
      files.includes(required),
      `dist/${required} is missing, so the module graph was not built: run \`make wasm\``,
    );
  }
  const offenders = files.flatMap((file) => topLevelAwaits(join(DIST, file)));
  assert.deepEqual(
    offenders,
    [],
    "a top-level `await` makes the module graph asynchronous, so a CommonJS `require` of " +
      "this package fails with ERR_REQUIRE_ASYNC_MODULE:",
  );
});

test("each generated glue is the module system its target requires", async () => {
  // The package publishes one loader per runtime target, and what makes each of them a
  // loader is the module system `wasm-pack` wrote it in: `dist/pkg/opportune_wasm.js`
  // is the `web` target's ES module, whose `initSync` and default `init` are how the
  // Worker, browser and Web Worker slices initialise it, and `dist/pkg/opportune_wasm.cjs`
  // is the `nodejs` target's CommonJS module, which `#wasm` requires. Neither the walk
  // above nor the packed-package suite reads either file as what it is — the walk
  // parses both as plain JavaScript whatever their module system, and the packed suite
  // only lists them — so a build that copied one target's glue under the other's name
  // would leave a tarball whose loader for that runtime cannot be parsed, with every
  // gate green until the first consumer of that runtime tried it.
  const webGlue = await import(pathToFileURL(join(DIST, "pkg", "opportune_wasm.js")).href);
  assert.equal(typeof webGlue.WasmEngine, "function", "the web glue exports the engine class");
  assert.equal(
    typeof webGlue.initSync,
    "function",
    "the web glue exports the initialiser its runtime calls",
  );
  assert.equal(
    typeof webGlue.default,
    "function",
    "the web glue exports the asynchronous initialiser",
  );

  const nodeGlue = require(join(DIST, "pkg", "opportune_wasm.cjs"));
  assert.equal(typeof nodeGlue.WasmEngine, "function", "the node glue exports the engine class");
});

test("only the bundled-runtime loaders carry a static .wasm import", () => {
  // A runtime with no filesystem can only get the module as a module its bundle compiled,
  // which means a static `.wasm` import and a bundler rule to compile it — a
  // `CompiledWasm` rule for Wrangler. Nothing else may have one: Node reads and compiles
  // the binary itself, and the browser and Web Worker slices receive a URL the caller
  // supplies. A `.wasm` import leaking into any of those graphs is an import the runtime
  // either cannot resolve or silently resolves to something that is not a
  // `WebAssembly.Module`.
  const reachable = shipped().flatMap((file) =>
    importedSpecifiers(join(DIST, file))
      .filter(({ specifier }) => /\.wasm(?:\?module)?$/.test(specifier))
      .map(({ specifier, dynamic }) => ({ file, specifier, dynamic })),
  );
  assert.deepEqual(
    reachable,
    [
      { file: "wasm-workerd.js", specifier: "./pkg/opportune_wasm_bg.wasm", dynamic: false },
    ],
    "a static `.wasm` import outside the loader that needs one, or a dynamic one inside " +
      "it, is not a module graph the runtime it belongs to can resolve",
  );
});

test("the bundled-runtime loader compiles no bytes", () => {
  // `workerd` refuses `WebAssembly.compile` and `new WebAssembly.Module(bytes)` outright,
  // so the one thing this loader must never do is compile: it hands the precompiled module
  // to `initSync` and lets the generated glue instantiate it. Reading the tree rather than
  // the text matters here — the loader's own documentation names both calls, and a comment
  // is not a call.
  assert.deepEqual(
    webAssemblyUses(join(DIST, "wasm-workerd.js")),
    [],
    "dist/wasm-workerd.js reaches into the `WebAssembly` API, which workerd refuses",
  );
  assert.deepEqual(
    compilingUses(join(DIST, "wasm-workerd.js")),
    [],
    "the Worker loader compiles the module itself instead of instantiating the one its " +
      "bundle carries, which workerd refuses at run time",
  );
});

test("every loader instantiates on the first call, not when imported", () => {
  // Compilation is eager for the Worker — the bundle compiled the module at startup — but
  // instantiation is deferred, so a deployment that imports the package to reach a preset
  // builder or a generated type instantiates nothing. The web loader has no compilation at
  // all and must not load on import either. That deferral is the same one the Node loader
  // makes with `require`, and it is invisible to every behavioural case in this suite.
  for (const file of ["wasm-workerd.js", "wasm-web.js"]) {
    const sites = topLevelSites(
      join(DIST, file),
      (node, content) =>
        isCall(node) && /(?:initSync|\binit)$/.test(nodeText(node.callee, content)),
    );
    assert.deepEqual(
      sites,
      [],
      `dist/${file} instantiates the module while the module graph is evaluated, not on the ` +
        "first engine call",
    );
  }
});

test("the ambient .wasm declarations stay out of the emitted declarations", () => {
  // `src/wasm-bindgen.d.ts` declares the `*.wasm` specifier so the typechecker can read
  // the Worker loader on a checkout where the wasm build has not run. It is
  // an input `.d.ts`, so `tsc` emits nothing for it, and the package publishes only
  // `dist/` — but an ambient module declaration that reached a consumer's program would
  // tell its compiler that a `.wasm` import is a `WebAssembly.Module`, which is true only
  // inside a bundler that compiles one. The check reads the emitted tree, because that is
  // the half a consumer ever loads.
  const declarations = [];
  const walk = (dir) => {
    for (const entry of readdirSync(dir, { withFileTypes: true })) {
      const path = join(dir, entry.name);
      if (entry.isDirectory()) {
        walk(path);
      } else if (entry.name.endsWith(".d.ts")) {
        declarations.push(path);
      }
    }
  };
  walk(DIST);
  assert.notDeepEqual(declarations, [], "dist/ carries no declaration file, so this test proves nothing");
  assert.deepEqual(
    declarations
      .filter((file) => /declare module\s+"\*\.wasm/.test(readFileSync(file, "utf8")))
      .map((file) => relative(DIST, file)),
    [],
    "an emitted declaration ambiently declares a `.wasm` module, which only a bundler can honour",
  );
  assert.equal(
    existsSync(join(DIST, "wasm-bindgen.d.ts")),
    false,
    "the ambient declaration file was emitted into the published tree",
  );
});

test("the loader walkers read the tree, not the text", () => {
  // Each check above is only evidence while it still matches what it looks for. A comment
  // naming `WebAssembly.compile`, an import specifier in a string, and a deferred import
  // are the three shapes a text search would get wrong in either direction, so the
  // walkers are shown on a file that holds them: the module-scope call is found, the one
  // inside a function is not, and a comment is neither.
  const dir = mkdtempSync(join(tmpdir(), "opportune-loader-await-"));
  try {
    const file = join(dir, "fixture.js");
    writeFileSync(
      file,
      [
        "// new WebAssembly.Module(bytes) in a comment is not a call.",
        "new WebAssembly.Module(bytes);",
        "const deferred = () => new WebAssembly.Module(bytes);",
        "initSync({ module });",
        "const later = () => initSync({ module });",
        'import data from "./static.wasm";',
        'const dynamic = () => import("./dynamic.wasm");',
        "export { data, deferred, later, dynamic };",
        "",
      ].join("\n"),
    );
    const uses = webAssemblyUses(file);
    assert.equal(
      uses.length,
      2,
      "the WebAssembly walk counted the comment as a use, or missed a real one",
    );
    assert.deepEqual(
      uses.map((site) => site.split(":")[1]),
      ["2", "3"],
      "the WebAssembly walk reports the two calls and not the comment above them",
    );
    assert.deepEqual(
      importedSpecifiers(file),
      [
        { specifier: "./static.wasm", dynamic: false },
        { specifier: "./dynamic.wasm", dynamic: true },
      ],
      "the import walk missed a static import or could not tell it from a deferred one",
    );
    assert.equal(
      topLevelSites(
        file,
        (node, content) => isCall(node) && nodeText(node.callee, content).endsWith("initSync"),
      ).length,
      1,
      "the top-level call walk did not separate the module-scope initialisation from the deferred one",
    );
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

test("the loader walkers recognise optional calls and members", (t) => {
  const dir = mkdtempSync(join(tmpdir(), "opportune-loader-optional-"));
  t.after(() => rmSync(dir, { recursive: true, force: true }));
  const file = join(dir, "fixture.js");
  writeFileSync(
    file,
    [
      "WebAssembly?.compile(bytes);",
      "WebAssembly.compile?.(bytes);",
      "init?.({ module });",
      "initSync?.({ module });",
      "const later = () => init?.({ module });",
      "export { later };",
      "",
    ].join("\n"),
  );
  assert.equal(webAssemblyUses(file).length, 2, "optional WebAssembly members escaped the walk");
  assert.equal(compilingUses(file).length, 2, "optional WebAssembly calls escaped the walk");
  assert.equal(
    topLevelSites(file, isCall).length,
    4,
    "optional initialisers were not separated from the deferred call",
  );
});

test("the manifest resolves `#wasm` to the loader each runtime names", () => {
  // A condition list is matched in declaration order. The Worker condition has to come
  // before the browser and worker conditions a Wrangler bundle also supplies, and both
  // browser conditions have to come before `node`/`default`, or the runtime that names one
  // gets the filesystem loader it cannot use. `browser` and `worker` share the one web
  // loader: the runtime is the same and only the manifest key differs.
  const conditions = MANIFEST.imports["#wasm"];
  const BUNDLED = {
    workerd: "./dist/wasm-workerd.js",
    browser: "./dist/wasm-web.js",
    worker: "./dist/wasm-web.js",
  };
  for (const [condition, target] of Object.entries(BUNDLED)) {
    assert.equal(conditions[condition], target, `\`#wasm\` names no ${condition} loader`);
    const order = Object.keys(conditions);
    for (const later of ["node", "default"]) {
      assert.ok(
        order.indexOf(condition) < order.indexOf(later),
        `\`#wasm\` declares \`${later}\` before \`${condition}\`, so that runtime loads a loader that cannot run there`,
      );
    }
  }
  const order = Object.keys(conditions);
  for (const later of ["browser", "worker"])
  {
    assert.ok(
      order.indexOf("workerd") < order.indexOf(later),
      "`#wasm` declares a browser condition before `workerd`, so a Wrangler bundle resolves the wrong loader",
    );
  }
  // Every target the map names has to be in the build, including the generated glue the
  // bundled-runtime loaders initialise. The `types` entry is a declaration rather than a shipped
  // module, so it is checked as a file rather than through the module walk.
  const files = shipped();
  for (const target of Object.values(conditions).filter((entry) => entry.endsWith(".js"))) {
    assert.ok(files.includes(relative(DIST, join(PACKAGE, target))), `\`${target}\` is not in dist/`);
  }
  assert.ok(
    existsSync(join(PACKAGE, conditions.types)),
    `\`${conditions.types}\`, the type \`#wasm\` resolves for a consumer, was not emitted`,
  );
  assert.equal(MANIFEST.imports["#wasm-glue"], "./dist/pkg/opportune_wasm.js");
  assert.ok(
    files.includes(relative(DIST, join(PACKAGE, MANIFEST.imports["#wasm-glue"]))),
    "`#wasm-glue` names a file this build does not ship",
  );
});
