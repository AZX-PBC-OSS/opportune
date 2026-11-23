"""The base model every generated model inherits, and the keyed union dispatch.

``_models.py`` is generated from the combined schema and never hand-edited, so the
settings that are decisions rather than transcriptions live here: every generated
model validates an assignment as it validates a construction, a provider's block
status fails closed on the engine's reading of it, and a union of form models is
entered through one key-named door rather than tried arm by arm.

That first setting matters because a request is assembled field by field —
``rule.who = …``, ``hours.who = …`` — and a model that accepted an invalid
assignment would carry a value the engine rejects, moving the rejection from the
line that wrote it to the serialisation that sends it. The engine's own error is
still the authority on what a document may contain; this keeps the model honest
about what it holds.

:class:`KeyedUnion` is here for the same reason: it is metadata the codegen
pipeline attaches to a generated union alias, so the forms are derived from the
arms the schema declares rather than from a list kept beside them.

:func:`document_segments` walks a Pydantic validation finding's location against
the models it is about, so a pointer into the document a caller wrote comes out
without the tags a validator adds to say which union arm it picked.
"""

from collections.abc import Callable, Mapping, Sequence
from dataclasses import dataclass
from functools import cache
from types import UnionType
from typing import (
    Annotated,
    Any,
    LiteralString,
    TypeAliasType,
    TypeGuard,
    Union,
    cast,
    get_args,
    get_origin,
    get_type_hints,
)

from pydantic import (
    BaseModel,
    ConfigDict,
    GetCoreSchemaHandler,
    GetJsonSchemaHandler,
    PydanticUserError,
)
from pydantic.fields import FieldInfo
from pydantic.json_schema import JsonSchemaValue
from pydantic_core import PydanticCustomError, core_schema

_MATCH_ALL = "true"
"""The tag of the one form a document states as a literal rather than as a mapping.

``MatchAll`` is the JSON literal ``true``, so no key names it; a selector that is
``true`` selects this arm.
"""

_BLOCK_STATUSES = frozenset({"free", "tentative", "busy"})
"""The statuses the engine knows; a provider's value outside them reads as `busy`."""


class OpportuneModel(BaseModel):
    """The base of every generated model: assignments are validated too."""

    model_config = ConfigDict(validate_assignment=True)


def fails_closed_to_busy(value: object) -> object:
    """A provider's block status, with every unrecognised value read as `busy`.

    A provider reports its own vocabulary — ``workingElsewhere``, ``oof``, an empty
    string — and reading one of those as free would offer time that is taken, so
    the engine's fail-closed reading is applied where the document is first read.
    Only a string is normalised: a value of another type is left for the field's
    own validation to reject, which is a malformed document rather than a status a
    provider reported.
    """
    if isinstance(value, str) and value not in _BLOCK_STATUSES:
        return "busy"
    return value


@dataclass(frozen=True)
class _Form:
    """One arm of a keyed union: what names it and the tag it is selected by."""

    tag: str
    """The tag this arm's choice is selected by; the first wire key that names it."""
    keys: tuple[str, ...]
    """Every spelling that names this arm: its wire keys, then the field names it reads."""
    names: tuple[str, ...]
    """The wire keys among `keys`: what a finding lists, because that is the document."""
    model: type[BaseModel] | None
    """The arm's model, or `None` for a form a document states as a literal."""


class KeyedUnion:
    """Metadata entering a union of form models through the key that names each form.

    ``Selector``, ``Term`` and ``LeadTimeMax`` are unions of models, and Pydantic's
    union reports a value none of them accepts once per arm — and again inside every
    arm that recurses — so one misspelled selector key, term or lead-time maximum is
    a wall of findings, none of which says which keys the document has. Each of those
    unions is a mapping whose keys state its form, so the form is decidable before
    any arm runs: this metadata derives each arm's keys from the arm's own fields,
    refuses a mapping that names none of them or more than one, and keys the union
    so only the named arm validates. A bad value inside a form is then that form's
    own finding, at the field it is about, one level in as much as at the top.

    The metadata is attached to the generated aliases by the codegen post-pass,
    because they are unions of the generated models and only the generated module
    can name the arms. `label` names the union in a finding and, with its spaces
    spelled as underscores, prefixes the error codes (``selector_form_unknown``).
    """

    def __init__(self, label: str) -> None:
        self._label = label
        self._prefix = label.replace(" ", "_")

    def __get_pydantic_core_schema__(
        self, source_type: object, handler: GetCoreSchemaHandler
    ) -> core_schema.CoreSchema:
        inner = handler(source_type)
        forms = _forms(get_args(source_type))
        return core_schema.no_info_before_validator_function(
            _dispatch(self._label, self._prefix, forms), _keyed_union(inner, forms)
        )

    def __get_pydantic_json_schema__(
        self, schema: core_schema.CoreSchema, handler: GetJsonSchemaHandler
    ) -> JsonSchemaValue:
        """The document states the union the wire schema states, ``anyOf`` over the arms.

        Which arm validates is the validator's business, not the document's: a keyed
        union renders as ``oneOf``, while the wire schema this document has to agree
        with — the generated ``schema.json`` the TypeScript types come from — says
        ``anyOf`` over the same forms. The arms are mutually exclusive either way, so
        only the keyword differs; it is stated here as the wire states it.
        """
        document = handler(schema)
        one_of = document.pop("oneOf", None)
        if one_of is not None and "anyOf" not in document:
            document["anyOf"] = one_of
        return document


@cache
def _forms(arms: tuple[Any, ...]) -> tuple[_Form, ...]:
    """Each arm's dispatch metadata, derived from the arm's own fields.

    Deriving a form reads every field of the arm, and a pointer walk asks for the
    same union's forms once per finding, so they are derived once per union. The arms
    are the generated models' own, and a type is hashable; the result is frozen, so a
    cached tuple is handed out rather than rebuilt.
    """
    forms: list[_Form] = []
    for arm in arms:
        if not (isinstance(arm, type) and issubclass(arm, BaseModel)):
            forms.append(_Form(_MATCH_ALL, (), (), None))
            continue
        keys, names = _keys_of(arm)
        if not names:
            raise PydanticUserError(
                f"`{arm.__name__}` is a union arm with no field naming it",
                code="callable-discriminator-no-tag",
            )
        forms.append(_Form(names[0], keys, names, arm))
    return tuple(forms)


def _keys_of(model: type[BaseModel]) -> tuple[tuple[str, ...], tuple[str, ...]]:
    """The spellings that name `model`'s form, and the wire names among them.

    A form is defined before the alias that holds the arms, so a field whose name a
    Python keyword spells (``not`` is written ``not_``) has not read the
    ``Field(alias=…)`` that spells the keyword when the alias is first built. The
    declared annotation still carries it, so the wire name is read there while the
    model is incomplete.

    A model that populates by name accepts each field's Python name as well as its
    wire name — which is the spelling ``model_dump()`` writes, so a dumped document
    round-trips — while a finding still lists only the wire names, because those
    are what a document states.
    """
    by_name = bool(model.model_config.get("populate_by_name"))
    keys: list[str] = []
    names: list[str] = []
    declared: dict[str, Any] | None = None
    for name, field in model.model_fields.items():
        wire = field.alias
        if wire is None and not model.__pydantic_complete__:
            if declared is None:
                declared = get_type_hints(model, include_extras=True)
            wire = _declared_alias(declared[name])
        wire = wire or name
        names.append(wire)
        keys.append(wire)
        if by_name and name not in keys:
            keys.append(name)
    return tuple(keys), tuple(names)


def _declared_alias(declared: Any) -> str | None:
    """The wire alias an annotation's `Field(…)` metadata states, if it states one."""
    for metadata in get_args(declared)[1:]:
        alias = getattr(metadata, "alias", None)
        if isinstance(alias, str):
            return alias
    return None


def _keyed_union(
    schema: core_schema.CoreSchema, forms: tuple[_Form, ...]
) -> core_schema.CoreSchema:
    """`schema` with each arm of the union selected by the tag that names it.

    A schema that is not the arm union — the definition reference the recursion into
    this alias yields, or an alias somebody applied the metadata to — is returned as
    it is: the definition it names already carries the dispatch.
    """
    if schema["type"] != "union":
        return schema
    entries = schema["choices"]
    if len(forms) != len(entries):
        return schema
    choices: dict[str, core_schema.CoreSchema] = {}
    for form, entry in zip(forms, entries, strict=True):
        if isinstance(entry, tuple):
            # A per-choice mode is not a keyed union's shape; leave the union alone.
            return schema
        choices[form.tag] = entry
    keyed = core_schema.tagged_union_schema(choices=choices, discriminator=_discriminator(forms))
    # What the alias says about its own *document* — its title, through Pydantic's
    # JSON-schema updates, and its `ref` — is not the validation shape, so it stays:
    # only the type and its choices are replaced.
    return cast("core_schema.CoreSchema", {**schema, **keyed})


def _is_json_object(value: object) -> TypeGuard[Mapping[Any, Any]]:
    """Whether `value` is the JSON object a form's key is read from.

    ``Any`` is the honest type here rather than a lazily-typed container: the value
    is arbitrary caller JSON, read before Pydantic has looked at any field.
    """
    return isinstance(value, Mapping)


def _refusal(code: str, template: str, context: dict[str, Any]) -> PydanticCustomError:
    """A finding whose code and message are built from the union's own label.

    pydantic-core types both as ``LiteralString`` because a template is formatted,
    never built from caller data. These are built from the union's own label, once
    per union rather than at each raise, so the type each call site must state is
    restored here.
    """
    return PydanticCustomError(
        cast("LiteralString", code), cast("LiteralString", template), context
    )


def _dispatch(label: str, prefix: str, forms: tuple[_Form, ...]) -> Callable[[object], object]:
    """The before-validator that refuses a value naming no form, or more than one."""

    accepted = {key: form for form in forms for key in form.keys}
    names = ", ".join(sorted({name for form in forms for name in form.names}))
    literal = any(form.model is None for form in forms)
    one_key_arms = all(len(form.names) == 1 for form in forms if form.model is not None)
    if literal:
        not_a_form = "A {label} is `true` or a mapping naming one of {names}; `{kind}` is neither."
        unknown_form = (
            "`{key}` is not a {label} form; a {label} is `true` or a mapping naming one of {names}."
        )
    else:
        not_a_form = "A {label} is a mapping naming one of {names}; `{kind}` is not."
        unknown_form = (
            "`{key}` is not a {label} form; a {label} is a mapping naming one of {names}."
        )
    if one_key_arms:
        too_many = (
            "A {label} names exactly one form, one of {names}; this mapping names {count} keys."
        )
    else:
        too_many = (
            "A {label} names exactly one form, one of {names}; this mapping names {count} forms."
        )

    def dispatched(value: object) -> object:
        if isinstance(value, BaseModel) or (value is True and literal):
            return value
        if not _is_json_object(value):
            raise _refusal(
                f"{prefix}_not_a_form",
                not_a_form,
                {"label": label, "names": names, "kind": type(value).__name__},
            )
        keys = list(value)
        for key in keys:
            if not isinstance(key, str) or key not in accepted:
                raise _refusal(
                    f"{prefix}_form_unknown",
                    unknown_form,
                    {"key": str(key), "label": label, "names": names},
                )
        named = {accepted[key].tag for key in keys}
        if len(named) != 1:
            raise _refusal(
                f"{prefix}_forms",
                too_many,
                {"label": label, "names": names, "count": len(named)},
            )
        return value

    return dispatched


def _discriminator(forms: tuple[_Form, ...]) -> Callable[[Any], str]:
    """The tag `value` names: the one its keys state, or the arm it was built as.

    Everything :func:`_dispatch` refuses is refused before this runs, so what is
    left is a mapping naming one form, a selector a caller already built, or the
    literal form. A model of the wrong kind has no form to name, so its type name is
    the tag: the keyed union names that tag as one it does not expect and lists the
    ones it does, in one finding rather than one per arm.
    """
    tags = {key: form.tag for form in forms for key in form.keys}

    def form_of(value: Any) -> str:
        if _is_json_object(value) and value:
            return tags[next(iter(value))]
        for form in forms:
            if form.model is not None and isinstance(value, form.model):
                return form.tag
        if value is True:
            return _MATCH_ALL
        return type(value).__name__

    return form_of


def document_segments(root: object, location: Sequence[object]) -> list[object] | None:
    """The segments of `root`'s document that a validation finding's `location` names.

    Pydantic's location is not the document's path: entering a union of models adds
    the tag of the arm it selected as a segment of its own — the `kind` a rule's
    discriminator chose, and the key a selector names its form with. Those tags are
    the validator's bookkeeping, not keys the caller wrote, so a pointer built from
    the location unchanged names a place in no document. Walking `root`'s own types
    tells the two apart segment by segment, which is what a document nesting the
    same key needs: in `{"not": {"not": …}}` every level contributes a tag and then
    the key it duplicates, so each tag is dropped and both keys stay.

    `None` when the location is not rooted in `root` at all — its first segment
    names no field of that document and no tag of one of its unions. A caller with
    several candidate documents reads that as "not this one" and asks the next,
    rather than taking a half-resolved pointer that dropped a key it could not
    account for. A segment *after* the first that names no field is kept as the
    caller wrote it, with the tags already dropped: a key no model reads is exactly
    what an `extra_forbidden` finding reports, and its pointer has to name it.

    `root` is any annotation the document is read into: a model, or one of the
    generated union aliases a route may declare as its body.
    """
    kept: list[object] = []
    current: object = root
    for index, segment in enumerate(location):
        step = _step(current, segment)
        if step is None:
            if index == 0:
                return None
            kept.extend(location[index:])
            return kept
        current, named = step
        if named:
            kept.append(segment)
    return kept


def _step(current: object, segment: object) -> tuple[object, bool] | None:
    """The type `segment` is read against, and whether it names a key of the document.

    A union's tag is consumed and dropped; a mapping key, an array index and a field
    name are all keys the caller wrote, so they are kept.
    """
    annotation, metadata = _unwrapped(current)
    origin = get_origin(annotation)
    # `X | Y` and `Union[X, Y]` are one type on 3.14 and two on 3.12, where the
    # generated models' PEP 604 unions report `types.UnionType` as their origin.
    if origin is Union or origin is UnionType:
        arms = tuple(arm for arm in get_args(annotation) if arm is not type(None))
        if len(arms) == 1:
            return _step(arms[0], segment)
        tags = _tags(arms, metadata)
        if tags is not None:
            arm = tags.get(segment) if isinstance(segment, str) else None
            return None if arm is None else (arm, False)
        for arm in arms:
            step = _step(arm, segment)
            if step is not None:
                return step
        return None
    if origin is list:
        args = get_args(annotation)
        return None if not args else (args[0], True)
    if origin is dict:
        args = get_args(annotation)
        return (args[1], True) if len(args) == 2 else None
    if origin is tuple:
        return _tuple_step(get_args(annotation), segment)
    if isinstance(annotation, type) and issubclass(annotation, BaseModel):
        field = _field(annotation, segment)
        return None if field is None else (field.annotation, True)
    return None


def _unwrapped(annotation: object) -> tuple[object, tuple[object, ...]]:
    """`annotation` with its `Annotated` wrapping and named aliases removed.

    What comes back beside it is the metadata those layers carried, which is where a
    union states how its arms are tagged.
    """
    metadata: list[object] = []
    while True:
        if isinstance(annotation, TypeAliasType):
            annotation = annotation.__value__
        elif get_origin(annotation) is Annotated:
            args = get_args(annotation)
            metadata.extend(args[1:])
            annotation = args[0]
        else:
            return annotation, tuple(metadata)


def _tags(arms: tuple[object, ...], metadata: Sequence[object]) -> dict[object, object] | None:
    """The tag each arm is selected by, for a union whose location reports one.

    A union no metadata keys — a plain union of models, or a field that may be
    `None` — adds no segment of its own, so there is no tag to strip and the arm is
    found by the key the document names.
    """
    if any(isinstance(entry, KeyedUnion) for entry in metadata):
        return {form.tag: form.model for form in _forms(arms) if form.model is not None}
    for entry in metadata:
        if isinstance(entry, FieldInfo) and isinstance(entry.discriminator, str):
            return _discriminated(arms, entry.discriminator)
    return None


@cache
def _discriminated(arms: tuple[object, ...], discriminator: str) -> dict[object, object]:
    """The literal each arm's discriminator field states, and that arm.

    Derived once per union for the same reason as :func:`_forms`, and handed out to
    be read: every caller looks a tag up in it and none writes to it.
    """
    tagged: dict[object, object] = {}
    for arm in arms:
        if isinstance(arm, type) and issubclass(arm, BaseModel):
            field = arm.model_fields.get(discriminator)
            if field is not None:
                tagged.update({tag: arm for tag in get_args(field.annotation)})
    return tagged


def _field(model: type[BaseModel], segment: object) -> FieldInfo | None:
    """The field `segment` names in `model`, by its wire alias or its Python name."""
    if not isinstance(segment, str):
        return None
    for name, field in model.model_fields.items():
        if segment in (name, field.alias):
            return field
    return None


def _tuple_step(args: tuple[object, ...], segment: object) -> tuple[object, bool] | None:
    """The element type of a tuple annotation at the index `segment` names."""
    if not args:
        return None
    if args[-1] is Ellipsis:
        return args[0], True
    if isinstance(segment, int) and -len(args) <= segment < len(args):
        return args[segment], True
    return None
