//! Property tests: tags and selectors must agree with a naive string-based
//! reference model — tag hierarchy as string prefixes, selectors as a plain
//! recursive interpreter over plain strings.

use opportune::model::ParticipantKind;
use opportune::tags::{ParticipantContext, Role, Selector, Tag};
use opportune_testkit::generators;
use proptest::prelude::*;

/// The naive model's selector: the same grammar, spelled over plain strings.
#[derive(Debug, Clone)]
enum NaiveSelector {
    Tag(String),
    Any(Vec<NaiveSelector>),
    All(Vec<NaiveSelector>),
    Not(Box<NaiveSelector>),
    Ids(Vec<String>),
    Role(&'static str),
    Kind(&'static str),
    True,
}

/// The naive evaluator over plain strings: `interview` matches an item tag
/// spelled exactly or under a `/`-prefixed parent; `participant` is `Some`
/// for the participant context and `None` for the item context, where the
/// participant-only variants match nothing.
fn naive_matches(
    selector: &NaiveSelector,
    tags: &[String],
    participant: Option<(&str, &str, &str)>,
) -> bool {
    match selector {
        NaiveSelector::Tag(tag) => tags
            .iter()
            .any(|item| item == tag || item.starts_with(&format!("{tag}/"))),
        NaiveSelector::Any(selectors) => selectors
            .iter()
            .any(|s| naive_matches(s, tags, participant)),
        NaiveSelector::All(selectors) => selectors
            .iter()
            .all(|s| naive_matches(s, tags, participant)),
        NaiveSelector::Not(inner) => !naive_matches(inner, tags, participant),
        NaiveSelector::Ids(ids) => match participant {
            Some((id, _, _)) => ids.iter().any(|candidate| candidate == id),
            None => false,
        },
        NaiveSelector::Role(expected) => match participant {
            Some((_, role, _)) => *expected == role,
            None => false,
        },
        NaiveSelector::Kind(expected) => match participant {
            Some((_, _, kind)) => *expected == kind,
            None => false,
        },
        NaiveSelector::True => true,
    }
}

fn role_string(role: Role) -> &'static str {
    match role {
        Role::Host => "host",
        Role::Attendee => "attendee",
    }
}

fn kind_string(kind: ParticipantKind) -> &'static str {
    match kind {
        ParticipantKind::Person => "person",
        ParticipantKind::Resource => "resource",
    }
}

/// The production selector built from the naive model's shape, through the
/// validating constructors.
fn to_production(selector: &NaiveSelector) -> Selector {
    match selector {
        NaiveSelector::Tag(tag) => Selector::Tag(Tag::new(tag).unwrap()),
        NaiveSelector::Any(selectors) => {
            Selector::Any(selectors.iter().map(to_production).collect())
        }
        NaiveSelector::All(selectors) => {
            Selector::All(selectors.iter().map(to_production).collect())
        }
        NaiveSelector::Not(inner) => Selector::Not(Box::new(to_production(inner))),
        NaiveSelector::Ids(ids) => Selector::Ids(ids.clone()),
        NaiveSelector::Role(role) => match *role {
            "host" => Selector::Role(Role::Host),
            _ => Selector::Role(Role::Attendee),
        },
        NaiveSelector::Kind(kind) => match *kind {
            "person" => Selector::kind(ParticipantKind::Person),
            _ => Selector::kind(ParticipantKind::Resource),
        },
        NaiveSelector::True => Selector::True,
    }
}

/// A selector of bounded nesting depth; the leaves are tag, IDs, role, kind and
/// `true`.
fn naive_selector(depth: u8) -> BoxedStrategy<NaiveSelector> {
    let tag_leaf = generators::tag_path().prop_map(NaiveSelector::Tag);
    let ids_leaf = prop::collection::vec("[a-z]{1,4}", 1..=3).prop_map(NaiveSelector::Ids);
    let role_leaf = prop_oneof![
        Just(NaiveSelector::Role("host")),
        Just(NaiveSelector::Role("attendee")),
    ];
    let kind_leaf = prop_oneof![
        Just(NaiveSelector::Kind("person")),
        Just(NaiveSelector::Kind("resource")),
    ];
    let true_leaf = Just(NaiveSelector::True);

    if depth == 0 {
        prop_oneof![tag_leaf, ids_leaf, role_leaf, kind_leaf, true_leaf].boxed()
    } else {
        let nested = || prop::collection::vec(naive_selector(depth - 1), 0..=3);
        prop_oneof![
            tag_leaf,
            ids_leaf,
            role_leaf,
            kind_leaf,
            true_leaf,
            nested().prop_map(NaiveSelector::Any),
            nested().prop_map(NaiveSelector::All),
            naive_selector(depth - 1).prop_map(|s| NaiveSelector::Not(Box::new(s))),
        ]
        .boxed()
    }
}

proptest! {
    #[test]
    fn tag_hierarchy_matches_the_string_prefix_model(
        segments in proptest::collection::vec("[A-Za-z0-9._-]{1,8}", 1..=4),
    ) {
        let tag = Tag::new(&segments.join("/")).unwrap();
        // The tag and every ancestor prefix must match it.
        for prefix_len in 1..=segments.len() {
            let ancestor = Tag::new(&segments[..prefix_len].join("/")).unwrap();
            prop_assert!(ancestor.is_ancestor_or_self_of(&tag));
            let item_tags = [tag.clone()];
            let selector = Selector::Tag(ancestor.clone());
            prop_assert!(selector.matches_item(&item_tags));
        }
        // A sibling that shares only a partial final segment is not an ancestor.
        let last = segments.last().unwrap();
        if last.len() > 1 {
            let mut sibling_segments = segments.clone();
            sibling_segments[segments.len() - 1] = last[..last.len() - 1].to_owned();
            let sibling = Tag::new(&sibling_segments.join("/")).unwrap();
            prop_assert!(!sibling.is_ancestor_or_self_of(&tag));
        }
    }

    #[test]
    fn selectors_agree_with_the_naive_model(
        selector in naive_selector(3),
        item_tags in proptest::collection::vec(generators::tag_path(), 0..=3),
        participant_id in "[a-z]{1,4}",
        role_index in 0..2u8,
        kind_index in 0..2u8,
    ) {
        let production = to_production(&selector);
        let tags: Vec<Tag> = item_tags.iter().map(|t| Tag::new(t).unwrap()).collect();
        let role = if role_index == 0 { Role::Host } else { Role::Attendee };
        let kind = if kind_index == 0 {
            ParticipantKind::Person
        } else {
            ParticipantKind::Resource
        };

        prop_assert_eq!(
            production.matches_item(&tags),
            naive_matches(&selector, &item_tags, None),
            "item context: production vs naive model"
        );
        prop_assert_eq!(
            production.matches_participant(ParticipantContext {
                id: &participant_id,
                kind,
                role,
                tags: &tags,
            }),
            naive_matches(
                &selector,
                &item_tags,
                Some((&participant_id, role_string(role), kind_string(kind)))
            ),
            "participant context: production vs naive model"
        );
    }
}
