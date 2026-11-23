"""The codegen post-pass's own bookkeeping, checked without the generator.

`bin/postprocess_models.py` finds the end of a generated field declaration by counting
the brackets the declaration opens, with quoted prose ignored, and holds every wire
integer to the bounds the schema publishes. Both are what makes the pass fail loudly
rather than ship a bound attached to the wrong field or a range that quietly vanished
between the schema and the model. `bin/` is a script directory rather than an
importable package, so the module is loaded from its path: the same file `make codegen`
and `make check-codegen` run.
"""

import importlib.util
import json
import sys
from collections.abc import Callable, Mapping
from pathlib import Path
from types import ModuleType
from typing import Any, Protocol, cast

import pytest

POSTPROCESS = Path(__file__).parents[1] / "bin" / "postprocess_models.py"
"""The post-pass `make codegen` runs over the generated models."""


class _Integer(Protocol):
    """One wire integer the post-pass read, as its `IntegerField` states it."""

    model: str
    field: str
    format: str
    minimum: int
    maximum: int


def _postprocess() -> ModuleType:
    """The post-pass loaded as a module, cached for the test run."""
    name = "opportune_postprocess_models"
    loaded = sys.modules.get(name)
    if loaded is not None:
        return loaded
    spec = importlib.util.spec_from_file_location(name, POSTPROCESS)
    assert spec is not None, "the post-pass is a file the spec loader reads"
    assert spec.loader is not None, "the post-pass has a loader"
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


def _unbalanced() -> Callable[[str], int]:
    """The post-pass's bracket counter."""
    return cast("Callable[[str], int]", _postprocess()._unbalanced)


def _statement() -> Callable[[list[str], int], int]:
    """The post-pass's end-of-statement finder."""
    return cast("Callable[[list[str], int], int]", _postprocess()._statement)


def _integer_fields() -> Callable[[Path], tuple[_Integer, ...]]:
    """The post-pass's reader of the schema's wire integers."""
    return cast("Callable[[Path], tuple[_Integer, ...]]", _postprocess().integer_fields)


def _verify_integer_bounds() -> Callable[[list[str], tuple[_Integer, ...]], None]:
    """The post-pass's check that the generated fields kept their bounds."""
    return cast(
        "Callable[[list[str], tuple[_Integer, ...]], None]",
        _postprocess()._verify_integer_bounds,
    )


def _open_models() -> Callable[[Path], dict[str, bool]]:
    """The post-pass's reader of which definitions the schema leaves open."""
    return cast("Callable[[Path], dict[str, bool]]", _postprocess()._open_models)


def _state_extra() -> Callable[[list[str], Mapping[str, bool]], None]:
    """The post-pass's writer of each class's `extra` policy."""
    return cast(
        "Callable[[list[str], Mapping[str, bool]], None]",
        _postprocess()._state_extra,
    )


def _schema(tmp_path: Path, properties: dict[str, Any]) -> Path:
    """A one-definition schema at `tmp_path`, titled as `Limits` is generated."""
    path = tmp_path / "schema.json"
    path.write_text(
        json.dumps({"$defs": {"WireLimits": {"title": "Limits", "properties": properties}}}),
        encoding="utf-8",
    )
    return path


def _described(
    *, model: str, field: str, format: str, minimum: int, maximum: int
) -> tuple[str, str, str, int, int]:
    """The `(model, field, format, minimum, maximum)` a read integer states."""
    return (model, field, format, minimum, maximum)


def _stated(integer: "_Integer") -> tuple[str, str, str, int, int]:
    """`integer` as the five values a test compares."""
    return (integer.model, integer.field, integer.format, integer.minimum, integer.maximum)


def test_a_bracket_count_includes_the_braces_of_a_mapping() -> None:
    """A mapping default's braces are part of the statement that holds it.

    Counting only parentheses and brackets would treat a line that closes the
    annotation and opens a mapping — ``] = {`` — as the end of the field, so a default
    spelled on the following lines would fall outside the text the bound rewrites read.
    """
    unbalanced = _unbalanced()

    assert unbalanced("[({})]") == 0
    assert unbalanced("[{}") == 1, "an unclosed mapping keeps the statement open"
    assert unbalanced("}") == -1
    assert unbalanced('Field(description="a } and a {")') == 0, (
        "a bracket inside the description literal is prose, not the statement's shape"
    )


def test_a_field_statement_spans_a_multi_line_mapping_default() -> None:
    """The statement ends after the mapping, not at the annotation's closing bracket.

    The generator writes a default as the Python value it is, and a mapping one can be
    laid out over several lines — the shape a schema with a constrained map value
    already produces a helper for. The last line of the field is the mapping's closing
    brace, which is where the text a check reads ends.
    """
    lines = [
        "    weights: Annotated[",
        "        dict[str, int],",
        "        Field(description='The weight each participant carries.'),",
        "    ] = {",
        "        'ana': 1,",
        "    }",
        "    next_field: int",
    ]

    assert _statement()(lines, 0) == 5


def test_every_wire_integer_is_read_with_the_bounds_the_schema_states(tmp_path: Path) -> None:
    """The two positions the generator writes a wire integer in are both read.

    A field's own node carries the range, and so does an `Option<u64>`: the exported
    schema states one node whose `type` admits both `integer` and `null`. A map's
    values state theirs one level in, under the object arm of the nullable map.
    """
    schema = tmp_path / "schema.json"
    schema.write_text(
        json.dumps(
            {
                "$defs": {
                    "WireLimits": {
                        "title": "Limits",
                        "properties": {
                            "max_participants": {
                                "type": ["integer", "null"],
                                "format": "uint64",
                                "minimum": 0,
                                "maximum": 18446744073709551615,
                            }
                        },
                    },
                    "WireRequest": {
                        "title": "Request",
                        "properties": {
                            "weights": {
                                "anyOf": [
                                    {
                                        "type": "object",
                                        "additionalProperties": {
                                            "type": "integer",
                                            "format": "int32",
                                            "minimum": -2147483648,
                                            "maximum": 2147483647,
                                        },
                                    },
                                    {"type": "null"},
                                ]
                            }
                        },
                    },
                }
            }
        ),
        encoding="utf-8",
    )

    assert [_stated(integer) for integer in _integer_fields()(schema)] == [
        _described(
            model="Limits",
            field="max_participants",
            format="uint64",
            minimum=0,
            maximum=18446744073709551615,
        ),
        _described(
            model="Request",
            field="weights",
            format="int32",
            minimum=-2147483648,
            maximum=2147483647,
        ),
    ]


def test_a_wire_integer_without_its_published_maximum_fails_the_pass(tmp_path: Path) -> None:
    """A format whose upper bound the schema stopped publishing is a hard failure.

    The bounds are the schema's to state so the generator can write them; one that
    dropped a `maximum` would generate a model that admits an integer serde refuses,
    and the pass is the only thing that would notice.
    """
    schema = _schema(
        tmp_path,
        {
            "max_participants": {
                "type": "integer",
                "format": "uint64",
                "minimum": 0,
            }
        },
    )

    with pytest.raises(SystemExit, match="without its bounds"):
        _integer_fields()(schema)


def test_a_wire_integer_in_a_position_the_pass_does_not_read_fails(tmp_path: Path) -> None:
    """An integer nested somewhere no class and field can be named from fails.

    Such a position cannot be resolved to a generated field, so checking it would be a
    guess: the pass refuses it instead of reporting success over an unchecked range.
    """
    schema = _schema(
        tmp_path,
        {
            "offers": {
                "type": "array",
                "items": {
                    "type": "integer",
                    "format": "uint32",
                    "minimum": 0,
                    "maximum": 4294967295,
                },
            }
        },
    )

    with pytest.raises(SystemExit, match="position this pass does not read"):
        _integer_fields()(schema)


def test_a_generated_field_that_lost_its_bound_fails_the_pass(tmp_path: Path) -> None:
    """The check reads the generated text, so a bound dropped there is caught.

    The generator writes `ge`/`le` from the schema, and this pass adds nothing to the
    text; the assertion is what keeps a constraint from disappearing between the two
    documents. The same field with its `le` stated passes, so the failure is the bound
    and not the walk.
    """
    schema = _schema(
        tmp_path,
        {
            "max_participants": {
                "type": ["integer", "null"],
                "format": "uint64",
                "minimum": 0,
                "maximum": 18446744073709551615,
            }
        },
    )
    integers = _integer_fields()(schema)
    verify = _verify_integer_bounds()

    lines = [
        "class Limits(OpportuneModel):",
        "    model_config = ConfigDict(",
        "        extra='forbid',",
        "        populate_by_name=True,",
        "    )",
        "    max_participants: Annotated[",
        "        int | None,",
        "        Field(",
        "            description='Maximum number of participants in one request.',",
        "            ge=0,",
        "        ),",
        "    ] = None",
        "",
        "class Next(OpportuneModel):",
        "    pass",
    ]

    with pytest.raises(SystemExit, match="missing the uint64 bounds"):
        verify(lines, integers)

    lines.insert(10, "            le=18446744073709551615,")
    verify(lines, integers)


def test_every_titled_object_schema_is_read_with_its_extra_policy(tmp_path: Path) -> None:
    """The pass reads `additionalProperties`, including a union's inline arms.

    The exporter closes every request and leaves every response open, and the union
    arms it inlines carry their own answer. Reading the keyword rather than naming
    the open definitions is what keeps a model from disagreeing with the wire when
    a definition moves from one side to the other; a map's `additionalProperties`
    alone names no class and is skipped.
    """
    schema = tmp_path / "schema.json"
    schema.write_text(
        json.dumps(
            {
                "$defs": {
                    "WireResponse": {
                        "title": "Response",
                        "type": "object",
                        "properties": {"offers": {"type": "array"}},
                    },
                    "WireRequest": {
                        "title": "Request",
                        "type": "object",
                        "additionalProperties": False,
                        "properties": {"now": {"type": "string"}},
                    },
                    "WireOutcome": {
                        "title": "Outcome",
                        "oneOf": [
                            {
                                "title": "AvailableOutcome",
                                "type": "object",
                                "properties": {"status": {"const": "available"}},
                            }
                        ],
                    },
                    "WireWeights": {
                        "type": "object",
                        "additionalProperties": {"type": "integer", "format": "int32"},
                    },
                }
            }
        ),
        encoding="utf-8",
    )

    assert _open_models()(schema) == {
        "Request": False,
        "Response": True,
        "AvailableOutcome": True,
    }


def test_each_class_states_its_extra_policy_and_a_wrong_one_is_corrected() -> None:
    """An open model states `ignore` outright, rather than inheriting a default.

    The generator writes nothing for a definition that is open, so without this
    rewrite the open models would rely on Pydantic's default — and on the base
    model's config, which is that default until it is not. A class the generator
    wrote `forbid` for that its schema leaves open is corrected too.
    """
    lines = [
        "class Response(OpportuneModel):",
        "    model_config = ConfigDict(",
        "        populate_by_name=True,",
        "    )",
        "    dates: list[str]",
        "",
        "class Request(OpportuneModel):",
        "    model_config = ConfigDict(",
        '        extra="forbid",',
        "        populate_by_name=True,",
        "    )",
        "    now: str",
        "",
        "class Meta(OpportuneModel):",
        "    model_config = ConfigDict(",
        "        extra='forbid',",
        "        populate_by_name=True,",
        "    )",
        "    engine: str",
    ]

    _state_extra()(lines, {"Response": True, "Request": False, "Meta": True})

    text = "\n".join(lines)
    assert (
        "class Response(OpportuneModel):\n"
        "    model_config = ConfigDict(\n"
        "        extra='ignore',\n"
        "        populate_by_name=True,\n"
        "    )"
    ) in text, "an open model states `ignore` rather than relying on the default"
    assert (
        "class Request(OpportuneModel):\n"
        "    model_config = ConfigDict(\n"
        "        extra='forbid',\n"
        "        populate_by_name=True,\n"
        "    )"
    ) in text, "a closed model keeps `forbid`, whichever quote the generator wrote"
    assert (
        "class Meta(OpportuneModel):\n"
        "    model_config = ConfigDict(\n"
        "        extra='ignore',\n"
        "        populate_by_name=True,\n"
        "    )"
    ) in text, "a `forbid` the schema's openness contradicts is corrected"


def test_a_class_the_schema_does_not_title_fails_the_pass() -> None:
    """A generated class no definition accounts for is a hard failure.

    The pass derives the policy from the schema, so a class it cannot match has no
    policy to state. Guessing one — or silently skipping the class — would ship a
    model nobody checked against the wire.
    """
    lines = [
        "class Response(OpportuneModel):",
        "    model_config = ConfigDict(",
        "        populate_by_name=True,",
        "    )",
        "    dates: list[str]",
        "",
        "class Untitled(OpportuneModel):",
        "    model_config = ConfigDict(",
        "        populate_by_name=True,",
        "    )",
        "    value: str",
    ]

    with pytest.raises(SystemExit, match="disagree"):
        _state_extra()(lines, {"Response": True})
