"""Presets that build the general models, adding no semantics of their own.

Each preset generates its rule ID from its own name, so a rule always has an ID
to cite; ``available_only`` derives its ID from the attendee, so two attendees'
stated availability compose in one request. Rule IDs are unique in a request, so
two presets of the same kind need distinct ``rule_id`` values.

Every preset returns the generated model the engine reads — nothing parallel to
it — so its output serialises to the wire format, persists as a JSON document, and
plugs straight into a FastAPI route schema.

``unless`` and its participant-side twins ``and_who`` and ``except_who`` are the
narrowing helpers: each takes a rule a preset built and returns the same rule with
one selector intersected, so a broad tier can be loosened for a narrow exception —
or narrowed to a class of participants — without hand-building a selector and
without replacing the default ``host`` clause.

Every preset except ``available_only`` — whose ``who`` is the attendee it
returns — takes ``who`` and ``when``, which replace the rule's default
selectors: ``who`` says which participants it constrains and ``when`` which
bookings it applies to. Both are validated by the model the rule is made of, so a
selector the engine would refuse is refused here.

A value the generated models refuse raises :class:`~opportune.OpportuneError` with
code ``invalid_argument`` — the same exception every other boundary raises —
rather than the Pydantic ``ValidationError`` the models raise among themselves.
"""

import functools
import re
from collections.abc import Callable, Mapping, Sequence
from datetime import UTC, date, datetime, timedelta
from typing import Any, cast

from pydantic import BaseModel, ValidationError

from opportune._models import (
    AllSelector,
    CapMeasure,
    CapParams,
    CapPer,
    CapPeriod,
    CapRule,
    ClearanceParams,
    ClearanceRule,
    DateMax,
    DaysMax,
    DurationMax,
    IdsSelector,
    LeadTimeMax,
    LeadTimeParams,
    LeadTimeRule,
    NotSelector,
    Participant,
    RestParams,
    RestRule,
    RoleSelector,
    Rule,
    Selector,
    Span,
    StatusPolicyEntry,
    StatusPolicyParams,
    StatusPolicyRule,
    WindowParams,
    WindowRule,
)
from opportune.engine import SelectorInput, with_updates
from opportune.errors import rejected_argument, rejection

_UTC_DAY_MINUTES = 24 * 60

_EPOCH = datetime(1970, 1, 1, tzinfo=UTC)
"""Midnight UTC on 1970-01-01: the origin the whole-minute arithmetic counts from."""

_MAX_DATED_ENTRIES = 3660
"""The most dated entries one stated availability may need: ten years of UTC days.

A stated availability is a handful of hours an external attendee is reachable.
The ceiling is a guard, not a limit anyone should meet: the preset writes one
entry per UTC day of every span, so an availability without a ceiling turns one
bad argument into millions of dict entries written while the GIL is held.
"""

_WALL_RANGE = re.compile(
    r"(?:[01][0-9]|2[0-3]):[0-5][0-9]-(?:(?:[01][0-9]|2[0-3]):[0-5][0-9]|24:00)"
)
"""The wire spelling of one wall-clock range, such as `09:00-17:00` or `23:30-24:00`."""

_SUPPORTED_FROM = datetime(1975, 1, 1, tzinfo=UTC)
_SUPPORTED_TO = datetime(2100, 1, 1, tzinfo=UTC)
"""The earliest and latest instants a stated availability may name, inclusive both
ends: the range every `Instant` in the engine holds (`Instant::MIN`/`Instant::MAX`).

A span outside it has no dated window the engine could read, so it is refused before
any date key is written rather than written as a window over a date the engine
silently never matches."""

_SUPPORTED_RANGE = "1975-01-01T00:00Z..=2100-01-01T00:00Z"
"""The range above as a message names it, which is the engine's own wording."""

_DURATION = re.compile(r"\d+h(?:\d+m)?|\d+m")
"""A duration written in hours and minutes, such as `30m` or `1h30m`.

A day is not an exact length, so a shorthand that names one — `14d` — is not a
duration the engine reads and is refused here rather than at the next call.
"""

_CIVIL_DATE = re.compile(r"\d{4}-\d{2}-\d{2}")
"""A civil date written `YYYY-MM-DD`."""

_INSTANT = re.compile(
    r"\d{4}-\d{2}-\d{2}[Tt](?:[01]\d|2[0-3]):[0-5]\d"
    r"(?::([0-5]\d(?:\.\d{1,9})?))?"
    r"(?:[Zz]|[+-](?:[01]\d|2[0-3]):[0-5]\d)"
)
"""The wire spelling of an RFC 3339 instant: a date, a clock and an explicit offset."""

_WHOLE_MINUTE = re.compile(r"00(?:\.0+)?")
"""Seconds that add nothing to the minute: `00` or `00` followed by zeros."""


def _arguments_checked[**Parameters, Result](
    preset: Callable[Parameters, Result],
) -> Callable[Parameters, Result]:
    """Reject a preset value the generated models would refuse as the package's error.

    A preset builds generated models, so a value outside the schema — a negative
    cap limit, a selector the wire does not read — is a Pydantic
    ``ValidationError``. A caller of a preset gets the ``OpportuneError`` every
    other boundary raises instead, so one exception handler covers the package.
    """

    @functools.wraps(preset)
    def checked(*args: Parameters.args, **kwargs: Parameters.kwargs) -> Result:
        try:
            return preset(*args, **kwargs)
        except ValidationError as cause:
            raise rejection("invalid_argument", rejected_argument(cause)) from cause

    return checked


def _scoped[Rule: BaseModel](
    rule: Rule, who: SelectorInput | None, when: SelectorInput | None
) -> Rule:
    """``rule`` with ``who`` and ``when`` replaced where the caller named them."""
    if who is None and when is None:
        return rule
    changes: dict[str, object] = {}
    if who is not None:
        changes["who"] = who
    if when is not None:
        changes["when"] = when
    return with_updates(rule, **changes)


def _is_wall_range(value: object) -> bool:
    """Whether `value` is one wire wall-clock range a caller could have written."""
    return isinstance(value, str) and _WALL_RANGE.fullmatch(value) is not None


def _wall_ranges(stated: str | Sequence[str], where: str) -> list[str]:
    """The wall ranges `stated` names, rejecting a value that could never be one.

    ``where`` names the argument in the message, so a caller reads which value of
    a mapping was wrong rather than that some value was.
    """
    if isinstance(stated, str):
        ranges = [stated]
    elif isinstance(stated, Mapping):
        # A mapping is not a list of ranges, and iterating it would read its keys as
        # the ranges — which a caller who wrote `{"09:00-17:00": "x"}` never meant.
        raise rejection(
            "invalid_argument",
            f"{where} is one wall-clock range such as `09:00-17:00`, or a list of "
            "them, not a mapping",
        )
    else:
        try:
            ranges = list(stated)
        except TypeError as cause:
            raise rejection(
                "invalid_argument",
                f"{where} is one wall-clock range such as `09:00-17:00`, or a list of "
                f"them, not a {type(stated).__name__}: {cause}",
            ) from cause
    for wall_range in ranges:
        if not _is_wall_range(wall_range):
            raise rejection(
                "invalid_argument",
                f"{where} is one wall-clock range such as `09:00-17:00`, or a list of "
                f"them, not `{wall_range}`",
            )
    return ranges


def _lead_time_max(maximum: LeadTimeMax | Mapping[str, Any] | str | int) -> LeadTimeMax:
    """The typed lead-time maximum a shorthand names.

    A ``str`` is read as the duration or civil date it spells, and an ``int`` as a
    calendar-day count — the two shorthands a caller writes most often — so a
    typed model is one of several accepted forms rather than the only one. No
    value at all is not one of them: a ``horizon`` always bounds the start, and the
    rule's optional ``max`` is what would otherwise silently enforce nothing.
    """
    if isinstance(maximum, str):
        if _CIVIL_DATE.fullmatch(maximum):
            try:
                date.fromisoformat(maximum)
            except ValueError as cause:
                raise rejection(
                    "invalid_argument", f"`{maximum}` is not a civil date: {cause}"
                ) from cause
            return DateMax(date=maximum)
        if _DURATION.fullmatch(maximum):
            return DurationMax(duration=maximum)
        raise rejection(
            "invalid_argument",
            f"`{maximum}` is not a duration such as `4h` or `1h30m`, a date such as "
            "`2027-07-01`, or a number of days",
        )
    # Python callers are not type-checked, so the absent value is checked here.
    if maximum is None:  # pyright: ignore[reportUnnecessaryComparison]
        raise rejection("invalid_argument", "`None` is not a lead-time maximum")
    if isinstance(maximum, bool):
        raise rejection("invalid_argument", f"`{maximum}` is not a lead-time maximum")
    if isinstance(maximum, int):
        try:
            return DaysMax(days=maximum)
        except ValidationError as cause:
            # The model owns the bound; naming the value keeps the rejection about the
            # argument the caller wrote rather than the `days` field the shorthand
            # became, which is the JS preset's answer to the same input.
            raise rejection(
                "invalid_argument", f"`{maximum}` is not a number of calendar days"
            ) from cause
    return cast("LeadTimeMax", maximum)


@_arguments_checked
def business_hours(
    days: str = "mon-fri",
    hours: str | Sequence[str] = "09:00-17:00",
    *,
    zone: str = "participant",
    rule_id: str = "business_hours",
    who: SelectorInput | None = None,
    when: SelectorInput | None = None,
) -> WindowRule:
    """A ``window`` rule open ``hours`` on ``days``, read in ``zone``.

    ``days`` is a wire day key such as ``mon-fri`` or ``mon,wed``; ``hours`` is one
    wire wall range such as ``09:00-17:00`` or a list of them for a day split into
    several ranges. ``zone`` is ``participant``, ``event`` or an IANA name.

    :raises OpportuneError: when ``days`` is not a string, ``hours`` is a mapping
        or anything else that is neither a string nor a sequence of them, or an
        entry of ``hours`` is not a wall range. The engine still decides what a
        range or a day key means; this catches the shapes that could never be one.
    """
    # Python callers are not type-checked, so both shapes below are checked at runtime.
    if not isinstance(days, str):  # pyright: ignore[reportUnnecessaryIsInstance]
        raise rejection(
            "invalid_argument",
            f"`days` is a wire day key such as `mon-fri`, not a {type(days).__name__}",
        )
    ranges = _wall_ranges(hours, "`hours`")
    return _scoped(
        WindowRule(
            id=rule_id, kind="window", params=WindowParams(zone=zone, weekly={days: ranges})
        ),
        who,
        when,
    )


@_arguments_checked
def dated_hours(
    dates: Mapping[str, str | Sequence[str]],
    *,
    zone: str = "event",
    rule_id: str = "dated_hours",
    who: SelectorInput | None = None,
    when: SelectorInput | None = None,
) -> WindowRule:
    """A ``window`` rule open on ``dates`` only, read in ``zone``.

    ``dates`` maps a wire date key — ``2027-06-14`` or a range such as
    ``2027-06-14..2027-06-16`` — to the hours it has, so this is the conference
    booth shape: open on the days the event runs, and on no other. ``zone`` is
    ``event``, ``participant`` or an IANA name.

    The dates are the whole window rather than an override on top of weekly hours:
    a date no key covers is closed.

    :raises OpportuneError: when ``dates`` is not a mapping, a key is not a string,
        or a value is not a wall range or a list of them. The engine still decides
        what a date key and a range mean; this catches the shapes that could never
        be one.
    """
    return _scoped(
        WindowRule(
            id=rule_id,
            kind="window",
            params=WindowParams(zone=zone, dates=_hours_by_key(dates, "dates")),
        ),
        who,
        when,
    )


@_arguments_checked
def weekly_hours(
    weekly: Mapping[str, str | Sequence[str]],
    *,
    dates: Mapping[str, str | Sequence[str]] | None = None,
    zone: str = "participant",
    rule_id: str = "weekly_hours",
    who: SelectorInput | None = None,
    when: SelectorInput | None = None,
) -> WindowRule:
    """A ``window`` rule open on each weekday's own hours, read in ``zone``.

    ``weekly`` maps a wire day key — ``mon``, ``mon,wed`` or ``mon-fri`` — to the
    hours those days share: one wall range such as ``09:00-17:00``, or a list of
    them for a day split around lunch. A day no key covers is closed, so this is
    one rule for the ordinary week rather than one ``business_hours`` rule per day
    — and it is a window, so it gets no clearance and never triggers rest.

    ``dates`` overrides the weekly hours for the dates it names, keyed like
    :func:`dated_hours`; a date mapped to an empty list is closed, which is how a
    holiday is written. ``zone`` is ``participant``, ``event`` or an IANA name.

    :raises OpportuneError: when ``weekly`` or ``dates`` is not a mapping, a key is
        not a string, or a value is not a wall range or a list of them. An
        overlapping pair of ranges is the engine's rejection, with the pointer of
        the key that holds it.
    """
    params = WindowParams(
        zone=zone,
        weekly=_hours_by_key(weekly, "weekly"),
        dates={} if dates is None else _hours_by_key(dates, "dates"),
    )
    return _scoped(WindowRule(id=rule_id, kind="window", params=params), who, when)


@_arguments_checked
def buffers(
    before: str = "10m",
    after: str = "10m",
    *,
    min_gap: str = "0m",
    rule_id: str = "buffers",
    who: SelectorInput | None = None,
    when: SelectorInput | None = None,
) -> ClearanceRule:
    """A ``clearance`` rule: the gaps a booking needs before and after it.

    ``min_gap`` is the least gap between any two events sharing a participant;
    ``before`` and ``after`` widen only the events the booking sits beside, so the
    largest gap that applies is the one required.
    """
    return _scoped(
        ClearanceRule(
            id=rule_id,
            kind="clearance",
            params=ClearanceParams(before=before, after=after, min_gap=min_gap),
        ),
        who,
        when,
    )


@_arguments_checked
def min_notice(
    notice: str = "4h",
    *,
    rule_id: str = "min_notice",
    who: SelectorInput | None = None,
    when: SelectorInput | None = None,
) -> LeadTimeRule:
    """A ``lead_time`` rule carrying only a minimum notice."""
    return _scoped(
        LeadTimeRule(id=rule_id, kind="lead_time", params=LeadTimeParams(min=notice)),
        who,
        when,
    )


@_arguments_checked
def horizon(
    max: LeadTimeMax | Mapping[str, Any] | str | int,
    *,
    rule_id: str = "horizon",
    who: SelectorInput | None = None,
    when: SelectorInput | None = None,
) -> LeadTimeRule:
    """A ``lead_time`` rule carrying only a latest start.

    ``max`` is one of the generated forms — a duration, a date, a number of days,
    or a number of working days in a calendar — a mapping of one of those forms,
    or the shorthand for the first three: a duration or date string, such as
    ``"4h"`` or ``"2027-07-01"``, or a calendar-day count as an ``int``.

    :raises OpportuneError: when ``max`` is not one of the accepted forms — a
        string that is neither a duration nor a date in the engine's grammar, such
        as ``"14d"``, or no value at all. A day is 23 to 25 hours in a named zone,
        so it is never an exact duration. Pass
        :class:`~opportune.DaysMax` instead when calendar days are meant.
    """
    return _scoped(
        LeadTimeRule(
            id=rule_id,
            kind="lead_time",
            params=LeadTimeParams(min="0m", max=_lead_time_max(max)),
        ),
        who,
        when,
    )


@_arguments_checked
def rest(
    late_after: str = "18:00",
    not_before: str = "10:00",
    *,
    rule_id: str = "rest",
    who: SelectorInput | None = None,
    when: SelectorInput | None = None,
) -> RestRule:
    """A ``rest`` rule: no booking before ``not_before`` when anything runs at or
    after ``late_after`` the day before."""
    return _scoped(
        RestRule(
            id=rule_id,
            kind="rest",
            params=RestParams(late_after=late_after, not_before=not_before),
        ),
        who,
        when,
    )


@_arguments_checked
def cap_per_day(
    count: SelectorInput,
    limit: int,
    *,
    measure: CapMeasure = "count",
    per: CapPer = "each",
    zone: str = "participant",
    rule_id: str = "cap_per_day",
    who: SelectorInput | None = None,
    when: SelectorInput | None = None,
) -> CapRule:
    """A ``cap`` rule of at most ``limit`` per civil day.

    ``count`` selects what is counted and ``measure`` says how: ``"count"`` counts
    the events, ``"minutes"`` sums their length. ``per="each"`` limits each
    participant in the rule's ``who``; ``per="pooled"`` limits them together, and
    then ``zone`` must be ``event`` or an IANA name.

    :raises OpportuneError: when a pooled cap is left in the participant's own
        zone; see :func:`_cap`.
    """
    return _cap(
        count,
        limit,
        "day",
        measure=measure,
        per=per,
        zone=zone,
        rule_id=rule_id,
        who=who,
        when=when,
    )


@_arguments_checked
def cap_per_week(
    count: SelectorInput,
    limit: int,
    *,
    measure: CapMeasure = "count",
    per: CapPer = "each",
    zone: str = "participant",
    week_starts: str = "mon",
    rule_id: str = "cap_per_week",
    who: SelectorInput | None = None,
    when: SelectorInput | None = None,
) -> CapRule:
    """A ``cap`` rule of at most ``limit`` per week.

    A week starts on ``week_starts`` and runs seven civil days. ``measure``,
    ``per`` and ``zone`` mean what they mean in :func:`cap_per_day`.

    :raises OpportuneError: when a pooled cap is left in the participant's own
        zone; see :func:`_cap`.
    """
    return _cap(
        count,
        limit,
        "week",
        measure=measure,
        per=per,
        zone=zone,
        week_starts=week_starts,
        rule_id=rule_id,
        who=who,
        when=when,
    )


@_arguments_checked
def cap_per_month(
    count: SelectorInput,
    limit: int,
    *,
    measure: CapMeasure = "count",
    per: CapPer = "each",
    zone: str = "participant",
    rule_id: str = "cap_per_month",
    who: SelectorInput | None = None,
    when: SelectorInput | None = None,
) -> CapRule:
    """A ``cap`` rule of at most ``limit`` per civil month.

    ``measure``, ``per`` and ``zone`` mean what they mean in :func:`cap_per_day`.

    :raises OpportuneError: when a pooled cap is left in the participant's own
        zone; see :func:`_cap`.
    """
    return _cap(
        count,
        limit,
        "month",
        measure=measure,
        per=per,
        zone=zone,
        rule_id=rule_id,
        who=who,
        when=when,
    )


@_arguments_checked
def status_policy(
    entries: Sequence[StatusPolicyEntry | Mapping[str, Any]],
    *,
    rule_id: str = "status_policy",
    who: SelectorInput | None = None,
    when: SelectorInput | None = None,
) -> StatusPolicyRule:
    """A ``status_policy`` rule: entries tried in order to decide which blocks block.

    Entries are evaluated in the order given across applicable policies, so a
    narrower policy belongs before a broader one. Each entry may be a
    :class:`~opportune.StatusPolicyEntry` or its wire mapping.

    ``entries`` is a :class:`~collections.abc.Sequence` because the order is the
    meaning: a set, a ``dict``'s values or any other unordered (or one-shot) iterable
    would decide the order for the caller, so it is refused here rather than silently
    producing a policy whose precedence nobody wrote.

    :raises OpportuneError: when ``entries`` is not a sequence of entries, or an
        entry or selector is refused by the models.
    """
    if isinstance(entries, (str, bytes, Mapping, set, frozenset)):
        raise rejection(
            "invalid_argument",
            f"`entries` is a sequence of status-policy entries, not a {type(entries).__name__}",
        )
    if not isinstance(entries, Sequence):  # pyright: ignore[reportUnnecessaryIsInstance]
        raise rejection(
            "invalid_argument",
            f"`entries` is a sequence of status-policy entries, not a "
            f"{type(entries).__name__}: the order entries are written in is the order "
            "they apply in, so only an ordered sequence can state it",
        )
    try:
        stated = list(entries)
    except TypeError as cause:
        raise rejection(
            "invalid_argument",
            f"`entries` is a sequence of status-policy entries, not a "
            f"{type(entries).__name__}: {cause}",
        ) from cause
    return _scoped(
        StatusPolicyRule(
            id=rule_id,
            kind="status_policy",
            params=StatusPolicyParams(entries=cast("list[StatusPolicyEntry]", stated)),
        ),
        who,
        when,
    )


@_arguments_checked
def unless(rule: Rule, selector: SelectorInput) -> Rule:
    """``rule`` with ``selector`` excepted from the bookings it applies to.

    This is how a broad tier is loosened for a narrow one. A rule whose ``when`` is
    unconditional becomes ``{"not": selector}``; one that is already scoped keeps
    its scope and adds the exception, as ``{"all": [existing, {"not": selector}]}``.

    Excluding the exception from the broad rule is what makes a hierarchy work: a
    ``window`` rule intersected with a second window leaves only the hours the two
    share, so an exception written as another window on the narrow tier could only
    narrow the booking further. A tier that should apply to everything *but* the
    exception excludes it here instead::

        hours = presets.unless(
            presets.business_hours("mon-fri", "09:00-17:00"), {"tag": "vip"}
        )

    ``selector`` is a :data:`SelectorInput`, so a mapping such as
    ``{"tag": "vip"}`` and a typed selector are the same argument.

    :raises OpportuneError: when ``rule`` is not a rule model, or ``selector`` is not
        a selector the model accepts.
    """
    # Python callers are not type-checked, so the model below is checked at runtime.
    # Every rule model carries `when`, and no other generated model does, so the one
    # field this function writes is also what says the argument is a rule at all.
    if not isinstance(rule, BaseModel) or "when" not in type(rule).model_fields:  # pyright: ignore[reportUnnecessaryIsInstance]
        raise rejection(
            "invalid_argument",
            f"`rule` is a rule the presets build, not a {type(rule).__name__}",
        )
    excluded = NotSelector(not_=cast("Selector", selector))
    when = rule.when
    return with_updates(
        rule,
        when=excluded if when is None or when is True else AllSelector(all=[when, excluded]),
    )


def _host_default() -> Selector:
    """The wire's default ``who``: every host.

    A rule read back from storage may carry the explicit ``null`` that the wire
    reads as the same default, so a narrowing call starts from the clause the
    engine applies rather than from a missing one: intersecting with nothing would
    hand the rule to every participant, attendees included.
    """
    return RoleSelector(role="host")


def _rule_with_who(rule: object) -> Rule:
    """``rule`` as the rule model it is, or the package's rejection.

    Every rule model carries ``who``, and no other generated model does, so the one
    field these helpers write is also what says the argument is a rule at all.
    """
    # Python callers are not type-checked, so the model is checked at runtime.
    if not isinstance(rule, BaseModel) or "who" not in type(rule).model_fields:  # pyright: ignore[reportUnnecessaryIsInstance]
        raise rejection(
            "invalid_argument",
            f"`rule` is a rule the presets build, not a {type(rule).__name__}",
        )
    return cast("Rule", rule)


@_arguments_checked
def and_who(rule: Rule, selector: SelectorInput) -> Rule:
    """``rule`` with ``who`` narrowed to the participants ``selector`` also selects.

    This is the participant-side twin of :func:`unless`: it **intersects** with the
    rule's existing ``who`` instead of replacing it, so a rule that already applies
    to every host keeps applying to hosts and only to those hosts ``selector``
    picks out. That distinction is the point of the helper: replacing ``who``
    outright with ``{"kind": "person"}`` reaches every person, attendees included,
    while narrowing keeps the default ``{"role": "host"}`` clause beside the new
    one, so a host rule never starts judging an attendee's calendar::

        hours = presets.and_who(
            presets.business_hours("mon-fri", "09:00-17:00"), {"kind": "person"}
        )

    An unconditional ``who`` — the model's ``True``, or the ``null`` a document may
    write — becomes ``selector``; an ``all`` gains the selector as one more clause,
    so chained calls stay flat rather than nesting an ``all`` in an ``all``;
    anything else becomes ``all([existing, selector])``.

    ``selector`` is a :data:`SelectorInput`, so a mapping such as
    ``{"ids": ["ana"]}`` and a typed selector are the same argument.

    :raises OpportuneError: when ``rule`` is not a rule model, or ``selector`` is not
        a selector the model accepts.
    """
    model = _rule_with_who(rule)
    existing = _host_default() if model.who is None else model.who
    narrowed: Selector
    if existing is True:
        narrowed = cast("Selector", selector)
    elif isinstance(existing, AllSelector):
        narrowed = AllSelector(all=[*existing.all, cast("Selector", selector)])
    else:
        narrowed = AllSelector(all=[existing, cast("Selector", selector)])
    return with_updates(model, who=narrowed)


@_arguments_checked
def except_who(rule: Rule, selector: SelectorInput) -> Rule:
    """``rule`` with the participants ``selector`` selects excepted from ``who``.

    The participant-side twin of :func:`unless`, written in terms of
    :func:`and_who`: it narrows ``who`` by ``not(selector)``, so the rule keeps
    applying to every host but the excepted ones — and, because the intersection
    keeps the default ``{"role": "host"}`` clause, it never reaches an attendee::

        cap = presets.except_who(
            presets.cap_per_day({"tag": "interview"}, 2), {"ids": ["ceo"]}
        )

    :raises OpportuneError: when ``rule`` is not a rule model, or ``selector`` is not
        a selector the model accepts.
    """
    return and_who(rule, NotSelector(not_=cast("Selector", selector)))


@_arguments_checked
def available_only(
    id: str,
    spans: Sequence[Span],
    *,
    rule_id: str | None = None,
) -> tuple[Participant, WindowRule]:
    """Stated availability for an external attendee: the attendee, and the
    ``window`` rule that makes exactly those UTC instants available to them.

    The spans are coalesced first and then written as dated ranges split at UTC
    midnight, with a span ending at midnight written as ending ``24:00``. A window
    rather than blocks is deliberate: windows get no clearance and never trigger
    rest, so stated availability cannot be mistaken for a meeting, and an excluded
    start explains itself as ``outside_window`` citing this rule's ID.

    The rule's ID is ``available_only:{id}`` unless ``rule_id`` names another, so
    two attendees' availability compose in one request without colliding on an ID.

    :raises OpportuneError: when ``spans`` is not a sequence of pairs, a span is
        not a pair of RFC 3339 instants, is not aligned to a whole minute, names
        an instant outside ``1975-01-01T00:00Z..=2100-01-01T00:00Z`` — the range
        every instant the engine holds, checked before any date key is written —
        ends at or before it starts, or needs more than ten years of dated
        entries, one per UTC day of every span. A stated availability is a few
        hours of reachability, not three million days of it.
    """
    attendee = Participant(id=id, role="attendee", zone="UTC")
    rule = WindowRule(
        id=f"available_only:{id}" if rule_id is None else rule_id,
        kind="window",
        params=WindowParams(zone="UTC", dates=_utc_dates(spans)),
        who=IdsSelector(ids=[id]),
    )
    return attendee, rule


def _hours_by_key(stated: Mapping[str, str | Sequence[str]], where: str) -> dict[str, list[str]]:
    """The hours each key of a window mapping states, or the rejection one that
    could never be a mapping of ranges raises.

    ``where`` names the argument in the message, so a caller reads which value of a
    mapping was wrong rather than that some value was. ``dated_hours`` and
    ``weekly_hours`` are the same mapping read into the two fields of a window, so
    both normalise it here.
    """
    # Python callers are not type-checked, so the shape below is checked at runtime.
    if not isinstance(stated, Mapping):  # pyright: ignore[reportUnnecessaryIsInstance]
        raise rejection(
            "invalid_argument",
            f"`{where}` is a mapping of keys to hours, not a {type(stated).__name__}",
        )
    hours: dict[str, list[str]] = {}
    for key, value in stated.items():
        if not isinstance(key, str):  # pyright: ignore[reportUnnecessaryIsInstance]
            raise rejection(
                "invalid_argument",
                f"a `{where}` key is a weekday or date key such as `mon` or "
                f"`2027-06-14`, not `{key}`",
            )
        hours[key] = _wall_ranges(value, f"`{where}[{key}]`")
    return hours


def _cap(
    count: SelectorInput,
    limit: int,
    period: CapPeriod,
    *,
    measure: CapMeasure,
    per: CapPer,
    zone: str,
    week_starts: str = "mon",
    rule_id: str,
    who: SelectorInput | None = None,
    when: SelectorInput | None = None,
) -> CapRule:
    if per == "pooled" and zone == "participant":
        raise rejection(
            "invalid_argument",
            f"`{rule_id}` pools its cap, so `zone` must be `event` or an IANA name — "
            "the zone the cap is counted in — not the default `participant`: a pooled "
            "cap counts the participants together, and each one's own zone would "
            "count the same event against a different day",
        )
    return _scoped(
        CapRule(
            id=rule_id,
            kind="cap",
            params=CapParams(
                # `count` is the model, or the mapping a document states: the field's own
                # validation reads that mapping into the form it names.
                count=cast("Selector", count),
                limit=limit,
                measure=measure,
                per=per,
                period=period,
                zone=zone,
                week_starts=week_starts,
            ),
        ),
        who,
        when,
    )


def _utc_dates(spans: Sequence[Span]) -> dict[str, list[str]]:
    """The spans as dated wall ranges in UTC, coalesced and split at midnight.

    Every endpoint is a whole minute counted from the epoch, so the date of each
    midnight boundary is integer arithmetic rather than a float timestamp; the
    ceiling on the dated entries bounds the day loop whatever the caller states,
    and ``_utc_minute`` bounds every endpoint to a year this writes.
    """
    ranges: list[tuple[int, int]] = []
    try:
        stated = list(spans)
    except TypeError as cause:
        raise rejection(
            "invalid_argument",
            "`spans` is a sequence of RFC 3339 instant pairs, not a "
            f"{type(spans).__name__}: {cause}",
        ) from cause
    for span in stated:
        try:
            first, last = span
        except (TypeError, ValueError) as cause:
            raise rejection(
                "invalid_argument",
                f"a stated span is a pair of RFC 3339 instants, not `{span}`: {cause}",
            ) from cause
        start = _utc_minute(first)
        end = _utc_minute(last)
        if end <= start:
            raise rejection(
                "invalid_argument", f"the span `{first}..{last}` must end after it starts"
            )
        ranges.append((start, end))

    merged: list[tuple[int, int]] = []
    for start, end in sorted(ranges):
        if merged and start <= merged[-1][1]:
            merged[-1] = (merged[-1][0], max(merged[-1][1], end))
        else:
            merged.append((start, end))

    # The ceiling bounds what the preset writes, so it counts the whole stated
    # availability: spans that each fit the ceiling can still add up to millions of
    # entries written while the GIL is held. Two disjoint spans on one date write
    # two entries under one key, so this is the entries written, not the keys.
    entries = sum(
        (end - 1) // _UTC_DAY_MINUTES - start // _UTC_DAY_MINUTES + 1 for start, end in merged
    )
    if entries > _MAX_DATED_ENTRIES:
        raise rejection(
            "invalid_argument",
            f"the stated spans need {entries} dated entries, more than the "
            f"{_MAX_DATED_ENTRIES} this preset accepts: one is written per UTC day of every span",
        )

    dates: dict[str, list[str]] = {}
    for start, end in merged:
        cursor = start
        while cursor < end:
            day = cursor // _UTC_DAY_MINUTES
            day_start = day * _UTC_DAY_MINUTES
            stop = min(end, day_start + _UTC_DAY_MINUTES)
            date = (_EPOCH + timedelta(days=day)).date().isoformat()
            until = _clock(stop - day_start)
            dates.setdefault(date, []).append(f"{_clock(cursor - day_start)}-{until}")
            cursor = day_start + _UTC_DAY_MINUTES
    return dates


def _utc_minute(instant: str) -> int:
    """The whole minute an RFC 3339 instant names.

    The instant must be one of the engine's own instants: a value outside
    ``1975-01-01T00:00Z..=2100-01-01T00:00Z`` has no dated window the engine could
    read, so it is refused here with the range named rather than written as a
    window the engine silently never matches.
    """
    if not isinstance(instant, str):  # pyright: ignore[reportUnnecessaryIsInstance]
        raise rejection(
            "invalid_argument",
            f"`{instant}` is not an RFC 3339 instant with an explicit offset",
        )
    match = _INSTANT.fullmatch(instant)
    if match is None:
        raise rejection(
            "invalid_argument",
            f"`{instant}` is not an RFC 3339 instant with an explicit offset",
        )
    seconds = match.group(1)
    if seconds is not None and _WHOLE_MINUTE.fullmatch(seconds) is None:
        raise rejection("invalid_argument", f"`{instant}` is not aligned to a whole minute")
    try:
        # RFC 3339 lets the separator and the UTC designator be lower case, and the
        # TypeScript twin accepts both spellings. `fromisoformat` does not: it reads a
        # lower-case `t` but refuses a lower-case `z`, which would refuse here what the
        # other binding writes a window for. The spelling is already constrained to
        # digits, `-`, `:`, `.`, `+` and those two letters, so folding it is total.
        moment = datetime.fromisoformat(instant.upper())
    except ValueError as cause:
        raise rejection(
            "invalid_argument", f"`{instant}` is not an RFC 3339 instant: {cause}"
        ) from cause
    if moment.tzinfo is None:
        raise rejection("invalid_argument", f"`{instant}` names no offset, so it is not an instant")
    if moment < _SUPPORTED_FROM or moment > _SUPPORTED_TO:
        raise rejection(
            "invalid_argument",
            f"`{instant}` is outside the instants this package accepts, "
            f"{_SUPPORTED_RANGE}: a stated availability is written as dated windows, "
            "and every instant the engine holds is in that range",
        )
    delta = moment - _EPOCH
    if delta % timedelta(minutes=1) != timedelta(0):
        raise rejection("invalid_argument", f"`{instant}` is not aligned to a whole minute")
    return delta // timedelta(minutes=1)


def _clock(minute_of_day: int) -> str:
    """A minute of the day as a wire wall time."""
    return f"{minute_of_day // 60:02d}:{minute_of_day % 60:02d}"


__all__ = [
    "and_who",
    "available_only",
    "buffers",
    "business_hours",
    "cap_per_day",
    "cap_per_month",
    "cap_per_week",
    "dated_hours",
    "except_who",
    "horizon",
    "min_notice",
    "rest",
    "status_policy",
    "unless",
    "weekly_hours",
]
