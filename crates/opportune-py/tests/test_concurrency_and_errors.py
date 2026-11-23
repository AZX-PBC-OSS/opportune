"""Concurrent execution on one engine, and the structured exception callers catch.

The engine is frozen and releases the GIL around every call, so eight threads
sharing one instance must answer identically to a single-threaded run — including
when they interleave two different requests. Every rejection crosses the package
boundary as :class:`~opportune.OpportuneError` with the native cause chained and
its findings as generated models.

The second half of the file is the other half of that contract: no argument, payload
or span a caller can write may leave the package as a bare ``UnicodeEncodeError``,
``UnicodeDecodeError``, ``TypeError``, ``ValueError``, ``RecursionError`` or
``ValidationError`` when an ``OpportuneError`` handler is what an application
installs. The only exception is a model field assignment, where
:class:`pydantic.ValidationError` is the generated model's own contract.
"""

import asyncio
import importlib
import json
import threading
from collections.abc import Callable, Mapping
from concurrent.futures import Future, ThreadPoolExecutor
from pathlib import Path
from typing import Any

import pytest
from pydantic import ValidationError

from opportune import (
    AvailableCalendarOutcome,
    AvailableWorkingHoursOutcome,
    Diagnostic,
    Diagnostics,
    Engine,
    Explain,
    Goal,
    IdsSelector,
    Limits,
    OpportuneError,
    Part,
    Participant,
    Proposal,
    ProposalEvent,
    RangesQuery,
    Request,
    RequiredRanges,
    Requirement,
    Response,
    SlotCheck,
    TagSelector,
    Travel,
    UnavailableCalendarOutcome,
    ZonePeriod,
    default_limits,
    find_availability,
    find_availability_async,
    find_availability_json_async,
    http_status_for_error,
    limits_with,
    person,
    presets,
    required_ranges,
    required_ranges_async,
    required_ranges_json_async,
    single_goal,
    upgrade_calendar,
    upgrade_goal,
    upgrade_rule,
    validate,
    validate_json_async,
    with_updates,
)
from opportune.adapters import google, graph
from opportune.engine import default_engine

_THREADS = 8
_CALLS = 32

CRATES = Path(__file__).parents[2]
"""The workspace's `crates/` directory, where the shared fixtures live."""

TZDB_2018 = CRATES / "opportune" / "tests" / "fixtures" / "tzdb-2018"
"""A real `TZif` tree whose `+VERSION` says `2018f` and which holds no American zones."""

NEW_YORK_PAYLOAD = json.dumps(
    {
        "value": [
            {
                "id": "AAMkAD",
                "isAllDay": False,
                "showAs": "busy",
                "start": {
                    "dateTime": "2018-07-02T09:00:00.0000000",
                    "timeZone": "America/New_York",
                },
                "end": {
                    "dateTime": "2018-07-02T10:00:00.0000000",
                    "timeZone": "America/New_York",
                },
            }
        ]
    }
)
"""A `calendarView` response stamped in a zone the 2018 fixture tree does not hold."""

WORKING_HOURS = json.dumps(
    {
        "daysOfWeek": ["monday"],
        "startTime": "09:00:00",
        "endTime": "17:00:00",
        "timeZone": {"name": "America/New_York"},
    }
)
"""A `workingHours` object in the same zone."""

SURROGATE = "\ud800"
"""One unpaired UTF-16 surrogate: a Python string with no UTF-8 spelling."""


def _request(window: tuple[str, str]) -> Request:
    return Request(
        now="2027-06-14T07:00Z",
        goal=single_goal(
            "30m",
            Requirement(all=[person("ana")]),
            zone="UTC",
            search_window=window,
        ),
        participants=[Participant(id="ana", zone="UTC", calendar=[])],
        rules=[presets.business_hours("mon-sun", "09:00-17:00")],
    )


def test_eight_threads_sharing_one_engine_get_identical_deterministic_answers() -> None:
    engine = Engine()
    requests = [
        _request(("2027-06-14T09:00Z", "2027-06-14T11:00Z")),
        _request(("2027-06-14T18:00Z", "2027-06-14T20:00Z")),
    ]
    expected = [
        engine.find_availability(request).model_dump_json(by_alias=True) for request in requests
    ]
    order = [index % len(requests) for index in range(_CALLS)]

    with ThreadPoolExecutor(max_workers=_THREADS) as pool:
        futures = [pool.submit(engine.find_availability, requests[index]) for index in order]
        answers = [future.result().model_dump_json(by_alias=True) for future in futures]

    assert answers == [expected[index] for index in order]


def test_the_engine_reports_the_database_version_it_answers_with() -> None:
    engine = Engine()
    response = engine.find_availability(_request(("2027-06-14T09:00Z", "2027-06-14T11:00Z")))

    assert response.meta.tzdb == engine.tzdb_version


def test_the_free_functions_share_one_lazily_built_default_engine() -> None:
    request = _request(("2027-06-14T09:00Z", "2027-06-14T11:00Z"))

    assert default_engine() is default_engine()
    assert find_availability(request).model_dump_json(
        by_alias=True
    ) == default_engine().find_availability(request).model_dump_json(by_alias=True)


def test_malformed_json_raises_the_packages_exception_with_the_native_cause() -> None:
    with pytest.raises(OpportuneError) as raised:
        upgrade_rule("{not json", 1)

    error = raised.value
    assert error.code == "invalid_json"
    assert error.path is None, "the whole document is at fault, not one of its fields"
    assert error.errors == []
    assert error.__cause__ is not None, "the native rejection stays attached"
    assert str(error.__cause__) == error.message


def test_a_rejected_request_carries_its_findings_as_generated_models() -> None:
    broken = with_updates(
        _request(("2027-06-14T09:00Z", "2027-06-14T11:00Z")),
        participants=[Participant(id="ana", zone="Mars/Olympus_Mons", calendar=[])],
    )

    with pytest.raises(OpportuneError) as raised:
        find_availability(broken)

    error = raised.value
    assert error.code == "invalid_request"
    assert [finding.path for finding in error.errors] == ["/participants/0/zone"]
    assert isinstance(error.errors[0], Diagnostic)
    assert error.errors[0].code == "zone_unknown"
    assert "Mars/Olympus_Mons" in error.errors[0].message


def test_a_structural_failure_names_the_field_it_is_about() -> None:
    with pytest.raises(OpportuneError) as raised:
        upgrade_rule('{"id": "r_min", "kind": "lead_time", "params": {"min": "1d"}}', 1)

    error = raised.value
    assert error.code == "duration_contains_days"
    assert error.path == "/params/min", "an RFC 6901 pointer into the stored rule"


def test_an_upgrade_rejects_a_schema_version_this_build_does_not_read() -> None:
    rule = '{"id": "r_min", "kind": "lead_time", "params": {"min": "4h"}}'

    upgraded: dict[str, Any] = json.loads(upgrade_rule(rule, 1))
    assert upgraded["id"] == "r_min"

    with pytest.raises(OpportuneError) as raised:
        upgrade_rule(rule, 2)
    assert raised.value.code == "unsupported_schema_version"


def test_the_upgrade_helpers_rewrite_every_stored_document_kind() -> None:
    goal = (
        '{"zone": "UTC", "search_window": ["2027-06-14T00:00Z", "2027-06-15T00:00Z"],'
        ' "parts": [{"duration": "30m", "requirement": {"all": [{"person": "ana"}]}}]}'
    )

    calendar: dict[str, Any] = json.loads(upgrade_calendar('{"zone": "UTC"}', 1))
    upgraded_goal: dict[str, Any] = json.loads(upgrade_goal(goal, 1))

    assert calendar == {"zone": "UTC", "holidays": []}, "the canonical v1 form"
    assert upgraded_goal["display_zone"] == "UTC", "the canonical form names the default"
    assert upgraded_goal["parts"][0]["id"] == "main"


def test_validate_and_required_ranges_answer_through_the_free_functions() -> None:
    request = _request(("2027-06-14T09:00Z", "2027-06-14T11:00Z"))

    diagnostics = validate(request)
    assert diagnostics.errors == []
    assert diagnostics.warnings == []

    query = RangesQuery(
        now=request.now,
        goal=request.goal,
        participants=request.participants,
        rules=request.rules,
    )
    ranges = required_ranges(query)
    assert ranges.calendar == ("2027-06-13T00:00Z", "2027-06-16T00:00Z")
    assert ranges.occurrences is None, "no cap applies, so no occurrence data is needed"


def test_the_limits_argument_reaches_the_engine() -> None:
    engine = Engine(
        limits=Limits(
            max_participants=1,
            max_blocks_per_participant=10_000,
            max_search_window_days=92,
            max_candidate_starts=100_000,
            max_explain_limit=10_000,
        )
    )
    crowded = with_updates(
        _request(("2027-06-14T09:00Z", "2027-06-14T11:00Z")),
        participants=[
            Participant(id="ana", zone="UTC", calendar=[]),
            Participant(id="bo", zone="UTC", calendar=[]),
        ],
    )

    with pytest.raises(OpportuneError) as raised:
        engine.find_availability(crowded)

    error = raised.value
    assert error.code == "invalid_request"
    assert [(finding.code, finding.path) for finding in error.errors] == [
        ("participants_limit", "/participants")
    ]


def test_an_empty_limits_document_is_the_engines_own_defaults() -> None:
    """Every cap is optional, so `Limits()` states nothing and the engine fills each.

    An engine is built from the wire document, and an empty one has to mean
    ``Limits::default()`` rather than a cap of zero: a binding that writes only the
    limits it changes — a partial config file, a save of one field — must not
    silently refuse every request. A cap that *is* stated still applies.
    """
    request = _request(("2027-06-14T09:00Z", "2027-06-14T11:00Z"))

    empty = Limits()
    assert empty.max_participants is None, "no cap is required"
    assert empty.model_dump(exclude_none=True) == {}, "an empty document is what it writes"

    assert Engine(limits=empty).find_availability(request) == Engine().find_availability(request)

    crowded = with_updates(
        request,
        participants=[
            Participant(id="ana", zone="UTC", calendar=[]),
            Participant(id="bo", zone="UTC", calendar=[]),
        ],
    )
    assert Engine(limits=Limits()).find_availability(crowded).offers, (
        "the absent caps are the engine defaults, not zero"
    )

    with pytest.raises(OpportuneError) as raised:
        Engine(limits=Limits(max_participants=1)).find_availability(crowded)
    assert [(finding.code, finding.path) for finding in raised.value.errors] == [
        ("participants_limit", "/participants")
    ], "a stated cap reaches the engine with the rest defaulted"


def test_the_zone_period_cap_fails_closed_on_every_entry_point() -> None:
    """One participant field may not demand unbounded schedule work.

    The list is bounded like every other collection a request carries, and every
    entry point answers the cap before a schedule is resolved from it.
    """
    engine = Engine(limits=limits_with(max_zone_periods_per_participant=1))
    crowded = with_updates(
        _request(("2027-06-14T09:00Z", "2027-06-14T11:00Z")),
        participants=[
            Participant(
                id="ana",
                zone="UTC",
                calendar=[],
                zone_periods=[
                    ZonePeriod(from_="2027-06-14T00:00Z", until="2027-06-14T01:00Z", zone="UTC"),
                    ZonePeriod(from_="2027-06-14T02:00Z", until="2027-06-14T03:00Z", zone="UTC"),
                ],
            )
        ],
    )

    diagnostics = engine.validate(crowded)
    assert [(finding.code, finding.path) for finding in diagnostics.errors] == [
        ("zone_periods_limit", "/participants/0/zone_periods")
    ]

    query = RangesQuery(
        now=crowded.now,
        goal=crowded.goal,
        participants=crowded.participants,
        rules=crowded.rules,
    )
    for call in (
        lambda: engine.find_availability(crowded),
        lambda: engine.check_slot(crowded, Proposal(events=[])),
        lambda: engine.required_ranges(query),
        lambda: engine.required_ranges(crowded),
    ):
        with pytest.raises(OpportuneError) as raised:
            call()
        error = raised.value
        assert error.code == "invalid_request"
        assert [(finding.code, finding.path) for finding in error.errors] == [
            ("zone_periods_limit", "/participants/0/zone_periods")
        ], error.errors

    # The cap bounds the list rather than the request's shape: a participant
    # exactly at it is answered.
    within = with_updates(
        crowded,
        participants=[
            Participant(
                id="ana",
                zone="UTC",
                calendar=[],
                zone_periods=[
                    ZonePeriod(from_="2027-06-14T00:00Z", until="2027-06-14T01:00Z", zone="UTC")
                ],
            )
        ],
    )
    assert engine.find_availability(within).offers, "a participant at the cap is bookable"


def test_a_version_label_without_a_directory_is_rejected() -> None:
    with pytest.raises(OpportuneError) as raised:
        Engine(tzdb_version="2026f")

    assert raised.value.code == "tzdb_version_without_directory"


def test_a_limits_document_the_native_reader_refuses_is_a_server_failure() -> None:
    """The limits are the deployment's configuration, so they are never a caller's ``422``.

    A limits object assembled without the model's validation — a migration building
    one from a wider integer column, which is what ``model_construct`` is for — is
    written by this package's serialiser and refused by the native limits reader.
    That rejection arrives under the native document vocabulary (``invalid_type``,
    ``invalid_value``), which an application answers ``422``: a client whose own
    request was fine would be told its document was wrong while the deployment's
    configuration is what cannot be read. It is reported as ``invalid_limits``,
    which :func:`~opportune.http_status_for_error` answers ``500``, with the native
    failure and its findings kept.
    """
    wide = Limits.model_construct(max_participants=2**64)

    with pytest.raises(OpportuneError) as raised:
        Engine(limits=wide)

    error = raised.value
    assert error.code == "invalid_limits"
    assert http_status_for_error(error) == 500
    assert error.__cause__ is not None, "the native reader's failure stays attached"
    assert str(error.__cause__) in error.message, (
        "the reader's own sentence travels with the reclassified failure, so an operator "
        "still learns which limit and which type it refused"
    )

    # The same limit given as a mapping is refused by the generated model, before
    # the engine is built, and is the same server failure.
    with pytest.raises(OpportuneError) as raised:
        Engine(limits={**default_limits().model_dump(), "max_participants": 2**64})

    assert raised.value.code == "invalid_limits"
    assert http_status_for_error(raised.value) == 500
    assert [(finding.code, finding.path) for finding in raised.value.errors] == [
        ("less_than_equal", "/max_participants")
    ]

    # A limit the package's own serialiser cannot write is refused one step earlier,
    # by ``_json`` rather than by the native reader — a ``memoryview`` is what a
    # database driver hands back for a byte column, so a migration reading its limits
    # out of one builds exactly this. It is the same deployment failure, not the
    # ``invalid_json`` a caller's own document answers.
    with pytest.raises(OpportuneError) as raised:
        Engine(limits=Limits.model_construct(max_participants=memoryview(b"500")))

    assert raised.value.code == "invalid_limits"
    assert http_status_for_error(raised.value) == 500


def test_the_tzdb_directory_argument_reaches_the_engine() -> None:
    """A requested directory database is read, never silently replaced by the bundled one.

    Reading the bundled database instead would make the answers depend on the version
    this wheel happen to ship, which is the machine dependence the caller asked to
    avoid; an unusable directory therefore has to fail rather than fall back.
    """
    with pytest.raises(OpportuneError) as raised:
        Engine(tzdb_dir="/nonexistent/opportune-tzdb")

    assert raised.value.code == "tzdb_directory_unusable"


def test_a_surrogate_in_a_native_argument_raises_the_packages_exception() -> None:
    """A non-document argument has no UTF-8 spelling, and that is not a `UnicodeEncodeError`.

    `PyO3`'s own `&str` extraction raises the encoder's exception, which an
    application's `except OpportuneError` handler never sees. Every string argument
    the extension takes is read through the package's own rejection instead.
    """
    engine = Engine()

    with pytest.raises(OpportuneError) as raised:
        Engine(tzdb_dir=SURROGATE)
    assert raised.value.code == "argument_not_utf8"

    with pytest.raises(OpportuneError) as raised:
        Engine(tzdb_dir=str(TZDB_2018), tzdb_version=SURROGATE)
    assert raised.value.code == "argument_not_utf8"

    with pytest.raises(OpportuneError) as raised:
        engine.calendar_view_to_blocks(NEW_YORK_PAYLOAD, SURROGATE)
    assert raised.value.code == "argument_not_utf8"

    with pytest.raises(OpportuneError) as raised:
        engine.working_hours_to_rule(WORKING_HOURS, SURROGATE)
    assert raised.value.code == "argument_not_utf8"

    with pytest.raises(OpportuneError) as raised:
        google.events_list_to_blocks('{ "items": [] }', SURROGATE)
    assert raised.value.code == "argument_not_utf8"


def test_a_payload_that_is_not_utf8_raises_the_packages_exception() -> None:
    with pytest.raises(OpportuneError) as raised:
        Engine().calendar_view_to_blocks(b'{ "value": "\xff\xfe" }', "UTC")

    assert raised.value.code == "invalid_json"
    assert "UTF-8" in raised.value.message


def test_a_payload_that_json_cannot_write_raises_the_packages_exception() -> None:
    """A mapping holding something unserialisable is a rejection, not a `TypeError`."""
    with pytest.raises(OpportuneError) as raised:
        Engine().calendar_view_to_blocks({"value": object()}, "UTC")

    assert raised.value.code == "invalid_json"


def test_a_payload_that_names_itself_raises_the_packages_exception() -> None:
    """A mapping that points at itself has no JSON spelling, which is the caller's.

    The encoder refuses the circle rather than the boundary handing back a document
    no reader could have written, and the refusal is this package's error like every
    other unreadable payload — a handler written for it never sees a bare
    ``ValueError`` or ``RecursionError`` from the encoder underneath.
    """
    payload: dict[str, Any] = {"value": []}
    payload["self"] = payload

    with pytest.raises(OpportuneError) as raised:
        Engine().calendar_view_to_blocks(payload, "UTC")

    assert raised.value.code == "invalid_json"
    assert raised.value.__cause__ is not None, "the encoder's failure stays attached"


def test_a_payload_the_encoder_cannot_write_because_it_recurses_is_the_packages_exception(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    """The encoder's own recursion is this boundary's rejection, not an escapee.

    Nesting tens of thousands of levels deep is the only input that makes the
    encoder raise ``RecursionError``, and the depth at which it does is the stack
    this host gives the process, so the failure is injected: what is pinned is that
    the boundary answers the encoder's recursion with its own ``invalid_json``
    rather than letting a ``RecursionError`` reach a caller whose handler catches
    this package's error and nothing else.
    """

    def recurse(*_args: object, **_kwargs: object) -> str:
        raise RecursionError("maximum recursion depth exceeded")

    monkeypatch.setattr(json, "dumps", recurse)

    with pytest.raises(OpportuneError) as raised:
        Engine().calendar_view_to_blocks({"value": []}, "UTC")

    assert raised.value.code == "invalid_json"
    assert raised.value.__cause__ is not None, "the encoder's failure stays attached"


@pytest.mark.parametrize(
    ("span", "reason"),
    [
        (("2027-06-14T15:00Z", "2027-06-14T14:00Z"), "must end after it starts"),
        (("2027-06-14T23:30:30Z", "2027-06-15T00:30Z"), "not aligned to a whole minute"),
        (("2027-01-01T00:00Z", "2037-01-09T00:00Z"), "dated entries"),
        (("1970-01-01T00:00Z", "9970-01-01T00:00Z"), "outside the instants this package accepts"),
    ],
    ids=["reversed", "unaligned", "a-day-over-ten-years", "multi-millennium"],
)
def test_an_unusable_stated_span_raises_the_packages_exception(
    span: tuple[str, str], reason: str
) -> None:
    """A bad span is the package's rejection, before any date arithmetic runs.

    A span past ten years would otherwise be millions of dated entries written
    while the GIL is held: ten years is accepted, a day over it is not. A
    millennium-long span is refused earlier still, for the range reason that is
    true of it — an instant the engine does not hold — before the entries are
    even counted.
    """
    with pytest.raises(OpportuneError) as raised:
        presets.available_only("cand", [span])

    assert raised.value.code == "invalid_argument"
    assert reason in raised.value.message


def test_a_stated_span_of_exactly_the_longest_allowed_is_accepted() -> None:
    """The ceiling is a ceiling: ten years of stated availability still models."""
    _, availability = presets.available_only("cand", [("2027-01-01T00:00Z", "2037-01-08T00:00Z")])

    dates = availability.params.dates
    assert dates is not None
    assert len(dates) == 3660, "one entry per UTC day of the span, and no more"
    assert dates["2027-01-01"] == ["00:00-24:00"]
    assert dates["2037-01-07"] == ["00:00-24:00"], "the last day ends before the span does"


def test_the_ceiling_counts_every_span_of_the_availability() -> None:
    """The ceiling bounds the whole stated availability, not one span of it.

    One entry is written per UTC day of every span, so spans that each fit the
    ceiling still multiply: a candidate-submitted list of them would otherwise cost
    the process millions of entries written while the GIL is held.
    """
    ten_years = ("2027-01-01T00:00Z", "2037-01-08T00:00Z")
    with pytest.raises(OpportuneError) as raised:
        presets.available_only("cand", [ten_years, ("2038-01-01T00:00Z", "2048-01-09T00:00Z")])

    assert raised.value.code == "invalid_argument"
    assert "7320" in raised.value.message, "the message names the entries the spans need"

    # Half the ceiling each is still an availability of exactly the ceiling.
    _, availability = presets.available_only(
        "cand",
        [("2027-01-01T00:00Z", "2032-01-05T00:00Z"), ("2033-01-01T00:00Z", "2038-01-05T00:00Z")],
    )
    dates = availability.params.dates
    assert dates is not None
    assert len(dates) == 3660


def test_the_ceiling_counts_the_entries_written_not_the_dates_they_land_on() -> None:
    """A slot list costs one entry per slot-day, so it can exceed the ceiling on few dates.

    The ceiling bounds what the preset writes: two disjoint slots on one date are two
    entries under one key. Counting distinct dates instead would let a caller write
    an unbounded number of entries before the check, so the entries are what is
    counted — and the same availability stated as whole-day spans is compact.
    """
    slots = [
        (
            f"2027-06-{day:02d}T{minute // 60:02d}:{minute % 60:02d}Z",
            f"2027-06-{day:02d}T{(minute + 1) // 60:02d}:{(minute + 1) % 60:02d}Z",
        )
        for day in range(1, 7)
        for minute in range(0, 1222, 2)
    ]
    assert len(slots) == 3666, "611 one-minute slots on each of six days"

    with pytest.raises(OpportuneError) as raised:
        presets.available_only("cand", slots)

    assert raised.value.code == "invalid_argument"
    assert "3666 dated entries" in raised.value.message, raised.value.message

    _, compact = presets.available_only(
        "cand",
        [(f"2027-06-{day:02d}T00:00Z", f"2027-06-{day + 1:02d}T00:00Z") for day in range(1, 7)],
    )
    dates = compact.params.dates
    assert dates is not None
    assert len(dates) == 6, "the same six days stated as spans is six keys"


@pytest.mark.parametrize(
    "span",
    [
        ("9999-12-31T23:00-05:00", "9999-12-31T23:50-05:00"),
        ("0001-01-01T00:00+05:00", "0001-01-01T00:50+05:00"),
    ],
    ids=["an-offset-past-the-latest-instant", "an-offset-before-the-earliest-instant"],
)
def test_a_stated_span_an_offset_pushes_outside_the_engine_range_is_rejected(
    span: tuple[str, str],
) -> None:
    """An offset that moves an instant outside the engine's range is the range
    rejection, not a bare ``OverflowError``.

    ``datetime`` covers years 1..9999, so ``9999-12-31T23:00-05:00`` is a valid
    instant ``datetime`` can hold — and the range guard, which reads the instant
    the offset names, refuses it before any date arithmetic can overflow.
    """
    with pytest.raises(OpportuneError) as raised:
        presets.available_only("cand", [span])

    assert raised.value.code == "invalid_argument"
    assert "1975-01-01T00:00Z..=2100-01-01T00:00Z" in raised.value.message


def test_assigning_an_invalid_value_to_a_model_field_raises_validation() -> None:
    """`validate_assignment` keeps a model honest about the value it holds.

    Without it, `rule.who = {"bad": 1}` would sit in the model until the engine
    rejected the serialised request, moving the failure away from the line that
    wrote it.
    """
    hours = presets.business_hours()

    # An untyped caller can assign anything to the field; the model validates it.
    with pytest.raises(ValidationError):
        hours.who = {"bad": 1}  # pyright: ignore[reportAttributeAccessIssue]

    hours.who = IdsSelector(ids=["ana"])
    assert hours.who == IdsSelector(ids=["ana"])


@pytest.mark.filterwarnings("error")
def test_a_working_hours_rule_rebuilds_its_selectors_without_a_warning() -> None:
    """A caller running with `-W error` can replace the rule's selectors.

    The adapter rebuilds the rule as one validated value, so the outcome it returns
    holds a rule the engine accepts and no nested field is mutated in place.
    """
    outcome = graph.working_hours_to_rule(
        WORKING_HOURS, who=IdsSelector(ids=["ana"]), when=TagSelector(tag="booth")
    )

    assert isinstance(outcome, AvailableWorkingHoursOutcome), outcome
    assert outcome.rule.who == IdsSelector(ids=["ana"])
    assert outcome.rule.when == TagSelector(tag="booth")
    assert outcome.rule.params.weekly == {"mon": ["09:00-17:00"]}, "the conversion is untouched"
    wire = json.loads(outcome.model_dump_json(by_alias=True, exclude_none=True))
    assert wire["rule"]["kind"] == "window"


def test_an_engine_reads_its_own_database_for_adapters_and_evaluation() -> None:
    """A `tzdb_dir` engine converts payloads in the database it evaluates against.

    `America/New_York` is absent from the 2018 fixture tree, so a conversion that
    consulted the bundled database instead would answer `available` here while the
    search it feeds rejected the same zone. Both answers are observable, and they
    have to agree.
    """
    engine = Engine(tzdb_dir=str(TZDB_2018))

    unavailable = engine.calendar_view_to_blocks(NEW_YORK_PAYLOAD, "UTC")
    assert isinstance(unavailable, UnavailableCalendarOutcome), unavailable
    assert unavailable.code == "zone_unknown"

    hours = engine.working_hours_to_rule(WORKING_HOURS)
    assert hours.status == "unavailable", hours

    request = with_updates(
        _request(("2027-06-14T09:00Z", "2027-06-14T11:00Z")),
        participants=[Participant(id="ana", zone="America/New_York", calendar=[])],
    )
    with pytest.raises(OpportuneError) as raised:
        engine.find_availability(request)
    assert [finding.path for finding in raised.value.errors] == ["/participants/0/zone"]

    # The bundled database does resolve the zone, so the difference is the one the
    # engine was built with and not the payload.
    assert isinstance(
        Engine().calendar_view_to_blocks(NEW_YORK_PAYLOAD, "UTC"), AvailableCalendarOutcome
    )


def test_a_surrogate_in_a_model_field_raises_the_packages_exception() -> None:
    """A `str` field with no UTF-8 spelling is the package's rejection, not the encoder's.

    ``json.loads`` and ``bytes.decode(..., "surrogateescape")`` both hand back a
    string carrying a lone surrogate, and a caller that read a participant ID or a
    tag that way has a model the Pydantic *serializer* refuses to write. That
    failure is on the same boundary as a non-UTF-8 payload argument: an application
    with an ``except OpportuneError`` handler must see the rejection, not the
    serializer's own exception type.
    """
    request = with_updates(
        _request(("2027-06-14T09:00Z", "2027-06-14T11:00Z")),
        participants=[Participant(id=SURROGATE, zone="UTC", calendar=[])],
    )

    with pytest.raises(OpportuneError) as raised:
        Engine().find_availability(request)

    assert raised.value.code == "invalid_json"
    assert raised.value.__cause__ is not None, "the encoder's cause is chained, not dropped"


def test_a_category_map_json_cannot_write_raises_the_packages_exception() -> None:
    """The category map is payload-derived argument text, so it fails the same way."""
    with pytest.raises(OpportuneError) as raised:
        # An untyped caller's mapping: a tag read as bytes is not JSON text.
        Engine().calendar_view_to_blocks(
            {"value": []},
            "UTC",
            {"Interview": b"tags/interview"},  # pyright: ignore[reportArgumentType]
        )

    assert raised.value.code == "invalid_json"


@pytest.mark.filterwarnings("error")
def test_the_adapter_stores_a_selector_the_model_would_accept() -> None:
    """The rule the adapter hands back is validated, so a value that is not a
    selector is refused where it is written rather than serialised later.

    ``model_copy(update=...)`` skips validation, so a replacement written as the
    wire's own shape would be stored as a bare ``dict`` and only complain — with a
    ``UserWarning`` — when the rule is finally serialised. The replacement is the
    model's whole-value validation, raised as the package's own rejection so the
    handler an application installs for ``OpportuneError`` sees it too.
    """
    with pytest.raises(OpportuneError) as raised:
        graph.working_hours_to_rule(WORKING_HOURS, who={"bad": 1})
    assert raised.value.code == "invalid_argument"

    outcome = graph.working_hours_to_rule(WORKING_HOURS, who={"ids": ["ana"]})
    assert isinstance(outcome, AvailableWorkingHoursOutcome), outcome
    assert outcome.rule.who == IdsSelector(ids=["ana"]), "a wire-shaped dict is adopted, validated"


def test_a_span_collection_that_is_not_one_raises_the_packages_exception() -> None:
    """`spans` is checked as shape before it is walked, as each span is after.

    A caller forwarding a column that came back empty passes `None` where the
    signature says a sequence of RFC 3339 instant pairs. That is the same argument
    the preset already refuses when a single entry is mis-shaped, so it is refused
    the same way rather than as the loop's own `TypeError`.
    """
    for spans in (None, 5):
        with pytest.raises(OpportuneError) as raised:
            presets.available_only("cand", spans)  # pyright: ignore[reportArgumentType]

        assert raised.value.code == "invalid_argument"
        assert "`spans`" in raised.value.message, raised.value.message


# ------------------------------------------------------- async entry points ---


class _GatedEngine(Engine):
    """An engine whose search waits for the event loop to open a gate.

    Only the loop opens it, so an entry point that ran the search on the loop's own
    thread blocks until the wait expires and reports that the loop never ran while
    the engine searched. That is the failure a blocking ``*_async`` method
    produces, and the one a socket server sees as every request stalling.
    """

    def __init__(self) -> None:
        super().__init__()
        self.gate = threading.Event()

    def find_availability(self, request: Request | Mapping[str, object]) -> Response:
        if not self.gate.wait(timeout=10):
            raise AssertionError("the event loop never ran while the engine searched")
        return super().find_availability(request)


def test_an_async_call_lets_the_event_loop_run_while_the_engine_searches() -> None:
    engine = _GatedEngine()
    request = _request(("2027-06-14T09:00Z", "2027-06-14T11:00Z"))

    async def drive() -> Response:
        search = asyncio.create_task(engine.find_availability_async(request))
        await asyncio.sleep(0)
        engine.gate.set()
        return await search

    response = asyncio.run(drive())

    assert next(offer.events[0].start for offer in response.offers) == "2027-06-14T09:00Z"


def test_the_async_entry_points_answer_what_the_synchronous_ones_do() -> None:
    engine = Engine()
    request = _request(("2027-06-14T09:00Z", "2027-06-14T11:00Z"))

    async def drive() -> tuple[Response, str, Diagnostics, str]:
        return (
            await engine.find_availability_async(request),
            await engine.find_availability_json_async(request),
            await engine.validate_async(request),
            await engine.validate_json_async(request),
        )

    response, response_json, diagnostics, diagnostics_json = asyncio.run(drive())

    assert response.model_dump_json(by_alias=True) == engine.find_availability(
        request
    ).model_dump_json(by_alias=True)
    assert response_json == engine.find_availability_json(request)
    assert diagnostics.errors == []
    assert diagnostics.model_dump_json() == engine.validate(request).model_dump_json()
    assert diagnostics_json == engine.validate_json(request)


def test_the_async_slot_check_and_ranges_answer_what_the_synchronous_ones_do() -> None:
    engine = Engine()
    request = _request(("2027-06-14T09:00Z", "2027-06-14T11:00Z"))
    proposal = Proposal(
        events=[ProposalEvent(part="main", start="2027-06-14T09:00Z", assignment=[["ana"]])]
    )
    query = RangesQuery(
        now=request.now,
        goal=request.goal,
        participants=request.participants,
        rules=request.rules,
    )

    async def drive() -> tuple[SlotCheck, str, RequiredRanges, str]:
        return (
            await engine.check_slot_async(request, proposal),
            await engine.check_slot_json_async(request, proposal),
            await engine.required_ranges_async(query),
            await engine.required_ranges_json_async(query),
        )

    check, check_json, ranges, ranges_json = asyncio.run(drive())

    assert check.valid is True
    assert check.model_dump_json(by_alias=True) == engine.check_slot(
        request, proposal
    ).model_dump_json(by_alias=True)
    assert check_json == engine.check_slot_json(request, proposal)
    assert ranges.calendar == ("2027-06-13T00:00Z", "2027-06-16T00:00Z")
    assert ranges.model_dump_json(by_alias=True) == engine.required_ranges(query).model_dump_json(
        by_alias=True
    )
    assert ranges_json == engine.required_ranges_json(query)


def test_the_module_level_async_function_answers_from_the_shared_engine() -> None:
    request = _request(("2027-06-14T09:00Z", "2027-06-14T11:00Z"))

    answer = asyncio.run(find_availability_async(request))

    assert answer.model_dump_json(by_alias=True) == find_availability(request).model_dump_json(
        by_alias=True
    )


_ASYNC_OPERATIONS = [
    ("find_availability_async", "find_availability"),
    ("find_availability_json_async", "find_availability_json"),
    ("check_slot_async", "check_slot"),
    ("check_slot_json_async", "check_slot_json"),
    ("validate_async", "validate"),
    ("validate_json_async", "validate_json"),
    ("required_ranges_async", "required_ranges"),
    ("required_ranges_json_async", "required_ranges_json"),
]
"""Every async entry point beside the synchronous call it has to offload."""


@pytest.mark.parametrize(("method", "operation"), _ASYNC_OPERATIONS)
def test_every_async_entry_point_runs_its_synchronous_call_off_the_loop(
    method: str, operation: str
) -> None:
    """All eight await the work off the loop, not only the route's own search.

    An async method whose body called the synchronous one directly would answer
    every answer-equality test above and stall the server exactly as the blocking
    call does. Each is driven the way the search's gate test drives its own: the
    synchronous call waits on a gate only the loop thread opens, so running it on
    the loop deadlocks until the wait expires.
    """
    engine = Engine()
    gate = threading.Event()
    entered = threading.Event()
    request = _request(("2027-06-14T09:00Z", "2027-06-14T11:00Z"))
    proposal = Proposal(
        events=[ProposalEvent(part="main", start="2027-06-14T09:00Z", assignment=[["ana"]])]
    )
    query = RangesQuery(
        now=request.now,
        goal=request.goal,
        participants=request.participants,
        rules=request.rules,
    )
    arguments: tuple[object, ...] = (
        (request, proposal)
        if operation.startswith("check_slot")
        else (query,)
        if operation.startswith("required_ranges")
        else (request,)
    )
    synchronous = getattr(engine, operation)

    def waits(*given: object) -> object:
        entered.set()
        if not gate.wait(timeout=10):
            raise AssertionError(f"{method} ran {operation} on the event loop")
        return synchronous(*given)

    setattr(engine, operation, waits)

    async def drive() -> object:
        work = asyncio.create_task(getattr(engine, method)(*arguments))
        await asyncio.sleep(0)
        gate.set()
        return await work

    asyncio.run(drive())

    assert entered.is_set(), f"{method} never ran {operation}"


class _RecordingPool(ThreadPoolExecutor):
    """A one-worker pool that records the thread each piece of work ran on."""

    def __init__(self) -> None:
        super().__init__(max_workers=1)
        self.threads: set[int] = set()

    def submit[T](self, fn: Callable[..., T], /, *args: Any, **kwargs: Any) -> Future[T]:
        """Submit `fn` wrapped so the thread that actually ran it is recorded."""

        def recorded() -> T:
            self.threads.add(threading.get_ident())
            return fn(*args, **kwargs)

        return super().submit(recorded)


def test_a_supplied_executor_runs_the_search_on_its_own_worker_thread() -> None:
    """``executor=`` keeps the CPU-bound search off the loop and off the shared pool.

    ``asyncio.to_thread`` queues work on the loop's default pool, which every other
    library in the process shares; a service under load passes its own bounded
    executor so searches cannot be stuck behind unrelated blocking work. What is
    asserted is that the supplied pool — not the default one — ran the search, and
    that it ran on a worker rather than on the awaiting thread.
    """
    engine = Engine()
    request = _request(("2027-06-14T09:00Z", "2027-06-14T11:00Z"))
    expected = engine.find_availability(request)
    pool = _RecordingPool()
    loop_thread = threading.get_ident()
    try:
        answer = asyncio.run(engine.find_availability_async(request, executor=pool))
    finally:
        pool.shutdown()

    assert answer == expected, "the supplied executor answers what the synchronous call does"
    assert len(pool.threads) == 1, "the supplied executor ran the search exactly once"
    assert loop_thread not in pool.threads, "the search did not run on the awaiting thread"


def test_the_module_level_async_functions_forward_a_supplied_executor() -> None:
    """The free functions take the same keyword, so a route may pass its own pool."""
    request = _request(("2027-06-14T09:00Z", "2027-06-14T11:00Z"))
    pool = _RecordingPool()
    try:
        answer = asyncio.run(find_availability_async(request, executor=pool))
    finally:
        pool.shutdown()

    assert answer == find_availability(request)
    assert len(pool.threads) == 1, "the pool passed through the free function ran the search"


# ------------------------------------------------------- argument boundaries ---


def test_a_path_object_names_the_time_zone_directory() -> None:
    """A configured application holds a `Path`, not the string it renders to."""
    engine = Engine(tzdb_dir=TZDB_2018)

    assert engine.tzdb_version == "2018f", "the fixture directory's own `+VERSION`"
    assert repr(engine) == "Engine(tzdb_version='2018f')"


def test_a_time_zone_directory_that_is_not_a_path_is_the_packages_rejection() -> None:
    """A configuration holding a non-path is the package's error, not `os.fspath`'s.

    `Engine(tzdb_dir=...)` documents one failure class for a database it cannot
    build, so a value a settings object handed over — `os.fspath` raises `TypeError`
    for anything that is neither text nor `os.PathLike` — is answered by the class an
    `except OpportuneError` handler already catches, exactly as `tzdb_version` is.
    """
    with pytest.raises(OpportuneError) as raised:
        Engine(tzdb_dir=5)  # pyright: ignore[reportArgumentType]

    assert raised.value.code == "invalid_argument"
    assert "time zone directory" in raised.value.message, raised.value.message


@pytest.mark.parametrize(
    "version",
    [None, -1, 1.5, "1", 2**32],
    ids=["none", "negative", "float", "text", "past-32-bits"],
)
def test_an_upgrade_rejects_a_from_version_that_is_not_a_schema_version(
    version: object,
) -> None:
    """The native functions take a `u32`; the package answers first, in its own class."""
    rule = '{"id": "r_min", "kind": "lead_time", "params": {"min": "4h"}}'

    with pytest.raises(OpportuneError) as raised:
        upgrade_rule(rule, version)  # pyright: ignore[reportArgumentType]

    assert raised.value.code == "invalid_argument"
    assert "schema version" in raised.value.message


def test_a_text_argument_that_is_not_text_is_the_packages_rejection() -> None:
    """`None` in a zone column is a rejection an `OpportuneError` handler already has.

    A zone, a rule ID, a version label and a stored document are all text the native
    layer reads, and every one of them would otherwise leave as `PyO3`'s own
    ``TypeError`` — a class an ``except OpportuneError`` handler never catches.
    """
    engine = Engine()

    for zone in (None, 5):
        with pytest.raises(OpportuneError) as raised:
            engine.calendar_view_to_blocks(NEW_YORK_PAYLOAD, zone)  # pyright: ignore[reportArgumentType]
        assert raised.value.code == "invalid_argument"
        assert "participant zone" in raised.value.message

    with pytest.raises(OpportuneError) as raised:
        google.events_list_to_blocks('{ "items": [] }', None)  # pyright: ignore[reportArgumentType]
    assert raised.value.code == "invalid_argument"

    with pytest.raises(OpportuneError) as raised:
        engine.working_hours_to_rule(WORKING_HOURS, 5)  # pyright: ignore[reportArgumentType]
    assert raised.value.code == "invalid_argument"
    assert "rule ID" in raised.value.message

    with pytest.raises(OpportuneError) as raised:
        Engine(tzdb_dir=TZDB_2018, tzdb_version=5)  # pyright: ignore[reportArgumentType]
    assert raised.value.code == "invalid_argument"
    assert "time zone database version" in raised.value.message

    with pytest.raises(OpportuneError) as raised:
        upgrade_calendar(5, 1)  # pyright: ignore[reportArgumentType]
    assert raised.value.code == "invalid_argument"
    assert "stored calendar" in raised.value.message


def test_a_payload_that_is_a_list_of_pairs_is_not_a_mapping() -> None:
    """`dict([("value", [])])` would read the pair list as a payload nobody wrote."""
    with pytest.raises(OpportuneError) as raised:
        Engine().calendar_view_to_blocks([("value", [])], "UTC")  # pyright: ignore[reportArgumentType]

    assert raised.value.code == "invalid_argument"
    assert "list" in raised.value.message


def test_a_document_held_as_a_mapping_is_read_as_the_model() -> None:
    """A request from a queue, a fixture file or a JSON column is a mapping."""
    request = _request(("2027-06-14T09:00Z", "2027-06-14T11:00Z"))
    document: dict[str, Any] = json.loads(request.model_dump_json(by_alias=True, exclude_none=True))
    engine = Engine(limits=default_limits().model_dump())

    from_mapping = engine.find_availability(document)

    assert from_mapping.model_dump_json(by_alias=True) == engine.find_availability(
        request
    ).model_dump_json(by_alias=True)


def test_a_value_that_is_not_a_document_is_the_packages_rejection() -> None:
    """Neither a model nor a mapping is answered by the package, not by the encoder."""
    engine = Engine()

    with pytest.raises(OpportuneError) as raised:
        engine.find_availability("not a request")  # pyright: ignore[reportArgumentType]
    assert raised.value.code == "invalid_request"
    assert "the request is neither a Request nor a mapping" in raised.value.message

    with pytest.raises(OpportuneError) as raised:
        engine.check_slot(
            _request(("2027-06-14T09:00Z", "2027-06-14T11:00Z")),
            {"events": [{"part": "main"}]},
        )
    assert raised.value.code == "invalid_request"
    assert "the proposal is not a Proposal" in raised.value.message


def test_a_malformed_mapping_is_the_callers_document_on_every_entry_point() -> None:
    """A malformed mapping a route forwards is the caller's document, so it is a ``422``.

    The engine reads the request, the proposal and the ranges query from mappings a
    caller may hold — a JSON body, a queue message, a fixture — and one that is
    malformed is the caller's to fix. Classified as ``invalid_argument`` it read as
    this package's own argument checking, which a handler answers ``500``, so a
    client whose typo the engine had found was told the deployment was broken.
    Every entry point answers ``invalid_request`` with the findings that name the
    fields to change, and the engine's own rejection of a well-formed document is
    untouched beside it.
    """
    engine = Engine()
    request = _request(("2027-06-14T09:00Z", "2027-06-14T11:00Z"))
    malformed: dict[str, Any] = {"now": 5}

    for call in (
        lambda: engine.find_availability(malformed),
        lambda: engine.find_availability_json(malformed),
        lambda: asyncio.run(engine.find_availability_async(malformed)),
        lambda: asyncio.run(engine.find_availability_json_async(malformed)),
        lambda: engine.validate(malformed),
        lambda: engine.validate_json(malformed),
        lambda: asyncio.run(engine.validate_async(malformed)),
        lambda: asyncio.run(engine.validate_json_async(malformed)),
        lambda: engine.required_ranges(malformed),
        lambda: engine.required_ranges_json(malformed),
        lambda: asyncio.run(engine.required_ranges_async(malformed)),
        lambda: asyncio.run(engine.required_ranges_json_async(malformed)),
        lambda: engine.check_slot(malformed, Proposal(events=[])),
        lambda: engine.check_slot(request, {"events": [{"part": "main"}]}),
        lambda: asyncio.run(engine.check_slot_async(malformed, Proposal(events=[]))),
        lambda: asyncio.run(engine.check_slot_json_async(request, {"events": [{"part": "main"}]})),
        lambda: find_availability(malformed),
        lambda: validate(malformed),
        lambda: required_ranges(malformed),
    ):
        with pytest.raises(OpportuneError) as raised:
            call()

        error = raised.value
        assert error.code == "invalid_request", error.message
        assert http_status_for_error(error) == 422, error.code
        assert error.errors, "the client is told which fields to change"


def test_a_refused_mapping_names_the_field_within_the_rule_it_is_about() -> None:
    """A forwarded mapping's findings point where the route's own ``422`` points.

    The engine reads a request, a proposal and a ranges query from mappings, and a
    finding that named the wrong place — the validator's own union tag, an index
    into the union's arms, the wire layer's ``body`` parameter — sends the client to
    a field that is fine. A nested field inside a discriminated rule is the case
    that tells a pointer into the client's document from one that kept a tag: `cap`
    is the arm the validator picked, not a key the client wrote, and the one bad
    value is the eleventh field of a mapping whose other twelve are valid.
    """
    engine = Engine()
    document: dict[str, Any] = {
        "now": "2027-06-14T08:00Z",
        "goal": {
            "parts": [{"duration": "30m", "requirement": {"all": [{"person": "ana"}]}}],
            "search_window": ["2027-06-14T09:00Z", "2027-06-14T11:00Z"],
            "zone": "UTC",
        },
        "participants": [{"id": "ana", "zone": "UTC"}],
        "rules": [
            {
                "id": "capped",
                "kind": "cap",
                "params": {
                    "count": True,
                    "limit": "x",
                    "measure": "count",
                    "per": "each",
                    "period": "day",
                    "zone": "UTC",
                },
            }
        ],
        "limit": 5,
    }

    with pytest.raises(OpportuneError) as raised:
        engine.find_availability(document)

    findings = raised.value.errors
    assert [(finding.code, finding.path) for finding in findings] == [
        ("int_parsing", "/rules/0/params/limit")
    ], findings
    # The sentence a handler logs and serves has to name the caller's own pointer too.
    # Pydantic's location still holds the `cap` tag the union added to say which arm
    # it picked, so a message built from it names a key the caller never wrote and
    # disagrees with the finding beside it in the one body.
    assert findings[0].path is not None
    assert findings[0].path in raised.value.message, raised.value.message
    assert "cap/params" not in raised.value.message, raised.value.message


def test_a_refused_ranges_query_names_its_field_from_either_document() -> None:
    """The query's own findings and a request document's point at the same field.

    ``required_ranges`` reads a mapping as the query or as the request the query is
    projected from, and the pointer resolver is told about neither document class
    by name — it walks the documents the models declare. A finding inside the goal
    therefore has to name ``/goal/parts/0/duration`` whether or not the mapping
    carried a request-only field, which is what decides which document it was read
    as.
    """
    engine = Engine()
    goal: dict[str, Any] = {
        "parts": [{"duration": 5, "requirement": {"all": [{"person": "ana"}]}}],
        "search_window": ["2027-06-14T09:00Z", "2027-06-14T11:00Z"],
        "zone": "UTC",
    }

    documents: list[dict[str, object]] = [
        {"now": "2027-06-14T08:00Z", "goal": goal},
        {"now": "2027-06-14T08:00Z", "goal": goal, "occurrences": []},
    ]
    for document in documents:
        with pytest.raises(OpportuneError) as raised:
            engine.required_ranges(document)

        assert [(finding.code, finding.path) for finding in raised.value.errors] == [
            ("string_type", "/goal/parts/0/duration")
        ], document


def test_required_ranges_reads_a_request_as_its_query() -> None:
    """A request carries the query's four fields, so a caller hands it over as it is.

    ``now``, ``goal``, ``participants`` and ``rules`` are the whole of what a ranges
    query reads; everything else a request carries is about the search that follows
    it — the occurrences a cap counts, the named calendars, the offer and explain
    limits, the weights, the events being moved. A caller holding the request should
    not have to project it by hand, so the query is read out of it, and a request
    document read as a mapping is projected the same way.
    """
    engine = Engine()
    request = _request(("2027-06-14T09:00Z", "2027-06-14T11:00Z"))
    query = RangesQuery(
        now=request.now,
        goal=request.goal,
        participants=request.participants,
        rules=request.rules,
    )
    expected = engine.required_ranges(query)

    document = json.loads(request.model_dump_json(by_alias=True, exclude_none=True))
    document.update(
        {
            "calendars": {"org": {"working_days": "mon-fri", "zone": "UTC"}},
            "occurrences": [],
            "explain": {"limit": 3},
            "limit": 5,
            "weights": {"ana": 2},
            "exclude_event_keys": ["existing-booking"],
        }
    )

    for given in (request, query, document, request.model_dump()):
        assert engine.required_ranges(given) == expected, type(given).__name__

    assert required_ranges(request) == expected
    assert asyncio.run(required_ranges_async(document)) == expected
    assert RequiredRanges.model_validate_json(engine.required_ranges_json(request)) == expected


def test_required_ranges_still_refuses_a_document_it_cannot_read() -> None:
    """The projection reads known fields; it does not make the reader more forgiving.

    A misspelled key is still the query's own finding rather than something to drop,
    and a request-only field a caller did write is validated as the request's before
    it is left behind: a value the engine would refuse is refused here, not skipped.
    """
    engine = Engine()
    document = json.loads(
        _request(("2027-06-14T09:00Z", "2027-06-14T11:00Z")).model_dump_json(
            by_alias=True, exclude_none=True
        )
    )

    for override in ({"limitt": 1}, {"now": 5}, {"limit": "x"}, {"occurrences": 5}):
        with pytest.raises(OpportuneError) as raised:
            engine.required_ranges({**document, **override})

        error = raised.value
        assert error.code == "invalid_request", override
        assert error.errors, override
        assert http_status_for_error(error) == 422


def test_an_out_of_range_proposal_is_the_packages_rejection_with_its_pointer() -> None:
    """A booking whose required travel has no span past the model's range.

    The rejection keeps the stable code and the pointer into the proposal, and the
    search withholds the same start instead of offering a booking a caller could
    not persist.
    """
    engine = Engine()
    request = Request(
        now="2099-12-31T22:00Z",
        goal=Goal(
            parts=[
                Part(
                    duration="30m",
                    requirement=Requirement(all=[person("ana")]),
                    travel=Travel(before="0m", after="1h"),
                )
            ],
            search_window=("2099-12-31T22:00Z", "2100-01-01T00:00Z"),
            zone="UTC",
        ),
        participants=[Participant(id="ana", zone="UTC", calendar=[])],
    )
    proposal = Proposal(
        events=[
            ProposalEvent(
                part="main",
                start="2099-12-31T23:30Z",
                assignment=[["ana"]],
            )
        ]
    )

    with pytest.raises(OpportuneError) as raised:
        engine.check_slot(request, proposal)

    error = raised.value
    assert error.code == "proposal_out_of_range"
    assert error.path == "/events/0/start"
    assert "ana" in error.message

    searching = with_updates(
        request,
        goal=with_updates(request.goal, candidates=["2099-12-31T23:30Z"]),
        explain=Explain(limit=10),
    )
    response = engine.find_availability(searching)
    assert response.offers == [], "the withheld start is never offered"
    assert response.excluded == [], "and no scheduling reason is invented for it"


def test_category_map_non_mapping_and_invalid_engine_argument_are_rejected() -> None:
    """Non-mapping `category_map` and non-`Engine` adapter arguments raise
    `OpportuneError('invalid_argument')` instead of coercing or raising `AttributeError`."""
    engine = Engine()
    for bad_map in ([("Interview", "interview")], ["ab"], 123):
        with pytest.raises(OpportuneError) as raised:
            engine.calendar_view_to_blocks(
                {"value": []},
                "UTC",
                bad_map,  # pyright: ignore[reportArgumentType]
            )
        assert raised.value.code == "invalid_argument", repr(bad_map)
        assert http_status_for_error(raised.value) == 500

    for call in (
        lambda: graph.calendar_view_to_blocks({"value": []}, "UTC", engine="not-an-engine"),  # pyright: ignore[reportArgumentType]
        lambda: google.events_list_to_blocks({"items": []}, "UTC", engine=123),  # pyright: ignore[reportArgumentType]
    ):
        with pytest.raises(OpportuneError) as raised:
            call()
        assert raised.value.code == "invalid_argument"
        assert "`engine`" in raised.value.message


def test_document_segments_confirms_tuple_elements_in_mapping_validation() -> None:
    """Mappings holding Python tuples (such as `Span`) confirm accurately in
    `error_envelope_from_validation_error`."""
    bad_request: dict[str, object] = {
        "now": "2027-06-14T08:00Z",
        "goal": {
            "parts": [{"duration": "30m", "requirement": {"all": ({"person": 5},)}}],
            "search_window": ("2027-06-14T09:00Z", 5),
            "zone": "UTC",
        },
        "participants": [{"id": "ana", "zone": "UTC"}],
    }
    with pytest.raises(OpportuneError) as raised:
        find_availability(bad_request)

    assert raised.value.code == "invalid_request"
    assert sorted((f.code, f.path) for f in raised.value.errors) == [
        ("string_type", "/goal/parts/0/requirement/all/0/person"),
        ("string_type", "/goal/search_window/1"),
    ]


# -------------------------------------------- answers the models must read back ---


def test_an_answer_the_models_cannot_read_is_the_packages_server_failure(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    """A drift between the writer and the generated models is a 500, not a traceback.

    The engine writes from the same schema these models are generated from, so an
    answer one of them refuses is a defect in this build. Without the wrapper the
    ``ValidationError`` escapes the package — a handler written for
    :class:`~opportune.OpportuneError`, the one exception the package promises, never
    sees it — and the route answers an unhandled-failure traceback. The wrapper
    classifies it ``serialization_failed``, which an application answers ``500``
    without the message or the pointer, and keeps the model's findings on ``__cause__``.
    """
    engine = Engine()
    request = _request(("2027-06-14T09:00Z", "2027-06-14T11:00Z"))
    proposal = Proposal(
        events=[ProposalEvent(part="main", start="2027-06-14T09:00Z", assignment=[["ana"]])]
    )
    stranded = '{"offers": "not the array the model reads"}'

    def stranded_answer(_self: Engine, _request: object) -> str:
        """The engine's own text, replaced by one the models refuse."""
        return stranded

    def stranded_json(_self: Engine, *_args: object) -> str:
        """The same substitution for the entry point each operation reads."""
        return stranded

    monkeypatch.setattr(Engine, "find_availability_json", stranded_answer)

    with pytest.raises(OpportuneError) as raised:
        engine.find_availability(request)

    error = raised.value
    assert error.code == "serialization_failed"
    assert http_status_for_error(error) == 500, "the client's document was not at fault"
    assert error.errors == [], "a deployment's failure carries no client-facing findings"
    assert isinstance(error.__cause__, ValidationError), "the model's findings stay on the cause"
    assert "response" in error.message

    for method, operation, arguments in (
        ("check_slot_json", "check_slot", (request, proposal)),
        ("validate_json", "validate", (request,)),
        ("required_ranges_json", "required_ranges", (request,)),
    ):
        monkeypatch.setattr(Engine, method, stranded_json)
        with pytest.raises(OpportuneError) as raised:
            getattr(engine, operation)(*arguments)
        assert raised.value.code == "serialization_failed", operation
        monkeypatch.undo()


def test_an_adapter_outcome_the_models_cannot_read_is_the_packages_server_failure(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    """The adapter answers are read through the same wrapper as the search's.

    A conversion is the one path where the models read a document the *Rust adapter*
    wrote rather than the engine's own serializer, so it is the most likely place for
    the two to drift; the outcome is classified the same way, and the route answers
    ``500`` rather than an unhandled ``ValidationError``.
    """
    engine_module = importlib.import_module("opportune.engine")

    class Drifted:
        """A reader that refuses every answer, as a stricter model would."""

        def validate_json(self, _: str) -> object:
            """Refuse the answer the way a stricter model would: by reading a bad one."""
            Response.model_validate_json('{"dates": "not the array the model reads"}')
            raise AssertionError("the drifted document validated")

    monkeypatch.setattr(engine_module, "_CALENDAR_OUTCOME", Drifted())

    with pytest.raises(OpportuneError) as raised:
        Engine().calendar_view_to_blocks({"value": []}, "UTC")

    assert raised.value.code == "serialization_failed"
    assert http_status_for_error(raised.value) == 500
    assert isinstance(raised.value.__cause__, ValidationError)


def test_a_mapping_keyed_by_something_other_than_text_is_refused() -> None:
    """JSON object keys are text, so a mapping with another key type has no spelling.

    ``json.dumps`` would coerce ``{1: "interview"}`` to ``{"1": "interview"}`` and hand
    the adapter a category map the caller never wrote, exactly as ``dict()`` would read
    a list of pairs as a mapping. Both are refused as the package's own rejection
    rather than silently rewritten.
    """
    engine = Engine()

    for bad_map in ({1: "interview"}, {True: "interview"}, {("a", "b"): "interview"}):
        with pytest.raises(OpportuneError) as raised:
            engine.calendar_view_to_blocks(
                {"value": []},
                "UTC",
                bad_map,  # pyright: ignore[reportArgumentType]
            )
        assert raised.value.code == "invalid_argument", repr(bad_map)
        assert "key that is not text" in raised.value.message, raised.value.message

    with pytest.raises(OpportuneError) as raised:
        engine.get_schedule_to_blocks({1: "value"})  # pyright: ignore[reportArgumentType]
    assert raised.value.code == "invalid_argument"
    assert "key that is not text" in raised.value.message, raised.value.message


def test_the_default_limits_are_a_fresh_copy_for_every_caller() -> None:
    """The validated template is cached; the model a caller gets is not.

    ``default_limits`` is read on every ``limits_with`` override, so the document is
    validated once. ``Limits`` validates assignment, so handing the same instance to
    two callers would let one of them write a default the next one starts from.
    """
    first = default_limits()
    first.max_participants = 1

    second = default_limits()

    assert second is not first
    assert second.max_participants == 500, "one caller's assignment is not the next's default"
    assert default_limits().max_participants == 500


def test_the_module_level_json_async_functions_answer_the_synchronous_text() -> None:
    """The four text entry points have the async twins the typed ones already had.

    A route that hands the engine's own document on untouched — the fast path — also
    runs on the shared engine, so it needs the awaited form; without it the route has
    to reach for ``default_engine()``, which is not the package's surface.
    """
    request = _request(("2027-06-14T09:00Z", "2027-06-14T11:00Z"))
    proposal = Proposal(
        events=[ProposalEvent(part="main", start="2027-06-14T09:00Z", assignment=[["ana"]])]
    )

    import opportune

    for name in (
        "find_availability_json_async",
        "check_slot_json_async",
        "validate_json_async",
        "required_ranges_json_async",
    ):
        assert name in opportune.__all__, name
        assert hasattr(opportune, name), name

    assert asyncio.run(find_availability_json_async(request)) == opportune.find_availability_json(
        request
    )
    assert asyncio.run(opportune.check_slot_json_async(request, proposal)) == (
        opportune.check_slot_json(request, proposal)
    )
    assert asyncio.run(validate_json_async(request)) == opportune.validate_json(request)
    assert asyncio.run(required_ranges_json_async(request)) == (
        opportune.required_ranges_json(request)
    )
