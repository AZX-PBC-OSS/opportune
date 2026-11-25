"""The engine a caller builds once, and the free functions over a shared one.

``Engine`` wraps the native engine with the generated Pydantic models, so a
request goes out as ``model_dump_json(by_alias=True, exclude_none=True)`` and an
answer comes back through ``model_validate_json``: Pydantic v2's Rust-backed JSON
core stays on the fast path, and the wire format the Rust layer writes against is
the one the generated models describe.

The module-level functions answer from one lazily built engine. Build your own
``Engine`` and reuse it when you need custom limits or a directory database —
a FastAPI application builds one at startup and stores it on ``app.state``.

The ``*_json`` methods are the same four calls with the answer left as its JSON
text: a FastAPI route that returns ``fastapi.Response(content=..., media_type="application/json")``
hands the engine's own document straight on, instead of parsing it into a model and
writing it back out. Do not pass the string to ``JSONResponse``, which would
re-encode it as a quoted JSON string.

The ``*_async`` methods are those calls awaited off the event loop: an ``async def``
route awaits one instead of running the CPU-bound search on the loop's own thread.
Each takes an optional keyword-only ``executor``: ``None`` runs the call on the
loop's shared default pool through ``asyncio.to_thread``, and a
``concurrent.futures.Executor`` runs it on that pool instead, so a service can keep
its CPU-bound searches on a dedicated, bounded ``ThreadPoolExecutor``.
"""

# The native module is private by design: `opportune` is its public surface, and
# this module is that surface for the engine — the one place a Python caller's
# engine is built. Naming its class anywhere else would be a second wrapper.
# pyright: reportPrivateUsage=false

import asyncio
import json
import os
import threading
from collections.abc import Callable, Mapping
from concurrent.futures import Executor
from functools import lru_cache
from typing import Any, cast

from pydantic import BaseModel, TypeAdapter, ValidationError
from pydantic_core import PydanticSerializationError

import opportune._opportune as _native
from opportune._models import (
    AvailableWorkingHoursOutcome,
    CalendarOutcome,
    Diagnostics,
    GraphWorkingHoursOutcome,
    Limits,
    MatchAll,
    Proposal,
    RangesQuery,
    Request,
    RequiredRanges,
    Response,
    Selector,
    SlotCheck,
)
from opportune.errors import (
    INVALID_LIMITS,
    INVALID_REQUEST,
    OpportuneError,
    configuration_rejection,
    error_envelope_from_validation_error,
    rejected_argument,
    rejection,
    translating,
)

type Payload = str | bytes | Mapping[str, object]
"""A provider payload: its JSON text, its UTF-8 bytes, or a parsed JSON object.

The last form is what ``httpx2.Response.json()`` returns, so an adapter call can
take the response object's body directly.
"""

type SelectorInput = Selector | Mapping[str, Any] | MatchAll
"""A selector a helper takes: the generated model, or the form a document states.

A selector is one of the generated form models, and each one reads the mapping the
wire states and the `true` that selects everyone. A helper therefore takes both
spellings — a recipe, a JSON column, an HTTP body and a test all hold the document
— and the model validates the mapping into the form it names, exactly as it does
for a request field. ``Any`` is the honest element type: the value is a caller's
JSON, read before any field has looked at it.
"""

_CALENDAR_OUTCOME: TypeAdapter[CalendarOutcome] = TypeAdapter(CalendarOutcome)
_SCHEDULE_OUTCOMES: TypeAdapter[dict[str, CalendarOutcome]] = TypeAdapter(
    dict[str, CalendarOutcome]
)
_WORKING_HOURS_OUTCOME: TypeAdapter[GraphWorkingHoursOutcome] = TypeAdapter(
    GraphWorkingHoursOutcome
)


def _json(value: BaseModel, code: str = "invalid_json") -> str:
    """The wire text for a generated model.

    ``by_alias`` writes the names the wire format uses (``schema`` for
    ``Meta.schema_``, ``from`` for ``ZonePeriod.from_``); ``exclude_none`` leaves
    optional fields absent so the wire layer applies its own documented defaults
    rather than reading a JSON ``null``.

    A document holding a string with no UTF-8 spelling — a lone surrogate, which
    ``json.loads`` and a ``surrogateescape`` decode both hand back — is the
    package's rejection rather than the serializer's: the encoder's failure is
    the same argument the extension refuses, one step earlier.

    ``code`` classifies that rejection, for the same reason :func:`_as_model` takes
    one: a document a caller sent is ``invalid_json``, which an application answers
    ``422``, while the limits an engine is built from are the deployment's own
    configuration and are answered as a server failure.
    """
    try:
        return value.model_dump_json(by_alias=True, exclude_none=True)
    except PydanticSerializationError as cause:
        raise rejection(
            code, f"the document is not UTF-8 text, so it has no JSON spelling: {cause}"
        ) from cause


def _mapping_text(mapping: Mapping[str, object], what: str) -> str:
    """`mapping` as JSON text, or the package's rejection when it has no JSON spelling.

    A mapping that is circular or nested past the encoder's own depth is no more
    writable than one holding a value JSON cannot spell, and both are the caller's
    document rather than the server's failure: the recursion the encoder raises is
    reported here instead of escaping as a bare ``RecursionError``.

    Every key has to be text. ``json.dumps`` would otherwise coerce a key of another
    type — ``{1: "interview"}`` becomes ``{"1": "interview"}`` — and hand the adapter
    a document the caller never wrote, as silently as ``dict()`` would read a list of
    pairs. A mapping a loop or a database built with non-text keys is refused here for
    the same reason a wrong value is.
    """
    document = dict(mapping)
    for key in document:
        # Python callers are not type-checked, so the key type is checked here.
        if not isinstance(key, str):  # pyright: ignore[reportUnnecessaryIsInstance]
            raise rejection(
                "invalid_argument",
                f"{what} has a key that is not text: {key!r}",
            )
    try:
        return json.dumps(document)
    except (TypeError, ValueError, RecursionError) as cause:
        raise rejection(
            "invalid_json", f"{what} is not a mapping JSON can write: {cause}"
        ) from cause


def _payload_text(payload: Payload) -> str:
    """The JSON text of a provider payload, whatever form it arrived in.

    A payload that is not UTF-8, not serialisable, or not one of the three forms a
    payload has is the package's own rejection rather than the decoder's or the
    encoder's: an adapter called from an exception handler written for
    :class:`~opportune.OpportuneError` must not see a bare ``UnicodeDecodeError``,
    ``TypeError`` or ``ValueError`` instead.

    The mapping check is what keeps a list of pairs from reaching ``dict()``, which
    would quietly read ``[("value", [])]`` as ``{"value": []}`` and hand the
    adapter a payload the caller never wrote.
    """
    if isinstance(payload, Mapping):
        return _mapping_text(payload, "the payload")
    return _text_payload(payload)


def _text_payload(payload: object) -> str:
    """A payload that is JSON text already, decoded when it arrived as UTF-8 bytes.

    A value that is neither is the package's rejection: the parameter is `object`
    rather than the payload's own type because a caller who never saw the
    signature — a framework handler, a JSON column — is exactly who reaches here.
    """
    if isinstance(payload, str):
        return payload
    if isinstance(payload, bytes):
        try:
            return payload.decode("utf-8")
        except UnicodeDecodeError as cause:
            raise rejection(
                "invalid_json",
                f"the payload is not UTF-8 text, so it has no JSON spelling: {cause}",
            ) from cause
    raise rejection(
        "invalid_argument",
        f"the payload is neither JSON text nor a JSON object: {type(payload).__name__}",
    )


def _text(value: object, what: str) -> str:
    """`value` as the text `what` is, or the package's rejection naming its type.

    A zone name, a rule ID, a stored document and a database directory are all read
    by the native layer as UTF-8 text, so a value that is not text would leave the
    package as `PyO3`'s own ``TypeError`` — which an ``except OpportuneError``
    handler never sees.
    """
    if not isinstance(value, str):
        raise rejection("invalid_argument", f"{what} is not text: {type(value).__name__}")
    return value


def _directory(tzdb_dir: str | os.PathLike[str] | None) -> str | None:
    """The directory `tzdb_dir` names, as text, or `None` for the bundled database.

    A ``pathlib.Path`` is what a configured application has, so the argument is any
    path-like object. The native layer reads the directory name as UTF-8 text, and
    ``os.fspath`` of a bytes ``PathLike`` is bytes, which has no spelling there: it
    is rejected as the package's own failure rather than as `PyO3`'s ``TypeError``.

    ``os.fspath`` itself raises ``TypeError`` for a value that is neither text nor
    path-like — a settings object holding a number, say — so that failure is the
    package's rejection too: this is the boundary an application's own
    ``except OpportuneError`` covers, and it must not be the one place a raw
    ``TypeError`` escapes it.
    """
    if tzdb_dir is None:
        return None
    try:
        spelled = os.fspath(tzdb_dir)
    except TypeError as cause:
        raise rejection(
            "invalid_argument",
            f"the time zone directory is not a path: {type(tzdb_dir).__name__}",
        ) from cause
    # A `PathLike[bytes]` spells a bytes directory at runtime, while the annotation
    # promises a text path, so the text check is a real one: `_text` reads an
    # `object` and raises the package's rejection for whatever is not text.
    return _text(spelled, "the time zone directory")


def _as_model[Model: BaseModel](value: object, model: type[Model], what: str, code: str) -> Model:
    """`value` as `model`: the model itself, or the mapping a caller holds it as.

    A document that arrived as a queue message, a fixture file or a database
    column is a mapping a caller should not have to rebuild by hand, and a value
    that is neither the model nor a mapping is answered with the package's
    rejection rather than with the serialiser's ``AttributeError`` on the next line.

    ``code`` is what the rejection is classified as, because the documents this
    package reads for a caller have opposite owners: a request, proposal or query a
    route forwards is the caller's document — ``invalid_request``, which an
    application answers ``422`` with the findings naming each field to change —
    while the limits an engine is built from are the deployment's own configuration.

    :raises OpportuneError: with code ``code`` when the mapping is not a `model` and
        when the value is neither. A refused document carries the findings
        :func:`~opportune.error_envelope_from_validation_error` makes from a
        validation failure, each with the pointer of the field it is about.
    """
    if isinstance(value, model):
        return value
    if not isinstance(value, Mapping):
        raise rejection(
            code,
            f"{what} is neither a {model.__name__} nor a mapping: {type(value).__name__}",
        )
    document = cast("Mapping[str, object]", value)
    try:
        return model.model_validate(document)
    except ValidationError as cause:
        findings = error_envelope_from_validation_error(cause, document=document).errors
        first = findings[0]
        # The sentence names the pointer the findings carry, not Pydantic's own
        # location: that one still holds the tag a union added to say which arm it
        # picked, so it names a key the caller never wrote.
        where = first.path or "the document"
        raise OpportuneError(
            code,
            f"{what} is not a {model.__name__} (`{where}`: {first.message})",
            path=None,
            errors=findings,
        ) from cause


_REQUEST_ONLY_SPELLINGS: frozenset[str] = frozenset(
    spelling
    for name, field in Request.model_fields.items()
    if name not in RangesQuery.model_fields
    for spelling in (name, field.alias)
    if spelling is not None
)
"""Every spelling of a field a request carries that a ranges query does not.

A request says what to look for, when, and on whose calendars; everything else it
carries is about the search that follows — the occurrences a cap counts, the named
calendars, the offer and explain limits. The set is read off the two models rather
than listed here, so a field either one gains is accounted for by the union.
"""


def _query(value: object) -> RangesQuery:
    """`value` as the query :meth:`Engine.required_ranges` reads.

    ``now``, ``goal``, ``participants`` and ``rules`` are the whole of what a query
    reads, and a request carries all four beside the fields only the search that
    follows reads, so a caller holding one hands it over as it is: the query's own
    fields are read out of it and the rest is left behind, as the TypeScript
    binding's ``requiredRanges(request)`` does with the same document.

    A mapping is read as whichever document it is. One carrying a request-only
    field is validated as the request and projected, so a request-only value the
    engine would refuse is refused here rather than dropped unread; one carrying
    none is a query, and a key neither model declares is still the query's own
    finding rather than something to strip.
    """
    if isinstance(value, RangesQuery):
        return value
    if isinstance(value, Request):
        return _query_of_request(value)
    if not isinstance(value, Mapping):
        raise rejection(
            INVALID_REQUEST,
            "the ranges query is neither a RangesQuery nor a Request nor a mapping: "
            f"{type(value).__name__}",
        )
    document = dict(cast("Mapping[str, object]", value))
    if _REQUEST_ONLY_SPELLINGS.isdisjoint(document):
        return _as_model(document, RangesQuery, "the ranges query", INVALID_REQUEST)
    return _query_of_request(_as_model(document, Request, "the request", INVALID_REQUEST))


def _query_of_request(request: Request) -> RangesQuery:
    """The query a request answers: the four fields the two documents share."""
    return RangesQuery(
        now=request.now,
        goal=request.goal,
        participants=request.participants,
        rules=request.rules,
    )


def _answer[Answer](text: str, read: Callable[[str], Answer], what: str) -> Answer:
    """The engine's answer read as `read` reads it, or the package's server failure.

    The engine writes every answer from the same schema these models are generated
    from, so an answer one of them refuses is a defect in this build rather than a
    caller's document — a new status, a field the writer dropped, a ``null`` the
    model has no arm for. It is classified ``serialization_failed``, which
    :func:`~opportune.http_status_for_error` answers ``500`` without the message or
    the pointer, and the model's own findings — the field it could not read and why —
    stay on the ``__cause__`` for a log.

    ``read`` is a model's ``model_validate_json`` or a ``TypeAdapter``'s
    ``validate_json``: both take the answer's JSON text and are Rust-backed, so the
    parse stays on the fast path the rest of this module uses. Without this wrapper a
    schema mismatch would escape as a bare ``pydantic.ValidationError``, which a
    handler written for :class:`~opportune.OpportuneError` — the one exception the
    package promises — never sees.

    :raises OpportuneError: with code ``serialization_failed`` when `text` is not the
        document `read` reads.
    """
    try:
        return read(text)
    except ValidationError as cause:
        findings = cause.errors(include_url=False)
        if not findings:
            raise rejection(
                "serialization_failed",
                f"the engine's {what} is not the document this package reads",
            ) from cause
        finding = findings[0]
        where = "/".join(str(part) for part in finding["loc"]) or "the document"
        raise rejection(
            "serialization_failed",
            f"the engine's {what} is not the document this package reads "
            f"(`{where}`: {finding['msg']})",
        ) from cause


async def _off_loop[Result](
    call: Callable[..., Result], *arguments: object, executor: Executor | None = None
) -> Result:
    """``call(*arguments)`` run off the event loop, on ``executor`` when one is given.

    The search and the other calls are CPU-bound, and every native call releases the
    GIL while it works, so a worker thread genuinely runs beside the loop. Without an
    executor the work goes to ``asyncio.to_thread`` and the loop's shared default
    pool, which is right for a route or two; a service that knows its own load passes
    a dedicated, bounded ``ThreadPoolExecutor`` so searches cannot be queued behind
    another library's blocking work, and cannot starve the loop's default pool.
    """
    if executor is None:
        return await asyncio.to_thread(call, *arguments)
    return await asyncio.get_running_loop().run_in_executor(executor, call, *arguments)


class Engine:
    """An engine: the time zone database and the limits, built once and shared.

    Every thread may call one instance: the native engine is frozen, and every
    call releases the GIL around its Rust work, so calls run in parallel rather
    than taking turns.

    >>> import opportune
    >>> engine = opportune.Engine()
    >>> bool(engine.tzdb_version)
    True
    """

    _inner: _native._Engine

    def __init__(
        self,
        tzdb_dir: str | os.PathLike[str] | None = None,
        tzdb_version: str | None = None,
        limits: Limits | Mapping[str, object] | None = None,
    ) -> None:
        """Build the engine.

        :param tzdb_dir: A directory of ``TZif`` files to resolve zones against —
            a ``str`` or any path-like object such as a ``pathlib.Path`` — or
            ``None`` for the database bundled into the extension.
        :param tzdb_version: The version label ``tzdb_dir`` reports, used when the
            directory has no ``+VERSION`` file. Passing one without ``tzdb_dir``
            raises :class:`OpportuneError`, since the bundled database reports its
            own version.
        :param limits: The request-size limits, or the mapping one is read from, or
            ``None`` for the engine's defaults.
        :raises OpportuneError: when the database cannot be built, the version is
            missing, the limits are not the shape the engine reads, or a version
            label is given with no directory. Every one of them is this
            deployment's own configuration, so it is answered as a server failure
            rather than as a caller's ``422``.
        """
        limits_json = (
            None
            if limits is None
            else _json(_as_model(limits, Limits, "the limits", INVALID_LIMITS), INVALID_LIMITS)
        )
        version = (
            None if tzdb_version is None else _text(tzdb_version, "the time zone database version")
        )
        try:
            self._inner = _native._Engine(_directory(tzdb_dir), version, limits_json)
        except _native.OpportuneError as rejected:
            raise configuration_rejection(rejected) from rejected

    def __repr__(self) -> str:
        """The engine's database version: what a log line about an engine needs."""
        return f"Engine(tzdb_version={self.tzdb_version!r})"

    @property
    def tzdb_version(self) -> str:
        """The version of the time zone database, as ``meta.tzdb`` reports it."""
        return self._inner.tzdb_version

    def find_availability(self, request: Request | Mapping[str, object]) -> Response:
        """Every candidate start the request's calendars, rules and caps allow.

        ``request`` is the generated model, or the mapping it is read from.

        :raises OpportuneError: when the request is rejected. Its findings land in
            ``.errors``, each with the pointer of the field it is about.
        """
        return _answer(
            self.find_availability_json(request), Response.model_validate_json, "response"
        )

    async def find_availability_async(
        self,
        request: Request | Mapping[str, object],
        *,
        executor: Executor | None = None,
    ) -> Response:
        """The same answer as :meth:`find_availability`, awaited off the event loop.

        An ``async def`` route awaits this instead of calling the synchronous
        method: the search is CPU-bound, so the call runs on a worker thread and the
        loop keeps serving. Every native call releases the GIL while it works, so the
        worker genuinely runs beside the loop rather than taking turns with it.

        ``executor`` runs the search on a pool of the caller's choosing — a bounded
        ``ThreadPoolExecutor`` owned by the service — instead of the loop's shared
        default one, so high-load deployments isolate their CPU-bound searches;
        ``None`` keeps the default ``asyncio.to_thread`` behaviour.
        """
        return await _off_loop(self.find_availability, request, executor=executor)

    def find_availability_json(self, request: Request | Mapping[str, object]) -> str:
        """The same answer as :meth:`find_availability`, as its JSON text.

        The fast path for ``fastapi.Response(content=..., media_type="application/json")``:
        the engine's answer is handed on as the document it already is, so a server
        never parses it into ``Response`` and writes it back out under the GIL. Do
        not pass this text to ``JSONResponse``, which would re-encode it as a JSON
        string literal.

        :raises OpportuneError: when the request is rejected.
        """
        with translating():
            return self._inner.find_availability_json(
                _json(_as_model(request, Request, "the request", INVALID_REQUEST))
            )

    async def find_availability_json_async(
        self,
        request: Request | Mapping[str, object],
        *,
        executor: Executor | None = None,
    ) -> str:
        """The same text as :meth:`find_availability_json`, awaited off the event loop."""
        return await _off_loop(self.find_availability_json, request, executor=executor)

    def check_slot(
        self,
        request: Request | Mapping[str, object],
        proposal: Proposal | Mapping[str, object],
    ) -> SlotCheck:
        """Judge one proposed booking against the same request.

        :raises OpportuneError: when either document is rejected. A proposal that
            does not fit the request — an unknown part, the wrong number of terms
            or hosts — names the proposal field at fault in ``.path``.
        """
        return _answer(
            self.check_slot_json(request, proposal), SlotCheck.model_validate_json, "slot check"
        )

    async def check_slot_async(
        self,
        request: Request | Mapping[str, object],
        proposal: Proposal | Mapping[str, object],
        *,
        executor: Executor | None = None,
    ) -> SlotCheck:
        """The same judgement as :meth:`check_slot`, awaited off the event loop."""
        return await _off_loop(self.check_slot, request, proposal, executor=executor)

    def check_slot_json(
        self,
        request: Request | Mapping[str, object],
        proposal: Proposal | Mapping[str, object],
    ) -> str:
        """The same judgement as :meth:`check_slot`, as its JSON text.

        :raises OpportuneError: when either document is rejected.
        """
        with translating():
            return self._inner.check_slot_json(
                _json(_as_model(request, Request, "the request", INVALID_REQUEST)),
                _json(_as_model(proposal, Proposal, "the proposal", INVALID_REQUEST)),
            )

    async def check_slot_json_async(
        self,
        request: Request | Mapping[str, object],
        proposal: Proposal | Mapping[str, object],
        *,
        executor: Executor | None = None,
    ) -> str:
        """The same text as :meth:`check_slot_json`, awaited off the event loop."""
        return await _off_loop(self.check_slot_json, request, proposal, executor=executor)

    def validate(self, request: Request | Mapping[str, object]) -> Diagnostics:
        """The request's static errors and warnings.

        :raises OpportuneError: when the document is rejected.
        """
        return _answer(self.validate_json(request), Diagnostics.model_validate_json, "diagnostics")

    async def validate_async(
        self, request: Request | Mapping[str, object], *, executor: Executor | None = None
    ) -> Diagnostics:
        """The same diagnostics as :meth:`validate`, awaited off the event loop."""
        return await _off_loop(self.validate, request, executor=executor)

    def validate_json(self, request: Request | Mapping[str, object]) -> str:
        """The same diagnostics as :meth:`validate`, as their JSON text.

        :raises OpportuneError: when the document is rejected.
        """
        with translating():
            return self._inner.validate_json(
                _json(_as_model(request, Request, "the request", INVALID_REQUEST))
            )

    async def validate_json_async(
        self, request: Request | Mapping[str, object], *, executor: Executor | None = None
    ) -> str:
        """The same text as :meth:`validate_json`, awaited off the event loop."""
        return await _off_loop(self.validate_json, request, executor=executor)

    def required_ranges(
        self, query: RangesQuery | Request | Mapping[str, object]
    ) -> RequiredRanges:
        """The calendar and occurrence data a request needs, before fetching any.

        ``query`` is the ranges query, the request it was projected from, or the
        mapping either is read from.

        The returned hulls begin a two-phase fetch, not a one-pass query: a cap
        merges every record sharing an event key across all supplied time ranges.
        With ``occurrences`` present, fetch the initial calendar and
        occurrence-range data, collect the event keys observed on every
        participant's own-span occurrence-range records, then fetch every record
        carrying one of those keys on every participant -- all-day and otherwise
        nonmatching records included -- and evaluate that closed request against a
        consistent snapshot. A one-pass fetch is key-complete only when every copy
        of an event has the identical own span; otherwise fetch all occurrences
        and blocks across the full supported time domain or fail closed. See
        ``RequiredRanges`` in the Rust documentation for the whole contract.

        :raises OpportuneError: when the document is rejected.
        """
        return _answer(
            self.required_ranges_json(query), RequiredRanges.model_validate_json, "ranges"
        )

    async def required_ranges_async(
        self,
        query: RangesQuery | Request | Mapping[str, object],
        *,
        executor: Executor | None = None,
    ) -> RequiredRanges:
        """The same ranges as :meth:`required_ranges`, awaited off the event loop."""
        return await _off_loop(self.required_ranges, query, executor=executor)

    def required_ranges_json(self, query: RangesQuery | Request | Mapping[str, object]) -> str:
        """The same ranges as :meth:`required_ranges`, as their JSON text.

        The two-phase fetch contract :meth:`required_ranges` describes applies to
        these ranges unchanged.

        :raises OpportuneError: when the document is rejected.
        """
        with translating():
            return self._inner.required_ranges_json(_json(_query(query)))

    async def required_ranges_json_async(
        self,
        query: RangesQuery | Request | Mapping[str, object],
        *,
        executor: Executor | None = None,
    ) -> str:
        """The same text as :meth:`required_ranges_json`, awaited off the event loop."""
        return await _off_loop(self.required_ranges_json, query, executor=executor)

    def calendar_view_to_blocks(
        self,
        payload: Payload,
        participant_zone: str,
        category_map: Mapping[str, str] | None = None,
    ) -> CalendarOutcome:
        """Convert a Graph ``calendarView`` response into blocks or ``unavailable``.

        An unreadable payload is an ``unavailable`` outcome, never an empty
        calendar: an empty block list would read as "completely free". Zone names
        resolve against this engine's database, so a payload and the request it
        feeds are read in one database.

        :raises OpportuneError: when the participant zone is blank or the category
            map is not an object of tag paths.
        """
        if category_map is None:
            category_map_json = None
        elif not isinstance(category_map, Mapping):  # pyright: ignore[reportUnnecessaryIsInstance]
            raise rejection(
                "invalid_argument",
                f"the category map is not a mapping: {type(category_map).__name__}",
            )
        else:
            category_map_json = _mapping_text(category_map, "the category map")
        with translating():
            answer = self._inner.graph_calendar_view_to_blocks_json(
                _payload_text(payload),
                _text(participant_zone, "the participant zone"),
                category_map_json,
            )
        return _answer(answer, _CALENDAR_OUTCOME.validate_json, "calendar outcome")

    def get_schedule_to_blocks(self, payload: Payload) -> dict[str, CalendarOutcome]:
        """Convert a Graph ``getSchedule`` response into outcomes keyed by schedule ID.

        A schedule the provider reported an error for is an ``unavailable``
        outcome; the healthy schedules in the same batch are still converted.

        :raises OpportuneError: when the document is not a ``getSchedule`` response.
        """
        with translating():
            answer = self._inner.graph_get_schedule_to_blocks_json(_payload_text(payload))
        return _answer(answer, _SCHEDULE_OUTCOMES.validate_json, "schedule outcomes")

    def working_hours_to_rule(
        self,
        payload: Payload,
        rule_id: str = "working_hours",
        *,
        who: SelectorInput | None = None,
        when: SelectorInput | None = None,
    ) -> GraphWorkingHoursOutcome:
        """Convert one mailbox's Graph ``workingHours`` into a ``window`` rule.

        A custom Graph time zone is an ``unavailable`` outcome rather than an
        exception, so a caller falls back to its own window without catching one;
        the blocks converted from the same schedule response are unaffected. Zone
        names resolve against this engine's database, as they do for the blocks.

        ``rule_id`` is the ID the rule takes; ``who`` and ``when`` replace the rule's
        default selectors. The rule is replaced as a validated whole when either is
        given, so the returned outcome always holds a rule the engine would accept.

        :raises OpportuneError: when ``who`` or ``when`` is not a selector the rule
            accepts, as the package's own rejection rather than as the model's
            ``pydantic.ValidationError``, whose findings stay on the ``__cause__``,
            and when the engine's time zone database is unusable.
        """
        with translating():
            answer = self._inner.graph_working_hours_to_rule_json(
                _payload_text(payload), _text(rule_id, "the rule ID")
            )
        outcome = _answer(answer, _WORKING_HOURS_OUTCOME.validate_json, "working hours outcome")
        if not isinstance(outcome, AvailableWorkingHoursOutcome) or (who is None and when is None):
            return outcome
        replacements: dict[str, object] = {}
        if who is not None:
            replacements["who"] = who
        if when is not None:
            replacements["when"] = when
        try:
            outcome.rule = with_updates(outcome.rule, **replacements)
        except ValidationError as cause:
            raise rejection("invalid_argument", rejected_argument(cause)) from cause
        return outcome

    def events_list_to_blocks(self, payload: Payload, participant_zone: str) -> CalendarOutcome:
        """Convert a Google ``events.list`` response into blocks or ``unavailable``.

        Zone names resolve against this engine's database, so a payload and the
        request it feeds are read in one database.

        :raises OpportuneError: when the participant zone is blank.
        """
        with translating():
            answer = self._inner.google_events_list_to_blocks_json(
                _payload_text(payload), _text(participant_zone, "the participant zone")
            )
        return _answer(answer, _CALENDAR_OUTCOME.validate_json, "calendar outcome")

    def free_busy_to_blocks(self, payload: Payload) -> dict[str, CalendarOutcome]:
        """Convert a Google ``freeBusy.query`` response into outcomes keyed by calendar ID.

        A calendar the provider reported an error for is an ``unavailable``
        outcome; the healthy calendars are still converted.

        :raises OpportuneError: when the document is not a ``freeBusy.query``
            response.
        """
        with translating():
            answer = self._inner.google_free_busy_to_blocks_json(_payload_text(payload))
        return _answer(answer, _SCHEDULE_OUTCOMES.validate_json, "free/busy outcomes")


@lru_cache(maxsize=1)
def _default_limits_template() -> Limits:
    """The engine's defaults, validated once from the native document.

    Building a :class:`Limits` validates ten fields, and :func:`limits_with` reads the
    default document on every override, so the validated template is kept. It is never
    handed out: :func:`default_limits` copies it, because a Pydantic model is mutable
    and a caller that wrote one field would otherwise write it for every later caller.
    """
    return Limits.model_validate_json(_native.DEFAULT_LIMITS_JSON)


def default_limits() -> Limits:
    """The engine's own default request-size limits.

    These are the limits an engine built with no ``limits`` argument applies, read
    from the Rust crate's ``Limits::default()`` rather than restated here, so
    ``Engine(limits=limits_with(...))`` starts from exactly what it replaces.

    The model returned is the caller's own: the validated document is kept, and every
    call deep-copies it, so an assignment to one caller's ``Limits`` — which
    ``validate_assignment`` allows — is not visible to the next.
    """
    return _default_limits_template().model_copy(deep=True)


def limits_with(**overrides: object) -> Limits:
    """The default limits with ``overrides`` applied, validated.

    >>> import opportune
    >>> opportune.limits_with(max_search_window_days=120).max_search_window_days
    120

    :raises pydantic.ValidationError: when an override is not a limit the engine
        reads or is outside its range.
    """
    return with_updates(default_limits(), **overrides)


def with_updates[Model: BaseModel](model: Model, **changes: object) -> Model:
    """A validated copy of ``model`` with the named fields replaced.

    ``model_copy(update=...)`` skips validation, so a field set through it can hold
    a value the model would never have accepted; this validates the merged document
    instead, which is what makes an override safe.

    A field is named either by its Python name or by its wire alias, whichever the
    caller holds: ``model_dump()`` writes the Python name, so replacing
    ``Meta.schema_`` by its alias would otherwise present both spellings of one field
    and Pydantic would read the spare as an extra key. The replaced field's other
    spelling is dropped before the change is merged.

    :raises pydantic.ValidationError: when a replacement, or the model it leaves
        behind, is not a value the model accepts.
    """
    document = model.model_dump()
    for name, field in type(model).model_fields.items():
        spellings = [name, *([field.alias] if field.alias is not None else [])]
        replaced = [spelling for spelling in spellings if spelling in changes]
        if not replaced:
            continue
        for spare in spellings:
            if spare not in replaced:
                document.pop(spare, None)
    return type(model).model_validate({**document, **changes})


_shared_engine: Engine | None = None
_DEFAULT_ENGINE_LOCK = threading.Lock()


def default_engine() -> Engine:
    """The engine the module-level functions share, built on first use.

    Building it under a lock means concurrent first calls construct exactly one
    engine: constructing one loads the whole bundled time zone database.
    """
    global _shared_engine
    engine = _shared_engine
    if engine is None:
        with _DEFAULT_ENGINE_LOCK:
            if _shared_engine is None:
                _shared_engine = Engine()
            engine = _shared_engine
    return engine


def bound_engine(engine: Engine | None) -> Engine:
    """The engine a call runs on: the caller's own, or the shared one.

    Every adapter wrapper takes an optional engine for the reason the module-level
    functions take none: a caller with its own database and limits passes it, and
    every other caller gets the shared engine without having to build one.

    :raises OpportuneError: with code ``invalid_argument`` when ``engine`` is
        neither ``None`` nor an :class:`Engine` instance.
    """
    if engine is None:
        return default_engine()
    if not isinstance(engine, Engine):  # pyright: ignore[reportUnnecessaryIsInstance]
        raise rejection(
            "invalid_argument",
            f"`engine` must be an Engine instance, not {type(engine).__name__}",
        )
    return engine


def find_availability(request: Request | Mapping[str, object]) -> Response:
    """Every candidate start the request allows, from the shared engine."""
    return default_engine().find_availability(request)


async def find_availability_async(
    request: Request | Mapping[str, object], *, executor: Executor | None = None
) -> Response:
    """The same answer as :func:`find_availability`, awaited off the event loop."""
    return await default_engine().find_availability_async(request, executor=executor)


def find_availability_json(request: Request | Mapping[str, object]) -> str:
    """The same answer as :func:`find_availability`, as its JSON text."""
    return default_engine().find_availability_json(request)


async def find_availability_json_async(
    request: Request | Mapping[str, object], *, executor: Executor | None = None
) -> str:
    """The same text as :func:`find_availability_json`, awaited off the event loop."""
    return await default_engine().find_availability_json_async(request, executor=executor)


def check_slot(
    request: Request | Mapping[str, object], proposal: Proposal | Mapping[str, object]
) -> SlotCheck:
    """Judge one proposed booking against the request, from the shared engine."""
    return default_engine().check_slot(request, proposal)


async def check_slot_async(
    request: Request | Mapping[str, object],
    proposal: Proposal | Mapping[str, object],
    *,
    executor: Executor | None = None,
) -> SlotCheck:
    """The same judgement as :func:`check_slot`, awaited off the event loop."""
    return await default_engine().check_slot_async(request, proposal, executor=executor)


def check_slot_json(
    request: Request | Mapping[str, object], proposal: Proposal | Mapping[str, object]
) -> str:
    """The same judgement as :func:`check_slot`, as its JSON text."""
    return default_engine().check_slot_json(request, proposal)


async def check_slot_json_async(
    request: Request | Mapping[str, object],
    proposal: Proposal | Mapping[str, object],
    *,
    executor: Executor | None = None,
) -> str:
    """The same text as :func:`check_slot_json`, awaited off the event loop."""
    return await default_engine().check_slot_json_async(request, proposal, executor=executor)


def validate(request: Request | Mapping[str, object]) -> Diagnostics:
    """The request's static errors and warnings, from the shared engine."""
    return default_engine().validate(request)


async def validate_async(
    request: Request | Mapping[str, object], *, executor: Executor | None = None
) -> Diagnostics:
    """The same diagnostics as :func:`validate`, awaited off the event loop."""
    return await default_engine().validate_async(request, executor=executor)


def validate_json(request: Request | Mapping[str, object]) -> str:
    """The same diagnostics as :func:`validate`, as their JSON text."""
    return default_engine().validate_json(request)


async def validate_json_async(
    request: Request | Mapping[str, object], *, executor: Executor | None = None
) -> str:
    """The same text as :func:`validate_json`, awaited off the event loop."""
    return await default_engine().validate_json_async(request, executor=executor)


def required_ranges(query: RangesQuery | Request | Mapping[str, object]) -> RequiredRanges:
    """The data a request needs, from the shared engine.

    ``query`` is the ranges query, the request it was projected from, or the mapping
    either is read from. The returned hulls require the two-phase, all-participant
    event-key closure :meth:`Engine.required_ranges` describes; a one-pass cap fetch
    is safe only under the identical-own-span invariant.
    """
    return default_engine().required_ranges(query)


async def required_ranges_async(
    query: RangesQuery | Request | Mapping[str, object], *, executor: Executor | None = None
) -> RequiredRanges:
    """The same ranges as :func:`required_ranges`, awaited off the event loop."""
    return await default_engine().required_ranges_async(query, executor=executor)


def required_ranges_json(query: RangesQuery | Request | Mapping[str, object]) -> str:
    """The same ranges and two-phase fetch contract as :func:`required_ranges`, as JSON."""
    return default_engine().required_ranges_json(query)


async def required_ranges_json_async(
    query: RangesQuery | Request | Mapping[str, object], *, executor: Executor | None = None
) -> str:
    """The same text as :func:`required_ranges_json`, awaited off the event loop."""
    return await default_engine().required_ranges_json_async(query, executor=executor)


def _schema_version(from_version: object) -> int:
    """`from_version` as the schema version an upgrade reads, or the package's rejection.

    The native functions take a ``u32``, so a version this package passed on
    unchecked would leave as `PyO3`'s own ``OverflowError`` or ``TypeError`` —
    which an ``except OpportuneError`` handler never sees.
    """
    if not isinstance(from_version, int) or isinstance(from_version, bool):
        raise rejection(
            "invalid_argument",
            f"the schema version is not an integer: {from_version!r}",
        )
    if not 0 <= from_version <= 0xFFFF_FFFF:
        raise rejection(
            "invalid_argument",
            f"the schema version is outside the range an upgrade reads: {from_version}",
        )
    return from_version


def upgrade_rule(rule: str, from_version: int) -> str:
    """Rewrite a stored rule as the current schema version's canonical JSON.

    :raises OpportuneError: with code ``unsupported_schema_version`` when this
        build does not read ``from_version``, the parser's own failure when the
        document is not a valid rule of that version, and ``invalid_argument``
        when ``from_version`` is not an integer a schema version can be.
    """
    with translating():
        return _native.upgrade_rule_json(
            _text(rule, "the stored rule"), _schema_version(from_version)
        )


def upgrade_calendar(calendar: str, from_version: int) -> str:
    """Rewrite a stored calendar as the current schema version's canonical JSON.

    :raises OpportuneError: for the same reasons :func:`upgrade_rule` raises.
    """
    with translating():
        return _native.upgrade_calendar_json(
            _text(calendar, "the stored calendar"), _schema_version(from_version)
        )


def upgrade_goal(goal: str, from_version: int) -> str:
    """Rewrite a stored goal as the current schema version's canonical JSON.

    :raises OpportuneError: for the same reasons :func:`upgrade_rule` raises.
    """
    with translating():
        return _native.upgrade_goal_json(
            _text(goal, "the stored goal"), _schema_version(from_version)
        )


__all__ = [
    "Engine",
    "Payload",
    "SelectorInput",
    "check_slot",
    "check_slot_async",
    "check_slot_json",
    "check_slot_json_async",
    "default_limits",
    "find_availability",
    "find_availability_async",
    "find_availability_json",
    "find_availability_json_async",
    "limits_with",
    "required_ranges",
    "required_ranges_async",
    "required_ranges_json",
    "required_ranges_json_async",
    "upgrade_calendar",
    "upgrade_goal",
    "upgrade_rule",
    "validate",
    "validate_async",
    "validate_json",
    "validate_json_async",
    "with_updates",
]
