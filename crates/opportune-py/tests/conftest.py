"""Fixtures shared by the test suite."""

import json
from collections.abc import Callable
from pathlib import Path
from typing import Any

import pytest

FIXTURES = Path(__file__).parent / "fixtures"
ADAPTER_FIXTURES = Path(__file__).parents[2] / "opportune-adapters" / "tests" / "fixtures"


@pytest.fixture(scope="session")
def load_json_fixture() -> Callable[[str], Any]:
    """Return a loader for a JSON document under `tests/fixtures`."""

    def load(name: str) -> Any:
        return json.loads((FIXTURES / name).read_text(encoding="utf-8"))

    return load


@pytest.fixture(scope="session")
def adapter_payload() -> Callable[[str], str]:
    """Return a loader for a recorded provider payload, as its JSON text.

    The payloads are the ones the Rust adapter suite records under
    `crates/opportune-adapters/tests/fixtures`, so the Python wrappers are tested
    against the same realistic responses.
    """

    def load(name: str) -> str:
        return (ADAPTER_FIXTURES / name).read_text(encoding="utf-8")

    return load
