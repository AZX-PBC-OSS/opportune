"""Recorded provider payloads through the Python adapters into a real search.

The scenarios are the ones the Rust adapter suite proves end to end in
`crates/opportune-adapters/tests/graph_engine_test.rs` and `google_engine_test.rs`,
rebuilt through the public Python surface: the same recorded payloads convert, the
same request goes to `opportune.Engine`, and the same offers, exclusions and
`check_slot` answer come back. A conversion that drops, widens or mistags a block
shows up as wrong availability rather than as a green adapter unit test.
"""

from collections.abc import Callable

from opportune import (
    AnnotatedBlock,
    AvailableCalendarOutcome,
    AvailableWorkingHoursOutcome,
    Block,
    Engine,
    Explain,
    Goal,
    IdsSelector,
    Part,
    Participant,
    Request,
    Requirement,
    Response,
    StatusPolicyEntry,
    StatusPolicyParams,
    StatusPolicyRule,
    TagSelector,
    UnavailableCalendarOutcome,
    WindowRule,
    offer_calendar_unknown,
    offer_start,
    offer_to_proposal,
    person,
    presets,
    with_updates,
)
from opportune.adapters import google, graph

CATEGORIES = {"Interview": "interview"}
"""The category map the Rust end-to-end scenario passes to the Graph adapter."""


def _graph_mailbox(payload: str, participant_zone: str) -> list[AnnotatedBlock]:
    outcome = graph.calendar_view_to_blocks(payload, participant_zone, CATEGORIES)
    assert isinstance(outcome, AvailableCalendarOutcome), outcome
    assert not outcome.diagnostics, outcome.diagnostics
    return outcome.blocks


def _google_calendar(payload: str, participant_zone: str) -> list[AnnotatedBlock]:
    outcome = google.events_list_to_blocks(payload, participant_zone)
    assert isinstance(outcome, AvailableCalendarOutcome), outcome
    assert not outcome.diagnostics, outcome.diagnostics
    return outcome.blocks


def _free_busy_calendar(payload: str, calendar: str) -> list[Block]:
    outcome = google.free_busy_to_blocks(payload)[calendar]
    assert isinstance(outcome, AvailableCalendarOutcome), outcome
    assert not outcome.diagnostics, outcome.diagnostics
    return [entry.block for entry in outcome.blocks]


def _block_for(entries: list[AnnotatedBlock], event_id: str) -> Block:
    return next(entry.block for entry in entries if entry.event_id == event_id)


def _blocks(entries: list[AnnotatedBlock]) -> list[Block]:
    return [entry.block for entry in entries]


def _graph_mailboxes(
    adapter_payload: Callable[[str], str],
) -> tuple[list[AnnotatedBlock], list[AnnotatedBlock]]:
    return (
        _graph_mailbox(adapter_payload("graph/calendar_view_ana.json"), "America/New_York"),
        _graph_mailbox(adapter_payload("graph/calendar_view_bo.json"), "Europe/London"),
    )


def _graph_working_hours(payload: str) -> WindowRule:
    outcome = graph.working_hours_to_rule(payload)
    assert isinstance(outcome, AvailableWorkingHoursOutcome), outcome
    outcome.rule.who = IdsSelector(ids=["ana"])
    return outcome.rule


def _graph_request(working_hours: WindowRule, ana: list[Block], bo: list[Block]) -> Request:
    return Request(
        now="2027-03-08T00:00Z",
        goal=Goal(
            zone="UTC",
            search_window=("2027-03-08T00:00Z", "2027-03-13T00:00Z"),
            parts=[
                Part(
                    duration="1h",
                    requirement=Requirement(all=[person("ana"), person("bo")]),
                    tags=["interview"],
                )
            ],
        ),
        participants=[
            Participant(id="ana", zone="America/New_York", calendar=ana),
            Participant(id="bo", zone="Europe/London", calendar=bo),
        ],
        rules=[
            working_hours,
            presets.rest(),
            presets.cap_per_day(
                TagSelector(tag="interview"),
                2,
                per="pooled",
                zone="event",
                rule_id="interviews_per_day",
            ),
        ],
        explain=Explain(limit=1000),
    )


def _google_request(ana: list[Block], bo: list[Block], room: list[Block]) -> Request:
    hours = presets.business_hours("mon-fri", "09:00-17:00")
    hours.who = IdsSelector(ids=["ana"])
    focus_policy = StatusPolicyRule(
        id="focus_policy",
        kind="status_policy",
        params=StatusPolicyParams(
            entries=[StatusPolicyEntry(match=TagSelector(tag="status/focus"), effect="ignore")]
        ),
    )
    return Request(
        now="2027-03-08T00:00Z",
        goal=Goal(
            zone="UTC",
            search_window=("2027-03-08T00:00Z", "2027-03-13T00:00Z"),
            parts=[
                Part(
                    duration="1h",
                    requirement=Requirement(all=[person("ana"), person("bo"), person("room-4")]),
                    tags=["interview"],
                )
            ],
        ),
        participants=[
            Participant(id="ana", zone="America/New_York", calendar=ana),
            Participant(id="bo", zone="Asia/Tokyo", calendar=bo),
            Participant(id="room-4", zone="UTC", calendar=room),
        ],
        rules=[
            hours,
            focus_policy,
            presets.cap_per_day(
                TagSelector(tag="interview"),
                2,
                per="pooled",
                zone="event",
                rule_id="interviews_per_day",
            ),
        ],
        explain=Explain(limit=1000),
    )


def _google_calendars(
    adapter_payload: Callable[[str], str],
) -> tuple[list[AnnotatedBlock], list[AnnotatedBlock], list[Block]]:
    return (
        _google_calendar(adapter_payload("google/events_list_ana.json"), "America/New_York"),
        _google_calendar(adapter_payload("google/events_list_bo.json"), "Asia/Tokyo"),
        _free_busy_calendar(adapter_payload("google/free_busy.json"), "room-4@example.com"),
    )


def _starts(response: Response) -> list[str]:
    return [offer_start(offer) for offer in response.offers]


def _starts_between(starts: list[str], first: str, until: str) -> list[str]:
    return [start for start in starts if first <= start < until]


def _reasons_between(
    response: Response, first: str, until: str
) -> list[tuple[str | None, str, str | None]]:
    return [
        (reason.participant, reason.code, reason.rule)
        for excluded in response.excluded
        if first <= excluded.start < until
        for reason in excluded.reasons
    ]


def _confirmed(engine: Engine, request: Request, start: str) -> bool:
    booked = next(
        offer for offer in engine.find_availability(request).offers if offer_start(offer) == start
    )
    return engine.check_slot(request, offer_to_proposal(booked)).valid


def test_the_two_recorded_mailboxes_share_one_event_key_per_shared_event(
    adapter_payload: Callable[[str], str],
) -> None:
    ana, bo = _graph_mailboxes(adapter_payload)

    shared = _block_for(ana, "AAMkAGI2THAAA-ana-interview-0900")
    assert shared.event_key == "040000008200E00074C5B7101A82E00800000000INTERVIEW0900"
    assert _block_for(bo, "AAMkAGI2THBBB-interview-0900").event_key == shared.event_key, (
        "the same event in two mailboxes shares one key, which is what a pooled cap dedupes on"
    )

    day_off = _block_for(ana, "AAMkAGI2THAAA-ana-pto-0311")
    assert day_off.all_day
    assert day_off.span == ("2027-03-11T05:00Z", "2027-03-12T05:00Z"), (
        "an all-day event resolves from its civil date in the participant's zone"
    )


def test_recorded_mailboxes_become_the_offers_the_rules_describe(
    adapter_payload: Callable[[str], str],
) -> None:
    ana, bo = _graph_mailboxes(adapter_payload)
    engine = Engine()
    request = _graph_request(
        _graph_working_hours(adapter_payload("graph/working_hours.json")),
        _blocks(ana),
        _blocks(bo),
    )

    assert engine.validate(request).errors == []
    response = engine.find_availability(request)

    assert response.offers, "the recorded mailboxes leave bookable time"
    for offer in response.offers:
        assert "2027-03-08T14:00Z" <= offer_start(offer)
        assert offer.events[-1].end <= "2027-03-12T22:00Z", (
            f"the offer at {offer_start(offer)} runs past Ana's working hours"
        )

    starts = _starts(response)
    assert "2027-03-09T14:00Z" in starts, (
        "the shared interview counts once for the pooled cap, leaving room on 9 March"
    )
    assert _starts_between(starts, "2027-03-10T00:00Z", "2027-03-10T22:00Z") == [], (
        "two interviews plus a booking pass the cap of two on 10 March"
    )
    assert ("ana", "cap_reached", "interviews_per_day") in _reasons_between(
        response, "2027-03-10T00:00Z", "2027-03-11T00:00Z"
    ), "the exclusions on 10 March name the cap"
    assert _starts_between(starts, "2027-03-11T00:00Z", "2027-03-12T05:00Z") == [], (
        "the all-day block occupies 11 March, so no offer starts inside it"
    )

    assert _confirmed(engine, request, "2027-03-09T14:00Z"), (
        "the offer the search produced is one `check_slot` confirms"
    )


def test_a_timed_block_triggers_the_rest_rule_that_an_all_day_block_does_not(
    adapter_payload: Callable[[str], str],
) -> None:
    ana, bo = _graph_mailboxes(adapter_payload)
    working_hours = _graph_working_hours(adapter_payload("graph/working_hours.json"))
    engine = Engine()

    def offered(blocks: list[Block]) -> bool:
        response = engine.find_availability(_graph_request(working_hours, blocks, _blocks(bo)))
        return "2027-03-12T14:00Z" in _starts(response)

    assert offered(_blocks(ana)), (
        "an all-day block is not rest activity, so 09:00 on 12 March is offered"
    )
    timed = [
        with_updates(block, all_day=False) if block.all_day else block for block in _blocks(ana)
    ]
    assert not offered(timed), (
        "the same block reported as timed is rest activity, so the start it protects is not offered"
    )


def test_the_two_recorded_calendars_share_one_key_per_recurring_instance(
    adapter_payload: Callable[[str], str],
) -> None:
    ana, bo, room = _google_calendars(adapter_payload)

    shared = _block_for(ana, "ana-shared-interview-0900")
    assert shared.event_key == "series-interview@example.com@2027-03-09T15:00:00Z"
    assert _block_for(bo, "bo-shared-interview-0900").event_key == shared.event_key, (
        "the two attendees received the instance in different offsets and derived one key"
    )
    assert _block_for(ana, "ana-shared-interview-1000").event_key == (
        "series-interview@example.com@2027-03-10T15:00:00Z"
    ), "a distinct instance derives a distinct key"
    assert _block_for(bo, "bo-shared-interview-1000").event_key == (
        "series-interview@example.com@2027-03-10T15:00:00Z"
    )

    focus = _block_for(ana, "ana-focus-0312")
    assert focus.status == "busy"
    assert focus.tags == ["status/focus"]
    day_off = _block_for(ana, "ana-pto-0311")
    assert day_off.all_day
    assert day_off.span == ("2027-03-11T05:00Z", "2027-03-12T05:00Z")

    assert len(room) == 2
    assert all(block.event_key is None and block.status == "busy" for block in room), (
        "free/busy ranges carry no event identity"
    )


def test_recorded_calendars_become_the_offers_the_rules_describe(
    adapter_payload: Callable[[str], str],
) -> None:
    ana, bo, room = _google_calendars(adapter_payload)
    engine = Engine()
    request = _google_request(_blocks(ana), _blocks(bo), room)

    assert engine.validate(request).errors == []
    response = engine.find_availability(request)

    assert response.offers, "the recorded calendars leave bookable time"
    for offer in response.offers:
        assert "2027-03-08T14:00Z" <= offer_start(offer)
        assert offer.events[-1].end <= "2027-03-12T22:00Z", (
            f"the offer at {offer_start(offer)} runs past Ana's working hours"
        )

    starts = _starts(response)
    assert "2027-03-09T14:00Z" in starts, "the shared interview counts once for the pooled cap"
    assert "2027-03-09T16:00Z" not in starts, "the room's free/busy range blocks that start"
    assert "2027-03-09T17:00Z" in starts, "the room is free again after its busy range"
    assert _starts_between(starts, "2027-03-10T00:00Z", "2027-03-11T00:00Z") == [], (
        "two interviews plus a booking pass the cap of two on 10 March"
    )
    assert ("ana", "cap_reached", "interviews_per_day") in _reasons_between(
        response, "2027-03-10T00:00Z", "2027-03-11T00:00Z"
    ), "the exclusions on 10 March name the cap"
    assert _starts_between(starts, "2027-03-11T00:00Z", "2027-03-12T05:00Z") == [], (
        "the all-day day off occupies 11 March, so no offer starts inside it"
    )
    assert "2027-03-12T14:00Z" in starts, "a `status/focus` block the policy ignores does not block"

    assert _confirmed(engine, request, "2027-03-09T14:00Z"), (
        "the offer the search produced is one `check_slot` confirms"
    )


def test_focus_time_blocks_when_the_policy_does_not_ignore_it(
    adapter_payload: Callable[[str], str],
) -> None:
    ana, bo, room = _google_calendars(adapter_payload)
    engine = Engine()
    request = _google_request(_blocks(ana), _blocks(bo), room)
    without_policy = with_updates(
        request,
        rules=[rule for rule in request.rules or [] if rule.id != "focus_policy"],
    )

    starts = _starts(engine.find_availability(without_policy))

    assert "2027-03-12T14:00Z" not in starts, (
        "the focus block covers 14:00-16:00Z on 12 March once nothing ignores it"
    )
    assert "2027-03-12T16:00Z" in starts


def test_an_errored_mailbox_is_an_unknown_calendar_rather_than_a_free_one(
    adapter_payload: Callable[[str], str],
) -> None:
    """A mailbox Graph reported an error for must never read as completely free.

    The whole path is the caller's: the adapter answers `unavailable` for the
    mailbox, the caller passes that host with no calendar, and every offer resting
    on it is flagged `calendar_unknown` so an application can chase the calendar
    before it confirms the booking.
    """
    outcomes = graph.get_schedule_to_blocks(adapter_payload("graph/get_schedule.json"))

    room = outcomes["room-vienna@example.com"]
    assert isinstance(room, UnavailableCalendarOutcome), room
    assert "not resolvable" in room.message, "the provider's own cause is kept"

    ana = outcomes["ana@example.com"]
    assert isinstance(ana, AvailableCalendarOutcome), ana

    request = Request(
        now="2027-03-09T08:00Z",
        goal=Goal(
            zone="UTC",
            search_window=("2027-03-09T14:00Z", "2027-03-09T17:00Z"),
            parts=[
                Part(
                    duration="30m",
                    requirement=Requirement(
                        all=[person("ana@example.com"), person("room-vienna@example.com")]
                    ),
                )
            ],
        ),
        participants=[
            Participant(id="ana@example.com", zone="UTC", calendar=_blocks(ana.blocks)),
            Participant(id="room-vienna@example.com", zone="UTC", calendar=None),
        ],
    )

    response = Engine().find_availability(request)

    assert response.offers, "the healthy mailbox leaves bookable time"
    assert all(offer_calendar_unknown(offer) for offer in response.offers), (
        "an offer that rests on the unknown calendar says so"
    )
    assert _starts_between(_starts(response), "2027-03-09T15:00Z", "2027-03-09T16:00Z") == [], (
        "the healthy mailbox's busy hour still blocks"
    )
