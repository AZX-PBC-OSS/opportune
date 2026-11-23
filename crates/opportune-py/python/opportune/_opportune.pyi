"""The native extension: JSON in, JSON out, and one structured exception."""

__version__: str

class OpportuneError(Exception):
    """A rejected request, document or engine configuration.

    A rejected request carries every finding its validation produced in
    :attr:`errors`, each with the RFC 6901 JSON pointer of the request field it is
    about, so an exception handler can answer a client with the exact place in the
    document it sent that has to change.
    """

    code: str
    """The stable, machine-readable code, such as ``invalid_json``."""

    message: str
    """A sentence naming the value that was rejected.

    The text may be reworded; match on :attr:`code` instead.
    """

    path: str | None
    """The RFC 6901 JSON pointer of the field at fault.

    ``None`` when the failure is about the document as a whole.
    """

    errors: list[dict[str, str | None]]
    """The findings a rejected request carries.

    Each entry holds ``code``, ``message`` and ``path``, the last of which is
    ``None`` when the finding is not about one field.
    """

class _Engine:
    """An engine: the time zone database and the limits, shared by every thread.

    Build one once and reuse it. Every method releases the GIL around its Rust
    work, so threads sharing one instance run in parallel, and every method takes
    and returns JSON documents.

    The four query methods answer from this engine, so ``tzdb_dir`` and
    ``limits_json`` apply to them. The Graph and Google conversions resolve the
    zone names a payload carries against this engine's database too, so a custom
    directory is used for adapters and evaluation alike. Google ``freeBusy``
    ranges carry their own offsets and read no zone name.
    """

    def __init__(
        self,
        tzdb_dir: str | None = None,
        tzdb_version: str | None = None,
        limits_json: str | None = None,
    ) -> None:
        """Build the engine.

        :param tzdb_dir: A directory of ``TZif`` files to resolve zones against,
            or ``None`` for the database bundled into the extension.
        :param tzdb_version: The version label ``tzdb_dir`` reports, used when the
            directory has no ``+VERSION`` file. Passing one without ``tzdb_dir``
            raises :class:`OpportuneError`, because the bundled database reports
            its own version.
        :param limits_json: The request-size limits as JSON, or ``None`` for the
            engine's defaults.
        :raises OpportuneError: when the database cannot be built, the version is
            missing, the limits are not the shape the engine reads, or a version
            label is given with no directory.
        """

    @property
    def tzdb_version(self) -> str:
        """The version of the time zone database, as ``meta.tzdb`` reports it."""

    def __repr__(self) -> str:
        """The engine as a debugger or a log line shows it, with its database version."""

    def find_availability_json(self, request_json: str) -> str:
        """Return every bookable start the request allows, as a JSON response.

        :raises OpportuneError: when the document is not a request, or the request
            is invalid. The findings land in ``errors``.
        """

    def check_slot_json(self, request_json: str, proposal_json: str) -> str:
        """Judge one proposed booking, as a JSON slot check.

        :raises OpportuneError: when either document is rejected. A proposal that
            does not fit the request — an unknown part, the wrong number of terms
            or hosts, a duplicate host — names the proposal field at fault in
            ``path``, such as ``/events/0/assignment``.
        """

    def validate_json(self, request_json: str) -> str:
        """Return the request's static errors and warnings, as JSON diagnostics.

        :raises OpportuneError: when the document is not a request.
        """

    def required_ranges_json(self, query_json: str) -> str:
        """Return the data a request needs, as a JSON required-ranges value.

        :raises OpportuneError: when the document is rejected.
        """

    def graph_calendar_view_to_blocks_json(
        self,
        payload_json: str,
        participant_zone: str,
        category_map_json: str | None = None,
    ) -> str:
        """Convert a Graph ``calendarView`` response, as a JSON calendar outcome.

        Zone names resolve against this engine's database.

        :raises OpportuneError: when the participant zone is blank, the category
            map is not an object of tag paths, or the engine's database is
            unusable. An unreadable payload is an ``unavailable`` outcome.
        """

    def graph_get_schedule_to_blocks_json(self, payload_json: str) -> str:
        """Convert a Graph ``getSchedule`` response, as JSON outcomes keyed by schedule ID.

        :raises OpportuneError: when the document is not a ``getSchedule``
            response. A schedule the provider reported an error for is an
            ``unavailable`` outcome.
        """

    def graph_working_hours_to_rule_json(
        self, payload_json: str, rule_id: str | None = None
    ) -> str:
        """Convert one mailbox's Graph ``workingHours``, as a JSON working-hours outcome.

        ``rule_id`` is the ID the resulting ``window`` rule takes, defaulting to
        ``working_hours``. Zone names resolve against this engine's database.

        :raises OpportuneError: only when the engine's time zone database is
            unusable. A custom time zone is an ``unavailable`` outcome.
        """

    def google_events_list_to_blocks_json(self, payload_json: str, participant_zone: str) -> str:
        """Convert a Google ``events.list`` response, as a JSON calendar outcome.

        Zone names resolve against this engine's database.

        :raises OpportuneError: when the participant zone is blank. An unreadable
            payload is an ``unavailable`` outcome.
        """

    def google_free_busy_to_blocks_json(self, payload_json: str) -> str:
        """Convert a Google ``freeBusy.query`` response, as JSON outcomes keyed by calendar ID.

        :raises OpportuneError: when the document is not a ``freeBusy.query``
            response. A calendar the provider reported an error for is an
            ``unavailable`` outcome.
        """

def upgrade_rule_json(rule_json: str, from_version: int) -> str:
    """Rewrite a stored rule as the current schema version's canonical JSON.

    :raises OpportuneError: with code ``unsupported_schema_version`` when this
        build does not read ``from_version``, and the parser's own failure when the
        document is not a valid rule of that version.
    """

def upgrade_calendar_json(calendar_json: str, from_version: int) -> str:
    """Rewrite a stored calendar as the current schema version's canonical JSON.

    :raises OpportuneError: for the same reasons :func:`upgrade_rule_json` raises.
    """

def upgrade_goal_json(goal_json: str, from_version: int) -> str:
    """Rewrite a stored goal as the current schema version's canonical JSON.

    :raises OpportuneError: for the same reasons :func:`upgrade_rule_json` raises.
    """

MARKER_TAGS: str
"""The extended property, category or field a caller's own tags are read from."""

MARKER_EVENT_KEY: str
"""The marker carrying a caller's own event key."""

GRAPH_EXTENDED_PROPERTY_NAMESPACE_GUID: str
"""The GUID of opportune's permanent Graph extended-property set."""

GRAPH_EXTENDED_PROPERTY_TAGS_ID: str
"""The Graph extended-property ID that carries :data:`MARKER_TAGS`."""

GRAPH_EXTENDED_PROPERTY_EVENT_KEY_ID: str
"""The Graph extended-property ID that carries :data:`MARKER_EVENT_KEY`."""

GRAPH_EXTENDED_PROPERTY_EXPAND: str
"""The Graph ``$expand`` value that fetches both markers with an event."""

DEFAULT_LIMITS_JSON: str
"""The engine's default request-size limits, as the JSON document ``Limits`` reads."""
