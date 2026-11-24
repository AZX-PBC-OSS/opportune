# Dependencies and licensing

## Licensing

The project is dual-licensed under MIT and Apache-2.0, and the dependency tree is permissive-only. The policy is checked twice by `cargo-deny`:

- `cargo deny check` verifies advisories, licenses, bans, and sources across the default-feature graph.
- `cargo deny --config deny.minimal.toml check licenses` re-checks the license policy with default features disabled, ensuring a strict-adopter build does not inherit optional licenses.

Both checks run in CI and in the publish workflow's verify job against the tagged commit before any crate publishes to crates.io.

`deny.toml`'s `[licenses] allow` is the authoritative default list — MIT, Apache-2.0, `Apache-2.0 WITH LLVM-exception`, and the two Unicode data license identifiers. A crate offering `Unlicense OR MIT` (such as `jiff`) satisfies the list through MIT. `deny.minimal.toml` drops `Unicode-DFS-2016`, which is only pulled in by the optional `windows-zones` feature.

## Rust dependencies

The workspace pins its toolchain in `rust-toolchain.toml` and commits one `Cargo.lock` covering all crates; CI builds with `--locked` so dependency updates are always explicit in review. The minimum supported Rust version (MSRV) is set by the strictest dependency and tested in CI on that exact toolchain.

Criteria for adding a Rust dependency:

- License on the allow-list
- Active maintenance and WASM compatibility
- Compatibility with the workspace MSRV
- Small transitive tree, encapsulated behind a single internal module

In-house implementations are used only where general-purpose crates carry different semantics (such as half-open minute intervals and term-ordered weighted matching). Dependabot opens weekly grouped PRs for Cargo dependencies, and `cargo-deny` vets every update before merge.

## npm dependencies

The `opportune` npm package has zero runtime dependencies: the compiled `.wasm` module and generated TypeScript wrapper ship inside the tarball. Everything listed in `crates/opportune-wasm/js/package.json` (`typescript`, `json-schema-to-typescript`, `publint`, `@arethetypeswrong/cli`, `shell-quote`, `yaml`, and `@types/node`) is a `devDependency` and is omitted from the published tarball. CI installs from the committed `package-lock.json` via `npm ci --ignore-scripts`.

`cargo-deny` covers the Rust crates compiled into the WASM module, while a separate weekly Dependabot lane tracks `/crates/opportune-wasm/js`.

## Python dependencies

`crates/opportune-py/pyproject.toml` declares one runtime dependency: `pydantic` (MIT), because the generated models in `opportune._models` are Pydantic v2 models shipped inside the wheel. Development and documentation tools (`datamodel-code-generator`, `fastapi`, `httpx2`, `pyright`, `pytest`, `ruff`, `mkdocs`, `mkdocs-material`) live in `dev` and `docs` dependency groups and are never installed by consumers.

Python dependencies are managed by `uv` with a committed `uv.lock`, synced with `--locked` in CI. `pyproject.toml` sets `exclude-newer = "48 hours"` so re-resolution never adopts a release published within the last two days. Dependabot updates the Python graph weekly. Note that `cargo-deny` checks the Cargo graph only, not Python packages.

### Documentation tooling

`mkdocs-material` is in maintenance mode (critical fixes and security updates only), with upstream maintenance ending on **2027-05-05**. Its maintainers' successor is Zensical. The `docs` dependency group is non-default and only used to build the documentation site, so it never reaches consumers of the Rust crates, Python wheel, or npm package; the EOL date is tracked here to plan migration ahead of time.

## GitHub Actions

Every workflow action is pinned to a commit SHA, and Dependabot keeps those pins updated with upstream security fixes.

## Reporting vulnerabilities

See [`SECURITY.md`](https://github.com/AZX-PBC-OSS/opportune/blob/main/SECURITY.md) — please use GitHub Security Advisories rather than a public issue.
