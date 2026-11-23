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
import {
  existsSync,
  mkdirSync,
  mkdtempSync,
  readdirSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve, sep } from "node:path";
import test from "node:test";

/** This suite sits at `crates/opportune-wasm/js/tests`, so the root is four levels up. */
const ROOT = join(import.meta.dirname, "..", "..", "..", "..");

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
function workflowFiles() {
  return readdirSync(join(ROOT, ".github", "workflows"))
    .filter((name) => name.endsWith(".yml"))
    .map((name) => `.github/workflows/${name}`);
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

/** The `jobs:` table of a workflow, each job mapped to the lines of its block. */
function jobsIn(text) {
  const lines = text.split("\n");
  const start = lines.indexOf("jobs:");
  assert.notEqual(start, -1, "the workflow declares its jobs at the top level");
  const jobs = new Map();
  let current = null;
  for (const line of lines.slice(start + 1)) {
    const declared = /^ {2}([A-Za-z0-9_-]+):\s*$/.exec(line);
    if (declared) {
      current = declared[1];
      jobs.set(current, []);
      continue;
    }
    if (current !== null) jobs.get(current).push(line);
  }
  return jobs;
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
  // on any single call site.
  const installs = [];
  const offenders = [];
  for (const file of ["Makefile", ...workflowFiles()]) {
    for (const [index, line] of read(file).split("\n").entries()) {
      const command = line.trim();
      if (command.startsWith("#")) continue;
      if (!/\bnpm\b/.test(command) || !/\b(ci|install|i)\b/.test(command)) continue;
      installs.push(`${file}:${index + 1}`);
      if (!/--ignore-scripts\b/.test(command)) offenders.push(`${file}:${index + 1}: ${command}`);
    }
  }

  assert.notDeepEqual(installs, [], "no npm install was found, so this test proves nothing");
  assert.deepEqual(
    offenders,
    [],
    "these npm installs run dependency lifecycle scripts; in publish-npm that is " +
      "arbitrary code in the job holding id-token: write",
  );
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
  // tarball, installs it into a temporary consumer project, imports every declared
  // subpath, `require`s the CommonJS entry and compiles the declarations for a consumer
  // whose only lib is `ES2024`. A lane on the declared floor deleted in a review leaves
  // every gate green — the suite still runs on the newest LTS — so the lane is pinned to
  // the manifest's own numbers rather than to copies of them: a floor moved without its
  // lane fails here instead of shipping as a promise nothing executes.
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
    .flatMap((file) => [...jobsIn(read(file))].map(([job, lines]) => [`${file} ${job}`, lines.join("\n")]))
    .find(([, body]) =>
      body.includes("node --test crates/opportune-wasm/js/tests/packed-package.test.mjs"),
    );
  assert.ok(
    lane !== undefined,
    "no CI job runs the packed-package suite, so nothing proves what the package installs onto",
  );
  const [where, body] = lane;
  assert.ok(
    body.includes(`node-version: "${floor(engines.node, 'node')}"`),
    `${where}: the packed-package suite runs on no lane naming \`engines.node\` (${engines.node}), ` +
      "so the floor a consumer reads is not one CI executes",
  );
  assert.ok(
    body.includes(`bun-version: "${floor(engines.bun, 'bun')}"`),
    `${where}: the packed-package job installs no Bun naming \`engines.bun\` (${engines.bun})`,
  );
  assert.match(
    body,
    /bun test crates\/opportune-wasm\/js\/tests\//,
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

  const OWN_HOOKS = ["preinstall", "install", "postinstall", "prepare", "prepack", "prepublishOnly"];
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
  // The shapes looked for are the two uv commands that compile the extension today
  // (`uv build`, and an installing `uv sync`); a job that starts compiling it another way
  // would need this rule widened, which is the one way this test could go quietly stale.
  // Comments discuss `uv build` and the pin constantly, so only command lines count.
  const isCommand = (line) => !/^\s*#/.test(line);
  const problems = [];
  for (const file of workflowFiles()) {
    for (const [job, lines] of jobsIn(read(file))) {
      const firstUv = lines.findIndex(
        (line) => isCommand(line) && /\buv (build|sync)\b/.test(line) && !/--no-install-project/.test(line),
      );
      if (firstUv === -1) continue;
      const pin = lines.findIndex((line) => isCommand(line) && /RUSTUP_TOOLCHAIN=/.test(line));
      if (pin === -1) {
        problems.push(`${file} ${job}: compiles the extension with no RUSTUP_TOOLCHAIN export`);
        continue;
      }
      if (pin > firstUv) {
        problems.push(`${file} ${job}: exports RUSTUP_TOOLCHAIN after the first uv command`);
      }
      if (!lines.slice(pin, firstUv).some((line) => line.includes("GITHUB_ENV"))) {
        problems.push(`${file} ${job}: exports RUSTUP_TOOLCHAIN outside GITHUB_ENV, so later steps do not see it`);
      }
      // The export is fail-closed only while an unparseable pin reddens the job: an empty
      // RUSTUP_TOOLCHAIN is no override at all, so rustup falls back to the runner image's
      // default compiler and the job goes green having compiled with a toolchain nobody
      // declared — the drift this step exists to close. `check-version` also rejects a
      // malformed pin, but in the lint job, and these jobs do not wait for lint.
      const guard = lines.findIndex(
        (line) => isCommand(line) && /\[\s*-z\s*"\$\{?channel\}?"\s*\]/.test(line),
      );
      if (guard === -1 || guard > pin) {
        problems.push(`${file} ${job}: exports the pin without refusing an empty channel first`);
      } else if (!lines.slice(guard, pin).some((line) => isCommand(line) && /exit 1/.test(line))) {
        problems.push(`${file} ${job}: the empty-channel guard does not fail the job`);
      }
    }
  }
  assert.deepEqual(problems, [], "these CI jobs compile the extension without the pinned toolchain");
});

test("lint and the release tag both run the one check-version target", () => {
  // The release tag's job used to repeat three of the comparisons inline. It now delegates
  // so that the manifest and toolchain checks added here are enforced at the tag too; a
  // job that grew its own copy again, or a lint target that stopped depending on the
  // shared one, would each leave half the gate unrun with everything green. The two
  // comparisons that stay inline are pinned beside it: without the ref-type guard a
  // branch dispatch compares a branch name against a version, and without the tag test
  // a `v0.2.0` tag on an 0.1.0 tree publishes four artefacts under a version nothing
  // declared.
  const lint = /^lint:([^\n]*)$/m.exec(read("Makefile"));
  assert.ok(lint, "the Makefile declares a lint target");
  assert.match(lint[1], /\bcheck-version\b/, "make lint does not run check-version");

  const tag = jobsIn(read(".github/workflows/publish.yml")).get("version-check");
  assert.ok(tag, "publish.yml declares a version-check job");
  const step = tag.join("\n");
  assert.ok(
    tag.some((line) => /^\s+make check-version$/.test(line)),
    "publish.yml's version-check does not run the shared check-version target",
  );
  assert.match(
    step,
    /test "v\$\{version\}" = "\$\{GITHUB_REF_NAME\}"/,
    "publish.yml's version-check no longer compares the tag with the workspace version",
  );
  assert.match(
    step,
    /if \[ "\$\{GITHUB_REF_TYPE\}" != "tag" \]/,
    "publish.yml's version-check no longer refuses a non-tag dispatch",
  );
});

test("release-please reconciles the Python lockfile with the bumped version", () => {
  // `release-please` bumps the version sources but touches no lockfile. The Cargo
  // workspace is reconciled by `cargo metadata`; the Python project's `uv.lock` is
  // the other generated lockfile in the bumped tree, and every `uv run --locked`
  // on the release PR reads it. The step that regenerates it is invisible to the
  // compiler, so deleting it would leave a green run and a release branch whose
  // Python lane fails. The setup action has to be the same pinned revision ci.yml
  // uses, or the release branch resolves the lock with a different uv than the PR.
  const releasePlease = jobsIn(read(".github/workflows/release-please.yml")).get("release-please");
  assert.ok(releasePlease, "release-please.yml declares the release-please job");
  const block = releasePlease.join("\n");

  const pinned = /astral-sh\/setup-uv@([0-9a-f]{40})/.exec(read(".github/workflows/ci.yml"))?.[1];
  assert.ok(pinned, "ci.yml pins astral-sh/setup-uv by commit");
  assert.match(
    block,
    new RegExp(`astral-sh/setup-uv@${pinned}\\b`),
    "release-please.yml does not install the uv revision ci.yml pins",
  );
  assert.match(
    block,
    /uv lock --project crates\/opportune-py/,
    "release-please.yml does not regenerate crates/opportune-py/uv.lock",
  );
  assert.match(
    block,
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
  // `build` runs for every pull request as well: GitHub keeps at most one *pending* run per
  // group ("any existing pending job or workflow in the same concurrency group will be
  // canceled and the new queued job or workflow will take its place"), so a static
  // workflow-level group lets a pull-request build displace a queued deploy of an
  // already-merged commit. That deploy then never happens, it reports as cancelled rather
  // than failed, and the site stays stale until the next docs change reaches main.
  const text = read(".github/workflows/docs.yml");
  const header = text.slice(0, text.indexOf("\njobs:"));
  if (/^concurrency:$/m.test(header)) {
    const group = /^\s+group:\s*(.+)$/m.exec(header)?.[1] ?? "";
    assert.match(
      group,
      /github\./,
      "docs.yml's workflow-level group is one static name, so a pull-request build joins " +
        "the deploy group and can displace a queued deploy",
    );
  }

  const deploy = jobsIn(text).get("deploy");
  assert.ok(deploy, "docs.yml declares a deploy job");
  const block = deploy.join("\n");
  assert.match(
    block,
    /^\s+concurrency:$/m,
    "docs.yml's deploy job declares no concurrency group, so two deploys can race on the Pages environment",
  );
  assert.match(block, /^\s+group: \S+$/m, "docs.yml's deploy job has no concurrency group name");
  assert.match(
    block,
    /^\s+cancel-in-progress: false$/m,
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
