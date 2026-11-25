"""The generated Pydantic models the package exposes.

`_models.py` is generated from the combined JSON Schema (`make codegen`) and is
never hand-edited, so these tests check the generated module rather than its
text: the shapes it must expose, that it reads the documents the engine's golden
corpus carries and the envelopes the adapter conversions emit, and that the two
properties the schema only states once each — the `true` selector and the
discriminated calendar outcome — survive generation.
"""

# The adapter conversions are reached through the native extension, the only producer of
# the envelopes these models describe, and `_Engine` is private because the package's
# public wrapper is `opportune.Engine`: reaching for it is the subject of the test rather
# than a leak into package code.
# pyright: reportPrivateUsage=false

import json
from collections.abc import Callable
from pathlib import Path
from typing import Annotated, Any

import pytest
from pydantic import BaseModel, ConfigDict, TypeAdapter, ValidationError

import opportune
from opportune import (
    Engine,
    OpportuneError,
    _models,
    _opportune,
    http_status_for_error,
    presets,
    with_updates,
)
from opportune._base import KeyedUnion, OpportuneModel

CRATES = Path(__file__).parents[2]
GOLDENS = CRATES / "opportune-testkit" / "goldens"
ADAPTER_FIXTURES = CRATES / "opportune-adapters" / "tests" / "fixtures"

EVERY_DEFAULT_NULL: dict[str, Any] = {
    "now": "2027-03-09T15:00Z",
    "goal": {
        "parts": [
            {
                "tags": None,
                "duration": "30m",
                "requirement": {"all": [{"person": "ana"}]},
                "travel": {"before": "0m", "after": "0m", "by_participant": None},
            }
        ],
        "search_window": ["2027-03-09T15:00Z", "2027-03-09T16:00Z"],
        "zone": "UTC",
    },
    "participants": [
        {
            "id": "ana",
            "zone": "UTC",
            "tags": None,
            "zone_periods": None,
            "calendar": [
                {
                    "span": ["2027-03-09T15:00Z", "2027-03-09T16:00Z"],
                    "status": None,
                    "all_day": None,
                    "tags": None,
                }
            ],
        }
    ],
    "rules": [
        {
            "id": "w",
            "kind": "window",
            "who": None,
            "when": None,
            "params": {"zone": "UTC", "weekly": None, "dates": None},
        }
    ],
    "occurrences": None,
    "exclude_event_keys": None,
    "calendars": {"org": {"zone": "UTC", "working_days": "mon-fri", "holidays": None}},
    "weights": None,
}
"""A request writing `None` on every defaulted field this document reaches.

The request-level collections are `EVERY_DEFAULT_NULL_LISTS` below: a document that
names a participant cannot also spell the participant list as `null`, so the sites a
populated request reaches and the sites only an empty one reaches need two documents.
"""

THE_SAME_REQUEST_OMITTED: dict[str, Any] = {
    "now": "2027-03-09T15:00Z",
    "goal": {
        "parts": [
            {
                "duration": "30m",
                "requirement": {"all": [{"person": "ana"}]},
                "travel": {"before": "0m", "after": "0m"},
            }
        ],
        "search_window": ["2027-03-09T15:00Z", "2027-03-09T16:00Z"],
        "zone": "UTC",
    },
    "participants": [
        {
            "id": "ana",
            "zone": "UTC",
            "calendar": [{"span": ["2027-03-09T15:00Z", "2027-03-09T16:00Z"]}],
        }
    ],
    "rules": [{"id": "w", "kind": "window", "params": {"zone": "UTC"}}],
    "calendars": {"org": {"zone": "UTC", "working_days": "mon-fri"}},
}
"""The document the same request is, with every null left out."""

EVERY_DEFAULT_NULL_LISTS: dict[str, Any] = {
    "now": "2027-03-09T15:00Z",
    "goal": {
        "parts": [
            {
                "tags": None,
                "duration": "30m",
                "requirement": {"all": [{"person": "ana"}]},
                "travel": {"before": "0m", "after": "0m", "by_participant": None},
            }
        ],
        "search_window": ["2027-03-09T15:00Z", "2027-03-09T16:00Z"],
        "zone": "UTC",
    },
    "participants": None,
    "rules": None,
    "occurrences": None,
    "exclude_event_keys": None,
    "calendars": None,
    "weights": None,
}
"""The request-level collections this format defaults, each written as `None`."""

THE_SAME_REQUEST_WITH_NULL_LISTS_OMITTED: dict[str, Any] = {
    "now": "2027-03-09T15:00Z",
    "goal": {
        "parts": [
            {
                "duration": "30m",
                "requirement": {"all": [{"person": "ana"}]},
                "travel": {"before": "0m", "after": "0m"},
            }
        ],
        "search_window": ["2027-03-09T15:00Z", "2027-03-09T16:00Z"],
        "zone": "UTC",
    },
}
"""The same document with the six keys left out entirely."""

AN_OCCURRENCE_WITH_NULL_TAGS: dict[str, Any] = {
    "occurrences": [
        {
            "event_key": "e",
            "span": ["2027-03-09T15:00Z", "2027-03-09T15:30Z"],
            "tags": None,
            "participants": ["ana"],
        }
    ]
}
"""An occurrence whose tag list is `null`, which a request with no occurrence cannot reach."""

THE_SAME_OCCURRENCE_UNTAGGED: dict[str, Any] = {
    "occurrences": [
        {
            "event_key": "e",
            "span": ["2027-03-09T15:00Z", "2027-03-09T15:30Z"],
            "participants": ["ana"],
        }
    ]
}
"""The same occurrence with the tag list left out."""

RULE_PARAMS: dict[str, dict[str, Any]] = {
    "blackout": {"zone": "UTC", "spans": []},
    "clearance": {"before": "0m", "after": "0m", "min_gap": "0m"},
    "lead_time": {"min": "0m"},
    "status_policy": {"entries": []},
    "rest": {"late_after": "17:00", "not_before": "09:00"},
    "cap": {
        "count": True,
        "limit": 1,
        "measure": "count",
        "per": "each",
        "period": "day",
        "zone": "UTC",
    },
}
"""The smallest valid parameters for each rule kind other than `window`."""


def _goldens() -> list[dict[str, Any]]:
    return [json.loads(path.read_text(encoding="utf-8")) for path in sorted(GOLDENS.glob("*.json"))]


def _document(value: Any) -> Any:
    """The golden's document, which the byte-pinned keys carry as its own text."""
    return json.loads(value) if isinstance(value, str) else value


def _fixture(provider: str, name: str) -> str:
    return (ADAPTER_FIXTURES / provider / name).read_text(encoding="utf-8")


def test_the_generated_module_exposes_every_wire_and_adapter_shape() -> None:
    for name in [
        "Request",
        "Response",
        "Offer",
        "OfferEvent",
        "DateCount",
        "Excluded",
        "Reason",
        "Proposal",
        "SlotCheck",
        "RangesQuery",
        "RequiredRanges",
        "Diagnostics",
        "Diagnostic",
        "Limits",
        "WindowRule",
        "BlackoutRule",
        "ClearanceRule",
        "LeadTimeRule",
        "StatusPolicyRule",
        "RestRule",
        "CapRule",
        "Selector",
        "Participant",
        "Block",
        "Occurrence",
        "Goal",
        "Part",
        "CalendarSpec",
        "AnnotatedBlock",
        "CalendarOutcome",
        "AvailableCalendarOutcome",
        "UnavailableCalendarOutcome",
        "AdapterDiagnostic",
        "GraphWorkingHoursOutcome",
    ]:
        assert hasattr(_models, name), f"the generated models have no `{name}`"


def test_every_golden_document_reads_into_the_generated_models() -> None:
    """The generated models accept what the engine's own wire layer writes."""
    goldens = _goldens()
    assert goldens, "the golden corpus is empty"

    for golden in goldens:
        _models.Request.model_validate(golden["request"])
        _models.Response.model_validate(_document(golden["response"]))
        for key, model in [
            ("proposal", _models.Proposal),
            ("slot_check", _models.SlotCheck),
            ("ranges_query", _models.RangesQuery),
            ("required_ranges", _models.RequiredRanges),
        ]:
            if key in golden:
                model.model_validate(_document(golden[key]))


def test_a_provider_status_outside_the_three_reads_as_busy_at_the_model_boundary() -> None:
    """A block's status is the provider's own string, and an unknown one is `busy`.

    The published asymmetry: a block's status is provider data, so the generated model
    accepts the open string the wire layer reads and normalises a value from a
    calendar's own vocabulary to `busy` while the document is read, the same reading
    the engine fails safe on. A value of another type stays a malformed document, and
    a status-policy entry's `status` is the caller's own configuration, so it stays the
    closed three.
    """

    def block(status: str | None) -> _models.Block:
        return _models.Block.model_validate(
            {"span": ["2027-03-09T15:00Z", "2027-03-09T16:00Z"], "status": status}
        )

    for reported in ("workingElsewhere", "oof", ""):
        assert block(reported).status == "busy", f"`{reported}` is not a status the engine knows"
    for known in ("free", "tentative", "busy"):
        assert block(known).status == known, f"`{known}` is a status the engine knows"
    assert block(None).status is None, "an absent status stays absent, and the wire reads busy"

    with pytest.raises(ValidationError) as raised:
        _models.Block.model_validate(
            {"span": ["2027-03-09T15:00Z", "2027-03-09T16:00Z"], "status": 5}
        )
    assert raised.value.errors()[0]["type"] == "string_type", "a number is not a provider status"

    with pytest.raises(ValidationError):
        _models.StatusPolicyEntry(effect="block", status="oof")  # pyright: ignore[reportArgumentType]


def test_the_published_block_status_is_open_where_a_policy_entry_status_is_closed() -> None:
    """The document states the asymmetry the validator applies.

    A block's `status` is provider data, so the model advertises an open string and an
    unrecognised value is normalised as the block is read; a status-policy entry's
    `status` is the caller's own configuration, so its published set is still the
    closed three. Normalising the block by narrowing the field instead would pass the
    validation tests above — the before-validator runs whatever the annotation says —
    while the document a client generates its types from would reject the provider
    statuses the wire reads. The statuses the boundary preserves are therefore taken
    from the closed definition the document publishes rather than restated here, so a
    schema that grows one surfaces here instead of being rewritten to `busy`.
    """
    block_status = _models.Block.model_json_schema()["properties"]["status"]
    arms = block_status["anyOf"]
    assert {arm.get("type") for arm in arms} == {"string", "null"}, (
        f"a provider's own status, or its absence, is what the document admits: {block_status}"
    )
    assert all(set(arm) == {"type"} for arm in arms), (
        f"no arm narrows the open string to an enumerated status: {block_status}"
    )
    assert "enum" not in block_status and "const" not in block_status, (
        f"nothing narrows the property to the statuses the engine enumerates: {block_status}"
    )

    entry = _models.StatusPolicyEntry.model_json_schema()
    assert entry["properties"]["status"]["anyOf"][0]["$ref"] == "#/$defs/Status"
    declared = entry["$defs"]["Status"]["enum"]
    assert declared, "the closed definition names the statuses an entry may state"

    survived = [
        _models.Block.model_validate(
            {"span": ["2027-03-09T15:00Z", "2027-03-09T16:00Z"], "status": known}
        ).status
        for known in declared
    ]
    assert survived == declared, "every status the document names survives the boundary"


def test_every_generated_model_states_the_extra_policy_its_definition_states() -> None:
    """A response tolerates a newer engine's field; a request refuses a stray key.

    The exporter states ``additionalProperties: false`` on every request and input
    object and omits it on every response and output one, because serde reads the
    latter without ``deny_unknown_fields``. A generated model has to agree with that
    document, or an additive engine release becomes a breaking one: an older
    deployment would refuse an answer its successor wrote. Which is which is read
    off each model's own published schema — Pydantic emits
    ``additionalProperties: false`` only for ``extra="forbid"`` — rather than from a
    list kept beside it, so a definition the schema opens or closes surfaces here.
    """
    open_models: list[str] = []
    closed_models: list[str] = []

    def published(model: type[BaseModel]) -> dict[str, Any]:
        """The model's own object schema, with a recursive model's `$ref` resolved."""
        schema = model.model_json_schema()
        ref = schema.get("$ref")
        if isinstance(ref, str):
            return schema["$defs"][ref.rsplit("/", 1)[-1]]
        return schema

    for name in _models.__all__:
        model = getattr(_models, name)
        if not (isinstance(model, type) and issubclass(model, BaseModel)):
            continue
        if model.__module__ != _models.__name__:
            continue
        if published(model).get("additionalProperties") is False:
            assert model.model_config.get("extra") == "forbid", name
            closed_models.append(name)
        else:
            assert model.model_config.get("extra") == "ignore", name
            open_models.append(name)

    assert closed_models, "no model refuses a stray key, so a request could carry one"
    assert open_models, "no model tolerates an additive field, so a newer engine breaks this one"
    assert {"Request", "Goal", "WindowRule", "Rule"} & set(closed_models) == {
        "Request",
        "Goal",
        "WindowRule",
    }, "every request and input object is closed"
    assert {"Response", "Offer", "SlotCheck", "Meta", "RequiredRanges"} <= set(open_models), (
        "every response and output object is open"
    )


def test_a_response_tolerates_an_additive_field_where_a_request_refuses_one() -> None:
    """A rolling deploy reads a newer engine's answer, and still rejects a stray key.

    The answer a deployment reads may have been written by a newer instance, so each
    response and output shape accepts a field this build has no name for and drops
    it when the answer is dumped. A request runs the other way: it is this caller's
    own document, and a key the schema does not name is a mistake the engine would
    refuse after the document had crossed the boundary — so the model refuses it
    first, at the field the caller wrote.
    """
    request_document: dict[str, Any] = {
        "now": "2027-06-14T08:00Z",
        "goal": {
            "zone": "UTC",
            "search_window": ["2027-06-14T09:00Z", "2027-06-14T11:00Z"],
            "parts": [{"duration": "30m", "requirement": {"all": [{"person": "ana"}]}}],
        },
        "participants": [{"id": "ana", "zone": "UTC", "calendar": []}],
    }
    request = _models.Request.model_validate(request_document)
    engine = Engine()

    answer = json.loads(engine.find_availability_json(request))
    assert answer["offers"], "the fixture offers at least one start"
    answer["future_field"] = "v0.2"
    answer["meta"]["future_field"] = "v0.2"
    for offer in answer["offers"]:
        offer["future_field"] = "v0.2"
        for event in offer["events"]:
            event["future_field"] = "v0.2"

    parsed = _models.Response.model_validate_json(json.dumps(answer))
    assert len(parsed.offers) == len(answer["offers"]), "every offer survives the parse"
    assert parsed.meta == _models.Meta.model_validate(answer["meta"])
    assert "future_field" not in parsed.model_dump(), (
        "an additive field is dropped, because this build has nothing to serve for it"
    )

    # The engine's own answer path reads the same document: an answer carrying a
    # field this build has no name for is not a `serialization_failed` defect.
    engine.find_availability_json = lambda _request: json.dumps(answer)  # pyright: ignore[reportAttributeAccessIssue]
    assert engine.find_availability(request) == parsed

    proposal = opportune.offer_to_proposal(parsed.offers[0])
    check = json.loads(engine.check_slot_json(request, proposal))
    check["future_field"] = "v0.2"
    check["meta"]["future_field"] = "v0.2"
    assert _models.SlotCheck.model_validate_json(json.dumps(check)).valid is True

    ranges = json.loads(engine.required_ranges_json(request))
    assert "occurrences" not in ranges, (
        "an absent optional output field is omitted, matching every other one"
    )
    ranges["future_field"] = "v0.2"
    assert _models.RequiredRanges.model_validate_json(json.dumps(ranges)) == engine.required_ranges(
        request
    )

    for document in (
        {**request_document, "future_field": "v0.2"},
        {
            **request_document,
            "goal": {**request_document["goal"], "future_field": "v0.2"},
        },
        {
            **request_document,
            "rules": [
                {"id": "hours", "kind": "window", "params": {"zone": "UTC"}, "future_field": 1}
            ],
        },
    ):
        with pytest.raises(ValidationError) as raised:
            _models.Request.model_validate(document)
        assert raised.value.errors(include_url=False)[0]["type"] == "extra_forbidden"

    with pytest.raises(ValidationError) as raised:
        _models.WindowRule.model_validate(
            {
                "id": "hours",
                "kind": "window",
                "params": {"zone": "UTC", "weekly": {"mon": ["09:00-17:00"]}},
                "future_field": "v0.2",
            }
        )
    assert raised.value.errors(include_url=False)[0]["loc"] == ("future_field",)


def test_a_provider_status_outside_the_three_fails_safe_in_a_search() -> None:
    """The model's normalised `busy` is the reading the engine acts on."""

    def request(status: str) -> _models.Request:
        return _models.Request.model_validate(
            {
                "now": "2027-03-09T15:00Z",
                "goal": {
                    "parts": [{"duration": "30m", "requirement": {"all": [{"person": "ana"}]}}],
                    "search_window": ["2027-03-09T15:00Z", "2027-03-09T16:00Z"],
                    "zone": "UTC",
                },
                "participants": [
                    {
                        "id": "ana",
                        "zone": "UTC",
                        "calendar": [
                            {
                                "span": ["2027-03-09T15:00Z", "2027-03-09T16:00Z"],
                                "status": status,
                                "tags": [],
                            }
                        ],
                    }
                ],
            }
        )

    engine = Engine()
    assert engine.find_availability(request("free")).offers, "a free block leaves the hour open"
    assert not engine.find_availability(request("workingElsewhere")).offers, (
        "a status the engine does not know fails safe as busy, never as free time"
    )


def test_a_model_carrying_none_answers_as_the_field_omitted() -> None:
    """A model's `None` is the default the wire applies to an omission.

    The generated models admit `None` on every defaulted field, and the binding
    serialises with `exclude_none`, so a mapping that filled every key in presents
    the document an omission does: the engine answers the two spellings identically,
    and the window a caller nulled shares no hours rather than failing to read.
    """
    engine = Engine()
    with_none = _models.Request.model_validate(EVERY_DEFAULT_NULL)
    omitted = _models.Request.model_validate(THE_SAME_REQUEST_OMITTED)

    answer = engine.find_availability(with_none)
    assert answer.offers == [], "a window with no hours shares none"
    assert [(count.date, count.offers) for count in answer.dates] == [("2027-03-09", 0)]

    assert engine.find_availability_json(with_none) == engine.find_availability_json(omitted), (
        "a null on a defaulted field is the same document as leaving it out"
    )


def test_a_model_carrying_none_on_every_list_answers_as_the_field_omitted() -> None:
    """A `null` list a caller's mapper wrote is the empty list, at the request's own sites.

    `participants`, `rules`, `occurrences`, `exclude_event_keys`, `calendars` and
    `weights` are the defaulted fields a request read back from a stored JSON
    document spells as `null`, and six of that document's keys are not ones a
    populated request can also null — so they need the empty document. A `null` that
    emptied the participant list rather than failing to read is what the unknown
    participant in the goal proves:
    the document parsed, it simply names nobody.
    """
    engine = Engine()
    with_none = _models.Request.model_validate(EVERY_DEFAULT_NULL_LISTS)
    omitted = _models.Request.model_validate(THE_SAME_REQUEST_WITH_NULL_LISTS_OMITTED)

    diagnostics = engine.validate(with_none)
    assert [(finding.code, finding.path) for finding in diagnostics.errors] == [
        ("participant_unknown", "/goal/parts/0/requirement/all/0")
    ], "a null participant list is an empty one, not an unreadable document"

    assert engine.validate_json(with_none) == engine.validate_json(omitted), (
        "a null list is the same document as leaving it out"
    )


def test_an_occurrence_carrying_none_tags_answers_as_the_field_omitted() -> None:
    """An occurrence's `null` tag list is an untagged occurrence, not an unreadable one.

    It is the one defaulted field a request with no occurrence cannot reach, so the
    request-level document above never exercises it.
    """
    engine = Engine()
    with_none = _models.Request.model_validate(
        {**EVERY_DEFAULT_NULL, **AN_OCCURRENCE_WITH_NULL_TAGS}
    )
    untagged = _models.Request.model_validate(
        {**EVERY_DEFAULT_NULL, **THE_SAME_OCCURRENCE_UNTAGGED}
    )

    assert engine.validate(with_none).errors == [], (
        "a null tag list on an occurrence is an empty one"
    )

    assert engine.validate_json(with_none) == engine.validate_json(untagged), (
        "a null tag list is the same document as leaving it out"
    )


def test_every_rule_kind_carrying_none_selectors_answers_as_the_field_omitted() -> None:
    """A rule's `null` `who`/`when` is the default the model states, in every kind.

    The request-level document above reaches only the `window` kind, and each kind
    states the pair for itself, so a kind that stopped admitting `null` would leave
    the published schema claiming something the models reject. The rules are a
    discriminated union, so this also pins that a `null` beside `kind` does not turn
    one rule into a union explosion.
    """
    engine = Engine()
    with_none = _models.Request.model_validate(
        {
            **EVERY_DEFAULT_NULL,
            "rules": [
                {"id": kind, "kind": kind, "who": None, "when": None, "params": params}
                for kind, params in RULE_PARAMS.items()
            ],
        }
    )
    omitted = _models.Request.model_validate(
        {
            **EVERY_DEFAULT_NULL,
            "rules": [
                {"id": kind, "kind": kind, "params": params} for kind, params in RULE_PARAMS.items()
            ],
        }
    )

    assert engine.validate(with_none).errors == [], (
        "a null who and when is every host and every booking, in every kind"
    )

    assert engine.validate_json(with_none) == engine.validate_json(omitted), (
        "a null selector pair is the same document as leaving it out"
    )


def test_the_match_all_selector_stays_the_constant_true() -> None:
    """`--collapse-root-models` would drop this, so the models must enforce it."""
    rule: dict[str, Any] = {
        "id": "hours",
        "kind": "window",
        "params": {"weekly": {"mon": ["09:00-17:00"]}, "zone": "UTC"},
    }
    validated = _models.WindowRule.model_validate(rule)
    assert validated.when is True, "an absent `when` must default to the `true` selector"

    assert _models.WindowRule.model_validate({**rule, "when": True}).when is True
    with pytest.raises(ValidationError):
        _models.WindowRule.model_validate({**rule, "when": False})


def test_the_adapter_envelopes_read_the_conversions_own_output() -> None:
    """Every envelope the extension emits validates as its generated model."""
    engine = _opportune._Engine()
    outcome: TypeAdapter[Any] = TypeAdapter(_models.CalendarOutcome)

    graph = json.loads(
        engine.graph_calendar_view_to_blocks_json(
            _fixture("graph", "calendar_view_ana.json"), "America/Los_Angeles"
        )
    )
    assert isinstance(outcome.validate_python(graph), _models.AvailableCalendarOutcome)

    schedule = json.loads(
        engine.graph_get_schedule_to_blocks_json(_fixture("graph", "get_schedule.json"))
    )
    assert {type(outcome.validate_python(entry)) for entry in schedule.values()} == {
        _models.AvailableCalendarOutcome,
        _models.UnavailableCalendarOutcome,
    }

    google = json.loads(
        engine.google_events_list_to_blocks_json(
            _fixture("google", "events_list_ana.json"), "America/Los_Angeles"
        )
    )
    assert isinstance(outcome.validate_python(google), _models.AvailableCalendarOutcome)

    free_busy = json.loads(
        engine.google_free_busy_to_blocks_json(_fixture("google", "free_busy.json"))
    )
    assert isinstance(
        outcome.validate_python(free_busy["bo@example.com"]),
        _models.AvailableCalendarOutcome,
    )

    working_hours: TypeAdapter[Any] = TypeAdapter(_models.GraphWorkingHoursOutcome)
    assert isinstance(
        working_hours.validate_python(
            json.loads(
                engine.graph_working_hours_to_rule_json(_fixture("graph", "working_hours.json"))
            )
        ),
        _models.AvailableWorkingHoursOutcome,
    )

    # No committed fixture carries a custom Graph time zone, one of the working-hours
    # failures the adapter reports as an `unavailable` outcome rather than raising.
    custom_zone = json.dumps(
        {
            "daysOfWeek": ["monday"],
            "startTime": "09:00:00.0000000",
            "endTime": "17:00:00.0000000",
            "timeZone": {"@odata.type": "#microsoft.graph.customTimeZone", "bias": 60},
        }
    )
    assert isinstance(
        working_hours.validate_python(
            json.loads(engine.graph_working_hours_to_rule_json(custom_zone))
        ),
        _models.UnavailableWorkingHoursOutcome,
    )


def test_a_calendar_outcome_cannot_carry_both_variant_shapes() -> None:
    outcome: TypeAdapter[Any] = TypeAdapter(_models.CalendarOutcome)

    unavailable = outcome.validate_python(
        {"status": "unavailable", "code": "unexpanded_series", "message": "expand it"}
    )
    assert isinstance(unavailable, _models.UnavailableCalendarOutcome)

    with pytest.raises(ValidationError):
        outcome.validate_python(
            {
                "status": "available",
                "blocks": [],
                "diagnostics": [],
                "code": "unexpanded_series",
            }
        )


def test_every_union_the_wire_discriminates_is_a_tagged_union() -> None:
    """The tag is what turns a bad field into one error instead of one per member.

    The schema's `discriminator` reaches the generated model, so Pydantic picks the
    variant before it validates the fields, and a caller sees the offending field
    rather than the union's whole membership.
    """
    # The rule union is reachable through the request that holds it, so its schema
    # comes from the model rather than from an adapter over the alias.
    rule: dict[str, Any] = _models.Request.model_json_schema()["$defs"]["Rule"]
    assert rule["discriminator"]["propertyName"] == "kind", rule
    assert rule["discriminator"]["mapping"]["cap"] == "#/$defs/CapRule"
    assert len(rule["oneOf"]) >= 2

    # The adapter envelopes are read on their own, so each one needs its own adapter.
    calendar: TypeAdapter[Any] = TypeAdapter(_models.CalendarOutcome)
    working_hours: TypeAdapter[Any] = TypeAdapter(_models.GraphWorkingHoursOutcome)
    for adapter in (calendar, working_hours):
        envelope: dict[str, Any] = adapter.json_schema()
        assert envelope["discriminator"]["propertyName"] == "status", envelope
        assert len(envelope["oneOf"]) == 2, "an outcome is exactly available or unavailable"


def test_a_bad_field_inside_a_discriminated_rule_is_one_finding() -> None:
    adapter: TypeAdapter[Any] = TypeAdapter(_models.Rule)
    broken = {
        "id": "interviews",
        "kind": "cap",
        "params": {
            "count": {"tag": "interview"},
            "limit": -1,
            "measure": "count",
            "per": "each",
            "period": "day",
            "zone": "participant",
        },
    }

    with pytest.raises(ValidationError) as raised:
        adapter.validate_python(broken)

    findings = raised.value.errors()
    assert len(findings) == 1, f"one bad field is one finding, not one per union member: {findings}"
    assert findings[0]["loc"][-2:] == ("params", "limit")


def test_every_field_but_the_structural_ones_carries_its_schema_prose() -> None:
    """The schema's prose is part of the API document, so it reaches every field.

    The exceptions are structural: the tag that discriminates a union and the single
    key a selector form is named after say what they are by existing.
    """
    undocumented: list[str] = []
    for name, value in vars(_models).items():
        if not (isinstance(value, type) and issubclass(value, BaseModel)) or value is BaseModel:
            continue
        for field, info in value.model_fields.items():
            if field in {"kind", "status"} or name.endswith("Selector"):
                continue
            if not (info.description or "").strip():
                undocumented.append(f"{name}.{field}")

    assert not undocumented, f"the schema's prose did not reach {undocumented}"


def test_the_generated_module_carries_no_stray_alias() -> None:
    """`type Model = Any` is the generator's unused root alias, stripped by codegen."""
    assert not hasattr(_models, "Model")


def test_an_override_names_the_field_rather_than_its_wire_alias() -> None:
    """`with_updates` takes either spelling, and leaves one of them behind.

    The field name is what ``model_dump()`` writes, so naming the field by its wire
    alias would otherwise leave both spellings in the merged document and Pydantic
    would read the spare as an extra key.
    """
    meta = _models.Meta(engine="1.0.0", schema_=1, tzdb="2026f")

    updated = with_updates(meta, tzdb="2025a")

    assert updated == _models.Meta(engine="1.0.0", schema_=1, tzdb="2025a")
    assert updated.model_dump(by_alias=True)["schema"] == 1

    assert with_updates(meta, schema_=2).schema_ == 2
    assert with_updates(meta, **{"schema": 3}).schema_ == 3


def test_a_rule_holding_an_aliased_selector_round_trips_through_a_dump_and_an_update() -> None:
    """`not` is a Python keyword, so its field is `not_` and its wire key is `not`.

    ``model_dump()`` writes the field name, so a dumped selector reaches the
    dispatcher as ``{"not_": …}``: it has to accept the spelling the model accepts,
    or every round trip through a dump — an update, a database row, a re-read —
    rejects a selector the model itself built.
    """
    inner = _models.TagSelector(tag="x")
    rule = presets.cap_per_day(True, 2, who=_models.NotSelector(not_=inner))
    who = rule.who
    assert isinstance(who, _models.NotSelector), who
    nested = _models.NotSelector(not_=who)

    dumped = rule.model_dump()
    assert _models.CapRule.model_validate(dumped) == rule, "a dumped rule reads back unchanged"
    assert _models.NotSelector.model_validate(nested.model_dump()) == nested

    renumbered = with_updates(rule, id="renumbered")
    assert renumbered.id == "renumbered"
    assert renumbered.who == rule.who, "the update replaced the rule's ID and nothing else"

    assert _models.CapRule.model_validate_json(rule.model_dump_json(by_alias=True)) == rule


def test_a_request_round_trips_through_its_own_dump_with_aliased_fields() -> None:
    """The document `model_dump()` writes is one `Request.model_validate` reads.

    Both alias families take part: a rule's negated selector (`not_`) and the
    response-shaped aliases a caller copies between documents (`schema_`, `from_`).
    """
    request = _models.Request.model_validate(
        {
            "now": "2027-03-09T15:00Z",
            "goal": {
                "parts": [{"duration": "30m", "requirement": {"all": [{"person": "ana"}]}}],
                "search_window": ["2027-03-09T15:00Z", "2027-03-09T16:00Z"],
                "zone": "UTC",
            },
            "participants": [
                {
                    "id": "ana",
                    "zone": "UTC",
                    "zone_periods": [
                        {"from": "2027-03-01T00:00Z", "until": "2027-04-01T00:00Z", "zone": "UTC"}
                    ],
                }
            ],
            "rules": [
                {
                    "id": "hours",
                    "kind": "window",
                    "params": {"weekly": {"mon": ["09:00-17:00"]}, "zone": "UTC"},
                    "who": {"not": {"tag": "x"}},
                }
            ],
        }
    )

    again = _models.Request.model_validate(request.model_dump())

    assert again == request
    assert again.model_dump_json(by_alias=True, exclude_none=True) == request.model_dump_json(
        by_alias=True, exclude_none=True
    ), "the wire document is unchanged by the round trip"


def test_every_keyed_union_names_its_forms_in_one_finding() -> None:
    """A typo in a term or a lead-time maximum is one finding, like a selector's.

    Neither union is discriminated by the schema, so without the dispatch Pydantic
    tries every arm: five findings for a term, eleven for a lead-time maximum, none
    of which says which keys the document has.
    """
    for adapter, value, named in (
        (TypeAdapter(_models.Term), {"persn": "ana"}, "person"),
        (
            TypeAdapter(_models.LeadTimeMax),
            {"calendar": "org", "working_dayz": 4},
            "working_days",
        ),
    ):
        with pytest.raises(ValidationError) as raised:
            adapter.validate_python(value)
        findings = raised.value.errors()
        assert len(findings) == 1, f"one unknown key is one finding, not a wall: {findings}"
        assert findings[0]["type"].endswith("_form_unknown"), findings[0]
        assert named in findings[0]["msg"], "the finding names the forms a caller may write"

    # Each arm still validates the values it does accept, one-key and two-key alike.
    term = TypeAdapter(_models.Term).validate_python({"at_least": 2, "of": True})
    assert term == _models.AtLeastTerm(at_least=2, of=True)
    assert TypeAdapter(_models.Term).validate_python({"of": True, "at_least": 2}) == term
    assert TypeAdapter(_models.Term).validate_python({"person": "ana"}) == _models.PersonTerm(
        person="ana"
    )
    assert TypeAdapter(_models.LeadTimeMax).validate_python({"days": 4}) == _models.DaysMax(days=4)
    working_days = _models.WorkingDaysMax(calendar="org", working_days=4)
    assert (
        TypeAdapter(_models.LeadTimeMax).validate_python({"calendar": "org", "working_days": 4})
        == working_days
    )
    assert (
        TypeAdapter(_models.LeadTimeMax).validate_python({"working_days": 4, "calendar": "org"})
        == working_days
    ), "mapping order does not choose a two-key arm"


def test_a_non_string_form_key_is_one_validation_finding() -> None:
    """A Python mapping's non-string key is rejected by validation, not a `KeyError`."""
    with pytest.raises(ValidationError) as raised:
        TypeAdapter(_models.Selector).validate_python({None: True})

    findings = raised.value.errors()
    assert len(findings) == 1, findings
    assert findings[0]["type"] == "selector_form_unknown"
    assert "`None`" in findings[0]["msg"]


def test_a_keyed_union_derives_its_forms_from_the_arms_it_is_attached_to() -> None:
    """A new arm is accepted and named without editing a list kept beside it.

    The forms are the arms' own keys, so a union defined outside the generated module
    — a schema that grows a form, or a test's own — is dispatched by what it declares.
    """

    class ColourForm(OpportuneModel):
        model_config = ConfigDict(extra="forbid", populate_by_name=True)
        colour: str

    class SizeForm(OpportuneModel):
        model_config = ConfigDict(extra="forbid", populate_by_name=True)
        size: int

    finish: TypeAdapter[ColourForm | SizeForm] = TypeAdapter(
        Annotated[ColourForm | SizeForm, KeyedUnion("finish")]
    )

    assert finish.validate_python({"colour": "red"}) == ColourForm(colour="red")
    assert finish.validate_python({"size": 3}) == SizeForm(size=3)

    with pytest.raises(ValidationError) as raised:
        finish.validate_python({"color": "red"})
    findings = raised.value.errors()
    assert len(findings) == 1, findings
    assert findings[0]["type"] == "finish_form_unknown"
    assert "`color`" in findings[0]["msg"]
    assert "colour, size" in findings[0]["msg"], "both arms are named, so the typo is obvious"


def test_a_kind_selector_is_dispatched_from_the_generated_union() -> None:
    """The generated `Selector` reaches `KindSelector` the way it reaches every form.

    Nothing lists the arms beside the union: the dispatch derives each form's key
    from the arm, so a selector a schema grows is reachable without an edit here.
    """
    dispatch: TypeAdapter[_models.Selector] = TypeAdapter(_models.Selector)

    for kind in ("person", "resource"):
        assert dispatch.validate_python({"kind": kind}) == _models.KindSelector(kind=kind)

    assert opportune.KindSelector is _models.KindSelector, "the package root exports it"

    with pytest.raises(ValidationError) as raised:
        dispatch.validate_python({"kind": "room"})
    findings = raised.value.errors()
    assert len(findings) == 1, findings
    assert findings[0]["type"] == "literal_error"
    assert findings[0]["loc"] == ("kind", "kind"), "the finding is the form's own field"
    assert findings[0]["msg"] == "Input should be 'person' or 'resource'"


UNSIGNED_FIELDS: list[tuple[Callable[[int], BaseModel], int]] = [
    (lambda value: _models.DaysMax(days=value), 0xFFFF_FFFF),
    (lambda value: _models.Meta(schema_=value, engine="1", tzdb="1"), 0xFFFF_FFFF),
    (lambda value: _models.Explain(limit=value), 0xFFFF_FFFF_FFFF_FFFF),
    (lambda value: opportune.limits_with(max_participants=value), 0xFFFF_FFFF_FFFF_FFFF),
]
"""A generated field of each wire width, and the largest value that width admits."""


@pytest.mark.parametrize(
    ("build", "bound"),
    UNSIGNED_FIELDS,
    ids=["uint32", "aliased-uint32", "uint64", "limits-uint64"],
)
def test_a_wire_unsigned_integer_refuses_a_value_past_its_width(
    build: Callable[[int], BaseModel],
    bound: int,
) -> None:
    """A `u32`/`u64` field is bounded by the width the schema states as its `format`.

    The schema states the width and no `maximum`, so the generated models would
    otherwise admit an integer the engine's own reader refuses — a rejection that
    arrived only after the document had crossed the boundary.
    """
    assert build(bound).model_dump(by_alias=True), "the boundary value is a valid document"

    with pytest.raises(ValidationError) as raised:
        build(bound + 1)

    finding = raised.value.errors(include_url=False)[0]
    assert finding["type"] == "less_than_equal", finding


@pytest.mark.parametrize(
    "boundary",
    [2**31 - 1, -(2**31)],
    ids=["i32-max", "i32-min"],
)
def test_request_weights_accepts_i32_boundaries_and_reaches_the_engine(boundary: int) -> None:
    """`Request.weights` admits the full signed 32-bit range and passes it to the engine."""
    request = _models.Request.model_validate(
        {
            **THE_SAME_REQUEST_OMITTED,
            "participants": [{"id": "ana", "zone": "UTC", "calendar": []}],
            "rules": [],
            "weights": {"ana": boundary},
        }
    )
    assert request.weights == {"ana": boundary}
    response = Engine().find_availability(request)
    assert response.offers, "the engine evaluated the request with the boundary weight"


@pytest.mark.parametrize(
    ("out_of_range", "expected_type"),
    [(2**31, "less_than_equal"), (-(2**31) - 1, "greater_than_equal")],
    ids=["past-i32-max", "past-i32-min"],
)
def test_request_weights_refuses_values_outside_i32(
    out_of_range: int,
    expected_type: str,
) -> None:
    """`Request.weights` values outside `i32` fail at Pydantic naming `('weights', 'ana')`."""
    with pytest.raises(ValidationError) as raised:
        _models.Request.model_validate(
            {**THE_SAME_REQUEST_OMITTED, "weights": {"ana": out_of_range}}
        )

    finding = raised.value.errors(include_url=False)[0]
    assert finding["type"] == expected_type, finding
    assert finding["loc"] == ("weights", "ana"), finding


@pytest.mark.parametrize(
    ("weight", "expected_code"),
    [(2**31, "less_than_equal"), (-(2**31) - 1, "greater_than_equal")],
    ids=["past-i32-max", "past-i32-min"],
)
def test_an_out_of_range_weight_is_the_requests_finding_not_the_readers(
    weight: int,
    expected_code: str,
) -> None:
    """A forwarded document is refused at `/weights/{id}`, before the native reader.

    A weight is an `i32` on the wire, so a mapping carrying one outside that range
    used to cross the whole Python boundary and be refused by the reader. That
    rejection carried the same pointer, but as an `invalid_value` with an empty
    finding list and a message naming a byte column of the serialised text rather
    than the entry the client wrote. The model's own bound refuses it as the
    request instead: `invalid_request`, with the finding a caller reads the field
    from.
    """
    document = {**THE_SAME_REQUEST_OMITTED, "weights": {"ana": weight}}

    with pytest.raises(OpportuneError) as raised:
        Engine().validate(document)

    error = raised.value
    assert error.code == "invalid_request", error.message
    assert http_status_for_error(error) == 422
    assert [(finding.code, finding.path) for finding in error.errors] == [
        (expected_code, "/weights/ana")
    ], error.message
