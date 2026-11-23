//! Hierarchical, case-sensitive tags and boolean selectors over them.
//!
//! A [`Tag`] is a `/`-separated path of segments built from letters, digits,
//! `-`, `_` and `.`, validated once at construction. A tag implies its
//! ancestors: `interview/front-end` matches `interview`, so a rule keyed on
//! the parent applies to everything filed under it. Tags are case-sensitive,
//! so directory IDs can be used as they are.
//!
//! A [`Selector`] is a boolean combination of tag, participant-ID, role and kind
//! matches, evaluated against either an item's tags ([`Selector::matches_item`])
//! or a participant's ID, role, kind and tags
//! ([`Selector::matches_participant`]).
//!
//! ```
//! use opportune::model::ParticipantKind;
//! use opportune::tags::{Role, Selector, Tag};
//!
//! let selector = Selector::Tag(Tag::new("interview").unwrap());
//! let item = [Tag::new("interview/front-end").unwrap()];
//! assert!(selector.matches_item(&item));
//!
//! let attendee = opportune::tags::ParticipantContext {
//!     id: "tom",
//!     kind: ParticipantKind::Person,
//!     role: Role::Attendee,
//!     tags: &item,
//! };
//! assert!(selector.matches_participant(attendee));
//! ```

use std::collections::BTreeSet;

use thiserror::Error;

/// A tag path: `/`-separated segments of letters, digits, `-`, `_` and `.`.
///
/// Construct through [`Tag::new`], which validates the path once; a valid
/// [`Tag`] is therefore always well-formed, and matching compares validated
/// values only.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Tag(String);

impl Tag {
    /// Validates and creates a tag path.
    ///
    /// Segments are separated by `/`; leading, trailing and doubled
    /// separators are empty segments and are rejected, as are disallowed
    /// characters and the empty path. Matching is case-sensitive, so
    /// `interview/Front-End` and `interview/front-end` are different tags.
    ///
    /// ```
    /// # use opportune::tags::Tag;
    /// let tag = Tag::new("interview/front-end").unwrap();
    /// assert_eq!(tag.as_str(), "interview/front-end");
    /// assert_eq!(tag.segments().collect::<Vec<_>>(), ["interview", "front-end"]);
    /// assert!(Tag::new("a.b_c-9").is_ok());
    /// assert!(Tag::new("interview/").is_err());
    /// ```
    ///
    /// # Errors
    /// Returns [`TagError::Empty`] for the empty path,
    /// [`TagError::EmptySegment`] for a leading, trailing or doubled `/`, and
    /// [`TagError::InvalidCharacter`] for a character outside the segment
    /// alphabet.
    pub fn new(tag: &str) -> Result<Self, TagError> {
        validate(tag)?;
        Ok(Self(tag.to_owned()))
    }

    /// The validated tag path.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The path's segments, in order.
    pub fn segments(&self) -> impl Iterator<Item = &str> {
        self.0.split('/')
    }

    /// Whether `descendant` is this tag or a tag filed under it: `interview`
    /// matches `interview` and `interview/front-end`, but neither
    /// `Interview` nor `interview/front` (a sibling, not a prefix).
    #[must_use]
    pub fn is_ancestor_or_self_of(&self, descendant: &Tag) -> bool {
        descendant
            .0
            .strip_prefix(self.0.as_str())
            .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
    }
}

impl core::fmt::Display for Tag {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(&self.0)
    }
}

impl TryFrom<&str> for Tag {
    type Error = TagError;

    fn try_from(tag: &str) -> Result<Self, TagError> {
        Self::new(tag)
    }
}

impl TryFrom<String> for Tag {
    type Error = TagError;

    fn try_from(tag: String) -> Result<Self, TagError> {
        validate(&tag)?;
        Ok(Self(tag))
    }
}

fn validate(tag: &str) -> Result<(), TagError> {
    if tag.is_empty() {
        return Err(TagError::Empty);
    }
    for (segment_index, segment) in tag.split('/').enumerate() {
        if segment.is_empty() {
            return Err(TagError::EmptySegment { segment_index });
        }
        for character in segment.chars() {
            if !matches!(character, 'a'..='z' | 'A'..='Z' | '0'..='9' | '-' | '_' | '.') {
                return Err(TagError::InvalidCharacter {
                    segment_index,
                    character,
                });
            }
        }
    }
    Ok(())
}

/// Why a tag path was rejected.
///
/// The [`TagError::code`] values are stable: callers match on them across
/// releases.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[non_exhaustive]
pub enum TagError {
    /// The tag path was empty.
    #[error("a tag must not be empty")]
    Empty,
    /// A `/`-separated segment was empty: no leading, trailing or doubled `/`.
    #[error("tag segment {segment_index} is empty: no leading, trailing or doubled `/`")]
    EmptySegment {
        /// The position of the empty segment, counting from 0.
        segment_index: usize,
    },
    /// A segment held a character outside the tag alphabet.
    #[error(
        "tag segment {segment_index} contains `{character}`, which is not a letter, digit, `-`, `_` or `.`"
    )]
    InvalidCharacter {
        /// The position of the offending segment, counting from 0.
        segment_index: usize,
        /// The character outside the tag alphabet.
        character: char,
    },
}

impl TagError {
    /// A stable, machine-readable code for this failure.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::Empty => "tag_empty",
            Self::EmptySegment { .. } => "tag_empty_segment",
            Self::InvalidCharacter { .. } => "tag_invalid_character",
        }
    }
}

/// A participant's role in a request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Role {
    /// A participant assigned to bookings through the requirement.
    Host,
    /// A participant who must be free for every offer but is never assigned.
    Attendee,
}

impl Role {
    /// The role's wire name: `host` or `attendee`.
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Host => "host",
            Self::Attendee => "attendee",
        }
    }
}

impl core::fmt::Display for Role {
    /// Formats the role as its wire name.
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Whether a participant is a person or a resource such as a room or a vehicle.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ParticipantKind {
    /// A person.
    #[default]
    Person,
    /// A resource: a room, a vehicle, a piece of equipment.
    Resource,
}

impl ParticipantKind {
    /// The kind's wire name: `person` or `resource`.
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Person => "person",
            Self::Resource => "resource",
        }
    }
}

impl core::fmt::Display for ParticipantKind {
    /// Formats the kind as its wire name.
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The context one participant presents to a selector.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParticipantContext<'a> {
    /// The participant's opaque, stable ID, matched case-sensitively.
    pub id: &'a str,
    /// Whether the participant is a person or a resource.
    pub kind: ParticipantKind,
    /// The participant's role in the request.
    pub role: Role,
    /// The tags the participant carries.
    pub tags: &'a [Tag],
}

/// A boolean selector over tags, participant IDs, roles and kinds.
///
/// Evaluate against an item's tags with [`Selector::matches_item`], or
/// against a participant's ID, kind, role and tags with
/// [`Selector::matches_participant`]. The participant-only variants
/// ([`Selector::Ids`], [`Selector::Role`] and [`Selector::Kind`]) match no item:
/// they select participants, and validation restricts them to participant
/// selectors such as `who` clauses. Because
/// they match no item, a [`Selector::Not`] around any of them matches every item,
/// so a clause evaluated against item tags should avoid all three.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Selector {
    /// Matches items or participants carrying this tag or a descendant of it.
    Tag(Tag),
    /// Matches when any nested selector matches. An empty `any` matches
    /// nothing.
    Any(Vec<Selector>),
    /// Matches when every nested selector matches. An empty `all` matches
    /// everything.
    All(Vec<Selector>),
    /// Matches when the nested selector does not.
    Not(Box<Selector>),
    /// Matches participants whose ID is in the list, case-sensitively.
    /// Matches no item.
    Ids(Vec<String>),
    /// Matches participants with this role. Matches no item.
    Role(Role),
    /// Matches participants of this kind, so a human-only rule never reaches a
    /// room or a vehicle. Matches no item.
    Kind(ParticipantKind),
    /// Matches everything.
    True,
}

impl core::ops::Not for Selector {
    type Output = Self;

    /// The negation of a selector, so `!Selector::Tag(tag)` says the same thing
    /// as [`Selector::not`].
    fn not(self) -> Self::Output {
        Self::Not(Box::new(self))
    }
}

impl Selector {
    /// A selector matching the tag or one of its descendants.
    ///
    /// ```
    /// use opportune::model::ParticipantKind;
    /// use opportune::tags::{ParticipantContext, Role, Selector, Tag};
    ///
    /// let hosts = Selector::all([
    ///     Selector::tag(Tag::new("team/sales").unwrap()),
    ///     Selector::not(Selector::role(Role::Attendee)),
    /// ]);
    /// let selected = Selector::any([hosts, Selector::ids(["tom"])]);
    /// let tags = [Tag::new("team/sales/emea").unwrap()];
    /// let tom = ParticipantContext {
    ///     id: "tom",
    ///     kind: ParticipantKind::Person,
    ///     role: Role::Host,
    ///     tags: &tags,
    /// };
    /// assert!(selected.matches_participant(tom));
    /// ```
    #[must_use]
    pub fn tag(tag: Tag) -> Self {
        Self::Tag(tag)
    }

    /// A selector matching participants whose ID is in `ids`.
    #[must_use]
    pub fn ids(ids: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Self::Ids(ids.into_iter().map(Into::into).collect())
    }

    /// A selector matching when any of `selectors` matches. Empty matches
    /// nothing.
    #[must_use]
    pub fn any(selectors: impl IntoIterator<Item = Self>) -> Self {
        Self::Any(selectors.into_iter().collect())
    }

    /// A selector matching when every one of `selectors` matches. Empty matches
    /// everything.
    #[must_use]
    pub fn all(selectors: impl IntoIterator<Item = Self>) -> Self {
        Self::All(selectors.into_iter().collect())
    }

    /// A selector matching when `selector` does not.
    // Clippy asks for `std::ops::Not`; the `Not` impl exists as well, but the
    // constructor is the name a caller building a selector reads.
    #[allow(clippy::should_implement_trait)]
    #[must_use]
    pub fn not(selector: Self) -> Self {
        Self::Not(Box::new(selector))
    }
    /// A selector matching participants with this role.
    #[must_use]
    pub fn role(role: Role) -> Self {
        Self::Role(role)
    }

    /// A selector matching participants of this kind.
    ///
    /// ```
    /// use opportune::model::ParticipantKind;
    /// use opportune::tags::{ParticipantContext, Role, Selector};
    ///
    /// let people = Selector::kind(ParticipantKind::Person);
    /// let room = ParticipantContext {
    ///     id: "room-4",
    ///     kind: ParticipantKind::Resource,
    ///     role: Role::Host,
    ///     tags: &[],
    /// };
    /// assert!(!people.matches_participant(room));
    /// assert!(!people.matches_item(&[]));
    /// ```
    #[must_use]
    pub fn kind(kind: ParticipantKind) -> Self {
        Self::Kind(kind)
    }

    /// Whether an item carrying `tags` is selected.
    ///
    /// ```
    /// # use opportune::tags::{Selector, Tag};
    /// let any = Selector::Any(vec![
    ///     Selector::Tag(Tag::new("status/tentative").unwrap()),
    ///     Selector::Tag(Tag::new("veto/crm-freeze").unwrap()),
    /// ]);
    /// let blocked = [Tag::new("veto/crm-freeze").unwrap()];
    /// assert!(any.matches_item(&blocked));
    ///
    /// // `ids`, `role` and `kind` select participants, never items:
    /// let ids = Selector::Ids(vec!["tom".to_owned()]);
    /// assert!(!ids.matches_item(&blocked));
    /// ```
    #[must_use]
    pub fn matches_item(&self, tags: &[Tag]) -> bool {
        match self {
            Self::Tag(tag) => tags.iter().any(|item| tag.is_ancestor_or_self_of(item)),
            Self::Any(selectors) => selectors.iter().any(|s| s.matches_item(tags)),
            Self::All(selectors) => selectors.iter().all(|s| s.matches_item(tags)),
            Self::Not(selector) => !selector.matches_item(tags),
            Self::Ids(_) | Self::Role(_) | Self::Kind(_) => false,
            Self::True => true,
        }
    }

    /// Whether the participant is selected.
    ///
    /// ```
    /// # use opportune::model::ParticipantKind;
    /// # use opportune::tags::{ParticipantContext, Role, Selector, Tag};
    /// let selector = Selector::All(vec![
    ///     Selector::Tag(Tag::new("interview").unwrap()),
    ///     Selector::Not(Box::new(Selector::Role(Role::Host))),
    /// ]);
    /// let tags = [Tag::new("interview/front-end").unwrap()];
    /// let attendee = ParticipantContext {
    ///     id: "tom",
    ///     kind: ParticipantKind::Person,
    ///     role: Role::Attendee,
    ///     tags: &tags,
    /// };
    /// assert!(selector.matches_participant(attendee));
    ///
    /// let host = ParticipantContext {
    ///     id: "ana",
    ///     kind: ParticipantKind::Person,
    ///     role: Role::Host,
    ///     tags: &tags,
    /// };
    /// assert!(!selector.matches_participant(host));
    /// ```
    #[must_use]
    pub fn matches_participant(&self, participant: ParticipantContext<'_>) -> bool {
        match self {
            Self::Tag(tag) => participant
                .tags
                .iter()
                .any(|item| tag.is_ancestor_or_self_of(item)),
            Self::Any(selectors) => selectors.iter().any(|s| s.matches_participant(participant)),
            Self::All(selectors) => selectors.iter().all(|s| s.matches_participant(participant)),
            Self::Not(selector) => !selector.matches_participant(participant),
            Self::Ids(ids) => ids.iter().any(|id| id == participant.id),
            Self::Role(role) => *role == participant.role,
            Self::Kind(kind) => *kind == participant.kind,
            Self::True => true,
        }
    }
}

/// A selector prepared for repeated matching against one request's participants.
///
/// Every `ids` clause is answered from a [`BTreeSet`], which is built once when
/// the request prepares its rules. Matching then costs a binary search per clause
/// instead of a scan of the clause's list, so a request whose selector names many
/// IDs no longer pays `participants × ids` string comparisons: a search evaluates
/// each rule's `who` once per participant, and this keeps that product down to
/// `participants × log(ids)`.
#[derive(Debug)]
pub(crate) enum PreparedSelector<'a> {
    /// The tag or one of its descendants.
    Tag(&'a Tag),
    /// Anything at least one nested selector matches.
    Any(Vec<Self>),
    /// Everything every nested selector matches.
    All(Vec<Self>),
    /// Anything the nested selector does not match.
    Not(Box<Self>),
    /// Participants whose ID is in the set.
    Ids(BTreeSet<&'a str>),
    /// Participants with this role.
    Role(Role),
    /// Participants of this kind.
    Kind(ParticipantKind),
    /// Everything.
    True,
}

impl<'a> From<&'a Selector> for PreparedSelector<'a> {
    fn from(selector: &'a Selector) -> Self {
        match selector {
            Selector::Tag(tag) => Self::Tag(tag),
            Selector::Any(selectors) => Self::Any(selectors.iter().map(Self::from).collect()),
            Selector::All(selectors) => Self::All(selectors.iter().map(Self::from).collect()),
            Selector::Not(selector) => Self::Not(Box::new(Self::from(selector.as_ref()))),
            Selector::Ids(ids) => Self::Ids(ids.iter().map(String::as_str).collect()),
            Selector::Role(role) => Self::Role(*role),
            Selector::Kind(kind) => Self::Kind(*kind),
            Selector::True => Self::True,
        }
    }
}

impl PreparedSelector<'_> {
    /// Whether the participant is selected.
    ///
    /// It answers exactly what [`Selector::matches_participant`] answers; only the
    /// `ids` clause is read differently, from the set built when this was prepared.
    #[must_use]
    pub(crate) fn matches_participant(&self, participant: ParticipantContext<'_>) -> bool {
        match self {
            Self::Tag(tag) => participant
                .tags
                .iter()
                .any(|item| tag.is_ancestor_or_self_of(item)),
            Self::Any(selectors) => selectors.iter().any(|s| s.matches_participant(participant)),
            Self::All(selectors) => selectors.iter().all(|s| s.matches_participant(participant)),
            Self::Not(selector) => !selector.matches_participant(participant),
            Self::Ids(ids) => ids.contains(participant.id),
            Self::Role(role) => *role == participant.role,
            Self::Kind(kind) => *kind == participant.kind,
            Self::True => true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{ParticipantContext, ParticipantKind, Role, Selector, Tag, TagError};

    fn tag(path: &str) -> Tag {
        Tag::new(path).unwrap()
    }

    #[test]
    fn valid_tags_construct_cleanly() {
        assert_eq!(tag("interview").as_str(), "interview");
        assert_eq!(tag("interview/front-end").as_str(), "interview/front-end");
        assert_eq!(tag("a.b_c-d/9").as_str(), "a.b_c-d/9");
        assert_eq!(tag("X/y/Z").as_str(), "X/y/Z");
        assert_eq!(tag("interview/front-end").segments().count(), 2);
    }

    #[test]
    fn invalid_tags_are_rejected_with_stable_codes() {
        assert_eq!(Tag::new(""), Err(TagError::Empty));
        assert_eq!(TagError::Empty.code(), "tag_empty");

        for (bad, expected_index) in [("/lead", 0), ("trail/", 1), ("dou//ble", 1)] {
            assert_eq!(
                Tag::new(bad),
                Err(TagError::EmptySegment {
                    segment_index: expected_index
                })
            );
            assert_eq!(
                TagError::EmptySegment { segment_index: 0 }.code(),
                "tag_empty_segment"
            );
        }

        assert_eq!(
            Tag::new("interview/Front End"),
            Err(TagError::InvalidCharacter {
                segment_index: 1,
                character: ' '
            })
        );
        assert_eq!(
            Tag::new("interview/front:ender"),
            Err(TagError::InvalidCharacter {
                segment_index: 1,
                character: ':'
            })
        );
        assert_eq!(
            Tag::new("caf\u{e9}"),
            Err(TagError::InvalidCharacter {
                segment_index: 0,
                character: '\u{e9}'
            })
        );
        assert_eq!(
            TagError::InvalidCharacter {
                segment_index: 0,
                character: ':'
            }
            .code(),
            "tag_invalid_character"
        );
    }

    #[test]
    fn every_tag_constructor_rejects_the_same_invalid_spellings() {
        // Ancestor matching compares validated paths byte by byte (`a` is an
        // ancestor of `a/b`, never of `a.b`), so it holds only while no `Tag`
        // can exist in a spelling other than the canonical one. `Tag::new` is
        // covered above; the two conversions are separate constructor bodies
        // and must enforce exactly the same rules, or a caller taking either
        // of them can build a tag that silently fails to match its children.
        let invalid = [
            ("", "tag_empty"),
            ("/lead", "tag_empty_segment"),
            ("trail/", "tag_empty_segment"),
            ("dou//ble", "tag_empty_segment"),
            ("interview/Front End", "tag_invalid_character"),
            ("caf\u{e9}", "tag_invalid_character"),
        ];
        for (input, expected_code) in invalid {
            assert_eq!(
                Tag::new(input).unwrap_err().code(),
                expected_code,
                "Tag::new({input:?})"
            );
            assert_eq!(
                Tag::try_from(input).unwrap_err().code(),
                expected_code,
                "TryFrom<&str> for {input:?}"
            );
            assert_eq!(
                Tag::try_from(input.to_owned()).unwrap_err().code(),
                expected_code,
                "TryFrom<String> for {input:?}"
            );
        }

        for input in ["interview", "interview/front-end", "a.b_c-d/9", "X/y/Z"] {
            assert_eq!(Tag::try_from(input).unwrap().as_str(), input);
            assert_eq!(Tag::try_from(input.to_owned()).unwrap().as_str(), input);
        }
    }

    #[test]
    fn tags_match_themselves_and_every_ancestor_prefix_case_sensitively() {
        let item = tag("interview/front-end");
        assert!(tag("interview/front-end").is_ancestor_or_self_of(&item));
        assert!(tag("interview").is_ancestor_or_self_of(&item));
        assert!(!tag("Interview").is_ancestor_or_self_of(&item));
        assert!(!tag("interview/Front-End").is_ancestor_or_self_of(&item));
        // `interview/front` is a sibling of `interview/front-end`, not an
        // ancestor: a shared prefix run is not a shared path.
        assert!(!tag("interview/front").is_ancestor_or_self_of(&item));
        assert!(!tag("interview/front-end/extra").is_ancestor_or_self_of(&item));
        assert!(!tag("inter").is_ancestor_or_self_of(&item));
    }

    #[test]
    fn tag_selectors_match_the_tag_and_its_descendants() {
        let selector = Selector::Tag(tag("interview"));
        assert!(selector.matches_item(&[tag("interview")]));
        assert!(selector.matches_item(&[tag("interview/front-end")]));
        assert!(selector.matches_item(&[tag("interview/front")]));
        assert!(!selector.matches_item(&[tag("Interview")]));
        assert!(!selector.matches_item(&[tag("inter")]));
        assert!(!selector.matches_item(&[tag("status/tentative")]));
        assert!(!selector.matches_item(&[]));
    }

    #[test]
    fn participant_selectors_match_ids_roles_and_kinds() {
        let context = ParticipantContext {
            id: "Tom",
            kind: ParticipantKind::Person,
            role: Role::Attendee,
            tags: &[tag("team/sales")],
        };
        assert!(Selector::Ids(vec!["Tom".to_owned()]).matches_participant(context));
        assert!(!Selector::Ids(vec!["tom".to_owned()]).matches_participant(context));
        assert!(Selector::Role(Role::Attendee).matches_participant(context));
        assert!(!Selector::Role(Role::Host).matches_participant(context));
        assert!(Selector::kind(ParticipantKind::Person).matches_participant(context));
        assert!(!Selector::kind(ParticipantKind::Resource).matches_participant(context));
    }

    #[test]
    fn a_participants_context_is_its_own_id_kind_role_and_tags() {
        use crate::model::Participant;
        use crate::time::ZoneName;

        let mut participant = Participant::new("room-4", ZoneName::new("UTC").unwrap())
            .with_kind(ParticipantKind::Resource)
            .with_tags(vec![tag("kind/room")]);
        participant.role = Role::Attendee;
        assert_eq!(
            participant.context(),
            ParticipantContext {
                id: "room-4",
                kind: ParticipantKind::Resource,
                role: Role::Attendee,
                tags: &participant.tags,
            }
        );
    }

    #[test]
    fn participant_only_selectors_never_match_items() {
        let tags = [tag("team/sales")];
        assert!(!Selector::Ids(vec!["Tom".to_owned()]).matches_item(&tags));
        assert!(!Selector::Role(Role::Host).matches_item(&tags));
        assert!(!Selector::kind(ParticipantKind::Resource).matches_item(&tags));
        // Negating one therefore matches every item, which is why a clause
        // evaluated against item tags should contain none of them.
        assert!(Selector::Not(Box::new(Selector::Ids(vec!["Tom".to_owned()]))).matches_item(&tags));
        assert!(Selector::Not(Box::new(Selector::Role(Role::Host))).matches_item(&tags));
        assert!(
            Selector::Not(Box::new(Selector::kind(ParticipantKind::Person))).matches_item(&tags)
        );
    }

    /// A prepared selector answers exactly what the selector it was built from
    /// answers, including a repeated ID and a `not` around an `ids` clause.
    #[test]
    fn a_prepared_selector_matches_exactly_like_its_selector() {
        use super::PreparedSelector;

        let selector = Selector::Any(vec![
            Selector::Ids(vec!["tom".to_owned(), "tom".to_owned(), "ana".to_owned()]),
            Selector::Not(Box::new(Selector::Ids(vec!["bo".to_owned()]))),
            Selector::All(vec![
                Selector::Tag(tag("team/sales")),
                Selector::kind(ParticipantKind::Person),
            ]),
        ]);
        let prepared = PreparedSelector::from(&selector);
        let tagged = [tag("team/sales/emea")];
        let untagged: [Tag; 0] = [];
        for (id, role, tags) in [
            ("tom", Role::Host, &tagged[..]),
            ("bo", Role::Host, &tagged[..]),
            ("cy", Role::Attendee, &untagged[..]),
            ("ana", Role::Attendee, &tagged[..]),
        ] {
            let context = ParticipantContext {
                id,
                kind: ParticipantKind::Person,
                role,
                tags,
            };
            assert_eq!(
                prepared.matches_participant(context),
                selector.matches_participant(context),
                "`{id}`"
            );
        }
    }

    #[test]
    fn empty_any_matches_nothing_and_empty_all_matches_everything() {
        let tags = [tag("team/sales")];
        let context = ParticipantContext {
            id: "tom",
            kind: ParticipantKind::Person,
            role: Role::Host,
            tags: &tags,
        };
        assert!(!Selector::Any(Vec::new()).matches_item(&tags));
        assert!(Selector::All(Vec::new()).matches_item(&tags));
        assert!(!Selector::Any(Vec::new()).matches_participant(context));
        assert!(Selector::All(Vec::new()).matches_participant(context));
    }

    #[test]
    fn boolean_combinations_nest_over_both_contexts() {
        let selector = Selector::Any(vec![
            Selector::All(vec![
                Selector::Tag(tag("veto")),
                Selector::Not(Box::new(Selector::Tag(tag("veto/crm-freeze")))),
            ]),
            Selector::True,
        ]);
        assert!(selector.matches_item(&[tag("veto/crm-freeze")]));
        assert!(selector.matches_item(&[tag("veto/travel-ban")]));
        assert!(selector.matches_item(&[]));

        let participant_only = Selector::All(vec![
            Selector::Not(Box::new(Selector::Role(Role::Host))),
            Selector::True,
        ]);
        let tags = [];
        let host = ParticipantContext {
            id: "a",
            kind: ParticipantKind::Person,
            role: Role::Host,
            tags: &tags,
        };
        let attendee = ParticipantContext {
            id: "b",
            kind: ParticipantKind::Person,
            role: Role::Attendee,
            tags: &tags,
        };
        assert!(!participant_only.matches_participant(host));
        assert!(participant_only.matches_participant(attendee));
    }
}
