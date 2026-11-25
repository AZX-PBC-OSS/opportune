"""End-to-end booking-product scenarios through the public Python surface.

Each test exercises a real backend workflow — field-service travel composing
with clearance and prior-job travel, split and per-weekday hours, multi-duration
public links, multi-resource sales calls with optional room tiering, conference
pool windows, event-type caps, minimum notice, provider ID correlation, and
last-second slot confirmation — using only the generated Pydantic models,
presets, sugar, adapters and `Engine`.
"""

import json
from collections.abc import Callable

import pytest
from pydantic import TypeAdapter

from opportune import (
    AvailableCalendarOutcome,
    BlackoutParams,
    BlackoutRule,
    Block,
    CalendarSpec,
    DaysMax,
    Engine,
    Explain,
    Goal,
    Grid,
    Offer,
    OpportuneError,
    Part,
    Participant,
    ParticipantTravel,
    Proposal,
    ProposalEvent,
    Request,
    Requirement,
    Response,
    RoleSelector,
    Rule,
    TagSelector,
    Travel,
    WindowParams,
    WindowRule,
    WorkingDaysMax,
    at_least,
    offer_assigned_hosts,
    offer_end,
    offer_reserve,
    offer_start,
    offer_to_proposal,
    person,
    presets,
    single_goal,
    upgrade_calendar,
    upgrade_goal,
    upgrade_rule,
    with_updates,
)
from opportune.adapters import google, graph

NY = "America/New_York"


def _block(
    start: str,
    end: str,
    *,
    tags: list[str] | None = None,
    event_key: str | None = None,
    travel_before: str | None = None,
    travel_after: str | None = None,
) -> Block:
    return Block(
        span=(start, end),
        status="busy",
        tags=tags if tags is not None else [],
        event_key=event_key,
        travel_before=travel_before,
        travel_after=travel_after,
    )


def _days_with_offers(response: Response) -> list[str]:
    return [f"{entry.date}={entry.offers}" for entry in response.dates if entry.offers > 0]


def _holds(offer: Offer) -> list[str]:
    return [
        f"{hold.kind} {hold.participant} {hold.span[0]}..{hold.span[1]}"
        for hold in offer_reserve(offer)
    ]


def _sales_team(*, room_busy: bool) -> list[Participant]:
    room_calendar = (
        [_block("2027-03-08T14:00Z", "2027-03-08T18:00Z", event_key="room-b-busy")]
        if room_busy
        else []
    )
    return [
        Participant(id="sales-1", role="host", zone=NY, calendar=[], tags=["sales"]),
        Participant(id="se-1", role="host", zone=NY, calendar=[], tags=["sales-engineer"]),
        Participant(id="se-2", role="host", zone=NY, calendar=[], tags=["sales-engineer"]),
        Participant(
            id="room-a",
            role="host",
            kind="resource",
            zone=NY,
            tags=["room"],
            calendar=[_block("2027-03-08T14:00Z", "2027-03-08T18:00Z", event_key="room-a-busy")],
        ),
        Participant(
            id="room-b",
            role="host",
            kind="resource",
            zone=NY,
            tags=["room"],
            calendar=room_calendar,
        ),
    ]


def test_field_service_travel_composes_with_clearance_and_exempts_customer() -> None:
    """Driving time to a new job composes with prior-job travel and clearance,
    reserves travel spans only for the engineer, and rejects a clash naming the
    prior job's event key.
    """
    engine = Engine()
    bob = Participant(
        id="bob",
        role="host",
        zone=NY,
        calendar=[
            _block(
                "2027-03-08T18:00Z",
                "2027-03-08T20:00Z",
                tags=["job/plumbing"],
                event_key="job-101",
                travel_after="20m",
            )
        ],
    )
    joe = Participant(id="joe", role="attendee", zone=NY, calendar=[])
    request = Request(
        now="2027-03-06T12:00Z",
        goal=Goal(
            zone=NY,
            search_window=("2027-03-08T18:00Z", "2027-03-08T23:00Z"),
            candidates=["2027-03-08T20:54Z", "2027-03-08T20:55Z"],
            parts=[
                Part(
                    id="visit",
                    duration="60m",
                    tags=["job/plumbing"],
                    requirement=Requirement(all=[person("bob")]),
                    travel=Travel(
                        before="25m",
                        after="25m",
                        by_participant={"joe": ParticipantTravel(before="0m", after="0m")},
                    ),
                )
            ],
        ),
        participants=[bob, joe],
        rules=[
            presets.business_hours("mon-sat", "08:00-18:00"),
            presets.buffers("10m", "10m"),
        ],
        explain=Explain(limit=10),
    )

    response = engine.find_availability(request)
    assert [offer_start(offer) for offer in response.offers] == ["2027-03-08T20:55Z"]
    assert _holds(response.offers[0]) == [
        "travel bob 2027-03-08T20:30Z..2027-03-08T20:55Z",
        "travel bob 2027-03-08T21:55Z..2027-03-08T22:20Z",
    ]
    assert len(response.excluded) == 1
    assert response.excluded[0].start == "2027-03-08T20:54Z"
    assert [(r.code, r.participant, r.rule) for r in response.excluded[0].reasons] == [
        ("clearance", "bob", "buffers")
    ]

    # Confirmed booking agrees on the exact travel holds, while a start inside
    # the prior job's travel-after span fails citing that job's event_key.
    confirmed = engine.check_slot(request, offer_to_proposal(response.offers[0]))
    assert confirmed.valid is True
    assert [
        f"{h.kind} {h.participant} {h.span[0]}..{h.span[1]}" for h in confirmed.reserve
    ] == _holds(response.offers[0])

    too_soon = Proposal(
        events=[ProposalEvent(part="visit", start="2027-03-08T20:15Z", assignment=[["bob"]])]
    )
    refused = engine.check_slot(request, too_soon)
    assert refused.valid is False
    assert [(r.code, r.participant, r.event_key) for r in refused.reasons] == [
        ("blocked", "bob", "job-101")
    ]


def test_split_and_per_weekday_working_hours() -> None:
    engine = Engine()
    request = Request(
        now="2027-03-06T12:00Z",
        goal=Goal(
            zone=NY,
            search_window=("2027-03-08T00:00Z", "2027-03-15T00:00Z"),
            parts=[Part(id="main", duration="30m", requirement=Requirement(all=[person("ana")]))],
        ),
        participants=[Participant(id="ana", role="host", zone=NY, calendar=[])],
        rules=[
            WindowRule(
                id="hours",
                kind="window",
                params=WindowParams(
                    zone="participant",
                    weekly={
                        "mon-thu": ["08:30-12:00", "13:00-18:00"],
                        "fri": ["09:00-15:00"],
                        "sat": ["10:00-12:00"],
                    },
                ),
            )
        ],
        limit=100,
    )

    response = engine.find_availability(request)
    assert _days_with_offers(response) == [
        "2027-03-08=17",
        "2027-03-09=17",
        "2027-03-10=17",
        "2027-03-11=17",
        "2027-03-12=12",
        "2027-03-13=4",
    ]
    # 12:00-13:00 New York is 17:00Z-18:00Z on 2027-03-08 (EST).
    lunch = [
        offer_start(o)
        for o in response.offers
        if "2027-03-08T16:45Z" <= offer_start(o) <= "2027-03-08T17:30Z"
    ]
    assert lunch == []


def test_public_event_types_across_durations() -> None:
    engine = Engine()
    day_one_counts: list[int] = []
    for duration in ("30m", "45m", "60m"):
        request = Request(
            now="2027-03-05T12:00Z",
            goal=Goal(
                zone=NY,
                search_window=("2027-03-08T00:00Z", "2027-03-20T00:00Z"),
                grid=Grid(step="15m", zone=NY),
                parts=[
                    Part(
                        id="main",
                        duration=duration,
                        requirement=Requirement(all=[at_least(1, RoleSelector(role="host"))]),
                    )
                ],
            ),
            participants=[
                Participant(id="ana", role="host", zone=NY, calendar=[]),
                Participant(id="invitee", role="attendee", zone="Europe/Berlin", calendar=[]),
            ],
            rules=[
                presets.business_hours(),
                presets.buffers("10m", "10m"),
                presets.min_notice("4h"),
                presets.horizon(DaysMax(days=30)),
                presets.rest(),
            ],
            limit=20,
        )
        response = engine.find_availability(request)
        day_one = next(entry.offers for entry in response.dates if entry.date == "2027-03-08")
        day_one_counts.append(day_one)
        assert response.offers[0].flags == []
        assert offer_assigned_hosts(response.offers[0]) == ["ana"]

    assert day_one_counts == [31, 30, 29]


def test_multi_resource_sales_call_and_single_query_optional_room_tiering() -> None:
    engine = Engine()

    def make_request(
        room_count: int, *, room_busy: bool, weights: dict[str, int] | None = None
    ) -> Request:
        return Request(
            now="2027-03-06T12:00Z",
            goal=Goal(
                zone=NY,
                search_window=("2027-03-08T14:00Z", "2027-03-08T20:00Z"),
                parts=[
                    Part(
                        id="demo",
                        duration="60m",
                        tags=["demo"],
                        requirement=Requirement(
                            all=[
                                at_least(1, TagSelector(tag="sales-engineer")),
                                person("sales-1"),
                                at_least(room_count, TagSelector(tag="room")),
                            ]
                        ),
                    )
                ],
            ),
            participants=_sales_team(room_busy=room_busy),
            rules=[presets.business_hours(), presets.buffers("10m", "10m")],
            weights=weights if weights is not None else {},
        )

    # Required room: skips busy room-a, assigns room-b, and respects SE weights.
    required = engine.find_availability(
        make_request(1, room_busy=False, weights={"se-1": 2, "se-2": 8})
    )
    assert required.offers[0].events[0].assignment == [["se-2"], ["sales-1"], ["room-b"]]
    assert required.offers[0].events[0].eligible == [["se-1", "se-2"], ["sales-1"], ["room-b"]]

    # Optional room (`at_least=0`): no slot requires a room, so one query returns the
    # slots with no free room and the later slots that could take one. A UI badges the
    # second tier from `eligible[2]`, the pool the room term could be filled from.
    optional_req = make_request(0, room_busy=True)
    optional = engine.find_availability(optional_req)
    without_room = [o for o in optional.offers if not o.events[0].eligible[2]]
    with_room = [o for o in optional.offers if o.events[0].eligible[2]]
    assert without_room and with_room
    assert with_room[0].events[0].eligible[2] == ["room-a", "room-b"]
    assert with_room[0].events[0].assignment == [["se-1"], ["sales-1"], []]

    with_room_proposal = Proposal(
        events=[
            ProposalEvent(
                part="demo",
                start=offer_start(with_room[0]),
                assignment=[["se-1"], ["sales-1"], ["room-b"]],
            )
        ]
    )
    with pytest.raises(OpportuneError) as rejected:
        engine.check_slot(optional_req, with_room_proposal)
    assert rejected.value.code == "proposal_host_count"
    assert rejected.value.path == "/events/0/assignment/2"

    assert engine.check_slot(make_request(1, room_busy=True), with_room_proposal).valid is True


def test_conference_pool_window_and_event_type_caps() -> None:
    engine = Engine()
    delegates = [
        Participant(
            id=f"gtm-{index:02d}",
            role="host",
            zone=NY,
            tags=["gtm"],
            calendar=(
                [
                    _block(
                        "2027-03-09T14:00Z", "2027-03-09T20:00Z", event_key=f"gtm-{index:02d}-busy"
                    )
                ]
                if index % 3 == 0
                else []
            ),
        )
        for index in range(24)
    ]
    conference = Request(
        now="2027-03-06T12:00Z",
        goal=Goal(
            zone=NY,
            search_window=("2027-03-09T00:00Z", "2027-03-12T00:00Z"),
            parts=[
                Part(
                    id="booth",
                    duration="20m",
                    tags=["conference"],
                    requirement=Requirement(all=[at_least(1, TagSelector(tag="gtm"))]),
                )
            ],
        ),
        participants=delegates,
        rules=[
            WindowRule(
                id="conference_hours",
                kind="window",
                who=TagSelector(tag="gtm"),
                params=WindowParams(
                    zone=NY,
                    dates={
                        "2027-03-09": ["09:00-17:00"],
                        "2027-03-10": ["09:00-17:00"],
                        "2027-03-11": ["09:00-15:00"],
                    },
                ),
            ),
            presets.buffers("5m", "5m"),
        ],
        limit=200,
    )
    conf_answer = engine.find_availability(conference)
    assert len(conf_answer.offers) == 66
    assert conf_answer.truncated is False
    assert _days_with_offers(conf_answer) == [
        "2027-03-09=24",
        "2027-03-10=24",
        "2027-03-11=18",
    ]

    # Event-type cap closes the day for matching tagged parts only.
    def interview_request(part_tags: list[str]) -> Request:
        return Request(
            now="2027-03-06T12:00Z",
            goal=Goal(
                zone=NY,
                search_window=("2027-03-08T00:00Z", "2027-03-13T00:00Z"),
                parts=[
                    Part(
                        id="main",
                        duration="45m",
                        tags=part_tags,
                        requirement=Requirement(all=[person("ana")]),
                    )
                ],
            ),
            participants=[
                Participant(
                    id="ana",
                    role="host",
                    zone=NY,
                    calendar=[
                        _block(
                            "2027-03-08T14:00Z",
                            "2027-03-08T14:45Z",
                            tags=["interview"],
                            event_key="i-1",
                        ),
                        _block(
                            "2027-03-08T15:00Z",
                            "2027-03-08T15:45Z",
                            tags=["interview"],
                            event_key="i-2",
                        ),
                    ],
                )
            ],
            rules=[
                presets.business_hours(),
                presets.cap_per_day(TagSelector(tag="interview"), 2, per="each"),
            ],
            limit=60,
            explain=Explain(limit=500),
        )

    capped = engine.find_availability(interview_request(["interview"]))
    assert _days_with_offers(capped) == [
        "2027-03-09=15",
        "2027-03-10=15",
        "2027-03-11=15",
        "2027-03-12=15",
    ]
    cap_reasons = [
        reason
        for entry in capped.excluded
        for reason in entry.reasons
        if reason.code == "cap_reached"
    ]
    assert cap_reasons and cap_reasons[0].params is not None
    assert (
        cap_reasons[0].params.bucket,
        cap_reasons[0].params.used,
        cap_reasons[0].params.limit,
    ) == (
        "2027-03-08",
        2,
        2,
    )

    untagged = engine.find_availability(interview_request([]))
    assert next(e.offers for e in untagged.dates if e.date == "2027-03-08") == 11


def test_provider_ids_correlate_through_adapters_and_last_second_check_slot(
    adapter_payload: Callable[[str], str],
) -> None:
    engine = Engine()
    ana_outcome = google.events_list_to_blocks(
        adapter_payload("google/events_list_ana.json"), NY, engine=engine
    )
    bo_outcome = graph.calendar_view_to_blocks(
        adapter_payload("graph/calendar_view_bo.json"),
        NY,
        {"Interview": "interview"},
        engine=engine,
    )
    assert isinstance(ana_outcome, AvailableCalendarOutcome)
    assert isinstance(bo_outcome, AvailableCalendarOutcome)
    assert all(entry.event_id for entry in ana_outcome.blocks)
    assert all(entry.event_id for entry in bo_outcome.blocks)
    shared_key = bo_outcome.blocks[0].block.event_key
    assert shared_key

    request = Request(
        now="2027-03-08T12:00Z",
        goal=Goal(
            zone=NY,
            search_window=("2027-03-09T14:00Z", "2027-03-09T20:00Z"),
            parts=[
                Part(
                    id="panel",
                    duration="60m",
                    tags=["interview"],
                    requirement=Requirement(all=[person("ana"), person("bo")]),
                )
            ],
        ),
        participants=[
            Participant(
                id="ana", role="host", zone=NY, calendar=[b.block for b in ana_outcome.blocks]
            ),
            Participant(
                id="bo", role="host", zone=NY, calendar=[b.block for b in bo_outcome.blocks]
            ),
        ],
        rules=[presets.business_hours()],
    )

    # JSON fast path returns the same structure as the typed path.
    listed = engine.find_availability(request)
    raw_json = engine.find_availability_json(request)
    assert json.loads(raw_json) == json.loads(
        listed.model_dump_json(by_alias=True, exclude_none=True)
    )

    # Checking a clashing slot preserves the provider's event_key and tags.
    clash_proposal = Proposal(
        events=[
            ProposalEvent(part="panel", start="2027-03-09T15:00Z", assignment=[["ana"], ["bo"]])
        ]
    )
    clash = engine.check_slot(request, clash_proposal)
    assert clash.valid is False
    bo_reason = next(r for r in clash.reasons if r.participant == "bo")
    assert bo_reason.code == "blocked"
    assert bo_reason.event_key == shared_key
    assert "interview" in (bo_reason.tags or [])

    # Last-second confirmation on an offered slot succeeds, then detects a fresh clash.
    chosen = offer_to_proposal(listed.offers[0])
    assert engine.check_slot(request, chosen).valid is True
    assert json.loads(engine.check_slot_json(request, chosen))["valid"] is True

    race_block = _block(
        offer_start(listed.offers[0]),
        offer_end(listed.offers[0]),
        tags=["interview"],
        event_key="race-booking-99",
    )
    assert request.participants is not None
    raced = with_updates(
        request,
        participants=[
            with_updates(
                request.participants[0],
                calendar=[*(request.participants[0].calendar or []), race_block],
            ),
            request.participants[1],
        ],
    )
    raced_check = engine.check_slot(raced, chosen)
    assert raced_check.valid is False
    assert [(r.code, r.participant, r.event_key) for r in raced_check.reasons] == [
        ("blocked", "ana", "race-booking-99")
    ]

    # Internal override: off-grid start outside the original search window validates cleanly.
    off_grid = Proposal(
        events=[
            ProposalEvent(part="panel", start="2027-03-10T18:05Z", assignment=[["ana"], ["bo"]])
        ]
    )
    assert engine.check_slot(request, off_grid).valid is True


def test_stored_rules_goals_and_calendars_round_trip_for_document_stores() -> None:
    """Rules, goals and working calendars persist as canonical JSON documents
    (document stores, object storage, JSON columns, etc.), upgrade cleanly through
    `upgrade_*`, hydrate back into strict Pydantic models, and produce identical
    engine answers.
    """
    rule_adapter: TypeAdapter[Rule] = TypeAdapter(Rule)
    engine = Engine()

    original_rules: list[Rule] = [
        presets.business_hours("mon-fri", ["09:00-12:00", "13:00-17:00"]),
        presets.buffers("10m", "15m"),
        presets.min_notice("2h"),
        presets.cap_per_day(TagSelector(tag="interview"), 2, per="each"),
    ]
    original_goal = Goal(
        zone=NY,
        search_window=("2027-03-08T00:00Z", "2027-03-10T00:00Z"),
        parts=[
            Part(
                id="interview",
                duration="45m",
                tags=["interview"],
                requirement=Requirement(all=[person("ana")]),
            )
        ],
    )
    original_calendar = CalendarSpec(
        zone=NY,
        working_days="mon-fri",
        holidays=["2027-03-09"],
    )

    reloaded_rules = [
        rule_adapter.validate_json(
            upgrade_rule(rule.model_dump_json(by_alias=True, exclude_none=True), 1)
        )
        for rule in original_rules
    ]
    reloaded_goal = Goal.model_validate_json(
        upgrade_goal(original_goal.model_dump_json(by_alias=True, exclude_none=True), 1)
    )
    reloaded_calendar = CalendarSpec.model_validate_json(
        upgrade_calendar(original_calendar.model_dump_json(by_alias=True, exclude_none=True), 1)
    )

    before = Request(
        now="2027-03-06T12:00Z",
        goal=original_goal,
        participants=[Participant(id="ana", role="host", zone=NY, calendar=[])],
        rules=original_rules,
        calendars={"us-ny": original_calendar},
    )
    after = Request(
        now="2027-03-06T12:00Z",
        goal=reloaded_goal,
        participants=[Participant(id="ana", role="host", zone=NY, calendar=[])],
        rules=reloaded_rules,
        calendars={"us-ny": reloaded_calendar},
    )
    assert engine.find_availability_json(before) == engine.find_availability_json(after)


def test_a_calendar_holiday_moves_the_working_day_horizon() -> None:
    """`CalendarSpec.holidays` is the named calendar's own data, and the horizon reads it.

    A one-working-day horizon ends on the first working day after `now`, and a
    holiday is not a working day: the same request without the holiday offers starts
    on Monday and nothing on Tuesday, and with it Monday is no longer the first
    working day, so Tuesday is offered too.
    """
    engine = Engine()

    def request(holidays: list[str]) -> Request:
        return Request(
            now="2027-03-12T00:00Z",  # Friday
            goal=Goal(
                zone="UTC",
                search_window=("2027-03-12T00:00Z", "2027-03-17T00:00Z"),
                parts=[Part(duration="30m", requirement=Requirement(all=[person("ana")]))],
            ),
            participants=[Participant(id="ana", zone="UTC", calendar=[])],
            calendars={
                "org": CalendarSpec(
                    zone="UTC",
                    working_days="mon,tue,wed,thu,fri",
                    holidays=holidays,
                )
            },
            rules=[presets.horizon(WorkingDaysMax(calendar="org", working_days=1))],
        )

    def dates_with_offers(holidays: list[str]) -> list[str]:
        return [
            entry.date
            for entry in engine.find_availability(request(holidays)).dates
            if entry.offers
        ]

    plain = dates_with_offers([])
    holiday = dates_with_offers(["2027-03-15"])

    assert plain[-1] == "2027-03-15", "Monday is the first working day after Friday"
    assert "2027-03-16" not in plain
    assert holiday[-1] == "2027-03-16", "the holiday pushes the horizon to Tuesday"
    assert "2027-03-15" in holiday, "the holiday is a working-day count, not a closure"


def test_a_blackout_rule_withdraws_the_candidate_starts_it_covers() -> None:
    """A blackout's local spans remove starts whose own span overlaps them.

    The rule is keyed to the event zone and covers 10:00-11:00, so the 10:00 and
    10:30 starts (each booking 30 minutes) are withdrawn, the 09:30 start that ends
    where the blackout begins is offered, and the reason the excluded starts carry
    names the rule that removed them.
    """
    request = Request(
        now="2027-03-09T09:00Z",
        goal=single_goal(
            "30m",
            Requirement(all=[person("ana")]),
            zone="UTC",
            search_window=("2027-03-09T09:00Z", "2027-03-09T13:00Z"),
        ),
        participants=[Participant(id="ana", zone="UTC", calendar=[])],
        rules=[
            BlackoutRule(
                id="lunch",
                kind="blackout",
                params=BlackoutParams(
                    zone="event", spans=[("2027-03-09T10:00", "2027-03-09T11:00")]
                ),
            )
        ],
        explain=Explain(limit=10),
    )

    response = Engine().find_availability(request)

    starts = [offer_start(offer) for offer in response.offers]
    assert starts == [
        "2027-03-09T09:00Z",
        "2027-03-09T09:30Z",
        "2027-03-09T11:00Z",
        "2027-03-09T11:30Z",
        "2027-03-09T12:00Z",
        "2027-03-09T12:30Z",
    ], starts
    assert [entry.start for entry in response.excluded] == [
        "2027-03-09T10:00Z",
        "2027-03-09T10:30Z",
    ]
    assert [(reason.code, reason.rule) for reason in response.excluded[0].reasons] == [
        ("blackout", "lunch")
    ]
