"""Google Calendar payload conversions.

Call the provider like this, or the conversions see the wrong data:

- Query ``events.list`` with ``singleEvents=true``: an unexpanded series is
  reported as an ``unavailable`` outcome.
- Never set ``maxAttendees``. Google may then omit the user's own attendee entry,
  which is what ``self`` status and ``attendeesOmitted`` are read from.
- Batch ``freeBusy.query`` within Google's per-call calendar limit.

A per-calendar failure the provider reported (``calendars[id].errors``) is an
``unavailable`` outcome, never an empty block list, because an empty list would
read as "completely free". Blocks from free/busy data carry no event key and are
never all-day.
"""

from opportune._constants import MARKER_EVENT_KEY as MARKER_EVENT_KEY
from opportune._constants import MARKER_TAGS as MARKER_TAGS
from opportune._models import CalendarOutcome
from opportune.engine import Engine, Payload, bound_engine


def events_list_to_blocks(
    payload: Payload, participant_zone: str, *, engine: Engine | None = None
) -> CalendarOutcome:
    """Convert a Google ``events.list`` response into blocks or ``unavailable``.

    ``participant_zone`` is the calendar owner's IANA zone, read for all-day
    events when the response names no ``timeZone`` of its own. ``engine``
    defaults to the shared engine.
    """
    return bound_engine(engine).events_list_to_blocks(payload, participant_zone)


def free_busy_to_blocks(
    payload: Payload, *, engine: Engine | None = None
) -> dict[str, CalendarOutcome]:
    """Convert a Google ``freeBusy.query`` response into outcomes keyed by calendar ID.

    A calendar the provider reported an error for is an ``unavailable`` outcome;
    the healthy calendars are still converted.
    """
    return bound_engine(engine).free_busy_to_blocks(payload)


__all__ = [
    "MARKER_EVENT_KEY",
    "MARKER_TAGS",
    "events_list_to_blocks",
    "free_busy_to_blocks",
]
