"""U1 and the enterprise scenarios, built with presets and single-part sugar.

Every request here is built from the package's own surface — presets build the
rules, sugar builds the goal, terms and proposal — so a preset that produced a
model the engine rejects, or a sugar helper that drifted from the general shapes,
fails here rather than in an application.
"""

from collections.abc import Callable, Sequence

import pytest
from pydantic import ValidationError

import opportune
from opportune import (
    MARKER_EVENT_KEY,
    MARKER_TAGS,
    AllSelector,
    AtLeastTerm,
    Block,
    CalendarSpec,
    CapRule,
    DateMax,
    DaysMax,
    DurationMax,
    Engine,
    Explain,
    Goal,
    Grid,
    IdsSelector,
    KindSelector,
    Limits,
    NotSelector,
    Offer,
    OfferEvent,
    OpportuneError,
    Part,
    Participant,
    ParticipantTravel,
    PublicOffer,
    PublicResponse,
    PublicSlotCheck,
    Request,
    Requirement,
    ReserveSpan,
    Response,
    Role,
    RoleSelector,
    Rule,
    Selector,
    StatusPolicyEntry,
    StatusPolicyRule,
    TagSelector,
    Travel,
    WindowRule,
    WorkingDaysMax,
    ZonePeriod,
    _models,
    and_who,
    at_least,
    check_slot,
    check_slot_json,
    default_limits,
    except_who,
    find_availability,
    find_availability_json,
    limits_with,
    offer_assigned_hosts,
    offer_calendar_unknown,
    offer_end,
    offer_reserve,
    offer_start,
    offer_to_proposal,
    person,
    presets,
    public_response,
    public_slot_check,
    required_ranges_json,
    single_goal,
    unless,
    validate,
    validate_json,
    with_updates,
)
from opportune.adapters import google, graph


def _block(span: tuple[str, str], tags: Sequence[str] = (), *, key: str | None = None) -> Block:
    return Block(span=span, tags=list(tags), event_key=key)


def _reasons_at(response: Response, start: str) -> list[tuple[str | None, str, str | None]]:
    """The first failing reason of every participant the response excluded at `start`."""
    for excluded in response.excluded:
        if excluded.start == start:
            return [(reason.participant, reason.code, reason.rule) for reason in excluded.reasons]
    raise AssertionError(f"the response excludes no start at {start}")


def test_u1_a_personal_thirty_minute_link_takes_about_ten_lines() -> None:
    request = Request(
        now="2027-06-14T07:00Z",
        goal=single_goal(
            "30m",
            Requirement(all=[person("rich")]),
            zone="Europe/London",
            search_window=("2027-06-13T23:00Z", "2027-06-18T23:00Z"),
        ),
        participants=[Participant(id="rich", zone="Europe/London", calendar=[])],
        rules=[presets.business_hours(), presets.buffers(), presets.min_notice()],
    )

    response = find_availability(request)

    assert offer_start(response.offers[0]) == "2027-06-14T11:00Z", "four hours of notice"
    assert offer_end(response.offers[0]) == "2027-06-14T11:30Z"
    assert response.offers[0].flags == [], "a supplied calendar is not unknown"
    assert response.dates[0].offers == 10


def test_u17_the_booth_window_narrows_the_team_day_in_the_conference_zone() -> None:
    request = Request(
        now="2027-06-14T08:00Z",
        goal=Goal(
            zone="America/Anchorage",
            search_window=("2027-06-14T16:00Z", "2027-06-15T02:00Z"),
            parts=[
                Part(
                    duration="30m",
                    tags=["booth"],
                    requirement=Requirement(all=[person("team")]),
                )
            ],
        ),
        participants=[
            Participant(
                id="team",
                zone="UTC",
                calendar=[],
                zone_periods=[
                    ZonePeriod(
                        **{
                            "from": "2027-06-13T08:00Z",
                            "until": "2027-06-15T08:00Z",
                            "zone": "America/Anchorage",
                        }
                    )
                ],
            )
        ],
        rules=[
            presets.business_hours(
                "mon-sun",
                "10:00-16:00",
                zone="event",
                rule_id="booth",
                when=TagSelector(tag="booth"),
            ),
            presets.business_hours("mon-sun", "08:00-18:00", rule_id="hours"),
        ],
        explain=Explain(limit=100),
    )

    response = find_availability(request)

    assert [offer_start(offer) for offer in response.offers][:2] == [
        "2027-06-14T18:00Z",
        "2027-06-14T18:30Z",
    ], "10:00 in Anchorage, the booth's opening, not the team's 08:00"
    assert len(response.offers) == 12, "six booth hours of half-hour slots"
    assert ("team", "outside_window", "booth") in _reasons_at(response, "2027-06-14T16:00Z"), (
        "the team's own day starts before the booth opens"
    )


def test_u7_u11_the_panel_respects_each_interviewers_daily_cap() -> None:
    def booked(host: str, day: str) -> list[Block]:
        return [
            _block(
                (f"2027-06-{day}T00:15Z", f"2027-06-{day}T00:45Z"),
                ["interview"],
                key=f"{host}-{day}-a",
            ),
            _block(
                (f"2027-06-{day}T00:45Z", f"2027-06-{day}T01:15Z"),
                ["interview"],
                key=f"{host}-{day}-b",
            ),
        ]

    pool = ["ana", "bo", "cy"]
    request = Request(
        now="2027-06-13T18:15Z",
        goal=Goal(
            zone="Asia/Kathmandu",
            search_window=("2027-06-14T03:15Z", "2027-06-14T06:15Z"),
            candidates=[
                "2027-06-14T03:15Z",
                "2027-06-14T03:45Z",
                "2027-06-14T04:15Z",
                "2027-06-14T04:45Z",
                "2027-06-14T05:15Z",
                "2027-06-14T05:45Z",
            ],
            parts=[
                Part(
                    duration="30m",
                    tags=["interview"],
                    requirement=Requirement(
                        all=[at_least(1, TagSelector(tag="pool/interviewers"))]
                    ),
                )
            ],
        ),
        participants=[
            *(
                Participant(
                    id=host,
                    zone="Asia/Kathmandu",
                    tags=["pool/interviewers"],
                    calendar=booked(host, "14"),
                )
                for host in pool
            ),
            Participant(
                id="dee",
                zone="Asia/Kathmandu",
                tags=["pool/interviewers"],
                calendar=[_block(("2027-06-14T04:15Z", "2027-06-14T06:15Z"), key="dee-block")],
            ),
        ],
        rules=[
            presets.business_hours("mon-sun", "09:00-18:00", rule_id="hours"),
            presets.cap_per_day(TagSelector(tag="interview"), 2, rule_id="each_day"),
            presets.cap_per_week(
                TagSelector(tag="interview"), 8, per="pooled", zone="event", rule_id="pool_week"
            ),
        ],
        explain=Explain(limit=100),
    )

    response = find_availability(request)

    assert [offer_assigned_hosts(offer) for offer in response.offers] == [["dee"], ["dee"]], (
        "ana, bo and cy are at two interviews each; only dee can take the panel"
    )
    assert {
        code for _, code, rule in _reasons_at(response, "2027-06-14T04:15Z") if rule == "each_day"
    } == {"cap_reached"}

    over_quota = with_updates(
        request,
        participants=[
            *(
                Participant(
                    id=host,
                    zone="Asia/Kathmandu",
                    tags=["pool/interviewers"],
                    calendar=[*booked(host, "14"), *booked(host, "15")],
                )
                for host in pool
            ),
            Participant(
                id="dee",
                zone="Asia/Kathmandu",
                tags=["pool/interviewers"],
                calendar=[_block(("2027-06-14T04:15Z", "2027-06-14T06:15Z"), key="dee-block")],
            ),
        ],
    )

    exhausted = find_availability(over_quota)

    assert exhausted.offers == [], "the pool has used its eight interviews for the week"
    assert exhausted.warnings == [], "every counted block carries its own key"
    assert ("dee", "cap_reached", "pool_week") in _reasons_at(exhausted, "2027-06-14T03:15Z")


def test_u4_u22_weighted_round_robin_offers_confirm_with_check_slot() -> None:
    request = Request(
        now="2027-06-13T13:30Z",
        goal=Goal(
            zone="Australia/Lord_Howe",
            search_window=("2027-06-13T22:30Z", "2027-06-14T01:30Z"),
            parts=[
                Part(
                    duration="30m",
                    requirement=Requirement(all=[at_least(1, TagSelector(tag="team/sales"))]),
                )
            ],
        ),
        participants=[
            Participant(
                id="ana",
                zone="Australia/Lord_Howe",
                tags=["team/sales"],
                calendar=[_block(("2027-06-13T23:30Z", "2027-06-14T00:30Z"))],
            ),
            Participant(
                id="bo",
                zone="Australia/Lord_Howe",
                tags=["team/sales"],
                calendar=[_block(("2027-06-14T00:00Z", "2027-06-14T00:30Z"))],
            ),
            Participant(
                id="cy",
                zone="Australia/Lord_Howe",
                tags=["team/sales"],
                calendar=[
                    _block(("2027-06-13T22:30Z", "2027-06-13T23:30Z")),
                    _block(("2027-06-14T00:00Z", "2027-06-14T00:30Z")),
                ],
            ),
        ],
        rules=[presets.business_hours("mon-sun", "09:00-18:00", rule_id="hours")],
        weights={"bo": 2, "cy": 5},
    )

    response = find_availability(request)

    assigned = [offer_assigned_hosts(offer) for offer in response.offers]
    assert assigned == [["bo"], ["bo"], ["cy"], ["cy"], ["cy"]], "the weights decide the rotation"
    assert not offer_calendar_unknown(response.offers[0])

    proposal = offer_to_proposal(response.offers[0])
    checked = check_slot(request, proposal)
    assert checked.valid, "a returned offer always confirms"
    assert checked.reasons == []

    moved = with_updates(
        proposal,
        events=[with_updates(proposal.events[0], start="2027-06-14T00:00Z")],
    )
    refused = check_slot(request, moved)
    assert not refused.valid, "an offered slot moved onto a busy block no longer confirms"
    assert "blocked" in {reason.code for reason in refused.reasons}


def test_u14_short_horizon_and_notice_bound_the_dates_offered() -> None:
    request = Request(
        now="2027-06-17T02:15Z",
        goal=single_goal(
            "30m",
            Requirement(all=[person("rich")]),
            zone="Asia/Kathmandu",
            search_window=("2027-06-16T18:15Z", "2027-06-23T18:15Z"),
        ),
        participants=[Participant(id="rich", zone="Asia/Kathmandu", calendar=[])],
        rules=[
            presets.min_notice("1h"),
            presets.horizon(WorkingDaysMax(working_days=3, calendar="org")),
        ],
        calendars={"org": CalendarSpec(zone="Asia/Kathmandu", working_days="mon,tue,wed,thu,fri")},
    )

    response = find_availability(request)

    assert offer_start(response.offers[0]) == "2027-06-17T03:15Z", "one hour of notice"
    assert [date.date for date in response.dates if date.offers][-1] == "2027-06-22", (
        "Friday, Monday and Tuesday are the three working days the horizon allows"
    )
    assert response.dates[-1].offers == 0, "the date after the horizon is listed but unbookable"


def test_u15_rest_protects_the_morning_after_a_late_finish() -> None:
    request = Request(
        now="2027-06-13T13:30Z",
        goal=single_goal(
            "30m",
            Requirement(all=[person("ana")]),
            zone="Australia/Lord_Howe",
            search_window=("2027-06-14T19:30Z", "2027-06-15T01:30Z"),
        ),
        participants=[
            Participant(
                id="ana",
                zone="Australia/Lord_Howe",
                calendar=[_block(("2027-06-14T08:00Z", "2027-06-14T09:30Z"))],
            )
        ],
        rules=[
            presets.business_hours("mon-sun", "06:00-18:00", rule_id="hours"),
            presets.rest(),
        ],
        explain=Explain(limit=100),
    )

    response = find_availability(request)

    assert [offer_start(offer) for offer in response.offers] == [
        "2027-06-14T23:30Z",
        "2027-06-15T00:00Z",
        "2027-06-15T00:30Z",
        "2027-06-15T01:00Z",
    ], "10:00 in Lord Howe, after the evening that ran past 18:00"
    assert ("ana", "rest", "rest") in _reasons_at(response, "2027-06-14T19:30Z")


def test_u20_the_available_only_preset_limits_the_attendee_to_the_stated_spans() -> None:
    candidate, availability = presets.available_only(
        "cand",
        [
            ("2027-06-14T14:00Z", "2027-06-14T15:00Z"),
            ("2027-06-14T15:30Z", "2027-06-14T16:30Z"),
        ],
    )
    request = Request(
        now="2027-06-14T00:00Z",
        goal=single_goal(
            "30m",
            Requirement(all=[person("ana")]),
            zone="UTC",
            search_window=("2027-06-14T13:00Z", "2027-06-14T17:00Z"),
        ),
        participants=[Participant(id="ana", zone="UTC", calendar=[]), candidate],
        rules=[availability],
        explain=Explain(limit=100),
    )

    response = find_availability(request)

    assert [offer_start(offer) for offer in response.offers] == [
        "2027-06-14T14:00Z",
        "2027-06-14T14:30Z",
        "2027-06-14T15:30Z",
        "2027-06-14T16:00Z",
    ]
    assert ("cand", "outside_window", "available_only:cand") in _reasons_at(
        response, "2027-06-14T15:00Z"
    )


def test_available_only_coalesces_spans_and_splits_them_at_utc_midnight() -> None:
    candidate, availability = presets.available_only(
        "cand",
        [
            ("2027-06-14T23:30Z", "2027-06-15T00:30Z"),
            ("2027-06-15T00:30Z", "2027-06-15T01:00Z"),
        ],
    )

    assert isinstance(availability, WindowRule)
    assert availability.who == IdsSelector(ids=["cand"]), "the window binds only the attendee"
    assert availability.params.dates == {
        "2027-06-14": ["23:30-24:00"],
        "2027-06-15": ["00:00-01:00"],
    }, "one coalesced span, split at UTC midnight, the first half ending 24:00"

    request = Request(
        now="2027-06-14T00:00Z",
        goal=single_goal(
            "30m",
            Requirement(all=[person("ana")]),
            zone="UTC",
            search_window=("2027-06-14T23:00Z", "2027-06-15T01:30Z"),
        ),
        participants=[Participant(id="ana", zone="UTC", calendar=[]), candidate],
        rules=[availability],
    )

    assert [offer_start(offer) for offer in find_availability(request).offers] == [
        "2027-06-14T23:30Z",
        "2027-06-15T00:00Z",
        "2027-06-15T00:30Z",
    ]


def test_the_buffers_preset_holds_the_gap_after_an_existing_meeting() -> None:
    request = Request(
        now="2027-06-14T09:00Z",
        goal=Goal(
            zone="UTC",
            search_window=("2027-06-14T10:00Z", "2027-06-14T11:30Z"),
            grid=Grid(step="10m"),
            parts=[Part(duration="20m", requirement=Requirement(all=[person("ana")]))],
        ),
        participants=[
            Participant(
                id="ana",
                zone="UTC",
                calendar=[_block(("2027-06-14T10:00Z", "2027-06-14T10:40Z"))],
            )
        ],
        rules=[presets.buffers()],
        explain=Explain(limit=100),
    )

    response = find_availability(request)

    assert offer_start(response.offers[0]) == "2027-06-14T10:50Z", "ten minutes clear first"
    assert ("ana", "clearance", "buffers") in _reasons_at(response, "2027-06-14T10:40Z")


def test_buffers_min_gap_keeps_two_events_apart() -> None:
    """The least gap between two events holds even with no other buffer."""
    rule = presets.buffers("0m", "0m", min_gap="15m")
    assert rule.params.min_gap == "15m"

    request = Request(
        now="2027-06-14T08:00Z",
        goal=Goal(
            zone="UTC",
            search_window=("2027-06-14T10:30Z", "2027-06-14T12:00Z"),
            candidates=["2027-06-14T11:00Z", "2027-06-14T11:15Z"],
            parts=[Part(duration="30m", requirement=Requirement(all=[person("ana")]))],
        ),
        participants=[
            Participant(
                id="ana",
                zone="UTC",
                calendar=[_block(("2027-06-14T10:05Z", "2027-06-14T11:00Z"))],
            )
        ],
        rules=[rule],
        explain=Explain(limit=100),
    )

    assert validate(request).errors == []
    response = find_availability(request)

    assert [offer_start(offer) for offer in response.offers] == ["2027-06-14T11:15Z"], (
        "five minutes after the meeting is too close; exactly fifteen is enough"
    )
    assert ("ana", "clearance", "buffers") in _reasons_at(response, "2027-06-14T11:00Z")


def test_dated_hours_opens_a_booth_only_on_the_conference_dates() -> None:
    """A dated window is the whole window: a date it does not open is closed."""
    request = Request(
        now="2027-06-14T06:00Z",
        goal=Goal(
            zone="America/New_York",
            search_window=("2027-06-14T04:00Z", "2027-06-17T04:00Z"),
            parts=[Part(duration="30m", requirement=Requirement(all=[person("team")]))],
        ),
        participants=[Participant(id="team", zone="America/New_York", calendar=[])],
        rules=[
            presets.business_hours("mon-fri", "08:00-18:00", rule_id="hours"),
            presets.dated_hours(
                {
                    "2027-06-14": "10:00-16:00",
                    "2027-06-15": ["10:00-12:00", "13:00-16:00"],
                    "2027-06-16": [],
                },
                zone="event",
            ),
        ],
        explain=Explain(limit=100),
    )

    response = find_availability(request)

    assert [f"{entry.date}:{entry.offers}" for entry in response.dates] == [
        "2027-06-14:12",
        "2027-06-15:10",
        "2027-06-16:0",
    ], "the booth's two days, and the conference date its own hours leave closed"
    assert response.offers[0].events[0].start == "2027-06-14T14:00Z", (
        "10:00 in New York, the booth's opening"
    )
    assert ("team", "outside_window", "dated_hours") in _reasons_at(
        response, "2027-06-15T16:00Z"
    ), "12:00-13:00 is between the Tuesday sessions"
    assert ("team", "outside_window", "dated_hours") in _reasons_at(
        response, "2027-06-16T14:00Z"
    ), "a conference date with no hours is closed"


def test_who_and_when_scope_a_preset_rule() -> None:
    """`who` narrows the participants a rule constrains and `when` the parts it applies to."""

    def hours_with(who: IdsSelector | None) -> Request:
        return Request(
            now="2027-06-14T00:00Z",
            goal=Goal(
                zone="UTC",
                search_window=("2027-06-14T00:00Z", "2027-06-15T00:00Z"),
                parts=[Part(duration="30m", requirement=Requirement(all=[person("ana")]))],
            ),
            participants=[
                Participant(id="ana", zone="UTC", calendar=[]),
                Participant(id="bo", zone="UTC", calendar=[]),
            ],
            rules=[presets.business_hours("mon-sun", "09:00-17:00", who=who)],
        )

    everyone = find_availability(hours_with(None))
    assert len(everyone.offers) == 16, "nine to five in half-hour starts"

    only_bo = find_availability(hours_with(IdsSelector(ids=["bo"])))
    assert len(only_bo.offers) == 48, (
        "ana is the only required host, and the rule constrains bo alone"
    )
    assert offer_start(only_bo.offers[0]) == "2027-06-14T00:00Z"

    def demo_capped(when: Selector) -> Request:
        return Request(
            now="2027-06-14T08:00Z",
            goal=Goal(
                zone="UTC",
                search_window=("2027-06-14T08:00Z", "2027-06-15T18:00Z"),
                candidates=["2027-06-14T14:00Z", "2027-06-15T14:00Z"],
                parts=[
                    Part(
                        duration="30m",
                        tags=["sales/demo"],
                        requirement=Requirement(all=[person("ana")]),
                    )
                ],
            ),
            participants=[
                Participant(
                    id="ana",
                    zone="UTC",
                    calendar=[
                        _block(
                            ("2027-06-14T09:00Z", "2027-06-14T09:30Z"),
                            ["sales/demo"],
                            key="demo/1",
                        )
                    ],
                )
            ],
            rules=[presets.cap_per_day(TagSelector(tag="sales/demo"), 1, when=when)],
            explain=Explain(limit=100),
        )

    matched = find_availability(demo_capped(TagSelector(tag="sales/demo")))
    assert [offer_start(offer) for offer in matched.offers] == ["2027-06-15T14:00Z"]

    unmatched = find_availability(demo_capped(TagSelector(tag="other")))
    assert [offer_start(offer) for offer in unmatched.offers] == [
        "2027-06-14T14:00Z",
        "2027-06-15T14:00Z",
    ], "a `when` the part does not match leaves the cap unapplied"


def test_cap_presets_scoped_by_a_negated_selector_round_trip_to_the_engine() -> None:
    """A negated selector is the one a rule's dump spells with its field name.

    ``not`` is a Python keyword, so the field is ``not_`` and ``model_dump()`` writes
    that. A cap scoped with ``NotSelector`` therefore has to read back from the
    model's own dump — which is what ``with_updates``, a stored row and a re-read all
    do — or every preset call that passes ``who=`` raises instead of returning its
    rule. The search answers the model and the dump identically, which is the wire
    promise the dump exists for.
    """
    negated = NotSelector(not_=IdsSelector(ids=["bo"]))
    rules: list[Rule] = [
        presets.cap_per_day(True, 2, rule_id="capped_day", who=negated),
        presets.cap_per_week(True, 5, rule_id="capped_week", who=negated),
        presets.cap_per_month(True, 20, rule_id="capped_month", who=negated),
    ]
    request = Request(
        now="2027-06-14T00:00Z",
        goal=Goal(
            zone="UTC",
            search_window=("2027-06-14T00:00Z", "2027-06-15T00:00Z"),
            parts=[Part(duration="30m", requirement=Requirement(all=[person("ana")]))],
        ),
        participants=[
            Participant(id="ana", zone="UTC", calendar=[]),
            Participant(id="bo", zone="UTC", calendar=[]),
        ],
        rules=rules,
    )
    assert [rule.who for rule in rules] == [negated, negated, negated], "the scoping survives"

    dumped = request.model_dump()
    assert Request.model_validate(dumped) == request

    engine = Engine()
    assert engine.find_availability(dumped) == engine.find_availability(request)


def test_horizon_accepts_a_duration_a_date_and_a_calendar_day_count() -> None:
    """The latest start is typed, or the shorthand a caller writes most often."""
    assert presets.horizon("4h").params.max == DurationMax(duration="4h")
    assert presets.horizon("1h30m").params.max == DurationMax(duration="1h30m")
    assert presets.horizon("2027-07-01").params.max == DateMax(date="2027-07-01")
    assert presets.horizon(7).params.max == DaysMax(days=7)
    assert presets.horizon(DaysMax(days=3)).params.max == DaysMax(days=3), (
        "a typed form passes through unchanged"
    )

    for shorthand in ("14d", "soon", "2027-02-30"):
        with pytest.raises(OpportuneError) as raised:
            presets.horizon(shorthand)
        assert raised.value.code == "invalid_argument", shorthand
        assert shorthand in raised.value.message, raised.value.message

    for shorthand in ("14d", "soon"):
        with pytest.raises(OpportuneError) as raised:
            presets.horizon(shorthand)
        assert "is not a duration such as" in raised.value.message, (
            "the message names what the value is not, rather than claiming it is a "
            "duration the engine reads"
        )

    with pytest.raises(OpportuneError) as raised:
        presets.horizon(-1)
    assert raised.value.code == "invalid_argument", raised.value.message
    assert "-1" in raised.value.message, (
        "a day count that cannot be one is refused naming the value the caller wrote, "
        "not a `days` argument this signature does not have"
    )

    with pytest.raises(OpportuneError) as raised:
        presets.horizon(1.5)  # pyright: ignore[reportArgumentType]
    assert raised.value.code == "invalid_argument", raised.value.message

    request = Request(
        now="2027-06-14T08:00Z",
        goal=single_goal(
            "30m",
            Requirement(all=[person("ana")]),
            zone="UTC",
            search_window=("2027-06-14T08:00Z", "2027-06-24T08:00Z"),
        ),
        participants=[Participant(id="ana", zone="UTC", calendar=[])],
        rules=[presets.horizon(2)],
    )

    bounded = find_availability(request)
    assert [entry.date for entry in bounded.dates if entry.offers][-1] == "2027-06-16", (
        "two calendar days after today is the last date a start may fall on"
    )


def test_horizon_refuses_a_missing_maximum_instead_of_dropping_the_bound() -> None:
    """A caller whose horizon is unset gets the rejection, not an unbounded rule."""
    with pytest.raises(OpportuneError) as raised:
        presets.horizon(None)  # pyright: ignore[reportArgumentType]
    assert raised.value.code == "invalid_argument", raised.value.message
    assert "None" in raised.value.message, raised.value.message


def test_who_and_when_scope_the_lead_time_and_clearance_presets() -> None:
    """The scope options reach every rule kind, not only windows and caps."""

    def notice(when: Selector) -> list[str]:
        request = Request(
            now="2027-06-14T08:00Z",
            goal=Goal(
                zone="UTC",
                search_window=("2027-06-14T08:00Z", "2027-06-14T18:00Z"),
                candidates=["2027-06-14T09:00Z"],
                parts=[
                    Part(
                        duration="30m",
                        tags=["demo"],
                        requirement=Requirement(all=[person("ana")]),
                    )
                ],
            ),
            participants=[Participant(id="ana", zone="UTC", calendar=[])],
            rules=[presets.min_notice("4h", when=when)],
        )
        return [offer_start(offer) for offer in find_availability(request).offers]

    assert notice(TagSelector(tag="demo")) == [], "the notice binds the matching part"
    assert notice(TagSelector(tag="other")) == ["2027-06-14T09:00Z"], (
        "a `when` the part does not match leaves the notice unapplied"
    )

    def minimum_gap(who: Selector) -> list[str]:
        request = Request(
            now="2027-06-14T08:00Z",
            goal=Goal(
                zone="UTC",
                search_window=("2027-06-14T08:00Z", "2027-06-14T18:00Z"),
                candidates=["2027-06-14T10:00Z", "2027-06-14T11:30Z"],
                parts=[Part(duration="30m", requirement=Requirement(all=[person("ana")]))],
            ),
            participants=[
                Participant(
                    id="ana",
                    zone="UTC",
                    calendar=[_block(("2027-06-14T11:00Z", "2027-06-14T11:30Z"), key="m1")],
                ),
                Participant(id="bo", zone="UTC", calendar=[]),
            ],
            rules=[presets.buffers("0m", "0m", min_gap="45m", who=who)],
        )
        return [offer_start(offer) for offer in find_availability(request).offers]

    assert minimum_gap(IdsSelector(ids=["ana"])) == [], (
        "thirty minutes before the meeting and none after are both under the gap"
    )
    assert minimum_gap(IdsSelector(ids=["bo"])) == [
        "2027-06-14T10:00Z",
        "2027-06-14T11:30Z",
    ], "ana's meeting does not constrain a rule scoped to bo"


def test_a_preset_rejects_an_argument_the_models_would_refuse() -> None:
    """A preset's invalid value is the package's error, never a ValidationError."""
    for call, named in [
        (lambda: presets.cap_per_day(TagSelector(tag="interview"), -1), "limit"),
        (lambda: presets.cap_per_month(TagSelector(tag="demo"), -1), "limit"),
        (lambda: presets.business_hours(who={"idz": ["ana"]}), "who"),
    ]:
        with pytest.raises(OpportuneError) as raised:
            call()
        assert raised.value.code == "invalid_argument", raised.value.message
        assert named in raised.value.message, raised.value.message
        assert raised.value.__cause__ is not None, "the model's finding stays attached"


def test_a_selector_mapping_is_the_rule_a_typed_selector_builds() -> None:
    """A preset takes the mapping a document states, and the model reads it into the form.

    ``SelectorInput`` is what makes a recipe, a JSON column and an HTTP body the same
    argument, so a mapping has to be *validated into* the generated form rather than
    carried through: a rule holding a bare dict is one the engine would refuse to
    dump, and the docs promise the two spellings build the same rule. ``who``'s
    ``with_updates`` path and ``count``'s ``CapParams`` field are the two places a
    preset hands a mapping to a validated field, so both are exercised here.
    """
    mapped = presets.business_hours(who={"ids": ["ana"]}, when={"tag": "interview"})
    typed = presets.business_hours(who=IdsSelector(ids=["ana"]), when=TagSelector(tag="interview"))

    assert mapped == typed
    assert isinstance(mapped.who, IdsSelector), mapped.who
    assert isinstance(mapped.when, TagSelector), mapped.when

    capped = presets.cap_per_day({"tag": "interview"}, 2)
    assert capped == presets.cap_per_day(TagSelector(tag="interview"), 2)
    assert isinstance(capped.params.count, TagSelector), capped.params.count


def test_a_sugar_selector_the_models_refuse_is_the_packages_error() -> None:
    """A sugar helper takes the mapping a document states, so it rejects one as a preset does.

    ``at_least`` accepted only a built selector before it took ``SelectorInput``, so a
    mapping it could not read was a ``pydantic.ValidationError`` raised out of the
    model — an exception an application's handler for this package's own error never
    sees, even though a raw mapping is a supported typed input path.
    """
    with pytest.raises(OpportuneError) as raised:
        at_least(1, {"idz": ["ana"]})

    assert raised.value.code == "invalid_argument", raised.value.message
    assert "`of`" in raised.value.message, raised.value.message
    assert raised.value.__cause__ is not None, "the model's finding stays attached"

    assert at_least(1, {"ids": ["ana"]}) == at_least(1, IdsSelector(ids=["ana"])), (
        "a mapping the models read is the selector it names"
    )
    assert at_least(1, True) == AtLeastTerm(at_least=1, of=True), (
        "`True` is the selector that names everyone, and it survives the wrapper"
    )


def test_a_preset_selector_typo_names_the_forms_a_selector_has() -> None:
    """The wrapper reports the one finding the model raises, so the sentence is actionable.

    A union reports a misspelled key once per arm, and the wrapper would then hand the
    caller the first arm's wording ("Input should be True"); the ``Selector`` dispatch
    makes the model itself raise the key and the forms it could have been.
    """
    with pytest.raises(OpportuneError) as raised:
        presets.business_hours(who={"idz": ["ana"]})

    message = raised.value.message
    assert "`idz`" in message, message
    assert "tag" in message and "role" in message, "the message names the forms a caller may write"
    assert "Input should be" not in message, message


def test_an_offer_with_no_events_is_the_packages_error_not_an_index_error() -> None:
    """No engine answer is eventless, and a handler catches one exception for both."""
    empty = Offer(events=[], flags=[])

    for accessor in (offer_start, offer_end):
        with pytest.raises(OpportuneError) as raised:
            accessor(empty)
        assert raised.value.code == "invalid_argument"
        assert "no events" in raised.value.message

    assert offer_reserve(empty) == []
    assert offer_assigned_hosts(empty) == []


def test_offer_reserve_returns_the_travel_every_event_holds() -> None:
    """A multi-part offer holds travel per part, and the accessor flattens all of it.

    Reading only the first event's reserve would pass every single-part fixture
    while dropping the holds a caller writes for the later parts of one visit.
    """
    to_the_visit = ReserveSpan(
        kind="travel",
        participant="bob",
        span=("2027-03-08T20:30Z", "2027-03-08T20:55Z"),
    )
    to_the_follow_up = ReserveSpan(
        kind="travel",
        participant="ana",
        span=("2027-03-08T21:00Z", "2027-03-08T21:20Z"),
    )
    offer = Offer(
        events=[
            OfferEvent(
                assignment=[["bob"]],
                eligible=[["bob"]],
                end="2027-03-08T21:00Z",
                part="visit",
                reserve=[to_the_visit],
                start="2027-03-08T20:55Z",
            ),
            OfferEvent(
                assignment=[["ana"]],
                eligible=[["ana"]],
                end="2027-03-08T21:50Z",
                part="follow_up",
                reserve=[to_the_follow_up],
                start="2027-03-08T21:20Z",
            ),
        ],
        flags=[],
    )

    assert offer_reserve(offer) == [to_the_visit, to_the_follow_up], (
        "every event's holds, in the order the offer's events are in"
    )


OFFER_DOCUMENT: dict[str, object] = {
    "events": [
        {
            "assignment": [["ana"]],
            "eligible": [["ana"]],
            "end": "2027-06-14T10:00Z",
            "part": "main",
            "reserve": [],
            "start": "2027-06-14T09:30Z",
        }
    ],
    "flags": ["calendar_unknown"],
}
"""One offer as the document a caller stores it as, rather than as the model."""


def test_every_offer_accessor_reads_the_mapping_an_offer_is_stored_as() -> None:
    """An offer held in a JSON column is the argument, not one rebuilt by hand."""
    typed = Offer.model_validate(OFFER_DOCUMENT)

    assert offer_start(OFFER_DOCUMENT) == offer_start(typed) == "2027-06-14T09:30Z"
    assert offer_end(OFFER_DOCUMENT) == offer_end(typed) == "2027-06-14T10:00Z"
    assert offer_assigned_hosts(OFFER_DOCUMENT) == offer_assigned_hosts(typed) == ["ana"]
    assert offer_calendar_unknown(OFFER_DOCUMENT) is True
    assert offer_reserve(OFFER_DOCUMENT) == offer_reserve(typed) == []
    assert offer_to_proposal(OFFER_DOCUMENT) == offer_to_proposal(typed)
    assert offer_to_proposal(OFFER_DOCUMENT).events[0].start == "2027-06-14T09:30Z"


@pytest.mark.parametrize(
    "offer",
    [{}, {"events": "x", "flags": []}, {"events": [5], "flags": []}, 5, None],
    ids=["empty", "events-not-a-list", "event-not-an-event", "not-a-document", "none"],
)
def test_an_offer_that_is_not_one_is_the_packages_rejection(offer: object) -> None:
    """A malformed offer is refused by every accessor, as this package's own error.

    The accessors read a returned model's fields, so a document that is not an offer —
    a column holding `{}`, a list where an event belongs — used to raise
    ``AttributeError`` or ``IndexError`` from this module's own code, which a handler
    written for ``OpportuneError`` never sees.
    """
    for accessor in (
        offer_start,
        offer_end,
        offer_assigned_hosts,
        offer_calendar_unknown,
        offer_reserve,
        offer_to_proposal,
    ):
        with pytest.raises(OpportuneError) as raised:
            accessor(offer)  # pyright: ignore[reportArgumentType]
        assert raised.value.code == "invalid_argument", raised.value.message
        assert "`offer`" in raised.value.message, raised.value.message
        assert raised.value.__cause__ is not None, "the model's finding stays attached"


def test_u13_a_monthly_cap_counts_the_months_demos() -> None:
    """A monthly cap rolls at the calendar month, leaving the next month free."""
    request = Request(
        now="2027-06-14T08:00Z",
        goal=Goal(
            zone="UTC",
            search_window=("2027-06-14T08:00Z", "2027-07-02T18:00Z"),
            candidates=["2027-06-16T14:00Z", "2027-07-01T14:00Z"],
            parts=[
                Part(
                    duration="30m",
                    tags=["sales/demo"],
                    requirement=Requirement(all=[person("ana")]),
                )
            ],
        ),
        participants=[
            Participant(
                id="ana",
                zone="UTC",
                calendar=[
                    Block(
                        span=("2027-06-14T09:00Z", "2027-06-14T10:00Z"),
                        tags=["sales/demo"],
                        event_key="demo/1",
                    ),
                    Block(
                        span=("2027-06-15T09:00Z", "2027-06-15T10:00Z"),
                        tags=["sales/demo"],
                        event_key="demo/2",
                    ),
                ],
            )
        ],
        rules=[presets.cap_per_month(TagSelector(tag="sales/demo"), 2)],
        explain=Explain(limit=100),
    )

    assert validate(request).errors == [], "the engine reads the rule the preset built"
    response = find_availability(request)

    assert [offer_start(offer) for offer in response.offers] == ["2027-07-01T14:00Z"], (
        "June's two demos exhaust the month; July's bucket is empty"
    )
    assert ("ana", "cap_reached", "cap_per_month") in _reasons_at(response, "2027-06-16T14:00Z")


def _demo_request(booked: list[tuple[str, str]]) -> Request:
    """A fortnight of 30-minute `sales/demo` bookings for one host, capped weekly."""
    return Request(
        now="2027-06-14T08:00Z",
        goal=Goal(
            zone="UTC",
            search_window=("2027-06-14T08:00Z", "2027-06-17T18:00Z"),
            parts=[
                Part(
                    duration="30m",
                    tags=["sales/demo"],
                    requirement=Requirement(all=[person("ana")]),
                )
            ],
        ),
        participants=[
            Participant(
                id="ana",
                zone="UTC",
                calendar=[
                    Block(span=span, tags=["sales/demo"], event_key=f"demo/{index}")
                    for index, span in enumerate(booked)
                ],
            )
        ],
        rules=[presets.cap_per_week(TagSelector(tag="sales/demo"), 360, measure="minutes")],
        explain=Explain(limit=100),
    )


def test_u13_a_cap_measured_in_minutes_bounds_the_hours_of_demos() -> None:
    """Six hours of demo time a week, measured in minutes rather than counted."""
    rule = presets.cap_per_week(TagSelector(tag="sales/demo"), 360, measure="minutes")
    assert (rule.params.measure, rule.params.limit, rule.params.period) == ("minutes", 360, "week")

    spent = [("2027-06-14T09:00Z", "2027-06-14T12:00Z"), ("2027-06-15T09:00Z", "2027-06-15T12:00Z")]
    request = _demo_request(spent)
    assert validate(request).errors == [], "the engine reads the rule the preset built"

    exhausted = find_availability(request)
    assert exhausted.offers == [], "the week's six hours are already booked"
    assert ("ana", "cap_reached", "cap_per_week") in _reasons_at(exhausted, "2027-06-14T08:00Z")

    room = find_availability(_demo_request([spent[0], ("2027-06-15T09:00Z", "2027-06-15T11:00Z")]))
    assert room.offers, "five hours booked leaves an hour of demo time to book"


def test_a_pooled_cap_demands_the_zone_it_is_counted_in() -> None:
    """A pooled cap has no participant to take a zone from, so the caller must name one."""
    with pytest.raises(OpportuneError) as raised:
        presets.cap_per_week(TagSelector(tag="interview"), 8, per="pooled")

    assert raised.value.code == "invalid_argument"
    assert "zone" in raised.value.message

    pooled = presets.cap_per_week(TagSelector(tag="interview"), 8, per="pooled", zone="event")
    assert (pooled.params.per, pooled.params.zone) == ("pooled", "event")

    each = presets.cap_per_day(TagSelector(tag="interview"), 2)
    assert (each.params.per, each.params.zone) == ("each", "participant"), (
        "the defaults stay the per-participant cap a single-host rule means"
    )


def test_business_hours_rejects_an_argument_it_could_never_mean() -> None:
    """A pair of clock times is two values, not one range, and the error names the value."""
    with pytest.raises(OpportuneError) as raised:
        presets.business_hours(hours=("09:00", "17:00"))

    assert raised.value.code == "invalid_argument"
    assert "09:00" in raised.value.message

    with pytest.raises(OpportuneError) as raised:
        # An untyped caller passes a list where the signature says a day key.
        presets.business_hours(days=["mon", "tue"])  # pyright: ignore[reportArgumentType]
    assert raised.value.code == "invalid_argument"
    assert "days" in raised.value.message

    for hours in (None, 5):
        with pytest.raises(OpportuneError) as raised:
            # An optional value the caller did not set is `None`, not a range list.
            presets.business_hours(hours=hours)  # pyright: ignore[reportArgumentType]
        assert raised.value.code == "invalid_argument"
        assert "`hours`" in raised.value.message, raised.value.message

    split = presets.business_hours("mon-fri", ["09:00-12:00", "13:00-17:00", "23:30-24:00"])
    assert split.params.weekly == {"mon-fri": ["09:00-12:00", "13:00-17:00", "23:30-24:00"]}


def _single_goal(**fields: object) -> Goal:
    """A one-part goal whose optional fields are the ones a test hands it."""
    return single_goal(
        "30m",
        Requirement(all=[person("ana")]),
        zone="UTC",
        search_window=("2027-06-14T08:00Z", "2027-06-14T18:00Z"),
        **fields,  # pyright: ignore[reportArgumentType]
    )


@pytest.mark.parametrize(
    ("call", "named"),
    [
        (lambda: presets.business_hours(hours={"09:00-17:00": "x"}), "`hours`"),  # pyright: ignore[reportArgumentType]
        (lambda: presets.weekly_hours({"mon": {"09:00-17:00": "x"}}), "`weekly[mon]`"),  # pyright: ignore[reportArgumentType]
        (
            lambda: presets.dated_hours({"2027-06-14": {"09:00-17:00": "x"}}),  # pyright: ignore[reportArgumentType]
            "`dates[2027-06-14]`",
        ),
        (lambda: _single_goal(tags={"interview": True}), "`tags`"),
        (lambda: _single_goal(candidates={"2027-06-14T09:00Z": True}), "`candidates`"),
    ],
    ids=["hours", "weekly", "dated", "tags", "candidates"],
)
def test_a_mapping_where_a_sequence_of_values_belongs_is_the_packages_rejection(
    call: Callable[[], object],
    named: str,
) -> None:
    """A dict where a list belongs is refused, never read as its keys.

    Iterating a mapping yields its keys, and a key that happens to be a wall range or
    a tag reads as a perfectly valid value: ``{"09:00-17:00": "x"}`` would have been
    one open day and ``{"interview": True}`` one tag, so the value the caller wrote
    was silently replaced by one they never named.
    """
    with pytest.raises(OpportuneError) as raised:
        call()

    assert raised.value.code == "invalid_argument", raised.value.message
    assert named in raised.value.message, raised.value.message


def test_person_refuses_an_id_that_is_not_text() -> None:
    """A participant is named by text, and the model's refusal is this package's error."""
    for id in (5, None, ["ana"]):
        with pytest.raises(OpportuneError) as raised:
            person(id)  # pyright: ignore[reportArgumentType]
        assert raised.value.code == "invalid_argument", raised.value.message
        assert "`id`" in raised.value.message, raised.value.message
        assert type(id).__name__ in raised.value.message, raised.value.message

    assert person("ana").person == "ana"


def test_available_only_refuses_an_id_or_rule_id_that_is_not_text() -> None:
    """Stated availability names an attendee and a rule, both by text.

    The preset builds the attendee and the rule it returns, so a value outside the
    schema is the package's rejection rather than the model's ``ValidationError``
    escaping a caller's handler for it.
    """
    spans = [("2027-06-14T09:00Z", "2027-06-14T10:00Z")]

    for call in (
        lambda: presets.available_only(5, spans),  # pyright: ignore[reportArgumentType]
        lambda: presets.available_only("ana", spans, rule_id=5),  # pyright: ignore[reportArgumentType]
    ):
        with pytest.raises(OpportuneError) as raised:
            call()
        assert raised.value.code == "invalid_argument", raised.value.message
        assert "id" in raised.value.message, raised.value.message

    attendee, rule = presets.available_only("ana", spans)
    assert (attendee.id, rule.id) == ("ana", "available_only:ana")


def test_available_only_refuses_a_stated_instant_outside_the_engine_range() -> None:
    """A stated span endpoint outside the instants the engine holds is the
    package's ``invalid_argument``, not a dated window the engine never matches.

    The preset writes a stated availability as dated windows over UTC days, so a
    span in 1900 once produced a rule that validated clean, warned
    ``no_candidate_in_window`` and matched no offer — a caller error dressed up as
    an empty calendar. The endpoints must be instants the engine holds, so both
    ends are bounded inclusively: exactly ``1975-01-01T00:00Z`` or
    ``2100-01-01T00:00Z`` is accepted, one minute past either is refused, and the
    refusal comes before any date key is written.
    """
    rejections = (
        (("1900-06-01T00:00Z", "1900-06-01T02:00Z"), "1900-06-01T00:00Z"),
        (("1974-12-31T23:59Z", "2027-06-14T10:00Z"), "1974-12-31T23:59Z"),
        (("2027-06-14T09:00Z", "2100-01-01T00:01Z"), "2100-01-01T00:01Z"),
    )
    for span, instant in rejections:
        with pytest.raises(OpportuneError) as raised:
            presets.available_only("cand", [span])
        assert raised.value.code == "invalid_argument", raised.value.message
        assert instant in raised.value.message, raised.value.message
        assert "1975-01-01T00:00Z..=2100-01-01T00:00Z" in raised.value.message, raised.value.message

    for boundary, span in (
        ("1975-01-01T00:00Z", ("1975-01-01T00:00Z", "1975-01-01T02:00Z")),
        ("2100-01-01T00:00Z", ("2099-12-31T22:00Z", "2100-01-01T00:00Z")),
    ):
        _, rule = presets.available_only("cand", [span])
        assert isinstance(rule, WindowRule)
        assert rule.params.dates, f"the inclusive endpoint {boundary} is accepted"


def test_an_available_only_span_inside_the_engine_range_validates_and_offers() -> None:
    """A span inside the supported range still validates clean and matches.

    The stated availability sits inside the search window, so the engine's own
    reading of the window — not a boundary tolerance — decides what is bookable.
    The spans that touch either endpoint have their own test below.
    """
    candidate, availability = presets.available_only(
        "cand", [("2027-06-14T14:00Z", "2027-06-14T16:00Z")]
    )
    request = Request(
        now="2027-06-14T00:00Z",
        goal=single_goal(
            "30m",
            Requirement(all=[person("ana")]),
            zone="UTC",
            search_window=("2027-06-14T13:00Z", "2027-06-14T17:00Z"),
        ),
        participants=[Participant(id="ana", zone="UTC", calendar=[]), candidate],
        rules=[availability],
        explain=Explain(limit=100),
    )

    diagnostics = opportune.validate(request)
    assert diagnostics.errors == []
    assert diagnostics.warnings == [], (
        "the stated availability is inside the search window, so it is never the "
        "no_candidate_in_window warning an inert rule reads as"
    )
    response = find_availability(request)
    assert [offer_start(offer) for offer in response.offers] == [
        "2027-06-14T14:00Z",
        "2027-06-14T14:30Z",
        "2027-06-14T15:00Z",
        "2027-06-14T15:30Z",
    ]


def test_an_available_only_endpoint_is_the_instant_it_names_not_the_date_it_writes() -> None:
    """A span endpoint is the instant it spells, in whatever offset it is written.

    The engine holds instants, so the range must read the instant an endpoint names
    rather than the civil date on its face: ``1974-12-31T19:00-05:00`` is
    ``1975-01-01T00:00Z`` and is accepted, while ``1975-01-01T00:59+01:00`` is a
    minute before the range and is refused although its date reads 1975. Judging the
    date instead would accept a span naming an instant no `Instant` covers — writing
    the inert rule this guard exists to refuse — and refuse one that is in range.
    """
    candidate, availability = presets.available_only(
        "cand", [("1974-12-31T19:00-05:00", "1975-01-01T02:00Z")]
    )
    assert availability.params.dates == {"1975-01-01": ["00:00-02:00"]}, (
        "the offset spelling of the earliest instant is in range"
    )

    request = Request(
        now="1975-01-01T00:00Z",
        goal=single_goal(
            "30m",
            Requirement(all=[person("ana")]),
            zone="UTC",
            search_window=("1975-01-01T00:00Z", "1975-01-01T04:00Z"),
        ),
        participants=[Participant(id="ana", zone="UTC", calendar=[]), candidate],
        rules=[availability],
        explain=Explain(limit=100),
    )
    assert validate(request).errors == [], "the engine reads the window at its own earliest instant"

    for span, instant in (
        (("1975-01-01T00:59+01:00", "1975-01-02T00:00Z"), "1975-01-01T00:59+01:00"),
        (("2099-12-31T00:00Z", "2099-12-31T20:00-05:00"), "2099-12-31T20:00-05:00"),
    ):
        with pytest.raises(OpportuneError) as raised:
            presets.available_only("cand", [span])
        assert raised.value.code == "invalid_argument", raised.value.message
        assert instant in raised.value.message, raised.value.message
        assert "1975-01-01T00:00Z..=2100-01-01T00:00Z" in raised.value.message, raised.value.message


def test_an_available_only_span_at_either_end_of_the_engine_range_is_bookable() -> None:
    """The inclusive endpoints are instants the engine holds, not merely accepted text.

    The range is the engine's own, so a span touching ``1975-01-01T00:00Z`` or ending
    at ``2100-01-01T00:00Z`` writes a window the engine reads and offers from, with no
    ``no_candidate_in_window`` warning — the accepted boundary is not the inert rule
    one minute outside it would be.
    """
    boundary_requests = (
        (
            ("1975-01-01T00:00Z", "1975-01-01T01:00Z"),
            "1975-01-01T00:00Z",
            ("1975-01-01T00:00Z", "1975-01-01T02:00Z"),
            ["1975-01-01T00:00Z", "1975-01-01T00:30Z"],
        ),
        (
            ("2099-12-31T23:00Z", "2100-01-01T00:00Z"),
            "2099-12-31T00:00Z",
            ("2099-12-31T22:00Z", "2100-01-01T00:00Z"),
            ["2099-12-31T23:00Z", "2099-12-31T23:30Z"],
        ),
    )
    for span, now, window, expected in boundary_requests:
        candidate, availability = presets.available_only("cand", [span])
        request = Request(
            now=now,
            goal=single_goal(
                "30m",
                Requirement(all=[person("ana")]),
                zone="UTC",
                search_window=window,
            ),
            participants=[Participant(id="ana", zone="UTC", calendar=[]), candidate],
            rules=[availability],
            explain=Explain(limit=100),
        )

        diagnostics = validate(request)
        assert diagnostics.errors == [], span
        assert diagnostics.warnings == [], span
        assert [offer_start(offer) for offer in find_availability(request).offers] == expected, span


def test_an_available_only_endpoint_accepts_the_lower_case_rfc_3339_spelling() -> None:
    """RFC 3339 lets the separator and the UTC designator be lower case.

    ``2027-06-14t09:00z`` is the instant ``2027-06-14T09:00Z`` names, so it writes the
    window the TypeScript ``availableOnly`` writes for the same spelling rather than being
    refused as text that is not an instant. Folding the spelling changes nothing else: the
    range still reads the instant, a lower-case endpoint outside it is still refused with
    the range named, and the message echoes the spelling the caller wrote.
    """
    _, upper = presets.available_only("cand", [("2027-06-14T09:00Z", "2027-06-14T11:30Z")])
    _, lower = presets.available_only("cand", [("2027-06-14t09:00z", "2027-06-14t11:30z")])
    _, offset = presets.available_only("cand", [("2027-06-14t09:00+00:00", "2027-06-14T11:30Z")])
    assert upper.params.dates == {"2027-06-14": ["09:00-11:30"]}
    assert lower.params.dates == upper.params.dates, "the lower-case spelling is the same instant"
    assert offset.params.dates == upper.params.dates

    with pytest.raises(OpportuneError) as raised:
        presets.available_only("cand", [("1900-06-01t00:00z", "1900-06-01T02:00Z")])
    assert raised.value.code == "invalid_argument", raised.value.message
    assert "`1900-06-01t00:00z`" in raised.value.message, raised.value.message
    assert "1975-01-01T00:00Z..=2100-01-01T00:00Z" in raised.value.message, raised.value.message


def test_horizon_refuses_a_day_count_past_the_wire_range() -> None:
    """A day count is a `u32` on the wire, so the boundary refuses what serde would.

    The ceiling comes from the generated model's own bound, which the codegen derives
    from the schema's integer `format`; the preset inherits it rather than restating
    a number that could drift from the schema.
    """
    assert presets.horizon(0xFFFF_FFFF).params.max == DaysMax(days=0xFFFF_FFFF)

    with pytest.raises(OpportuneError) as raised:
        presets.horizon(0x1_0000_0000)

    assert raised.value.code == "invalid_argument", raised.value.message
    assert "4294967296" in raised.value.message, raised.value.message


def test_the_default_limits_are_the_engines_own_and_override_cleanly() -> None:
    """The reading of §10 defaults is the engine's, and an override reaches it."""
    defaults = default_limits()
    assert defaults.model_dump() == {
        "max_participants": 500,
        "max_blocks_per_participant": 10_000,
        "max_search_window_days": 92,
        "max_candidate_starts": 100_000,
        "max_explain_limit": 10_000,
        "max_rules": 500,
        "max_occurrences": 50_000,
        "max_requirement_slots": 64,
        "max_duration_minutes": 10_080,
        "max_zone_periods_per_participant": 1_000,
    }

    overridden = limits_with(max_search_window_days=120)
    assert isinstance(overridden, Limits)
    assert overridden.max_search_window_days == 120
    assert overridden.max_participants == defaults.max_participants

    assert with_updates(defaults, max_explain_limit=1).max_explain_limit == 1

    with pytest.raises(ValidationError):
        limits_with(max_participants=-1)
    with pytest.raises(ValidationError):
        limits_with(no_such_limit=1)

    # The override is a document the native engine parses, not only a model: an
    # engine built from it enforces the window it names.
    narrow = Engine(limits=limits_with(max_search_window_days=1))
    with pytest.raises(OpportuneError) as raised:
        narrow.find_availability(
            Request(
                now="2027-06-14T08:00Z",
                goal=single_goal(
                    "30m",
                    Requirement(all=[person("ana")]),
                    zone="UTC",
                    search_window=("2027-06-14T08:00Z", "2027-06-18T08:00Z"),
                ),
                participants=[Participant(id="ana", zone="UTC", calendar=[])],
            )
        )
    assert [(finding.code, finding.path) for finding in raised.value.errors] == [
        ("search_window_limit", "/goal/search_window")
    ]


def test_an_offer_limit_truncates_the_list_and_an_explain_limit_the_exclusions() -> None:
    """Reaching a limit truncates; it is not a validation error (§10)."""
    request = Request(
        now="2027-06-14T08:00Z",
        goal=single_goal(
            "30m",
            Requirement(all=[person("ana")]),
            zone="UTC",
            search_window=("2027-06-14T08:00Z", "2027-06-14T13:00Z"),
        ),
        participants=[Participant(id="ana", zone="UTC", calendar=[])],
        rules=[presets.business_hours("mon-sun", "09:00-12:00")],
        limit=2,
    )

    truncated = find_availability(request)
    assert len(truncated.offers) == 2
    assert truncated.truncated is True
    assert truncated.dates[0].offers == 6, "the date still reports every offer it had"

    explained = find_availability(with_updates(request, limit=None, explain=Explain(limit=1)))
    assert len(explained.offers) == 6
    assert len(explained.excluded) == 1
    assert explained.excluded_truncated is True


def test_every_generated_model_and_type_alias_is_exported_from_the_package() -> None:
    """The generated module declares its exports, so the check is exhaustive.

    ``_models.__all__`` is written from the generated module's own ``class`` and
    ``type ... =`` lines by the codegen post-pass, so a model or an ``Annotated``
    alias the schema gains is covered the moment the codegen writes it — a
    ``__module__`` walk alone cannot see the aliases. A generated name the package
    root does not re-export is a model a caller can only reach by importing a
    private module.
    """
    assert _models.__all__, "the generated module declares what it defines"

    unexported = sorted(name for name in _models.__all__ if name not in opportune.__all__)
    assert not unexported, f"`opportune` does not export {unexported}"
    assert all(hasattr(opportune, name) for name in _models.__all__), (
        "a name in `__all__` is not reachable as an attribute, so `from opportune import *` "
        "would fail on it"
    )

    defined = {
        name
        for name, value in vars(_models).items()
        if not name.startswith("_") and getattr(value, "__module__", None) == _models.__name__
    }
    assert defined <= set(_models.__all__), (
        f"the declaration misses the models {sorted(defined - set(_models.__all__))}"
    )
    for alias in ("Rule", "Selector", "Span"):
        assert alias in _models.__all__, f"the declaration covers the `{alias}` type alias"


def test_the_sugar_module_is_reachable_from_the_package() -> None:
    """`opportune.sugar` is a submodule a caller reaches through the package.

    Pyright strict accepts `opportune.sugar.person(…)` only when the package declares
    the module, so the export is what makes the sugar helpers reachable under the same
    name a runtime import finds, and what `from opportune import *` carries.
    """
    assert "sugar" in opportune.__all__
    assert opportune.sugar.person("ana") == person("ana")
    assert opportune.sugar.single_goal is single_goal


def test_the_public_projection_is_reachable_and_typed_by_the_package() -> None:
    """A public route's two models and two helpers are the package's own surface.

    The projection is exported from the package root rather than reached through a
    private module, so a route can declare ``PublicResponse``/``PublicSlotCheck`` as
    its ``response_model`` and call the helper that builds one. ``Payload`` is
    exported for the same reason: the adapter signatures name it, so a caller
    annotating the payload it forwards should be able to name it too.
    """
    exported = {
        "Payload",
        "PublicOffer",
        "PublicOfferEvent",
        "PublicResponse",
        "PublicSlotCheck",
        "public_response",
        "public_slot_check",
    }
    missing = sorted(exported - set(opportune.__all__))
    assert not missing, f"`opportune` does not export {missing}"
    assert all(hasattr(opportune, name) for name in exported)
    assert opportune.public_response is public_response
    assert opportune.public_slot_check is public_slot_check

    engine = Engine()
    request = Request(
        now="2027-06-14T08:00Z",
        goal=single_goal(
            "30m",
            Requirement(all=[person("ana")]),
            zone="UTC",
            search_window=("2027-06-14T09:00Z", "2027-06-14T11:00Z"),
        ),
        participants=[Participant(id="ana", zone="UTC", calendar=[])],
    )
    answer = engine.find_availability(request)
    projected = public_response(answer)

    assert isinstance(projected, PublicResponse)
    assert isinstance(projected.offers[0], PublicOffer)
    assert projected.offers[0].events[0].part == "main"
    assert set(projected.model_dump()) == {"dates", "meta", "offers", "truncated"}
    assert set(answer.model_dump()) == {
        "dates",
        "excluded",
        "excluded_truncated",
        "meta",
        "offers",
        "truncated",
        "warnings",
    }, "the full answer an operator route serves is untouched"

    check = engine.check_slot(
        request,
        {"events": [{"part": "main", "start": "2027-06-14T09:00Z", "assignment": [["ana"]]}]},
    )
    verdict = public_slot_check(check)
    assert isinstance(verdict, PublicSlotCheck)
    assert verdict.valid is check.valid
    assert set(verdict.model_dump()) == {"flags", "meta", "valid"}


def test_every_field_the_public_projection_keeps_is_the_answers_own() -> None:
    """A projection is a field-by-field copy, so each field it keeps reads its own.

    The shape tests say which fields a visitor is served; this one says each of them
    still holds what the answer held. The two truncation flags are the pair that
    makes the difference visible: an explanation cut short leaves
    ``excluded_truncated`` true while the offer list is whole, so a projection that
    read the wrong one tells a visitor its list of times is incomplete — or that a
    cut list is every time available — with nothing in the document looking wrong.
    """
    engine = Engine()
    request = Request(
        now="2027-03-09T15:00Z",
        goal=single_goal(
            "30m",
            Requirement(all=[person("ana")]),
            zone="UTC",
            search_window=("2027-03-09T15:00Z", "2027-03-09T18:00Z"),
        ),
        # No calendar at all, so every offer is flagged `calendar_unknown`: the one
        # flag the projection keeps is a list a copy could leave behind.
        participants=[Participant(id="ana", zone="UTC")],
        rules=[presets.business_hours("mon-sun", "09:00-17:00")],
        # One exclusion is reported, so the operator's answer says the explanation
        # was cut short while the offer list itself was not.
        explain=Explain(limit=1),
    )
    answer = engine.find_availability(request)
    assert answer.truncated is False and answer.excluded_truncated is True, (
        "the fixture distinguishes the two flags"
    )

    projected = public_response(answer)

    assert projected.truncated is answer.truncated
    assert projected.dates == answer.dates
    assert projected.meta == answer.meta
    assert len(projected.offers) == len(answer.offers)
    for kept, source in zip(projected.offers, answer.offers, strict=True):
        assert kept.flags == source.flags == ["calendar_unknown"]
        assert [(event.part, event.start, event.end) for event in kept.events] == [
            (event.part, event.start, event.end) for event in source.events
        ]

    check = engine.check_slot(
        request,
        {"events": [{"part": "main", "start": "2027-03-09T15:00Z", "assignment": [["ana"]]}]},
    )
    verdict = public_slot_check(check)

    assert verdict.valid is check.valid
    assert verdict.flags == check.flags == ["calendar_unknown"]
    assert verdict.meta == check.meta


def test_the_adapters_marker_constants_are_reachable_from_the_package() -> None:
    assert MARKER_TAGS == graph.MARKER_TAGS == google.MARKER_TAGS == "opportune.tags"
    assert (
        MARKER_EVENT_KEY
        == graph.MARKER_EVENT_KEY
        == google.MARKER_EVENT_KEY
        == ("opportune.event_key")
    )


U2_WEEKLY_HOURS = {
    "id": "weekly_hours",
    "kind": "window",
    "who": {"ids": ["ana"]},
    "when": {"tag": "interview"},
    "params": {
        "zone": "event",
        "weekly": {
            "mon": ["09:00-17:00"],
            "tue": ["09:00-12:00", "14:00-18:00"],
            "wed": ["09:00-17:00"],
            "thu": ["10:00-16:00"],
            "fri": ["22:00-02:00"],
        },
        "dates": {"2027-06-15": ["10:00-12:00"], "2027-12-25": []},
    },
}
"""The U2 schedule every binding writes from the same inputs, as one wire rule."""


def test_weekly_hours_writes_the_document_every_binding_reads() -> None:
    """Per-weekday hours, a split shift, a midnight-crossing range, a closed date
    and an override are one rule, encoded as the Rust and TypeScript presets encode
    them."""
    rule = presets.weekly_hours(
        {
            "mon": "09:00-17:00",
            "tue": ["09:00-12:00", "14:00-18:00"],
            "wed": "09:00-17:00",
            "thu": "10:00-16:00",
            "fri": "22:00-02:00",
        },
        dates={"2027-06-15": "10:00-12:00", "2027-12-25": []},
        zone="event",
        who={"ids": ["ana"]},
        when={"tag": "interview"},
    )

    assert rule.model_dump(mode="json", by_alias=True, exclude_none=True) == U2_WEEKLY_HOURS


def test_u2_weekly_hours_offers_only_the_days_and_hours_it_names() -> None:
    """The ordinary week is one rule: each weekday's own hours, a closed holiday
    and a date that keeps only its own hours."""
    request = Request(
        now="2027-06-14T00:00Z",
        goal=single_goal(
            "30m",
            Requirement(all=[person("ana")]),
            zone="UTC",
            search_window=("2027-06-14T00:00Z", "2027-06-18T00:00Z"),
        ),
        participants=[Participant(id="ana", zone="UTC", calendar=[])],
        rules=[
            presets.weekly_hours(
                {
                    "mon": "09:00-17:00",
                    "tue": ["09:00-12:00", "14:00-18:00"],
                    "wed": "09:00-17:00",
                    "thu": "10:00-16:00",
                },
                # The Monday holiday is closed and Wednesday gets its own hours,
                # both overriding the weekday key.
                dates={"2027-06-14": [], "2027-06-16": ["13:00-15:00"]},
                zone="UTC",
            )
        ],
    )

    starts = [offer_start(offer) for offer in find_availability(request).offers]

    assert "2027-06-14T09:00Z" not in starts, "the closed date overrides Monday's hours"
    assert "2027-06-15T09:00Z" in starts, "Tuesday's morning session"
    assert "2027-06-15T13:00Z" not in starts, "Tuesday's lunch is closed"
    assert "2027-06-15T14:00Z" in starts, "Tuesday's afternoon session"
    assert "2027-06-16T09:30Z" not in starts, "the date replaces Wednesday's shared hours"
    assert "2027-06-16T13:00Z" in starts, "the date's own hours"
    assert "2027-06-16T15:00Z" not in starts, "and nothing after them"
    assert "2027-06-17T09:30Z" not in starts, "Thursday starts at 10:00"
    assert "2027-06-17T10:00Z" in starts
    assert "2027-06-17T16:00Z" not in starts, "a 30-minute booking cannot fit at 16:00"
    assert not any(start.startswith("2027-06-18") for start in starts), "Friday has no hours"


def test_weekly_hours_rejects_a_shape_or_an_overlap_it_could_never_mean() -> None:
    """The preset names the argument whose shape is wrong, and leaves range
    overlap to the engine, which reports the key that holds it."""
    with pytest.raises(OpportuneError) as raised:
        presets.weekly_hours(["mon"])  # pyright: ignore[reportArgumentType]
    assert raised.value.code == "invalid_argument"
    assert "`weekly`" in raised.value.message

    with pytest.raises(OpportuneError) as raised:
        presets.weekly_hours({"mon": "9-5"})
    assert raised.value.code == "invalid_argument"
    assert "`weekly[mon]`" in raised.value.message

    request = Request(
        now="2027-06-14T00:00Z",
        goal=single_goal(
            "30m",
            Requirement(all=[person("ana")]),
            zone="UTC",
            search_window=("2027-06-14T00:00Z", "2027-06-15T00:00Z"),
        ),
        participants=[Participant(id="ana", zone="UTC", calendar=[])],
        rules=[presets.weekly_hours({"mon": ["09:00-12:00", "11:00-13:00"]}, zone="UTC")],
    )
    with pytest.raises(OpportuneError) as raised:
        find_availability(request)
    assert raised.value.code == "window_ranges_overlap"
    assert raised.value.path == "/rules/0/params/weekly/mon"

    # Two keys that claim one day are the mistake this preset exists to make
    # unnecessary, so it is rejected rather than intersected or merged.
    repeated = with_updates(
        request,
        rules=[presets.weekly_hours({"mon": "09:00-17:00", "mon-fri": "10:00-16:00"}, zone="UTC")],
    )
    with pytest.raises(OpportuneError) as raised:
        find_availability(repeated)
    assert raised.value.code == "window_weekday_repeated"
    assert raised.value.path == "/rules/0/params/weekly"


def test_available_only_for_two_attendees_composes_in_one_request() -> None:
    """The attendee decides the rule ID, so a second attendee is a second rule."""
    # Each attendee states different availability, so the offers are the
    # intersection: either rule alone would offer a start the other forbids.
    ana, ana_availability = presets.available_only(
        "ana", [("2027-06-14T10:00Z", "2027-06-14T11:00Z")]
    )
    bob, bob_availability = presets.available_only(
        "bob", [("2027-06-14T10:30Z", "2027-06-14T11:30Z")]
    )
    assert (ana_availability.id, bob_availability.id) == (
        "available_only:ana",
        "available_only:bob",
    )

    renamed = presets.available_only(
        "cy", [("2027-06-14T10:00Z", "2027-06-14T11:00Z")], rule_id="cy_available"
    )[1]
    assert renamed.id == "cy_available", "an explicit ID still wins"

    request = Request(
        now="2027-06-14T00:00Z",
        goal=single_goal(
            "30m",
            Requirement(all=[person("xx")]),
            zone="UTC",
            search_window=("2027-06-14T09:00Z", "2027-06-14T12:00Z"),
        ),
        participants=[Participant(id="xx", zone="UTC", calendar=[]), ana, bob],
        rules=[ana_availability, bob_availability],
    )

    assert validate(request).errors == [], "two attendees' availability does not collide"
    assert [offer_start(offer) for offer in find_availability(request).offers] == [
        "2027-06-14T10:30Z"
    ], "both attendees' stated availability is in force, not just one of them"


def test_single_goal_carries_the_fields_a_real_booking_link_needs() -> None:
    """The display zone, grid, candidates, part ID, tags and travel reach the goal
    and the part, and every one of them still defaults when it is not named."""
    travel = Travel(
        before="15m",
        after="15m",
        by_participant={"ana": ParticipantTravel(before="30m", after="30m")},
    )
    goal = single_goal(
        "45m",
        Requirement(all=[person("ana")]),
        zone="Europe/London",
        search_window=("2027-06-14T08:00Z", "2027-06-15T08:00Z"),
        display_zone="America/New_York",
        grid={"step": "15m"},
        part_id="interview",
        tags=["interview/front-end"],
        travel=travel,
    )

    assert goal.display_zone == "America/New_York"
    assert goal.grid == Grid(step="15m")
    part = goal.parts[0]
    assert (part.id, part.tags, part.travel) == (
        "interview",
        ["interview/front-end"],
        travel,
    )

    request = Request(
        now="2027-06-14T08:00Z",
        goal=goal,
        participants=[Participant(id="ana", zone="Europe/London", calendar=[])],
    )
    assert validate(request).errors == []

    exact = single_goal(
        "30m",
        Requirement(all=[person("ana")]),
        zone="UTC",
        search_window=("2027-06-14T08:00Z", "2027-06-15T08:00Z"),
        candidates=["2027-06-14T09:00Z"],
    )
    assert exact.candidates == ["2027-06-14T09:00Z"]
    assert validate(with_updates(request, goal=exact)).errors == []

    plain = single_goal(
        "30m",
        Requirement(all=[person("ana")]),
        zone="UTC",
        search_window=("2027-06-14T08:00Z", "2027-06-15T08:00Z"),
    )
    assert (plain.display_zone, plain.grid, plain.candidates) == (None, None, None)
    assert (plain.parts[0].id, plain.parts[0].tags, plain.parts[0].travel) == (
        None,
        [],
        None,
    ), "omitting the new fields keeps the shapes the shortcut always built"


def test_single_goal_rejects_a_model_failure_as_the_packages_error() -> None:
    """A mapping the generated goal refuses is the package's rejection, not a
    ``ValidationError`` an application handler for ``OpportuneError`` misses."""
    with pytest.raises(OpportuneError) as raised:
        single_goal(
            "30m",
            Requirement(all=[person("ana")]),
            zone="UTC",
            search_window=("2027-06-14T08:00Z", "2027-06-15T08:00Z"),
            grid={"step": 5},
        )

    assert raised.value.code == "invalid_argument"
    assert isinstance(raised.value.__cause__, ValidationError)


def test_single_goal_reads_one_string_tag_or_candidate_as_one_value() -> None:
    """``str`` satisfies ``Sequence[str]``, so a caller holding one tag or one
    instant as a string type-checks; reading it as a sequence of characters would
    build a goal the caller never wrote — tag letters, and a ``when`` selector that
    names the tag silently stops applying to the part."""
    tagged = single_goal(
        "30m",
        Requirement(all=[person("ana")]),
        zone="UTC",
        search_window=("2027-06-14T08:00Z", "2027-06-14T12:00Z"),
        tags="interview",
    )
    assert tagged.parts[0].tags == ["interview"]

    request = Request(
        now="2027-06-14T08:00Z",
        goal=tagged,
        participants=[Participant(id="ana", zone="UTC", calendar=[])],
        rules=[presets.business_hours("mon", "09:00-17:00", zone="UTC", when={"tag": "interview"})],
    )
    assert [offer_start(offer) for offer in find_availability(request).offers] == [
        "2027-06-14T09:00Z",
        "2027-06-14T09:30Z",
        "2027-06-14T10:00Z",
        "2027-06-14T10:30Z",
        "2027-06-14T11:00Z",
        "2027-06-14T11:30Z",
    ], "the rule scoped to the tag applies to the one-tag part"

    exact = single_goal(
        "30m",
        Requirement(all=[person("ana")]),
        zone="UTC",
        search_window=("2027-06-14T08:00Z", "2027-06-15T08:00Z"),
        candidates="2027-06-14T09:00Z",
    )
    assert exact.candidates == ["2027-06-14T09:00Z"]

    with pytest.raises(OpportuneError) as raised:
        single_goal(
            "30m",
            Requirement(all=[person("ana")]),
            zone="UTC",
            search_window=("2027-06-14T08:00Z", "2027-06-15T08:00Z"),
            tags=5,  # pyright: ignore[reportArgumentType]
        )
    assert raised.value.code == "invalid_argument"
    assert "`tags`" in raised.value.message


def test_weekly_hours_carries_a_crossing_range_and_an_explicitly_closed_day() -> None:
    """Wire shapes exercised nowhere else in the preset suite: a
    weekday range measured past midnight, and a weekday mapped to no hours at all."""

    def starts(weekly: dict[str, str | Sequence[str]], window: tuple[str, str]) -> list[str]:
        request = Request(
            now=window[0],
            goal=single_goal(
                "30m",
                Requirement(all=[person("ana")]),
                zone="UTC",
                search_window=window,
            ),
            participants=[Participant(id="ana", zone="UTC", calendar=[])],
            rules=[presets.weekly_hours(weekly, zone="UTC")],
        )
        return [offer_start(offer) for offer in find_availability(request).offers]

    assert starts({"fri": "22:00-02:00"}, ("2027-06-18T20:00Z", "2027-06-19T04:00Z")) == [
        "2027-06-18T22:00Z",
        "2027-06-18T22:30Z",
        "2027-06-18T23:00Z",
        "2027-06-18T23:30Z",
        "2027-06-19T00:00Z",
        "2027-06-19T00:30Z",
        "2027-06-19T01:00Z",
        "2027-06-19T01:30Z",
    ], "Friday's range is open past midnight into Saturday"
    explicitly_closed = starts(
        {"mon": [], "tue": "09:00-17:00"},
        ("2027-06-14T00:00Z", "2027-06-16T00:00Z"),
    )
    assert not [start for start in explicitly_closed if start.startswith("2027-06-14")], (
        "a weekday mapped to no hours is closed, not an invalid rule"
    )
    assert explicitly_closed[0] == "2027-06-15T09:00Z", "its sibling weekday still opens"


def test_unless_excludes_its_selector_from_an_unconditional_and_a_scoped_rule() -> None:
    """One call composes the exception, and nothing but ``when`` changes."""
    assert unless is presets.unless, "the package re-exports the preset it documents"

    hours = presets.business_hours("mon-fri", "09:00-17:00")
    vip = TagSelector(tag="vip")

    unconditional = unless(hours, vip)
    assert unconditional.when == NotSelector(not_=vip)
    assert unconditional.who == hours.who, "`who` is untouched"
    assert unconditional.id == hours.id, "the ID is untouched"
    assert unconditional.params == hours.params, "the kind is untouched"

    interview = TagSelector(tag="interview")
    scoped = unless(with_updates(hours, when=interview), vip)
    assert scoped.when == AllSelector(all=[interview, NotSelector(not_=vip)]), (
        "a scoped rule keeps its scope and adds the exception"
    )

    kept = scoped.when
    assert kept is not None
    twice = unless(scoped, TagSelector(tag="board"))
    assert twice.when == AllSelector(all=[kept, NotSelector(not_=TagSelector(tag="board"))]), (
        "chaining a second exception nests a new `all` rather than replacing the first"
    )

    # A rule whose `when` was left at the wire's `null` is unconditional too: the
    # model permits it and the engine reads it as `true`.
    unprefixed = WindowRule.model_validate({**hours.model_dump(), "when": None})
    assert unless(unprefixed, vip).when == NotSelector(not_=vip)

    # `True` is the model's own spelling of every booking, and what a rule read back
    # from storage carries: the wire writes `when` on every rule.
    explicitly_all = WindowRule.model_validate({**hours.model_dump(), "when": True})
    assert explicitly_all.when is True
    assert unless(explicitly_all, vip).when == NotSelector(not_=vip)

    # A scope that is itself a negation composes the same way; `not_` is the field
    # `model_dump()` writes and `with_updates` reads back.
    internal = NotSelector(not_=TagSelector(tag="internal"))
    negated = unless(with_updates(hours, when=internal), vip)
    assert negated.when == AllSelector(all=[internal, NotSelector(not_=vip)])


def test_the_excepted_tag_is_exempt_from_the_global_and_group_windows() -> None:
    """The exception loosens both tiers instead of intersecting with them to nothing."""

    def offers(part_tags: list[str]) -> list[str]:
        request = Request(
            now="2027-06-14T00:00Z",
            goal=Goal(
                zone="UTC",
                search_window=("2027-06-14T00:00Z", "2027-06-20T00:00Z"),
                candidates=[
                    "2027-06-14T10:00Z",
                    "2027-06-14T14:00Z",
                    "2027-06-19T10:00Z",
                ],
                parts=[
                    Part(
                        duration="30m",
                        tags=part_tags,
                        requirement=Requirement(all=[person("ana")]),
                    )
                ],
            ),
            participants=[Participant(id="ana", zone="UTC", calendar=[])],
            rules=[
                unless(presets.business_hours(zone="UTC"), {"tag": "vip"}),
                unless(
                    presets.business_hours(
                        "mon-fri",
                        "09:00-12:00",
                        zone="UTC",
                        rule_id="interview_hours",
                        when=TagSelector(tag="interview"),
                    ),
                    {"tag": "vip"},
                ),
            ],
            explain=Explain(limit=100),
        )
        return [offer_start(offer) for offer in find_availability(request).offers]

    assert offers(["interview"]) == ["2027-06-14T10:00Z"], (
        "an ordinary interview gets only the hours both tiers are open"
    )
    assert offers(["interview", "vip"]) == [
        "2027-06-14T10:00Z",
        "2027-06-14T14:00Z",
        "2027-06-19T10:00Z",
    ], "the excepted booking is exempt from both windows"


def test_unless_rejects_a_value_it_could_never_mean() -> None:
    """A rule that is not a model, and a selector the model refuses, are the package's error."""
    hours = presets.business_hours()
    with pytest.raises(OpportuneError) as raised:
        unless({"kind": "window"}, TagSelector(tag="vip"))  # pyright: ignore[reportArgumentType]
    assert raised.value.code == "invalid_argument"
    assert "`rule`" in raised.value.message

    with pytest.raises(OpportuneError) as raised:
        unless(hours, {"idz": ["ana"]})
    assert raised.value.code == "invalid_argument"
    assert "`not_`" in raised.value.message, (
        "the rejection names the selector field the argument became"
    )


def test_unless_rejects_a_model_that_is_not_a_rule() -> None:
    """A generated model that is not a rule is the package's error, not an attribute miss.

    ``unless`` reads ``rule.when``; a model of another shape has no such field, so a
    guard that only asks "is this a pydantic model" lets the read raise
    ``AttributeError`` past the boundary the docstring promises.
    """
    with pytest.raises(OpportuneError) as raised:
        unless(Participant(id="ana", zone="UTC"), TagSelector(tag="vip"))  # pyright: ignore[reportArgumentType]
    assert raised.value.code == "invalid_argument"
    assert "`rule`" in raised.value.message
    assert "Participant" in raised.value.message, "the rejection names the type it got"

    with pytest.raises(OpportuneError) as raised:
        unless(presets.business_hours().params, TagSelector(tag="vip"))  # pyright: ignore[reportArgumentType]
    assert raised.value.code == "invalid_argument"
    assert "`rule`" in raised.value.message


def test_and_who_and_except_who_narrow_the_default_host_selector_without_nesting() -> None:
    """The participant-side twins intersect with ``who`` instead of replacing it.

    Replacing ``who`` is what a selector argument is usually taken to do, and
    replacing the default ``{"role": "host"}`` with ``{"kind": "person"}`` quietly
    widens a host rule to every person — attendees included. These helpers keep the
    host clause beside the new one, and a chain of them appends to the same ``all``
    rather than nesting one inside another.
    """
    assert and_who is presets.and_who, "the package re-exports the preset it documents"
    assert except_who is presets.except_who

    hours = presets.business_hours()
    assert hours.who == RoleSelector(role="host"), "a preset starts from every host"

    people = KindSelector(kind="person")
    narrowed = and_who(hours, people)
    assert isinstance(narrowed, WindowRule), "the runtime rule type is kept"
    assert narrowed.who == AllSelector(all=[RoleSelector(role="host"), people]), (
        "the default host clause survives the narrowing"
    )
    assert narrowed.when == hours.when, "`when` is untouched"
    assert narrowed.params == hours.params, "the kind is untouched"
    assert narrowed.id == hours.id, "the ID is untouched"

    exempt = except_who(narrowed, IdsSelector(ids=["ceo"]))
    assert exempt.who == AllSelector(
        all=[
            RoleSelector(role="host"),
            people,
            NotSelector(not_=IdsSelector(ids=["ceo"])),
        ]
    ), "a chained exception appends to the same `all` instead of nesting one"

    single = with_updates(hours, who=IdsSelector(ids=["ana"]))
    assert and_who(single, people).who == AllSelector(all=[IdsSelector(ids=["ana"]), people]), (
        "a single clause becomes the first of an `all`"
    )

    everyone = with_updates(hours, who=True)
    assert and_who(everyone, {"tag": "vip"}).who == TagSelector(tag="vip"), (
        "an unconditional `who` takes the selector alone"
    )

    # A rule read back from storage can carry the wire's explicit `null`, which the
    # engine reads as the same default: narrowing it starts from the host clause
    # rather than from nothing, which would hand the rule to every attendee too.
    null_who = with_updates(hours, who=None)
    assert and_who(null_who, people).who == AllSelector(all=[RoleSelector(role="host"), people])


def test_and_who_keeps_a_host_rule_off_a_room_and_an_attendee() -> None:
    """``and_who(person)`` narrows a host rule without ever reaching an attendee.

    A ``rest`` rule reaches whoever its ``who`` selects, and everyone here finished
    late on Monday, so the rule's reach is exactly what decides Tuesday's early
    slot. A resource is a host and a person may be an attendee; the intersection
    leaves the room out and keeps the default host clause beside ``kind: person``.
    """
    late = Block(span=("2027-06-14T22:00Z", "2027-06-14T23:30Z"), event_key="late")
    rest = presets.rest(late_after="22:00", not_before="08:00")

    def offered(host: str, participants: list[Participant], rule: Rule) -> list[str]:
        request = Request(
            now="2027-06-14T00:00Z",
            goal=single_goal(
                "30m",
                Requirement(all=[person(host)]),
                zone="UTC",
                search_window=("2027-06-15T07:00Z", "2027-06-15T07:30Z"),
                candidates=["2027-06-15T07:00Z"],
            ),
            participants=participants,
            rules=[rule],
        )
        assert validate(request).errors == [], "the fixture is a valid request"
        return [offer_start(offer) for offer in find_availability(request).offers]

    room = Participant(id="room-4", kind="resource", zone="UTC", calendar=[late])
    assert offered("room-4", [room], rest) == [], (
        "the default `who` is every host, and a room is a host"
    )
    assert offered("room-4", [room], and_who(rest, {"kind": "person"})) == ["2027-06-15T07:00Z"], (
        "`and_who(person)` keeps a resource room out of a host rule"
    )

    ana = Participant(id="ana", zone="UTC", calendar=[])
    tom = Participant(id="tom", role="attendee", zone="UTC", calendar=[late])
    assert offered("ana", [ana, tom], rest) == ["2027-06-15T07:00Z"], (
        "the default `who` never reaches an attendee"
    )
    assert offered("ana", [ana, tom], and_who(rest, {"kind": "person"})) == ["2027-06-15T07:00Z"], (
        "`and_who(person)` keeps the default host clause, so the attendee stays out"
    )
    assert offered("ana", [ana, tom], with_updates(rest, who=KindSelector(kind="person"))) == [], (
        "the twin `with_updates(who=person)` drops the host clause and reaches the attendee"
    )


def test_except_who_exempts_the_named_host_and_leaves_attendees_alone() -> None:
    """``except_who`` narrows ``who`` by ``not(selector)``: every other host is judged.

    The exempt host fits while the ordinary host is still bound by the cap, and the
    default ``Role::Host`` clause is what keeps an attendee out of the judgement.
    """

    def held(id: str, role: Role) -> Participant:
        return Participant(
            id=id,
            role=role,
            zone="UTC",
            calendar=[Block(span=("2027-06-14T09:00Z", "2027-06-14T09:30Z"), tags=["meeting"])],
        )

    def free(id: str) -> Participant:
        return Participant(id=id, zone="UTC", calendar=[])

    def cap() -> Rule:
        return presets.except_who(presets.cap_per_day({"tag": "meeting"}, 1), {"ids": ["ceo"]})

    def offered(host: str, participants: list[Participant], rule: Rule) -> list[str]:
        request = Request(
            now="2027-06-14T00:00Z",
            goal=single_goal(
                "30m",
                Requirement(all=[person(host)]),
                zone="UTC",
                search_window=("2027-06-14T14:00Z", "2027-06-14T14:30Z"),
                candidates=["2027-06-14T14:00Z"],
                tags=["meeting"],
            ),
            participants=participants,
            rules=[rule],
        )
        assert validate(request).errors == [], "the fixture is a valid request"
        return [offer_start(offer) for offer in find_availability(request).offers]

    assert offered("ana", [held("ana", "host"), free("ceo")], cap()) == [], (
        "the cap still binds a host it does not except"
    )
    assert offered("ceo", [held("ceo", "host")], cap()) == ["2027-06-14T14:00Z"], (
        "`except_who` narrows `who`, so the exempt host fits"
    )
    assert offered("ana", [free("ana"), held("tom", "attendee"), free("ceo")], cap()) == [
        "2027-06-14T14:00Z"
    ], "the default `Role::Host` clause keeps an attendee out of the cap"

    reaching_people = presets.except_who(
        with_updates(presets.cap_per_day({"tag": "meeting"}, 1), who=KindSelector(kind="person")),
        {"ids": ["ceo"]},
    )
    assert (
        offered("ana", [free("ana"), held("tom", "attendee"), free("ceo")], reaching_people) == []
    ), "a cap whose `who` is replaced with `kind: person` reaches the attendee"


def test_and_who_and_except_who_reject_a_value_they_could_never_mean() -> None:
    """A rule that is not a model, and a selector the model refuses, are the package's error."""
    hours = presets.business_hours()
    for narrow, field in ((and_who, "`all`"), (except_who, "`not_`")):
        with pytest.raises(OpportuneError) as raised:
            narrow({"kind": "window"}, {"tag": "vip"})  # pyright: ignore[reportArgumentType]
        assert raised.value.code == "invalid_argument"
        assert "`rule`" in raised.value.message

        with pytest.raises(OpportuneError) as raised:
            narrow(hours, {"idz": ["ana"]})
        assert raised.value.code == "invalid_argument"
        assert field in raised.value.message, (
            "the rejection names the selector field the argument became"
        )
        assert "`idz`" in raised.value.message, "and the key the caller wrote"

        with pytest.raises(OpportuneError) as raised:
            narrow(hours.params, {"tag": "vip"})  # pyright: ignore[reportArgumentType]
        assert raised.value.code == "invalid_argument"
        assert "`rule`" in raised.value.message, "a generated model that is not a rule is refused"


def test_public_projection_accepts_mappings_and_rejects_malformed_inputs() -> None:
    """`public_response` and `public_slot_check` read models or wire mappings and
    never leak raw AttributeError or ValidationError."""
    request = Request(
        now="2027-06-14T08:00Z",
        goal=single_goal(
            "30m",
            {"all": [person("ana")]},
            zone="UTC",
            search_window=("2027-06-14T09:00Z", "2027-06-14T11:00Z"),
        ),
        participants=[Participant(id="ana", zone="UTC", calendar=[])],
    )
    response = find_availability(request)
    response_doc = response.model_dump(by_alias=True, exclude_none=True)
    assert public_response(response_doc) == public_response(response)

    check = check_slot(request, offer_to_proposal(response.offers[0]))
    check_doc = check.model_dump(by_alias=True, exclude_none=True)
    assert public_slot_check(check_doc) == public_slot_check(check)

    # A document written by a newer engine carries fields this build has no name
    # for. The response models are open for exactly that, so the projection reads
    # what it knows and drops the rest rather than failing a rolling deploy.
    assert public_response({**response_doc, "future_field": "v0.2"}) == public_response(response)
    assert public_slot_check({**check_doc, "future_field": "v0.2"}) == public_slot_check(check)

    bad_responses: tuple[object, ...] = (
        None,
        5,
        "not a response",
        {},
        {**response_doc, "dates": "2027-06-14"},
    )
    for bad in bad_responses:
        with pytest.raises(OpportuneError) as raised:
            public_response(bad)  # pyright: ignore[reportArgumentType]
        assert raised.value.code == "invalid_argument", repr(bad)
        assert "`response`" in raised.value.message
        if isinstance(bad, dict):
            assert isinstance(raised.value.__cause__, ValidationError)

    bad_checks: tuple[object, ...] = (
        None,
        5,
        [],
        {},
        {**check_doc, "valid": "nope"},
    )
    for bad in bad_checks:
        with pytest.raises(OpportuneError) as raised:
            public_slot_check(bad)  # pyright: ignore[reportArgumentType]
        assert raised.value.code == "invalid_argument", repr(bad)
        assert "`check`" in raised.value.message
        if isinstance(bad, dict):
            assert isinstance(raised.value.__cause__, ValidationError)


def _stored_projection_documents() -> tuple[dict[str, object], dict[str, object]]:
    """One answer and one slot check, as the wire documents a deployment stores."""
    request = Request(
        now="2027-06-14T08:00Z",
        goal=single_goal(
            "30m",
            {"all": [person("ana")]},
            zone="UTC",
            search_window=("2027-06-14T09:00Z", "2027-06-14T11:00Z"),
        ),
        participants=[Participant(id="ana", zone="UTC", calendar=[])],
    )
    response = find_availability(request)
    check = check_slot(request, offer_to_proposal(response.offers[0]))
    return (
        response.model_dump(by_alias=True, exclude_none=True),
        check.model_dump(by_alias=True, exclude_none=True),
    )


@pytest.mark.parametrize(
    ("dropped", "changed", "field"),
    [
        ((), {"dates": "2027-06-14"}, "dates"),
        (("offers",), {}, "offers"),
    ],
    ids=["dates-is-text", "offers-missing"],
)
def test_a_refused_response_document_is_rejected_at_the_field_the_model_named(
    dropped: tuple[str, ...], changed: dict[str, object], field: str
) -> None:
    """A stored answer the model refuses names the field that is wrong, not the argument.

    What is at fault is the document's own field — `dates` written as text, an
    answer with no `offers` — so the rejection has to name that field, with the
    model's finding chained for the log or debugger reading a stored document. A
    rejection naming only the helper's own argument, or one with no chained finding,
    leaves an operator holding a document with nowhere to look.

    A key no response names is deliberately not a case: the response models are
    open, because serde reads them without `deny_unknown_fields`, so an older
    deployment has to accept a field a newer engine added. The projection drops it
    instead, which is what makes the additive release safe mid-roll.

    The package's own shape for the class is kept: the argument itself is what the
    error is about, so there is no request field path and no finding list on the
    error, and the model's finding stays on the `__cause__`.
    """
    response_doc, _ = _stored_projection_documents()
    for key in dropped:
        response_doc.pop(key)
    response_doc.update(changed)

    with pytest.raises(OpportuneError) as raised:
        public_response(response_doc)

    assert raised.value.code == "invalid_argument"
    assert f"`{field}`" in raised.value.message, raised.value.message
    assert raised.value.path is None, "the stored document is at fault, not a request field"
    assert raised.value.errors == [], "the model's own findings stay on the cause"
    assert isinstance(raised.value.__cause__, ValidationError)
    assert raised.value.__cause__.errors(include_url=False)[0]["loc"] == (field,), (
        "the chained finding names the field the message names"
    )


@pytest.mark.parametrize(
    ("dropped", "changed", "field"),
    [
        ((), {"valid": "nope"}, "valid"),
        (("flags",), {}, "flags"),
    ],
    ids=["valid-is-text", "flags-missing"],
)
def test_a_refused_slot_check_document_is_rejected_at_the_field_the_model_named(
    dropped: tuple[str, ...], changed: dict[str, object], field: str
) -> None:
    """A stored slot check the model refuses names the verdict's field at fault.

    The confirmation helper reads a stored check as readily as the search helper
    reads a stored answer, so the same contract holds: `valid` given as text, or a
    check with no `flags`, is rejected at that field with the model's finding
    chained, rather than as an `AttributeError` from reading the mapping or a
    `ValidationError` Pydantic raised past the boundary. A key no check names is not
    a case here either: the verdict is open to a newer engine's additive field for
    the same reason the answer is.
    """
    _, check_doc = _stored_projection_documents()
    for key in dropped:
        check_doc.pop(key)
    check_doc.update(changed)

    with pytest.raises(OpportuneError) as raised:
        public_slot_check(check_doc)

    assert raised.value.code == "invalid_argument"
    assert f"`{field}`" in raised.value.message, raised.value.message
    assert raised.value.path is None, "the stored document is at fault, not a request field"
    assert raised.value.errors == [], "the model's own findings stay on the cause"
    assert isinstance(raised.value.__cause__, ValidationError)
    assert raised.value.__cause__.errors(include_url=False)[0]["loc"] == (field,), (
        "the chained finding names the field the message names"
    )


def test_available_only_rejects_non_rfc3339_and_sub_minute_utc_offsets() -> None:
    """Strict RFC 3339 parsing rejects ISO week dates, basic forms, space separators,
    and sub-minute UTC offsets instead of silently flooring them."""
    for bad_instant in (
        "2027-W24-1T09:00Z",
        "20270614T0900Z",
        "2027-06-14 09:00Z",
        "2027-06-14T09:00:00+00:00:30",
        "2027-06-14T09:00:00.000001Z",
    ):
        with pytest.raises(OpportuneError) as raised:
            presets.available_only("cand", [(bad_instant, "2027-06-14T10:00Z")])
        assert raised.value.code == "invalid_argument", bad_instant


def test_top_level_json_free_functions_match_engine_methods() -> None:
    """Top-level `*_json` free functions mirror `Engine.*_json` and TypeScript exports."""
    request = Request(
        now="2027-06-14T08:00Z",
        goal=single_goal(
            "30m",
            Requirement(all=[person("ana")]),
            zone="UTC",
            search_window=("2027-06-14T09:00Z", "2027-06-14T10:00Z"),
        ),
        participants=[Participant(id="ana", zone="UTC", calendar=[])],
    )
    engine = Engine()
    proposal = offer_to_proposal(engine.find_availability(request).offers[0])

    assert find_availability_json(request) == engine.find_availability_json(request)
    assert check_slot_json(request, proposal) == engine.check_slot_json(request, proposal)
    assert validate_json(request) == engine.validate_json(request)
    assert required_ranges_json(request) == engine.required_ranges_json(request)


def test_unless_keeps_runtime_rule_type_and_horizon_accepts_mapping() -> None:
    """`unless` keeps runtime rule instances; `horizon` accepts mapping forms."""
    capped = unless(presets.cap_per_day({"tag": "interview"}, 2), {"tag": "vip"})
    assert isinstance(capped, CapRule)
    assert capped.params.limit == 2

    policy = unless(
        presets.status_policy([StatusPolicyEntry(status="tentative", effect="ignore")]),
        {"tag": "vip"},
    )
    assert isinstance(policy, StatusPolicyRule)
    assert policy.kind == "status_policy"

    mapped_horizon = presets.horizon({"calendar": "org", "working_days": 3})
    assert mapped_horizon.params.max == WorkingDaysMax(calendar="org", working_days=3)


def test_status_policy_preset_kind_selector_and_ignores_resource_warning() -> None:
    """An unscoped `status_policy` warns on resources; scoping with `KindSelector`
    silences the warning while keeping tentative holds blocking on resources."""
    unscoped = presets.status_policy([{"status": "tentative", "effect": "ignore"}])
    assert isinstance(unscoped, StatusPolicyRule)
    assert unscoped.id == "status_policy"

    for bad_entries in (None, "tentative", {"status": "tentative"}, [5]):
        with pytest.raises(OpportuneError) as raised:
            presets.status_policy(bad_entries)  # pyright: ignore[reportArgumentType]
        assert raised.value.code == "invalid_argument"

    base = Request(
        now="2027-06-14T08:00Z",
        goal=single_goal(
            "30m",
            Requirement(all=[person("ana"), at_least(1, KindSelector(kind="resource"))]),
            zone="UTC",
            search_window=("2027-06-14T09:00Z", "2027-06-14T10:00Z"),
        ),
        participants=[
            Participant(
                id="ana",
                kind="person",
                zone="UTC",
                calendar=[
                    Block(span=("2027-06-14T09:00Z", "2027-06-14T09:30Z"), status="tentative")
                ],
            ),
            Participant(
                id="room-a",
                kind="resource",
                zone="UTC",
                calendar=[
                    Block(span=("2027-06-14T09:00Z", "2027-06-14T09:30Z"), status="tentative")
                ],
            ),
        ],
        rules=[unscoped],
    )
    diagnostics = validate(base)
    assert diagnostics.errors == []
    assert any(
        w.code == "status_policy_ignores_resource" and w.path == "/rules/0"
        for w in diagnostics.warnings
    ), diagnostics.warnings

    scoped = presets.status_policy(
        [StatusPolicyEntry(status="tentative", effect="ignore")],
        who=KindSelector(kind="person"),
    )
    safe = with_updates(base, rules=[scoped])
    assert validate(safe).warnings == []
    assert [offer_start(o) for o in find_availability(safe).offers] == ["2027-06-14T09:30Z"]


def test_status_policy_refuses_an_iterable_whose_order_it_would_choose() -> None:
    """The order entries are written in is the precedence they apply in.

    A set, a mapping's values or a one-shot iterable would let the preset decide that
    order, silently producing a policy whose precedence nobody wrote — which is the one
    thing a caller reading a rule back cannot debug. Only an ordered sequence states it,
    and the rejection names that reason.
    """
    entry = {"status": "tentative", "effect": "ignore"}

    for unordered in (
        {"status", "effect"},
        frozenset({"status"}),
        set[str](),
        {"first": entry, "second": entry}.values(),
        (stated for stated in [entry]),
    ):
        with pytest.raises(OpportuneError) as raised:
            presets.status_policy(unordered)  # pyright: ignore[reportArgumentType]
        assert raised.value.code == "invalid_argument", repr(unordered)
        assert "sequence" in raised.value.message, raised.value.message

    stated = presets.status_policy((entry,))
    assert stated.params.entries == [StatusPolicyEntry(status="tentative", effect="ignore")]


def test_exclude_event_keys_rescheduling_and_validation() -> None:
    """`exclude_event_keys` frees the event's time and cap slot; empty keys and
    excess key counts fail validation with exact pointers."""
    request = Request(
        now="2027-06-14T08:00Z",
        goal=single_goal(
            "30m",
            Requirement(all=[person("ana")]),
            zone="UTC",
            search_window=("2027-06-14T09:00Z", "2027-06-14T11:00Z"),
            tags="interview",
        ),
        participants=[
            Participant(
                id="ana",
                zone="UTC",
                calendar=[
                    _block(
                        ("2027-06-14T09:00Z", "2027-06-14T09:30Z"), ["interview"], key="evt-move"
                    )
                ],
            )
        ],
        rules=[presets.cap_per_day({"tag": "interview"}, 1)],
    )
    assert find_availability(request).offers == [], "daily cap of 1 is already used"

    rescheduled = find_availability(with_updates(request, exclude_event_keys=["evt-move"]))
    starts = [offer_start(o) for o in rescheduled.offers]
    assert "2027-06-14T09:00Z" in starts
    assert "2027-06-14T10:00Z" in starts

    empty = validate(with_updates(request, exclude_event_keys=["  "]))
    assert [(e.code, e.path) for e in empty.errors] == [
        ("event_key_empty", "/exclude_event_keys/0")
    ]

    narrow = Engine(limits=limits_with(max_occurrences=1))
    too_many = narrow.validate(with_updates(request, exclude_event_keys=["k1", "k2"]))
    assert [(e.code, e.path) for e in too_many.errors] == [
        ("exclude_event_keys_limit", "/exclude_event_keys")
    ]


def test_all_day_blocks_are_excluded_from_cap_counting_while_still_blocking() -> None:
    """An `all_day=True` block still blocks its span without consuming a weekly cap."""
    request = Request(
        now="2027-06-14T08:00Z",
        goal=single_goal(
            "30m",
            Requirement(all=[person("ana")]),
            zone="UTC",
            search_window=("2027-06-14T09:00Z", "2027-06-16T10:00Z"),
            candidates=["2027-06-14T09:00Z", "2027-06-15T09:00Z"],
            tags="interview",
        ),
        participants=[
            Participant(
                id="ana",
                zone="UTC",
                calendar=[
                    Block(
                        span=("2027-06-14T00:00Z", "2027-06-15T00:00Z"),
                        status="busy",
                        all_day=True,
                        tags=["interview"],
                        event_key="ooo-monday",
                    )
                ],
            )
        ],
        rules=[presets.cap_per_week({"tag": "interview"}, 1)],
    )
    assert [offer_start(o) for o in find_availability(request).offers] == ["2027-06-15T09:00Z"]
