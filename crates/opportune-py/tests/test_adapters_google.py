"""The Google Calendar adapter wrappers: payloads in, generated models out.

As with the Graph wrappers, these tests pin the Python surface: the payload forms
accepted, the marker constants, that a per-calendar provider error is an
``unavailable`` outcome rather than an empty calendar, and that converted blocks
drive a real search.
"""

import json
from collections.abc import Callable
from typing import Any

import pytest

from opportune import (
    AvailableCalendarOutcome,
    Block,
    Engine,
    Explain,
    OpportuneError,
    Participant,
    Request,
    Requirement,
    UnavailableCalendarOutcome,
    offer_start,
    person,
    presets,
    single_goal,
)
from opportune.adapters import google


def _request(calendar: list[Block]) -> Request:
    return Request(
        now="2027-03-09T12:00Z",
        goal=single_goal(
            "1h",
            Requirement(all=[person("ana")]),
            zone="UTC",
            search_window=("2027-03-09T14:00Z", "2027-03-09T18:00Z"),
        ),
        participants=[Participant(id="ana", zone="America/Los_Angeles", calendar=calendar)],
        rules=[presets.business_hours("mon-sun", "00:00-24:00")],
        explain=Explain(limit=100),
    )


def test_events_list_converts_the_same_from_text_bytes_and_a_parsed_dict(
    adapter_payload: Callable[[str], str],
) -> None:
    payload = adapter_payload("google/events_list_ana.json")
    parsed: dict[str, Any] = json.loads(payload)

    from_text = google.events_list_to_blocks(payload, "America/Los_Angeles")
    from_bytes = google.events_list_to_blocks(payload.encode(), "America/Los_Angeles")
    from_dict = google.events_list_to_blocks(parsed, "America/Los_Angeles")

    assert from_bytes == from_text
    assert from_dict == from_text
    assert isinstance(from_text, AvailableCalendarOutcome)
    blocks = {entry.event_id: entry.block for entry in from_text.blocks}
    assert blocks["ana-shared-interview-0900"].event_key == (
        "series-interview@example.com@2027-03-09T15:00:00Z"
    )
    assert blocks["ana-focus-0312"].tags == ["status/focus"]
    assert blocks["ana-deep-work-0312"].status == "free", "transparent events do not block"


def test_the_marker_constants_are_exported() -> None:
    assert google.MARKER_TAGS == "opportune.tags"
    assert google.MARKER_EVENT_KEY == "opportune.event_key"


def test_a_calendar_the_provider_errored_on_is_unavailable_not_empty(
    adapter_payload: Callable[[str], str],
) -> None:
    calendars = google.free_busy_to_blocks(adapter_payload("google/free_busy.json"))

    errored = calendars["room-vienna@example.com"]
    assert isinstance(errored, UnavailableCalendarOutcome)
    assert errored.code == "provider_error"

    healthy = calendars["room-4@example.com"]
    assert isinstance(healthy, AvailableCalendarOutcome)
    assert [entry.block.span for entry in healthy.blocks] == [
        ("2027-03-09T16:00Z", "2027-03-09T17:00Z"),
        ("2027-03-10T03:00Z", "2027-03-10T04:00Z"),
    ]
    assert all(
        entry.block.event_key is None and not entry.block.all_day for entry in healthy.blocks
    )

    empty = calendars["bo@example.com"]
    assert isinstance(empty, AvailableCalendarOutcome)
    assert empty.blocks == [], "an answered calendar with no busy ranges is known to be free"


def test_an_unreadable_payload_is_an_unavailable_outcome() -> None:
    outcome = google.events_list_to_blocks("{not json", "UTC")

    assert isinstance(outcome, UnavailableCalendarOutcome)
    assert outcome.code == "payload_invalid"


def test_a_blank_participant_zone_is_rejected(adapter_payload: Callable[[str], str]) -> None:
    with pytest.raises(OpportuneError) as raised:
        google.events_list_to_blocks(adapter_payload("google/events_list_ana.json"), "   ")

    assert raised.value.code == "zone_name_invalid"
    assert raised.value.path == "/participant_zone"


def test_converted_blocks_drive_a_search_and_explain_themselves(
    adapter_payload: Callable[[str], str],
) -> None:
    engine = Engine()
    outcome = google.events_list_to_blocks(
        adapter_payload("google/events_list_ana.json"),
        "America/Los_Angeles",
        engine=engine,
    )
    assert isinstance(outcome, AvailableCalendarOutcome)

    response = engine.find_availability(_request([entry.block for entry in outcome.blocks]))

    starts = [offer_start(offer) for offer in response.offers]
    assert starts[0] == "2027-03-09T14:00Z"
    assert "2027-03-09T15:00Z" not in starts, "the converted interview blocks that start"
    assert "2027-03-09T16:00Z" in starts, "the afternoon frees up once the interview ends"
    excluded = {entry.start: entry for entry in response.excluded}
    blocked = excluded["2027-03-09T15:00Z"]
    assert [reason.code for reason in blocked.reasons] == ["blocked"]
    assert blocked.reasons[0].event_key == "series-interview@example.com@2027-03-09T15:00:00Z"
