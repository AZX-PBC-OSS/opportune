"""Cross-language parity: every committed Rust golden, answered by Python.

The corpus under `crates/opportune-testkit/goldens` is written by the Rust
testkit, and each file stores its answers — `response`, `slot_check`,
`required_ranges` — as the exact text `opportune_json::to_json` wrote. This suite
replays every file twice:

* through the native JSON engine, where the answer must equal that committed text
  byte for byte, so a Python-side re-encoding cannot hide a difference; and
* through the typed Pydantic surface, where the answer must equal the golden's own
  model and re-serialise to the golden's own values, so the generated models read
  and write the wire format the Rust engine answers with.

The typed path compares values rather than bytes, and cannot do otherwise: the Rust
serializer writes fields in declaration order (`offers` first in a response) while
the generated models write them in the schema's alphabetical order (`dates` first),
so the two dumps differ in key order over identical values. Byte equality is the
native path's claim, and it holds for every golden.

A golden is the Rust engine's own answer, so this is parity against Rust rather
than Python against itself.
"""

# The native engine is what the public `opportune.Engine` wraps; this suite
# compares the two, so reaching for the private class is the subject rather than a
# leak.
# pyright: reportPrivateUsage=false

import json
from pathlib import Path
from typing import Any

import pytest

import opportune
from opportune import (
    Diagnostics,
    Proposal,
    RangesQuery,
    Request,
    RequiredRanges,
    Response,
    SlotCheck,
    _opportune,
)
from opportune.engine import Engine

GOLDENS = sorted((Path(__file__).parents[2] / "opportune-testkit" / "goldens").glob("*.json"))


def _load(path: Path) -> dict[str, Any]:
    document: dict[str, Any] = json.loads(path.read_text(encoding="utf-8"))
    return document


GOLDEN = [(path, _load(path)) for path in GOLDENS]

WITH_SLOT_CHECK = [(path, golden) for path, golden in GOLDEN if "slot_check" in golden]
WITH_REQUIRED_RANGES = [(path, golden) for path, golden in GOLDEN if "required_ranges" in golden]

USE_CASES = {f"U{number}" for number in range(1, 23)}
RULE_KINDS = {"window", "blackout", "clearance", "lead_time", "status_policy", "rest", "cap"}


def _native() -> _opportune._Engine:
    return _opportune._Engine()


def _for_current_engine(golden_json: str) -> str:
    """Substitute the current package version for the golden's recorded `meta.engine`.

    Keeps exact byte-for-byte parity on every field while preventing version bumps
    from invalidating the committed corpus.
    """
    recorded = json.loads(golden_json)["meta"]["engine"]
    assert isinstance(recorded, str), f"missing string meta.engine in golden: {recorded!r}"
    needle = f'"engine":"{recorded}"'
    assert golden_json.count(needle) == 1, f"expected one {needle} in golden"
    return golden_json.replace(needle, f'"engine":"{opportune.__version__}"', 1)


@pytest.mark.parametrize(("path", "golden"), GOLDEN, ids=[path.stem for path in GOLDENS])
def test_a_golden_response_is_answered_byte_for_byte_and_by_the_typed_models(
    path: Path, golden: dict[str, Any]
) -> None:
    request_text = json.dumps(golden["request"])
    expected_json = _for_current_engine(golden["response"])

    native_answer = _native().find_availability_json(request_text)
    assert json.loads(native_answer)["meta"]["engine"] == opportune.__version__, (
        f"{path.name}: native meta.engine must match opportune.__version__"
    )
    assert native_answer == expected_json, f"{path.name}: the native answer differs"

    expected = Response.model_validate_json(expected_json)
    typed_answer = Engine().find_availability(Request.model_validate_json(request_text))
    assert typed_answer == expected, f"{path.name}: the typed answer differs"
    # Against the golden document, not against a second parse of it: the model
    # equality above already implies two dumps of one value agree, so only the
    # golden's own values can show the models no longer write the wire format.
    assert json.loads(typed_answer.model_dump_json(by_alias=True, exclude_none=True)) == json.loads(
        expected_json
    ), f"{path.name}: the typed answer does not re-serialise to the golden's values"


@pytest.mark.parametrize(
    ("path", "golden"), WITH_SLOT_CHECK, ids=[path.stem for path, _ in WITH_SLOT_CHECK]
)
def test_a_golden_slot_check_is_answered_byte_for_byte_and_by_the_typed_models(
    path: Path, golden: dict[str, Any]
) -> None:
    request_text = json.dumps(golden["request"])
    proposal_text = json.dumps(golden["proposal"])
    expected_json = _for_current_engine(golden["slot_check"])

    native_answer = _native().check_slot_json(request_text, proposal_text)
    assert json.loads(native_answer)["meta"]["engine"] == opportune.__version__, (
        f"{path.name}: native slot_check meta.engine must match opportune.__version__"
    )
    assert native_answer == expected_json, f"{path.name}: the native check differs"

    request = Request.model_validate_json(request_text)
    expected = SlotCheck.model_validate_json(expected_json)
    typed_answer = Engine().check_slot(request, Proposal.model_validate(golden["proposal"]))
    assert typed_answer == expected, f"{path.name}: the typed check differs"
    assert json.loads(typed_answer.model_dump_json(by_alias=True, exclude_none=True)) == json.loads(
        expected_json
    ), f"{path.name}: the typed check does not re-serialise to the golden's values"


@pytest.mark.parametrize(
    ("path", "golden"),
    WITH_REQUIRED_RANGES,
    ids=[path.stem for path, _ in WITH_REQUIRED_RANGES],
)
def test_golden_required_ranges_are_answered_byte_for_byte_and_by_the_typed_models(
    path: Path, golden: dict[str, Any]
) -> None:
    query_text = json.dumps(golden["ranges_query"])

    native_answer = _native().required_ranges_json(query_text)
    assert native_answer == golden["required_ranges"], f"{path.name}: the native ranges differ"

    expected = RequiredRanges.model_validate_json(golden["required_ranges"])
    typed_answer = Engine().required_ranges(RangesQuery.model_validate(golden["ranges_query"]))
    assert typed_answer == expected, f"{path.name}: the typed ranges differ"
    # An absent optional field is omitted rather than written as `null` — the wire
    # writes no `occurrences` when no cap applies, matching every other optional
    # response field — so the re-serialisation asks for the same spelling.
    assert json.loads(typed_answer.model_dump_json(by_alias=True, exclude_none=True)) == json.loads(
        golden["required_ranges"]
    ), f"{path.name}: the typed ranges do not re-serialise to the golden's values"


def test_the_corpus_covers_every_phase_a_use_case_and_rule_kind() -> None:
    """A shrunken corpus must fail here rather than pass with less to verify."""
    use_cases = {golden["use_case"] for _, golden in GOLDEN}

    assert USE_CASES | RULE_KINDS <= use_cases, (
        f"the corpus no longer covers {sorted((USE_CASES | RULE_KINDS) - use_cases)}"
    )
    assert WITH_SLOT_CHECK, "no golden carries a proposal to confirm"
    assert WITH_REQUIRED_RANGES, "no golden carries a required-ranges query"


def test_each_fast_path_method_answers_what_its_typed_method_answers() -> None:
    """The four `*_json` calls are the typed calls with the answer left as its text.

    Each one hands its own document to the native layer, so a wrapper that paired
    the wrong argument — a proposal where the request goes, say — would answer a
    document the engine wrote for other input and nothing else in the suite would
    see it. One case carrying a proposal pins the first three, and the first case
    carrying a ranges query pins the fourth.
    """
    path, golden = next((path, golden) for path, golden in GOLDEN if "slot_check" in golden)
    range_path, range_golden = next(
        (path, golden) for path, golden in GOLDEN if "required_ranges" in golden
    )
    engine = Engine()
    request = Request.model_validate(golden["request"])
    proposal = Proposal.model_validate(golden["proposal"])
    query = RangesQuery.model_validate(range_golden["ranges_query"])

    assert Response.model_validate_json(engine.find_availability_json(request)) == (
        engine.find_availability(request)
    ), path.name
    assert SlotCheck.model_validate_json(engine.check_slot_json(request, proposal)) == (
        engine.check_slot(request, proposal)
    ), path.name
    assert Diagnostics.model_validate_json(engine.validate_json(request)) == engine.validate(
        request
    ), path.name
    assert RequiredRanges.model_validate_json(engine.required_ranges_json(query)) == (
        engine.required_ranges(query)
    ), range_path.name
