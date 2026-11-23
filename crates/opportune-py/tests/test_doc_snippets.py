"""The documentation snippet gate, and the red-team case that proves it bites.

The gate extracts every classified `python` and `ts` fence from `README.md` and
the pages under `docs/` and checks it against the package a reader would install;
see `crates/opportune-py/bin/doc_snippets.py`. A gate that has stopped failing
is indistinguishable from a corpus that has stopped being stale, so the tests
here run the real driver over a copy of the documentation with one snippet made
stale or one block written a way the scanner must not miss, and assert a non-zero
exit that names the intended snippet. The copy links the repository's `crates/`
tree so the recorded fixtures are the real ones — the test is POSIX-only for that
reason, as the rest of the workspace's shell gates are.
"""

from __future__ import annotations

import shutil
import subprocess
import sys
from pathlib import Path

import pytest

#: The repository root: this file lives at `crates/opportune-py/tests/`.
ROOT = Path(__file__).resolve().parents[3]
DRIVER = ROOT / "crates/opportune-py/bin/doc_snippets.py"

#: The stale assertion every mutation swaps in: type-correct, wrong value, so only
#: the execution half of the lane can catch it.
STALE_ASSERT = (
    'assert page["2027-06-14"][0] == "2027-06-14T11:00Z"',
    'assert page["2027-06-14"][0] == "2027-06-14T12:00Z"',
)


def stale_checkout(tmp_path: Path, page: str, *replacements: tuple[str, str]) -> Path:
    """A checkout whose `page` applies every `(old, new)` replacement in order."""
    root = tmp_path / "checkout"
    root.mkdir()
    shutil.copytree(ROOT / "docs", root / "docs")
    shutil.copy2(ROOT / "README.md", root / "README.md")
    (root / "crates").symlink_to(ROOT / "crates", target_is_directory=True)
    target = root / page
    text = target.read_text(encoding="utf-8")
    for old, new in replacements:
        assert old in text, f"{page} no longer carries {old!r}, so this test would prove nothing"
        text = text.replace(old, new, 1)
    target.write_text(text, encoding="utf-8")
    return root


def gate(root: Path, *arguments: str) -> subprocess.CompletedProcess[str]:
    """Run the real driver over `root` with `arguments`."""
    return subprocess.run(
        [sys.executable, str(DRIVER), "--root", str(root), *arguments],
        text=True,
        capture_output=True,
        check=False,
    )


def indent_block(page: Path, identifier: str, spaces: int) -> None:
    """Indent one classified block — identifier, fence, body and closer — by `spaces`.

    Written the way a page nests a block: the fence and its body share the
    indentation, which a reader strips relative to the opener.
    """
    lines = page.read_text(encoding="utf-8").split("\n")
    marker = lines.index(f"<!-- snippet: {identifier} -->")
    closer = next(
        index for index in range(marker + 2, len(lines)) if lines[index].strip() in {"```", "````"}
    )
    pad = " " * spaces
    for index in range(marker, closer + 1):
        lines[index] = pad + lines[index]
    page.write_text("\n".join(lines), encoding="utf-8")


def test_the_committed_documentation_is_classified(tmp_path: Path) -> None:
    """Every gated fence on the committed pages is named by the manifest."""
    result = gate(ROOT)
    assert result.returncode == 0, result.stderr
    assert "classified fences" in result.stdout


def test_a_stale_python_snippet_fails_the_python_lane(tmp_path: Path) -> None:
    """A snippet whose assertion no longer matches the engine exits non-zero.

    The mutation is type-correct — a wrong expected count — so nothing but the
    execution half of the lane can catch it.
    """
    root = stale_checkout(
        tmp_path,
        "docs/recipes.md",
        STALE_ASSERT,
    )
    result = gate(root, "--python")
    output = f"{result.stdout}{result.stderr}"
    assert "classified fences" in result.stdout, output
    assert result.returncode != 0, "the Python lane accepted a stale snippet"
    # The execution half failed on the intended snippet, not classification or Pyright.
    assert "`personal-link-python` raised AssertionError" in output, output


def test_the_provider_fallback_keeps_the_mailbox_scope(tmp_path: Path) -> None:
    """The custom-zone fallback must not turn Ana's rule into a rule for every host."""
    root = stale_checkout(
        tmp_path,
        "docs/recipes.md",
        (
            'hours = graph.working_hours_to_rule(working_hours_payload, who={"ids": ["ana"]})',
            "hours = graph.working_hours_to_rule(\n"
            "    {\n"
            '        "daysOfWeek": ["monday"],\n'
            '        "startTime": "09:00:00.0000000",\n'
            '        "endTime": "17:00:00.0000000",\n'
            '        "timeZone": {\n'
            '            "@odata.type": "#microsoft.graph.customTimeZone",\n'
            '            "bias": 60,\n'
            "        },\n"
            "    },\n"
            '    who={"ids": ["ana"]},\n'
            ")",
        ),
    )
    result = gate(root, "--python")
    output = f"{result.stdout}{result.stderr}"
    assert result.returncode == 0, output


def test_a_python_snippet_that_stops_type_checking_fails_the_python_lane(
    tmp_path: Path,
) -> None:
    """A type-only rot — a value the annotation rejects — exits non-zero.

    The snippet still runs, so only the Pyright half can catch this one; the case
    is what proves the lane does not merely execute the blocks.
    """
    root = stale_checkout(
        tmp_path,
        "docs/recipes.md",
        (
            "page: dict[str, list[str]] = {count.date: [] for count in response.dates}",
            "page: dict[str, list[str]] = {count.date: [] for count in response.dates}\n"
            '_stale: int = "not an integer"',
        ),
    )
    result = gate(root, "--python")
    output = f"{result.stdout}{result.stderr}"
    assert result.returncode != 0, "the Python lane accepted a snippet Pyright rejects"
    assert "Pyright over the Python snippets failed" in output, output


def test_a_fence_with_a_render_option_is_still_classified(tmp_path: Path) -> None:
    """A `title=` on the fence is a rendering option, not a different language.

    This is a classification property, so it does not need to pay for a full
    compiler and execution lane; the behavioral cases exercise the extracted body.
    """
    root = stale_checkout(
        tmp_path,
        "docs/recipes.md",
        (
            "<!-- snippet: personal-link-python -->\n```python\n",
            '<!-- snippet: personal-link-python -->\n```python title="One person, one meeting"\n',
        ),
    )
    result = gate(root)
    assert result.returncode == 0, result.stderr
    assert "classified fences" in result.stdout


def test_an_unclassified_tilde_fence_fails_classification(tmp_path: Path) -> None:
    """Markdown's other fenced-block spelling cannot bypass the gate.

    PyMdown renders a tilde fence exactly like a backtick fence, so a Python
    program behind one still needs a stable identifier and a manifest entry.
    """
    root = stale_checkout(tmp_path, "docs/recipes.md", ("", ""))
    (root / "docs/extra.md").write_text(
        "# A tilde-fenced page\n\n~~~python\nassert 1 == 2\n~~~\n",
        encoding="utf-8",
    )
    result = gate(root)
    output = f"{result.stdout}{result.stderr}"
    assert result.returncode != 0, "an unclassified Python block behind tildes passed"
    assert "extra.md" in output, output


def test_a_classified_tilde_fence_is_executed(tmp_path: Path) -> None:
    """A valid tilde-fenced program reaches the same execution lane as backticks."""
    root = stale_checkout(
        tmp_path,
        "docs/recipes.md",
        (
            "<!-- snippet: personal-link-python -->\n```python\n",
            "<!-- snippet: personal-link-python -->\n~~~python\n",
        ),
        (
            'assert len(page["2027-06-14"]) == 10\n```',
            'assert len(page["2027-06-14"]) == 10\n~~~',
        ),
        STALE_ASSERT,
    )
    result = gate(root, "--python")
    output = f"{result.stdout}{result.stderr}"
    assert result.returncode != 0, "the Python lane accepted a stale tilde-fenced snippet"
    assert "`personal-link-python` raised AssertionError" in output, output


def test_an_over_indented_closing_fence_does_not_truncate_a_snippet(
    tmp_path: Path,
) -> None:
    """A closing fence may use three spaces, not four, as CommonMark counts them.

    The four-space line is part of the program. Treating it as the closer would
    execute only the prefix and silently ignore the stale code that follows it.
    """
    root = stale_checkout(
        tmp_path,
        "docs/recipes.md",
        (
            'assert len(page["2027-06-14"]) == 10\n```',
            'assert len(page["2027-06-14"]) == 10\n    ```\nassert False\n```',
        ),
    )
    result = gate(root, "--python")
    output = f"{result.stdout}{result.stderr}"
    assert result.returncode != 0, "a closing fence indented four spaces truncated the snippet"
    assert "personal-link-python" in output, output


def test_an_indented_fence_is_still_checked(tmp_path: Path) -> None:
    """A fence indented up to three spaces is still a fence the page renders.

    The whole block is indented — identifier, fence, body and closer — the way a
    page shares the indentation of a nested block. The body only compiles once the
    opener's indentation is stripped from it, so this pins the dedent as well as
    the classification.
    """
    root = stale_checkout(tmp_path, "docs/recipes.md", STALE_ASSERT)
    indent_block(root / "docs/recipes.md", "personal-link-python", 3)
    result = gate(root, "--python")
    output = f"{result.stdout}{result.stderr}"
    assert result.returncode != 0, "the Python lane accepted a stale indented snippet"
    assert "`personal-link-python` raised AssertionError" in output, output


def test_a_python_fence_nested_inside_a_container_is_classified(tmp_path: Path) -> None:
    """A block indented inside a list item renders as a Python example, so it is gated.

    The classifier reads fences by column, not by Markdown container, so it cannot
    run a block nested deeper than a fence may be indented. It fails rather than
    reporting a corpus it never read; either an identifier and the container's
    support, or dedenting the block, is the author's answer.
    """
    root = stale_checkout(tmp_path, "docs/recipes.md", ("", ""))
    page = root / "docs/recipes.md"
    page.write_text(
        page.read_text(encoding="utf-8")
        + "\n\n1. A numbered step.\n\n"
        + "    ```python\n    assert 1 == 2\n    ```\n",
        encoding="utf-8",
    )
    result = gate(root)
    output = f"{result.stdout}{result.stderr}"
    assert result.returncode != 0, "an unclassified Python block nested in a list passed"
    assert "indented" in output, output


def test_a_python_block_on_a_new_page_is_classified(tmp_path: Path) -> None:
    """A page the manifest has not met yet is still read, so its blocks are gated.

    The page-level equivalent of an unclassified fence: a new `docs/*.md` page with
    a Python example would otherwise never be opened, and the gate would report a
    corpus it had not finished reading.
    """
    root = stale_checkout(tmp_path, "docs/recipes.md", ("", ""))
    (root / "docs/extra.md").write_text(
        "# A page nobody listed\n\n```python\nassert 1 == 2\n```\n",
        encoding="utf-8",
    )
    result = gate(root)
    output = f"{result.stdout}{result.stderr}"
    assert result.returncode != 0, "a Python block on an unlisted page passed"
    assert "extra.md" in output, output


@pytest.mark.parametrize("tag", ["python3", "py3"])
def test_every_spelling_of_the_python_tag_is_classified(tag: str, tmp_path: Path) -> None:
    """`python3` and `py3` are Python to the renderer, so they are Python here.

    The execution path is shared after classification, so these alias cases only
    need the fast classifier rather than one full compiler run per spelling.
    """
    root = stale_checkout(
        tmp_path,
        "docs/recipes.md",
        (
            "<!-- snippet: personal-link-python -->\n```python\n",
            f"<!-- snippet: personal-link-python -->\n```{tag}\n",
        ),
    )
    result = gate(root)
    assert result.returncode == 0, result.stderr
    assert "classified fences" in result.stdout


def test_a_python_snippet_cannot_end_the_gate(tmp_path: Path) -> None:
    """A run block that exits the process must not hand the gate a success.

    The lane executes blocks in its own process, so a block ending with
    `sys.exit(0)` — what a script-style example ends with — would stop every block
    after it unchecked while the driver returned zero.
    """
    root = stale_checkout(
        tmp_path,
        "docs/recipes.md",
        (
            'assert page["2027-06-14"][0] == "2027-06-14T11:00Z"',
            'assert page["2027-06-14"][0] == "2027-06-14T11:00Z"\nimport sys\nsys.exit(0)',
        ),
    )
    result = gate(root, "--python")
    output = f"{result.stdout}{result.stderr}"
    assert result.returncode != 0, "a snippet that ended the gate with zero passed"
    assert "`personal-link-python` ended the gate" in output, output
