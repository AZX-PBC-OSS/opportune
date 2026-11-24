.DEFAULT_GOAL := check

.PHONY: check install dev lint check-version test test-rust test-py test-matrix wasm test-wasm test-wasm-workerd test-wasm-web wasm-size wheel deny docs docs-serve check-docs determinism stress codegen check-codegen codegen-ts check-codegen-ts check-docs-snippets test-docs-snippets-py test-docs-snippets-ts

# sccache pays off across repeated clean builds and worktrees, where rust-cache's
# per-checkout target directory cannot help. A local size cap keeps the cache
# bounded; ?= leaves both knobs overridable from the environment.
ifneq ($(shell command -v sccache 2>/dev/null),)
export RUSTC_WRAPPER ?= sccache
export CARGO_INCREMENTAL := 0
SCCACHE_CACHE_SIZE ?= 5G
export SCCACHE_CACHE_SIZE
endif

# uv builds the extension from an sdist unpacked in a cache directory outside the
# repository, where rust-toolchain.toml does not reach and rustup falls back to the
# developer's default toolchain. A default older than the MSRV then fails install, dev and
# wheel with a cryptic "feature `edition2024` is required", while lint and test — which run
# inside the repository, where the pin does apply — keep passing. Export the pin so every
# target compiles with the same toolchain; ?= leaves it overridable from the environment,
# and a cargo that is not a rustup shim simply ignores the variable.
RUSTUP_TOOLCHAIN ?= $(shell sed -n 's/^channel *= *"\(.*\)"/\1/p' rust-toolchain.toml)
ifneq ($(RUSTUP_TOOLCHAIN),)
export RUSTUP_TOOLCHAIN
endif

# The cargo commands run from the repository root, where the virtual workspace and
# Cargo.lock live. The uv commands name crates/opportune-py, where the Python
# project lives: `--project` selects the project but does not change the working
# directory, so the tools that resolve config from the working directory (pytest,
# pyright) get explicit paths into the project. `uv run --locked` for the same reason the
# cargo commands carry it: a run that finds pyproject.toml and uv.lock disagreeing fails
# instead of quietly rewriting the committed lock. (`uv build` resolves nothing, so it has
# no `--locked`; CI's `uv run --no-sync` follows an explicit `uv sync --locked`.)

install:
	uv sync --locked --project crates/opportune-py

# uv's wheel cache does not key on Rust sources, so force the extension rebuild
# after editing crates/*/src/.
dev:
	uv sync --locked --reinstall-package opportune --project crates/opportune-py

# Every publishable distribution carries its own copies of the licence texts because none may
# point at the repository root's: maturin rejects a glob that escapes the project directory and
# cargo silently drops a cross-directory `include` entry. Those copies are what ship in the
# wheel, in each .crate and in the npm tarball, whose manifest has to list them in `files`
# because npm's always-included licence pattern matches `LICENSE` but not the `-MIT`/`-APACHE`
# names, so lint fails if any of them drifts. `ci.yml` runs the same comparison for the crates
# and additionally asserts the copies are inside the packaged .crate artefacts.
#
# The committed Pydantic models are generated from the combined schema, so lint regenerates
# them and fails on any difference before it lints the rest.
lint: check-version check-codegen check-codegen-ts check-docs-snippets
	@for f in LICENSE-MIT LICENSE-APACHE; do \
		for crate in opportune opportune-json opportune-adapters opportune-py; do \
			cmp "$$f" "crates/$$crate/$$f" \
				|| { echo "crates/$$crate/$$f differs from $$f; the published artefact ships the copy"; exit 1; }; \
		done; \
		cmp "$$f" "crates/opportune-wasm/js/$$f" \
			|| { echo "crates/opportune-wasm/js/$$f differs from $$f; the npm package ships the copy"; exit 1; }; \
	done
	@for m in crates/*/Cargo.toml; do \
		awk '/^\[lints\]$$/{l=1; next} l && /^workspace = true$$/{ok=1} END{exit ok?0:1}' "$$m" \
		|| { echo "$$m has no [lints] workspace = true; it inherits no lint policy"; exit 1; }; \
	done
	cargo fmt --all --check
	cargo clippy --all-targets --all-features --locked -- -D warnings
	RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --all-features --locked
	uv run --locked --project crates/opportune-py ruff check crates/opportune-py
	uv run --locked --project crates/opportune-py ruff format --check crates/opportune-py
	uv run --locked --project crates/opportune-py pyright --project crates/opportune-py
	# `npm run check` is `tsc -p tsconfig.check.json`: the package's own strict compile over
	# `src/`, the examples and the suite, no emit. The generated types are part of
	# `src/`, so a schema change that json2ts renders into something the strict flags
	# reject fails here rather than in the package's build; the suite is part of it so
	# a document the generated types stop admitting fails here too. `check-codegen-ts`'s
	# own prerequisite installed `node_modules` from the committed lockfile.
	npm --prefix $(JS) run check

test: test-rust test-py test-wasm

# --all-features so the feature-gated adapter code and its dependencies are compiled,
# linted and tested here too, not only in whatever default feature set happens to be on.
test-rust:
	cargo nextest run --locked --all-features
	cargo test --doc --locked --all-features

test-py:
	uv run --locked --project crates/opportune-py pytest crates/opportune-py/tests
	$(MAKE) test-docs-snippets-py

# The builds the default-featured lint and test targets never compile: the core with
# every feature off, the wire layer and the adapters without the bundled time zone
# database, the adapters as a strict adopter sees them (`windows-zones` and
# `google` compiled out), and each wasm32 target. `ci.yml`'s feature-matrix job runs the
# same set, so a feature combination that stops building fails locally before it costs a
# runner. The adapters line is `cargo test`, not `cargo check`, because the build without
# the Windows mapping is pinned by tests behind `#[cfg(not(feature = "windows-zones"))]`
# — a Windows zone name must come back unresolved rather than be guessed at — and the
# --all-features suites never compile them. A conversion resolves zone names against a
# real database, so that test run keeps `bundled-tzdb` on; the graph-only and
# graph+json `cargo check` lines beside it prove the library needs no bundled data,
# and the `cargo tree` assertions prove the bundle is not in its normal dependencies
# at all — it would otherwise leak back in silently through either crate, including
# through `opportune-json`'s own defaults, which is what an adopter inherits.
#
# A guard that searches an empty stream reports success exactly when it stopped
# checking, so the tree is captured and `cargo tree`'s own status checked before the
# search runs over it: a mistyped package, feature or flag has to fail the guard, not
# turn it into a no-op. The search reads to end of input rather than `grep -q` for the
# other direction — under a `pipefail` shell an early-exiting grep can SIGPIPE
# the writer (`printf`) and make a leak look like a pass.
#
# The graph+json guard's feature list carries a comma, which `$(call)` would read as an
# argument separator; `$(comma)` is what it has to see instead.
comma := ,
define guard_no_bundled_tzdb
	@tree=$$(cargo tree -e normal $(1) --locked) || { \
		echo "test-matrix: cargo tree failed for $(1), so the guard cannot say there is no leak"; \
		exit 1; }; \
	if printf '%s\n' "$$tree" | grep jiff-tzdb >/dev/null; then \
		echo "test-matrix: $(2)"; \
		exit 1; \
	fi
endef

test-matrix:
	cargo test -p opportune --no-default-features --locked
	cargo test --doc -p opportune --no-default-features --locked
	cargo test -p opportune-json --no-default-features --locked
	$(call guard_no_bundled_tzdb,-p opportune-json,the bundled tz database reached opportune-json's default features)
	$(call guard_no_bundled_tzdb,-p opportune-json --no-default-features,jiff-tzdb reaches opportune-json's normal dependencies without bundled-tzdb)
	cargo check -p opportune-adapters --no-default-features --locked
	cargo check -p opportune-adapters --no-default-features --features graph --locked
	$(call guard_no_bundled_tzdb,-p opportune-adapters --no-default-features --features graph,jiff-tzdb reaches opportune-adapters' normal dependencies with graph alone)
	cargo check -p opportune-adapters --no-default-features --features graph,json --locked
	$(call guard_no_bundled_tzdb,-p opportune-adapters --no-default-features --features graph$(comma)json,jiff-tzdb reaches opportune-adapters' normal dependencies without bundled-tzdb)
	cargo check -p opportune-adapters --no-default-features --features google --locked
	$(call guard_no_bundled_tzdb,-p opportune-adapters --no-default-features --features google,jiff-tzdb reaches opportune-adapters' normal dependencies with google alone)
	cargo check -p opportune-adapters --no-default-features --features google,json --locked
	$(call guard_no_bundled_tzdb,-p opportune-adapters --no-default-features --features google$(comma)json,jiff-tzdb reaches opportune-adapters' normal dependencies with google and json)
	cargo check -p opportune-adapters --no-default-features --features json --locked
	$(call guard_no_bundled_tzdb,-p opportune-adapters --no-default-features --features json,jiff-tzdb reaches opportune-adapters' normal dependencies with json alone)
	cargo test -p opportune-adapters --no-default-features --features graph,bundled-tzdb --locked
	cargo test -p opportune-adapters --no-default-features --features google,bundled-tzdb --locked
	cargo test -p opportune-wasm --no-default-features --locked
	cargo check -p opportune-py --no-default-features --locked
	rustup target add wasm32-unknown-unknown
	cargo check -p opportune --target wasm32-unknown-unknown --locked
	cargo check -p opportune-json --no-default-features --target wasm32-unknown-unknown --locked
	cargo check -p opportune-adapters --features json --target wasm32-unknown-unknown --locked
	cargo check -p opportune-wasm --target wasm32-unknown-unknown --locked
	cargo check -p opportune-wasm --no-default-features --target wasm32-unknown-unknown --locked

wheel:
	uv build --project crates/opportune-py --out-dir dist

deny:
	cargo deny check
	# Minimal allow-list: the licence check repeated with default features off and a
	# reduced list. The Windows zone mapping is the one dependency only a default
	# feature reaches, so this proves a strict adopter compiling it out never
	# inherits its Unicode-DFS-2016 licence.
	cargo deny --config deny.minimal.toml check licenses

# The docs group is non-default, so the sync is part of the target: same exact
# --locked discipline as `install`. --no-default-groups and --no-install-project keep a
# standalone run lean: rendering Markdown must not build the Rust extension or install the
# linters. --strict fails on broken nav or links, matching the docs workflow's gate.
docs:
	uv sync --locked --group docs --no-install-project --no-default-groups --project crates/opportune-py
	uv run --no-sync --project crates/opportune-py mkdocs build --strict

# The same build for `check`, which has already installed the dev tooling and the
# extension. Syncing with `--no-default-groups` here would prune both, leaving the
# workspace venv without the tools the next target expects; adding the docs group to the
# existing environment keeps it intact.
check-docs:
	uv run --locked --group docs --project crates/opportune-py mkdocs build --strict

# Live-reload preview of the docs site. The same sync `docs` runs, for the reason
# stated there: the docs group is non-default, so `--no-sync` alone cannot find
# mkdocs on a clone that has not built the site yet.
docs-serve:
	uv sync --locked --group docs --no-install-project --no-default-groups --project crates/opportune-py
	uv run --no-sync --project crates/opportune-py mkdocs serve

# The Pydantic models in crates/opportune-py/python/opportune/_models.py are generated
# from the schema the Rust crates export, never hand-edited: `make codegen` rewrites
# them and `make check-codegen` regenerates them into a temporary directory and fails on
# any difference, which is what `make lint` and CI run. `--disable-timestamp` keeps the
# run's clock out of the file, and `--collapse-root-models` is deliberately absent
# because it drops `Literal[True]` from every selector position. `--use-title-as-name`
# takes each model's name from the schema's title, which is how a selector form and a
# rule kind become `TagSelector` and `WindowRule` rather than numbered classes.
#
# `export_schema` keeps its prose, so every generated field carries the same
# `description` FastAPI reports: a field's prose is part of the OpenAPI document an
# application serves, not only of the Rust API docs. The generator writes it as one
# string literal that no formatter wraps, so ruff's column gate is exempted for this
# one file in `pyproject.toml` and in the drift check below.
#
# `--base-class opportune._base.OpportuneModel` gives every generated model the
# package's base, which is where `validate_assignment` lives. The generator also emits
# an unused `type Model = Any` alias; the post-pass drops it before ruff runs, and
# ruff's own fix pass then removes the `Any` import it leaves behind.
MODELS := crates/opportune-py/python/opportune/_models.py
CODEGEN_FLAGS := --input-file-type jsonschema --output-model-type pydantic_v2.BaseModel \
	--target-python-version 3.12 --use-standard-collections --use-union-operator \
	--use-annotated --use-type-alias --use-title-as-name --field-constraints \
	--allow-population-by-field-name --enum-field-as-literal all --disable-timestamp \
	--base-class opportune._base.OpportuneModel
# The drift check formats its output in a temporary directory, outside the project,
# where ruff would discover no configuration and fall back to its defaults; naming the
# project's config keeps both paths byte-identical.
RUFF := uv run --locked --project crates/opportune-py ruff
RUFF_CONFIG := --config crates/opportune-py/pyproject.toml
# The config's per-file ignores are keyed on the committed path, so the temporary file
# the drift check writes is exempted from them explicitly. Formatting takes the config
# alone: `ruff format` has no rule selection.
RUFF_CODEGEN := $(RUFF_CONFIG) --ignore E501,RUF012

# The post-pass that attaches the keyed dispatch, bounds the wire integers, inlines a
# constrained map value's bound, normalises a block's status and writes `__all__`; the
# project's own environment runs it, and pyright checks it with the package.
POSTPROCESS := uv run --locked --project crates/opportune-py python \
	crates/opportune-py/bin/postprocess_models.py

# The generator writes what the schema says; the post-pass adds what a schema cannot and
# checks what must survive codegen. `Selector`, `Term` and `LeadTimeMax` are unions of
# generated models, and a union reports a value none of them accepts once per arm, so
# each alias carries the package's `KeyedUnion` metadata, which derives the forms from
# the arms and names them in one finding; only the generated module can name the arms,
# so the codegen attaches it. `Block.status` is the document's one open string, so its
# model normalises a provider's unknown value to `busy` the way the engine reads it.
# Every `int32`/`uint32`/`uint64` field in the schema publishes its inclusive minimum
# and maximum directly; the post-pass inlines any synthesised `TypeAliasType` map-value
# helper into its field annotation and verifies that every schema-bounded integer
# carried its `ge`/`le` bounds into `_models.py`. `__all__` lets a check prove every
# generated model and type alias reaches `opportune.__all__`. Every rewrite is
# byte-deterministic, and the post-pass fails loudly when an import, alias, class or
# bound it expects is absent, so a schema change surfaces there rather than shipping
# models that quietly lost the dispatch or the bound.
#
# The one spelling of the pipeline, so `codegen` and `check-codegen` cannot drift from
# each other: `$(1)` is the directory the schema and the models are written to.
# datamodel-codegen records the input's file name in the generated header, which is why
# the schema has the same name in both targets' directories.
define generate_models
	cargo run --quiet --locked -p opportune-testkit --bin export_schema > "$(1)/schema.json"; \
	uv run --locked --project crates/opportune-py datamodel-codegen \
		--input "$(1)/schema.json" --output "$(1)/models.py" $(CODEGEN_FLAGS); \
	$(POSTPROCESS) "$(1)/models.py" "$(1)/schema.json"; \
	$(RUFF) check --fix $(RUFF_CODEGEN) "$(1)/models.py"; \
	$(RUFF) format $(RUFF_CONFIG) "$(1)/models.py"
endef

codegen:
	@set -e; dir=$$(mktemp -d); trap 'rm -rf "$$dir"' EXIT; \
	$(call generate_models,$$dir); \
	cp "$$dir/models.py" "$(MODELS)"; \
	echo "codegen: wrote $(MODELS)"

check-codegen:
	@set -e; dir=$$(mktemp -d); trap 'rm -rf "$$dir"' EXIT; \
	$(call generate_models,$$dir); \
	if cmp -s "$$dir/models.py" "$(MODELS)"; then \
		echo "check-codegen: the committed models match the schema"; \
	else \
		diff -u "$(MODELS)" "$$dir/models.py" | head -60; \
		echo "check-codegen: $(MODELS) is out of date; run make codegen"; \
		exit 1; \
	fi

# The TypeScript types in crates/opportune-wasm/js/src/generated/types.ts are
# generated from the same combined schema as the Pydantic models, never
# hand-edited: `make codegen-ts` rewrites them and `make check-codegen-ts`
# regenerates them into a temporary directory and fails on any difference, which
# `make lint` runs. json-schema-to-typescript reads the draft-07 dialect,
# so `bin/prepare-schema.mjs` first rewrites the 2020-12 bundle into it (`$defs`
# → `definitions`, `prefixItems` tuples → `items` arrays) and drops the
# `default` sibling of a `$ref`, which json2ts would otherwise intersect into
# the referenced type — a guard with no live instance now, since a defaulted
# field states its shape as an `anyOf` with the `default` beside it. Sibling
# `description`s survive: they are what json2ts renders as each field's JSDoc.
# The root bundle itself is not a type, so the empty `interface Schema` json2ts
# emits for it is stripped: the awk pass drops that exact three-line block and
# nothing else, so a real definition named `Schema` — which the drift gate,
# regenerating the same output, could not police — survives.
#
# `--unreachableDefinitions` is what makes the definitions emit at all: the
# bundle's root carries no type of its own, so nothing is reachable and the flag
# is how the whole definitions map becomes the output. The generated file has
# to compile strict — `tsconfig.json` includes all of src, generated code
# included — and there is no formatter after json2ts, whose prettier pass is
# deterministic, so the file regenerates byte-identically and the drift gate is
# a plain byte comparison.
JS := crates/opportune-wasm/js
TYPES := $(JS)/src/generated/types.ts
SCHEMA := $(JS)/schema.json
JSON2TS := $(JS)/node_modules/.bin/json2ts
TS_BANNER := /* Generated from opportune JSON Schema — do not edit by hand; run make codegen-ts. */

# npm installs from the committed lockfile, and only when it changed: make
# reruns `npm ci` exactly when package-lock.json is newer than the installed
# json2ts binary, the same --locked discipline the cargo and uv commands carry.
# The targets depend on it, so a fresh clone bootstraps its node_modules by
# running either codegen target rather than failing on a missing tool.
#
# --ignore-scripts: no dependency in the committed lockfile ships a lifecycle
# script, so skipping them costs nothing here, and it is what keeps
# publish.yml's publish-npm — the job holding `id-token: write`, which reaches
# npm ci only through this target — from ever executing a dependency install
# script.
$(JSON2TS): $(JS)/package-lock.json
	npm --prefix $(JS) ci --silent --ignore-scripts
	touch $@

# The release version lives in six files: `version.txt`, the root manifest's
# `[workspace.package]`, the npm package's manifest, its lockfile's two version
# fields, `crates/opportune-wasm/js/src/constants.ts` and release-please's own
# manifest. `release-please`'s `extra-files` is the only thing that moves them
# together, and `publish.yml` refuses a tag where they disagree — but that check
# runs after the release PR has merged, where a silent drift costs the whole tag
# (every publishing lane is gated on it). This is the same comparison, on the PR
# that makes the bump. The toolchain pin is checked beside the versions:
# rust-toolchain.toml's channel must keep naming the declared MSRV (`rust-version`,
# which rustup spells with the patch component the manifest may omit), so a
# deliberate bump moves the two together and a drifted pin fails here instead of
# compiling everything with a compiler nobody declared.
check-version:
	@workspace="$$(python3 -c 'import tomllib; print(tomllib.load(open("Cargo.toml", "rb"))["workspace"]["package"]["version"])')"; \
	file="$$(python3 -c 'print(open("version.txt").read().strip())')"; \
	npm="$$(python3 -c 'import json; print(json.load(open("$(JS)/package.json"))["version"])')"; \
	npm_lock="$$(python3 -c 'import json; print(json.load(open("$(JS)/package-lock.json"))["version"])')"; \
	npm_lock_root="$$(python3 -c 'import json; print(json.load(open("$(JS)/package-lock.json"))["packages"][""]["version"])')"; \
	constants="$$(python3 -c 'import re; m = re.search(r"export const VERSION: string = \"([^\"]+)\"", open("$(JS)/src/constants.ts").read()); print(m.group(1) if m else "")')"; \
	manifest="$$(python3 -c 'import json; print(json.load(open(".release-please-manifest.json"))["."])')"; \
	msrv="$$(python3 -c 'import tomllib; print(tomllib.load(open("Cargo.toml", "rb"))["workspace"]["package"]["rust-version"])')"; \
	channel="$$(sed -n 's/^channel *= *"\(.*\)"/\1/p' rust-toolchain.toml)"; \
	if [ "$$workspace" != "$$file" ] || [ "$$workspace" != "$$npm" ] || [ "$$workspace" != "$$npm_lock" ] || [ "$$workspace" != "$$npm_lock_root" ] || [ "$$workspace" != "$$constants" ] || [ "$$workspace" != "$$manifest" ]; then \
		echo "version drift: Cargo.toml=$$workspace version.txt=$$file npm=$$npm npm-lock=$$npm_lock npm-lock-root=$$npm_lock_root constants.ts=$$constants .release-please-manifest.json=$$manifest"; \
		echo "release-please's extra-files entry is what keeps them equal; fix all six together"; \
		exit 1; \
	fi; \
	case "$$channel" in \
		"$$msrv"|"$${msrv}."*) ;; \
		*) echo "toolchain drift: rust-toolchain.toml channel ($$channel) does not name the declared MSRV rust-version ($$msrv); bump them together"; exit 1 ;; \
	esac; \
	printf 'check-version: %s in Cargo.toml, version.txt, the npm manifest, the npm lockfile (both version fields), constants.ts and .release-please-manifest.json; toolchain %s matches the MSRV\n' "$$workspace" "$$channel"

# The one spelling of the pipeline, so `codegen-ts` and `check-codegen-ts`
# cannot drift from each other: `$(1)` is the directory the schema and the
# types are written to. json2ts names the root type from the schema file's
# name, so the file it reads is called schema.json in both.
#
# The schema `export_schema` writes is kept as `schema.raw.json` before
# `prepare-schema.mjs` rewrites a copy for json2ts, and `codegen-ts` publishes
# that raw bundle as `js/schema.json`. The package therefore ships the same
# Draft 2020-12 document the Rust crates export — `$defs` and `prefixItems`
# intact — and the drift gate compares both files, so neither the types nor the
# schema can be hand-edited into disagreement with the crate.
define generate_types
	cargo run --quiet --locked -p opportune-testkit --bin export_schema > "$(1)/schema.raw.json"; \
	node "$(JS)/bin/prepare-schema.mjs" < "$(1)/schema.raw.json" > "$(1)/schema.json"; \
	$(JSON2TS) --input "$(1)/schema.json" --output "$(1)/types.raw.ts" \
		--additionalProperties=false --unreachableDefinitions \
		--bannerComment "$(TS_BANNER)"; \
	awk '/^export interface Schema \{$$/{held=$$0; getline a; getline b; if (a ~ /^  \[k: string\]: unknown;$$/ && b ~ /^}$$/) next; print held; print a; print b; next} {print}' "$(1)/types.raw.ts" > "$(1)/types.ts"; \
	rm "$(1)/types.raw.ts"
endef

codegen-ts: $(JSON2TS)
	@set -e; dir=$$(mktemp -d); trap 'rm -rf "$$dir"' EXIT; \
	$(call generate_types,$$dir); \
	mkdir -p $(JS)/src/generated; \
	cp "$$dir/types.ts" "$(TYPES)"; \
	cp "$$dir/schema.raw.json" "$(SCHEMA)"; \
	echo "codegen-ts: wrote $(TYPES) and $(SCHEMA)"

check-codegen-ts: $(JSON2TS)
	@set -e; dir=$$(mktemp -d); trap 'rm -rf "$$dir"' EXIT; \
	$(call generate_types,$$dir); \
	if cmp -s "$$dir/types.ts" "$(TYPES)"; then \
		echo "check-codegen-ts: the committed types match the schema"; \
	else \
		diff -u "$(TYPES)" "$$dir/types.ts" | head -60; \
		echo "check-codegen-ts: $(TYPES) is out of date; run make codegen-ts"; \
		exit 1; \
	fi; \
	if cmp -s "$$dir/schema.raw.json" "$(SCHEMA)"; then \
		echo "check-codegen-ts: the committed schema.json matches the exported schema"; \
	else \
		diff -u "$(SCHEMA)" "$$dir/schema.raw.json" | head -60; \
		echo "check-codegen-ts: $(SCHEMA) is out of date; run make codegen-ts"; \
		exit 1; \
	fi

# `pkg/` and `pkg-node/` are what wasm-pack writes — one crate built for two targets,
# emitting byte-identical modules and one glue per target — and `dist/` is what the
# package publishes, so the order is load-bearing: `npm run build` bundles the compiled
# module and both generated glues into `dist/pkg/`, and the suite answers from `dist/`,
# under whichever runner drives it. The `web` target's ESM glue is the loader the Edge,
# Worker and browser entries select; the `nodejs` target's CommonJS glue is the one Node
# and Bun require lazily, and it reads and instantiates the module itself. --release is
# the module the size budget is measured on and the one a consumer downloads.
wasm: $(JSON2TS)
	wasm-pack build crates/opportune-wasm --target web --out-dir js/pkg --release
	wasm-pack build crates/opportune-wasm --target nodejs --out-dir js/pkg-node --release
	npm --prefix $(JS) run build

# The budget is enforced as part of the suite target, on the same build the suite
# answers from: one release module serves both, and a package over budget is never the
# one the suite proved. The suite needs `dist/`, so it cannot run against a separately
# built module without the two drifting apart.
wasm-size: wasm
	node $(JS)/scripts/check-wasm-size.mjs

test-wasm: wasm-size
	npm --prefix $(JS) test
	$(MAKE) test-docs-snippets-ts

# The runtimes `test-wasm` cannot reach. Both harnesses are private, outside
# `package.json#files`, pinned by committed lockfiles, and consume the packed tarball a
# consumer installs rather than `dist/` — the artefact `test-wasm` uploads in CI. Every
# install uses `--ignore-scripts`, and the tarball is installed with `--no-save` so the
# committed lockfile stays what `npm ci` resolves. The `trap` leaves no tarball behind.

# Cloudflare Workers: `workerd` refuses `WebAssembly.compile` and
# `new WebAssembly.Module(bytes)`, so the package's `workerd` loader imports the module
# its bundle compiled instead of compiling one at run time — and only the real runtime can
# say whether that works. Cloudflare's Vitest plugin builds `integration/workerd` into the
# Worker `wrangler.jsonc` describes and runs the suite inside it.
WORKERD := crates/opportune-wasm/integration/workerd
WORKERD_TARBALL ?=

test-wasm-workerd: $(if $(WORKERD_TARBALL),,wasm)
	@set -e; dir=$$(mktemp -d); trap 'rm -rf "$$dir"' EXIT; \
	tarball='$(WORKERD_TARBALL)'; \
	npm --prefix $(WORKERD) ci --silent --ignore-scripts; \
	if [ -z "$$tarball" ]; then \
		(cd $(JS) && npm pack --pack-destination "$$dir" --ignore-scripts --silent); \
		tarball=$$(ls "$$dir"/*.tgz); \
	fi; \
	npm install --prefix $(WORKERD) --no-save --ignore-scripts --offline \
		--no-audit --no-fund --silent "$$tarball"; \
	cd $(WORKERD) && ./node_modules/.bin/vitest run

# Browsers and generic Web Workers: Vitest Browser Mode with Playwright drives Chromium,
# Firefox, WebKit, and a real module Web Worker against the packed tarball. Browsers are
# installed by the Playwright CLI, never by an npm lifecycle script.
WEB := crates/opportune-wasm/integration/web
WEB_TARBALL ?=

test-wasm-web: $(if $(WEB_TARBALL),,wasm)
	@set -e; dir=$$(mktemp -d); trap 'rm -rf "$$dir"' EXIT; \
	tarball='$(WEB_TARBALL)'; \
	npm --prefix $(WEB) ci --silent --ignore-scripts; \
	if [ -z "$$tarball" ]; then \
		(cd $(JS) && npm pack --pack-destination "$$dir" --ignore-scripts --silent); \
		tarball=$$(ls "$$dir"/*.tgz); \
	fi; \
	npm install --prefix $(WEB) --no-save --ignore-scripts --offline \
		--no-audit --no-fund --silent "$$tarball"; \
	cd $(WEB) && ./node_modules/.bin/playwright install chromium firefox webkit && \
		./node_modules/.bin/vitest run

# The Python and TypeScript code the documentation shows is a claim the gate checks
# against the packages a reader installs; `crates/opportune-py/bin/doc_snippets.py`
# extracts every classified fence from Markdown rather than keeping a second corpus.
# The classifier is standard-library Python and needs neither the extension nor a
# build, so `lint` runs it everywhere; each language's lane lives in the target whose
# environment it needs. Python's lane is Pyright strict plus execution against the
# project venv's extension; TypeScript's is `tsc` under the package's strict flags
# plus execution against the built `dist/`, so it runs after `make wasm`.
DOC_SNIPPETS := crates/opportune-py/bin/doc_snippets.py

check-docs-snippets:
	python3 $(DOC_SNIPPETS)

test-docs-snippets-py:
	uv run --locked --project crates/opportune-py python $(DOC_SNIPPETS) --python

test-docs-snippets-ts:
	python3 $(DOC_SNIPPETS) --typescript

check: lint test test-matrix check-docs

# The whole scenario suite runs in two separate OS processes and the two canonical
# renderings are compared byte for byte. Two calls inside one process would share
# every cache and iteration order between them, so they could agree while the
# engine is still order-dependent; this launches the dump binary twice instead.
determinism:
	@first=$$(mktemp); second=$$(mktemp); trap 'rm -f "$$first" "$$second"' EXIT; \
	cargo run --quiet --locked -p opportune-testkit --bin scenario_dump > "$$first" || { echo "determinism: the scenario dump failed"; exit 1; }; \
	cargo run --quiet --locked -p opportune-testkit --bin scenario_dump > "$$second" || { echo "determinism: the second scenario dump failed"; exit 1; }; \
	lines=$$(wc -l < "$$first" | tr -d ' '); \
	if [ "$$lines" -lt 1000 ]; then \
		echo "determinism: the dump produced $$lines lines, too few to be the scenario suite"; \
		exit 1; \
	fi; \
	if cmp -s "$$first" "$$second"; then \
		echo "determinism: two processes agreed on $$lines lines"; \
	else \
		diff -u "$$first" "$$second" | head -40; \
		echo "determinism: the two processes rendered the scenario suite differently"; \
		exit 1; \
	fi

# Fifty thousand generated oracle comparisons, far more than `make test` runs: the
# property gate compares a few hundred cases per property on every change, and this
# runs the same comparisons at a corpus size that only makes sense before a release.
stress:
	cargo run --release --locked -p opportune-testkit --bin stress
