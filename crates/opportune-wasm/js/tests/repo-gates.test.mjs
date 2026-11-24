/**
 * The repository gates no compiler reads: the release version, the toolchain pin and
 * the npm install path.
 *
 * `make check-version` is the one place the six version sources and the toolchain pin
 * are compared, and it is run by `make lint`, by ci.yml's lint job and — through the
 * refactor this exercises — by publish.yml's `version-check` on the tag. Nothing else
 * reads it, so every branch of it has to be exercised here rather than trusted: a
 * comparison that silently stopped checking one of the six files, or a prefix rule
 * that widened to accept a drifted pin, would keep every lane green until the tag that
 * needed it, where the cost is a release rather than a pull request. Each case runs the
 * real target in a throwaway tree holding copies of the real inputs, so the cases read
 * the committed files for their shape without ever writing to the checkout.
 *
 * The npm half is the privileged half. publish.yml's `publish-npm` holds
 * `id-token: write` and reaches `npm ci` only through the Makefile's `$(JSON2TS)`
 * prerequisite, so `--ignore-scripts` on that recipe is the whole of the guarantee
 * that a dependency cannot run code in that job. Two things make the flag lossless and
 * both are assumptions about files, not code: no dependency in the committed lockfile
 * ships an install script, and the package declares no lifecycle hook of its own. A
 * dependency bump that adds either would not fail anything — it would just start being
 * skipped — so both are asserted.
 *
 * The last group pins the wiring, which is invisible to every executable check: the
 * export step uv needs before it compiles the extension outside the checkout, the two
 * gates delegating to the one target, and Pages serialising its deploys. Deleting any
 * of them leaves a green run and a silently weaker guarantee, which is exactly the
 * shape of regression a suite has to catch by reading for, since nothing executes it.
 *
 * The packaging group pins the manifest entry that keeps a published crate's own suite
 * buildable: a `[package] exclude` for a test target whose inputs are repository files a
 * `.crate` does not carry. It derives the crates and the targets from the tree rather
 * than naming the two that need an entry today, so a third cannot ship the same
 * unbuildable package with the pin green.
 *
 * The last case reads the one limit the three bindings state in code rather than in a
 * schema: `available_only`'s dated-entry ceiling is a constant in Rust, Python and
 * TypeScript, and only the Rust one is pinned by its own suite. A release that moves
 * the ceiling in one language's copy makes that binding refuse an availability the
 * others accept, and no single-language suite can see the difference.
 *
 * ```sh
 * node --test crates/opportune-wasm/js/tests/repo-gates.test.mjs
 * ```
 */

import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { createRequire } from "node:module";
import {
  cpSync,
  existsSync,
  copyFileSync,
  mkdirSync,
  mkdtempSync,
  readdirSync,
  readFileSync,
  rmSync,
  symlinkSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve, sep } from "node:path";
import test from "node:test";
import shellQuote from "shell-quote";
import { parse as parseYaml } from "yaml";

/** The TypeScript parser the package already installs for its build and module-graph gates. */
const TS = createRequire(import.meta.url)("typescript");

/** This suite sits at `crates/opportune-wasm/js/tests`, so the root is four levels up. */
const ROOT = join(import.meta.dirname, "..", "..", "..", "..");

/** The npm package the build and release gates run against. */
const JS_PACKAGE = join(ROOT, "crates", "opportune-wasm", "js");

/** The npm package's manifest, lockfile and pure constants module. */
const MANIFEST = "crates/opportune-wasm/js/package.json";
const LOCKFILE = "crates/opportune-wasm/js/package-lock.json";
const CONSTANTS = "crates/opportune-wasm/js/src/constants.ts";

/** The files `check-version` reads, relative to the repository root. */
const VERSION_SOURCES = [
  "Cargo.toml",
  "version.txt",
  ".release-please-manifest.json",
  MANIFEST,
  LOCKFILE,
  CONSTANTS,
];

const read = (path) => readFileSync(join(ROOT, path), "utf8");

/** Every workflow file, as repository-relative paths. */
function workflowFiles(
  dir = join(ROOT, ".github", "workflows"),
  prefix = ".github/workflows",
) {
  return readdirSync(dir)
    .filter((name) => name.endsWith(".yml") || name.endsWith(".yaml"))
    .sort()
    .map((name) => `${prefix}/${name}`);
}

/** The directory the runtime-integration harnesses live in. */
const INTEGRATION = "crates/opportune-wasm/integration";

/**
 * Every harness file an npm install could be written into, as repository-relative paths.
 *
 * A harness is a private npm project with its own lockfile, so its installs are its own
 * concern — and the repository has one rule for every install in it, not one rule for the
 * Makefile and another for a tree the Makefile happens to call. Lockfiles are data and are
 * skipped. A manifest is included for its `scripts`, which npm hands to a shell; the scanner
 * reads those values rather than treating prose elsewhere in the JSON as commands.
 */
function harnessFiles(dir = join(ROOT, INTEGRATION), prefix = INTEGRATION) {
  if (!existsSync(dir)) {
    return [];
  }
  return readdirSync(dir, { withFileTypes: true })
    .flatMap((entry) => {
      const path = `${prefix}/${entry.name}`;
      if (entry.isDirectory()) {
        // Installs and build output are not harness sources: `node_modules`, esbuild's
        // `dist`, Vitest's caches and Playwright's reports are all generated trees.
        const generated = [
          "node_modules",
          "dist",
          "test-results",
          "playwright-report",
          ".git",
          ".wrangler",
          ".vitest",
        ].includes(entry.name);
        return generated ? [] : harnessFiles(join(dir, entry.name), path);
      }
      return entry.name === "package.json" || /(?:^Makefile$|\.(?:sh|[cm]?[jt]sx?|ya?ml)$)/.test(entry.name)
        ? [path]
        : [];
    })
    .sort();
}

/** Shell control operators that finish one command and can begin another. */
const COMMAND_SEPARATORS = new Set([";", ";;", ";&", ";;&", "&", "&&", "||", "|", "|&"]);

/** The npm subcommands that install packages, including both commands' documented aliases. */
const INSTALL_SUBCOMMAND_SOURCE =
  "ci|clean-install|ic|install-clean|isntall-clean|install|add|i|in|ins|inst|insta|instal|isnt|isnta|isntal|isntall";
const INSTALL_SUBCOMMAND = new RegExp(`^(?:${INSTALL_SUBCOMMAND_SOURCE})$`);
const INSTALL_SUBCOMMAND_WORD = new RegExp(`\\b(?:${INSTALL_SUBCOMMAND_SOURCE})\\b`);
const DEFAULT_INSTALL = new RegExp(
  `\\$\\{[A-Za-z_][A-Za-z0-9_]*:-(${INSTALL_SUBCOMMAND_SOURCE})\\}`,
  "g",
);

/** The commands a shell script holds, as word lists; operators split commands. */
function shellCommands(script) {
  // The shell deletes a backslash-newline before parsing — it joins the word, it does not
  // insert a space — so a word split across a continuation (`np\`⏎`m ci`) cannot evade the
  // gate as two unrelated words. Parse each remaining physical line separately: shell-quote's
  // comment token extends to the end of its input, so replacing newlines with semicolons
  // first would let one `#` hide every later command. `shell-quote` owns quoting, escapes,
  // substitutions and comments; this function only groups its tokens into commands.
  //
  // Every script is parsed, not only one naming a literal `npm`: a variable command name
  // (`$NPM ci`) is an install too, and the variable may be set outside the script — in the
  // workflow's `env`, or in an earlier step's exported environment. The caller judges the
  // token lists; a script that names no install is dropped there.
  const lines = script
    // `${SUBCOMMAND:-ci}` can select the default install. Judge that installing
    // outcome without treating every expansion after npm as a subcommand.
    .replace(DEFAULT_INSTALL, "$1")
    .replace(/\\\r?\n/g, "")
    .split(/\r?\n/);
  return lines.flatMap((line) => {
    let tokens;
    try {
      tokens = shellQuote.parse(line);
    } catch {
      // A line the shell parser cannot read is one this gate cannot judge; report it as
      // a single unparseable command rather than passing it. The caller drops it only
      // when the line names no install.
      return [[line]];
    }
    const commands = [[]];
    for (const token of tokens) {
      if (typeof token === "string") {
        // A command quoted for shell evaluation (`eval "npm ci"`, `sh -c "npm ci"`)
        // arrives as one word. Split only a word that itself contains an install;
        // ordinary quoted arguments retain their whitespace.
        const words = /\s/.test(token) && /\bnpm\b/.test(token) && INSTALL_SUBCOMMAND_WORD.test(token)
          ? token.split(/\s+/).filter(Boolean)
          : [token];
        commands.at(-1).push(...words);
        continue;
      }
      if ("comment" in token) break;
      if ("op" in token) {
        // Parentheses and redirections stay with their command: `$(JS)` arrives as `$`,
        // `(`, `JS`, `)` tokens, and splitting there would strand the install's own
        // subcommand. Nested npm invocations are rejected as ambiguous below instead.
        if (COMMAND_SEPARATORS.has(token.op) && commands.at(-1).length > 0) {
          commands.push([]);
        }
        continue;
      }
      // A variable (`$NPM`, which shell-quote leaves blank when unset) is a word with
      // no literal value to read; a glob reaches the parser as an `op` token and is
      // dropped above, so neither contributes a word to judge here.
      commands.at(-1).push("");
    }
    return commands.filter((words) => words.length > 0);
  });
}

/** The last relevant option; npm's last boolean value wins. */
function lastIgnoreScriptsOption(words) {
  return words.filter((word) => /^--(?:no-)?ignore-scripts(?:=.*)?$/.test(word)).at(-1);
}

/** A word that invokes npm, including an absolute or relative executable path. */
const NPM_COMMAND = /^(?:.*\/)?npm(?:\.(?:cjs|cmd|exe))?$/;

/** Whether `words` invoke npm with one of its installing subcommands. */
function runsNpmInstall(words) {
  // Associate an exact subcommand with an npm command word. Filenames (`install.js`),
  // eval strings and shell's `in` keyword are arguments, not npm install aliases.
  return words.some((word, index) =>
    NPM_COMMAND.test(word) &&
    words.slice(index + 1).some((argument) => INSTALL_SUBCOMMAND.test(argument))
  );
}

/** JavaScript process functions whose first argument is a command to execute. */
const PROCESS_COMMANDS = new Set(["spawn", "spawnSync", "exec", "execSync", "execFile", "execFileSync"]);

/** Whether a variable command name hides an installing subcommand, including behind a shell wrapper. */
function runsDynamicInstall(words) {
  if (/\bnpm\b/.test(words.join(" "))) return false;
  // Only a dynamic word in command position installs something: `command $NPM ci` and
  // `env $NPM ci` put a pass-through wrapper in front of it, so the first word after
  // the leading assignments decides. A dynamic word in argument position (`grep -q
  // $PATTERN ci.yml`, `tar -xzf "$ARCHIVE" -C /tmp/install`) is data, not a command —
  // flagging it would fail every ordinary workflow step that interpolates a value.
  const first = words.findIndex((word) => !/^[A-Za-z_][A-Za-z0-9_]*=/.test(word));
  const wrappers = new Set(["command", "env", "nice", "nohup", "time", "timeout", "sudo"]);
  const dynamic =
    words[first] === "" ||
    (wrappers.has(words[first] ?? "") && words.slice(first + 1).includes(""));
  return dynamic && words.slice(first + 1).some((word) => INSTALL_SUBCOMMAND.test(word));
}

/** Whether one literal npm invocation owns the options in `words`. */
function hasOneNpmInvocation(words) {
  // shell-quote does not evaluate command/process substitution. If one token list names
  // npm twice, an outer install's flag could be masking a nested install, so fail closed.
  return (words.join(" ").match(/\bnpm\b/g) ?? []).length === 1;
}

/** The `run` scripts a workflow holds. */
function workflowScripts(file, content) {
  const workflow = parseYaml(content);
  assert.equal(typeof workflow, "object", `${file} is a YAML document`);
  const scripts = [];
  for (const job of Object.values(workflow.jobs ?? {})) {
    for (const step of job?.steps ?? []) {
      if (typeof step?.run === "string") scripts.push({ file, text: step.run });
    }
  }
  return scripts;
}

/** A package manifest's `scripts` values, and nothing else in the file. */
function packageScripts(file, content) {
  const manifest = JSON.parse(content);
  return Object.values(manifest.scripts ?? {})
    .filter((command) => typeof command === "string")
    .map((text) => ({ file, text }));
}

/** A Markdown page's shell text: fenced code holds recipes whose continuations join. */
function markdownScripts(file, content) {
  return makefileScripts(file, content);
}

/** Whether a node is the module name of Node's process-spawning API. */
function isChildProcessModule(node) {
  return TS.isStringLiteral(node) && ["child_process", "node:child_process"].includes(node.text);
}

/** Whether an initializer is a direct `require("child_process")` call. */
function isChildProcessRequire(node) {
  return node !== undefined &&
    TS.isCallExpression(node) &&
    TS.isIdentifier(node.expression) &&
    node.expression.text === "require" &&
    node.arguments.length === 1 &&
    isChildProcessModule(node.arguments[0]);
}

/** Local process-function and whole-module bindings imported or required from child_process. */
function childProcessBindings(source) {
  const namespaces = new Set();
  const functions = new Map();
  const addFunction = (imported, local) => {
    if (PROCESS_COMMANDS.has(imported)) functions.set(local, imported);
  };
  for (const statement of source.statements) {
    if (!TS.isImportDeclaration(statement) || !isChildProcessModule(statement.moduleSpecifier)) continue;
    const clause = statement.importClause;
    if (clause === undefined || clause.isTypeOnly) continue;
    if (clause.name !== undefined) namespaces.add(clause.name.text);
    if (clause.namedBindings !== undefined && TS.isNamespaceImport(clause.namedBindings)) {
      namespaces.add(clause.namedBindings.name.text);
    }
    if (clause.namedBindings !== undefined && TS.isNamedImports(clause.namedBindings)) {
      for (const element of clause.namedBindings.elements) {
        if (!element.isTypeOnly) addFunction(element.propertyName?.text ?? element.name.text, element.name.text);
      }
    }
  }
  const visit = (node) => {
    if (TS.isVariableDeclaration(node) && isChildProcessRequire(node.initializer)) {
      if (TS.isIdentifier(node.name)) namespaces.add(node.name.text);
      if (TS.isObjectBindingPattern(node.name)) {
        for (const element of node.name.elements) {
          if (TS.isIdentifier(element.name)) {
            addFunction(element.propertyName?.text ?? element.name.text, element.name.text);
          }
        }
      }
    }
    node.forEachChild(visit);
  };
  visit(source);
  return { namespaces, functions };
}

/** The canonical process-function name a call invokes, excluding methods such as RegExp.exec. */
function processCallee(node, bindings) {
  if (!TS.isCallExpression(node)) return undefined;
  if (TS.isIdentifier(node.expression)) {
    return bindings.functions.get(node.expression.text) ??
      (PROCESS_COMMANDS.has(node.expression.text) ? node.expression.text : undefined);
  }
  if (
    TS.isPropertyAccessExpression(node.expression) &&
    TS.isIdentifier(node.expression.expression) &&
    bindings.namespaces.has(node.expression.expression.text) &&
    PROCESS_COMMANDS.has(node.expression.name.text)
  ) {
    return node.expression.name.text;
  }
  return undefined;
}

/** Literal command text, retaining a shell-visible placeholder for template substitutions. */
function commandText(node) {
  if (TS.isStringLiteral(node) || TS.isNoSubstitutionTemplateLiteral(node)) return node.text;
  if (TS.isTemplateExpression(node)) {
    return node.head.text + node.templateSpans
      .map((span) => `$OPPORTUNE_DYNAMIC${span.literal.text}`)
      .join("");
  }
  return undefined;
}

/** Whether a literal options argument explicitly enables a command shell. */
function usesShell(node) {
  return node.arguments.slice(1).some((argument) =>
    TS.isObjectLiteralExpression(argument) && argument.properties.some((property) => {
      if (
        !TS.isPropertyAssignment(property) ||
        !(TS.isIdentifier(property.name) || TS.isStringLiteral(property.name)) ||
        property.name.text !== "shell"
      ) {
        return false;
      }
      return property.initializer.kind === TS.SyntaxKind.TrueKeyword ||
        (commandText(property.initializer)?.length ?? 0) > 0;
    })
  );
}

/** The command word lists one process call executes, without rewriting its source text. */
function processCommands(node, callee) {
  const command = node.arguments[0];
  if (command === undefined) return [];
  const text = commandText(command);
  if (callee === "exec" || callee === "execSync") {
    return text === undefined ? [] : shellCommands(text);
  }

  const arguments_ = node.arguments[1];
  if (text !== undefined && usesShell(node)) {
    if (arguments_ === undefined || !TS.isArrayLiteralExpression(arguments_)) {
      return shellCommands(text);
    }
    const args = arguments_.elements.map(commandText);
    const suffix = args.length > 0
      ? ` ${args.map((argument) =>
          argument === undefined ? "$OPPORTUNE_DYNAMIC" : shellQuote.quote([argument])
        ).join(" ")}`
      : "";
    return shellCommands(`${text}${suffix}`);
  }

  // Without `shell`, spawn/execFile take an executable and an argument array, not shell
  // source. Keep those words structured so whitespace, quotes and punctuation stay data.
  const words = [text ?? ""];
  if (arguments_ !== undefined && TS.isArrayLiteralExpression(arguments_)) {
    for (const element of arguments_.elements) {
      words.push(commandText(element) ?? "");
    }
  }
  return [words];
}

/** Process invocations in JavaScript/TypeScript, or physical lines in a shell source. */
function sourceScripts(file, content) {
  if (!/\.[cm]?[jt]sx?$/.test(file)) {
    return content.split(/\r?\n/).map((text, index) => ({ file, line: index + 1, text }));
  }

  const kind = /\.tsx$/.test(file)
    ? TS.ScriptKind.TSX
    : /\.jsx$/.test(file)
      ? TS.ScriptKind.JSX
      : /\.[cm]?ts$/.test(file)
        ? TS.ScriptKind.TS
        : TS.ScriptKind.JS;
  const source = TS.createSourceFile(file, content, TS.ScriptTarget.ESNext, true, kind);
  const bindings = childProcessBindings(source);
  const scripts = [];
  const visit = (node) => {
    const callee = processCallee(node, bindings);
    if (callee !== undefined) {
      const commands = processCommands(node, callee);
      if (commands.length > 0) {
        scripts.push({
          file,
          line: source.getLineAndCharacterOfPosition(node.getStart(source)).line + 1,
          text: node.getText(source),
          commands,
        });
      }
    }
    node.forEachChild(visit);
  };
  visit(source);
  return scripts;
}

/** A Makefile's recipe lines, joined across shell continuations. */
function makefileScripts(file, content) {
  const units = [];
  let current;
  for (const [index, raw] of content.split("\n").entries()) {
    const text = raw.replace(/\r$/, "");
    current ??= { file, line: index + 1, text: "" };
    current.text += text;
    // An odd run of trailing backslashes continues the recipe onto the next line, and the
    // shell deletes the backslash and the break, so the continuation joins the word.
    if (/(?<!\\)(?:\\\\)*\\$/.test(text)) {
      current.text = current.text.slice(0, -1);
      continue;
    }
    units.push(current);
    current = undefined;
  }
  if (current !== undefined) units.push(current);
  return units;
}

/**
 * The npm installs `files` hold, and the ones that do not skip lifecycle scripts.
 *
 * A workflow is YAML and a package manifest is JSON, so both are read with the parser
 * that owns their grammar; a Makefile recipe is shell text with backslash continuations,
 * and a Markdown fenced block joins them the same way. JavaScript/TypeScript process
 * calls come from the TypeScript AST with their argument arrays intact; shell text and
 * `exec` command strings are tokenised with the shell parser. Each install is judged on
 * the options of its own command: a flag to the left of `;`/`&&`/`|` does not make the
 * install to the right safe.
 */
function npmInstalls(files) {
  const installs = [];
  const offenders = [];
  for (const [file, content] of files) {
    // Each format is read with the parser that owns it, and the branch is explicit: a
    // Markdown page's fenced shell is the one other text format whose continuations join
    // like a recipe's. JavaScript/TypeScript sources yield their actual process calls;
    // an unknown extension is not silently treated as a Makefile.
    const scripts = /(?:^|\/)package\.json$/.test(file)
      ? packageScripts(file, content)
      : /\.ya?ml$/.test(file)
        ? workflowScripts(file, content)
        : /(?:^|\/)Makefile$/.test(file)
          ? makefileScripts(file, content)
          : /\.md$/.test(file)
            ? markdownScripts(file, content)
            : sourceScripts(file, content);
    for (const { line, text, commands: parsed } of scripts) {
      const commands = parsed ?? shellCommands(text);
      const installing = commands.filter(
        (words) => runsNpmInstall(words) || runsDynamicInstall(words),
      );
      // A word the shell expands — `NPM=npm; $NPM ci` — has no literal npm name. Judge
      // that command too, including when a separate literal install already supplied a
      // flag; otherwise the safe command would make the dynamic one invisible. Never
      // flatten commands for this decision: an npm variable in one and shell's `in`
      // keyword in another are not an invocation.
      if (installing.length === 0) continue;
      const site = line === undefined ? file : `${file}:${line}`;
      installs.push(site);
      const safe =
        installing.length > 0 &&
        installing.every((words) =>
          hasOneNpmInvocation(words) && lastIgnoreScriptsOption(words) === "--ignore-scripts"
        );
      if (!safe) offenders.push(`${site}: ${text.trim()}`);
    }
  }
  return { installs, offenders };
}

/** One key of the workspace manifest's `[workspace.package]` table. */
function workspacePackage(key) {
  const section = read("Cargo.toml").split("[workspace.package]")[1]?.split("\n[")[0];
  const declared = new RegExp(`^${key} = "([^"]+)"$`, "m").exec(section ?? "");
  assert.ok(declared, `the workspace manifest declares ${key}`);
  return declared[1];
}

/** The release version, as the file release-please bumps states it. */
const version = () => read("version.txt").trim();

/** The next patch above `current`, so a drift case never repeats the committed value. */
function bumped(current) {
  const parts = current.split(".");
  return [...parts.slice(0, -1), String(Number(parts.at(-1)) + 1)].join(".");
}

/**
 * A throwaway tree holding the gate beside copies of every file it reads.
 *
 * The target resolves all of its inputs relative to the working directory, so a copy of
 * the Makefile and the version sources is a complete fixture: nothing is written to the
 * checkout, and each case starts from the committed values.
 */
function sandbox() {
  const root = mkdtempSync(join(tmpdir(), "opportune-release-gate-"));
  for (const file of ["Makefile", "rust-toolchain.toml", ...VERSION_SOURCES]) {
    const target = join(root, file);
    mkdirSync(dirname(target), { recursive: true });
    writeFileSync(target, read(file));
  }
  return root;
}

/** Runs the real target against `root` and returns the finished process. */
function checkVersion(root) {
  const result = spawnSync("make", ["check-version"], { cwd: root, encoding: "utf8" });
  assert.equal(result.error, undefined, `make check-version ran: ${result.error}`);
  return result;
}

/** Runs `body` against a fresh sandbox, removing the tree whatever the body does. */
function inSandbox(body) {
  const root = sandbox();
  try {
    return body(root);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
}

/** A `rust-toolchain.toml` naming `channel`, or declaring none when it is empty. */
function toolchainPinning(channel) {
  const declared = channel === "" ? "" : `channel = "${channel}"\n`;
  return `[toolchain]\n${declared}components = ["clippy", "rustfmt"]\nprofile = "minimal"\n`;
}

/** The `jobs` table of a workflow, parsed, keyed by job id. */
function jobsIn(text) {
  const workflow = parseYaml(text);
  assert.equal(typeof workflow, "object", "the workflow is a YAML document");
  assert.equal(typeof workflow.jobs, "object", "the workflow declares its jobs at the top level");
  return new Map(Object.entries(workflow.jobs));
}

test("the committed tree passes check-version", () => {
  // The control: without it a gate that failed for every input would satisfy every
  // drift case below.
  const result = inSandbox(checkVersion);
  assert.equal(result.status, 0, `check-version rejected the committed tree:\n${result.stdout}${result.stderr}`);
  for (const source of VERSION_SOURCES) {
    assert.ok(result.stdout.includes(version()), `${source} is not reported by check-version`);
  }
});

test("a version drifting in any one of the six sources fails check-version", () => {
  // release-please's `extra-files` is the only thing that moves these six together, and
  // the gate's whole job is to catch the PR where one of them was edited by hand. A
  // comparison that dropped any one of them would leave the drift to fail at the tag,
  // where every publishing lane is gated on it, instead of on the PR that caused it.
  const drifted = bumped(version());
  const mutations = {
    "version.txt": (root) => writeFileSync(join(root, "version.txt"), `${drifted}\n`),
    ".release-please-manifest.json": (root) => {
      const path = join(root, ".release-please-manifest.json");
      const manifest = JSON.parse(readFileSync(path, "utf8"));
      manifest["."] = drifted;
      writeFileSync(path, `${JSON.stringify(manifest, null, 2)}\n`);
    },
    [MANIFEST]: (root) => {
      const path = join(root, MANIFEST);
      const manifest = JSON.parse(readFileSync(path, "utf8"));
      manifest.version = drifted;
      writeFileSync(path, `${JSON.stringify(manifest, null, 2)}\n`);
    },
    [`${LOCKFILE}: $.version`]: (root) => {
      const path = join(root, LOCKFILE);
      const lockfile = JSON.parse(readFileSync(path, "utf8"));
      lockfile.version = drifted;
      writeFileSync(path, `${JSON.stringify(lockfile, null, 2)}\n`);
    },
    [`${LOCKFILE}: $.packages[""].version`]: (root) => {
      const path = join(root, LOCKFILE);
      const lockfile = JSON.parse(readFileSync(path, "utf8"));
      lockfile.packages[""].version = drifted;
      writeFileSync(path, `${JSON.stringify(lockfile, null, 2)}\n`);
    },
    [CONSTANTS]: (root) => {
      const path = join(root, CONSTANTS);
      const source = readFileSync(path, "utf8");
      const edited = source.replace(
        `export const VERSION: string = "${version()}";`,
        `export const VERSION: string = "${drifted}";`,
      );
      assert.notEqual(edited, source, "constants.ts states the release version");
      writeFileSync(path, edited);
    },
    "Cargo.toml": (root) => {
      const path = join(root, "Cargo.toml");
      const [head, tail] = readFileSync(path, "utf8").split("[workspace.package]");
      assert.ok(head !== undefined && tail !== undefined, "Cargo.toml has a [workspace.package] table");
      const edited = tail.replace(`version = "${version()}"`, `version = "${drifted}"`);
      assert.notEqual(edited, tail, "the workspace table states the release version");
      writeFileSync(path, `${head}[workspace.package]${edited}`);
    },
  };

  for (const [source, mutate] of Object.entries(mutations)) {
    const result = inSandbox((root) => {
      mutate(root);
      return checkVersion(root);
    });
    const output = `${result.stdout}${result.stderr}`;
    assert.notEqual(result.status, 0, `${source} at ${drifted} passed check-version:\n${output}`);
    assert.match(output, /version drift/, `${source} failed without naming the drift:\n${output}`);
    assert.ok(
      output.includes(drifted),
      `the diagnostic does not name ${drifted}, the value that drifted:\n${output}`,
    );
  }
});

test("the toolchain pin has to name the declared MSRV", () => {
  // `rust-toolchain.toml` decides which compiler every checkout and every release build
  // uses, and `rust-version` is the floor the crate promises. A pin left behind by an MSRV
  // bump compiles everything with a compiler nobody declared, which is why the gate
  // compares them. The bounds are what the shell `case` actually buys: a patch above the
  // floor or the three-component spelling rustup installs name the same compiler, while a
  // longer number sharing the prefix (`1.1000.0` for `1.100`) and a newer series do not.
  // A naive `startswith` would accept the first of those.
  const msrv = workspacePackage("rust-version");
  const [major, minor] = msrv.split(".");
  const newerSeries = `${major}.${Number(minor) + 1}.0`;
  const cases = [
    [msrv, true, "the declared floor itself"],
    [`${msrv}.0`, true, "the same floor with the patch component rustup names"],
    [`${msrv}.1`, true, "a patch release above the floor"],
    [`${msrv}0.0`, false, "a longer number that merely shares the prefix"],
    [newerSeries, false, "a newer series than the floor"],
    ["stable", false, "an unpinned channel"],
    ["", false, "no channel at all"],
  ];

  for (const [channel, accepted, why] of cases) {
    const result = inSandbox((root) => {
      writeFileSync(join(root, "rust-toolchain.toml"), toolchainPinning(channel));
      return checkVersion(root);
    });
    const output = `${result.stdout}${result.stderr}`;
    assert.equal(
      result.status === 0,
      accepted,
      `channel "${channel}" (${why}) was ${accepted ? "rejected" : "accepted"}:\n${output}`,
    );
    if (!accepted) {
      assert.match(output, /toolchain drift/, `channel "${channel}" (${why}) failed without naming the drift:\n${output}`);
    }
  }
});

test("every npm install in the repository skips dependency lifecycle scripts", () => {
  // publish.yml's publish-npm is the job holding `id-token: write`, and it reaches
  // `npm ci` only through the Makefile's `$(JSON2TS)` prerequisite: one flag on that
  // recipe, not a call in the workflow. A new install site — or a flag dropped from the
  // existing one — would run a dependency's install script in the privileged job with
  // nothing failing, so the invariant is checked across the whole repository rather than
  // on any single call site. The runtime harnesses are read for the same reason and not
  // because they run there: they never do, which is the point of the isolation the next
  // case pins.
  const harness = harnessFiles();
  const { installs, offenders } = npmInstalls([
    ["Makefile", read("Makefile")],
    [MANIFEST, read(MANIFEST)],
    ...workflowFiles().map((file) => [file, read(file)]),
    ...harness.map((file) => [file, read(file)]),
  ]);

  assert.notDeepEqual(installs, [], "no npm install was found, so this test proves nothing");
  assert.notDeepEqual(harness, [], `no harness file was read from ${INTEGRATION}`);
  assert.deepEqual(
    offenders,
    [],
    "these npm installs run dependency lifecycle scripts; in publish-npm that is " +
      "arbitrary code in the job holding id-token: write",
  );
});

test("the runtime harnesses stay private, locked and outside the published package", () => {
  // A harness installs a pinned tree of its own — `workerd` and `@cloudflare/vitest-plugin` — so it
  // must never travel in the tarball, and it must never be publishable itself. Neither is
  // enforced by anything npm does: `files` decides what the package ships, and `private`
  // is the only thing that stops an accidental `npm publish` of the harness. The directory
  // is a sibling of the package rather than a child of it, which is the arrangement this
  // pins: a harness moved inside `crates/opportune-wasm/js` would be packed whatever
  // `files` says.
  assert.ok(existsSync(join(ROOT, INTEGRATION)), `${INTEGRATION} holds no runtime harness`);
  const harnesses = readdirSync(join(ROOT, INTEGRATION), { withFileTypes: true })
    .filter((entry) => entry.isDirectory())
    .map((entry) => `${INTEGRATION}/${entry.name}`)
    .sort();
  assert.notDeepEqual(harnesses, [], `no harness lives under ${INTEGRATION}`);

  const pkg = join(ROOT, JS_PACKAGE);
  for (const dir of harnesses) {
    const manifest = JSON.parse(read(`${dir}/package.json`));
    assert.equal(manifest.private, true, `${dir} is not a private package, so npm could publish it`);
    assert.ok(
      existsSync(join(ROOT, dir, "package-lock.json")),
      `${dir} commits no lockfile, so \`npm ci --ignore-scripts\` has nothing to install from`,
    );
    assert.deepEqual(
      Object.keys(manifest.dependencies ?? {}),
      [],
      `${dir} declares a runtime dependency, and a harness exists to test the package's own`,
    );
    assert.ok(
      !resolve(ROOT, dir).startsWith(`${pkg}${sep}`),
      `${dir} sits inside the npm package's directory, where npm packs it whatever \`files\` says`,
    );
  }
  assert.deepEqual(
    JSON.parse(read(MANIFEST)).files.filter((entry) => entry.includes("integration")),
    [],
    "the npm package's `files` list names a runtime harness",
  );
});

test("the runtime-integration CI jobs are unprivileged, bounded and fed by the packed tarball", () => {
  // Each runtime a bundle-only loader serves has a job that installs a pinned tree and
  // executes it, so each may hold nothing but `contents: read` and must be bounded; the
  // flag on its installs is the general gate's business, and the isolation is this
  // one's. Each consumes the tarball `test-wasm` uploaded rather than rebuilding the
  // package, so what it runs is the artefact a consumer installs. The workflow is parsed
  // as YAML, so a permission, timeout or step cannot be hidden from the check by
  // indentation or a comment.
  //
  // The job set is derived rather than listed: a runtime-integration job is one that
  // downloads the tarball `test-wasm` uploads, which is the arrangement the isolation
  // exists for. Naming the two jobs this repository has today would leave a third runtime
  // lane installing a pinned tree with whatever permissions its author remembered, and
  // every gate green. The two names are still asserted present, so renaming a lane
  // narrows the derived set here instead of silently dropping it.
  const TARBALL = "npm-package-tarball";
  const EXPECTED = ["test-wasm-workerd", "test-wasm-web"];
  const downloadsTarball = (job) =>
    (job.steps ?? []).some(
      (step) =>
        /^actions\/download-artifact@[0-9a-f]{40}$/.test(step.uses ?? "") &&
        step.with?.name === TARBALL,
    );
  const jobs = jobsIn(read(".github/workflows/ci.yml"));
  const runtime = [...jobs]
    .filter(([, job]) => downloadsTarball(job))
    .sort(([left], [right]) => left.localeCompare(right));
  const names = runtime.map(([name]) => name);
  for (const name of EXPECTED) {
    assert.ok(
      names.includes(name),
      `ci.yml no longer feeds ${name} from the packed tarball, so the isolation this case ` +
        "pins has nothing to check",
    );
  }
  for (const [name, job] of runtime) {
    assert.deepEqual(
      job.permissions,
      { contents: "read" },
      `${name} must hold \`contents: read\` and no other permission`,
    );
    assert.equal(typeof job["timeout-minutes"], "number", `${name} has no explicit timeout`);
    assert.ok(
      [].concat(job.needs ?? []).includes("test-wasm"),
      `${name} does not wait for the build`,
    );
    const steps = job.steps ?? [];
    assert.ok(
      steps.some((step) => /^actions\/download-artifact@[0-9a-f]{40}$/.test(step.uses ?? "")),
      `${name} downloads no pinned artefact`,
    );
    assert.ok(
      steps.some((step) => /make test-wasm-[a-z-]+ [A-Z][A-Z0-9_]*TARBALL=/.test(step.run ?? "")),
      `${name} builds the package instead of running the artefact it downloaded`,
    );
  }

  const producer = jobs.get("test-wasm");
  assert.ok(producer !== undefined, "ci.yml declares no test-wasm job");
  const producerSteps = producer.steps ?? [];
  assert.ok(
    producerSteps.some((step) => /^actions\/upload-artifact@[0-9a-f]{40}$/.test(step.uses ?? "")),
    "test-wasm uploads no pinned artefact",
  );
  assert.ok(
    producerSteps.some((step) => step.with?.name === TARBALL),
    "test-wasm uploads the tarball the runtime-integration jobs wait for",
  );

  // The other half of the isolation: no job that holds `id-token: write` may reach a
  // harness at all. `publish-npm` is the one that matters, and the only way in is a
  // Makefile target — which is a name in a workflow step.
  for (const file of workflowFiles()) {
    for (const [jobName, job] of jobsIn(read(file))) {
      if (job.permissions?.["id-token"] !== "write") continue;
      const commands = (job.steps ?? []).map((step) => step.run ?? "").join("\n");
      assert.ok(
        !commands.includes(INTEGRATION),
        `${file} ${jobName} holds \`id-token: write\` and reaches a runtime harness`,
      );
      for (const name of names) {
        assert.ok(
          !commands.includes(name),
          `${file} ${jobName} holds \`id-token: write\` and reaches ${name}`,
        );
      }
    }
  }
});

test("the npm-install gate reads TypeScript harness entry points", () => {
  assert.ok(
    harnessFiles().includes(`${INTEGRATION}/web/vitest.config.ts`),
    "the Vitest config can spawn an install but is outside the scanner's file walk",
  );
  const { offenders } = npmInstalls([[
    `${INTEGRATION}/web/vitest.config.ts`,
    'spawnSync("npm", ["ci"]);',
  ]]);
  assert.equal(offenders.length, 1, "a TypeScript harness install has no --ignore-scripts");
});

test("the npm-install gate reads executable sources in hidden directories", () => {
  const root = mkdtempSync(join(tmpdir(), "opportune-harness-hidden-"));
  try {
    const source = 'spawnSync("npm", ["ci"]);\n';
    mkdirSync(join(root, ".scripts"));
    writeFileSync(join(root, ".scripts", "install.mjs"), source);
    const file = `${INTEGRATION}/web/.scripts/install.mjs`;
    assert.ok(
      harnessFiles(root, `${INTEGRATION}/web`).includes(file),
      "a hidden source directory is not necessarily generated output",
    );
    const { offenders } = npmInstalls([[file, source]]);
    assert.equal(offenders.length, 1, "the hidden source's install has no --ignore-scripts");
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test("the npm-install gate reads JSX and TSX harness entry points", () => {
  const root = mkdtempSync(join(tmpdir(), "opportune-harness-jsx-"));
  try {
    for (const extension of ["jsx", "tsx"]) {
      const source = [
        "const view = <div />;",
        "spawnSync(",
        '  "npm",',
        '  ["ci"],',
        ");",
        "",
      ].join("\n");
      writeFileSync(join(root, `fixture.${extension}`), source);
      const file = `${INTEGRATION}/web/fixture.${extension}`;
      assert.ok(
        harnessFiles(root, `${INTEGRATION}/web`).includes(file),
        `the harness walk excludes .${extension} source`,
      );
      const { offenders } = npmInstalls([[file, source]]);
      assert.equal(offenders.length, 1, `a multiline .${extension} install has no --ignore-scripts`);
    }
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test("a multiline JavaScript spawn cannot hide an npm install", () => {
  for (const comment of [undefined, "  // ))"]) {
    const source = [
      'const result = spawnSync(',
      '  "npm",',
      '  [',
      ...(comment === undefined ? [] : [comment]),
      '    "ci",',
      '  ],',
      ');',
      "",
    ].join("\n");
    const { installs, offenders } = npmInstalls([[`${INTEGRATION}/web/spawn.ts`, source]]);
    assert.equal(installs.length, 1, "the formatter's line breaks hid the npm invocation");
    assert.equal(offenders.length, 1, "the multiline spawn has no --ignore-scripts");
  }
});

test("JavaScript data and non-install process arguments stay outside the install gate", () => {
  for (const source of [
    'expect(commands).toEqual([\n  "npm",\n  "ci",\n]);',
    'spawnSync(process.execPath, ["install.js"]);',
    'spawnSync(process.execPath, ["-e", "for (const k in o) {}"]);',
    'const match = /npm/.exec("npm ci");',
    'const command = "npm ci";',
  ]) {
    assert.equal(
      npmInstalls([[`${INTEGRATION}/web/fixture.ts`, source]]).installs.length,
      0,
      `ordinary source was read as an npm install: ${source}`,
    );
  }
});

test("multiline process diagnostics preserve each call's original source", () => {
  const source = [
    'spawnSync("npm", ["ci"]); const result = spawnSync(',
    '  "npm",',
    '  ["install"],',
    ');',
    "",
  ].join("\n");
  const { offenders } = npmInstalls([[`${INTEGRATION}/web/spawn.ts`, source]]);
  assert.equal(offenders.length, 2, "each process call is its own install site");
  assert.ok(offenders.every((offender) => !offender.includes("spaspawnSync")), offenders.join("\n"));
  assert.ok(
    offenders.some((offender) => offender.includes('spawnSync(\n  "npm",\n  ["install"],\n)')),
    offenders.join("\n"),
  );
});

test("a JavaScript exec string is read as one shell command", () => {
  for (const [command, expected] of [
    ["npm ci", 1],
    ["npm ci --ignore-scripts", 0],
  ]) {
    for (const source of [
      `execSync(${JSON.stringify(command)});`,
      `import * as childProcess from "node:child_process"; childProcess.execSync(${JSON.stringify(command)});`,
      `const childProcess = require("child_process"); childProcess.execSync(${JSON.stringify(command)});`,
    ]) {
      const { installs, offenders } = npmInstalls([[`${INTEGRATION}/web/exec.ts`, source]]);
      assert.equal(installs.length, 1, `${command} was not read as an install: ${source}`);
      assert.equal(offenders.length, expected, source);
    }
  }
});

test("literal template strings cannot hide a JavaScript npm install", () => {
  for (const [args, expected] of [
    ["[`ci`]", 1],
    ["[`ci`, `--ignore-scripts`]", 0],
  ]) {
    const source = `spawnSync(\`npm\`, ${args});`;
    const { installs, offenders } = npmInstalls([[`${INTEGRATION}/web/spawn.ts`, source]]);
    assert.equal(installs.length, 1, "the literal template command was not read as npm");
    assert.equal(offenders.length, expected, source);
  }
});

test("JavaScript argument separators do not corrupt an install's options", () => {
  for (const source of [
    'spawnSync("npm", ["ci", "--ignore-scripts"], { stdio: "inherit" });',
    'execSync("npm ci --ignore-scripts", { cwd: "." });',
  ]) {
    const { installs, offenders } = npmInstalls([[`${INTEGRATION}/web/spawn.ts`, source]]);
    assert.equal(installs.length, 1, source);
    assert.equal(offenders.length, 0, source);
  }
});

test("shell-mode spawn and execFile command strings stay in the install gate", () => {
  for (const [source, expected] of [
    ['spawn("npm ci", { shell: true });', 1],
    ['spawnSync("npm ci", [], { shell: true });', 1],
    ['execFile("npm ci", { shell: true });', 1],
    ['spawn("npm", ["ci"], { shell: true });', 1],
    ['spawn("npm", ["install", pkg], { shell: true });', 1],
    ['spawn("npm", ["ci", "--prefix", dir], { shell: true });', 1],
    ['spawn("npm ci --ignore-scripts", [], { shell: true });', 0],
    ['spawn("npm", ["ci", "--ignore-scripts", dir], { shell: true });', 0],
  ]) {
    const { installs, offenders } = npmInstalls([[`${INTEGRATION}/web/spawn.ts`, source]]);
    assert.equal(installs.length, 1, `the shell command was not read as an install: ${source}`);
    assert.equal(offenders.length, expected, source);
  }
});

test("aliased and scoped child_process bindings stay in the install gate", () => {
  for (const source of [
    'import { spawn as run } from "node:child_process"; run("npm", ["ci"]);',
    'const { spawn: run } = require("node:child_process"); run("npm", ["ci"]);',
    'function f() { const cp = require("node:child_process"); return cp.spawn("npm", ["ci"]); }',
  ]) {
    const { installs, offenders } = npmInstalls([[`${INTEGRATION}/web/spawn.ts`, source]]);
    assert.equal(installs.length, 1, `the aliased process call was not read: ${source}`);
    assert.equal(offenders.length, 1, source);
  }
});

test("a computed JavaScript process command cannot hide an npm install", () => {
  for (const source of [
    'spawnSync(getNpm(), ["ci"]);',
    'execSync(`npm ci ${FLAG}`);',
  ]) {
    const { installs, offenders } = npmInstalls([[`${INTEGRATION}/web/spawn.ts`, source]]);
    assert.equal(installs.length, 1, `the computed command was not read as an install: ${source}`);
    assert.equal(offenders.length, 1, `the computed command has no certain --ignore-scripts: ${source}`);
  }
});

test("computed JavaScript non-install commands stay outside the install gate", () => {
  for (const source of [
    'spawnSync(getCommand(), ["run", "test"]);',
    'execSync(`npm run ${SCRIPT}`);',
  ]) {
    assert.equal(npmInstalls([[`${INTEGRATION}/web/spawn.ts`, source]]).installs.length, 0, source);
  }
});

test("a JavaScript variable in process command position cannot hide an npm install", () => {
  const source = 'const NPM = "npm"; spawnSync(NPM, ["ci"]);';
  const { installs, offenders } = npmInstalls([[`${INTEGRATION}/web/spawn.ts`, source]]);
  assert.equal(installs.length, 1, "the variable process command was not read as an install");
  assert.equal(offenders.length, 1, "the variable process command has no --ignore-scripts");
});

test("the npm-install gate reads package scripts, not manifest prose", () => {
  const manifest = JSON.stringify({
    description: "npm install in prose is not a command",
    scripts: { postinstall: "npm install left-pad" },
  });
  const { installs, offenders } = npmInstalls([["harness/package.json", manifest]]);
  assert.equal(installs.length, 1, "the script's install is a site");
  assert.equal(offenders.length, 1, "the script's install has no --ignore-scripts");
});

test("a backslash continuation is read as the one command the shell receives", () => {
  const content = [
    "jobs:",
    "  build:",
    "    steps:",
    "      - run: |",
    "          npm ci \\",
    "            --ignore-scripts",
    "          npm install left-pad",
    "",
  ].join("\n");
  const { installs, offenders } = npmInstalls([["ci.yml", content]]);
  assert.equal(installs.length, 1);
  assert.equal(offenders.length, 1, "the second install has no --ignore-scripts");
  assert.match(offenders[0], /npm install left-pad/);
});

test("a YAML folded scalar is read as the command the shell receives", () => {
  const content = [
    "jobs:",
    "  build:",
    "    steps:",
    "      - run: >",
    "          npm",
    "          ci",
    "",
  ].join("\n");
  const { installs, offenders } = npmInstalls([["ci.yaml", content]]);
  assert.equal(installs.length, 1);
  assert.equal(offenders.length, 1, "npm ci without --ignore-scripts is unsafe");
});

test("a flag on one command does not make the next install safe", () => {
  const content = [
    "jobs:",
    "  build:",
    "    steps:",
    "      - run: npm ci --ignore-scripts && npm install left-pad",
    "",
  ].join("\n");
  const { offenders } = npmInstalls([["ci.yml", content]]);
  assert.equal(offenders.length, 1, "the install to the right of && has no flag");
});

test("an install flag cannot cover another shell-executed install", () => {
  const commands = [
    "npm ci --ignore-scripts |& npm install left-pad",
    'npm ci --ignore-scripts "$(npm install left-pad)"',
  ];
  const counts = commands.map((command) => {
    const content = [
      "jobs:",
      "  build:",
      "    steps:",
      `      - run: ${JSON.stringify(command)}`,
      "",
    ].join("\n");
    return npmInstalls([["ci.yml", content]]).offenders.length;
  });
  assert.deepEqual(counts, [1, 1], "a second shell-executed install inherited the first install's flag");
});

test("a dynamic install cannot hide beside a flagged literal install", () => {
  const content = [
    "jobs:",
    "  build:",
    "    steps:",
    "      - run: 'NPM=npm; npm ci --ignore-scripts; $NPM install left-pad'",
    "",
  ].join("\n");
  const { offenders } = npmInstalls([["ci.yml", content]]);
  assert.equal(offenders.length, 1, "the variable invocation inherits neither npm's name nor the literal install's flag");
});

test("a shell wrapper cannot hide a dynamic npm install", () => {
  for (const command of [
    "command $NPM install left-pad",
    "env $NPM ci",
  ]) {
    const content = [
      "jobs:",
      "  build:",
      "    steps:",
      `      - run: ${JSON.stringify(command)}`,
      "",
    ].join("\n");
    const { installs, offenders } = npmInstalls([["ci.yml", content]]);
    assert.equal(installs.length, 1, `${command} was not read as an install`);
    assert.equal(offenders.length, 1, `${command} ran without --ignore-scripts`);
  }
});

test("a dynamic word in argument position is data, not an install", () => {
  // The interpellation cases a gate-by-substring would fail: the dynamic word names a
  // value, and the installing-looking word is a path or argument beside it.
  for (const command of [
    "grep -q $PATTERN ci.yml",
    "tar -xzf $ARCHIVE -C /tmp/install",
    "git diff --name-only $BASE ci.yml",
  ]) {
    const content = [
      "jobs:",
      "  build:",
      "    steps:",
      `      - run: ${JSON.stringify(command)}`,
      "",
    ].join("\n");
    const { installs, offenders } = npmInstalls([["ci.yml", content]]);
    assert.equal(installs.length, 0, `${command} was read as an npm install`);
  }
});

test("a shell comment cannot hide installs on following lines", () => {
  const content = [
    "jobs:",
    "  build:",
    "    steps:",
    "      - run: |",
    "          npm ci --ignore-scripts # install the locked toolchain",
    "          npm install left-pad",
    "",
  ].join("\n");
  const { offenders } = npmInstalls([["ci.yml", content]]);
  assert.equal(offenders.length, 1, "the install after the comment has no --ignore-scripts");
  assert.match(offenders[0], /npm install left-pad/);
});

test("an npm subcommand supplied by shell expansion fails closed", () => {
  for (const [flag, expected] of [["", 1], [" --ignore-scripts", 0]]) {
    const content = [
      "jobs:",
      "  build:",
      "    steps:",
      `      - run: npm \${SUBCOMMAND:-ci}${flag}`,
      "",
    ].join("\n");
    const { installs, offenders } = npmInstalls([["ci.yml", content]]);
    assert.equal(installs.length, 1, "the expanded subcommand was not read as a possible install");
    assert.equal(offenders.length, expected, `npm expansion${flag}`);
  }
});

test("a dynamic npm option value or run argument is not an install subcommand", () => {
  for (const command of ["npm --prefix $DIR run test", "npm run $SCRIPT"]) {
    const content = [
      "jobs:",
      "  build:",
      "    steps:",
      `      - run: ${command}`,
      "",
    ].join("\n");
    assert.equal(npmInstalls([["ci.yml", content]]).installs.length, 0, command);
  }
});

test("a variable installer named outside the script is still an unsafe site", () => {
  // `$NPM` may be set by the workflow's `env` or an earlier exported step, so the script
  // itself names no literal `npm` and no literal installing subcommand. Reading only
  // scripts that spell `npm` would let that invocation through; the scanner parses every
  // script and judges the variable command's own words.
  const content = [
    "jobs:",
    "  build:",
    "    steps:",
    "      - run: $NPM ci",
    "",
  ].join("\n");
  const { installs, offenders } = npmInstalls([["ci.yml", content]]);
  assert.equal(installs.length, 1, "the variable command is an install site");
  assert.equal(offenders.length, 1, "a variable install with no --ignore-scripts is unsafe");
});

test("a word split across a backslash continuation cannot evade the gate", () => {
  // The shell deletes the backslash and the newline, so `np\`/`m ci` is `npm ci`. Joining
  // the continuation with a space instead would leave two words that name no command and
  // the install would be invisible.
  const content = [
    "jobs:",
    "  build:",
    "    steps:",
    "      - run: |",
    "          np\\",
    "          m ci",
    "",
  ].join("\n");
  const { installs, offenders } = npmInstalls([["ci.yml", content]]);
  assert.equal(installs.length, 1, "the joined word names npm");
  assert.equal(offenders.length, 1, "the joined install has no --ignore-scripts");
});

test("npm's last ignore-scripts spelling decides whether the install is safe", () => {
  // npm reads the last boolean option, so `--no-ignore-scripts` turns the flag off and
  // `--ignore-scripts=false` is not `--ignore-scripts`; only an explicit final
  // `--ignore-scripts` skips dependency lifecycle scripts.
  const commands = [
    ["npm ci --no-ignore-scripts", 1],
    ["npm ci --ignore-scripts=false", 1],
    ["npm ci --no-ignore-scripts --ignore-scripts", 0],
  ];
  const counts = commands.map(([command]) => {
    const content = [
      "jobs:",
      "  build:",
      "    steps:",
      `      - run: ${JSON.stringify(command)}`,
      "",
    ].join("\n");
    return npmInstalls([["ci.yml", content]]).offenders.length;
  });
  assert.deepEqual(counts, commands.map(([, offenders]) => offenders));
});


test("every npm install alias requires --ignore-scripts", () => {
  // npm's own `install --help` lists these aliases. They reach the same installer and
  // lifecycle scripts as the canonical spelling, so none may be a scanner blind spot.
  for (const alias of [
    "add",
    "i",
    "in",
    "ins",
    "inst",
    "insta",
    "instal",
    "isnt",
    "isnta",
    "isntal",
    "isntall",
    "clean-install",
    "ic",
    "install-clean",
    "isntall-clean",
  ]) {
    for (const [flag, expected] of [["", 1], [" --ignore-scripts", 0]]) {
      const content = [
        "jobs:",
        "  build:",
        "    steps:",
        `      - run: npm ${alias} left-pad${flag}`,
        "",
      ].join("\n");
      const { installs, offenders } = npmInstalls([["ci.yml", content]]);
      assert.equal(installs.length, 1, `npm ${alias} was not read as an install`);
      assert.equal(offenders.length, expected, `npm ${alias}${flag}`);
    }
  }

  const control = [
    "jobs:",
    "  build:",
    "    steps:",
    "      - run: 'for package in a b; do echo \"npm package ${package}\"; done'",
    "",
  ].join("\n");
  assert.equal(
    npmInstalls([["ci.yml", control]]).installs.length,
    0,
    "shell's `in` keyword and later npm prose are not an `npm in` invocation",
  );
});

test("quoted text cannot hide an install, and .yaml files are read too", () => {
  const content = [
    "jobs:",
    "  build:",
    "    steps:",
    "      - run: 'echo \"not # a comment\"; npm ci'",
    "",
  ].join("\n");
  const { installs, offenders } = npmInstalls([["ci.yaml", content]]);
  assert.equal(installs.length, 1);
  assert.equal(offenders.length, 1);
});

test("documented source installs and manual publish skip lifecycle scripts", () => {
  // The TypeScript snippet gate does not execute shell fences or the maintainer
  // runbook, so removing one of these flags would otherwise leave every gate green.
  const building = read("docs/wasm.md").split("### Building from source")[1]?.split("\n### ")[0];
  assert.ok(building, "docs/wasm.md keeps its source-build section");
  const { installs, offenders } = npmInstalls([["docs/wasm.md", building]]);
  assert.equal(installs.length, 2, "both the tooling install and local tarball install are read");
  assert.deepEqual(offenders, []);

  const publish = /npm publish crates\/opportune-wasm\/js[^\n`]*/.exec(read("CONTRIBUTING.md"))?.[0];
  assert.ok(publish, "CONTRIBUTING.md keeps its manual npm publish command");
  assert.match(publish, /(?:^|\s)--ignore-scripts(?:\s|$)/);
});

test("building the package removes stale artifacts a previous build left in dist/", () => {
  // `tsc` only writes and `bundle-module` only copies, so a file left in `dist/`
  // by an older build — a renamed module's glue, a removed stub — survives into
  // the packed tarball with nothing failing. The build cleans the tree first, so
  // the sentinel a build cannot regenerate is gone when it returns.
  const root = mkdtempSync(join(tmpdir(), "opportune-clean-build-"));
  try {
    cpSync(join(JS_PACKAGE, "src"), join(root, "src"), { recursive: true });
    cpSync(join(JS_PACKAGE, "bin"), join(root, "bin"), { recursive: true });
    cpSync(join(JS_PACKAGE, "pkg"), join(root, "pkg"), { recursive: true });
    cpSync(join(JS_PACKAGE, "pkg-node"), join(root, "pkg-node"), { recursive: true });
    for (const file of ["package.json", "tsconfig.json", "tsconfig.build.json"]) {
      copyFileSync(join(JS_PACKAGE, file), join(root, file));
    }
    symlinkSync(join(JS_PACKAGE, "node_modules"), join(root, "node_modules"), "dir");
    const sentinel = join(root, "dist", "stale-glue.cjs");
    mkdirSync(dirname(sentinel), { recursive: true });
    writeFileSync(sentinel, "module.exports = 'stale';\n");

    const run = spawnSync("npm", ["run", "build"], { cwd: root, encoding: "utf8" });
    assert.equal(run.status, 0, `the build failed: ${run.stderr}`);
    assert.equal(
      existsSync(sentinel),
      false,
      "a file the build does not generate survived into dist/",
    );
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test("the bundle refuses to publish different web and Node Wasm modules", () => {
  // The two targets are built independently, so equal bytes are an assumption the
  // bundle must enforce rather than a property the packlist can infer. Damage only a
  // sandbox's Node copy: the real build outputs and the checkout stay untouched.
  const root = mkdtempSync(join(tmpdir(), "opportune-target-drift-"));
  try {
    cpSync(join(JS_PACKAGE, "bin"), join(root, "bin"), { recursive: true });
    cpSync(join(JS_PACKAGE, "pkg"), join(root, "pkg"), { recursive: true });
    cpSync(join(JS_PACKAGE, "pkg-node"), join(root, "pkg-node"), { recursive: true });
    const source = join(root, "pkg", "opportune_wasm_bg.wasm");
    const drifted = join(root, "pkg-node", "opportune_wasm_bg.wasm");
    const original = readFileSync(drifted);
    // An empty custom section is a valid Wasm module with different bytes, rather
    // than a corrupt file that another validation layer would reject first.
    writeFileSync(drifted, Buffer.concat([original, Buffer.from([0, 1, 0])]));

    const failed = spawnSync(process.execPath, ["bin/bundle-module.mjs"], {
      cwd: root,
      encoding: "utf8",
    });
    assert.notEqual(failed.status, 0, `different target modules were accepted:\n${failed.stdout}`);
    assert.match(
      failed.stderr,
      /different modules for the `web` and `nodejs` targets/,
      "the failure names the target drift and its repair",
    );

    writeFileSync(drifted, original);
    const repaired = spawnSync(process.execPath, ["bin/bundle-module.mjs"], {
      cwd: root,
      encoding: "utf8",
    });
    assert.equal(repaired.status, 0, `the restored targets did not bundle:\n${repaired.stderr}`);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test("the lockfile's root entry declares the floors the manifest does", () => {
  // `engines` is the runtime promise a consumer's package manager reads, and
  // `package-lock.json`'s root entry is the second copy of it that `npm ci` resolves the
  // tree from. Nothing compares the two: a floor raised in the manifest and not in the
  // lockfile leaves one committed file stating a different supported runtime, and no
  // install, test or gate notices.
  const manifest = JSON.parse(read(MANIFEST));
  const lockfile = JSON.parse(read(LOCKFILE));
  assert.deepEqual(
    lockfile.packages?.[""]?.engines,
    manifest.engines,
    "package-lock.json's root `engines` disagrees with package.json's: a floor moves in both",
  );
});

test("CI executes the runtime floors the npm package declares", () => {
  // `engines` is a promise, and the packed-package suite is what proves one: it packs the
  // tarball, installs it into a temporary consumer project and checks it with the standard
  // packaging tools. A lane on the declared floor deleted in a review leaves every gate
  // green — the suite still runs on the newest LTS — so the lane is pinned to the
  // manifest's own numbers rather than to copies of them.
  const engines = JSON.parse(read(MANIFEST)).engines;
  const floor = (range, runtime) => {
    const lowest = /^>=(\d+(?:\.\d+)*)$/.exec(range ?? "")?.[1];
    assert.ok(
      lowest !== undefined,
      `engines.${runtime} (${String(range)}) is not a floor this test can read`,
    );
    return lowest;
  };
  const lane = workflowFiles()
    .flatMap((file) => [...jobsIn(read(file))].map(([job, body]) => [`${file} ${job}`, body]))
    .find(([, body]) =>
      (body.steps ?? []).some((step) =>
        (step.run ?? "").includes("node --test crates/opportune-wasm/js/tests/packed-package.test.mjs"),
      ),
    );
  assert.ok(
    lane !== undefined,
    "no CI job runs the packed-package suite, so nothing proves what the package installs onto",
  );
  const [where, body] = lane;
  const steps = body.steps ?? [];
  assert.ok(
    steps.some((step) => step.with?.["node-version"] === floor(engines.node, "node")),
    `${where}: the packed-package suite runs on no lane naming \`engines.node\` (${engines.node}), ` +
      "so the floor a consumer reads is not one CI executes",
  );
  assert.ok(
    steps.some((step) => step.with?.["bun-version"] === floor(engines.bun, "bun")),
    `${where}: the packed-package job installs no Bun naming \`engines.bun\` (${engines.bun})`,
  );
  assert.ok(
    steps.some((step) => /bun test crates\/opportune-wasm\/js\/tests\//.test(step.run ?? "")),
    `${where}: the job runs the suite on Node and not under Bun, so the second declared runtime is unexecuted`,
  );
});

test("nothing in the npm package's install path needs a lifecycle script", () => {
  // `--ignore-scripts` is only free while this holds. npm records a dependency's
  // preinstall/install/postinstall in the lockfile it resolves from, and the package's own
  // hooks run at install, pack and publish time in the same job — so a dependency bump or
  // a new `prepare` that needs one would start silently not running instead of failing.
  const lockfile = JSON.parse(read(LOCKFILE));
  const needsScripts = Object.entries(lockfile.packages ?? {})
    .filter(([, entry]) => entry?.hasInstallScript === true)
    .map(([path]) => path);
  assert.deepEqual(
    needsScripts,
    [],
    "these locked dependencies ship an install script, so --ignore-scripts would skip it",
  );

  const OWN_HOOKS = [
    "preinstall",
    "install",
    "postinstall",
    "prepublish",
    "preprepare",
    "prepare",
    "postprepare",
    "prepack",
    "postpack",
    "prepublishOnly",
  ];
  const manifest = JSON.parse(read(MANIFEST));
  const declared = OWN_HOOKS.filter((hook) => hook in (manifest.scripts ?? {}));
  assert.deepEqual(
    declared,
    [],
    "the package declares a lifecycle hook that npm publish --ignore-scripts would skip " +
      "in the OIDC job; build the artefact in an earlier step instead",
  );
});

test("every job that compiles the extension exports the pinned toolchain", () => {
  // uv builds the extension from an sdist unpacked in its own cache, outside the checkout,
  // where `rust-toolchain.toml` does not reach: without the pin exported, rustup falls
  // back to whatever compiler the runner image ships, and the job stays green while
  // building with a toolchain nobody declared. The export has to reach the *following*
  // steps, so it goes through GITHUB_ENV; a `run: export` inside one step reaches nothing.
  const problems = [];
  for (const file of workflowFiles()) {
    for (const [job, body] of jobsIn(read(file))) {
      const steps = body.steps ?? [];
      const run = (step) => step.run ?? "";
      const firstUv = steps.findIndex(
        (step) => /\buv (build|sync)\b/.test(run(step)) && !/--no-install-project/.test(run(step)),
      );
      if (firstUv === -1) continue;
      const pin = steps.findIndex((step) => /RUSTUP_TOOLCHAIN=/.test(run(step)));
      if (pin === -1) {
        problems.push(`${file} ${job}: compiles the extension with no RUSTUP_TOOLCHAIN export`);
        continue;
      }
      if (pin > firstUv) {
        problems.push(`${file} ${job}: exports RUSTUP_TOOLCHAIN after the first uv command`);
      }
      const between = steps.slice(pin, firstUv).map(run).join("\n");
      if (!between.includes("GITHUB_ENV")) {
        problems.push(`${file} ${job}: exports RUSTUP_TOOLCHAIN outside GITHUB_ENV, so later steps do not see it`);
      }
      // The export is fail-closed only while an unparseable pin reddens the job: an empty
      // RUSTUP_TOOLCHAIN is no override at all, so rustup falls back to the runner image's
      // default compiler and the job goes green having compiled with a toolchain nobody
      // declared.
      const guard = steps.findIndex((step) => /\[\s*-z\s*"\$\{?channel\}?"\s*\]/.test(run(step)));
      if (guard === -1 || guard > pin) {
        problems.push(`${file} ${job}: exports the pin without refusing an empty channel first`);
      } else if (!/exit 1/.test(run(steps[guard]))) {
        // The guard and the export share a `run:` block, so the refusal is a line in that
        // block rather than a later step.
        problems.push(`${file} ${job}: the empty-channel guard does not fail the job`);
      }
    }
  }
  assert.deepEqual(problems, [], "these CI jobs compile the extension without the pinned toolchain");
});

test("lint and the release tag both run the one check-version target", () => {
  // The release tag's job delegates to the shared target so that the manifest and
  // toolchain checks are enforced at the tag too; a job that grew its own copy again, or
  // a lint target that stopped depending on the shared one, would leave half the gate
  // unrun with everything green.
  const lint = /^lint:([^\n]*)$/m.exec(read("Makefile"));
  assert.ok(lint, "the Makefile declares a lint target");
  assert.match(lint[1], /\bcheck-version\b/, "make lint does not run check-version");

  const tag = jobsIn(read(".github/workflows/publish.yml")).get("version-check");
  assert.ok(tag, "publish.yml declares a version-check job");
  const commands = (tag.steps ?? []).map((step) => step.run ?? "").join("\n");
  assert.ok(
    (tag.steps ?? []).some((step) => /^\s*make check-version$/m.test(step.run ?? "")),
    "publish.yml's version-check does not run the shared check-version target",
  );
  assert.match(
    commands,
    /test "v\$\{version\}" = "\$\{GITHUB_REF_NAME\}"/,
    "publish.yml's version-check no longer compares the tag with the workspace version",
  );
  assert.match(
    commands,
    /if \[ "\$\{GITHUB_REF_TYPE\}" != "tag" \]/,
    "publish.yml's version-check no longer refuses a non-tag dispatch",
  );
});

test("release-please reconciles the Python lockfile with the bumped version", () => {
  // `release-please` bumps the version sources but touches no lockfile. The Python
  // project's `uv.lock` is the other generated lockfile in the bumped tree, and every
  // `uv run --locked` on the release PR reads it. The step that regenerates it is
  // invisible to the compiler, so deleting it would leave a green run and a release
  // branch whose Python lane fails.
  const releasePlease = jobsIn(read(".github/workflows/release-please.yml")).get("release-please");
  assert.ok(releasePlease, "release-please.yml declares the release-please job");
  const steps = releasePlease.steps ?? [];
  const commands = steps.map((step) => step.run ?? "").join("\n");
  const uses = steps.map((step) => step.uses ?? "").join("\n");

  const pinned = /astral-sh\/setup-uv@([0-9a-f]{40})/.exec(read(".github/workflows/ci.yml"))?.[1];
  assert.ok(pinned, "ci.yml pins astral-sh/setup-uv by commit");
  assert.match(
    uses,
    new RegExp(`astral-sh/setup-uv@${pinned}\\b`),
    "release-please.yml does not install the uv revision ci.yml pins",
  );
  assert.match(
    commands,
    /uv lock --project crates\/opportune-py/,
    "release-please.yml does not regenerate crates/opportune-py/uv.lock",
  );
  assert.match(
    commands,
    /git add Cargo\.lock crates\/opportune-py\/uv\.lock/,
    "release-please.yml does not commit the Python lockfile it regenerated",
  );
});

test("a stray Bun lockfile cannot be committed beside the npm one", () => {
  // The npm package is managed through npm and commits its `package-lock.json`, while the
  // manifest still declares a Bun floor because the suite runs under both runners. A stray
  // `bun install` would otherwise add a second lockfile for a different resolver to the
  // same package. The control matters as much as the ignore: a pattern broad enough to
  // swallow the committed npm lockfile would pass the first half and break `npm ci`.
  const ignored = (path) => spawnSync("git", ["check-ignore", "-q", path], { cwd: ROOT }).status === 0;
  for (const lockfile of ["bun.lock", "bun.lockb"]) {
    assert.ok(ignored(lockfile), `${lockfile} is not ignored, so a stray Bun install would be committable`);
    assert.ok(
      ignored(`crates/opportune-wasm/js/${lockfile}`),
      `${lockfile} is not ignored inside the npm package, where a stray Bun install would run`,
    );
  }
  assert.ok(
    !ignored("crates/opportune-wasm/js/package-lock.json"),
    "the committed npm lockfile is ignored, so npm ci would resolve a different tree",
  );
});

test("Pages deployments are serialised without cancelling one in progress", () => {
  // Pages is one production environment, so two overlapping deploys race on it and a
  // cancelled in-progress deploy leaves that environment's state undefined — hence the
  // queue. The group has to belong to the deploy job rather than to the workflow, because
  // `build` runs for every pull request as well.
  const text = read(".github/workflows/docs.yml");
  const header = text.slice(0, text.indexOf("\njobs:"));
  const workflowConcurrency = parseYaml(header).concurrency;
  if (workflowConcurrency !== undefined) {
    const group = typeof workflowConcurrency === "string" ? workflowConcurrency : workflowConcurrency.group;
    assert.match(
      String(group ?? ""),
      /github\./,
      "docs.yml's workflow-level group is one static name, so a pull-request build joins " +
        "the deploy group and can displace a queued deploy",
    );
  }

  const deploy = jobsIn(text).get("deploy");
  assert.ok(deploy, "docs.yml declares a deploy job");
  assert.equal(typeof deploy.concurrency, "object", "docs.yml's deploy job declares no concurrency group");
  assert.match(String(deploy.concurrency.group ?? ""), /\S/, "docs.yml's deploy job has no group name");
  assert.equal(
    deploy.concurrency["cancel-in-progress"],
    false,
    "docs.yml cancels an in-progress deploy, leaving the Pages environment's state undefined",
  );
});

test("no published crate ships a test target whose inputs are outside the crate", () => {
  // A `.crate` carries one crate directory. A test that `include_str!`s a repository
  // page, or `include!`s another crate's snippet, therefore resolves that path to
  // nothing there and the *packaged* source's suite stops compiling: `cargo test`
  // against the extracted tarball fails on four `couldn't read tests/../../../docs/…`
  // errors. `cargo package` verifies the library only, so the push that breaks it is
  // green; the `exclude` entry is the whole of the guarantee, and no run exercises it.
  //
  // A crate is publishable unless its manifest says otherwise, and a target escapes when
  // one of its own `include!` paths resolves outside the crate directory — the target's
  // direct inputs, not a walk of the module graph it pulls in, which is where both
  // documentation tests keep their pages and the other crate's snippet corpus.
  // `publish = false` crates are skipped because they are never packaged; an `include`
  // list would name what ships instead of what does not, so the pin reports one rather
  // than guessing which of the two a target falls under.
  const problems = [];
  for (const crate of readdirSync(join(ROOT, "crates")).sort()) {
    const root = join(ROOT, "crates", crate);
    if (!existsSync(join(root, "Cargo.toml"))) {
      continue;
    }
    const manifest = read(`crates/${crate}/Cargo.toml`);
    if (/^publish\s*=\s*false/m.test(manifest)) {
      continue;
    }
    if (/^include\s*=/m.test(manifest)) {
      problems.push(`crates/${crate}/Cargo.toml declares an \`include\` list this pin cannot read`);
      continue;
    }
    const excluded = (/^exclude\s*=\s*\[([\s\S]*?)\]/m.exec(manifest)?.[1] ?? "")
      .split(",")
      .map((entry) => entry.trim().replaceAll('"', ""))
      .filter((entry) => entry !== "");
    for (const directory of ["tests", "benches"]) {
      const held = join(root, directory);
      if (!existsSync(held)) {
        continue;
      }
      for (const name of readdirSync(held).filter((entry) => entry.endsWith(".rs"))) {
        const target = `${directory}/${name}`;
        const included = readFileSync(join(held, name), "utf8");
        const escapesRoot = (resolved) =>
          resolved !== root && !resolved.startsWith(`${root}${sep}`);
        const compileTimeEscape = [...included.matchAll(/include(?:_str|_bytes)?!\(\s*"([^"]+)"/g)]
          .map(([, path]) => resolve(held, path))
          .some(escapesRoot);
        const manifestRelativePaths = [
          ...included.matchAll(
            /concat!\(\s*env!\(\s*"CARGO_MANIFEST_DIR"\s*\)\s*,\s*"([^"]+)"/g,
          ),
          ...included.matchAll(
            /env!\(\s*"CARGO_MANIFEST_DIR"\s*\)\s*\)\s*\.join\(\s*"([^"]+)"/g,
          ),
          ...included.matchAll(
            /format!\(\s*"\{\}([^"]+)"\s*,\s*env!\(\s*"CARGO_MANIFEST_DIR"\s*\)/g,
          ),
        ].map(([, suffix]) => resolve(root, suffix.replace(/^\/+/, "")));
        const runtimeEscape = manifestRelativePaths.some(escapesRoot);
        const outside = compileTimeEscape || runtimeEscape;
        const shipped = !excluded.some(
          (entry) => entry === target || target.startsWith(`${entry}/`),
        );
        if (outside && shipped) {
          problems.push(
            `crates/${crate} ships ${target} in its \`.crate\`, but the target reads files ` +
              `the package does not carry, so the published crate's own suite cannot pass`,
          );
        }
      }
    }
  }
  assert.deepEqual(problems, [], "a published crate ships a target whose inputs escape it");
});

/**
 * The `available_only` dated-entry ceiling in each binding, and where it is stated.
 *
 * The preset writes one dated window entry per UTC day of every stated span, so the
 * ceiling is what bounds one bad argument to a bounded amount of work. Each binding
 * holds its own copy because each language's preset builds the rule itself, and the
 * three copies are the same number by contract: `MAX_AVAILABLE_ONLY_DAYS` is the
 * public Rust constant `engine_a2.rs` pins to 3660, and the Python and TypeScript
 * presets are ports of that function.
 */
const AVAILABLE_ONLY_CEILING = [
  ["crates/opportune/src/presets.rs", /^pub const MAX_AVAILABLE_ONLY_DAYS: u32 = (?<ceiling>\d+);$/m],
  ["crates/opportune-py/python/opportune/presets.py", /^_MAX_DATED_ENTRIES = (?<ceiling>\d+)$/m],
  ["crates/opportune-wasm/js/src/presets.ts", /^const MAX_DATED_ENTRIES = (?<ceiling>\d+);$/m],
];

test("the available-only ceiling is one number in Rust, Python and TypeScript", () => {
  // A binding that moved its ceiling alone would accept an availability another
  // refuses, so the same request would answer differently depending on which package
  // served it — a cross-language drift no single binding's suite can observe. The Rust
  // constant is the source of truth; `engine_a2.rs` pins its value, and this pin holds
  // the two ports to it.
  const stated = AVAILABLE_ONLY_CEILING.map(([file, pattern]) => {
    const found = pattern.exec(read(file));
    assert.ok(found?.groups?.ceiling !== undefined, `${file} no longer states the ceiling this pin reads`);
    return [file, Number(found.groups.ceiling)];
  });
  const [[rustFile, rust]] = stated;
  for (const [file, ceiling] of stated.slice(1)) {
    assert.equal(
      ceiling,
      rust,
      `${file} accepts ${ceiling} dated entries where ${rustFile} accepts ${rust}: the same ` +
        "stated availability is refused by one package and written by another",
    );
  }
});
