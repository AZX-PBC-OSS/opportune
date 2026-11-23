"""The generated models served by a real FastAPI application.

This is the application an integration guide would write: an ``Engine`` built once
by the app's lifespan, a route that declares :class:`~opportune.Request` and
:class:`~opportune.Response` and returns the model, a second route that hands the
engine's own document straight back as the response body, an ``async def`` route
that awaits the engine off the event loop, a public booking route that serves the
projection a visitor may see, a second one that serves the projection of the answer the
app's cache holds rather than of a live search, and three exception handlers that serve
one :class:`~opportune.ErrorEnvelope` for a rejected request, for a request the request
model could not read, and for a change the package's own validation refused.

That second route answers with a plain ``Response`` carrying the engine's text and
the JSON media type: a ``JSONResponse`` would take the document as a *string* and
encode it again, so the body would be a JSON string holding JSON. Passing the text
through untouched is the whole point of the ``*_json`` fast path, and the test hits
it with the same request as the typed route and compares the two bodies.

The claims here are the ones an application gets wrong when the package only
half-supports it: the typed route's JSON is byte-for-byte the document the engine
wrote even where an ``excluded`` reason omits an optional field; a finding's
pointer names the field of the document a client sent and nothing of the
validator's own bookkeeping, so one resolver finds it whether the engine or the
request model rejected the request; the OpenAPI document a generated model
produces is useful (field descriptions, the span's tuple shape, the declared 422
envelope, the integer bound the models enforce); a bad field inside a
discriminated rule and a misspelled selector key are each one error at that field
rather than one per union member; both handlers serve the same envelope shape; two
apps keep their own engine configuration; and an adapter conversion's rejection
reaches the same handler; a public booking route serves the offers and none of the
operator's own detail however much the client asks the explanation to say, the
projection of a stored answer document is the one the typed call returns, and a stored
document the models cannot read reaches the same handler and is served as the envelope
rather than a traceback; and a ``pydantic.ValidationError`` from ``with_updates`` is
served as the same envelope as either of the other two.

The handler answers a rejection at the status :func:`~opportune.http_status_for_error`
classifies it as, which is what keeps a deployment's own failure out of a client's
``422``: a broken time zone database or a stored document this build cannot read is
answered ``500`` with none of its message or path.
"""

import asyncio
import json
import re
from collections.abc import AsyncGenerator, Callable
from contextlib import asynccontextmanager
from typing import Any, cast

import httpx2
import pytest
from fastapi import FastAPI
from fastapi import Request as HttpRequest
from fastapi.exceptions import RequestValidationError
from fastapi.responses import JSONResponse
from fastapi.responses import Response as HttpResponse
from fastapi.testclient import TestClient
from pydantic import BaseModel, TypeAdapter, ValidationError
from pydantic import Json as PydanticJson

from opportune import (
    CalendarOutcome,
    Diagnostic,
    Engine,
    ErrorEnvelope,
    LeadTimeMax,
    Limits,
    OpportuneError,
    Proposal,
    PublicResponse,
    PublicSlotCheck,
    Request,
    Requirement,
    Response,
    Rule,
    Selector,
    SlotCheck,
    Term,
    default_limits,
    error_envelope_from_opportune_error,
    error_envelope_from_validation_error,
    http_status_for_error,
    presets,
    public_response,
    public_slot_check,
    upgrade_rule,
    with_updates,
)

REQUEST: dict[str, Any] = {
    "now": "2027-03-09T15:00Z",
    "goal": {
        "parts": [{"duration": "30m", "requirement": {"all": [{"person": "ana"}]}}],
        "search_window": ["2027-03-09T15:00Z", "2027-03-09T18:00Z"],
        "zone": "UTC",
    },
    "participants": [{"id": "ana", "zone": "UTC", "calendar": []}],
}
"""One 30-minute booking for one participant over three empty hours."""

EXPLAINED: dict[str, Any] = {
    **REQUEST,
    "participants": [
        {
            "id": "ana",
            "zone": "UTC",
            "calendar": [{"span": ["2027-03-09T16:00Z", "2027-03-09T16:30Z"], "status": "busy"}],
        }
    ],
    "rules": [
        {
            "id": "business_hours",
            "kind": "window",
            "params": {"weekly": {"mon,tue,wed,thu,fri,sat,sun": ["09:00-17:00"]}, "zone": "UTC"},
        }
    ],
    "explain": {"limit": 10},
}
"""The same three hours with a busy block and a window rule, so starts are excluded.

Every excluded reason leaves at least one optional field absent (``event_key``,
``params``, ``rule``, ``participant``), which is the shape a typed route diverges on
when the response model writes ``null`` for them and the raw route does not.
"""

BAD_CAP: dict[str, Any] = {
    "id": "interviews",
    "kind": "cap",
    "params": {
        "count": {"tag": "interview"},
        "limit": -1,
        "measure": "count",
        "per": "each",
        "period": "day",
        "zone": "participant",
    },
}
"""A cap whose one bad field is the limit; every other member of the union is valid."""

WINDOW_RULE: dict[str, Any] = {
    "id": "business_hours",
    "kind": "window",
    "params": {"weekly": {"mon": ["09:00-17:00"]}, "zone": "UTC"},
}
"""A window rule with no selector of its own, so a test names only the one it is about."""

UNKNOWN_ZONE: dict[str, Any] = {
    **REQUEST,
    "participants": [{"id": "ana", "zone": "Mars/Olympus_Mons", "calendar": []}],
}
"""The same request with its one participant in a zone no database has."""

SURROGATE_ID: dict[str, Any] = {
    **REQUEST,
    "participants": [{"id": "\ud800", "zone": "UTC", "calendar": []}],
}
"""The same request with an ID that has no UTF-8 spelling in its JSON text."""

PUBLIC_EXPLAINED: dict[str, Any] = {**EXPLAINED, "explain": {"limit": 1000}}
"""The explained request asking for every exclusion the engine will report.

A public booking route can be sent this as readily as an operator route can, so the
projection is what has to keep the internal detail out of the answer.
"""

OPERATOR_FIELDS = frozenset(
    {
        "warnings",
        "excluded",
        "excluded_truncated",
        "eligible",
        "assignment",
        "reserve",
        "reasons",
    }
)
"""Every field a public projection leaves behind, written out rather than read off it.

A test that iterated the projection's own fields would stay green if one were added
back, which is the one mistake this list has to catch.
"""

CALENDAR_VIEW: dict[str, Any] = {
    "value": [
        {
            "id": "AAMkAD",
            "isAllDay": False,
            "showAs": "busy",
            "start": {"dateTime": "2027-03-09T09:00:00.0000000", "timeZone": "UTC"},
            "end": {"dateTime": "2027-03-09T10:00:00.0000000", "timeZone": "UTC"},
        }
    ]
}
"""A healthy Graph ``calendarView`` response: one busy hour."""

REJECTED_DOCUMENT: dict[int | str, dict[str, Any]] = {422: {"model": ErrorEnvelope}}
"""The 422 the routes declare, so the served document describes the real envelope."""


@asynccontextmanager
async def lifespan(app: FastAPI) -> AsyncGenerator[None]:
    """Build the one engine the app serves with, and drop it with the app."""
    app.state.engine = Engine()
    yield


def create_app(engine: Engine | None = None) -> FastAPI:
    """The application described in the module docstring.

    ``engine`` is a pre-built engine for a caller that already has one: an app with
    two configurations builds one per app, and a test driving the app over an ASGI
    transport has no lifespan to build one. Every other caller passes none, and the
    lifespan builds the engine the app serves with.

    The stored schema version is the app's own configuration: it is the version of
    the documents this deployment wrote, which is what an upgrade has to be told.
    """
    app = FastAPI(lifespan=None if engine is not None else lifespan)
    if engine is not None:
        app.state.engine = engine
    app.state.stored_schema_version = 1
    app.state.cached_answer = None

    def envelope(answer: ErrorEnvelope) -> JSONResponse:
        """The envelope as a body: the one shape both handlers serve."""
        return JSONResponse(status_code=422, content=answer.model_dump())

    @app.exception_handler(OpportuneError)
    async def rejected(_: HttpRequest, error: OpportuneError) -> JSONResponse:
        """The envelope for a rejection the caller can fix, and nothing else.

        A failure of the engine's configuration, of this deployment's stored data or
        of this app's own call is not the client's to fix, and its message and path
        name internals the client can neither see nor act on, so it is answered as
        a server failure that says neither.
        """
        if http_status_for_error(error) != 422:
            return JSONResponse(
                status_code=500,
                content={"code": "internal_error", "message": "the request could not be served"},
            )
        return envelope(error_envelope_from_opportune_error(error))

    @app.exception_handler(RequestValidationError)
    async def unreadable(_: HttpRequest, error: RequestValidationError) -> JSONResponse:
        """The same envelope for a document the request model refused."""
        return envelope(error_envelope_from_validation_error(error))

    @app.exception_handler(ValidationError)
    async def refused(_: HttpRequest, error: ValidationError) -> JSONResponse:
        """The same envelope for a change this package's own validation refused.

        ``with_updates`` revalidates the document it merges into, so a route applying
        a client's field change to a stored record meets Pydantic's own failure rather
        than the engine's — and a client that wrote a value no model accepts is the
        same client, reading the same envelope, whichever of the three raised.
        """
        return envelope(error_envelope_from_validation_error(error))

    @app.post("/availability", response_model_exclude_none=True, responses=REJECTED_DOCUMENT)
    def availability(request: Request, http: HttpRequest) -> Response:
        return http.app.state.engine.find_availability(request)

    @app.post("/availability/public", response_model_exclude_none=True, responses=REJECTED_DOCUMENT)
    def availability_public(request: Request, http: HttpRequest) -> PublicResponse:
        """The same search, projected for the visitor a booking link was sent to."""
        return public_response(http.app.state.engine.find_availability(request))

    @app.post("/availability/async", response_model_exclude_none=True, responses=REJECTED_DOCUMENT)
    async def availability_async(request: Request, http: HttpRequest) -> Response:
        return await http.app.state.engine.find_availability_async(request)

    @app.post("/availability/raw", responses=REJECTED_DOCUMENT)
    def availability_raw(request: Request, http: HttpRequest) -> HttpResponse:
        return HttpResponse(
            content=http.app.state.engine.find_availability_json(request),
            media_type="application/json",
        )

    @app.post("/availability/cached", response_model_exclude_none=True, responses=REJECTED_DOCUMENT)
    def availability_cached(request: Request, http: HttpRequest) -> PublicResponse:
        """The projection of the answer the app's cache holds, not of a live search.

        The cache is filled once with the engine's own JSON text and read back per
        hit, so what this route projects is a mapping — the document as stored,
        which the helper has to read rather than a model it is handed.
        """
        if http.app.state.cached_answer is None:
            http.app.state.cached_answer = http.app.state.engine.find_availability_json(request)
        return public_response(json.loads(http.app.state.cached_answer))

    @app.post("/check-slot", response_model_exclude_none=True, responses=REJECTED_DOCUMENT)
    def check_slot(request: Request, proposal: Proposal, http: HttpRequest) -> SlotCheck:
        """Judge a proposed booking: the confirmation step before one is written."""
        return http.app.state.engine.check_slot(request, proposal)

    @app.post("/check-slot/public", response_model_exclude_none=True, responses=REJECTED_DOCUMENT)
    def check_slot_public(
        request: Request, proposal: Proposal, http: HttpRequest
    ) -> PublicSlotCheck:
        """The same judgement, projected for the visitor confirming the booking."""
        return public_slot_check(http.app.state.engine.check_slot(request, proposal))

    @app.post("/rules/clone", responses=REJECTED_DOCUMENT)
    def clone_rule(overrides: dict[str, Any]) -> dict[str, str]:
        """The stored business-hours rule with the client's field changes applied.

        ``with_updates`` validates the merged document rather than copying fields
        over it, so a change no model accepts raises Pydantic's own failure here —
        the boundary the third exception handler serves.
        """
        rule = presets.business_hours("mon-fri", "09:00-17:00")
        return {"rule": with_updates(rule, **overrides).model_dump_json(by_alias=True)}

    @app.post("/rules/upgrade", responses=REJECTED_DOCUMENT)
    def upgrade(document: dict[str, Any], http: HttpRequest) -> dict[str, str]:
        """Rewrite a stored rule document this deployment wrote, as a read does."""
        return {"rule": upgrade_rule(json.dumps(document), http.app.state.stored_schema_version)}

    @app.get("/window/{days}", responses=REJECTED_DOCUMENT)
    def window(days: int, limit: int = 1) -> dict[str, int]:
        """An availability window, whose parameters arrive in the path and the query."""
        return {"days": days, "limit": limit}

    @app.post("/calendar-view", response_model_exclude_none=True, responses=REJECTED_DOCUMENT)
    def calendar_view(
        payload: dict[str, Any], http: HttpRequest, zone: str = "UTC"
    ) -> CalendarOutcome:
        return http.app.state.engine.calendar_view_to_blocks(payload, zone)

    return app


def test_the_typed_route_answers_with_the_engines_own_database_version() -> None:
    app = create_app()
    with TestClient(app) as client:
        answer = client.post("/availability", json=REQUEST)
        assert answer.status_code == 200, answer.text
        body = answer.json()
        assert body["meta"]["tzdb"] == app.state.engine.tzdb_version

    assert body["meta"]["schema"] == 1, "the served body names the wire schema version"
    assert next(offer["events"][0]["start"] for offer in body["offers"]) == "2027-03-09T15:00Z"
    assert body["truncated"] is False


def test_the_raw_route_answers_the_document_the_engine_wrote() -> None:
    """The fast path passes the engine's text on; it never re-encodes the answer."""
    app = create_app()
    with TestClient(app) as client:
        raw = client.post("/availability/raw", json=REQUEST)
        typed = client.post("/availability", json=REQUEST)
        assert raw.status_code == 200, raw.text
        assert raw.text == app.state.engine.find_availability_json(Request.model_validate(REQUEST))

    assert raw.headers["content-type"] == "application/json"
    assert json.loads(raw.text) == typed.json(), "both routes answer the same document"


def test_a_typed_route_with_exclusions_answers_the_document_the_raw_route_does() -> None:
    """Every optional field an excluded reason omits has to stay omitted.

    ``Reason.event_key``, ``params``, ``participant`` and ``rule`` are absent from
    the engine's document when they do not apply, and a response model that writes
    them as ``null`` hands a client a different document from the raw route's. The
    comparison is against the engine's own text, so a route that grew a
    ``response_model_exclude_none`` back out fails here rather than in a client.
    """
    app = create_app()
    with TestClient(app) as client:
        typed = client.post("/availability", json=EXPLAINED)
        raw = client.post("/availability/raw", json=EXPLAINED)

    assert typed.status_code == 200, typed.text
    assert raw.status_code == 200, raw.text
    assert typed.json() == json.loads(raw.text)
    assert "null" not in raw.text, "the engine's document omits what does not apply"

    excluded = typed.json()["excluded"]
    assert {reason["code"] for entry in excluded for reason in entry["reasons"]} == {
        "blocked",
        "outside_window",
    }
    blocked = next(
        reason for entry in excluded for reason in entry["reasons"] if reason["code"] == "blocked"
    )
    assert set(blocked) == {"participant", "code", "tags"}, "no absent field came back as null"


def _keys(value: object) -> set[str]:
    """Every key the JSON value `value` holds, at any depth."""
    if isinstance(value, dict):
        entries = cast("dict[str, object]", value)
        return set(entries) | {key for inner in entries.values() for key in _keys(inner)}
    if isinstance(value, list):
        return {key for inner in cast("list[object]", value) for key in _keys(inner)}
    return set()


def test_a_public_route_serves_no_operator_field_however_much_is_explained() -> None:
    """The explanation an operator asks for is what a booking link's visitor must not see.

    ``{"explain": {"limit": 1000}}`` asks the engine to report every start it did not
    offer, each with the participant, the block and the rule that kept it out of the
    list, and every offer says which hosts could fill each term and which were
    chosen. A visitor needs the offers and nothing of that, so the public route
    serves :func:`~opportune.public_response`'s projection. The operator route is
    compared in the same test: every field the projection drops is present there,
    which is what proves the projection — and not the request — removed it.
    """
    app = create_app()
    with TestClient(app) as client:
        public = client.post("/availability/public", json=PUBLIC_EXPLAINED)
        operator = client.post("/availability", json=PUBLIC_EXPLAINED)

    assert public.status_code == 200, public.text
    assert operator.status_code == 200, operator.text
    leaked = _keys(public.json()) & OPERATOR_FIELDS
    assert not leaked, f"a public answer carries the operator's own fields: {sorted(leaked)}"
    assert OPERATOR_FIELDS <= _keys(operator.json()), (
        "the operator route reports every one of these, so the request did ask for them"
    )

    body = public.json()
    assert body["offers"], "the visitor is still offered the times"
    assert set(body) == {"dates", "meta", "offers", "truncated"}
    assert set(body["offers"][0]) == {"events", "flags"}
    assert set(body["offers"][0]["events"][0]) == {"end", "part", "start"}


def test_a_public_route_projects_a_stored_answer_document_without_operator_fields() -> None:
    """A route serving a cached answer projects the document as stored.

    A route that caches the engine's answer as JSON hands the helper a mapping,
    not the model it was handed the first time, so the projection must be the
    same one the typed call returns — and no operator field may come back with
    it merely because the fields passed through a store.
    """
    app = create_app()
    with TestClient(app) as client:
        cached = client.post("/availability/cached", json=PUBLIC_EXPLAINED)
        typed = client.post("/availability/public", json=PUBLIC_EXPLAINED)

    assert cached.status_code == 200, cached.text
    assert cached.json() == typed.json(), "the stored document projects as the model does"
    leaked = _keys(cached.json()) & OPERATOR_FIELDS
    assert not leaked, f"a public answer carries the operator's own fields: {sorted(leaked)}"


def test_a_public_route_answers_the_envelope_for_a_stored_document_it_cannot_read() -> None:
    """A stored document an older deployment wrote under another shape is the app's data.

    A cache holds the engine's text from whenever it was written, so a route can
    hand the helper a document whose shape this build's models no longer
    describe. Before the helpers read mappings that raised a bare
    ``AttributeError`` and escaped as a 500 traceback; now the package's own
    rejection reaches the handler, which answers the failure the classification
    in :mod:`opportune.errors` gives a stored document — a server failure that
    says neither the message nor a field of the store.
    """
    app = create_app()
    with TestClient(app) as client:
        assert client.post("/availability/cached", json=PUBLIC_EXPLAINED).status_code == 200
        stored = json.loads(app.state.cached_answer)
        del stored["offers"]
        app.state.cached_answer = json.dumps(stored)
        answer = client.post("/availability/cached", json=PUBLIC_EXPLAINED)

    # An escaping AttributeError would propagate out of the TestClient rather
    # than arrive as this body: the handler served the package's rejection.
    assert answer.status_code == 500, answer.text
    assert answer.json() == {"code": "internal_error", "message": "the request could not be served"}
    assert "offers" not in answer.text, "the store's own fields stay out of the answer"


def test_a_public_confirmation_serves_the_verdict_without_why_it_failed() -> None:
    """A failed confirmation's reasons name the policies and the other participants."""
    proposal = {"events": [{"part": "main", "start": "2027-03-09T16:00Z", "assignment": [["ana"]]}]}
    app = create_app()
    with TestClient(app) as client:
        public = client.post(
            "/check-slot/public", json={"request": EXPLAINED, "proposal": proposal}
        )
        operator = client.post("/check-slot", json={"request": EXPLAINED, "proposal": proposal})

    assert public.status_code == 200, public.text
    assert operator.status_code == 200, operator.text
    assert operator.json()["valid"] is False
    assert operator.json()["reasons"], "the operator learns why the booking is refused"

    body = public.json()
    assert set(body) == {"flags", "meta", "valid"}
    assert body["valid"] is False, "the visitor still learns the booking cannot be made"


def test_a_validation_error_from_with_updates_is_served_as_the_same_envelope() -> None:
    """A package boundary that raises Pydantic's failure is the same client, same body.

    ``with_updates`` revalidates the document it merges into, so a route applying a
    client's field change to a stored rule meets :class:`pydantic.ValidationError`
    rather than the engine's rejection. Registering
    :func:`~opportune.error_envelope_from_validation_error` for it serves the one
    envelope, with the finding pointing at the key the client wrote.
    """
    app = create_app()
    with TestClient(app) as client:
        answer = client.post("/rules/clone", json={"id": 5})

    assert answer.status_code == 422, answer.text
    body = answer.json()
    assert set(body) == {"code", "message", "path", "errors"}
    assert body["code"] == "invalid_request"
    assert [(finding["code"], finding["path"]) for finding in body["errors"]] == [
        ("string_type", "/id")
    ]


def test_an_async_route_answers_over_an_asgi_transport() -> None:
    """A route written ``async def`` awaits the engine and answers the same document."""
    app = create_app(Engine())

    async def drive() -> httpx2.Response:
        transport = httpx2.ASGITransport(app=app)
        async with httpx2.AsyncClient(transport=transport, base_url="http://test") as client:
            return await client.post("/availability/async", json=EXPLAINED)

    answer = asyncio.run(drive())

    assert answer.status_code == 200, answer.text
    assert answer.json() == json.loads(app.state.engine.find_availability_json(EXPLAINED))
    assert answer.json()["meta"]["schema"] == 1


def test_the_served_selector_schema_is_the_wire_documents_any_of() -> None:
    """Which arm validates is a validator's business; the document states the wire.

    The published wire schema — the generated `schema.json` the TypeScript types and
    the engine's own goldens come from — describes a selector as `anyOf` over its
    forms under its own title. Keying the union so one arm reports is a
    validation decision, and a keyed union renders as `oneOf` and drops the alias's
    title, which leaves the served document describing the validator instead of the
    wire and disagreeing with the bundle a client may generate its types from.
    """
    app = create_app()
    with TestClient(app) as client:
        document = client.get("/openapi.json").json()

    selector = document["components"]["schemas"]["Selector"]
    assert "oneOf" not in selector, selector
    assert selector["title"] == "Selector", selector
    assert {entry["$ref"] for entry in selector["anyOf"]} == {
        f"#/components/schemas/{arm}"
        for arm in (
            "MatchAll",
            "TagSelector",
            "AnySelector",
            "AllSelector",
            "NotSelector",
            "IdsSelector",
            "RoleSelector",
            "KindSelector",
        )
    }, selector


def test_the_openapi_document_carries_the_field_prose_the_span_shape_and_the_envelope() -> None:
    """A generated model's schema is the API document, so its prose has to survive.

    A description is not prose when it reaches a client as the docstring's own
    markup: the envelope is this package's model rather than the schema's, so its
    fields state descriptions of their own and none of the source's cross-reference
    markup is served.
    """
    app = create_app()
    with TestClient(app) as client:
        answer = client.get("/openapi.json")

    assert answer.status_code == 200, answer.text
    document = answer.json()
    schemas = document["components"]["schemas"]

    assert schemas["Request"]["properties"]["now"]["description"].strip(), (
        "the current instant's description reaches the served schema"
    )
    span = schemas["Goal"]["properties"]["search_window"]
    assert span["$ref"] == "#/components/schemas/Span"
    assert schemas["Span"]["prefixItems"] == [{"type": "string"}, {"type": "string"}], (
        "a two-instant span is a tuple in the schema, not an unconstrained array"
    )
    assert schemas["Span"]["minItems"] == schemas["Span"]["maxItems"] == 2

    assert set(schemas["ErrorEnvelope"]["properties"]) == {"code", "message", "path", "errors"}
    envelope = schemas["ErrorEnvelope"]
    assert envelope["description"].strip(), "the envelope's own prose reaches the document"
    undescribed = [
        name for name, schema in envelope["properties"].items() if not schema.get("description", "")
    ]
    assert not undescribed, f"these envelope fields describe nothing to a client: {undescribed}"
    markup = re.findall(r":[a-z]+:`[^`]*`", json.dumps(document))
    assert not markup, f"documentation markup reached the served schema: {markup}"
    assert document["paths"]["/availability"]["post"]["responses"]["422"]["content"][
        "application/json"
    ]["schema"] == {"$ref": "#/components/schemas/ErrorEnvelope"}


# A JSON value: the model schemas this suite reads are nested maps, and the walkers
# below need their keys typed rather than `Any` to stay inside the project's strict
# pyright.
type Json = str | int | float | bool | list["Json"] | dict[str, "Json"] | None


def _defaulted_properties(value: Json, pointer: str, found: dict[str, dict[str, Json]]) -> None:
    """Collects every property schema in `value` that carries a `default`."""
    if isinstance(value, dict):
        for key, inner in value.items():
            if key == "properties" and isinstance(inner, dict):
                for field, schema in inner.items():
                    if isinstance(schema, dict) and "default" in schema:
                        found[f"{pointer}/properties/{field}"] = schema
            _defaulted_properties(inner, f"{pointer}/{key}", found)
    elif isinstance(value, list):
        for index, inner in enumerate(value):
            _defaulted_properties(inner, f"{pointer}/{index}", found)


def _admits_null(schema: dict[str, Json]) -> bool:
    """Whether `schema` admits `null`: a type that names it, or an arm that does."""
    types = schema.get("type")
    if types == "null" or (isinstance(types, list) and "null" in types):
        return True
    for keyword in ("anyOf", "oneOf"):
        arms = schema.get(keyword)
        if isinstance(arms, list) and any(
            _admits_null(arm) for arm in arms if isinstance(arm, dict)
        ):
            return True
    return False


def test_the_served_openapi_admits_null_wherever_a_field_defaults() -> None:
    """The served document and the published bundle describe one format.

    The bundle marks every field the wire reads an explicit `null` as the default of
    with a null arm, and the models this app serves are generated from that bundle,
    so the served document has to carry the same arm. Otherwise a client generated
    from the API document accepts a document the published bundle rejects, or the
    other way round, and one of the two is the artefact its consumer was told to
    trust. The field set is derived from the served document itself: a `default` is
    what makes a field one the specification defaults.
    """
    schemas = create_app().openapi()["components"]["schemas"]

    defaulted: dict[str, dict[str, Json]] = {}
    for name, schema in schemas.items():
        _defaulted_properties(schema, name, defaulted)
    assert len(defaulted) > 20, f"only {len(defaulted)} defaulted fields were found"

    silent = [pointer for pointer, schema in defaulted.items() if not _admits_null(schema)]
    assert not silent, (
        f"these fields default but their served schema has no null arm: {silent}; "
        "the published bundle states one, so the two documents disagree"
    )

    # One defaulted field pinned by name, so the walk above cannot pass by finding
    # nothing that matters: the served `participants` offers the array a caller writes
    # and the `null` a binding that fills every key writes.
    participants = schemas["Request"]["properties"]["participants"]
    assert participants["anyOf"][0]["type"] == "array", participants
    assert _admits_null(participants), participants


def _findings(answer: httpx2.Response) -> list[dict[str, Any]]:
    """The findings of the 422 `answer`'s envelope."""
    body: dict[str, Any] = answer.json()
    assert set(body) == {"code", "message", "path", "errors"}, body
    findings: list[dict[str, Any]] = body["errors"]
    return findings


def test_a_bad_field_inside_a_discriminated_rule_is_one_error_at_that_field() -> None:
    """The union explosion is what the schema's discriminator exists to stop."""
    app = create_app()
    with TestClient(app) as client:
        answer = client.post("/availability", json={**REQUEST, "rules": [BAD_CAP]})

    assert answer.status_code == 422, answer.text
    findings = _findings(answer)
    assert len(findings) == 1, f"one bad field is one finding, not one per union member: {findings}"
    # The rule's `kind` tag is the validator's bookkeeping, so the pointer names the
    # field the client wrote and nothing between it and the rule.
    assert findings[0]["path"] == "/rules/0/params/limit"
    assert findings[0]["code"] == "greater_than_equal"


CAP_LIMIT_PAST_U32: dict[str, Any] = {
    **BAD_CAP,
    "params": {**BAD_CAP["params"], "limit": 2**32},
}
"""A cap whose `u32` limit is one past the width the wire states."""


def test_a_wire_integer_past_its_width_is_a_finding_at_the_field_the_client_wrote() -> None:
    """An oversized `u32` is bounded by the model, so the client learns which key to change.

    `CapParams.limit` is a `u32` whose schema states only the minimum, so before the
    generated model carried the width's upper bound an oversized limit validated here
    and was refused by serde: the answer was a 422 whose envelope held no finding at
    all, whose path named the whole rule, and whose message was the engine's reader
    talking about `u32` and a column. The bound turns it into the same finding a
    negative limit already produces, at the key the client wrote.
    """
    app = create_app()
    document = {**REQUEST, "rules": [CAP_LIMIT_PAST_U32]}
    with TestClient(app) as client:
        answer = client.post("/availability", json=document)

    assert answer.status_code == 422, answer.text
    body = answer.json()
    assert body["code"] == "invalid_request", body
    assert [(finding["code"], finding["path"]) for finding in _findings(answer)] == [
        ("less_than_equal", "/rules/0/params/limit")
    ], body
    assert _resolve(document, "/rules/0/params/limit") == 2**32


@pytest.mark.parametrize(
    ("weight", "expected_code"),
    [
        (2**31, "less_than_equal"),
        (-(2**31) - 1, "greater_than_equal"),
    ],
    ids=["past-i32-max", "past-i32-min"],
)
def test_a_request_weight_outside_i32_is_a_finding_at_its_participant_pointer(
    weight: int,
    expected_code: str,
) -> None:
    """An out-of-range `Request.weights` entry fails at FastAPI/Pydantic with `/weights/{id}`."""
    app = create_app()
    document = {**REQUEST, "weights": {"ana": weight}}
    with TestClient(app) as client:
        answer = client.post("/availability", json=document)

    assert answer.status_code == 422, answer.text
    body = answer.json()
    assert body["code"] == "invalid_request", body
    assert [(finding["code"], finding["path"]) for finding in _findings(answer)] == [
        (expected_code, "/weights/ana")
    ], body
    assert _resolve(document, "/weights/ana") == weight


@pytest.mark.parametrize(
    "weight",
    [2**31 - 1, -(2**31)],
    ids=["i32-max", "i32-min"],
)
def test_a_request_weight_at_either_i32_boundary_reaches_the_engine(weight: int) -> None:
    """`i32::MAX` and `i32::MIN` weights validate cleanly and are answered by the engine."""
    app = create_app()
    with TestClient(app) as client:
        answer = client.post("/availability", json={**REQUEST, "weights": {"ana": weight}})

    assert answer.status_code == 200, answer.text
    assert answer.json()["offers"]


def test_the_served_openapi_states_the_weight_bound_the_models_enforce() -> None:
    """The API document a client generates its request type from states the weight range.

    A weight is constrained at the map's values, so the bound is one level inside the
    field: a served document that stated the value type without its range would let a
    client generated from it build a weight the server refuses, and the client would
    learn the range only from a `422`. The models enforce it either way, so nothing
    else here would notice the two documents disagreeing.
    """
    weights = create_app().openapi()["components"]["schemas"]["Request"]["properties"]["weights"]
    assert any(arm.get("type") == "object" for arm in weights["anyOf"]), weights
    map_arm = next(arm for arm in weights["anyOf"] if arm.get("type") == "object")
    values = map_arm["additionalProperties"]

    assert values["type"] == "integer", values
    assert values["minimum"] == -(2**31), values
    assert values["maximum"] == 2**31 - 1, values


@pytest.mark.parametrize(
    ("who", "code", "needle"),
    [
        ({"idz": ["ana"]}, "selector_form_unknown", "`idz`"),
        (["ana"], "selector_not_a_form", "`list`"),
        ({"ids": ["ana"], "tag": "x"}, "selector_forms", "names 2 keys"),
    ],
    ids=["misspelled-key", "not-a-mapping", "two-forms"],
)
def test_a_malformed_selector_is_one_error_naming_the_forms(
    who: object, code: str, needle: str
) -> None:
    """A selector is `true` or a mapping naming its form, so neither mistake is a wall.

    Without the dispatch the union reports every arm's rejection — and a recursive
    arm's rejections of the arms inside it — for both the misspelled key and the
    list, and none of them says what a selector may be.
    """
    app = create_app()
    with TestClient(app) as client:
        answer = client.post(
            "/availability",
            json={**REQUEST, "rules": [{**WINDOW_RULE, "who": who}]},
        )

    assert answer.status_code == 422, answer.text
    findings = _findings(answer)
    assert len(findings) == 1, findings
    assert findings[0]["path"] == "/rules/0/who"
    assert findings[0]["code"] == code
    assert needle in findings[0]["message"], findings[0]["message"]
    assert "tag" in findings[0]["message"], "the message names the forms a caller may write"


@pytest.mark.parametrize(
    ("key", "pointer"),
    [("a/b", "/a~1b"), ("a~b", "/a~0b"), ("a/b~c", "/a~1b~0c")],
    ids=["slash", "tilde", "both"],
)
def test_a_finding_pointer_escapes_the_key_the_client_wrote(key: str, pointer: str) -> None:
    """RFC 6901 escaping is how a client finds the field a finding names.

    An unexpected key is the one segment of a finding's location the caller chooses,
    so a pointer built by joining the segments unescaped names a different field
    than the one that has to change. `~` encodes to `~0` and `/` to `~1`, in that
    order, or a key holding both decodes to the wrong place.
    """
    app = create_app()
    with TestClient(app) as client:
        answer = client.post("/availability", json={**REQUEST, key: 1})

    assert answer.status_code == 422, answer.text
    assert [finding["path"] for finding in _findings(answer)] == [pointer]


NESTED_SELECTOR_TYPOS: list[tuple[dict[str, Any], str]] = [
    ({"any": [{"idz": ["ana"]}]}, "/rules/0/who/any/0"),
    ({"all": [True, {"idz": ["ana"]}]}, "/rules/0/who/all/1"),
    ({"not": {"idz": ["ana"]}}, "/rules/0/who/not"),
    ({"any": [{"any": [{"idz": []}]}]}, "/rules/0/who/any/0/any/0"),
]
"""One typo per recursive form, and one two levels in: where each one sits."""


@pytest.mark.parametrize(
    ("nested", "pointer"),
    NESTED_SELECTOR_TYPOS,
    ids=["any", "all", "not", "any-of-any"],
)
def test_a_selector_typo_nested_in_a_recursive_form_is_one_finding_too(
    nested: dict[str, Any], pointer: str
) -> None:
    """`any`/`all`/`not` hold selectors, so a typo one level in is the same mistake.

    The dispatch reads the inner mapping and raises the one finding that names the
    forms, but every outer arm still reports itself, so the client is told the
    document was rejected in 12 places and no finding says which key a selector may
    have — the union wall this dispatch exists to remove, one level in. The same
    holds for a known key with a bad value (`{"ids": "nope"}`), the spelling a
    caller writes after reading the message the dispatch prints.

    What is pinned is the contract the top-level case already meets: one finding,
    whose pointer names the mapping the caller wrote — through every `any`/`not`
    level and list index between — and whose message names the key and the forms. A
    pointer that stopped at `/rules/0/who`, or that carried one of the validator's
    tags on the way down, would pass a prefix check; the exact path is asserted for
    that reason, including a selector nested two levels in.
    """
    document = {**REQUEST, "rules": [{**WINDOW_RULE, "who": nested}]}
    app = create_app()
    with TestClient(app) as client:
        answer = client.post("/availability", json=document)

    assert answer.status_code == 422, answer.text
    findings = _findings(answer)
    assert len(findings) == 1, findings
    assert [finding["path"] for finding in findings] == [pointer], findings
    landed = _resolve(document, pointer)
    assert isinstance(landed, dict) and "idz" in landed, landed
    assert "`idz`" in findings[0]["message"], findings[0]["message"]
    assert "tag" in findings[0]["message"], "the message names the forms a caller may write"


def test_a_selector_form_that_refuses_its_value_reports_that_form_at_its_field() -> None:
    """The key selects its arm, so the arm's own finding is the only one reported.

    ``ids`` names the ids form, so the list it is given is that form's own failure at
    the field it is about, not a report from each arm the union could have tried. The
    pointer drops the tag the union reports for the form the caller named — it is the
    validator's bookkeeping, not a key — and names the field itself.
    """
    app = create_app()
    with TestClient(app) as client:
        answer = client.post(
            "/availability", json={**REQUEST, "rules": [{**WINDOW_RULE, "who": {"ids": "ana"}}]}
        )

    assert answer.status_code == 422, answer.text
    findings = _findings(answer)
    assert len(findings) == 1, findings
    assert findings[0]["path"] == "/rules/0/who/ids"
    assert findings[0]["code"] == "list_type"


def test_both_handlers_serve_the_one_envelope_shape() -> None:
    """A client reads one 422 contract whichever layer rejected its request."""
    app = create_app()
    broken = json.loads(json.dumps(REQUEST))
    broken["participants"][0]["zone"] = "Mars/Olympus_Mons"
    with TestClient(app) as client:
        from_engine = client.post("/availability", json=broken)
        from_model = client.post("/availability", json={**REQUEST, "rules": [BAD_CAP]})

    assert from_engine.status_code == from_model.status_code == 422
    engine_body = from_engine.json()
    model_body = from_model.json()
    assert set(engine_body) == set(model_body) == {"code", "message", "path", "errors"}
    assert engine_body["code"] == model_body["code"] == "invalid_request"
    assert engine_body["path"] is model_body["path"] is None, (
        "the pointers live on the findings, not on the failure"
    )
    assert [(finding["code"], finding["path"]) for finding in engine_body["errors"]] == [
        ("zone_unknown", "/participants/0/zone")
    ]
    # One resolver finds either layer's finding in the document the client sent: the
    # engine's pointer is a path into the body, and so is the model's.
    assert _resolve(broken, engine_body["errors"][0]["path"]) == "Mars/Olympus_Mons"
    assert _resolve({**REQUEST, "rules": [BAD_CAP]}, model_body["errors"][0]["path"]) == -1


def _resolve(document: Any, pointer: str) -> Any:
    """The value `pointer` names in `document`, with RFC 6901's escapes decoded."""
    for token in pointer.split("/")[1:]:
        key = token.replace("~1", "/").replace("~0", "~")
        if isinstance(document, list):
            document = cast("list[Any]", document)[int(key)]
        elif isinstance(document, dict):
            document = cast("dict[str, Any]", document)[key]
        else:
            raise AssertionError(f"{pointer} names no field of the document")
    return document


WINDOW_WITH_ZONE: dict[str, Any] = {
    "id": "business_hours",
    "kind": "window",
    "params": {"weekly": {"mon": ["09:00-17:00"]}, "zone": 5},
}
"""A window rule whose one bad field is the zone every window rule needs."""

BLACKOUT_OVERLONG_SPAN: dict[str, Any] = {
    "id": "lunch",
    "kind": "blackout",
    "params": {"zone": "UTC", "spans": [["2027-03-09T12:00", "2027-03-09T13:00", "x"]]},
}
"""A blackout whose span is an array of three local date-times rather than two."""

CLEARANCE_WITH_BAD_GAP: dict[str, Any] = {
    "id": "buffers",
    "kind": "clearance",
    "params": {"before": "10m", "after": "10m", "min_gap": 5},
}
"""A clearance rule whose one bad field is the gap it requires."""

LEAD_TIME_WITH_BAD_DAYS: dict[str, Any] = {
    "id": "horizon",
    "kind": "lead_time",
    "params": {"min": "1h", "max": {"days": "x"}},
    "who": True,
}
"""A lead-time rule whose calendar-day maximum is not a number."""

LEAD_TIME_WITH_BAD_WORKING_DAYS: dict[str, Any] = {
    "id": "horizon",
    "kind": "lead_time",
    "params": {"min": "1h", "max": {"calendar": "org", "working_days": -2}},
    "who": True,
}
"""A lead-time rule whose working-day maximum is negative."""

STATUS_POLICY_WITH_UNKNOWN_STATUS: dict[str, Any] = {
    "id": "provisional",
    "kind": "status_policy",
    "params": {"entries": [{"effect": "block", "status": "oof"}]},
}
"""A status-policy entry naming a status the closed set does not carry."""

REST_WITH_BAD_BOUNDARY: dict[str, Any] = {
    "id": "rest",
    "kind": "rest",
    "params": {"late_after": "18:00", "not_before": 5},
}
"""A rest rule whose one bad field is the wall clock it resumes at."""

WINDOW_WITH_NUMERIC_HOUR: dict[str, Any] = {
    "id": "business_hours",
    "kind": "window",
    "params": {"weekly": {"mon": [5]}, "zone": "UTC"},
}
"""A window rule whose Monday states an hour as a number, inside the day map."""

BLACKOUT_WITH_NUMERIC_END: dict[str, Any] = {
    "id": "lunch",
    "kind": "blackout",
    "params": {"zone": "UTC", "spans": [["2027-03-09T12:00", 5]]},
}
"""A blackout whose span ends at a number, the second element of the tuple."""

TERM_WITH_NUMERIC_PERSON: dict[str, Any] = {
    "goal": {
        "parts": [{"duration": "30m", "requirement": {"all": [{"person": 5}]}}],
        "search_window": ["2027-03-09T15:00Z", "2027-03-09T18:00Z"],
        "zone": "UTC",
    }
}
"""A request whose one bad field is the participant a requirement term names."""

NOT_WITH_BAD_IDS: dict[str, Any] = {"who": {"not": {"not": {"ids": "ana"}}}}
"""A window rule whose negated, negated form names its ids as a string."""

ALL_WITH_BAD_IDS: dict[str, Any] = {"who": {"all": [{"tag": "x"}, {"ids": 5}]}}
"""A window rule whose second `all` clause names its ids as a number."""

TAG_WITH_NUMERIC_TAG: dict[str, Any] = {"who": {"tag": 5}}
"""A selector naming the tag form, whose own field is spelled the union's tag."""

STRAY_KEY_IN_RULE: dict[str, Any] = {**WINDOW_RULE, "bogus": 1}
"""A window rule carrying a key no rule reads, the shape a mistyped field name takes."""

STRAY_KEY_IN_WINDOW_PARAMS: dict[str, Any] = {
    **WINDOW_RULE,
    "params": {**WINDOW_RULE["params"], "bogus": 1},
}
"""A window rule whose stray key sits one level below the rule's `kind` tag."""

ZONE_PERIOD_WITH_NUMERIC_FROM: dict[str, Any] = {
    "participants": [
        {
            "id": "ana",
            "zone": "UTC",
            "calendar": [],
            "zone_periods": [{"from": 5, "until": "2027-03-09T17:00Z", "zone": "America/New_York"}],
        }
    ]
}
"""A participant whose zone period states its start, an aliased key, as a number."""

POINTERS: list[tuple[dict[str, Any], str, Any]] = [
    ({"rules": [WINDOW_WITH_ZONE]}, "/rules/0/params/zone", 5),
    (
        {"rules": [BLACKOUT_OVERLONG_SPAN]},
        "/rules/0/params/spans/0",
        [
            "2027-03-09T12:00",
            "2027-03-09T13:00",
            "x",
        ],
    ),
    ({"rules": [CLEARANCE_WITH_BAD_GAP]}, "/rules/0/params/min_gap", 5),
    ({"rules": [BAD_CAP]}, "/rules/0/params/limit", -1),
    ({"rules": [LEAD_TIME_WITH_BAD_DAYS]}, "/rules/0/params/max/days", "x"),
    ({"rules": [LEAD_TIME_WITH_BAD_WORKING_DAYS]}, "/rules/0/params/max/working_days", -2),
    ({"rules": [REST_WITH_BAD_BOUNDARY]}, "/rules/0/params/not_before", 5),
    ({"rules": [WINDOW_WITH_NUMERIC_HOUR]}, "/rules/0/params/weekly/mon/0", 5),
    ({"rules": [BLACKOUT_WITH_NUMERIC_END]}, "/rules/0/params/spans/0/1", 5),
    ({"rules": [STATUS_POLICY_WITH_UNKNOWN_STATUS]}, "/rules/0/params/entries/0/status", "oof"),
    (TERM_WITH_NUMERIC_PERSON, "/goal/parts/0/requirement/all/0/person", 5),
    ({"rules": [{**WINDOW_RULE, **NOT_WITH_BAD_IDS}]}, "/rules/0/who/not/not/ids", "ana"),
    ({"rules": [{**WINDOW_RULE, **ALL_WITH_BAD_IDS}]}, "/rules/0/who/all/1/ids", 5),
    ({"rules": [{**WINDOW_RULE, **TAG_WITH_NUMERIC_TAG}]}, "/rules/0/who/tag", 5),
    ({"rules": [STRAY_KEY_IN_RULE]}, "/rules/0/bogus", 1),
    ({"rules": [STRAY_KEY_IN_WINDOW_PARAMS]}, "/rules/0/params/bogus", 1),
    (ZONE_PERIOD_WITH_NUMERIC_FROM, "/participants/0/zone_periods/0/from", 5),
]
"""One document per rule kind, term and lead-time form, per aliased key, and per stray key.

A stray key is the one finding whose segment is by definition absent from the
models: only the `kind` tag above it is the validator's, so the pointer has to drop
that tag and name the key the client wrote (it is the key's presence that the
`extra_forbidden` finding reports).
"""


@pytest.mark.parametrize(("override", "pointer", "value"), POINTERS)
def test_a_body_finding_points_at_the_field_the_client_wrote(
    override: dict[str, Any], pointer: str, value: Any
) -> None:
    """A finding's pointer resolves in the document the client sent, and only there.

    A frontend resolver walks one RFC 6901 pointer whether the engine or the request
    model rejected the request, so a pointer carrying the validator's own segments —
    the ``body`` parameter, the arm a union picked, the tag a rule's ``kind`` names —
    names a place in no document a client can show. What each case pins is the whole
    path from the request's own key down to the field at fault — through a rule's
    parameters, into a window's day map and a span's second element, around a nested
    selector, and onto the aliased keys a document writes (``not``, ``from``); the
    value is the one the client wrote there, so the pointer is proven to land on it
    rather than merely to start with the right prefix.
    """
    document = {**REQUEST, **override}
    app = create_app()
    with TestClient(app) as client:
        answer = client.post("/availability", json=document)

    assert answer.status_code == 422, answer.text
    findings = _findings(answer)
    assert [finding["path"] for finding in findings] == [pointer], findings
    assert _resolve(document, pointer) == value


def test_a_body_that_is_not_json_names_no_field_at_all() -> None:
    """A syntax error is about the text, not a field, so the finding carries no path.

    Starlette hands the request model bytes it could not parse, and Pydantic reports
    the character offset it stopped at — a place in the text, not an RFC 6901 pointer
    into a body. Serving it as a pointer would send a frontend resolver looking for a
    field named after the offset, and the engine spells the same failure with no path,
    so this one does too.
    """
    app = create_app()
    with TestClient(app) as client:
        answer = client.post(
            "/availability", content=b'{"now": ', headers={"content-type": "application/json"}
        )

    assert answer.status_code == 422, answer.text
    findings = _findings(answer)
    assert [(finding["code"], finding["path"]) for finding in findings] == [("json_invalid", None)]


def test_a_body_that_is_not_an_object_points_at_the_document_as_a_whole() -> None:
    """``""`` is RFC 6901's pointer to the document itself.

    A client that sends an array where the request is expected leaves no field to
    name, so the finding is about the body itself: the one pointer that resolves to
    the document rather than one built from a parameter name the client never wrote.
    The engine's own envelope spells the same reading as a null ``path``, so a
    resolver treats ``""`` as the document and a null ``path`` as no field at all.
    """
    app = create_app()
    with TestClient(app) as client:
        answer = client.post("/availability", json=[1, 2])

    assert answer.status_code == 422, answer.text
    assert [finding["path"] for finding in _findings(answer)] == [""]


def test_a_parameter_outside_the_body_keeps_the_segment_naming_where_it_arrived() -> None:
    """A query or path parameter is not a body field, so its pointer has to say so."""
    app = create_app()
    with TestClient(app) as client:
        answer = client.get("/window/soon", params={"limit": "ten"})

    assert answer.status_code == 422, answer.text
    assert sorted(finding["path"] for finding in _findings(answer)) == [
        "/path/days",
        "/query/limit",
    ]


class _WindowDraft(BaseModel):
    """A caller's own draft of a window rule."""

    id: int


class _StoredRule(BaseModel):
    """A caller's own body: a draft nested under the key its kind is spelled with."""

    window: _WindowDraft


class _LooseDraft(BaseModel):
    """A caller's own body whose only key is spelled like a term's form."""

    person: int


class _CapLimit(BaseModel):
    """A caller's own nested draft, whose field a sibling of its parent shares."""

    limit: int


class _CappedDraft(BaseModel):
    """A caller's own body under a rule kind's name, beside a sibling of that name.

    ``cap`` is the tag a cap rule is selected by and ``limit`` is a field of both
    levels, so the reading that drops ``cap`` as a tag still names a place in the
    body — a place holding a different value than the one the finding is about.
    """

    cap: _CapLimit
    limit: int


class _LimitDraft(BaseModel):
    """A required field whose parent can be equal to another parent."""

    limit: int


class _CapParamsDraft(BaseModel):
    """A nested object under a key that looks like a rule tag."""

    params: _LimitDraft


class _DuplicateParentDraft(BaseModel):
    """Two equal-but-distinct parents for two missing `limit` fields."""

    cap: _CapParamsDraft
    params: _LimitDraft


class _NestedPerson(BaseModel):
    """A caller's own draft nested under the key its own field is spelled with."""

    person: int


class _PersonDraft(BaseModel):
    """A caller's own body whose one key is a term's tag and holds the same key."""

    person: _NestedPerson


def _serving_envelopes(app: FastAPI) -> FastAPI:
    """`app` with the documented handler: one envelope for a refused body."""

    @app.exception_handler(RequestValidationError)
    async def unreadable(_: HttpRequest, error: RequestValidationError) -> JSONResponse:
        return JSONResponse(
            status_code=422, content=error_envelope_from_validation_error(error).model_dump()
        )

    return app


def _a_route_per_document_model() -> FastAPI:
    """An app whose body parameter *is* the document model rather than a request.

    ``Rule``, ``Selector`` and ``Term`` are the package's own documents, so a route
    that validates one — an "is this rule acceptable?" endpoint, a picker handing its
    mapping back — declares it directly instead of nesting it in a ``Request``.
    """
    app = _serving_envelopes(FastAPI())

    @app.post("/rule")
    def rule(document: Rule) -> dict[str, str]:
        return {}

    @app.post("/selector")
    def selector(document: Selector) -> dict[str, str]:
        return {}

    @app.post("/term")
    def term(document: Term) -> dict[str, str]:
        return {}

    @app.post("/max")
    def lead_time_max(document: LeadTimeMax) -> dict[str, str]:
        return {}

    return app


@pytest.mark.parametrize(
    ("route", "document", "pointer", "value"),
    [
        ("/rule", BAD_CAP, "/params/limit", -1),
        ("/rule", {**WINDOW_RULE, "bogus": 1}, "/bogus", 1),
        ("/rule", LEAD_TIME_WITH_BAD_WORKING_DAYS, "/params/max/working_days", -2),
        ("/selector", {"ids": 5}, "/ids", 5),
        ("/term", {"at_least": 2, "of": {"ids": 5}}, "/of/ids", 5),
        ("/max", {"calendar": 5, "working_days": 2}, "/calendar", 5),
    ],
    ids=[
        "rule",
        "rule-with-a-stray-key",
        "rule-nesting-a-lead-time-maximum",
        "selector",
        "term",
        "lead-time-max",
    ],
)
def test_a_body_that_is_the_document_model_points_at_the_field_the_client_wrote(
    route: str, document: dict[str, Any], pointer: str, value: Any
) -> None:
    """The tag a union reports is the validator's, whichever model the body is.

    ``Rule``, ``Selector`` and ``Term`` are documents a route may declare on its own,
    and one resolver reads their findings as it reads a request's: a pointer that
    carries the arm the validator picked (``/cap/params/limit``, ``/ids/ids``) names a
    key the client never wrote, so the field it has to change cannot be found.
    """
    app = _a_route_per_document_model()
    with TestClient(app) as client:
        answer = client.post(route, json=document)

    assert answer.status_code == 422, answer.text
    findings = _findings(answer)
    assert [finding["path"] for finding in findings] == [pointer], findings
    assert _resolve(document, pointer) == value


def test_a_missing_field_of_a_document_model_points_at_the_key_to_add() -> None:
    """A finding about a field the client has not written yet still drops the tag.

    The path a missing field names goes nowhere by definition, so the reading is
    confirmed by its parent — the object the finding states the field belongs to —
    rather than by the field itself. A bare ``Rule`` body therefore keeps the strip it
    did — the ``kind`` tag is consumed and ``id`` stands alone — rather than falling
    back to Pydantic's tag-bearing location now that an unresolvable segment stops a
    whole reading.
    """
    app = _a_route_per_document_model()
    document = {name: value for name, value in WINDOW_RULE.items() if name != "id"}
    with TestClient(app) as client:
        answer = client.post("/rule", json=document)

    assert answer.status_code == 422, answer.text
    assert [(finding["code"], finding["path"]) for finding in _findings(answer)] == [
        ("missing", "/id")
    ]


def test_a_route_with_two_body_parameters_keeps_each_parameters_name() -> None:
    """FastAPI nests each body parameter under its own name, and that name is a key.

    A route taking ``request`` and ``proposal`` reads one object holding both, so the
    client's own document has a ``request`` key above the request it sent. The
    pointer has to keep it and drop only the validator's ``body`` and the rule's
    ``kind`` tag, so a resolver finds ``/request/rules/0/params/limit`` in the body.
    """
    document = {
        "request": {**REQUEST, "rules": [BAD_CAP]},
        "proposal": {
            "events": [{"part": "main", "start": "2027-03-09T15:00Z", "assignment": [["ana"]]}]
        },
    }
    app = create_app()
    with TestClient(app) as client:
        answer = client.post("/check-slot", json=document)

    assert answer.status_code == 422, answer.text
    findings = _findings(answer)
    assert [finding["path"] for finding in findings] == ["/request/rules/0/params/limit"], findings
    assert _resolve(document, "/request/rules/0/params/limit") == -1


def _a_route_per_callers_own_model() -> FastAPI:
    """An app whose bodies are the caller's own models, not the package's.

    ``{"window": {"id": …}}`` wraps a draft under a key spelled like a rule kind, and
    ``{"person": …}`` is a mapping whose only key is spelled like a term's form. Both
    are keys the client wrote, so neither is the validator's tag. ``/capped`` and
    ``/nested`` are the two shapes where dropping such a key as a tag still leaves a
    reading that names a place in the body.
    """
    app = _serving_envelopes(FastAPI())

    @app.post("/stored")
    def stored(document: _StoredRule) -> dict[str, str]:
        return {}

    @app.post("/draft")
    def draft(document: _LooseDraft) -> dict[str, str]:
        return {}

    @app.post("/capped")
    def capped(document: _CappedDraft) -> dict[str, str]:
        return {}

    @app.post("/nested")
    def nested(document: _PersonDraft) -> dict[str, str]:
        return {}

    @app.post("/duplicate")
    def duplicate(document: _DuplicateParentDraft) -> dict[str, str]:
        return {}

    return app


@pytest.mark.parametrize(
    ("route", "document", "pointer", "value"),
    [
        ("/stored", {"window": {"id": "x"}}, "/window/id", "x"),
        ("/draft", {"person": "x"}, "/person", "x"),
        ("/capped", {"cap": {"limit": "x"}, "limit": 5}, "/cap/limit", "x"),
        ("/nested", {"person": {"person": "x"}}, "/person/person", "x"),
    ],
    ids=[
        "nested-under-a-kinds-name",
        "named-like-a-term",
        "a-sibling-holds-the-stripped-key",
        "the-same-key-nested",
    ],
)
def test_a_body_of_the_callers_own_model_keeps_the_keys_it_wrote(
    route: str, document: dict[str, Any], pointer: str, value: Any
) -> None:
    """A document these models do not describe is served the pointer into itself.

    The walk reads a key spelled like one of the package's union tags as the tag
    only when a document it declares accounts for the rest of the location; a
    caller's own body is read against the body the failure carries, so the reading
    that lands on the value the finding is about wins over one that would drop a key
    the client wrote. The last three cases are the ones a reading that merely *exists*
    in the body cannot tell apart: dropping ``cap`` or the outer ``person`` as a tag
    still names a place in the document — a sibling that holds the same value, a
    sibling that is fine, or the object the offending field sits inside.
    """
    app = _a_route_per_callers_own_model()
    with TestClient(app) as client:
        answer = client.post(route, json=document)

    assert answer.status_code == 422, answer.text
    assert [finding["path"] for finding in _findings(answer)] == [pointer]
    assert _resolve(document, pointer) == value


def test_a_sibling_holding_the_same_value_cannot_stand_in_for_the_stripped_key() -> None:
    """Dropping a real key as a union tag must not land on an equal-valued sibling.

    ``cap`` is the tag a cap rule is selected by and ``limit`` is a field of both
    levels, so dropping ``cap`` leaves a reading that still resolves — to a sibling
    the document also holds. When that sibling's value is the one the finding is
    about, equality cannot tell the two readings apart; only the location as
    reported, which spells out both keys, names the field the finding is about.
    """
    app = _a_route_per_callers_own_model()
    document = {"cap": {"limit": "x"}, "limit": "x"}
    with TestClient(app) as client:
        answer = client.post("/capped", json=document)

    assert answer.status_code == 422, answer.text
    assert [finding["path"] for finding in _findings(answer)] == ["/cap/limit", "/limit"]
    assert _resolve(document, "/cap/limit") == "x"


@pytest.mark.parametrize(
    ("root", "payload", "code", "pointer", "value"),
    [
        (
            Requirement,
            {"all": [{"person": 5}]},
            "string_type",
            "/all/0/person",
            5,
        ),
        (list[Rule], [BAD_CAP], "greater_than_equal", "/0/params/limit", -1),
        (
            list[Requirement],
            [{"all": [{"person": 5}]}],
            "string_type",
            "/0/all/0/person",
            5,
        ),
    ],
    ids=["class-document", "list-of-an-alias", "list-of-a-class"],
)
def test_a_document_named_by_the_caller_is_read_as_itself(
    root: Any, payload: Any, code: str, pointer: str, value: Any
) -> None:
    """A stored document validated on its own is read through its own keys.

    ``Requirement`` is a document the package declares as a class, and ``all`` is both
    its own key and one of ``Selector``'s form tags, so a walk with nothing to check
    a reading against cannot tell them apart — it strips ``all`` and names a key the
    document does not have. Naming the payload the caller validated, the shape the
    ``TypeAdapter`` pattern in ``docs/api.md`` holds in the same scope, makes the
    reading that lands on the offending value the one served.

    A JSON column holds a list as readily as an object, and a list of documents has a
    first reading of its own: ``Span`` is a two-element tuple alias, so it reads the
    leading index and keeps everything after it, and without a document that is the
    reading served (``/0/cap/params/limit``). The named document settles it the way
    it settles the class case — ``/0/params/limit`` and ``/0/all/0/person`` — so the
    parameter's contract is pinned for the containers a stored column holds, not only
    for a bare object.
    """
    adapter: TypeAdapter[Any] = TypeAdapter(root)
    with pytest.raises(ValidationError) as raised:
        adapter.validate_python(payload)

    envelope = error_envelope_from_validation_error(raised.value, document=payload)

    assert [(finding.code, finding.path) for finding in envelope.errors] == [(code, pointer)]
    assert _resolve(payload, pointer) == value


def test_missing_fields_with_equal_parents_keep_their_own_paths() -> None:
    """A missing field is confirmed by its identical parent, not an equal sibling.

    The two `limit` findings have equal `{}` inputs, but they belong to different
    objects. Equality alone therefore serves the second path twice; identity selects
    the first finding's actual parent while retaining equality for decoded copies.
    """
    app = _a_route_per_callers_own_model()
    with TestClient(app) as client:
        answer = client.post("/duplicate", json={"cap": {"params": {}}, "params": {}})

    assert answer.status_code == 422, answer.text
    assert [(finding["code"], finding["path"]) for finding in _findings(answer)] == [
        ("missing", "/cap/params/limit"),
        ("missing", "/params/limit"),
    ]


def test_a_missing_field_in_a_separately_decoded_document_uses_equality() -> None:
    """The `document=` contract also works when validation parsed a separate copy."""
    text = '{"all":[{"at_least":2}]}'
    adapter: TypeAdapter[Requirement] = TypeAdapter(Requirement)
    with pytest.raises(ValidationError) as raised:
        adapter.validate_json(text)

    envelope = error_envelope_from_validation_error(raised.value, document=json.loads(text))

    assert [(finding.code, finding.path) for finding in envelope.errors] == [
        ("missing", "/all/0/of")
    ]


def test_a_finding_about_a_missing_field_names_the_object_it_belongs_to() -> None:
    """An absent field is confirmed by its parent, so a tag-spelled key is not lost.

    ``{"window": {}}`` is a caller's own draft under a key spelled like a rule's
    ``kind``; the field at fault is the ``id`` its inner model requires, and the value
    the finding states is the empty object the field belongs to. That value is what
    tells the right reading from one that dropped ``window`` as a tag, which names a
    field this document does not have at all.
    """
    app = _a_route_per_callers_own_model()
    with TestClient(app) as client:
        answer = client.post("/stored", json={"window": {}})

    assert answer.status_code == 422, answer.text
    assert [(finding["code"], finding["path"]) for finding in _findings(answer)] == [
        ("missing", "/window/id")
    ]


def test_a_proposal_that_does_not_fit_is_a_422_the_client_can_act_on() -> None:
    """A confirmation's rejection is the caller's to fix, so it is answered as one."""
    app = create_app()
    proposal = {
        "events": [{"part": "other", "start": "2027-03-09T15:00Z", "assignment": [["ana"]]}]
    }
    with TestClient(app) as client:
        refused = client.post("/check-slot", json={"request": REQUEST, "proposal": proposal})
        accepted = client.post(
            "/check-slot",
            json={
                "request": REQUEST,
                "proposal": {
                    "events": [
                        {
                            "part": "main",
                            "start": "2027-03-09T15:00Z",
                            "assignment": [["ana"]],
                        }
                    ]
                },
            },
        )

    assert accepted.status_code == 200, accepted.text
    assert accepted.json()["valid"] is True

    assert refused.status_code == 422, refused.text
    envelope = refused.json()
    assert set(envelope) == {"code", "message", "path", "errors"}
    assert (envelope["code"], envelope["path"]) == ("proposal_part_unknown", "/events/0/part")
    assert envelope["errors"] == []


def test_a_stored_document_this_build_cannot_read_is_a_500_that_names_nothing_internal() -> None:
    """The deployment's own failure is a server error, and its internals stay on the server.

    The version a stored document was written with is the application's own
    configuration, so a build that cannot read it is not the client's mistake. The
    body is what a client sees of any such failure: no code, message or pointer from
    the engine's own vocabulary reaches it, so an unusable database directory's path
    cannot either.
    """
    app = create_app()
    app.state.stored_schema_version = 2
    with TestClient(app, raise_server_exceptions=False) as client:
        answer = client.post("/rules/upgrade", json=WINDOW_RULE)

    assert answer.status_code == 500, answer.text
    assert answer.json() == {
        "code": "internal_error",
        "message": "the request could not be served",
    }


@pytest.mark.parametrize(
    ("produce", "status"),
    [
        pytest.param(
            lambda: Engine(tzdb_version="2026f"), 500, id="tzdb-version-without-directory"
        ),
        pytest.param(
            lambda: Engine(tzdb_dir="/nonexistent/opportune-tzdb"),
            500,
            id="tzdb-directory-unusable",
        ),
        pytest.param(
            lambda: Engine(tzdb_dir="\ud800"),
            500,
            id="argument-not-utf8",
        ),
        pytest.param(
            lambda: presets.horizon(1.5),  # pyright: ignore[reportArgumentType]
            500,
            id="package-argument",
        ),
        pytest.param(lambda: upgrade_rule(json.dumps(WINDOW_RULE), 2), 500, id="stored-version"),
        pytest.param(
            lambda: Engine(limits=Limits.model_construct(max_participants=2**64)),
            500,
            id="limits-the-native-reader-refuses",
        ),
        pytest.param(
            lambda: Engine().find_availability({"now": 5}),
            422,
            id="malformed-request-mapping",
        ),
        pytest.param(
            lambda: Engine().required_ranges({"now": 5}),
            422,
            id="malformed-ranges-mapping",
        ),
        pytest.param(
            lambda: Engine().find_availability(SURROGATE_ID),
            422,
            id="unwritable-document",
        ),
        pytest.param(
            lambda: Engine().find_availability(UNKNOWN_ZONE),
            422,
            id="rejected-request",
        ),
    ],
)
def test_the_status_helper_separates_the_deployments_failure_from_the_callers(
    produce: Callable[[], object], status: int
) -> None:
    """A deployment's own failure and a refusal of what a caller sent are not one status.

    Each producer here is a real one — the engine's own construction, a preset given a
    value no form accepts, an upgrade of a stored document, a request document the
    engine rejects, and one whose text has no UTF-8 spelling — so the codes the helper
    classifies are the codes the package actually raises, and a code moving between
    the two classes shows up here.
    """
    with pytest.raises(OpportuneError) as raised:
        produce()

    assert http_status_for_error(raised.value) == status, raised.value.code


SERVER_CODES: tuple[str, ...] = (
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
)
"""Every code that names the deployment's own failure, written out.

Deliberately not read off the set the helper classifies by: a test that iterated that
set would stay green if a code were dropped from it, which is the one mistake this
vocabulary has to catch.
"""

CALLER_CODES: tuple[str, ...] = (
    "invalid_json",
    "invalid_request",
    "marker_tag_invalid",
    "proposal_part_unknown",
    "zone_unknown",
)
"""Codes a caller fixes by changing what it sent, as the native registries spell them."""


@pytest.mark.parametrize(
    ("code", "status"),
    [pytest.param(code, 500, id=code) for code in SERVER_CODES]
    + [pytest.param(code, 422, id=code) for code in CALLER_CODES],
)
def test_every_code_in_the_vocabulary_carries_the_status_its_class_names(
    code: str, status: int
) -> None:
    """The whole vocabulary is pinned, not only the codes this build can produce.

    Five of the eleven server codes name a failure no producer here can raise: a wheel
    compiled without the bundled time zone database answers ``tzdb_bundled_unavailable``,
    ``tzdb_not_configured`` or ``tzdb_version_missing``, a document the engine could not
    write answers ``serialization_failed``, and ``payload_invalid`` names a provider
    response the application fetched rather than anything a client sent. The table
    above is the only thing holding those five at ``500``, so dropping one from the set
    the helper classifies by would answer a deployment's own failure as a ``422``
    carrying its message — the leak the split exists to prevent — with the suite still
    green.
    """
    raised = OpportuneError(code, "the value was rejected", path=None, errors=[])

    assert http_status_for_error(raised) == status, raised.code


def test_the_validation_envelope_preserves_a_rejection_that_is_already_structured() -> None:
    """Handing the wrong helper an ``OpportuneError`` serves the right envelope.

    A handler registered for both failures — ``except (OpportuneError,
    ValidationError)`` — may call either helper, so the one that reads Pydantic
    findings has to answer a rejection with the rejection's own ``code``, ``message``,
    ``path`` and ``errors``. An empty ``invalid_request`` would drop the code a client
    switches on and the pointer a client fixes, and would do it silently.
    """
    finding = Diagnostic(
        code="zone_unknown", message="unknown time zone `Mars/Olympus_Mons`", path="/zone"
    )
    rejection_error = OpportuneError(
        "invalid_request",
        "the request is invalid",
        path=None,
        errors=[finding],
    )

    from_engine = error_envelope_from_validation_error(rejection_error)
    from_rejection = error_envelope_from_opportune_error(rejection_error)

    assert from_engine == from_rejection
    assert from_engine.code == "invalid_request"
    assert from_engine.message == "the request is invalid"
    assert from_engine.path is None
    assert from_engine.errors == [finding]

    # The same for a code that names one field, with its pointer kept.
    point = OpportuneError(
        "zone_name_invalid", "the zone is blank", path="/participant_zone", errors=[]
    )
    assert error_envelope_from_validation_error(point) == error_envelope_from_opportune_error(point)
    assert point.path == "/participant_zone"


def test_the_validation_envelope_answers_an_exception_that_carries_no_findings() -> None:
    """A catch-all handler gets an envelope, not a second failure inside the handler.

    The helper is the documented backstop for a handler registered for
    ``Exception``: an exception that carries no findings is answered with its own
    message and an empty finding list, and the package's own rejection is answered
    with its own envelope before the findings are looked for, so a *list* named
    ``errors`` is never called.
    """
    plain = error_envelope_from_validation_error(RuntimeError("boom"))
    assert plain.code == "invalid_request"
    assert plain.message == "boom"
    assert plain.errors == []


def test_the_status_helper_refuses_a_value_it_does_not_classify() -> None:
    """A ``ValidationError`` is the sibling helper's document, not this one's.

    Classifying by ``code`` against an exception that has none raised an
    ``AttributeError`` from inside the handler itself, which is the one place an
    application cannot afford a second failure: the helper names the class it reads
    instead.
    """
    with pytest.raises(TypeError) as raised:
        http_status_for_error(ValidationError.from_exception_data("x", []))  # pyright: ignore[reportArgumentType]

    assert "OpportuneError" in str(raised.value)
    assert "ValidationError" in str(raised.value)
    assert "error_envelope_from_validation_error" in str(raised.value)


def test_a_json_field_error_keeps_the_field_pointer() -> None:
    """A `Json[T]` field's syntax error is about that field, not the whole body."""

    class Document(BaseModel):
        payload: PydanticJson[int]

    payload = {"payload": "not json"}
    with pytest.raises(ValidationError) as raised:
        Document.model_validate(payload)

    envelope = error_envelope_from_validation_error(raised.value, document=payload)

    assert [(finding.code, finding.path) for finding in envelope.errors] == [
        ("json_invalid", "/payload")
    ]
    assert _resolve(payload, "/payload") == "not json"


def test_a_pydantic_field_named_body_is_not_mistaken_for_fastapis_parameter() -> None:
    """A plain model's `body` field is a key, not FastAPI's location marker.

    The helper also accepts a Pydantic ``ValidationError`` directly. Only a
    ``RequestValidationError`` carries the framework's ``body`` attribute, so that
    attribute is what distinguishes the parameter marker from a caller's own key.
    """

    class BodyDocument(BaseModel):
        body: int

    payload = {"body": "not an integer"}
    with pytest.raises(ValidationError) as raised:
        BodyDocument.model_validate(payload)

    for document in (None, payload):
        envelope = error_envelope_from_validation_error(raised.value, document=document)
        assert [(finding.code, finding.path) for finding in envelope.errors] == [
            ("int_parsing", "/body")
        ]
    assert _resolve(payload, "/body") == "not an integer"


def test_a_validation_error_with_no_body_still_points_at_the_field_the_caller_wrote() -> None:
    """A stored rule is read with a ``TypeAdapter``, and that failure carries no body.

    ``docs/api.md`` reads a stored rule back with ``TypeAdapter(opportune.Rule)``, so
    the envelope an application builds from that failure has to name the field the
    stored document states — with the ``kind`` tag dropped — even though there is no
    body to check a reading against. It is the same walk with the last verification
    step absent, and every route in this module carries a body, so only this pins it.
    """
    adapter: TypeAdapter[Rule] = TypeAdapter(Rule)
    with pytest.raises(ValidationError) as raised:
        adapter.validate_python(BAD_CAP)

    envelope = error_envelope_from_validation_error(raised.value)

    assert envelope.code == "invalid_request"
    assert [(finding.code, finding.path) for finding in envelope.errors] == [
        ("greater_than_equal", "/params/limit")
    ]


def test_a_rejection_the_engine_raises_reaches_the_exception_handler() -> None:
    app = create_app()
    body = json.loads(json.dumps(REQUEST))
    body["participants"][0]["zone"] = "Mars/Olympus_Mons"
    with TestClient(app) as client:
        answer = client.post("/availability", json=body)

    assert answer.status_code == 422, answer.text
    envelope = answer.json()
    assert envelope["code"] == "invalid_request"
    assert envelope["message"]
    assert [(finding["code"], finding["path"]) for finding in envelope["errors"]] == [
        ("zone_unknown", "/participants/0/zone")
    ]


def test_an_adapters_rejection_reaches_the_same_envelope() -> None:
    """A conversion failure is the engine's rejection, not the framework's 500."""
    app = create_app()
    with TestClient(app) as client:
        healthy = client.post("/calendar-view", json=CALENDAR_VIEW)
        blank_zone = client.post("/calendar-view", json=CALENDAR_VIEW, params={"zone": "  "})

    assert healthy.status_code == 200, healthy.text
    assert healthy.json()["blocks"][0]["event_id"] == "AAMkAD"

    assert blank_zone.status_code == 422, blank_zone.text
    envelope = blank_zone.json()
    assert set(envelope) == {"code", "message", "path", "errors"}
    assert (envelope["code"], envelope["path"]) == ("zone_name_invalid", "/participant_zone")
    assert envelope["errors"] == [], "the argument is at fault, not a field of a document"


def test_a_provider_payload_the_server_fetched_is_answered_500() -> None:
    """A Graph response the adapter cannot read is the deployment's failure.

    The client's own request was answered; what could not be read is a document the
    application fetched from Microsoft, so a route serving the conversion answers
    ``500`` with none of the adapter's message. Serving it as the ``422`` a caller
    fixes would accuse the caller of a payload it never sent, and would echo the
    provider's shape back to it.
    """
    app = FastAPI()

    def envelope(error: OpportuneError) -> JSONResponse:
        if http_status_for_error(error) != 422:
            return JSONResponse(
                status_code=500,
                content={"code": "internal_error", "message": "the request could not be served"},
            )
        return JSONResponse(
            status_code=422,
            content=error_envelope_from_opportune_error(error).model_dump(),
        )

    @app.exception_handler(OpportuneError)
    async def rejected(_: HttpRequest, error: OpportuneError) -> JSONResponse:
        return envelope(error)

    @app.post("/schedule")
    def schedule(payload: dict[str, Any], http: HttpRequest) -> dict[str, Any]:
        return http.app.state.engine.get_schedule_to_blocks(payload)

    app.state.engine = Engine()
    with pytest.raises(OpportuneError) as raised:
        app.state.engine.get_schedule_to_blocks({"value": 7})
    assert raised.value.code == "payload_invalid", "a real producer, not a synthetic code"
    assert http_status_for_error(raised.value) == 500

    with TestClient(app) as client:
        answer = client.post("/schedule", json={"value": 7})

    assert answer.status_code == 500, answer.text
    assert answer.json() == {
        "code": "internal_error",
        "message": "the request could not be served",
    }, "the adapter's message names the provider's shape, so it stays out of the body"


def test_two_apps_keep_their_own_engine_limits() -> None:
    """An engine is app-scoped state: two apps in one process never share it."""
    crowded: dict[str, Any] = {
        **REQUEST,
        "participants": [
            {"id": "ana", "zone": "UTC", "calendar": []},
            {"id": "bo", "zone": "UTC", "calendar": []},
        ],
        "goal": {
            "parts": [{"duration": "30m", "requirement": {"all": [{"person": "ana"}]}}],
            "search_window": ["2027-03-09T15:00Z", "2027-03-09T18:00Z"],
            "zone": "UTC",
        },
    }
    one_participant = create_app(Engine(limits=_limits(max_participants=1)))
    two_participants = create_app(Engine(limits=_limits(max_participants=2)))

    with TestClient(one_participant) as strict_client, TestClient(two_participants) as open_client:
        refused = strict_client.post("/availability", json=crowded)
        offered = open_client.post("/availability", json=crowded)

    assert refused.status_code == 422, refused.text
    assert [finding["code"] for finding in refused.json()["errors"]] == ["participants_limit"]
    assert offered.status_code == 200, offered.text


def _limits(*, max_participants: int) -> Limits:
    """The default limits with one replaced, so a test names only what it changes."""
    return with_updates(default_limits(), max_participants=max_participants)


def test_a_document_whose_text_has_no_utf8_spelling_is_a_rejection_either_route() -> None:
    """A client can send a lone surrogate escape, and both routes have to answer 422.

    JSON allows a `\\ud800` escape with no pair, and Pydantic's parser accepts it, so
    a body this small is enough to hand a request model a string with no UTF-8
    spelling. That is the package's rejection — the typed route serialises the model
    and the raw route serialises it too — and an application's `OpportuneError`
    handler has to see it rather than an unhandled encoder failure behind a 500.
    """
    poisoned = json.dumps(REQUEST).replace('"id": "ana"', '"id": "\\ud800"')
    assert "\\ud800" in poisoned, "the body carries the escape, not the character"

    app = create_app()
    with TestClient(app, raise_server_exceptions=False) as client:
        for route in ("/availability", "/availability/raw"):
            answer = client.post(
                route, content=poisoned.encode(), headers={"content-type": "application/json"}
            )

            assert answer.status_code == 422, f"{route}: {answer.status_code} {answer.text}"
            assert answer.json()["code"] == "invalid_json", answer.text
