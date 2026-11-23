"""The Graph adapter wrappers: payloads in, generated models out.

The payloads are the ones the Rust adapter suite records, so these tests pin the
Python surface rather than re-testing conversion rules: that the wrappers accept
the three payload forms an application has (``str``, ``bytes``, and
``httpx.Response.json()``'s ``dict``), that a provider failure is an ``unavailable``
outcome rather than an empty calendar, that the marker constants are reachable,
and that converted blocks drive a real search.
"""

import json
from collections.abc import Callable
from pathlib import Path
from typing import Any

import pytest
from pydantic import ValidationError

from opportune import (
    AvailableCalendarOutcome,
    AvailableWorkingHoursOutcome,
    Block,
    Engine,
    Explain,
    Goal,
    IdsSelector,
    OpportuneError,
    Part,
    Participant,
    Request,
    Requirement,
    TagSelector,
    UnavailableCalendarOutcome,
    UnavailableWorkingHoursOutcome,
    offer_start,
    person,
    presets,
    single_goal,
)
from opportune.adapters import graph

TZDB_2018 = Path(__file__).parents[2] / "opportune" / "tests" / "fixtures" / "tzdb-2018"
"""A `TZif` tree with no American zones, so a conversion has to answer `unavailable`."""


def _request(calendar: list[Block]) -> Request:
    return Request(
        now="2027-03-09T12:00Z",
        goal=single_goal(
            "1h",
            Requirement(all=[person("ana")]),
            zone="UTC",
            search_window=("2027-03-09T14:00Z", "2027-03-09T17:00Z"),
        ),
        participants=[Participant(id="ana", zone="America/Los_Angeles", calendar=calendar)],
        rules=[presets.business_hours("mon-sun", "00:00-24:00")],
        explain=Explain(limit=100),
    )


def test_a_calendar_view_converts_the_same_from_text_bytes_and_a_parsed_dict(
    adapter_payload: Callable[[str], str],
) -> None:
    payload = adapter_payload("graph/calendar_view_ana.json")
    parsed: dict[str, Any] = json.loads(payload)

    from_text = graph.calendar_view_to_blocks(
        payload, "America/Los_Angeles", {"Interview": "interview"}
    )
    from_bytes = graph.calendar_view_to_blocks(
        payload.encode(), "America/Los_Angeles", {"Interview": "interview"}
    )
    from_dict = graph.calendar_view_to_blocks(
        parsed, "America/Los_Angeles", {"Interview": "interview"}
    )

    assert from_bytes == from_text
    assert from_dict == from_text
    assert isinstance(from_text, AvailableCalendarOutcome)
    first = from_text.blocks[0]
    assert first.block.span == ("2027-03-09T15:00Z", "2027-03-09T16:00Z")
    assert first.block.tags == ["interview", "interview/front-end"], "the map plus the marker"
    assert first.block.event_key == "040000008200E00074C5B7101A82E00800000000INTERVIEW0900"


def test_the_marker_and_extended_property_constants_are_exported() -> None:
    assert graph.MARKER_TAGS == "opportune.tags"
    assert graph.MARKER_EVENT_KEY == "opportune.event_key"
    assert graph.EXTENDED_PROPERTY_NAMESPACE_GUID
    assert graph.EXTENDED_PROPERTY_TAGS_ID.endswith(graph.MARKER_TAGS)
    assert graph.EXTENDED_PROPERTY_EVENT_KEY_ID.endswith(graph.MARKER_EVENT_KEY)
    assert graph.EXTENDED_PROPERTY_TAGS_ID in graph.EXTENDED_PROPERTY_EXPAND


def test_an_unreadable_payload_is_an_unavailable_outcome_not_an_empty_calendar() -> None:
    outcome = graph.calendar_view_to_blocks("{not json", "UTC")

    assert isinstance(outcome, UnavailableCalendarOutcome)
    assert outcome.code == "payload_invalid"
    assert outcome.message


def test_a_schedule_the_provider_errored_on_isolates_that_calendar(
    adapter_payload: Callable[[str], str],
) -> None:
    schedules = graph.get_schedule_to_blocks(adapter_payload("graph/get_schedule.json"))

    errored = schedules["room-vienna@example.com"]
    assert isinstance(errored, UnavailableCalendarOutcome)
    assert errored.code == "provider_error"

    healthy = schedules["ana@example.com"]
    assert isinstance(healthy, AvailableCalendarOutcome)
    assert [entry.block.status for entry in healthy.blocks] == ["busy", "free"]
    assert all(
        entry.block.event_key is None and not entry.block.all_day for entry in healthy.blocks
    )
    assert [diagnostic.code for diagnostic in healthy.diagnostics] == ["zero_length_event"]


def test_working_hours_become_a_window_rule_and_keep_a_custom_zone_unavailable(
    adapter_payload: Callable[[str], str],
) -> None:
    outcome = graph.working_hours_to_rule(adapter_payload("graph/working_hours.json"))

    assert isinstance(outcome, AvailableWorkingHoursOutcome)
    assert outcome.rule.id == "working_hours"
    assert outcome.rule.params.zone == "America/New_York"
    assert outcome.rule.params.weekly == {
        "mon": ["09:00-17:00"],
        "tue": ["09:00-17:00"],
        "wed": ["09:00-17:00"],
        "thu": ["09:00-17:00"],
        "fri": ["09:00-17:00"],
    }

    custom = {
        "daysOfWeek": ["monday"],
        "startTime": "09:00:00.0000000",
        "endTime": "17:00:00.0000000",
        "timeZone": {"@odata.type": "#microsoft.graph.customTimeZone", "bias": 60},
    }
    unavailable = graph.working_hours_to_rule(custom)
    assert isinstance(unavailable, UnavailableWorkingHoursOutcome)
    assert unavailable.code == "custom_time_zone"

    scoped = graph.working_hours_to_rule(
        adapter_payload("graph/working_hours.json"),
        rule_id="office_hours",
        who=IdsSelector(ids=["ana"]),
        when=TagSelector(tag="onsite"),
    )
    assert isinstance(scoped, AvailableWorkingHoursOutcome)
    assert scoped.rule.id == "office_hours"
    assert scoped.rule.who == IdsSelector(ids=["ana"])
    assert scoped.rule.when == TagSelector(tag="onsite")


def test_the_engine_method_scopes_a_working_hours_rule_as_the_helper_does(
    adapter_payload: Callable[[str], str],
) -> None:
    """The helper is a delegate, so the two doors cannot answer differently.

    ``Engine.working_hours_to_rule`` takes the same ``who``/``when`` scoping the
    module-level helper offers, on whichever engine the caller built. An available
    outcome, an unavailable one from a stripped database and a refused selector each
    have to answer the same way through either door, and the engine the caller built
    is the one that reads the payload's zones.
    """
    payload = adapter_payload("graph/working_hours.json")
    engine = Engine()
    who = IdsSelector(ids=["ana"])
    when = TagSelector(tag="onsite")

    expected = graph.working_hours_to_rule(
        payload, "office_hours", who=who, when=when, engine=engine
    )
    assert isinstance(expected, AvailableWorkingHoursOutcome), expected
    assert engine.working_hours_to_rule(payload, "office_hours", who=who, when=when) == expected
    assert expected.rule.who == who and expected.rule.when == when

    stripped = Engine(tzdb_dir=str(TZDB_2018))
    unavailable = stripped.working_hours_to_rule(payload)
    assert isinstance(unavailable, UnavailableWorkingHoursOutcome), unavailable
    assert graph.working_hours_to_rule(payload, engine=stripped) == unavailable

    def refusal(call: Callable[..., object]) -> tuple[str, str]:
        with pytest.raises(OpportuneError) as raised:
            call(payload, who={"idz": ["ana"]})
        return raised.value.code, raised.value.message

    method_refusal = refusal(engine.working_hours_to_rule)
    assert method_refusal == refusal(graph.working_hours_to_rule), (
        "one implementation, so the method's rejection is the helper's"
    )
    assert method_refusal[0] == "invalid_argument"
    assert "`idz`" in method_refusal[1], "the dispatch's one finding reaches either door"


def test_a_selector_the_rule_refuses_is_the_packages_error(
    adapter_payload: Callable[[str], str],
) -> None:
    """`who`/`when` replace the converted rule's selectors, validated as one whole.

    The replacement goes through the rule model, so a selector the wire does not
    read is refused there. The caller's ``except OpportuneError`` handler — the one
    a FastAPI application registers — has to see that rejection: a bare Pydantic
    ``ValidationError`` escapes every handler written for the package and surfaces
    as a 500 on a request the client can fix.
    """
    with pytest.raises(OpportuneError) as raised:
        graph.working_hours_to_rule(
            adapter_payload("graph/working_hours.json"),
            who={"idz": ["ana"]},
        )

    assert raised.value.code == "invalid_argument"
    assert "`who`" in raised.value.message, raised.value.message
    assert "`idz`" in raised.value.message, "the dispatch's one finding reaches the caller"
    assert isinstance(raised.value.__cause__, ValidationError), "the model's findings stay attached"


def test_a_converted_working_hours_rule_constrains_a_real_search(
    adapter_payload: Callable[[str], str],
) -> None:
    """The returned rule is one the engine reads, not merely a well-shaped model."""
    outcome = graph.working_hours_to_rule(adapter_payload("graph/working_hours.json"))
    assert isinstance(outcome, AvailableWorkingHoursOutcome)

    response = Engine().find_availability(
        Request(
            now="2027-03-09T00:00Z",
            goal=Goal(
                zone="UTC",
                search_window=("2027-03-10T00:00Z", "2027-03-11T00:00Z"),
                parts=[Part(duration="1h", requirement=Requirement(all=[person("ana")]))],
            ),
            participants=[Participant(id="ana", zone="America/New_York", calendar=[])],
            rules=[outcome.rule],
        )
    )

    starts = [offer_start(offer) for offer in response.offers]
    assert starts[0] == "2027-03-10T14:00Z", "09:00 in New York, where the mailbox's day opens"
    assert starts[-1] == "2027-03-10T21:00Z", "an hour before 17:00, where it closes"
    assert "2027-03-10T13:00Z" not in starts
    assert "2027-03-10T22:00Z" not in starts


def test_converted_blocks_drive_a_search_and_explain_themselves(
    adapter_payload: Callable[[str], str],
) -> None:
    engine = Engine()
    outcome = graph.calendar_view_to_blocks(
        adapter_payload("graph/calendar_view_ana.json"),
        "America/Los_Angeles",
        {"Interview": "interview"},
        engine=engine,
    )
    assert isinstance(outcome, AvailableCalendarOutcome)

    response = engine.find_availability(_request([entry.block for entry in outcome.blocks]))

    starts = [offer_start(offer) for offer in response.offers]
    assert starts[0] == "2027-03-09T14:00Z"
    assert "2027-03-09T15:00Z" not in starts, "the converted interview blocks that start"
    excluded = {entry.start: entry for entry in response.excluded}
    blocked = excluded["2027-03-09T15:00Z"]
    assert [reason.code for reason in blocked.reasons] == ["blocked"]
    assert blocked.reasons[0].event_key == outcome.blocks[0].block.event_key


def test_a_blank_participant_zone_is_rejected(adapter_payload: Callable[[str], str]) -> None:
    with pytest.raises(OpportuneError) as raised:
        graph.calendar_view_to_blocks(adapter_payload("graph/calendar_view_ana.json"), "   ")

    assert raised.value.code == "zone_name_invalid"
    assert raised.value.path == "/participant_zone"
