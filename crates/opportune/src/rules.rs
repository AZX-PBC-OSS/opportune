//! Which rules apply to a participant and a booking, and how they combine.
//!
//! Applicability is the same for every kind: a rule applies when its `who`
//! selects the participant and its `when` matches the part's tags. Combination
//! uses fixed semantics per kind — intersection for windows,
//! union for blackouts, the largest gap for clearance, the largest `min` and the
//! earliest `max` for lead time, each rest rule kept independent, each cap kept
//! independent with only the caps whose `count` matches the booking selected, and
//! the first matching entry across rules in request order for status policies.

use crate::model::{Cap, Clearance, LeadTimeMax, Rest, Rule, RuleKind, StatusPolicy};
use crate::tags::{ParticipantContext, PreparedSelector, Selector, Tag};
use crate::time::{BlackoutSpec, Duration, WindowSpec};

/// The rule that supplied a combined value, and where it sits in request order
/// so ties across fields can cite the earliest.
pub(crate) type Supplier<'a> = Option<(&'a str, usize)>;

/// How far either side of the range it judges a `rest` rule reads, in minutes.
///
/// A booking's late minutes can fall on the local date before the early minutes
/// they pair with, and a local date is within a day of the UTC date wherever the
/// zone is, so five days covers every date adjacent to a booking. `eval` widens
/// the range it judges by this much, and `ranges` widens the padded search window
/// by the same amount before hulling each zone, so every instant rest can read is
/// inside the calendar data a caller is told to fetch.
pub(crate) const REST_REACH_MINUTES: i64 = 5 * 24 * 60;

/// The rules that applied to a participant, already combined where the kind
/// combines at selection time.
///
/// Each combined value keeps the rule that supplied it, so a reason can cite the
/// rule behind the exclusion. Where rules of the same kind combine by picking a
/// value and several supplied the same one, the earliest in request order is
/// kept.
#[derive(Debug, Default)]
pub(crate) struct Applicable<'a> {
    /// Every applicable window, the rule it came from and that rule's position in
    /// request order, to be intersected over the judged range.
    pub(crate) windows: Vec<(&'a str, usize, &'a WindowSpec)>,
    /// Every applicable blackout, the rule it came from and that rule's position
    /// in request order, to be unioned over the judged range.
    pub(crate) blackouts: Vec<(&'a str, usize, &'a BlackoutSpec)>,
    /// The widest clearance of any applicable rule: the largest gap each rule
    /// asked for, never a sum.
    pub(crate) clearance: Clearance,
    /// The rule that supplied each of `clearance`'s gaps.
    pub(crate) clearance_rules: ClearanceRules<'a>,
    /// The largest `lead_time.min`.
    pub(crate) lead_min: Duration,
    /// The rule that supplied `lead_min`: absent when no rule asks for notice,
    /// because the implicit minimum is zero.
    pub(crate) lead_min_rule: Supplier<'a>,
    /// Every applicable `lead_time.max`: all of them bound the start, and the
    /// strictest is the combined value. Each keeps its request position so a
    /// finding can point at the rule.
    pub(crate) lead_max: Vec<LeadMax<'a>>,
    /// Every applicable status policy, in request order: the first matching
    /// entry across them decides a block.
    pub(crate) status_policies: Vec<&'a StatusPolicy>,
    /// Every applicable rest rule, its ID, and its position in request order.
    /// Rest rules are not combined: each is evaluated independently.
    pub(crate) rests: Vec<(&'a str, usize, &'a Rest)>,
    /// Every applicable cap, in request order. A cap applies only when its
    /// `when` and its `count` both match the part's tags, so a cap measuring
    /// something other than this booking is not here at all.
    pub(crate) caps: Vec<(&'a str, &'a Cap)>,
}

/// One applicable `lead_time.max`, with the rule that supplied it and where the
/// rule sits in request order.
#[derive(Clone, Copy, Debug)]
pub(crate) struct LeadMax<'a> {
    /// The rule's ID, which a reason cites.
    pub(crate) id: &'a str,
    /// The rule's position in the request, so a finding can point at it.
    pub(crate) position: usize,
    /// The latest start the rule allows.
    pub(crate) max: &'a LeadTimeMax,
}

/// The rules that supplied each gap of a combined [`Clearance`].
#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct ClearanceRules<'a> {
    /// The rule that supplied the largest `before` gap.
    pub(crate) before: Supplier<'a>,
    /// The rule that supplied the largest `after` gap.
    pub(crate) after: Supplier<'a>,
    /// The rule that supplied the largest `min_gap`.
    pub(crate) min_gap: Supplier<'a>,
}

/// The rules a booking could select, decided once per request.
///
/// Applicability has two halves. A rule's `when` — and, for a cap, its `count` —
/// is matched against the booking's tags and so is the same for every participant,
/// while `who` varies per participant. A search evaluates every rule once per
/// participant, so this decides the booking half once and prepares each `who` for
/// repeated matching: the cost follows the request's rules rather than
/// `rules × participants × selector size`, and an `ids` clause is a set lookup
/// instead of a list scan per participant.
pub(crate) struct RuleMatcher<'a> {
    /// Every rule, in request order.
    rules: &'a [Rule],
    /// The rule's prepared `who` selector, or `None` when its `when` (or a cap's
    /// `count`) does not match the booking at all.
    for_part: Vec<Option<PreparedSelector<'a>>>,
}

impl<'a> RuleMatcher<'a> {
    /// Prepares `rules` for a booking carrying `part_tags`.
    pub(crate) fn new(rules: &'a [Rule], part_tags: &[Tag]) -> Self {
        let for_part = rules
            .iter()
            .map(|rule| {
                if !rule.when.matches_item(part_tags) {
                    return None;
                }
                // A cap applies to a booking only when its `count` matches the
                // part's tags as well as its `when`; both are about the booking,
                // not the participant, so both are decided here.
                if let RuleKind::Cap(cap) = &rule.kind
                    && !cap.count.matches_item(part_tags)
                {
                    return None;
                }
                Some(PreparedSelector::from(&rule.who))
            })
            .collect();
        Self { rules, for_part }
    }

    /// Selects and combines the rules that apply to `participant`.
    pub(crate) fn select(&self, participant: ParticipantContext<'_>) -> Applicable<'a> {
        let mut applicable = Applicable::default();
        for (position, rule) in self.rules.iter().enumerate() {
            let Some(who) = &self.for_part[position] else {
                continue;
            };
            if who.matches_participant(participant) {
                push_applicable(&mut applicable, position, rule);
            }
        }
        applicable
    }
}

/// Selects and combines the rules that apply to `participant` for a booking with
/// `part_tags`.
///
/// Every evaluation path builds one [`RuleMatcher`] per booking and reuses it for
/// every participant, so this one-shot form is only the unit tests' entry point.
#[cfg(test)]
pub(crate) fn select<'a>(
    rules: &'a [Rule],
    participant: ParticipantContext<'_>,
    part_tags: &[Tag],
) -> Applicable<'a> {
    RuleMatcher::new(rules, part_tags).select(participant)
}

/// Adds the value `rule` contributes, whose `when` and — for a cap — `count` the
/// caller has already matched.
fn push_applicable<'a>(applicable: &mut Applicable<'a>, position: usize, rule: &'a Rule) {
    let id = rule.id.as_str();
    match &rule.kind {
        RuleKind::Window(spec) => {
            applicable.windows.push((id, position, spec));
        }
        RuleKind::Blackout(spec) => {
            applicable.blackouts.push((id, position, spec));
        }
        RuleKind::Clearance(clearance) => {
            let sources = &mut applicable.clearance_rules;
            widest(
                &mut applicable.clearance.before,
                &mut sources.before,
                clearance.before,
                id,
                position,
            );
            widest(
                &mut applicable.clearance.after,
                &mut sources.after,
                clearance.after,
                id,
                position,
            );
            widest(
                &mut applicable.clearance.min_gap,
                &mut sources.min_gap,
                clearance.min_gap,
                id,
                position,
            );
        }
        RuleKind::LeadTime(lead_time) => {
            // The largest minimum notice wins; among equals the earliest rule
            // keeps the citation.
            widest(
                &mut applicable.lead_min,
                &mut applicable.lead_min_rule,
                lead_time.min,
                id,
                position,
            );
            if let Some(max) = &lead_time.max {
                applicable.lead_max.push(LeadMax { id, position, max });
            }
        }
        RuleKind::StatusPolicy(policy) => {
            applicable.status_policies.push(policy);
        }
        RuleKind::Rest(rest) => {
            applicable.rests.push((id, position, rest));
        }
        RuleKind::Cap(cap) => {
            applicable.caps.push((id, cap));
        }
    }
}

/// Keeps the largest `value` in `combined` and the rule that supplied it.
///
/// Only a larger value displaces the current supplier, so the earliest rule in
/// request order keeps the citation among equals. A rule that contributes
/// nothing — the zero notice or gap that is also the default — supplied no
/// exclusion and gets no citation.
fn widest<'a>(
    combined: &mut Duration,
    supplier: &mut Supplier<'a>,
    value: Duration,
    id: &'a str,
    position: usize,
) {
    if value > *combined {
        *combined = value;
        *supplier = Some((id, position));
    }
}

impl Applicable<'_> {
    /// Whether a block with `status` and `tags` blocks the participant.
    ///
    /// The first entry that matches across the applicable policies decides it;
    /// with no matching entry a `free` block is ignored and every other status
    /// blocks. An ignored block's travel is ignored with it, because the block
    /// itself never enters the blocked set.
    pub(crate) fn blocks(&self, status: crate::model::BlockStatus, tags: &[Tag]) -> bool {
        let effect = self
            .status_policies
            .iter()
            .find_map(|policy| policy.decide(status, tags));
        match effect {
            Some(crate::model::StatusEffect::Ignore) => false,
            Some(crate::model::StatusEffect::Block) => true,
            None => status != crate::model::BlockStatus::Free,
        }
    }
}

/// Every participant ID a selector names through its `ids` clauses.
pub(crate) fn named_ids(selector: &Selector) -> Vec<&str> {
    let mut ids = Vec::new();
    collect_named_ids(selector, &mut ids);
    ids
}

fn collect_named_ids<'a>(selector: &'a Selector, ids: &mut Vec<&'a str>) {
    match selector {
        Selector::Ids(named) => ids.extend(named.iter().map(String::as_str)),
        Selector::Any(selectors) | Selector::All(selectors) => {
            for selector in selectors {
                collect_named_ids(selector, ids);
            }
        }
        Selector::Not(inner) => collect_named_ids(inner, ids),
        Selector::Tag(_) | Selector::Role(_) | Selector::Kind(_) | Selector::True => {}
    }
}

/// Every tag a selector names, wherever it sits in the expression.
pub(crate) fn named_tags(selector: &Selector) -> Vec<&Tag> {
    let mut tags = Vec::new();
    collect_named_tags(selector, &mut tags);
    tags
}

fn collect_named_tags<'a>(selector: &'a Selector, tags: &mut Vec<&'a Tag>) {
    match selector {
        Selector::Tag(tag) => tags.push(tag),
        Selector::Any(selectors) | Selector::All(selectors) => {
            for selector in selectors {
                collect_named_tags(selector, tags);
            }
        }
        Selector::Not(inner) => collect_named_tags(inner, tags),
        Selector::Ids(_) | Selector::Role(_) | Selector::Kind(_) | Selector::True => {}
    }
}

/// Whether a selector uses a clause only a participant can satisfy.
///
/// `ids`, `role` and `kind` select participants, never items. In a clause matched
/// against an item's tags they can never be true, and under a `not` they are
/// then always true — a rule that reads as narrow and applies to everything — so
/// a clause evaluated against item tags must not contain any of them.
pub(crate) fn names_participants(selector: &Selector) -> bool {
    match selector {
        Selector::Ids(_) | Selector::Role(_) | Selector::Kind(_) => true,
        Selector::Any(selectors) | Selector::All(selectors) => {
            selectors.iter().any(names_participants)
        }
        Selector::Not(inner) => names_participants(inner),
        Selector::Tag(_) | Selector::True => false,
    }
}

#[cfg(test)]
mod tests {
    use super::{Applicable, named_ids, named_tags, names_participants, select};
    use crate::model::{
        BlockStatus, Clearance, LeadTime, LeadTimeMax, Rule, RuleKind, StatusEffect, StatusPolicy,
        StatusPolicyEntry,
    };
    use crate::tags::{ParticipantContext, Role, Selector, Tag};
    use crate::time::{Duration, ZoneName};

    fn tag(path: &str) -> Tag {
        Tag::new(path).unwrap()
    }

    fn host<'a>(id: &'a str, role: Role, tags: &'a [Tag]) -> ParticipantContext<'a> {
        ParticipantContext {
            id,
            kind: crate::model::ParticipantKind::Person,
            role,
            tags,
        }
    }

    /// The ID of the rule a combined value cites.
    fn cited(supplier: super::Supplier<'_>) -> Option<&str> {
        supplier.map(|(id, _)| id)
    }

    fn lead_time(minutes: u32) -> Rule {
        Rule::new(
            format!("lead-{minutes}"),
            RuleKind::LeadTime(LeadTime {
                min: Duration::from_minutes(minutes),
                max: None,
            }),
        )
    }

    #[test]
    fn a_rule_applies_only_where_who_and_when_both_match() {
        let tags = [tag("team/sales")];
        let hosts = host("ana", Role::Host, &tags);
        let rules = vec![
            lead_time(60),
            Rule::new("sales-only", RuleKind::Clearance(Clearance::default()))
                .with_who(Selector::Tag(tag("team/sales"))),
            Rule::new("interview-only", RuleKind::Clearance(Clearance::default()))
                .with_when(Selector::Tag(tag("interview"))),
        ];
        let applicable = select(&rules, hosts, &[tag("interview/front-end")]);
        // The first rule applies to every host and every booking; the second
        // matches the participant and the booking; the third matches the
        // booking but not the participant's tag.
        assert_eq!(applicable.lead_min.minutes(), 60);
        assert!(applicable.windows.is_empty());

        let attendees = host("tom", Role::Attendee, &tags);
        let applicable = select(&rules, attendees, &[]);
        // `who` defaults to hosts, so no rule reaches an attendee.
        assert_eq!(applicable.lead_min.minutes(), 0);
    }

    #[test]
    fn clearance_combines_gap_by_gap_and_lead_min_takes_the_largest() {
        let rules = vec![
            Rule::new(
                "a",
                RuleKind::Clearance(Clearance {
                    before: Duration::from_minutes(10),
                    after: Duration::from_minutes(0),
                    min_gap: Duration::from_minutes(5),
                }),
            ),
            lead_time(30),
            Rule::new(
                "b",
                RuleKind::Clearance(Clearance {
                    before: Duration::from_minutes(0),
                    after: Duration::from_minutes(20),
                    min_gap: Duration::from_minutes(15),
                }),
            ),
            lead_time(45),
        ];
        let applicable = select(&rules, host("ana", Role::Host, &[]), &[]);
        assert_eq!(applicable.clearance.before.minutes(), 10);
        assert_eq!(applicable.clearance.after.minutes(), 20);
        assert_eq!(applicable.clearance.min_gap.minutes(), 15);
        assert_eq!(
            applicable.lead_min.minutes(),
            45,
            "the largest minimum notice wins"
        );
        // Each combined value keeps the rule that supplied it.
        assert_eq!(cited(applicable.clearance_rules.before), Some("a"));
        assert_eq!(cited(applicable.clearance_rules.after), Some("b"));
        assert_eq!(cited(applicable.clearance_rules.min_gap), Some("b"));
        assert_eq!(cited(applicable.lead_min_rule), Some("lead-45"));
    }

    #[test]
    fn combined_values_cite_the_earliest_rule_among_equals() {
        let rules = vec![
            Rule::new(
                "first-gap",
                RuleKind::Clearance(Clearance {
                    before: Duration::from_minutes(0),
                    after: Duration::from_minutes(10),
                    min_gap: Duration::from_minutes(0),
                }),
            ),
            Rule::new(
                "second-gap",
                RuleKind::Clearance(Clearance {
                    before: Duration::from_minutes(0),
                    after: Duration::from_minutes(10),
                    min_gap: Duration::from_minutes(0),
                }),
            ),
            lead_time(30),
            lead_time(30),
        ];
        let applicable = select(&rules, host("ana", Role::Host, &[]), &[]);
        assert_eq!(cited(applicable.clearance_rules.after), Some("first-gap"));
        assert_eq!(cited(applicable.lead_min_rule), Some("lead-30"));
    }

    #[test]
    fn the_implicit_minimum_notice_has_no_rule_to_cite() {
        let applicable = select(&[], host("ana", Role::Host, &[]), &[]);
        assert_eq!(applicable.lead_min.minutes(), 0);
        assert_eq!(applicable.lead_min_rule, None);
    }

    #[test]
    fn a_rule_asking_for_no_notice_is_not_cited_for_it() {
        // A `lead_time` rule that only sets a maximum asks for no notice, so the
        // zero it contributes is the implicit minimum every request has.
        let rules = vec![Rule::new(
            "horizon",
            RuleKind::LeadTime(LeadTime {
                min: Duration::ZERO,
                max: Some(LeadTimeMax::Days(3)),
            }),
        )];
        let applicable = select(&rules, host("ana", Role::Host, &[]), &[]);
        assert_eq!(applicable.lead_min.minutes(), 0);
        assert_eq!(applicable.lead_min_rule, None);
        assert_eq!(applicable.clearance_rules.before, None);
    }

    #[test]
    fn lead_max_keeps_every_bound_in_request_order() {
        let rules = vec![
            Rule::new(
                "days",
                RuleKind::LeadTime(LeadTime {
                    min: Duration::ZERO,
                    max: Some(LeadTimeMax::Days(3)),
                }),
            ),
            Rule::new(
                "hours",
                RuleKind::LeadTime(LeadTime {
                    min: Duration::ZERO,
                    max: Some(LeadTimeMax::Duration(Duration::from_minutes(90))),
                }),
            ),
        ];
        let applicable = select(&rules, host("ana", Role::Host, &[]), &[]);
        assert_eq!(applicable.lead_max.len(), 2);
        assert_eq!(applicable.lead_max[0].id, "days");
        assert_eq!(applicable.lead_max[1].id, "hours");
    }

    #[test]
    fn a_status_policy_decides_a_block_and_ignores_travel_with_it() {
        let policy = Rule::new(
            "policy",
            RuleKind::StatusPolicy(StatusPolicy {
                entries: vec![
                    StatusPolicyEntry::status(BlockStatus::Tentative, StatusEffect::Ignore),
                    StatusPolicyEntry::tags(
                        Selector::Tag(tag("status/focus")),
                        StatusEffect::Block,
                    ),
                ],
            }),
        );
        let rules = [policy];
        let applicable = select(&rules, host("ana", Role::Host, &[]), &[]);
        // A tentative block is ignored by the status entry, and a busy one
        // tagged `status/focus` blocks: the first matching entry decides.
        assert!(!applicable.blocks(BlockStatus::Tentative, &[]));
        assert!(!applicable.blocks(BlockStatus::Tentative, &[tag("status/focus")]));
        assert!(applicable.blocks(BlockStatus::Busy, &[tag("status/focus")]));
        assert!(applicable.blocks(BlockStatus::Busy, &[]));
        assert!(!applicable.blocks(BlockStatus::Free, &[]));
        // With no status policy at all, `free` is the only ignored status.
        let none = Applicable::default();
        assert!(!none.blocks(BlockStatus::Free, &[]));
        assert!(none.blocks(BlockStatus::Tentative, &[]));
    }

    #[test]
    fn selectors_are_walked_through_every_boolean() {
        let selector = Selector::Any(vec![
            Selector::All(vec![
                Selector::Tag(tag("team/sales")),
                Selector::Not(Box::new(Selector::Tag(tag("level/junior")))),
            ]),
            Selector::Ids(vec!["tom".to_owned(), "ana".to_owned()]),
        ]);
        assert_eq!(named_ids(&selector), ["tom", "ana"]);
        assert_eq!(
            named_tags(&selector)
                .iter()
                .map(|tag| tag.as_str())
                .collect::<Vec<_>>(),
            ["team/sales", "level/junior"]
        );
        assert!(names_participants(&selector));
        assert!(!names_participants(&Selector::Tag(tag("interview"))));
        assert!(names_participants(&Selector::Not(Box::new(
            Selector::Role(Role::Attendee)
        ))));
    }

    #[test]
    fn rest_rules_are_collected_independently_in_request_order() {
        // Rest is not combined at selection time: every applicable rule is kept,
        // with its own ID, so each can exclude a booking on its own.
        use crate::model::Rest;
        use crate::time::TimeOfDay;

        let rest = |id: &str, late: u8, not_before: u8| {
            Rule::new(
                id,
                RuleKind::Rest(Rest::new(
                    TimeOfDay::new(late, 0).unwrap(),
                    TimeOfDay::new(not_before, 0).unwrap(),
                )),
            )
        };
        let rules = vec![rest("evening", 18, 10), lead_time(60), rest("late", 21, 8)];
        let applicable = select(&rules, host("ana", Role::Host, &[]), &[]);
        let kept: Vec<(&str, u8, u8)> = applicable
            .rests
            .iter()
            .map(|(id, _, rest)| (*id, rest.late_after.hour(), rest.not_before.hour()))
            .collect();
        assert_eq!(kept, [("evening", 18, 10), ("late", 21, 8)]);
        // A rest rule's `who` is respected like any other kind.
        let attendees = host("tom", Role::Attendee, &[]);
        assert!(select(&rules, attendees, &[]).rests.is_empty());
    }

    #[test]
    fn caps_are_collected_only_when_their_count_matches_the_booking() {
        use crate::model::{Cap, CapMeasure, CapPer, CapPeriod};
        use crate::time::{RuleZone, Weekday};

        let cap = |id: &str, counted: &str| {
            Rule::new(
                id,
                RuleKind::Cap(Cap::new(
                    Selector::Tag(tag(counted)),
                    CapMeasure::Count,
                    2,
                    CapPeriod::Day,
                    CapPer::Each,
                    RuleZone::Participant,
                    Weekday::Monday,
                )),
            )
        };
        let rules = vec![
            cap("interviews", "interview"),
            lead_time(60),
            cap("sales", "sales/demo"),
            cap("front-end", "interview/front-end"),
        ];
        // Both interview caps match the part's `interview/front-end` tag, in
        // request order; the sales cap measures another kind of booking and is
        // not selected at all.
        let applicable = select(
            &rules,
            host("ana", Role::Host, &[]),
            &[tag("interview/front-end")],
        );
        let ids: Vec<&str> = applicable.caps.iter().map(|(id, _)| *id).collect();
        assert_eq!(ids, ["interviews", "front-end"]);
        // A booking no cap counts gets none of them.
        assert_eq!(
            select(&rules, host("ana", Role::Host, &[]), &[tag("sales/demo")])
                .caps
                .len(),
            1
        );
        assert!(
            select(&rules, host("ana", Role::Host, &[]), &[])
                .caps
                .is_empty()
        );
    }

    #[test]
    fn a_zone_name_is_the_only_place_a_rule_names_a_zone() {
        // The wall-clock specifications a model rule carries name their zones,
        // so a request can hold them without a database.
        use crate::time::{RuleZone, WindowSpec};
        let spec = WindowSpec::new(
            RuleZone::Named(ZoneName::new("Europe/London").unwrap()),
            Vec::new(),
            Vec::new(),
        )
        .unwrap();
        assert_eq!(
            spec.zone(),
            &RuleZone::Named(ZoneName::new("Europe/London").unwrap())
        );
    }
}
