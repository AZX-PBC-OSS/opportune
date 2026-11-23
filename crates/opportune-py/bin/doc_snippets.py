"""The Python and TypeScript code the documentation shows, checked against the real packages.

Every `python` and `ts` fence in `README.md`, the `docs/` tree and the published
npm package's `README.md` is a snippet the documentation promises works: Python under
Pyright strict and, when the block is a whole program, executed against the installed
extension; TypeScript under the npm package's strict `tsc` (noUncheckedIndexedAccess
included) and executed against the built `dist/`. Markdown stays the one home of a
snippet body — this driver extracts it rather than keeping a second copy.

A fence opts into the gate with a stable identifier on the line before it:

    <!-- snippet: u1-python -->
    ```python
    ...
    ```

The manifest holds only metadata, keyed by that identifier: which page the
snippet lives on, its language, whether it is a whole program (`run`) or only a
block to type-check (`check`), and which recorded provider payloads it names. A
`python` or `ts` fence with no identifier fails the gate, so a new block cannot
be added without being classified; an identifier the manifest does not name, or
one whose page or language disagrees, fails it too.

A fence is read the way a renderer reads one: it may be indented up to three
spaces, and its info string names the language in its first word and leaves the
rest to the renderer — `title="…"` only captions the block. `py` and
`typescript` name the two gated languages too, so a page that spells the tag out
is checked rather than skipped. A gated block indented further than three spaces
sits inside a list or an admonition, a container this scanner does not read: it
fails the gate instead of leaving the block unchecked. A fence quoted behind a
blockquote marker is outside what this scanner reads at all; no page uses one,
and a page that starts to is a reason to teach the scanner the construct rather
than to trust the silence.

A snippet whose prose names a recorded provider payload gets it from the
fixture map in its manifest entry. The manifest spells the Python name of the
variable (`calendar_view_payload`); the TypeScript lane reads the same entry as
its camelCase twin (`calendarViewPayload`), which is the spelling the recipes
page promises for the two surfaces. The prologue that defines it is real code
reading the recorded file, so a snippet compiles and runs against the same
payload the adapter suites use.

    python3 crates/opportune-py/bin/doc_snippets.py
    uv run --project crates/opportune-py python .../doc_snippets.py --python
    python3 crates/opportune-py/bin/doc_snippets.py --typescript

With no lane named it classifies only, which is what `make check-docs-snippets`
runs in the fast lint stage; either lane first classifies, so a page cannot be
half-covered.
"""

from __future__ import annotations

import argparse
import json
import re
import shutil
import subprocess
import sys
import tempfile
from dataclasses import dataclass, field
from pathlib import Path
from typing import cast

#: The identifier comment a gated fence carries on the line before it, indented
#: with the fence when a page nests the block.
MARKER = re.compile(r"^ *<!-- snippet: (?P<id>[a-z0-9][a-z0-9-]*) -->$")

#: A fence opener or closer: indentation, three or more backticks or tildes, and
#: the info string a renderer reads. Only the first word of the info string names
#: the language; the rest is the renderer's (`title="…"` captions a block).
FENCE = re.compile(r"^(?P<indent> *)(?P<fence>`{3,}|~{3,})(?P<info>.*)$")

#: The languages this gate owns. A fence in either must be classified.
GATED_LANGUAGES = ("python", "ts")

#: Every spelling of a gated language, and the tag the manifest names it by.
LANGUAGE_NAMES = {
    "python": "python",
    "py": "python",
    "python3": "python",
    "py3": "python",
    "ts": "ts",
    "typescript": "ts",
}

#: How far a fence may be indented and still be read as one, as CommonMark counts
#: it. A gated block nested deeper sits inside a container this scanner cannot
#: read, so it is reported rather than skipped.
MAX_FENCE_INDENT = 3

#: The documentation surfaces the gate reads. A page is read whether or not the
#: manifest has been updated for it, so a block cannot escape checking by living
#: on a page nobody remembered to list. The npm package's README is a page a
#: reader installs rather than one they browse: its TypeScript fences are the
#: first code most consumers copy, so it is gated beside the repository's own.
PAGES = ("README.md", "docs/**/*.md", "crates/opportune-wasm/js/README.md")

#: The modes a manifest entry may name.
MODES = ("run", "check")

#: The repository root, from this file's position: `crates/opportune-py/bin/`.
ROOT = Path(__file__).resolve().parents[3]

#: Where the manifest and this driver live.
HERE = Path(__file__).resolve().parent

#: The manifest this driver reads, beside it.
MANIFEST = HERE / "doc_snippets.json"


@dataclass(frozen=True)
class Fence:
    """One fenced block on a page, and the identifier the page gave it."""

    page: str
    line: int
    language: str
    marker: str | None
    body: str
    closed: bool = True
    indent: int = 0


@dataclass(frozen=True)
class Snippet:
    """A classified fence: the body to check and the metadata that decides how."""

    id: str
    page: str
    line: int
    language: str
    mode: str
    fixtures: dict[str, str]
    body: str


@dataclass
class Manifest:
    """The pages the gate reads and the snippets they are expected to carry."""

    pages: list[str]
    fixtures_root: str
    typescript_package: str
    snippets: dict[str, dict[str, object]]
    errors: list[str] = field(default_factory=list[str])


def object_at(value: object) -> dict[str, object] | None:
    """`value` as a JSON object with string keys, or `None` when it is not one."""
    if not isinstance(value, dict):
        return None
    table = cast("dict[object, object]", value)
    if not all(isinstance(key, str) for key in table):
        return None
    return {str(key): item for key, item in table.items()}


def list_at(value: object) -> list[object] | None:
    """`value` as a list, or `None` when it is not one."""
    if not isinstance(value, list):
        return None
    return list(cast("list[object]", value))


def load_manifest(path: Path) -> Manifest:
    """The manifest at `path`, or the findings that make it unreadable."""
    try:
        raw: object = json.loads(path.read_text(encoding="utf-8"))
    except FileNotFoundError:
        return Manifest([], "", "", {}, [f"{path} does not exist"])
    except json.JSONDecodeError as error:
        return Manifest([], "", "", {}, [f"{path} is not JSON: {error}"])
    document = object_at(raw)
    if document is None:
        return Manifest([], "", "", {}, [f"{path} names no object"])
    errors: list[str] = []
    listed = list_at(document.get("pages"))
    if listed is None or not all(isinstance(page, str) for page in listed):
        errors.append(f"{path}: `pages` must list the Markdown files the gate reads")
        listed = []
    pages = [page for page in listed if isinstance(page, str)]
    fixtures_root = document.get("fixtures")
    typescript_package = document.get("typescript_package")
    for name, value in (
        ("fixtures", fixtures_root),
        ("typescript_package", typescript_package),
    ):
        if not isinstance(value, str) or not value:
            errors.append(f"{path}: `{name}` must name a directory")
    snippets: dict[str, dict[str, object]] = {}
    entries = list_at(document.get("snippets"))
    if entries is None:
        errors.append(f"{path}: `snippets` must be a list")
        entries = []
    for index, entry in enumerate(entries):
        where = f"{path}: snippets[{index}]"
        fields = object_at(entry)
        if fields is None:
            errors.append(f"{where} is not an object")
            continue
        identifier = fields.get("id")
        if not isinstance(identifier, str) or not identifier:
            errors.append(f"{where} names no `id`")
            continue
        if identifier in snippets:
            errors.append(f"{where} repeats the id `{identifier}`")
            continue
        page = fields.get("page")
        language = fields.get("language")
        mode = fields.get("mode")
        if not isinstance(page, str) or page not in pages:
            errors.append(
                f"{where} (`{identifier}`) names page {page!r}, which `pages` does not list"
            )
        if language not in GATED_LANGUAGES:
            errors.append(
                f"{where} (`{identifier}`) names language {language!r}, "
                f"not one of {GATED_LANGUAGES}"
            )
        if mode not in MODES:
            errors.append(f"{where} (`{identifier}`) names mode {mode!r}, not one of {MODES}")
        fixtures = object_at(fields.get("fixtures", {}))
        if fixtures is None:
            message = f"{where} (`{identifier}`) has a `fixtures` map that is not name -> path"
            errors.append(message)
            fixtures = {}
        snippets[identifier] = {
            "page": page,
            "language": language,
            "mode": mode,
            "fixtures": {name: value for name, value in fixtures.items() if isinstance(value, str)},
        }
    return Manifest(
        pages,
        fixtures_root if isinstance(fixtures_root, str) else "",
        typescript_package if isinstance(typescript_package, str) else "",
        snippets,
        errors,
    )


def fence_language(info: str) -> str:
    """The language a fence's info string names, in the tag the manifest knows it by."""
    words = info.split()
    if not words:
        return ""
    tag = words[0].lower()
    return LANGUAGE_NAMES.get(tag, tag)


def dedent(lines: list[str], indent: int) -> list[str]:
    """`lines` with up to `indent` leading spaces removed, as a renderer reads a fence."""
    stripped: list[str] = []
    for line in lines:
        leading = len(line) - len(line.lstrip(" "))
        stripped.append(line[min(indent, leading) :])
    return stripped


def fences(root: Path, pages: list[str]) -> list[Fence]:
    """Every fenced block on `pages`, with the identifier comment naming it, if any."""
    found: list[Fence] = []
    for page in pages:
        lines = (root / page).read_text(encoding="utf-8").split("\n")
        marker: str | None = None
        opened: tuple[int, str, int, str, str | None] | None = None
        for number, line in enumerate(lines, start=1):
            if opened is not None:
                start, opener, indent, language, named = opened
                # A closer uses the opener's delimiter, is indented no more than
                # three spaces, carries no info string and is at least as long, so
                # a longer fence wrapping a shorter one — or a backtick fence
                # wrapping tildes — stays open.
                closing = FENCE.match(line)
                if (
                    closing is not None
                    and len(closing["indent"]) <= MAX_FENCE_INDENT
                    and closing["fence"][0] == opener[0]
                    and len(closing["fence"]) >= len(opener)
                    and not closing["info"].strip()
                ):
                    body = "\n".join(dedent(lines[start : number - 1], indent))
                    found.append(Fence(page, start, language, named, body, indent=indent))
                    opened = None
                continue
            opening = FENCE.match(line)
            if opening is not None:
                language = fence_language(opening["info"])
                if language:
                    opened = (number, opening["fence"], len(opening["indent"]), language, marker)
                    marker = None
                    continue
            identified = MARKER.match(line)
            if identified is not None:
                marker = identified["id"]
            elif line.strip():
                marker = None
        if opened is not None:
            start, _, indent, language, _ = opened
            found.append(Fence(page, start, language, None, "", closed=False, indent=indent))
    return found


def documentation_pages(root: Path) -> list[str]:
    """Every documentation page under `root`, as relative paths, in a stable order."""
    return sorted(
        {
            page.relative_to(root).as_posix()
            for pattern in PAGES
            for page in root.glob(pattern)
            if page.is_file()
        }
    )


def classify(root: Path, manifest: Manifest) -> tuple[list[Snippet], list[str]]:
    """Every gated fence, paired with its manifest entry, or the findings that stop it."""
    errors = list(manifest.errors)
    snippets: list[Snippet] = []
    seen: dict[str, Fence] = {}
    pages = documentation_pages(root)
    for page in manifest.pages:
        if page not in pages:
            errors.append(f"the manifest reads {page}, which is not a page on disk")
    for fence in fences(root, pages):
        if fence.language not in GATED_LANGUAGES:
            continue
        where = f"{fence.page}:{fence.line}"
        if fence.indent > MAX_FENCE_INDENT:
            errors.append(
                f"{where}: a `{fence.language}` block is indented {fence.indent} spaces, "
                "inside a list or an admonition this gate does not read, so its code "
                "would go unchecked; indent it three spaces or fewer, or label it "
                "`text` if it is not a program a reader can copy"
            )
            continue
        if not fence.closed:
            errors.append(f"{where}: the `{fence.language}` block is never closed")
            continue
        if fence.marker is None:
            errors.append(
                f"{where}: a `{fence.language}` block has no `<!-- snippet: id -->` "
                "identifier, so it is not classified; name it in the manifest, or "
                "label it `text` if it is not code the gate can check"
            )
            continue
        entry = manifest.snippets.get(fence.marker)
        if entry is None:
            errors.append(f"{where}: `{fence.marker}` is not an entry in the snippet manifest")
            continue
        if fence.marker in seen:
            first = seen[fence.marker]
            errors.append(
                f"{where}: `{fence.marker}` is also the identifier at {first.page}:{first.line}"
            )
            continue
        seen[fence.marker] = fence
        if entry["page"] != fence.page:
            errors.append(f"{where}: `{fence.marker}` is declared for {entry['page']}")
        if entry["language"] != fence.language:
            errors.append(
                f"{where}: `{fence.marker}` is declared as {entry['language']}, "
                f"not `{fence.language}`"
            )
        language = entry["language"]
        mode = entry["mode"]
        fixtures = object_at(entry["fixtures"]) or {}
        if not isinstance(language, str) or not isinstance(mode, str):
            continue
        named: dict[str, str] = {
            name: value for name, value in fixtures.items() if isinstance(value, str)
        }
        snippet = Snippet(
            fence.marker,
            fence.page,
            fence.line,
            fence.language,
            mode,
            named,
            fence.body,
        )
        errors.extend(validate_fixtures(root, manifest, snippet))
        snippets.append(snippet)
    for identifier, entry in manifest.snippets.items():
        if identifier not in seen:
            errors.append(
                f"the manifest names `{identifier}` on {entry['page']}, "
                "but no such snippet is there"
            )
    return snippets, errors


def validate_fixtures(root: Path, manifest: Manifest, snippet: Snippet) -> list[str]:
    """The findings for a snippet's fixture paths that do not name a recorded file."""
    errors: list[str] = []
    for name, relative in snippet.fixtures.items():
        if not (root / manifest.fixtures_root / relative).is_file():
            errors.append(
                f"{snippet.page}:{snippet.line}: `{snippet.id}` names fixture "
                f"{relative!r} for `{name}`, which is not a file under "
                f"{manifest.fixtures_root}"
            )
    return errors


def camel_case(name: str) -> str:
    """The TypeScript spelling of a fixture name: `calendar_view` -> `calendarView`."""
    head, *rest = name.split("_")
    return head + "".join(word.capitalize() for word in rest)


def py_fixture_prologue(root: Path, manifest: Manifest, snippet: Snippet) -> str:
    """The Python lines that read this snippet's recorded provider payloads."""
    lines = [
        "# The recorded provider payloads this block's prose names.",
        "from pathlib import Path as _SnippetPath",
    ]
    for name, relative in snippet.fixtures.items():
        path = root / manifest.fixtures_root / relative
        lines.append(f"{name}: str = _SnippetPath({str(path)!r}).read_text(encoding='utf-8')")
    return "\n".join(lines)


def ts_fixture_prologue(root: Path, manifest: Manifest, snippet: Snippet) -> str:
    """The TypeScript lines that read this snippet's recorded provider payloads."""
    lines = [
        "// The recorded provider payloads this block's prose names.",
        'import { readFileSync as _snippetReadFileSync } from "node:fs";',
    ]
    for name, relative in snippet.fixtures.items():
        path = root / manifest.fixtures_root / relative
        lines.append(
            f'const {camel_case(name)}: string = _snippetReadFileSync({str(path)!r}, "utf8");'
        )
    return "\n".join(lines)


def compose(root: Path, manifest: Manifest, snippet: Snippet) -> str:
    """The snippet body with the prologue its fixtures need, as one program."""
    prologue = (
        py_fixture_prologue(root, manifest, snippet)
        if snippet.language == "python"
        else ts_fixture_prologue(root, manifest, snippet)
    )
    if not snippet.fixtures:
        return snippet.body + "\n"
    return f"{prologue}\n\n{snippet.body}\n"


def run(command: list[str], cwd: Path) -> subprocess.CompletedProcess[str]:
    """Run `command`, echoing it first so a failure says what produced the output."""
    print(f"+ {' '.join(command)}", flush=True)
    return subprocess.run(command, cwd=cwd, text=True, capture_output=True, check=False)


def report(process: subprocess.CompletedProcess[str], what: str) -> list[str]:
    """The finding for a failed `process`, with its output kept."""
    if process.returncode == 0:
        return []
    output = f"{process.stdout}{process.stderr}".strip()
    return [f"{what} failed with status {process.returncode}:\n{output}"]


def python_lane(root: Path, manifest: Manifest, snippets: list[Snippet]) -> list[str]:
    """Pyright-strict every Python snippet, then execute the ones that are programs."""
    pyright = shutil.which("pyright")
    if pyright is None:
        return ["pyright is not on PATH; run this lane through the project's uv environment"]
    errors: list[str] = []
    workdir = Path(tempfile.mkdtemp(prefix="opportune-doc-snippets-"))
    try:
        for snippet in snippets:
            source = compose(root, manifest, snippet)
            (workdir / f"{snippet.id}.py").write_text(source, encoding="utf-8")
        config = workdir / "pyrightconfig.json"
        config.write_text(
            json.dumps(
                {
                    "typeCheckingMode": "strict",
                    "pythonVersion": "3.12",
                    "reportMissingTypeStubs": True,
                    "extraPaths": [str(root / "crates/opportune-py/python")],
                    "include": ["./*.py"],
                },
                indent=2,
            ),
            encoding="utf-8",
        )
        check = run([pyright, "-p", str(config)], root)
        errors.extend(report(check, "Pyright over the Python snippets"))
        if errors:
            return errors
        for snippet in snippets:
            if snippet.mode != "run":
                continue
            source = compose(root, manifest, snippet)
            namespace: dict[str, object] = {"__name__": f"doc_snippet_{snippet.id}"}
            try:
                exec(compile(source, f"{snippet.page}:{snippet.line}", "exec"), namespace)
            except SystemExit as ended:
                # The lane executes in its own process, so an exit the block asked for
                # would stop the blocks after it unchecked and hand the gate a zero.
                errors.append(
                    f"{snippet.page}:{snippet.line}: `{snippet.id}` ended the gate with "
                    f"`SystemExit({ended.code!r})`; a snippet runs inside the gate, so "
                    "it has to return rather than stop the process"
                )
            except Exception as error:  # noqa: BLE001 - any failure is the finding to report
                errors.append(
                    f"{snippet.page}:{snippet.line}: `{snippet.id}` raised "
                    f"{type(error).__name__}: {error}"
                )
    finally:
        shutil.rmtree(workdir, ignore_errors=True)
    return errors


def typescript_lane(root: Path, manifest: Manifest, snippets: list[Snippet]) -> list[str]:
    """Type-check every TypeScript snippet strictly, then execute the ones that are programs."""
    node = shutil.which("node")
    if node is None:
        return ["node is not on PATH"]
    package = root / manifest.typescript_package
    compiler = package / "node_modules/typescript/bin/tsc"
    if not compiler.is_file():
        return [f"{compiler} is missing; run npm ci in {manifest.typescript_package}"]
    errors: list[str] = []
    scratch = Path(tempfile.mkdtemp(dir=package, prefix=".doc-snippets-"))
    try:
        sources = scratch / "snippets"
        sources.mkdir()
        for snippet in snippets:
            source = compose(root, manifest, snippet)
            (sources / f"{snippet.id}.mts").write_text(source, encoding="utf-8")
        # `extends` brings the package's strict flags and its `opportune` alias, so the
        # snippets type-check against `src/` exactly as the package's own suite does.
        # The temporary `rootDir` is the package, because the aliased sources are part
        # of the program too; only the snippets are run, from the emitted tree.
        config = scratch / "tsconfig.json"
        config.write_text(
            json.dumps(
                {
                    "extends": "../tsconfig.check.json",
                    "compilerOptions": {
                        "noEmit": False,
                        "noEmitOnError": True,
                        "rootDir": "..",
                        "outDir": "./run",
                    },
                    "include": ["snippets"],
                },
                indent=2,
            ),
            encoding="utf-8",
        )
        check = run([node, str(compiler), "-p", str(config)], package)
        errors.extend(report(check, "tsc over the TypeScript snippets"))
        if errors:
            return errors
        for snippet in snippets:
            if snippet.mode != "run":
                continue
            matches = list((scratch / "run").rglob(f"{snippet.id}.mjs"))
            if len(matches) != 1:
                errors.append(
                    f"{snippet.page}:{snippet.line}: `{snippet.id}` emitted "
                    f"{len(matches)} modules, not one"
                )
                continue
            # The block runs behind a wrapper that reports its own completion, so a
            # block that ends the process with a success status is a finding rather
            # than a silent pass that leaves every later block unchecked.
            marker = f"__doc_snippet_finished__{snippet.id}__"
            wrapper = scratch / f"{snippet.id}.run.mjs"
            wrapper.write_text(
                f"await import({json.dumps(matches[0].as_uri())});\n"
                f"process.stdout.write({json.dumps(marker)});\n",
                encoding="utf-8",
            )
            finished = run([node, str(wrapper)], package)
            if finished.returncode == 0 and marker not in finished.stdout:
                errors.append(
                    f"{snippet.page}:{snippet.line}: `{snippet.id}` ended the process "
                    "before the gate could finish running it; a snippet runs inside "
                    "the gate, so it has to return rather than stop the process"
                )
                continue
            errors.extend(report(finished, f"`{snippet.id}`"))
    finally:
        shutil.rmtree(scratch, ignore_errors=True)
    return errors


def fail(errors: list[str]) -> int:
    """Print every finding and return the status the gate exits with."""
    for error in errors:
        print(f"doc-snippets: {error}", file=sys.stderr)
    return 1 if errors else 0


def main(argv: list[str]) -> int:
    """The gate's entry point: classify, then whichever lanes were asked for."""
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=Path, default=ROOT, help="the repository root to read")
    parser.add_argument("--python", action="store_true", help="run the Python lane")
    parser.add_argument("--typescript", action="store_true", help="run the TypeScript lane")
    arguments = parser.parse_args(argv)
    root: Path = arguments.root
    manifest = load_manifest(MANIFEST)
    snippets, errors = classify(root, manifest)
    if errors:
        return fail(errors)
    print(
        f"doc-snippets: {len(snippets)} classified fences across "
        f"{len(documentation_pages(root))} pages"
    )
    if arguments.python:
        python = [snippet for snippet in snippets if snippet.language == "python"]
        errors.extend(python_lane(root, manifest, python))
    if arguments.typescript:
        typescript = [snippet for snippet in snippets if snippet.language == "ts"]
        errors.extend(typescript_lane(root, manifest, typescript))
    if errors:
        return fail(errors)
    print("doc-snippets: every checked snippet passed")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
