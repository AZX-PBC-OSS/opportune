# Contributing to opportune

Thank you for your interest in contributing to `opportune`.

## Prerequisites

- **Rust:** 1.100 or newer (`rust-toolchain.toml` pins the exact toolchain)
- **Python:** 3.12 or newer
- **Node.js:** 26 (LTS) for development tooling (`pyright` runs its language server on `node`; the published npm package supports Node 24+)
- **Bun:** 1.4.2 or newer (second supported JS runtime; run `bun test crates/opportune-wasm/js/tests/` for the Bun lane)
- **[uv](https://docs.astral.sh/uv/)** and **[cargo-nextest](https://nexte.st/)**
- **[wasm-pack](https://rustwasm.github.io/wasm-pack/) 0.15.1** for WASM builds and `make check` (CI pins 0.15.1 so `wasm-opt -Oz` matches the size budget)
- **Optional:** `sccache` (used automatically by `Makefile`), `cargo-deny`

## Workspace Layout

| Path | Purpose |
| --- | --- |
| `crates/opportune/` | Pure-Rust core crate: scheduling domain, engine, presets, and `VERSION` |
| `crates/opportune-json/` | Wire types, validating conversions, RFC 6901 JSON-pointer errors, JSON Schema export, and upgrades |
| `crates/opportune-adapters/` | Microsoft Graph and Google Calendar payload conversions into domain blocks and rules |
| `crates/opportune-testkit/` | Independent reference oracle, property generators, golden corpus, and benchmarks (`publish = false`) |
| `crates/opportune-py/` | PyO3 bindings (`_opportune`), typed Python package with generated Pydantic v2 models, presets, adapters, and tests |
| `crates/opportune-wasm/` | WASM binding crate, `js/` npm package with generated TypeScript types, presets, adapters, and Node/Bun tests, and `integration/{workerd,web}` private harnesses that run the packed tarball in real `workerd` and in Chromium/Firefox/WebKit |
| `docs/` | MkDocs Material documentation site published to [azx-pbc-oss.github.io/opportune](https://azx-pbc-oss.github.io/opportune/) |
| `.github/workflows/` | CI, `release-please`, docs deployment, and crates.io/PyPI/npm publishing |

## Common Development Commands

```console
make install           # create the project .venv and build the Python extension
make dev               # rebuild the Python extension after editing crates/*/src/
make lint              # version checks, codegen drift (Python + TS), license copies, doc snippets, rustfmt, clippy, rustdoc, ruff, pyright, tsc
make test              # cargo nextest --all-features, doctests, pytest, checked Python snippets, and Node WASM + TS snippet suites
make test-matrix       # feature-flag combinations and wasm32 target checks
make check             # lint + test + test-matrix + check-docs
make codegen           # regenerate Pydantic models from the combined JSON Schema
make codegen-ts        # regenerate TypeScript types and the packaged schema.json from the same schema
make wasm              # build the release WASM module and npm package dist/
make test-wasm         # build WASM, check the size budget, run the Node suite + TS snippets, and run publint + attw on the packed tarball
make test-wasm-workerd # run the workerd harness suite inside real Cloudflare `workerd` (@cloudflare/vitest-plugin)
make test-wasm-web     # run the web harness suite in Chromium/Firefox/WebKit + a module Web Worker (Vitest Browser Mode + Playwright)
make wasm-size         # measure the release WASM module against the gzipped budget
make deny              # cargo-deny license, advisory, ban, and source policy
make wheel             # build the Python sdist and wheel into dist/
make docs              # build the documentation site with --strict
make docs-serve        # live-reload preview of the documentation site
```

### Slow Verification Targets

- `make test-wasm-workerd` installs the packed tarball into `crates/opportune-wasm/integration/workerd` and runs its Vitest suite inside real Cloudflare `workerd` via `@cloudflare/vitest-plugin`, proving the package's `workerd` loader against a precompiled static `.wasm` import (local `workerd`, so no Cloudflare account or network). The deployment-time Wrangler `CompiledWasm` rule is not exercised: the Vitest plugin compiles `.wasm` itself.
- `make test-wasm-web` installs Playwright's Chromium, Firefox, and WebKit and runs Vitest Browser Mode against the packed tarball, plus a real module Web Worker bundle. Override the browser set with `OPPORTUNE_WEB_BROWSERS=chromium`.
- `make determinism` runs the scenario suite in two separate OS processes and compares canonical output byte-for-byte (CI gates on this in addition to the in-suite check).
- `make stress` runs ~50,000 generated engine-vs.-oracle comparisons spanning every rule kind, multi-participant matching, caps, occurrences, rest, zone periods, and daylight-saving transitions.

### Codegen and Fixtures

- `crates/opportune-py/python/opportune/_models.py`, `crates/opportune-wasm/js/src/generated/types.ts` and `crates/opportune-wasm/js/schema.json` are generated from the combined Rust JSON Schema and must never be hand-edited. After changing a wire type or schema description, run `make codegen` and `make codegen-ts` and commit the result. `make check-codegen-ts` fails on a `types.ts` drift and on a packaged `schema.json` that no longer matches the bundle `export_schema` writes.
- JSON test fixtures under `crates/opportune-py/tests/fixtures/` are loaded via `load_json_fixture` in `crates/opportune-py/tests/conftest.py`.

### Caching and Lockfile Discipline

- `Makefile` sets `RUSTC_WRAPPER=sccache` when `sccache` is installed and caps the local cache at `5G` (`SCCACHE_CACHE_SIZE`). Override either via environment variables. CI uses `Swatinem/rust-cache`.
- `crates/opportune-py/pyproject.toml` sets `exclude-newer = "48 hours"` so `uv lock` never resolves releases published within the last two days. `uv.lock` and `Cargo.lock` are committed and all builds run with `--locked`.
- Toolchain versions are pinned in `rust-toolchain.toml` and `Cargo.toml` (`rust-version`); bump them together deliberately because Dependabot does not track `rust-toolchain.toml`.
- New workspace crates must inherit `[lints] workspace = true` and shared metadata. See [`docs/crates.md`](docs/crates.md) for workspace conventions.

## Before Opening a Pull Request

Run both local gates:

```console
make check     # lint, tests, feature matrix, and docs
make deny      # license, advisory, ban, and source policy
```

CI additionally checks the Bun test lane, the Cloudflare `workerd` and browser/module Web Worker WASM lanes (`make test-wasm-workerd`, `make test-wasm-web`), two-process `make determinism`, MSRV builds, and wheel/sdist packaging.

### Pull Request Titles (Conventional Commits)

PR titles are checked by `.github/workflows/commitlint.yml` and parsed by `release-please` to compute the next version and changelog entry.

- Allowed types: `feat`, `fix`, `docs`, `style`, `refactor`, `perf`, `test`, `chore`, `ci`, `build`, `revert`
- Start the subject with a lowercase letter (for example, `feat: add window builder`, not `Add window builder`).
- With `bump-minor-pre-major` enabled, breaking changes before `1.0.0` bump the minor version (`0.1.0` → `0.2.0`).

## Releasing (Maintainers)

`release-please` opens a release PR from Conventional Commits, bumping `version.txt`, `Cargo.toml` (`$.workspace.package.version`), `crates/opportune-wasm/js/package.json` (`$.version`), `crates/opportune-wasm/js/package-lock.json` (`$.version` and `$.packages[""].version`), and the `VERSION` constant in `crates/opportune-wasm/js/src/constants.ts` together, and `.github/workflows/release-please.yml` reconciles both `Cargo.lock` and `crates/opportune-py/uv.lock` with the bumped workspace version. `make check-version` verifies all six version sources — `version.txt`, `Cargo.toml`, the npm manifest, both npm lockfile version fields, `constants.ts`, and `.release-please-manifest.json` — and `crates/opportune-py/pyproject.toml` reads its version dynamically from Cargo via maturin.

Merging the release PR creates a `vX.Y.Z` tag and triggers `.github/workflows/publish.yml`, which re-runs license, advisory, and test checks, then publishes:

1. Rust crates in dependency order: `opportune` → `opportune-json` → `opportune-adapters` (waiting for crates.io visibility between each).
2. Python sdist and `cp312-abi3` wheels to PyPI.
3. The `opportune` npm package built and tested from the tag.

Re-running the workflow skips registries that already hold the tagged version, so partial releases resume cleanly.

### Trusted Publishing (OIDC) Bootstrap

All three registries publish via OIDC Trusted Publishing with no stored API tokens. Before the very first `0.1.0` publish:

- **PyPI:** Configure a pending trusted publisher for `opportune` with workflow `publish.yml` and environment `pypi`.
- **npm:** npm requires the package to exist before adding a trusted publisher. Publish `0.1.0` manually after `make test-wasm` (`npm publish crates/opportune-wasm/js --ignore-scripts --access public`), then add a trusted publisher for `opportune` with workflow `publish.yml`, environment `npm`, and **Allow npm publish** checked (`repository.url` must match this repo for `--provenance`).
- **crates.io:** Publish `0.1.0` manually in dependency order (`cargo publish -p opportune`, then `-p opportune-json`, then `-p opportune-adapters`), waiting for each crate to become visible before the next, then configure a trusted publisher on **each** of the three crates with workflow `publish.yml` and environment `crates-io`.
- **Tag trigger:** Tags created with the default `GITHUB_TOKEN` do not trigger workflows. Set the `RELEASE_PLEASE_APP_ID` repository variable and `RELEASE_PLEASE_APP_PRIVATE_KEY` secret so release tags start `publish.yml`, or dispatch `publish.yml` manually against the tag.

## Security

See [`SECURITY.md`](SECURITY.md) for private vulnerability reporting.
