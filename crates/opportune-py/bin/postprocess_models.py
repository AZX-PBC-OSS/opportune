"""The codegen post-pass: what the generated models cannot say for themselves.

``make codegen`` writes ``python/opportune/_models.py`` from the combined schema and
then runs this script over it; ``make check-codegen`` runs the same steps into a
temporary directory and compares the bytes. Six rewrites are made here, because a
JSON Schema cannot express them:

- ``Selector``, ``Term`` and ``LeadTimeMax`` are unions of generated models, so
  Pydantic reports a value none of them accepts once per arm. Each alias carries
  :class:`opportune._base.KeyedUnion`, which derives the forms from the arms and
  names them in one finding. Only the generated module can name the arms, so the
  metadata can only be attached here.
- ``Block.status`` is the document's one open string, so its model normalises a
  provider's unrecognised status to ``busy`` the way the engine reads it.
- Every class states its own ``extra`` policy, read from its schema definition's
  ``additionalProperties``: the exporter closes every request and input object and
  leaves every response and output object open, because serde reads the latter
  without ``deny_unknown_fields``. An open model states ``extra='ignore'`` so an
  older client parses a field a newer engine added, and a closed one keeps
  ``extra='forbid'``. Stating it on the class rather than inheriting Pydantic's
  default is what keeps a base-model change from silently re-closing every answer.
- A constrained map value (such as ``Request.weights``, whose ``int32`` schema
  values carry explicit ``minimum``/``maximum`` bounds) causes ``datamodel-codegen``
  to synthesise a ``TypeAliasType`` helper and import ``typing_extensions``. The
  post-pass inlines those helpers directly into the field annotations and drops the
  ``typing_extensions`` import, so no synthetic name reaches ``__all__``.
- ``__all__`` lists every model and type alias the module defines, so a check can
  prove the package root re-exports each one.
- The generator's unused ``type Model = Any`` alias is dropped (ruff then removes
  the ``Any`` import it leaves behind).

One pass only *checks* rather than rewrites: the schema exporter publishes the
``minimum``/``maximum`` every ``int32``, ``uint32`` and ``uint64`` implies, so the
generator writes each ``ge``/``le`` constraint itself. :func:`_verify_integer_bounds`
reads those bounds back off the schema and proves every generated field still states
them — an integer past the wire range would otherwise validate here and be refused by
serde after the document had already crossed the boundary.

Each rewrite fails loudly when the shape it edits is absent: a schema change that
renames an alias, its title, ``Block``, its model config, the base import, or moves a
wire integer into a position this pass does not read surfaces here rather than
shipping models that quietly lost the dispatch, the bound, or gained one somewhere
else.
"""

import json
import re
import sys
from collections.abc import Iterator, Mapping
from dataclasses import dataclass
from pathlib import Path
from typing import Any, cast

BASE_IMPORT = "from opportune._base import OpportuneModel"
BASE_IMPORT_WITH_DISPATCH = (
    "from opportune._base import KeyedUnion, OpportuneModel, fails_closed_to_busy"
)
PYDANTIC_IMPORT = "from pydantic import ConfigDict, Field"
PYDANTIC_IMPORT_WITH_VALIDATOR = "from pydantic import ConfigDict, Field, field_validator"

KEYED_UNIONS = {"Selector": "selector", "Term": "term", "LeadTimeMax": "lead time max"}
"""The generated alias whose forms a key names, and the label its findings use."""

BLOCK = "class Block(OpportuneModel):"
BLOCK_CONFIG = (
    "    model_config = ConfigDict(",
    "        extra='forbid',",
    "        populate_by_name=True,",
    "    )",
)
"""The whole generated `Block` config, in the generator's own spelling: the
status validator is added after it.

The post-pass runs before ruff normalises quotes, so these lines are what
datamodel-codegen writes, not what the committed module reads. The config is
matched as a sequence rather than by its closing bracket alone, because a
reshaped config would otherwise put the status normaliser after whichever
class's config closed next — including another model that states a `status` of
its own.
"""
BLOCK_STATUS_VALIDATOR = (
    '    _status_fails_closed = field_validator("status", mode="before")(fails_closed_to_busy)'
)

CONFIG_OPEN = "    model_config = ConfigDict("
"""The generated model config's first line, as the generator writes it."""

EXTRA = re.compile(r"^        extra=(['\"])[a-z]+\1,$")
"""The generated `extra=…` setting, in either quote style the generator writes."""

UNUSED_ALIAS = "type Model = Any"
TYPING_EXTENSIONS_IMPORT = "from typing_extensions import TypeAliasType"

INTEGER_BOUNDS = {
    "int32": (-2_147_483_648, 2_147_483_647),
    "uint32": (0, 4_294_967_295),
    "uint64": (0, 18_446_744_073_709_551_615),
}
"""The inclusive range each wire integer `format` implies.

The schema exporter publishes both ends as `minimum`/`maximum`, so the generator
writes the `ge`/`le` constraints itself. This table is what the pass holds the schema
and the generated model to: a format the wire never uses is absent, and carrying one
would fail the walk below rather than be guessed at.
"""

TYPE_ALIAS_TYPE = re.compile(r"^([A-Za-z_][A-Za-z0-9_]*) = TypeAliasType\(")
"""A synthesised `Name = TypeAliasType("Name", ...)` helper emitted by datamodel-codegen."""

FIELD = re.compile(r"^    ([A-Za-z_][A-Za-z0-9_]*): ")
"""A generated field declaration: four spaces, the field's name, then its type."""

ALIAS = re.compile(r"""alias=(['\"])(.*?)\1""")
"""A `Field(…)`'s wire name, in either quote style the generator writes before ruff runs."""

DEFINITION = re.compile(
    r"^(?:class ([A-Za-z_][A-Za-z0-9_]*)|type ([A-Za-z_][A-Za-z0-9_]*) =)", re.MULTILINE
)

ALL_HEADER = """

# Every model and type alias this module defines, in sorted order, so a check can
# prove the package root re-exports every one of them.
__all__ = [
"""


@dataclass(frozen=True)
class IntegerField:
    """One generated field whose value is a wire integer of a fixed width."""

    model: str
    """The generated class's name: the schema definition's title."""
    field: str
    """The field's wire name, which is the declaration's name or its `alias=`."""
    format: str
    """The wire `format` the field carries, such as `uint32`."""
    minimum: int
    maximum: int
    """The inclusive bounds that format implies, as the schema states them."""
    path: tuple[str, ...]
    """Where the schema states the integer, for a failure a person can follow."""


def _only(lines: list[str], expected: str, what: str) -> int:
    """The index of the one line that is `expected`, or the failure naming `what`."""
    hits = [index for index, line in enumerate(lines) if line == expected]
    if len(hits) != 1:
        raise SystemExit(f"postprocess_models: expected one {what}, found {len(hits)}")
    return hits[0]


def _keyed(lines: list[str], name: str, label: str) -> None:
    """Attach the keyed dispatch to the `Annotated` alias `type <name> = Annotated[`."""
    opening = _only(lines, f"type {name} = Annotated[", f"`{name}` alias")
    closing = lines.index("]", opening)
    lines.insert(closing, f'    KeyedUnion("{label}"),')


def _block_status(lines: list[str]) -> None:
    """Add the fail-closed status validator to `Block`, after its model config.

    The config has to be the one `Block` is generated with: attaching the validator
    to another class would normalise a status that other class owns, silently.
    """
    start = _only(lines, BLOCK, f"`{BLOCK}` class")
    config = lines[start + 1 : start + 1 + len(BLOCK_CONFIG)]
    if tuple(config) != BLOCK_CONFIG:
        raise SystemExit(
            f"postprocess_models: expected the `Block` model config after `{BLOCK}`, found {config}"
        )
    lines.insert(start + 1 + len(BLOCK_CONFIG), BLOCK_STATUS_VALIDATOR)


def integer_fields(schema: Path) -> tuple[IntegerField, ...]:
    """Every wire integer the schema states as a field, with the bounds it must keep.

    The walk finds each integer by its `format` rather than by a field it expects, so
    a wire integer added anywhere it is handled surfaces here; one that lands in a
    position this pass cannot read a class and field name out of fails the walk rather
    than passing unchecked. The bounds the node states have to be exactly the range
    its format implies: a schema that published a different one would generate a model
    that enforces the wrong limit.
    """
    # `Any` is the honest type for a parsed JSON document: its shape is checked as it
    # is read, and the generator is the only reader of the fields not checked here.
    document: dict[str, Any] = json.loads(schema.read_text(encoding="utf-8"))
    definitions: dict[str, Any] = document["$defs"]
    fields: list[IntegerField] = []
    for path, stated, entry in sorted(_integer_formats(document), key=lambda item: item[0]):
        handled = (
            # The field's own type: an `Option<u64>` states one node whose `type`
            # admits both `integer` and `null`.
            len(path) == 4 and path[0] == "$defs" and path[2] == "properties"
        ) or (
            # A map's values, such as `Request.weights`: the object arm of the
            # nullable map states them under `additionalProperties`.
            len(path) == 7
            and path[0] == "$defs"
            and path[2] == "properties"
            and path[4] == "anyOf"
            and path[6] == "additionalProperties"
        )
        if not handled:
            raise SystemExit(
                "postprocess_models: a wire integer is stated in a position this pass "
                f"does not read: {'/'.join(path)}"
            )
        minimum, maximum = INTEGER_BOUNDS[stated]
        if entry.get("minimum") != minimum or entry.get("maximum") != maximum:
            raise SystemExit(
                f"postprocess_models: {'/'.join(path)} declares format `{stated}` "
                f"without its bounds [{minimum}, {maximum}]: {entry!r}"
            )
        name, field = path[1], path[3]
        title: str = definitions[name].get("title", name)
        fields.append(IntegerField(title, field, stated, minimum, maximum, path))
    if not fields:
        raise SystemExit("postprocess_models: the schema states no wire integers at all")
    return tuple(fields)


def _integer_formats(
    document: object, path: tuple[str, ...] = ()
) -> Iterator[tuple[tuple[str, ...], str, Mapping[object, object]]]:
    """Every wire integer `format` in `document`, its path, and its schema node."""
    if isinstance(document, Mapping):
        entries = cast("Mapping[object, object]", document)
        stated = entries.get("format")
        if isinstance(stated, str) and stated in INTEGER_BOUNDS:
            yield path, stated, entries
        for key, inner in entries.items():
            yield from _integer_formats(inner, (*path, str(key)))
    elif isinstance(document, list):
        for index, inner in enumerate(cast("list[object]", document)):
            yield from _integer_formats(inner, (*path, str(index)))


def _open_models(schema: Path) -> dict[str, bool]:
    """Each generated class's name, and whether its definition is left open.

    The exporter states `additionalProperties: false` on every request and input
    object, because serde refuses a key it does not name, and omits it on every
    response and output object, because serde reads those without
    `deny_unknown_fields` so an older client accepts a field a newer engine added.
    The map is read off that keyword rather than from a list kept beside it, so a
    definition the exporter opens or closes moves with the wire, and walking for
    titled object schemas rather than only the named definitions holds a union's
    inline arms — a `CalendarOutcome`'s variants — to the same answer.
    """
    # `Any` is the honest type for a parsed JSON document: its shape is checked as
    # it is read, and the generator is the only reader of the fields not checked here.
    document: dict[str, Any] = json.loads(schema.read_text(encoding="utf-8"))
    models: dict[str, bool] = {}
    for title, additional in _object_definitions(document):
        if title in models:
            raise SystemExit(f"postprocess_models: two object schemas are titled `{title}`")
        models[title] = additional is not False
    if not models:
        raise SystemExit("postprocess_models: the schema states no titled object at all")
    return models


def _object_definitions(document: object) -> Iterator[tuple[str, object]]:
    """Every titled object schema in `document`, with its `additionalProperties`.

    An object schema is one whose `type` admits `object` and which states
    `properties`; a map's `additionalProperties` alone (`Request.weights`) names no
    class and is skipped.
    """
    if isinstance(document, Mapping):
        entries = cast("Mapping[object, object]", document)
        stated = entries.get("type")
        admits_object = stated == "object" or (
            isinstance(stated, list) and "object" in cast("list[object]", stated)
        )
        title = entries.get("title")
        if (
            admits_object
            and isinstance(title, str)
            and isinstance(entries.get("properties"), Mapping)
        ):
            yield title, entries.get("additionalProperties")
        for inner in entries.values():
            yield from _object_definitions(inner)
    elif isinstance(document, list):
        for inner in cast("list[object]", document):
            yield from _object_definitions(inner)


def _state_extra(lines: list[str], models: Mapping[str, bool]) -> None:
    """State each generated class's `extra` policy from the schema's `additionalProperties`.

    The generator writes `extra='forbid'` for a definition that states
    `additionalProperties: false` and nothing at all for one that does not, which
    leaves an open model relying on Pydantic's default — and on the base model's
    config, which is that same default until the day it is not. Every class states
    its policy outright instead: `forbid` where the wire refuses an unknown key,
    `ignore` where serde tolerates one. A response carrying a newer engine's
    additive field then parses, and a request carrying a stray key is still refused.
    """
    generated = {
        line.removeprefix("class ").removesuffix("(OpportuneModel):")
        for line in lines
        if line.startswith("class ") and line.endswith("(OpportuneModel):")
    }
    if generated != set(models):
        raise SystemExit(
            "postprocess_models: the schema's titled objects and the generated classes "
            f"disagree about {sorted(generated ^ set(models))}"
        )
    for name, is_open in models.items():
        _state_one_extra(lines, name, "ignore" if is_open else "forbid")


def _state_one_extra(lines: list[str], model: str, extra: str) -> None:
    """Write `extra='<extra>',` into `model`'s generated config, adding it if absent."""
    start = _only(lines, f"class {model}(OpportuneModel):", f"`{model}` class")
    if start + 1 >= len(lines) or lines[start + 1] != CONFIG_OPEN:
        raise SystemExit(
            f"postprocess_models: expected the `{model}` model config after its class, "
            f"found {lines[start + 1 : start + 2]}"
        )
    close = _statement(lines, start + 1)
    for index in range(start + 2, close + 1):
        if EXTRA.match(lines[index]):
            lines[index] = f"        extra='{extra}',"
            return
    lines.insert(start + 2, f"        extra='{extra}',")


def _inline_type_alias_types(lines: list[str]) -> list[str]:
    """Inline datamodel-codegen's synthesised `TypeAliasType` helpers into field types.

    Constrained `additionalProperties` (such as `Request.weights`'s `int32` values)
    produce a top-level `WeightsAdditionalProperty = TypeAliasType(...)` helper and a
    `typing_extensions` import. Inlining the alias keeps the field's exact Pydantic
    constraint without leaking a synthetic helper into `__all__` or requiring
    `typing_extensions` on Python 3.12+.
    """
    aliases: dict[str, str] = {}
    kept: list[str] = []
    index = 0
    while index < len(lines):
        match = TYPE_ALIAS_TYPE.match(lines[index])
        if match is None:
            kept.append(lines[index])
            index += 1
            continue
        name = match.group(1)
        close = _statement(lines, index)
        statement = " ".join(line.strip() for line in lines[index : close + 1])
        prefix = f"{name} = TypeAliasType("
        if not statement.startswith(prefix) or not statement.endswith(")"):
            raise SystemExit(f"postprocess_models: unexpected TypeAliasType shape: {statement!r}")
        inner = statement[len(prefix) : -1].strip()
        label_match = re.match(r"""^(['"])([A-Za-z_][A-Za-z0-9_]*)\1\s*,\s*(.+)$""", inner)
        if label_match is None or label_match.group(2) != name:
            raise SystemExit(
                f"postprocess_models: TypeAliasType `{name}` does not name itself: {statement!r}"
            )
        aliases[name] = label_match.group(3).rstrip(",").strip()
        index = close + 1

    if aliases:
        kept = [line for line in kept if line != TYPING_EXTENSIONS_IMPORT]
        for name, target in aliases.items():
            pattern = re.compile(rf"\b{re.escape(name)}\b")
            replaced = 0
            for pos, line in enumerate(kept):
                updated, count = pattern.subn(
                    lambda _match, replacement=target: replacement,
                    line,
                )
                if count:
                    kept[pos] = updated
                    replaced += count
            if replaced == 0:
                raise SystemExit(f"postprocess_models: TypeAliasType `{name}` was never referenced")

    if any("TypeAliasType" in line for line in kept):
        raise SystemExit("postprocess_models: unexpected `TypeAliasType` remains after inlining")
    return kept


def _verify_integer_bounds(lines: list[str], integers: tuple[IntegerField, ...]) -> None:
    """Assert every wire integer field states the exact `ge`/`le` the schema declares.

    The generator writes the constraints from the schema's `minimum`/`maximum`, so
    this pass adds nothing to the text: it proves each one reached the field it belongs
    to. A constraint lost between the schema and the model would let an integer past
    the wire range validate here and be refused by serde after the document had
    already crossed the boundary — the failure the published bounds exist to prevent.

    A field is named by its wire name, which the generator writes as the declaration's
    own name — or, where a name the base model already uses had to be spelled
    differently, as the `alias=` inside the declaration's `Field(…)`.
    """
    for integer in integers:
        start = _only(lines, f"class {integer.model}(OpportuneModel):", f"`{integer.model}` class")
        for index in range(start + 1, _class_end(lines, start)):
            declared = FIELD.match(lines[index])
            if declared is None:
                continue
            close = _statement(lines, index)
            text = "\n".join(lines[index : close + 1])
            aliased = {match.group(2) for match in ALIAS.finditer(text)}
            if declared.group(1) != integer.field and integer.field not in aliased:
                continue
            if f"ge={integer.minimum}" not in text or f"le={integer.maximum}" not in text:
                raise SystemExit(
                    f"postprocess_models: `{integer.model}.{integer.field}` is missing the "
                    f"{integer.format} bounds ge={integer.minimum}, le={integer.maximum} "
                    f"the schema states at {'/'.join(integer.path)}: {text!r}"
                )
            break
        else:
            raise SystemExit(
                f"postprocess_models: expected `{integer.model}` to declare "
                f"`{integer.field}`, and it does not"
            )


def _class_end(lines: list[str], start: int) -> int:
    """The index one past the class whose header is `lines[start]`.

    A generated definition starts in the first column and every line of its body is
    indented, so the first unindented line after the header ends it.
    """
    for index in range(start + 1, len(lines)):
        if lines[index] and not lines[index][0].isspace():
            return index
    return len(lines)


def _statement(lines: list[str], index: int) -> int:
    """The last line of the statement that starts at `index`."""
    depth = _unbalanced(lines[index])
    close = index
    while depth > 0:
        close += 1
        if close == len(lines):
            raise SystemExit("postprocess_models: a field statement is never closed")
        depth += _unbalanced(lines[close])
    return close


def _unbalanced(text: str) -> int:
    """How many brackets `text` opens and leaves open, with quoted prose ignored.

    A field's description is a quoted literal the generator writes, so a bracket in
    one is prose rather than the shape of the statement around it. Braces are counted
    with the parentheses and brackets: a default the generator writes as a mapping —
    ``cast(…, {"role": "host"})``, and whatever a future schema spells — can span
    lines, and a statement that stopped at its closing ``]`` would leave that mapping
    outside the field a bound has to be added to.
    """
    return sum(
        1 if text[index] in "[({" else -1 for index in _unquoted(text) if text[index] in "[](){}"
    )


def _unquoted(text: str) -> Iterator[int]:
    """The offsets in `text` that are outside its quoted prose.

    The generator writes a description as a one-line literal before ruff normalises
    its quotes, so both quote styles open one.
    """
    quote = ""
    escaped = False
    for index, char in enumerate(text):
        if quote:
            if escaped:
                escaped = False
            elif char == "\\":
                escaped = True
            elif char == quote:
                quote = ""
        elif char in "\"'":
            quote = char
        else:
            yield index


def rewritten(
    lines: list[str], integers: tuple[IntegerField, ...], models: Mapping[str, bool]
) -> list[str]:
    """`lines` with the dispatch, the status normalisation, the policies and the exports."""
    lines = _inline_type_alias_types(list(lines))
    lines[_only(lines, BASE_IMPORT, f"`{BASE_IMPORT}` import")] = BASE_IMPORT_WITH_DISPATCH
    lines[_only(lines, PYDANTIC_IMPORT, f"`{PYDANTIC_IMPORT}` import")] = (
        PYDANTIC_IMPORT_WITH_VALIDATOR
    )
    for name, label in KEYED_UNIONS.items():
        _keyed(lines, name, label)
    _state_extra(lines, models)
    _block_status(lines)
    _verify_integer_bounds(lines, integers)
    return [line for line in lines if line != UNUSED_ALIAS]


def named(text: str) -> list[str]:
    """Every class and type alias the module defines, in sorted order."""
    names: set[str] = set()
    for match in DEFINITION.finditer(text):
        name = next(group for group in match.groups() if group is not None)
        names.add(name)
    return sorted(names)


def main(path: Path, schema: Path) -> None:
    """Rewrite the generated module at `path` in place, holding it to the `schema`."""
    lines = rewritten(
        path.read_text(encoding="utf-8").splitlines(),
        integer_fields(schema),
        _open_models(schema),
    )
    exports = "\n".join(f'    "{name}",' for name in named("\n".join(lines)))
    path.write_text("\n".join(lines) + ALL_HEADER + exports + "]\n", encoding="utf-8")


if __name__ == "__main__":
    if len(sys.argv) != 3:
        raise SystemExit("usage: postprocess_models.py <generated models module> <combined schema>")
    main(Path(sys.argv[1]), Path(sys.argv[2]))
