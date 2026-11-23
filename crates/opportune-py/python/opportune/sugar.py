"""Single-part sugar: shortcuts that produce the general models.

Nothing here is a parallel type. ``single_goal`` builds a :class:`~opportune.Goal`
with one :class:`~opportune.Part`, the ``person``/``at_least`` helpers build the
requirement terms the engine matches on, and the ``offer_*`` accessors read a
returned :class:`~opportune.Offer` and rewrite it as the :class:`~opportune.Proposal`
``check_slot`` takes. The one-person, one-meeting case is about ten lines.

An offer is read as the model, or as the mapping a stored or received one is, so an
accessor never reads a field off a document that is not an offer. An offer with no
events is not something the engine returns, so the accessors that need one raise
:class:`~opportune.OpportuneError` rather than an ``IndexError`` or ``AttributeError``
a handler written for the package's own exception would miss.
"""

from collections.abc import Mapping, Sequence
from typing import Any, cast

from pydantic import ValidationError

from opportune._models import (
    AtLeastTerm,
    Goal,
    Grid,
    Offer,
    PersonTerm,
    Proposal,
    ProposalEvent,
    Requirement,
    ReserveSpan,
    Selector,
    Span,
    Travel,
)
from opportune.engine import SelectorInput
from opportune.errors import rejected_argument, rejection


def single_goal(
    duration: str,
    requirement: Requirement | Mapping[str, Any],
    *,
    zone: str,
    search_window: Span,
    display_zone: str | None = None,
    grid: Grid | Mapping[str, Any] | None = None,
    candidates: str | Sequence[str] | None = None,
    part_id: str | None = None,
    tags: str | Sequence[str] | None = None,
    travel: Travel | Mapping[str, Any] | None = None,
) -> Goal:
    """A goal with one part: the shortcut for the common single-meeting case.

    It builds exactly what ``Goal(parts=[Part(...)], ...)`` builds, so nothing is
    lost by using it. The part's ID defaults to ``main`` and the display zone to
    the goal's zone; ``part_id``, ``tags`` and ``travel`` reach the part, and
    ``display_zone``, ``grid`` and ``candidates`` the goal, so a real booking link
    or a provider's candidate list needs no second construction step.

    ``grid`` and ``travel`` are the generated models or the mapping a document
    states, which the goal's own validation reads into the form it names. ``tags``
    and ``candidates`` are a list or the one value a string spells; a mapping is
    neither, and its keys are not the values the caller meant.

    :raises OpportuneError: when the goal or one of its fields is not a value the
        generated models accept, whose findings stay on the ``__cause__``.
    """
    part: dict[str, object] = {"duration": duration, "requirement": requirement}
    if part_id is not None:
        part["id"] = part_id
    if tags is not None:
        part["tags"] = _one_or_more(tags, "tags")
    if travel is not None:
        part["travel"] = travel
    goal: dict[str, object] = {
        "parts": [part],
        "search_window": search_window,
        "zone": zone,
    }
    if display_zone is not None:
        goal["display_zone"] = display_zone
    if grid is not None:
        goal["grid"] = grid
    if candidates is not None:
        goal["candidates"] = _one_or_more(candidates, "candidates")
    try:
        return Goal.model_validate(goal)
    except ValidationError as cause:
        raise rejection("invalid_argument", rejected_argument(cause)) from cause


def _one_or_more(stated: str | Sequence[str], where: str) -> list[str]:
    """``stated`` as the list of values it spells.

    ``str`` satisfies ``Sequence[str]``, so a caller holding one tag or one instant
    as a string passes the annotation; reading it as a sequence of characters would
    build a goal nobody wrote. A mapping passes that annotation too, while iterating
    it would read its keys as the values: both are the package's rejection rather
    than a goal holding something the caller never named.
    """
    if isinstance(stated, str):
        return [stated]
    if isinstance(stated, Mapping):
        # A mapping is not a list of values, and iterating it would read its keys as
        # the values — which a caller who wrote `{"interview": True}` never meant.
        raise rejection(
            "invalid_argument",
            f"`{where}` is one value or a list of them, not a mapping",
        )
    try:
        return list(stated)
    except TypeError as cause:
        raise rejection(
            "invalid_argument",
            f"`{where}` is one value or a list of them, not a {type(stated).__name__}: {cause}",
        ) from cause


def person(id: str) -> PersonTerm:
    """A requirement term naming one participant.

    :raises OpportuneError: when ``id`` is not the text a participant is named by,
        rather than the model's ``pydantic.ValidationError``.
    """
    # Python callers are not type-checked, so the shape below is checked at runtime.
    if not isinstance(id, str):  # pyright: ignore[reportUnnecessaryIsInstance]
        raise rejection(
            "invalid_argument",
            f"`id` is a participant ID such as `ana`, not a {type(id).__name__}",
        )
    return PersonTerm(person=id)


def at_least(n: int, of: SelectorInput) -> AtLeastTerm:
    """A requirement term satisfied by any ``n`` participants ``of`` selects.

    :raises OpportuneError: when ``of`` is not a selector the models read, as the
        package's own rejection rather than as the model's
        ``pydantic.ValidationError``, whose findings stay on the ``__cause__``.
    """
    try:
        # `of` is the model, or the mapping a document states: the field's own
        # validation reads that mapping into the form it names.
        return AtLeastTerm(at_least=n, of=cast("Selector", of))
    except ValidationError as cause:
        raise rejection("invalid_argument", rejected_argument(cause)) from cause


def _as_offer(offer: Offer | Mapping[str, object]) -> Offer:
    """``offer`` as the model the accessors read, or the rejection an invalid one raises.

    An offer arrives as the engine's answer, and a caller who stored it in a JSON
    column or received it over HTTP holds the document it was written as; both are
    accepted. Reading a field off a malformed one is then the package's rejection
    rather than an ``AttributeError`` from this package's own code.

    :raises OpportuneError: with code ``invalid_argument`` naming the field of the
        offer that is at fault; the model's finding stays on the ``__cause__``.
    """
    try:
        return Offer.model_validate(offer)
    except ValidationError as cause:
        finding = cause.errors(include_url=False)[0]
        where = "/".join(str(part) for part in finding["loc"]) or "the document"
        raise rejection(
            "invalid_argument",
            f"`offer` is not an offer the engine returns (`{where}`: {finding['msg']})",
        ) from cause


def offer_start(offer: Offer | Mapping[str, object]) -> str:
    """The earliest start the offer books, as an RFC 3339 instant.

    :raises OpportuneError: when ``offer`` is not an offer the engine returns, or
        books no event, which no engine answer does.
    """
    booked = _as_offer(offer)
    if not booked.events:
        raise rejection("invalid_argument", "the offer books no events")
    return booked.events[0].start


def offer_end(offer: Offer | Mapping[str, object]) -> str:
    """The latest end the offer books, as an RFC 3339 instant.

    :raises OpportuneError: when ``offer`` is not an offer the engine returns, or
        books no event, which no engine answer does.
    """
    booked = _as_offer(offer)
    if not booked.events:
        raise rejection("invalid_argument", "the offer books no events")
    return booked.events[-1].end


def offer_assigned_hosts(offer: Offer | Mapping[str, object]) -> list[str]:
    """The participants assigned to the offer, in term order.

    :raises OpportuneError: when ``offer`` is not an offer the engine returns.
    """
    return [host for event in _as_offer(offer).events for term in event.assignment for host in term]


def offer_calendar_unknown(offer: Offer | Mapping[str, object]) -> bool:
    """Whether the offer rests on a host whose calendar was never supplied.

    :raises OpportuneError: when ``offer`` is not an offer the engine returns.
    """
    return "calendar_unknown" in _as_offer(offer).flags


def offer_reserve(offer: Offer | Mapping[str, object]) -> list[ReserveSpan]:
    """The spans a confirmed offer would hold on its participants' calendars.

    Travel is part of the booking the caller must write, so an offer's reserve is
    what to put on each participant's calendar beside the event itself; the spans
    come in the order the offer's events do.

    :raises OpportuneError: when ``offer`` is not an offer the engine returns.
    """
    return [span for event in _as_offer(offer).events for span in event.reserve]


def offer_to_proposal(offer: Offer | Mapping[str, object]) -> Proposal:
    """The offer as the proposal ``check_slot`` confirms.

    :raises OpportuneError: when ``offer`` is not an offer the engine returns.
    """
    return Proposal(
        events=[
            ProposalEvent(part=event.part, start=event.start, assignment=event.assignment)
            for event in _as_offer(offer).events
        ]
    )


__all__ = [
    "at_least",
    "offer_assigned_hosts",
    "offer_calendar_unknown",
    "offer_end",
    "offer_reserve",
    "offer_start",
    "offer_to_proposal",
    "person",
    "single_goal",
]
