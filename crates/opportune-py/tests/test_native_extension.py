"""The native extension's runtime surface, which its type stub only describes.

The stub is checked by Pyright and the module by its Rust tests, so this file
exists for the contract neither can see: that the compiled extension really
exports the names the stub declares — no more and no fewer — with the constants
and exception attributes it promises. An application's FastAPI exception handler
reads them, and a removed export is a runtime `AttributeError` that Pyright reports
as no error at the call site because the stub still declares it.
"""

# `_Engine` is private to the extension, which is deliberate: the public
# `opportune.Engine` wraps it. This file tests the native surface underneath that
# wrapper, so reaching for the private name is the subject rather than a leak.
# pyright: reportPrivateUsage=false

import ast
import json
from pathlib import Path
from typing import Any

import pytest

from opportune import _opportune

STUB = Path(__file__).parents[1] / "python" / "opportune" / "_opportune.pyi"
"""The type stub that describes the extension."""

REQUEST: dict[str, Any] = {
    "now": "2027-03-09T15:00Z",
    "goal": {
        "parts": [{"duration": "30m", "requirement": {"all": [{"person": "ana"}]}}],
        "search_window": ["2027-03-09T15:00Z", "2027-03-09T18:00Z"],
        "zone": "UTC",
    },
    "participants": [{"id": "ana", "zone": "UTC", "calendar": []}],
}


def test_the_engine_answers_a_request_and_reports_its_database_version() -> None:
    engine = _opportune._Engine()
    assert engine.tzdb_version

    response: dict[str, Any] = json.loads(engine.find_availability_json(json.dumps(REQUEST)))
    assert response["offers"][0]["events"][0]["start"] == "2027-03-09T15:00Z"
    assert response["meta"]["tzdb"] == engine.tzdb_version


def test_a_rejected_document_raises_the_exception_the_stub_declares() -> None:
    engine = _opportune._Engine()
    with pytest.raises(_opportune.OpportuneError) as raised:
        engine.find_availability_json("")

    error = raised.value
    assert error.code == "invalid_json"
    assert error.message
    assert error.path is None, "the whole document is at fault, not one of its fields"
    assert error.errors == []


def test_a_rejected_request_carries_its_findings_and_their_pointers() -> None:
    engine = _opportune._Engine()
    broken = json.loads(json.dumps(REQUEST))
    broken["participants"][0]["zone"] = "Mars/Olympus_Mons"

    with pytest.raises(_opportune.OpportuneError) as raised:
        engine.find_availability_json(json.dumps(broken))

    assert raised.value.code == "invalid_request"
    assert [finding["path"] for finding in raised.value.errors] == ["/participants/0/zone"]


def test_an_upgrade_raises_on_a_schema_version_this_build_does_not_read() -> None:
    rule = '{"id": "r_min", "kind": "lead_time", "params": { "min": "4h" }}'
    assert json.loads(_opportune.upgrade_rule_json(rule, 1))["id"] == "r_min"

    with pytest.raises(_opportune.OpportuneError) as raised:
        _opportune.upgrade_rule_json(rule, 99)
    assert raised.value.code == "unsupported_schema_version"


def test_the_marker_constants_the_adapters_share_are_exported() -> None:
    assert _opportune.MARKER_TAGS == "opportune.tags"
    assert _opportune.MARKER_EVENT_KEY == "opportune.event_key"
    assert _opportune.GRAPH_EXTENDED_PROPERTY_NAMESPACE_GUID
    assert _opportune.GRAPH_EXTENDED_PROPERTY_TAGS_ID.endswith("opportune.tags")
    assert _opportune.GRAPH_EXTENDED_PROPERTY_EVENT_KEY_ID.endswith("opportune.event_key")
    assert "singleValueExtendedProperties" in _opportune.GRAPH_EXTENDED_PROPERTY_EXPAND


def test_the_default_limits_document_is_the_engines_own_defaults() -> None:
    """The defaults `default_limits()` reads are the ones the engine applies."""
    document: dict[str, Any] = json.loads(_opportune.DEFAULT_LIMITS_JSON)

    assert document == {
        "max_participants": 500,
        "max_blocks_per_participant": 10_000,
        "max_search_window_days": 92,
        "max_candidate_starts": 100_000,
        "max_explain_limit": 10_000,
        "max_rules": 500,
        "max_occurrences": 50_000,
        "max_requirement_slots": 64,
        "max_duration_minutes": 10_080,
        "max_zone_periods_per_participant": 1_000,
    }
    assert list(document) == [
        "max_participants",
        "max_blocks_per_participant",
        "max_search_window_days",
        "max_candidate_starts",
        "max_explain_limit",
        "max_rules",
        "max_occurrences",
        "max_requirement_slots",
        "max_duration_minutes",
        "max_zone_periods_per_participant",
    ], "the document is written in the wire struct's declaration order, as every answer is"


def _stub_names() -> set[str]:
    """Everything the stub declares: classes, functions and annotated constants."""
    names: set[str] = set()
    for node in ast.parse(STUB.read_text(encoding="utf-8")).body:
        if isinstance(node, (ast.ClassDef, ast.FunctionDef, ast.AsyncFunctionDef)):
            names.add(node.name)
        elif isinstance(node, ast.AnnAssign) and isinstance(node.target, ast.Name):
            names.add(node.target.id)
        elif isinstance(node, ast.Assign):
            names.update(target.id for target in node.targets if isinstance(target, ast.Name))
    return {name for name in names if not name.startswith("__")}


def test_the_stub_declares_exactly_what_the_extension_exports() -> None:
    """A name in one and not the other is a runtime `AttributeError` or an unused export."""
    exported = {name for name in dir(_opportune) if not name.startswith("__")}
    declared = _stub_names()

    assert declared - exported == set(), "the stub declares names the module does not export"
    assert exported - declared == set(), "the module exports names the stub does not declare"
