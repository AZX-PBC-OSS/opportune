"""Microsoft Graph / Entra Outlook payload conversions.

Call the provider like this, or the conversions see the wrong data:

- Read ``calendarView``, not ``/events``: recurrence expansion is the caller's
  job, and an unexpanded series is reported as an ``unavailable`` outcome.
- Send ``Prefer: outlook.timezone="UTC"``. Graph ``dateTimeTimeZone`` values carry
  no offset, so a payload stamped in another zone is read in that zone's local
  time — an all-day event is read from its civil dates in the participant's zone
  whatever the payload stamps on it.
- ``$expand`` the marker properties with :data:`EXTENDED_PROPERTY_EXPAND`, or the
  caller's own tags and event keys are absent.
- Chunk ``getSchedule`` to at most 20 schedules and 62 days per call.

A per-calendar failure the provider reported (``scheduleInformation.error``) is
an ``unavailable`` outcome, never an empty block list, because an empty list
would read as "completely free".
"""

from collections.abc import Mapping

import opportune._opportune as _native
from opportune._constants import MARKER_EVENT_KEY as MARKER_EVENT_KEY
from opportune._constants import MARKER_TAGS as MARKER_TAGS
from opportune._models import (
    CalendarOutcome,
    GraphWorkingHoursOutcome,
)
from opportune.engine import Engine, Payload, SelectorInput, bound_engine

EXTENDED_PROPERTY_NAMESPACE_GUID = _native.GRAPH_EXTENDED_PROPERTY_NAMESPACE_GUID
"""The GUID of opportune's permanent Graph extended-property set."""

EXTENDED_PROPERTY_TAGS_ID = _native.GRAPH_EXTENDED_PROPERTY_TAGS_ID
"""The Graph extended-property ID that carries :data:`MARKER_TAGS`."""

EXTENDED_PROPERTY_EVENT_KEY_ID = _native.GRAPH_EXTENDED_PROPERTY_EVENT_KEY_ID
"""The Graph extended-property ID that carries :data:`MARKER_EVENT_KEY`."""

EXTENDED_PROPERTY_EXPAND = _native.GRAPH_EXTENDED_PROPERTY_EXPAND
"""The Graph ``$expand`` value that fetches both markers with an event."""


def calendar_view_to_blocks(
    payload: Payload,
    participant_zone: str,
    category_map: Mapping[str, str] | None = None,
    *,
    engine: Engine | None = None,
) -> CalendarOutcome:
    """Convert a Graph ``calendarView`` response into blocks or ``unavailable``.

    ``participant_zone`` is the mailbox owner's IANA zone, which all-day events
    are resolved in. ``category_map`` maps a category name to a tag path; an
    unmapped category becomes ``category/<slug>``, and one with an empty slug is
    dropped. ``engine`` defaults to the shared engine.
    """
    return bound_engine(engine).calendar_view_to_blocks(payload, participant_zone, category_map)


def get_schedule_to_blocks(
    payload: Payload, *, engine: Engine | None = None
) -> dict[str, CalendarOutcome]:
    """Convert a Graph ``getSchedule`` response into outcomes keyed by schedule ID.

    A schedule the provider reported an error for is an ``unavailable`` outcome;
    the healthy schedules in the same batch are still converted. Blocks from free
    or busy data carry no event key and are never all-day.
    """
    return bound_engine(engine).get_schedule_to_blocks(payload)


def working_hours_to_rule(
    payload: Payload,
    rule_id: str = "working_hours",
    *,
    who: SelectorInput | None = None,
    when: SelectorInput | None = None,
    engine: Engine | None = None,
) -> GraphWorkingHoursOutcome:
    """Convert one mailbox's Graph ``workingHours`` into a ``window`` rule.

    ``rule_id`` is the ID the rule takes; ``who`` and ``when`` replace the rule's
    default selectors, and ``engine`` defaults to the shared engine. A custom Graph
    time zone is an ``unavailable`` outcome, so a caller falls back to its own window
    without catching an exception, and the blocks converted from the same schedule
    response are unaffected.

    :raises OpportuneError: when ``who`` or ``when`` is not a selector the rule
        accepts, as the package's own rejection rather than as the model's
        ``pydantic.ValidationError``, whose findings stay on the ``__cause__``.
    """
    return bound_engine(engine).working_hours_to_rule(payload, rule_id, who=who, when=when)


__all__ = [
    "EXTENDED_PROPERTY_EVENT_KEY_ID",
    "EXTENDED_PROPERTY_EXPAND",
    "EXTENDED_PROPERTY_NAMESPACE_GUID",
    "EXTENDED_PROPERTY_TAGS_ID",
    "MARKER_EVENT_KEY",
    "MARKER_TAGS",
    "calendar_view_to_blocks",
    "get_schedule_to_blocks",
    "working_hours_to_rule",
]
