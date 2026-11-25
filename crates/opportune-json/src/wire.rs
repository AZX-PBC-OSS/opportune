//! The JSON wire shapes: one type per object the format spells out.
//!
//! Wire types hold what the document holds, not what the domain means. A
//! duration is a string here and a [`Duration`](opportune::time::Duration) after
//! conversion; a block status is a string here, so an unrecognized one can still
//! be mapped, and a [`BlockStatus`](opportune::model::BlockStatus) after
//! conversion. That separation is what lets every rejection name the exact JSON
//! pointer it was reading, and what keeps parsing from ever changing what the
//! engine means.
//!
//! Every request struct denies unknown fields, no struct flattens another, and
//! every map that reaches the output is a `BTreeMap`, so two documents with the
//! same meaning serialize to the same bytes. The answer types — a response, a slot
//! check, a diagnostics report, a required-ranges envelope — are read back too, so
//! an HTTP client builds one from the engine's JSON; they accept a field they do
//! not name, because a service reading a newer engine's answer must not fail on an
//! additive field. The published schema still states the closure for the format it
//! describes.
//!
//! # Widths
//!
//! A count the document carries is read as `u64`, never as pointer-width
//! `usize`: a request that names four billion offers is the same document in a
//! browser and on a server, and neither may refuse it where the other accepts it.
//! Converting into the domain's `usize` saturates, so a 32-bit target treats an
//! unrepresentable count as "more than anything it can hold": an `explain.limit`
//! that large is still the configured-limits finding rather than a parse failure,
//! and an offer limit that large keeps every offer. Either way both targets give
//! the same answer.
//!
//! # Nulls
//!
//! Any field with a wire format default accepts an explicit `null` as the same
//! thing as leaving it out, so a binding that writes every key presents the same
//! document an omission does. The schema states it too: every such field carries
//! `#[schemars(transform = null_as_default)]` beside the `deserialize_with` that
//! implements it, which widens the field's own shape with the `null` arm the
//! deserializer reads.
//!
//! # Unions
//!
//! `Rule` is an internally tagged enum on `kind`, one named struct per kind. The
//! selector is dispatched by hand because it has a non-object form (`true`) and
//! its object forms are single-key, so a visitor can name the key that was wrong.
//! The requirement term and the `lead_time` maximum are untagged unions of named
//! structs: each form carries more than one key, and an untagged union accepts
//! those keys in any order, which a hand-written visitor reading them in order
//! would not.

use std::collections::BTreeMap;
use std::fmt;

use serde::de::{self, MapAccess, SeqAccess, Visitor};
use serde::ser::SerializeMap;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::primitives::{FALSE_SELECTOR, MatchAll};

/// A two-element array of strings: a span's start and end, or a blackout's
/// local date-times.
///
/// The JSON spells both as an array of two strings, so the wire reads both into
/// one shape and leaves the meaning to the field that holds it. The array is
/// read by hand rather than derived so that an array of the wrong length is one
/// `invalid_length` failure naming the shape a caller can write, rather than
/// serde's two spellings of the same mistake — one of which reports a
/// three-element array as a JSON syntax error.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct WireSpan(
    /// The span's first instant, or the local date-time it starts at.
    pub String,
    /// One past the span's last minute, or the local date-time it ends at.
    pub String,
);

impl<'de> Deserialize<'de> for WireSpan {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_tuple(2, SpanVisitor)
    }
}

/// Reads a span's two strings and rejects every other array length.
struct SpanVisitor;

impl<'de> Visitor<'de> for SpanVisitor {
    type Value = WireSpan;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("an array of two strings: a start and an end")
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
        let start = seq
            .next_element::<String>()?
            .ok_or_else(|| de::Error::invalid_length(0, &self))?;
        let end = seq
            .next_element::<String>()?
            .ok_or_else(|| de::Error::invalid_length(1, &self))?;
        if seq.next_element::<de::IgnoredAny>()?.is_some() {
            return Err(de::Error::invalid_length(3, &self));
        }
        Ok(WireSpan(start, end))
    }
}

/// A request to search for availability.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct WireRequest {
    /// The current instant, which the wire rounds up to the next minute.
    pub now: String,
    /// What is being booked.
    pub goal: WireGoal,
    /// Everyone the request knows about.
    #[serde(default, deserialize_with = "deserialize_null_default")]
    #[cfg_attr(feature = "schema", schemars(transform = null_as_default))]
    pub participants: Vec<WireParticipant>,
    /// The final rule list.
    #[serde(default, deserialize_with = "deserialize_null_default")]
    #[cfg_attr(feature = "schema", schemars(transform = null_as_default))]
    pub rules: Vec<WireRule>,
    /// The caller's bookings and sessions.
    #[serde(default, deserialize_with = "deserialize_null_default")]
    #[cfg_attr(feature = "schema", schemars(transform = null_as_default))]
    pub occurrences: Vec<WireOccurrence>,
    /// The caller's event keys for the events it is moving, which the engine
    /// ignores wherever an existing event would otherwise block time, trigger
    /// `rest` activity or count toward a cap.
    #[serde(
        default,
        skip_serializing_if = "Vec::is_empty",
        deserialize_with = "deserialize_null_default"
    )]
    #[cfg_attr(feature = "schema", schemars(transform = null_as_default))]
    pub exclude_event_keys: Vec<String>,
    /// Named working calendars, by name.
    #[serde(default, deserialize_with = "deserialize_null_default")]
    #[cfg_attr(feature = "schema", schemars(transform = null_as_default))]
    pub calendars: BTreeMap<String, WireCalendarSpec>,
    /// The weight each participant's assignment carries, by participant ID.
    #[serde(default, deserialize_with = "deserialize_null_default")]
    #[cfg_attr(feature = "schema", schemars(transform = null_as_default))]
    pub weights: BTreeMap<String, i32>,
    /// The maximum number of offers to return, or absent for the engine's default.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<u64>,
    /// Whether to report why candidate starts were not offered.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub explain: Option<WireExplain>,
}

/// How much exclusion detail a search returns.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct WireExplain {
    /// The maximum number of reasons returned across every excluded start.
    pub limit: u64,
}

/// What is being booked.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct WireGoal {
    /// The parts of the booking. Version 1 requires exactly one.
    pub parts: Vec<WirePart>,
    /// The grid candidate starts are drawn from, or absent for the default grid.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub grid: Option<WireGrid>,
    /// Caller-supplied candidate start instants, instead of a grid.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub candidates: Option<Vec<String>>,
    /// The range of starts to consider.
    pub search_window: WireSpan,
    /// The event zone.
    pub zone: String,
    /// The zone offers are grouped by date in, or absent for the event zone.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_zone: Option<String>,
}

/// A local clock grid of candidate starts.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct WireGrid {
    /// The step between candidate local minutes.
    pub step: String,
    /// The local minute each day's grid is anchored on, or absent for `00:00`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub anchor: Option<String>,
    /// The zone the grid's clock is read in, or absent for the event zone.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub zone: Option<String>,
}

/// One event within a goal.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct WirePart {
    /// The part's ID, or absent for `main`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// What is being booked.
    #[serde(default, deserialize_with = "deserialize_null_default")]
    #[cfg_attr(feature = "schema", schemars(transform = null_as_default))]
    pub tags: Vec<String>,
    /// The length of the event.
    pub duration: String,
    /// The terms that decide who can take it.
    pub requirement: WireRequirement,
    /// Travel the new booking needs, or absent for none.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub travel: Option<WireTravel>,
}

/// Who can take a part: every named term, filled by distinct hosts.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct WireRequirement {
    /// The terms every booking must satisfy.
    pub all: Vec<WireTerm>,
}

/// One clause of a requirement.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub enum WireTerm {
    /// One named host.
    Person(PersonTerm),
    /// At least a count of hosts among the participants a selector matches.
    AtLeast(AtLeastTerm),
}

/// A term filled by the participant with this ID.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct PersonTerm {
    /// The participant's ID.
    pub person: String,
}

/// A term needing a count of hosts among the participants a selector matches.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct AtLeastTerm {
    /// How many hosts the term needs.
    pub at_least: u32,
    /// Which participants can fill it.
    pub of: WireSelector,
}

/// Travel the new booking needs, in each direction, with per-participant
/// overrides.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct WireTravel {
    /// Travel needed before the event, for participants without an override.
    pub before: String,
    /// Travel needed after the event, for participants without an override.
    pub after: String,
    /// Per-participant travel, keyed by participant ID.
    #[serde(default, deserialize_with = "deserialize_null_default")]
    #[cfg_attr(feature = "schema", schemars(transform = null_as_default))]
    pub by_participant: BTreeMap<String, WireParticipantTravel>,
}

/// One participant's travel, replacing the part's travel in both directions.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct WireParticipantTravel {
    /// Travel needed before the event.
    pub before: String,
    /// Travel needed after the event.
    pub after: String,
}

/// A boolean selector over tags, participant IDs, roles and kinds.
///
/// The wire spells a selector as the bare `true` or as an object naming exactly
/// one of `tag`, `any`, `all`, `not`, `ids`, `role` or `kind`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WireSelector {
    /// Everything.
    MatchAll(MatchAll),
    /// The tag or one of its descendants.
    Tag(String),
    /// Anything at least one nested selector matches.
    Any(Vec<Self>),
    /// Everything every nested selector matches.
    All(Vec<Self>),
    /// Anything the nested selector does not match.
    Not(Box<Self>),
    /// Participants whose ID is in the list.
    Ids(Vec<String>),
    /// Participants with this role.
    Role(WireRole),
    /// Participants of this kind.
    Kind(WireParticipantKind),
}

/// The keys a selector object may name.
const SELECTOR_KEYS: &[&str] = &["tag", "any", "all", "not", "ids", "role", "kind"];

/// The sentence a selector object with no key, or with two, is rejected with.
const SELECTOR_SHAPE: &str = "a selector is `true` or an object naming exactly one of `tag`, `any`, `all`, `not`, `ids`, `role` and `kind`";

impl Serialize for WireSelector {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::MatchAll(match_all) => match_all.serialize(serializer),
            Self::Tag(tag) => one_entry(serializer, "tag", tag),
            Self::Any(selectors) => one_entry(serializer, "any", selectors),
            Self::All(selectors) => one_entry(serializer, "all", selectors),
            Self::Not(selector) => one_entry(serializer, "not", selector),
            Self::Ids(ids) => one_entry(serializer, "ids", ids),
            Self::Role(role) => one_entry(serializer, "role", role),
            Self::Kind(kind) => one_entry(serializer, "kind", kind),
        }
    }
}

/// Writes the single-key object `{ key: value }`.
fn one_entry<S: Serializer, T: Serialize + ?Sized>(
    serializer: S,
    key: &str,
    value: &T,
) -> Result<S::Ok, S::Error> {
    let mut map = serializer.serialize_map(Some(1))?;
    map.serialize_entry(key, value)?;
    map.end()
}

impl<'de> Deserialize<'de> for WireSelector {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(SelectorVisitor)
    }
}

/// Dispatches a selector on the one key its object names.
struct SelectorVisitor;

impl<'de> Visitor<'de> for SelectorVisitor {
    type Value = WireSelector;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(SELECTOR_SHAPE)
    }

    fn visit_bool<E: de::Error>(self, value: bool) -> Result<Self::Value, E> {
        if value {
            Ok(WireSelector::MatchAll(MatchAll))
        } else {
            Err(E::invalid_value(
                de::Unexpected::Bool(false),
                &FALSE_SELECTOR,
            ))
        }
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
        let Some(key) = map.next_key::<String>()? else {
            return Err(de::Error::custom(SELECTOR_SHAPE));
        };
        let selector = match key.as_str() {
            "tag" => WireSelector::Tag(map.next_value()?),
            "any" => WireSelector::Any(map.next_value()?),
            "all" => WireSelector::All(map.next_value()?),
            "not" => WireSelector::Not(map.next_value()?),
            "ids" => WireSelector::Ids(map.next_value()?),
            "role" => WireSelector::Role(map.next_value()?),
            "kind" => WireSelector::Kind(map.next_value()?),
            other => return Err(de::Error::unknown_field(other, SELECTOR_KEYS)),
        };
        if let Some(second) = map.next_key::<String>()? {
            return Err(de::Error::custom(format!(
                "a selector object names exactly one key, but `{second}` follows `{key}`",
            )));
        }
        Ok(selector)
    }
}

/// The schema of a selector: `true`, or an object naming exactly one key.
///
/// The forms are written by hand for the same reason the deserialiser is: the
/// key is the discriminant, and a derived schema would spell the union as seven
/// unconstrained objects rather than saying that each carries exactly one of
/// them.
#[cfg(feature = "schema")]
impl schemars::JsonSchema for WireSelector {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "WireSelector".into()
    }

    fn json_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        let keyed = [
            ("tag", "TagSelector", generator.subschema_for::<String>()),
            ("any", "AnySelector", generator.subschema_for::<Vec<Self>>()),
            ("all", "AllSelector", generator.subschema_for::<Vec<Self>>()),
            ("not", "NotSelector", generator.subschema_for::<Box<Self>>()),
            (
                "ids",
                "IdsSelector",
                generator.subschema_for::<Vec<String>>(),
            ),
            (
                "role",
                "RoleSelector",
                generator.subschema_for::<WireRole>(),
            ),
            (
                "kind",
                "KindSelector",
                generator.subschema_for::<WireParticipantKind>(),
            ),
        ];
        let mut forms = vec![generator.subschema_for::<MatchAll>()];
        forms.extend(
            keyed
                .into_iter()
                .map(|(key, title, value)| one_key(key, title, value)),
        );
        schemars::json_schema!({ "anyOf": forms })
    }
}

/// The schema of an object whose only key is `key`.
#[cfg(feature = "schema")]
fn one_key(key: &str, title: &str, value: schemars::Schema) -> schemars::Schema {
    let mut properties = serde_json::Map::new();
    properties.insert(key.to_owned(), value.to_value());
    // The title names the generated model for the form, so a binding builds a
    // selector as `TagSelector(tag=..)` rather than an anonymous numbered class.
    schemars::json_schema!({
        "type": "object",
        "title": title,
        "properties": properties,
        "required": [key],
        "additionalProperties": false,
    })
}

/// A participant's role in a request.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub enum WireRole {
    /// A participant assigned to bookings through the requirement.
    Host,
    /// A participant who must be free for every offer but is never assigned.
    Attendee,
}

/// Whether a participant is a person or a resource.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub enum WireParticipantKind {
    /// A person.
    Person,
    /// A room, a vehicle or a piece of equipment.
    Resource,
}

/// One participant and what is already on their calendar.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct WireParticipant {
    /// The participant's opaque, stable ID.
    pub id: String,
    /// Person or resource, or absent for a person.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<WireParticipantKind>,
    /// Host or attendee, or absent for a host.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<WireRole>,
    /// Everything the participant is tagged with.
    #[serde(default, deserialize_with = "deserialize_null_default")]
    #[cfg_attr(feature = "schema", schemars(transform = null_as_default))]
    pub tags: Vec<String>,
    /// The participant's home zone.
    pub zone: String,
    /// Spans in other zones.
    #[serde(default, deserialize_with = "deserialize_null_default")]
    #[cfg_attr(feature = "schema", schemars(transform = null_as_default))]
    pub zone_periods: Vec<WireZonePeriod>,
    /// The participant's blocks, or absent when no calendar was supplied.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub calendar: Option<Vec<WireBlock>>,
}

/// A span of instants during which a participant is in another zone.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct WireZonePeriod {
    /// The zone the participant is in during the period.
    pub zone: String,
    /// The first instant of the period.
    pub from: String,
    /// One past the last instant of the period.
    pub until: String,
}

/// How a block affects availability.
///
/// A block's status comes from a provider, so a document may hold a value this
/// layer has never seen: [`WireBlockStatus`]'s deserializer accepts any string,
/// and the converter reads any unrecognized value as `busy` (fail-closed). Its
/// schema is therefore an open string too, so a model generated from the schema
/// accepts a provider's own vocabulary instead of rejecting the document a real
/// calendar produced. A status-policy entry is caller-authored and takes the
/// closed [`WireStatus`] set instead.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WireBlockStatus(String);

impl WireBlockStatus {
    /// The status as the document wrote it.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<&str> for WireBlockStatus {
    fn from(status: &str) -> Self {
        Self(status.to_owned())
    }
}

impl Serialize for WireBlockStatus {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for WireBlockStatus {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        String::deserialize(deserializer).map(Self)
    }
}

/// States that a status is any string a provider sent, which the deserialiser
/// above reads and the converter fails safe on.
#[cfg(feature = "schema")]
impl schemars::JsonSchema for WireBlockStatus {
    fn inline_schema() -> bool {
        // Inlined so a document's `status` property carries the string directly,
        // rather than pointing at a definition a model generator has to name.
        true
    }

    fn schema_name() -> std::borrow::Cow<'static, str> {
        "WireBlockStatus".into()
    }

    fn json_schema(_generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({ "type": "string" })
    }
}

/// One span on a participant's calendar.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct WireBlock {
    /// The block's start and end instants, which round outward.
    pub span: WireSpan,
    /// How the block affects availability. A provider may report a status this
    /// crate does not know, and an unrecognised value reads as `busy`; absent
    /// means `busy`.
    #[serde(default = "default_status", deserialize_with = "deserialize_status")]
    #[cfg_attr(feature = "schema", schemars(transform = null_as_default))]
    pub status: WireBlockStatus,
    /// Whether the source marked the block all day.
    #[serde(default, deserialize_with = "deserialize_null_default")]
    #[cfg_attr(feature = "schema", schemars(transform = null_as_default))]
    pub all_day: bool,
    /// Everything the block is tagged with.
    #[serde(default, deserialize_with = "deserialize_null_default")]
    #[cfg_attr(feature = "schema", schemars(transform = null_as_default))]
    pub tags: Vec<String>,
    /// The caller's ID for the event this block is one instance of.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub event_key: Option<String>,
    /// Travel the caller computed to reach the block.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub travel_before: Option<String>,
    /// Travel the caller computed to leave the block.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub travel_after: Option<String>,
}

/// The status a block carries when the document does not say.
fn default_status() -> WireBlockStatus {
    WireBlockStatus("busy".to_owned())
}

/// The schema of a field the helpers below read an explicit `null` as the default
/// of: the field's own shape plus the `null` arm beside it, with the `default` and
/// the field's prose where a reader of the bundle expects them.
///
/// Applied through `#[schemars(transform = ...)]` on the same field as the
/// `deserialize_with` that implements the null tolerance, so the bundle states the
/// contract where the parser declares it rather than in a list kept elsewhere.
/// Every annotation schemars writes beside a field's shape — its default, its
/// prose — stays at the top level, and everything else describes the value and
/// belongs to the arm a `null` is the alternative to.
#[cfg(feature = "schema")]
fn null_as_default(schema: &mut schemars::Schema) {
    /// The keywords that annotate a field rather than describe the value it holds.
    const ANNOTATIONS: [&str; 5] = ["default", "description", "title", "deprecated", "examples"];

    let object = schema
        .as_object_mut()
        .expect("a derived field schema is an object");
    let mut annotated = serde_json::Map::new();
    let mut shape = serde_json::Map::new();
    for (keyword, value) in std::mem::take(object) {
        if ANNOTATIONS.contains(&keyword.as_str()) {
            annotated.insert(keyword, value);
        } else {
            shape.insert(keyword, value);
        }
    }
    annotated.insert(
        "anyOf".to_owned(),
        serde_json::json!([serde_json::Value::Object(shape), { "type": "null" }]),
    );
    *object = annotated;
}

/// Reads a field that defaults: an absent one and an explicit `null` both mean the
/// default, so a binding that writes every key presents the document an omission
/// does.
fn deserialize_null_default<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: Default + Deserialize<'de>,
{
    Ok(Option::<T>::deserialize(deserializer)?.unwrap_or_default())
}

/// Reads a block's `status`, which defaults to `busy`.
fn deserialize_status<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<WireBlockStatus, D::Error> {
    Ok(Option::<WireBlockStatus>::deserialize(deserializer)?.unwrap_or_else(default_status))
}

/// Reads a rule's `who`, which defaults to every host.
fn deserialize_who<'de, D: Deserializer<'de>>(deserializer: D) -> Result<WireSelector, D::Error> {
    Ok(Option::<WireSelector>::deserialize(deserializer)?.unwrap_or_else(default_who))
}

/// Reads a rule's `when`, which defaults to every booking.
fn deserialize_when<'de, D: Deserializer<'de>>(deserializer: D) -> Result<WireSelector, D::Error> {
    Ok(Option::<WireSelector>::deserialize(deserializer)?.unwrap_or_else(default_when))
}

/// One event the caller can identify, already expanded.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct WireOccurrence {
    /// The caller's ID for this event instance.
    pub event_key: String,
    /// When the event happens.
    pub span: WireSpan,
    /// Everything the event is tagged with.
    #[serde(default, deserialize_with = "deserialize_null_default")]
    #[cfg_attr(feature = "schema", schemars(transform = null_as_default))]
    pub tags: Vec<String>,
    /// The participants the event involves.
    pub participants: Vec<String>,
}

/// A named working calendar.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct WireCalendarSpec {
    /// The zone the calendar's civil dates are read in.
    pub zone: String,
    /// The weekdays the calendar works, written as day keys such as `mon-fri` or
    /// `mon,wed`.
    ///
    /// A count of working days needs a calendar that works at least one weekday:
    /// an absent list is the day-list syntax's spelling of "no working day",
    /// which the engine rejects with `working_days_empty`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub working_days: Option<String>,
    /// Dates that are not worked whatever the weekday.
    #[serde(default, deserialize_with = "deserialize_null_default")]
    #[cfg_attr(feature = "schema", schemars(transform = null_as_default))]
    pub holidays: Vec<String>,
}

/// One rule: an ID, which participants it constrains, which bookings it applies
/// to, and the kind-specific parameters.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub enum WireRule {
    /// When bookings may happen.
    Window(WindowRule),
    /// When bookings may not happen.
    Blackout(BlackoutRule),
    /// The gaps the new booking needs between meetings.
    Clearance(ClearanceRule),
    /// How soon and how far ahead a booking may start.
    LeadTime(LeadTimeRule),
    /// How statuses and tags affect availability.
    StatusPolicy(StatusPolicyRule),
    /// A late finish blocks an early start.
    Rest(RestRule),
    /// A limit on how much of a period may be booked.
    Cap(CapRule),
}

/// The default `who`: every host.
fn default_who() -> WireSelector {
    WireSelector::Role(WireRole::Host)
}

/// The default `when`: every booking.
fn default_when() -> WireSelector {
    WireSelector::MatchAll(MatchAll)
}

/// A `window` rule: when bookings may happen.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct WindowRule {
    /// The rule's ID, unique in the request.
    pub id: String,
    /// Which participants the rule constrains, or every host by default.
    #[serde(default = "default_who", deserialize_with = "deserialize_who")]
    #[cfg_attr(feature = "schema", schemars(transform = null_as_default))]
    pub who: WireSelector,
    /// Which bookings it applies to, or all of them by default.
    #[serde(default = "default_when", deserialize_with = "deserialize_when")]
    #[cfg_attr(feature = "schema", schemars(transform = null_as_default))]
    pub when: WireSelector,
    /// The window specification.
    pub params: WindowParams,
}

/// The parameters of a `window` rule.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct WindowParams {
    /// `participant`, `event`, or an IANA zone name.
    pub zone: String,
    /// The hours each day key shares, keyed by a weekday list or range.
    #[serde(default, deserialize_with = "deserialize_null_default")]
    #[cfg_attr(feature = "schema", schemars(transform = null_as_default))]
    pub weekly: BTreeMap<String, Vec<String>>,
    /// Hours that override the weekly ones, keyed by a date or date range.
    #[serde(default, deserialize_with = "deserialize_null_default")]
    #[cfg_attr(feature = "schema", schemars(transform = null_as_default))]
    pub dates: BTreeMap<String, Vec<String>>,
}

/// A `blackout` rule: when bookings may not happen.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct BlackoutRule {
    /// The rule's ID, unique in the request.
    pub id: String,
    /// Which participants the rule constrains, or every host by default.
    #[serde(default = "default_who", deserialize_with = "deserialize_who")]
    #[cfg_attr(feature = "schema", schemars(transform = null_as_default))]
    pub who: WireSelector,
    /// Which bookings it applies to, or all of them by default.
    #[serde(default = "default_when", deserialize_with = "deserialize_when")]
    #[cfg_attr(feature = "schema", schemars(transform = null_as_default))]
    pub when: WireSelector,
    /// The blackout specification.
    pub params: BlackoutParams,
}

/// The parameters of a `blackout` rule.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct BlackoutParams {
    /// `participant`, `event`, or an IANA zone name.
    pub zone: String,
    /// Local date-time spans inside which nothing may be booked.
    pub spans: Vec<WireSpan>,
}

/// A `clearance` rule: the gaps a booking needs between itself and other events.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ClearanceRule {
    /// The rule's ID, unique in the request.
    pub id: String,
    /// Which participants the rule constrains, or every host by default.
    #[serde(default = "default_who", deserialize_with = "deserialize_who")]
    #[cfg_attr(feature = "schema", schemars(transform = null_as_default))]
    pub who: WireSelector,
    /// Which bookings it applies to, or all of them by default.
    #[serde(default = "default_when", deserialize_with = "deserialize_when")]
    #[cfg_attr(feature = "schema", schemars(transform = null_as_default))]
    pub when: WireSelector,
    /// The clearance parameters.
    pub params: ClearanceParams,
}

/// The parameters of a `clearance` rule.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ClearanceParams {
    /// The gap the booking needs before it.
    pub before: String,
    /// The gap the booking needs after it.
    pub after: String,
    /// The least gap between any two events.
    pub min_gap: String,
}

/// A `lead_time` rule: how soon and how far ahead a booking may start.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct LeadTimeRule {
    /// The rule's ID, unique in the request.
    pub id: String,
    /// Which participants the rule constrains, or every host by default.
    #[serde(default = "default_who", deserialize_with = "deserialize_who")]
    #[cfg_attr(feature = "schema", schemars(transform = null_as_default))]
    pub who: WireSelector,
    /// Which bookings it applies to, or all of them by default.
    #[serde(default = "default_when", deserialize_with = "deserialize_when")]
    #[cfg_attr(feature = "schema", schemars(transform = null_as_default))]
    pub when: WireSelector,
    /// The lead-time parameters.
    pub params: LeadTimeParams,
}

/// The parameters of a `lead_time` rule.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct LeadTimeParams {
    /// The minimum notice, measured from the booking's occupied start.
    pub min: String,
    /// The latest start allowed, or absent for none beyond the search window.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max: Option<WireLeadTimeMax>,
}

/// A `lead_time` maximum: how far ahead a start may be.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub enum WireLeadTimeMax {
    /// At most `now` plus a duration.
    Duration(DurationMax),
    /// At most a count of calendar days after today.
    Days(DaysMax),
    /// At most the n-th working day after today, on a named calendar.
    WorkingDays(WorkingDaysMax),
    /// At most this local date in the event zone.
    Date(DateMax),
}

/// A `lead_time` maximum written as a duration.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct DurationMax {
    /// How far ahead a start may be.
    pub duration: String,
}

/// A `lead_time` maximum written as a count of calendar days.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct DaysMax {
    /// How many calendar days after today a start may fall on.
    pub days: u32,
}

/// A `lead_time` maximum written as a count of working days on a calendar.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct WorkingDaysMax {
    /// How many working days after today remain inside the horizon.
    pub working_days: u32,
    /// The request calendar that decides which days are working days.
    pub calendar: String,
}

/// A `lead_time` maximum written as a date in the event zone.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct DateMax {
    /// The last local date a start may fall on.
    pub date: String,
}

/// A `status_policy` rule: how statuses and tags affect availability.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct StatusPolicyRule {
    /// The rule's ID, unique in the request.
    pub id: String,
    /// Which participants the rule constrains, or every host by default.
    #[serde(default = "default_who", deserialize_with = "deserialize_who")]
    #[cfg_attr(feature = "schema", schemars(transform = null_as_default))]
    pub who: WireSelector,
    /// Which bookings it applies to, or all of them by default.
    #[serde(default = "default_when", deserialize_with = "deserialize_when")]
    #[cfg_attr(feature = "schema", schemars(transform = null_as_default))]
    pub when: WireSelector,
    /// The policy parameters.
    pub params: StatusPolicyParams,
}

/// The parameters of a `status_policy` rule.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct StatusPolicyParams {
    /// The entries, in the order the caller wrote them.
    pub entries: Vec<WireStatusPolicyEntry>,
}

/// One status-policy entry: what it matches and what it decides.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct WireStatusPolicyEntry {
    /// The block status this entry matches, if it matches on status.
    ///
    /// Unlike a block's `status`, this one is a closed set. A block's status is
    /// provider data, where an unrecognised value has to fail safe toward busy;
    /// an entry is caller-authored configuration, where an unrecognised value is
    /// a mistake. Reading one as `busy` would turn an entry meant to ignore one
    /// kind of event into one that ignores every busy block.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<WireStatus>,
    /// The tag selector this entry matches, if it matches on tags.
    #[serde(rename = "match", skip_serializing_if = "Option::is_none")]
    pub matched: Option<WireSelector>,
    /// What happens to a block the entry matches.
    pub effect: WireStatusEffect,
}

/// The statuses a status-policy entry may name.
///
/// A block's status is read as an open string, because a provider may report a
/// value this layer has never seen and `busy` is the safe reading of it. An entry
/// is the caller's own configuration, so a value outside this set is rejected
/// where it is written rather than silently matched against something else.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub enum WireStatus {
    /// Ignored by default: the participant is available.
    Free,
    /// Blocks by default.
    Tentative,
    /// Blocks by default.
    Busy,
}

/// What a matching status-policy entry does to a block.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub enum WireStatusEffect {
    /// The block is ignored.
    Ignore,
    /// The block blocks.
    Block,
}

/// A `rest` rule: a late finish blocks an early start.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct RestRule {
    /// The rule's ID, unique in the request.
    pub id: String,
    /// Which participants the rule constrains, or every host by default.
    #[serde(default = "default_who", deserialize_with = "deserialize_who")]
    #[cfg_attr(feature = "schema", schemars(transform = null_as_default))]
    pub who: WireSelector,
    /// Which bookings it applies to, or all of them by default.
    #[serde(default = "default_when", deserialize_with = "deserialize_when")]
    #[cfg_attr(feature = "schema", schemars(transform = null_as_default))]
    pub when: WireSelector,
    /// The rest parameters.
    pub params: RestParams,
}

/// The parameters of a `rest` rule.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct RestParams {
    /// The local time from which a finish counts as late.
    pub late_after: String,
    /// The local time before which a start counts as early.
    pub not_before: String,
}

/// A `cap` rule: a limit on how much of a civil period may be booked.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct CapRule {
    /// The rule's ID, unique in the request.
    pub id: String,
    /// Which participants the rule constrains, or every host by default.
    #[serde(default = "default_who", deserialize_with = "deserialize_who")]
    #[cfg_attr(feature = "schema", schemars(transform = null_as_default))]
    pub who: WireSelector,
    /// Which bookings it applies to, or all of them by default.
    #[serde(default = "default_when", deserialize_with = "deserialize_when")]
    #[cfg_attr(feature = "schema", schemars(transform = null_as_default))]
    pub when: WireSelector,
    /// The cap parameters.
    pub params: CapParams,
}

/// The parameters of a `cap` rule.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct CapParams {
    /// Which events the cap measures, matched against their tags.
    pub count: WireSelector,
    /// Whether events or their minutes count.
    pub measure: WireCapMeasure,
    /// The most the period may hold.
    pub limit: u32,
    /// The civil period a bucket covers.
    pub period: WireCapPeriod,
    /// Whether the limit applies to each participant or to the pool together.
    pub per: WireCapPer,
    /// The zone the bucket labels are read in.
    pub zone: String,
    /// The weekday a week bucket starts on, or absent for Monday.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub week_starts: Option<String>,
}

/// What a `cap` measures.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub enum WireCapMeasure {
    /// Each matching event counts once.
    Count,
    /// Each matching event contributes its own duration.
    Minutes,
}

/// The civil period a `cap` bucket covers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub enum WireCapPeriod {
    /// One civil day.
    Day,
    /// Seven civil days from the configured weekday.
    Week,
    /// One civil month.
    Month,
}

/// Whether a `cap` limits each participant or the pool together.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub enum WireCapPer {
    /// Each participant in `who` is limited on their own.
    Each,
    /// The participants in `who` are limited together.
    Pooled,
}

/// A booking a caller proposes to confirm.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct WireProposal {
    /// One proposed event per goal part.
    pub events: Vec<WireProposalEvent>,
}

/// One event of a [`WireProposal`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct WireProposalEvent {
    /// The goal part's ID.
    pub part: String,
    /// The proposed start.
    pub start: String,
    /// The hosts to assign, one list per term.
    pub assignment: Vec<Vec<String>>,
}

/// What calendar and occurrence data a request needs.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct WireRangesQuery {
    /// The current instant, which the wire rounds up to the next minute.
    pub now: String,
    /// What is being booked.
    pub goal: WireGoal,
    /// The final rule list.
    #[serde(default, deserialize_with = "deserialize_null_default")]
    #[cfg_attr(feature = "schema", schemars(transform = null_as_default))]
    pub rules: Vec<WireRule>,
    /// The participants, without their calendars.
    #[serde(default, deserialize_with = "deserialize_null_default")]
    #[cfg_attr(feature = "schema", schemars(transform = null_as_default))]
    pub participants: Vec<WireParticipant>,
}

/// Configurable caps on request size.
///
/// Every field is optional: an absent field — or an explicit `null`, which a
/// binding that writes every key emits — is the engine's default for that cap,
/// `Limits::default`. Writing a `Limits` back out states all ten, so one
/// document carries the engine's whole configuration.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct WireLimits {
    /// Maximum number of participants in one request, or absent for the engine
    /// default.
    #[serde(default)]
    pub max_participants: Option<u64>,
    /// Maximum number of blocks per participant, or absent for the engine
    /// default.
    #[serde(default)]
    pub max_blocks_per_participant: Option<u64>,
    /// Maximum search-window length in days, or absent for the engine default.
    ///
    /// A day count is already `u32` in the domain and no pointer width is
    /// involved, so it is read as one here too: a larger value is rejected rather
    /// than saturated into a limit no calendar could satisfy.
    #[serde(default)]
    pub max_search_window_days: Option<u32>,
    /// Maximum number of candidate starts in one search, or absent for the engine
    /// default.
    #[serde(default)]
    pub max_candidate_starts: Option<u64>,
    /// Maximum `explain.limit` a request may ask for, or absent for the engine
    /// default.
    #[serde(default)]
    pub max_explain_limit: Option<u64>,
    /// Maximum number of rules in one request, or absent for the engine default.
    #[serde(default)]
    pub max_rules: Option<u64>,
    /// Maximum number of occurrences in one request, or absent for the engine
    /// default.
    #[serde(default)]
    pub max_occurrences: Option<u64>,
    /// Maximum number of host slots one requirement may ask for, and the number of
    /// terms it may name, or absent for the engine default.
    #[serde(default)]
    pub max_requirement_slots: Option<u64>,
    /// Maximum length of a booking, a booking or block's travel or a clearance in
    /// minutes, or absent for the engine default.
    #[serde(default)]
    pub max_duration_minutes: Option<u32>,
    /// Maximum number of zone periods one participant may carry, or absent for the
    /// engine default.
    #[serde(default)]
    pub max_zone_periods_per_participant: Option<u64>,
}

/// What a search returns.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct WireResponse {
    /// The bookable times, in start order.
    pub offers: Vec<WireOffer>,
    /// Every civil date the search window touches, with its offer count.
    pub dates: Vec<WireDateCount>,
    /// Whether the offer limit cut the list short.
    pub truncated: bool,
    /// The candidate starts that were not offered.
    pub excluded: Vec<WireExcluded>,
    /// Whether the explanation limit cut the exclusion list short.
    pub excluded_truncated: bool,
    /// Request warnings.
    pub warnings: Vec<WireDiagnostic>,
    /// The versions the answer was produced from.
    pub meta: WireMeta,
}

/// One bookable time.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct WireOffer {
    /// One event per goal part.
    pub events: Vec<WireOfferEvent>,
    /// What the caller should know about the offer.
    pub flags: Vec<String>,
}

/// One booked event: its part, its span, who could take it and who was chosen.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct WireOfferEvent {
    /// The goal part's ID.
    pub part: String,
    /// The booking's start.
    pub start: String,
    /// The booking's end.
    pub end: String,
    /// Who could fill each term, one list per term.
    pub eligible: Vec<Vec<String>>,
    /// The chosen assignment, one list per term.
    pub assignment: Vec<Vec<String>>,
    /// Travel every participant the booking needs occupies.
    pub reserve: Vec<WireReserveSpan>,
}

/// One span of time the caller should hold on a participant's calendar.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct WireReserveSpan {
    /// The participant to hold it for.
    pub participant: String,
    /// What the hold is for.
    pub kind: String,
    /// The span to hold.
    pub span: WireSpan,
}

/// How many offers fall on one civil date.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct WireDateCount {
    /// The date in the display zone.
    pub date: String,
    /// How many offers start on it.
    pub offers: u64,
}

/// One candidate start that was not offered.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct WireExcluded {
    /// The start that was not offered.
    pub start: String,
    /// The goal part the start belonged to.
    pub part: String,
    /// Why the start was not offered.
    pub reasons: Vec<WireReason>,
}

/// One reason a participant cannot take a candidate start.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct WireReason {
    /// The participant the reason applies to, or absent for a start-level reason.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub participant: Option<String>,
    /// Why the start was excluded.
    pub code: String,
    /// The rule that produced the exclusion, when an explicit rule applies.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rule: Option<String>,
    /// The event key of the block that caused the exclusion, when a block did.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub event_key: Option<String>,
    /// The tags of the block that caused the exclusion, when a block did.
    pub tags: Vec<String>,
    /// The structured parameters the code carries, when it carries any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<WireReasonParams>,
}

/// The structured parameters a [`WireReason`] carries.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct WireReasonParams {
    /// How much the bucket holds before the booking's contribution.
    pub used: u64,
    /// The cap's limit.
    pub limit: u32,
    /// The bucket a cap was measured over.
    pub bucket: String,
}

/// One validation finding.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct WireDiagnostic {
    /// The stable, machine-readable code.
    pub code: String,
    /// The human-readable message.
    pub message: String,
    /// The RFC 6901 pointer of the request field this finding is about.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

/// The versions an answer was produced from.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct WireMeta {
    /// The engine's version.
    pub engine: String,
    /// The schema version of the response shape.
    pub schema: u32,
    /// The time zone database's version.
    pub tzdb: String,
}

/// What `validate` found in a request.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct WireDiagnostics {
    /// Everything that makes the request impossible.
    pub errors: Vec<WireDiagnostic>,
    /// Everything the caller should know that does not make it impossible.
    pub warnings: Vec<WireDiagnostic>,
}

/// The answer to checking one proposed booking.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct WireSlotCheck {
    /// Whether the booking can be made as proposed.
    pub valid: bool,
    /// Every reason the booking fails.
    pub reasons: Vec<WireReason>,
    /// What the caller should know that does not make it invalid.
    pub flags: Vec<String>,
    /// Travel every participant the booking needs occupies.
    pub reserve: Vec<WireReserveSpan>,
    /// The request's validation warnings.
    pub warnings: Vec<WireDiagnostic>,
    /// The versions the answer was produced from.
    pub meta: WireMeta,
}

/// One UTC envelope of data a request needs.
///
/// The hulls begin a two-phase fetch rather than a one-pass query: a cap merges
/// every record sharing an event key across all supplied time ranges. Fetch the
/// initial ranges, collect every event key observed on each participant's
/// own-span occurrence-range records, then fetch every record carrying one of
/// those keys — on every participant, all-day and otherwise nonmatching records
/// included — and evaluate that closed request against a consistent snapshot. A
/// one-pass fetch is key-complete only when every copy of an event has the
/// identical own span; otherwise fetch the full supported time domain or fail
/// closed. See the Rust `RequiredRanges` documentation for the whole contract.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct WireRequiredRanges {
    /// The UTC range every participant's calendar must cover.
    pub calendar: WireSpan,
    /// The UTC range of the initial occurrence-range fetch, or absent when no cap
    /// applies.
    ///
    /// Fetch every occurrence and calendar block whose own span overlaps it, then
    /// apply the one-round event-key closure described on this type. An omitted
    /// field and an explicit `null` both read as no occurrence range; the field is
    /// omitted rather than written as `null`, matching every other optional
    /// response field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub occurrences: Option<WireSpan>,
}
