"""The installed distribution and the files its consumers rely on."""

import importlib
import importlib.metadata
from collections.abc import Callable
from pathlib import Path
from typing import Any

import opportune


def test_metadata_declares_the_distribution_and_python_floor(
    load_json_fixture: Callable[[str], Any],
) -> None:
    expected = load_json_fixture("distribution.json")
    metadata = importlib.metadata.metadata(expected["distribution"])
    assert metadata["Name"] == expected["distribution"]
    assert metadata["Requires-Python"] == expected["requires_python"]


def test_the_wheel_declares_the_runtime_dependency_its_models_need() -> None:
    """`_models.py` ships in the wheel and imports Pydantic, so it is a runtime need."""
    requirements = importlib.metadata.requires("opportune") or []
    runtime = [requirement for requirement in requirements if "extra ==" not in requirement]

    assert any(requirement.startswith("pydantic") for requirement in runtime), runtime


def test_dual_licence_metadata_and_both_licence_texts_ship(
    load_json_fixture: Callable[[str], Any],
) -> None:
    """The distribution declares both licences and carries the text of each."""
    expected = load_json_fixture("distribution.json")
    distribution = importlib.metadata.distribution(expected["distribution"])

    assert distribution.metadata["License-Expression"] == expected["license_expression"]
    declared = distribution.metadata.get_all("License-File") or []
    assert sorted(declared) == sorted(expected["license_files"])

    packaged = {path.name: path for path in distribution.files or []}
    for name in expected["license_files"]:
        assert name in packaged, f"{name} is declared but not packaged"
        assert Path(packaged[name].locate()).is_file()


def test_package_version_matches_the_installed_distribution() -> None:
    """The crate version and the Python distribution version are bumped together."""
    assert opportune.__version__ == importlib.metadata.version("opportune")


def test_compiled_extension_is_importable_from_the_package(
    load_json_fixture: Callable[[str], Any],
) -> None:
    extension = importlib.import_module(load_json_fixture("distribution.json")["module"])
    assert extension.__version__ == opportune.__version__


def test_typing_marker_and_stub_ship_with_the_package(
    load_json_fixture: Callable[[str], Any],
) -> None:
    expected = load_json_fixture("distribution.json")
    package_dir = Path(opportune.__file__).parent
    assert (package_dir / expected["type_marker"]).is_file()
    assert (package_dir / expected["stub"]).is_file()
