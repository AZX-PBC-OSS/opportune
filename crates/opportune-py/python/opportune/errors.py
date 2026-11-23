"""The exception every public call raises when a document is rejected, and the
envelope a web application serves for one.

The native extension raises its own exception class over the FFI boundary. This
module translates it at the package boundary so a caller catches one class, and
so each finding arrives as the generated :class:`~opportune.Diagnostic` model
rather than as a bare dict: an exception handler can then serialise
``exc.errors`` for a client without reshaping it first.

:class:`ErrorEnvelope` is that serialised form, and the two ``error_envelope_from_*``
helpers build it from either failure a FastAPI application handles — a rejection
the engine raised and a document the generated request model refused. Both
arrive as one body shape, so a client reads one 422 contract whichever layer
rejected the request. :func:`http_status_for_error` says which of the two statuses
a rejection is answered with, so the same handler never serves a broken deployment's
failure as a client's own.
"""

from collections.abc import Generator, Mapping, Sequence
from contextlib import contextmanager
from typing import Protocol, TypeAliasType, TypeGuard, cast, runtime_checkable

from pydantic import ConfigDict, Field, ValidationError

import opportune._opportune as _native
from opportune import _models
from opportune._base import OpportuneModel, document_segments
from opportune._models import Diagnostic, Request

INVALID_REQUEST = "invalid_request"
"""The code an envelope carries when the request model refused the document.

It is the code the engine itself uses for a rejected request, so a client switches
on one code whichever layer rejected what it sent.
"""

INVALID_LIMITS = "invalid_limits"
"""The code an engine's construction failure carries when it is about the limits.

An engine is built from the deployment's own configuration — a database directory,
its version label and the limits document — so nothing it is built from is a
caller's request, and :func:`http_status_for_error` answers this code ``500``.
"""


class OpportuneError(Exception):
    """A rejected request, document or engine configuration.

    A rejected request carries every finding its validation produced in
    :attr:`errors`, each with the RFC 6901 JSON pointer of the request field it is
    about, so an exception handler can answer a client with the exact place in the
    document it sent that has to change.

    The native cause of the failure is chained as ``__cause__``, so a log or a
    debugger still sees the rejection the extension reported.
    """

    code: str
    """The stable, machine-readable code, such as ``invalid_json``."""

    message: str
    """A sentence naming the value that was rejected.

    The text may be reworded; match on :attr:`code` instead.
    """

    path: str | None
    """The RFC 6901 JSON pointer of the field at fault.

    ``None`` when the failure is about the document as a whole.
    """

    errors: list[Diagnostic]
    """The findings a rejected request carries, as generated models."""

    def __init__(
        self, code: str, message: str, *, path: str | None, errors: list[Diagnostic]
    ) -> None:
        super().__init__(message)
        self.code = code
        self.message = message
        self.path = path
        self.errors = errors

    @classmethod
    def from_native(cls, rejected: _native.OpportuneError) -> "OpportuneError":
        """The package's exception for a rejection the native layer raised."""
        return cls(
            rejected.code,
            rejected.message,
            path=rejected.path,
            errors=[Diagnostic.model_validate(finding) for finding in rejected.errors],
        )


class ErrorEnvelope(OpportuneModel):
    """The 422 body an application serves whichever layer rejected the request.

    Declare it as a route's ``responses={422: {"model": ErrorEnvelope}}`` and both
    this package's own rejection and the framework's request validation are
    documented as the one envelope they are served as.

    Serve it for the status the package classifies a rejection as, which is `422` for
    a failure the caller can fix; a `500` is the server's own failure, and an
    application answers that one without the message and path an envelope carries.
    """

    model_config = ConfigDict(extra="forbid", populate_by_name=True)

    code: str = Field(
        description="The stable, machine-readable code: `invalid_request` for a request "
        "either layer rejected, or the engine's own code when one argument or document "
        "is at fault."
    )
    message: str = Field(
        description="A sentence naming what was rejected. The text may be reworded; match "
        "on `code` instead."
    )
    path: str | None = Field(
        default=None,
        description="The RFC 6901 JSON pointer of the field the failure as a whole is about, "
        "or `null` for a request's findings: each of those carries its own pointer in "
        "`errors`.",
    )
    errors: list[Diagnostic] = Field(
        default_factory=list[Diagnostic],
        description="The findings, each with the RFC 6901 JSON pointer of the request field it "
        "is about. A finding's code is the engine's diagnostic code for a request it "
        "rejected, and the validator's stable name for a document a generated model "
        "refused. Both are stable; match on them rather than on the messages.",
    )


@runtime_checkable
class _DocumentRejection(Protocol):
    """A framework validation failure: Pydantic's, or FastAPI's wrapping of it."""

    def errors(self) -> Sequence[Mapping[str, object]]: ...


def rejection(code: str, message: str) -> OpportuneError:
    """A rejection of an argument this package could not read.

    The extension raises :class:`~opportune.OpportuneError` for everything it
    refuses, so the few checks the package itself makes — an argument with no
    UTF-8 spelling, a provider payload that is not UTF-8 or not serialisable, a
    preset given a value it cannot mean — raise the same class rather than a bare
    ``ValueError``, ``TypeError`` or ``UnicodeDecodeError`` no handler catches.

    There is no field ``path`` and no finding list: the argument itself is at
    fault, not a field of a document.
    """
    return OpportuneError(code, message, path=None, errors=[])


@contextmanager
def translating() -> Generator[None, None, None]:
    """Re-raise a native rejection as :class:`OpportuneError`, keeping its cause."""
    try:
        yield
    except _native.OpportuneError as rejected:
        raise OpportuneError.from_native(rejected) from rejected


def configuration_rejection(rejected: _native.OpportuneError) -> OpportuneError:
    """The package's exception for a rejection while an engine was being built.

    Everything an engine is built from is the deployment's own configuration — the
    time zone database directory, its version label and the limits document — so no
    rejection here is a caller's to fix. A code the vocabulary would otherwise serve
    as a ``422`` is the limits document, the one document an engine is built from,
    and it is reported as :data:`INVALID_LIMITS`; the native message, path and
    findings stay on it and the native failure stays chained as ``__cause__``.

    A code that already names the deployment's failure keeps it: the database codes
    are this package's vocabulary for one unusable directory, version or database,
    and a route that answers one of them answers it by name.
    """
    failure = OpportuneError.from_native(rejected)
    if http_status_for_error(failure) != 422:
        return failure
    return OpportuneError(
        INVALID_LIMITS,
        f"the limits are not a document the engine reads: {failure.message}",
        path=failure.path,
        errors=failure.errors,
    )


def rejected_argument(cause: ValidationError) -> str:
    """The caller's own argument name and why the model refused its value.

    A preset and an adapter build the rule model directly, so the first finding's
    outermost location is the argument the caller wrote (``who``, ``limit``) — except
    where the rule's own field carries a different name, as a ``rule_id`` is written to
    the rule as ``id`` and a ``notice`` as ``min``. The whole finding list stays on the
    ``__cause__``, so a handler that wants Pydantic's detail still has it.
    """
    finding = cause.errors(include_url=False)[0]
    location = finding["loc"]
    where = str(location[0]) if location else "the value"
    return f"`{where}` is not a value the rule accepts: {finding['msg']}"


_SERVER_ERROR_CODES = frozenset(
    {
        "argument_not_utf8",
        "invalid_argument",
        "invalid_limits",
        "payload_invalid",
        "serialization_failed",
        "tzdb_bundled_unavailable",
        "tzdb_directory_unusable",
        "tzdb_not_configured",
        "tzdb_version_missing",
        "tzdb_version_without_directory",
        "unsupported_schema_version",
    }
)
"""The codes that name the server's own failure rather than the caller's input.

Each one names something no field of a document can fix: the engine's time zone
database or its limits, writing a document as JSON, a provider payload the
application fetched, or this package's own argument checking.
"""


def http_status_for_error(exc: OpportuneError) -> int:
    """The status code an application answers `exc` with: ``422`` or ``500``.

    A rejection is one of two failures, and a client reads the difference: ``422``
    when the caller handed in something the engine or the models refused — a request
    document, a JSON body, a proposal — and ``500`` when the failure is the server's
    own, such as a time zone database the deployment cannot use. Serving every
    rejection as ``422`` tells a client its request was wrong when the deployment is
    broken, and hands it the internals of the failure along with the accusation.

    `exc` has to be this package's :class:`OpportuneError`: the helper classifies by
    the ``code`` attribute every rejection carries, and a value without one — a
    Pydantic ``ValidationError``, say, which the sibling
    :func:`error_envelope_from_validation_error` reads instead — is a ``TypeError``
    naming the class expected rather than an ``AttributeError`` from inside a handler.

    A code outside the set is answered ``422``, because the engine's codes are its
    vocabulary for documents it refused and it introduces one per rule of the format.
    Some codes are the reading of the code that holds them, and a route that knows
    which it holds answers them itself:

    - ``unsupported_schema_version`` is the server's failure for a version the
      application records and the caller's for a version it forwards.
    - ``invalid_argument`` is this package refusing an argument a helper was given —
      a preset's value, an adapter's payload, the version label an upgrade reads —
      rather than a document one of the engine's own calls read: a route handing
      such a value a caller's own field answers it ``422`` itself.
    - ``argument_not_utf8`` is a string this boundary could not read as UTF-8: a
      database directory the deployment configured, and also a zone name or a rule ID
      the route read out of a document the caller sent — a lone surrogate survives
      ``json.loads`` and a ``str`` field, so a client can write one. Such a route
      answers it ``422`` itself.

    A provider payload the application fetched is the server's own failure: the
    client's request was answered, and what could not be read is a Graph or Google
    response, so ``payload_invalid`` is answered ``500`` and its message stays out of
    the client's body.
    """
    if not isinstance(exc, OpportuneError):  # pyright: ignore[reportUnnecessaryIsInstance]
        raise TypeError(
            "http_status_for_error reads an OpportuneError, not a "
            f"{type(exc).__name__}: use error_envelope_from_validation_error for a "
            "validation failure",
        )
    return 500 if exc.code in _SERVER_ERROR_CODES else 422


def error_envelope_from_opportune_error(exc: OpportuneError) -> ErrorEnvelope:
    """The envelope for a rejection the engine raised."""
    return ErrorEnvelope(code=exc.code, message=exc.message, path=exc.path, errors=exc.errors)


def error_envelope_from_validation_error(
    exc: Exception, *, document: object = None
) -> ErrorEnvelope:
    """The envelope for a document the generated request model refused.

    ``exc`` is a Pydantic :class:`pydantic.ValidationError` or the FastAPI
    :class:`fastapi.exceptions.RequestValidationError` wrapping one: both report
    the same finding dictionaries, whose ``loc`` tuples this turns into the RFC 6901
    pointers :class:`~opportune.Diagnostic` carries, so a client reads one error
    contract whichever layer rejected its request, and one pointer resolver finds
    the field a finding names whether the engine or the request model rejected it.

    A JSON body's finding points into that body: the leading ``body`` segment is
    the parameter's, not a key the caller wrote, so it is dropped and the tag a
    union reports for the arm it picked is dropped with it. A parameter that is not
    in the body keeps its segment (``query``, ``path``, ``header``, ``cookie``),
    because such a finding names a place in the request that has no body field at
    that pointer.

    The body is not always a :class:`~opportune.Request`: a route may declare any
    document these models describe as its parameter — a stored
    :class:`~opportune.Rule` being checked, a :class:`~opportune.Selector` a picker
    hands back, a :class:`~opportune.LeadTimeMax` — and a route with two body
    parameters nests each under its name. The pointer is therefore resolved against
    the documents the models declare (:data:`_DOCUMENTS`), from the parameter name on
    to the field at fault, and the body decides between readings that differ. A
    finding about a model of the caller's own keeps the keys it wrote.

    ``document`` is the document the finding is about, for a failure that carries
    none: a Pydantic ``ValidationError`` is raised by a validator that was handed a
    payload, not an HTTP request, and the caller holds that payload. Naming it makes
    a pointer into that document exact the way a body's is — the reading served is
    the one that lands on the value the finding is about — which is what tells a
    document the package declares as a *class* (a stored :class:`~opportune.Goal`,
    :class:`~opportune.Part`, :class:`~opportune.Requirement`) from whichever union
    alias would otherwise read its first key as a form tag.

    An exception that carries no findings at all — a handler registered for
    :class:`Exception` receiving one — is answered with its own message and an
    empty finding list rather than by raising inside the handler.

    A rejection this package already structured — an :class:`OpportuneError`, which a
    handler registered for both failures may hand to the wrong one of the two helpers
    — is answered with that rejection's own envelope: its ``code``, ``message``,
    ``path`` and ``errors`` are the failure's, and the pointer into the document is
    the one the rejection already carries. Answering it as an empty
    ``invalid_request`` would drop the code a client switches on and the findings a
    handler exists to serve.
    """
    if isinstance(exc, OpportuneError):
        return error_envelope_from_opportune_error(exc)
    findings = _findings(exc, document)
    if not findings:
        return ErrorEnvelope(code=INVALID_REQUEST, message=str(exc))
    message = (
        findings[0].message
        if len(findings) == 1
        else f"the document was rejected in {len(findings)} places"
    )
    return ErrorEnvelope(code=INVALID_REQUEST, message=message, errors=findings)


def _findings(exc: Exception, document: object = None) -> list[Diagnostic]:
    """`exc`'s findings as generated models, each with a pointer into the document.

    The findings have to be *callable* off the exception, not only present: the
    runtime-checkable Protocol matches on the name alone, so an exception carrying a
    *list* named ``errors`` — this package's own :class:`OpportuneError` shaped one,
    which the entry point answers before reaching here, or a caller's own failure
    shaped the same way — would be called and raise inside the handler this helper
    exists to serve.

    A FastAPI validation failure carries the body it parsed; Pydantic's own does not,
    so the caller's ``document`` stands in for it where it is named.
    """
    if not isinstance(exc, _DocumentRejection) or not callable(exc.errors):
        return []
    carried = document if document is not None else getattr(exc, "body", None)
    body_parameter = hasattr(exc, "body")
    return [_finding(entry, carried, body_parameter=body_parameter) for entry in exc.errors()]


def _finding(entry: Mapping[str, object], document: object, *, body_parameter: bool) -> Diagnostic:
    """One finding as the generated model, with its pointer into the document.

    A body that never became a document — a JSON syntax error — names no field. What
    Pydantic reports for it is the character offset it stopped at, a place in the
    text rather than a pointer into a body, so the finding carries no path at all:
    the reading the engine's own ``invalid_json`` rejection uses for the same failure.
    A ``Json[T]`` field can produce the same code for its own value; its location
    names a field rather than an offset, so that pointer is kept.
    """
    code = str(entry.get("type", INVALID_REQUEST))
    location = entry.get("loc")
    malformed_body = (
        body_parameter
        and code == "json_invalid"
        and _segments(location)
        and len(location) == 2
        and location[0] == "body"
        and isinstance(location[1], int)
    )
    return Diagnostic(
        code=code,
        message=str(entry.get("msg", "the value was rejected")),
        path=None
        if malformed_body
        else _pointer(location, document, entry, body_parameter=body_parameter),
    )


_DOCUMENTS: tuple[object, ...] = (
    Request,
    *(value for value in vars(_models).values() if isinstance(value, TypeAliasType)),
)
"""The documents the models declare, which a route may declare as its body.

The request is the document a request body holds; beside it are the named types the
models declare, among them the union aliases a route may declare directly — a bare
:class:`~opportune.Rule`, :class:`~opportune.Selector`, :class:`~opportune.Term` or
:class:`~opportune.LeadTimeMax`. The set is read off the generated module rather than
listed here, so an alias the schema grows is one the walk already resolves.

The generated *classes* are not in it — the filter admits aliases, and ``Request`` is
named beside them — because a class document and an alias can read the same location
and nothing in the location says which one it is: ``{"ids": …}`` is both a bare
:class:`~opportune.Selector` and a document of :class:`~opportune.IdsSelector`'s
shape. With a document to check readings against, the one that lands on the finding's
value decides it, so a class document read with its body — or with ``document=``
named — is served a pointer into itself; a failure that names neither falls back to
the first reading, which for a class document whose first key an alias also reads is
a pointer that names no field of it.
"""


def _pointer(
    location: object,
    document: object,
    entry: Mapping[str, object],
    *,
    body_parameter: bool,
) -> str | None:
    """The RFC 6901 pointer a Pydantic finding's ``loc`` tuple names.

    Pydantic reports the location of a finding in the validator's own terms: a body
    parameter is ``body``, and a union of forms adds the tag of the arm it picked.
    Neither is a key the client wrote, so both are dropped and what is left is
    resolved against the documents the models declare (see :data:`_DOCUMENTS`);
    ``document`` is the body the client sent, or the payload the caller names for a
    failure that carries none, and the reading that lands on the value ``entry`` is
    about is the one served.
    """
    if not _segments(location) or not location:
        return None
    segments = list(location)[1:] if body_parameter and location[0] == "body" else list(location)
    if not segments:
        # The whole body was the finding — an array where the request is expected.
        # `""` is RFC 6901's pointer to the document itself.
        return ""
    return _join(
        _document_segments(
            segments,
            document,
            entry.get("input", _NO_INPUT),
            missing=entry.get("type") == "missing",
        )
    )


def _document_segments(
    segments: Sequence[object], document: object, expected: object, *, missing: bool
) -> Sequence[object]:
    """`segments` with the validator's own tag segments dropped.

    Every document in :data:`_DOCUMENTS` reads the location from each segment it may
    start at and keeps the readings that resolve it completely: a body that nests the
    request under a parameter name — FastAPI's shape for a route with two body
    parameters — is the parameter name followed by the request's own document, and
    the readings from a later segment keep that name in front. A reading that kept
    nothing is dropped: it consumed the whole location as tags, which is a caller's
    own document whose first key happens to be spelled like one of ours, not a body
    that is a form.

    When the failure carries the body, the reading served is the one that lands on
    the value the finding is about, so a caller's own body model wins over a reading
    that would have pointed at a key the client never wrote. Naming a place in the
    body is not enough to decide it: a caller's own document whose key is spelled
    like one of this package's tags can hold that key again one level down, and the
    reading that dropped the outer one still resolves — to a sibling that is fine, or
    to the object the offending field sits inside. Only the value tells the two apart.

    The location as reported is tried first, because a body that spells it out with
    the value the finding is about proves every segment is a key the client wrote —
    nothing at that position was a tag. That check is what keeps a real key spelled
    like a tag beside a sibling holding the same value: in
    ``{"cap": {"limit": "x"}, "limit": "x"}`` the reading that dropped ``cap`` lands
    on the sibling's equal value, while the reported location lands on the field the
    finding is about.

    A field the document does not have yet states the object it belongs to as its
    value, so its place is the reading's *parent*: a missing field's own path names
    nothing, and checking only that would leave every candidate unconfirmed and the
    first one served — a pointer that resolves to nothing whenever an alias root can
    also read the document's own keys.

    A location no reading confirms keeps the first reading, and one no document reads
    at all keeps Pydantic's segments, which is the pointer into a document these
    models do not describe. The readings are examined as they are built and the
    search stops at the first one the body confirms: a request that fails in a
    thousand places would otherwise build every candidate reading for every finding
    before looking at one. A failure that carries no body has nothing to check a
    reading against, so its first reading — the one this returns — is all there is to
    look for.
    """

    def confirmed(reading: Sequence[object], *, identity_only: bool = False) -> bool:
        """Whether `reading` names the field `expected` says the finding is about."""
        return _names(
            document,
            reading[:-1] if missing else reading,
            expected,
            identity_only=identity_only,
        )

    reported = list(segments)
    if document is not None and confirmed(reported, identity_only=missing):
        # The documented location resolves in the body: every segment is a key the
        # client wrote, so no shorter reading can have dropped a tag.
        return reported
    first: list[object] | None = None
    first_equal: list[object] | None = None
    for start in range(len(segments)):
        for root in _DOCUMENTS:
            kept = document_segments(root, segments[start:])
            if not kept:
                continue
            reading = [*segments[:start], *kept]
            if first is None:
                first = reading
                if document is None:
                    return reading
            if confirmed(reading, identity_only=missing):
                return reading
            if missing and first_equal is None and confirmed(reading):
                # A decoded copy has no identical parent; keep its equality match as
                # the fallback while looking for an identical one in this document.
                first_equal = reading
    if missing and first_equal is not None:
        # A decoded copy's parent is only ever equal, never identical, so the equality
        # match is its fallback once the whole document has been walked.
        return first_equal
    if document is not None and confirmed(reported):
        return reported
    return first if first is not None else reported


def _names(
    document: object,
    segments: Sequence[object],
    expected: object,
    *,
    identity_only: bool = False,
) -> bool:
    """Whether `segments` names the place in `document` that holds `expected`.

    `expected` is the value the finding states it is about, or :data:`_NO_INPUT` for
    a failure that states none — a hand-rolled exception shaped like Pydantic's —
    where the path is all there is to check. For a `missing` finding, identity is
    checked first: two equal empty objects can be different parents, and only the
    identical one proves which missing field the finding belongs to. Equality remains
    available for a document decoded separately from the validator's input.
    """
    for segment in segments:
        if isinstance(document, Mapping) and isinstance(segment, str):
            # A JSON object, whose keys `document` alone does not type.
            entries = cast("Mapping[str, object]", document)
            if segment not in entries:
                return False
            document = entries[segment]
        elif isinstance(document, (list, tuple)) and isinstance(segment, int):
            # A JSON array or Python tuple (such as a `Span`), whose elements
            # `document` alone does not type.
            elements = cast("Sequence[object]", document)
            if not -len(elements) <= segment < len(elements):
                return False
            document = elements[segment]
        else:
            return False
    if expected is _NO_INPUT or document is expected:
        return True
    return not identity_only and document == expected


def _join(segments: Sequence[object]) -> str:
    """`segments` as an RFC 6901 pointer, each token escaped."""
    return "".join(f"/{str(part).replace('~', '~0').replace('/', '~1')}" for part in segments)


def _segments(location: object) -> TypeGuard[Sequence[object]]:
    """Whether `location` is a finding's ``loc``: a sequence of path segments."""
    return isinstance(location, (list, tuple))


_NO_INPUT = object()
"""The marker for a finding that states no value of its own.

Pydantic reports the value each finding is about as ``input``, and a ``null`` is one
of the values it can be about, so its absence needs a marker of its own rather than
``None``.
"""

__all__ = [
    "ErrorEnvelope",
    "OpportuneError",
    "error_envelope_from_opportune_error",
    "error_envelope_from_validation_error",
    "http_status_for_error",
]
