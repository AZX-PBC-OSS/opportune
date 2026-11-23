"""The projection a public booking route serves: the bookable times, and nothing
about how they were found.

A :class:`~opportune.Response` answers an operator. Beside the offers it carries the
request's warnings, every candidate start that was not offered and why, and, for each
offer, who could have filled every term and who was chosen — the host's calendars,
the policies that shaped them and the other attendees. A public booking link shows a
visitor the times it can book, so a route serving one returns a projection of the
answer instead of the answer itself.

The projection is opt-in and costs an operator route nothing: ``Response`` and
``SlotCheck`` keep every field, and a route that serves an operator returns them
unchanged.

    from opportune import Engine, PublicResponse, Request, public_response

    engine = Engine()

    @app.post("/offers")
    def offers(request: Request) -> PublicResponse:
        return public_response(engine.find_availability(request))

Every field the projection keeps is one a visitor acts on — the offers, the dates
they fall on, whether the list was cut short, and the booking's validity — so a
visitor reads the same document shape an operator does, minus what is not theirs.
"""

from collections.abc import Mapping
from typing import Annotated

from pydantic import BaseModel, ConfigDict, Field, ValidationError

from opportune._base import OpportuneModel
from opportune._models import DateCount, Meta, Response, SlotCheck
from opportune.errors import rejection

__all__ = [
    "PublicOffer",
    "PublicOfferEvent",
    "PublicResponse",
    "PublicSlotCheck",
    "public_response",
    "public_slot_check",
]


class PublicOfferEvent(OpportuneModel):
    """One goal part's time in an offer, without who fills it."""

    model_config = ConfigDict(extra="forbid", populate_by_name=True)

    end: Annotated[str, Field(description="The booking's end.")]
    part: Annotated[str, Field(description="The goal part's ID.")]
    start: Annotated[str, Field(description="The booking's start.")]


class PublicOffer(OpportuneModel):
    """A bookable time: its parts, and what the visitor should know about it."""

    model_config = ConfigDict(extra="forbid", populate_by_name=True)

    events: Annotated[list[PublicOfferEvent], Field(description="One event per goal part.")]
    flags: Annotated[
        list[str],
        Field(
            description="What the visitor should know about the offer, such as a host "
            "whose calendar could not be read."
        ),
    ]


class PublicResponse(OpportuneModel):
    """The offers a public booking link serves, without the operator's own detail.

    The projection is closed where the engine's own ``Response`` is open: it is
    built field by field from an answer, so a key it does not name is a defect in
    this package rather than a field a newer engine added — and a field it has no
    name for is not one it could serve.
    """

    model_config = ConfigDict(extra="forbid", populate_by_name=True)

    dates: Annotated[
        list[DateCount],
        Field(description="Every civil date the search window touches, with its offer count."),
    ]
    meta: Annotated[Meta, Field(description="The versions the answer was produced from.")]
    offers: Annotated[list[PublicOffer], Field(description="The bookable times, in start order.")]
    truncated: Annotated[
        bool,
        Field(
            description="Whether the offer limit cut the list short, so a visitor knows to narrow "
            "the search rather than read the list as every time available."
        ),
    ]


class PublicSlotCheck(OpportuneModel):
    """The judgement of one proposed booking, without why it failed."""

    model_config = ConfigDict(extra="forbid", populate_by_name=True)

    flags: Annotated[
        list[str],
        Field(description="What the visitor should know that does not make it invalid."),
    ]
    meta: Annotated[Meta, Field(description="The versions the answer was produced from.")]
    valid: Annotated[bool, Field(description="Whether the booking can be made as proposed.")]


def _as_document[Model: BaseModel](value: object, model: type[Model], label: str) -> Model:
    """`value` as `model`, or the package's rejection naming what was wrong.

    A model instance passes through with no extra validation. A mapping — such as a
    document read back from a cache, queue or JSON column — is validated into
    `model`, whose fields must be the shape the engine writes: a ``dates`` given as
    text or an ``offers`` list that is missing is refused. The response models are
    *open*, because serde reads the same types without ``deny_unknown_fields``, so a
    field a newer engine added is tolerated here and then dropped — a stored answer
    written mid-roll parses rather than failing the deployment that reads it.
    Anything that is not a model or a mapping raises :class:`~opportune.OpportuneError`
    with code ``invalid_argument``, never a raw ``AttributeError``, ``TypeError`` or
    ``pydantic.ValidationError``.
    """
    if isinstance(value, model):
        return value
    if not isinstance(value, Mapping):
        raise rejection(
            "invalid_argument",
            f"`{label}` is neither a {model.__name__} nor a mapping: {type(value).__name__}",
        )
    try:
        return model.model_validate(value)
    except ValidationError as cause:
        finding = cause.errors(include_url=False)[0]
        where = "/".join(str(part) for part in finding["loc"]) or "the document"
        raise rejection(
            "invalid_argument",
            f"`{label}` is not a {model.__name__} the engine returns (`{where}`: {finding['msg']})",
        ) from cause


def public_response(response: Response | Mapping[str, object]) -> PublicResponse:
    """`response` with the operator-only fields left behind.

    The warnings the request produced, the excluded starts and their reasons, and
    each offer's eligible and assigned hosts are the operator's view of who and what
    shaped the answer; a public booking link serves the offers themselves.

    ``response`` may be a :class:`~opportune.Response` model or a mapping of its
    wire shape. The result is a model of its own rather than a stripped copy, so a
    route declaring it as its ``response_model`` serves a document whose schema
    states exactly the fields it carries.

    :raises OpportuneError: with code ``invalid_argument`` when ``response`` is not
        a ``Response`` or a valid response mapping.
    """
    validated = _as_document(response, Response, "response")
    return PublicResponse(
        dates=validated.dates,
        meta=validated.meta,
        offers=[
            PublicOffer(
                events=[
                    PublicOfferEvent(part=event.part, start=event.start, end=event.end)
                    for event in offer.events
                ],
                flags=offer.flags,
            )
            for offer in validated.offers
        ],
        truncated=validated.truncated,
    )


def public_slot_check(check: SlotCheck | Mapping[str, object]) -> PublicSlotCheck:
    """`check` with the operator-only fields left behind.

    Why a booking fails names the rules, the other participants' events and the
    reserve the operator's own booking needs, so a public confirmation serves the
    verdict itself: whether the booking can be made, and the flags that do not make
    it invalid.

    ``check`` may be a :class:`~opportune.SlotCheck` model or a mapping of its wire
    shape.

    :raises OpportuneError: with code ``invalid_argument`` when ``check`` is not a
        ``SlotCheck`` or a valid slot-check mapping.
    """
    validated = _as_document(check, SlotCheck, "check")
    return PublicSlotCheck(flags=validated.flags, meta=validated.meta, valid=validated.valid)
