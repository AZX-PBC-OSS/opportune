# Crate structure

The repository is a single Cargo workspace: one version, one lockfile, one release train. The root `Cargo.toml` is a virtual manifest holding shared package metadata, `[workspace.dependencies]`, `[workspace.lints]`, and the release profile. Crates live under `crates/<name>/`.

| Crate | Role |
| --- | --- |
| `opportune` | Pure-Rust scheduling core: domain types, builders, presets, availability engine, validation, and reasons. No PyO3, no serde. Published to crates.io. |
| `opportune-json` | Wire layer shared across bindings: wire types separate from domain types, validating conversions, RFC 6901 JSON-pointer errors, Draft 2020-12 JSON Schema export, and versioned upgrades. Published to crates.io. |
| `opportune-adapters` | Microsoft Graph (Entra / Outlook) and Google Calendar payload conversions into domain blocks and rules. Translation layer only — no network, no clock — with the per-calendar outcome contract, marker constants, Graph working-hours conversion, and `wasm32`-clean JSON envelopes. Published to crates.io. |
| `opportune-testkit` | Independent reference oracle, property generators, golden corpus, and benchmark workloads. Never published (`publish = false`). |
| `opportune-py` | PyO3 cdylib (`_opportune`) and typed `opportune` Python package (generated Pydantic v2 models, presets, sugar, and adapter wrappers), plus `pyproject.toml`, `uv.lock`, and tests. Published to PyPI (`publish = false` for crates.io). |
| `opportune-wasm` | WASM binding crate (`crates/opportune-wasm`) and `opportune` npm package (`crates/opportune-wasm/js`) with generated TypeScript types, presets, sugar, and Graph/Google adapter submodules. Published to npm (`publish = false` for crates.io). |

The publish workflow publishes the three crates.io packages in dependency order (`opportune`, `opportune-json`, `opportune-adapters`), waiting for each to appear on crates.io before verifying the next. `opportune-py`, `opportune-testkit`, and `opportune-wasm` set `publish = false` so they cannot be accidentally published to crates.io.

## Architectural boundaries

- **The core has no serde, no parsing, and no provider knowledge.** Every language binding crosses the boundary via JSON through `opportune-json`'s wire types. Scheduling semantics are defined once in Rust and shared identically across every surface.
- **Adapters translate, never interpret.** Provider payloads convert into standard domain types; adapters cannot alter engine semantics.
- **Single version source.** `release-please` bumps `version.txt`, `Cargo.toml` (`$.workspace.package.version`), `crates/opportune-wasm/js/package.json` (`$.version`), `crates/opportune-wasm/js/package-lock.json` (`$.version` and `$.packages[""].version`), and the `VERSION` constant in `crates/opportune-wasm/js/src/constants.ts` together; `.github/workflows/release-please.yml` reconciles both `Cargo.lock` and `crates/opportune-py/uv.lock`; `opportune-py` reads its version dynamically from Cargo via maturin. `make check-version` verifies the six version sources — `version.txt`, `Cargo.toml`, the npm manifest, both npm lockfile version fields, `constants.ts`, and `.release-please-manifest.json` — and checks that `rust-toolchain.toml` matches `rust-version` on every PR and release tag. Cross-crate path dependencies declare a compatible version range (`>=0.1.0, <0.2.0`), which must be updated in any release crossing `0.2.0`.
- **Single lint policy.** `[workspace.lints]` in the root manifest enforces `missing_docs = "deny"`, `unsafe_code = "forbid"`, and `clippy::all = "deny"`. Every workspace member inherits it via `[lints] workspace = true`, enforced by `make lint` and CI.

## Adding a crate

1. Create `crates/<name>/` and add it to `workspace.members` in the root `Cargo.toml`.
2. Inherit shared metadata (`version.workspace = true`, `edition.workspace = true`, `license.workspace = true`, etc.) and `[lints] workspace = true`.
3. Declare cross-crate path dependencies with both lower and upper version bounds (for example, `>=0.1.0, <0.2.0`; `cargo-deny` rejects version-free path dependencies).
4. Reuse shared crates (`serde`, `serde_json`, `thiserror`) via `{ workspace = true }`.
