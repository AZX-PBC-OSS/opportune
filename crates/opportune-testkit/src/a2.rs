//! A whole-request reference model for multi-participant evaluation.
//!
//! An [`A2Case`] is the plain description a generator produces: hosts and
//! attendees with zones, tags, weights and calendars; a multi-term requirement;
//! a goal with a grid or supplied candidates; and a list of rules, each naming
//! the participants its `who` selects. [`A2Case::to_request`] turns that
//! description into the engine's domain types, so a test can feed the same case
//! to both sides.
//!
//! [`A2Oracle`] decides each candidate start straight from the specification
//! text. Time is judged minute by minute through `jiff`, sharing none of the
//! engine's interval or breakpoint code; assignment is decided by the
//! brute-force enumeration in [`crate::matching`], sharing none of the engine's
//! matching code. The eligibility rules are applied in the response contract's
//! precedence order, so a disagreement shows up as a different decision, not just
//! a different list of offers.

use std::collections::BTreeMap;

use opportune::Engine;
use opportune::model::{
    Explain, Goal, Offer, Part, Participant, Proposal, ProposalEvent, Request, Requirement,
    Response, Rule, Span, Term, Travel, ZonePeriod,
};
use opportune::reasons::{ReasonCode, ReasonParams};
use opportune::tags::{Role, Selector, Tag};
use opportune::time::{Duration, Instant, TimeZoneDatabase, Zone, ZoneName};
use proptest::prelude::{prop_assert, prop_assert_eq};
use proptest::test_runner::TestCaseError;

use crate::a1::{
    BlockPlan, CapPlan, ClearancePlan, GridPlan, GridZonePlan, OccurrencePlan, RestPlan, RulePlan,
    StatusPolicyPlan, TravelPlan,
};
use crate::matching::{BruteTerm, brute_assign, brute_jointly_fillable};
use crate::oracle_support::{
    CapModel, LeadBound, MinuteFlags, RestDays, blocks, cap_usage, counted_events, minute_of_day,
    overlaps, resolve_bound, rest_activity_days, rest_excludes,
};
use crate::specs::{BlackoutCase, ResolvedZones, WindowCase, ZonePlan};

/// The offer limit a converted case asks for, high enough that no generated case
/// is truncated.
const OFFER_LIMIT: usize = 100_000;

/// The explanation limit a converted case asks for, the engine's default maximum.
const EXPLAIN_LIMIT: usize = 10_000;

/// One participant a generated case books.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct A2Participant {
    /// The participant's opaque, stable ID.
    pub id: String,
    /// Whether the participant is a host or an attendee.
    pub role: Role,
    /// The participant's home zone and disjoint travel legs.
    pub zones: ZonePlan,
    /// The participant's blocks, or `None` when no calendar was supplied.
    pub calendar: Option<Vec<BlockPlan>>,
    /// Everything the participant is tagged with.
    pub tags: Vec<String>,
    /// The weight the participant's assignment carries.
    pub weight: i32,
}

/// One term of a generated requirement.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TermPlan {
    /// One named host.
    Person(String),
    /// At least `count` hosts carrying any of `tags`.
    AtLeast {
        /// How many distinct hosts the term needs.
        count: u32,
        /// The tags the term's selector matches.
        tags: Vec<String>,
    },
}

/// One generated rule, with the participants its `who` selects.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct A2Rule {
    /// The rule itself.
    pub plan: RulePlan,
    /// One flag per participant: whether `who` selects them.
    pub who: Vec<bool>,
    /// Whether the rule's `when` matches the part's tags.
    pub applies_to_part: bool,
}

/// One generated multi-participant request, in plan form.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct A2Case {
    /// The current instant.
    pub now: Instant,
    /// The search window, half-open.
    pub search: (Instant, Instant),
    /// The event zone's IANA name.
    pub event: String,
    /// The zone offers are grouped by date in.
    pub display: String,
    /// The booking's length in minutes.
    pub duration: u32,
    /// A grid, or `None` for the engine's default grid.
    pub grid: Option<GridPlan>,
    /// Caller-supplied candidate starts, mutually exclusive with `grid`.
    pub candidates: Option<Vec<Instant>>,
    /// Travel the new booking needs.
    pub travel: TravelPlan,
    /// The part's tags, which decide which rules apply.
    pub part_tags: Vec<String>,
    /// Everyone the request knows about.
    pub participants: Vec<A2Participant>,
    /// The requirement's terms, in request order.
    pub terms: Vec<TermPlan>,
    /// The request's final rule list.
    pub rules: Vec<A2Rule>,
    /// The caller's identifiable events, which caps count.
    pub occurrences: Vec<OccurrencePlan>,
    /// Event keys ignored across blocking, rest, and caps (e.g. a booking being moved).
    pub exclude_event_keys: Vec<String>,
}

impl A2Case {
    /// The engine request this plan describes.
    ///
    /// # Panics
    /// Panics when a plan is not a valid request — an unparseable tag, zone or
    /// grid, or an inverted span. Generators emit valid plans, and scenarios
    /// build the domain types directly.
    #[must_use]
    pub fn to_request(&self) -> Request {
        let zone = ZoneName::new(&self.event).expect("the event zone is a zone name");
        let mut goal = Goal::new(
            zone,
            Span::new(self.search.0, self.search.1).expect("the search window is non-empty"),
            vec![self.part()],
        );
        goal.display_zone = ZoneName::new(&self.display).expect("the display zone is a zone name");
        goal.grid = self.grid.as_ref().map(GridPlan::to_grid);
        goal.candidates.clone_from(&self.candidates);

        let mut request = Request::new(self.now, goal);
        for participant in &self.participants {
            request.participants.push(participant.to_participant());
            request
                .weights
                .insert(participant.id.clone(), participant.weight);
        }
        request.rules = self
            .rules
            .iter()
            .map(|rule| rule.to_rule(&self.participants))
            .collect();
        request.occurrences = self
            .occurrences
            .iter()
            .map(OccurrencePlan::to_occurrence)
            .collect();
        request
            .exclude_event_keys
            .clone_from(&self.exclude_event_keys);
        request.limit = Some(OFFER_LIMIT);
        request.explain = Some(Explain::new(EXPLAIN_LIMIT));
        request
    }

    /// Whether distinct hosts can fill every term jointly even if everyone were
    /// free, judged by the brute-force enumeration rather than the engine.
    #[must_use]
    pub fn statically_fillable(&self) -> bool {
        let terms: Vec<BruteTerm> = self
            .terms
            .iter()
            .map(|term| BruteTerm {
                count: term.count(),
                candidates: self
                    .participants
                    .iter()
                    .filter(|participant| term.filled_by(participant))
                    .map(|participant| (participant.id.clone(), participant.weight))
                    .collect(),
            })
            .collect();
        brute_jointly_fillable(&terms)
    }

    /// A well-formed assignment for the requirement, taken from the participants
    /// that fill each term however unavailable they are, or `None` when a term
    /// has too few candidates to list its hosts at all. A test uses it to check a
    /// start the search excluded, where no eligible assignment exists.
    #[must_use]
    pub fn probe_assignment(&self) -> Option<Vec<Vec<String>>> {
        let terms: Vec<BruteTerm> = self
            .terms
            .iter()
            .map(|term| BruteTerm {
                count: term.count(),
                candidates: self
                    .participants
                    .iter()
                    .filter(|participant| term.filled_by(participant))
                    .map(|participant| (participant.id.clone(), participant.weight))
                    .collect(),
            })
            .collect();
        brute_assign(&terms)
    }

    fn part(&self) -> Part {
        let mut part = Part::new(
            Duration::from_minutes(self.duration),
            Requirement::new(self.terms.iter().map(TermPlan::to_term).collect()),
        );
        part.tags = self
            .part_tags
            .iter()
            .map(|tag| Tag::new(tag).expect("a generated tag is valid"))
            .collect();
        part.travel = Travel {
            before: Duration::from_minutes(self.travel.before),
            after: Duration::from_minutes(self.travel.after),
            by_participant: BTreeMap::new(),
        };
        part
    }

    fn in_window(&self, start: Instant) -> bool {
        self.search.0 <= start && start < self.search.1
    }

    /// The range the case is judged over: the search window widened by the
    /// largest travel before it and by the duration plus the largest travel after
    /// it, clamped to the supported instants.
    fn judged_range(&self) -> (i64, i64) {
        let from = self
            .search
            .0
            .minutes()
            .saturating_sub(i64::from(self.travel.before))
            .max(Instant::MIN.minutes());
        let until = self
            .search
            .1
            .minutes()
            .saturating_add(i64::from(self.duration) + i64::from(self.travel.after))
            .min(Instant::MAX.minutes());
        (from, until.max(from + 1))
    }
}

impl A2Participant {
    fn to_participant(&self) -> Participant {
        let mut participant = Participant::new(
            &self.id,
            ZoneName::new(&self.zones.home).expect("the home zone is a zone name"),
        );
        participant.role = self.role;
        participant.zone_periods = self
            .zones
            .periods
            .iter()
            .map(|(name, from, until)| {
                ZonePeriod::new(
                    ZoneName::new(name).expect("a leg names a zone"),
                    *from,
                    *until,
                )
                .expect("the generator emits disjoint legs")
            })
            .collect();
        participant.calendar = self
            .calendar
            .as_ref()
            .map(|blocks| blocks.iter().map(BlockPlan::to_block).collect());
        participant.tags = self
            .tags
            .iter()
            .map(|tag| Tag::new(tag).expect("a generated tag is valid"))
            .collect();
        participant
    }
}

impl TermPlan {
    fn to_term(&self) -> Term {
        match self {
            Self::Person(id) => Term::person(id.clone()),
            Self::AtLeast { count, tags } => Term::at_least(
                *count,
                Selector::Any(
                    tags.iter()
                        .map(|tag| Selector::Tag(Tag::new(tag).expect("a generated tag is valid")))
                        .collect(),
                ),
            ),
        }
    }

    fn count(&self) -> usize {
        self.to_term().slots()
    }

    /// Whether `participant` fills the term, judged straight from the plan.
    fn filled_by(&self, participant: &A2Participant) -> bool {
        if participant.role != Role::Host {
            return false;
        }
        match self {
            Self::Person(id) => participant.id == *id,
            Self::AtLeast { tags, .. } => tags.iter().any(|named| {
                let named = Tag::new(named).expect("a generated tag is valid");
                participant.tags.iter().any(|carried| {
                    named.is_ancestor_or_self_of(
                        &Tag::new(carried).expect("a generated tag is valid"),
                    )
                })
            }),
        }
    }
}

impl A2Rule {
    fn applies_to(&self, index: usize) -> bool {
        self.applies_to_part && self.who.get(index).copied().unwrap_or(false)
    }

    fn to_rule(&self, participants: &[A2Participant]) -> Rule {
        let mut rule = self.plan.to_rule();
        rule.who = who_selector(&self.who, participants);
        rule.when = if self.applies_to_part {
            Selector::True
        } else {
            Selector::Not(Box::new(Selector::True))
        };
        rule
    }
}

/// A `who` selector naming exactly the participants the mask selects.
fn who_selector(who: &[bool], participants: &[A2Participant]) -> Selector {
    let ids: Vec<String> = who
        .iter()
        .zip(participants)
        .filter(|(selected, _)| **selected)
        .map(|(_, participant)| participant.id.clone())
        .collect();
    if ids.is_empty() {
        // No participant at all, which no `ids` list can express on its own.
        Selector::Not(Box::new(Selector::True))
    } else {
        Selector::Ids(ids)
    }
}

/// What the reference model decides about one candidate start.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Decision {
    /// The start is bookable, with these eligible hosts per term and this
    /// assignment.
    Offered {
        /// Each term's eligible hosts, ascending by ID.
        eligible: Vec<Vec<String>>,
        /// The chosen assignment, one ascending-by-ID list per term.
        assignment: Vec<Vec<String>>,
        /// The travel every participant the booking needs occupies, as
        /// `(participant, (from, until))`, sorted by participant ID then start.
        reserve: Vec<(String, (i64, i64))>,
        /// Whether an assigned host has no calendar.
        calendar_unknown: bool,
    },
    /// The start is excluded, with the reasons the response contract reports.
    Excluded(Vec<(Option<String>, ReasonCode)>),
}

/// One participant, resolved for this request.
struct Member<'a> {
    plan: &'a A2Participant,
    zones: ResolvedZones,
    windows: Vec<MinuteFlags>,
    blackouts: Vec<MinuteFlags>,
    blocks: Vec<ResolvedBlock<'a>>,
    rests: Vec<(&'a RestPlan, RestDays)>,
    caps: Vec<CapModel>,
    lead_min: i64,
    bounds: Vec<LeadBound>,
}

struct ResolvedBlock<'a> {
    occupied: (i64, i64),
    widened: (i64, i64),
    plan: &'a BlockPlan,
}

/// A minute-by-minute reference model of one [`A2Case`].
pub struct A2Oracle<'a> {
    case: &'a A2Case,
    event: Zone,
    members: Vec<Member<'a>>,
    grid_step: i64,
    grid_anchor: i64,
    grid_zone: Zone,
}

impl<'a> A2Oracle<'a> {
    /// Resolves `case` against `db`.
    ///
    /// # Panics
    /// Panics when a zone the plan names is not in the database. Generators draw
    /// zone names from the bundled database's matrix.
    #[must_use]
    pub fn new(case: &'a A2Case, db: &TimeZoneDatabase) -> Self {
        let event = db
            .zone(&case.event)
            .unwrap_or_else(|err| panic!("`{}` is in the bundled database: {err}", case.event));
        let judged = case.judged_range();
        let calendars: Vec<(&str, &[BlockPlan])> = case
            .participants
            .iter()
            .map(|participant| {
                (
                    participant.id.as_str(),
                    participant.calendar.as_deref().unwrap_or(&[]),
                )
            })
            .collect();
        let events = counted_events(&case.occurrences, &calendars, &case.exclude_event_keys);
        let pooled = pooled_caps(case, db, &event, &events);

        let context = OracleContext {
            case,
            judged,
            policies: applicable_policies(case),
            events,
            pooled,
        };
        let members: Vec<Member<'_>> = case
            .participants
            .iter()
            .enumerate()
            .map(|(index, _)| resolve_member(&context, db, &event, index))
            .collect();

        let (grid_step, grid_anchor, grid_zone) = resolve_grid(case, db, &event);
        Self {
            case,
            event,
            members,
            grid_step,
            grid_anchor,
            grid_zone,
        }
    }

    /// The candidate starts the case's grid or supplied candidates produce,
    /// sorted and inside the search window.
    #[must_use]
    pub fn candidate_starts(&self) -> Vec<Instant> {
        if let Some(candidates) = &self.case.candidates {
            let mut kept: Vec<Instant> = candidates
                .iter()
                .copied()
                .filter(|start| self.case.in_window(*start))
                .collect();
            kept.sort_unstable();
            kept.dedup();
            return kept;
        }
        let mut starts = Vec::new();
        for minute in self.case.search.0.minutes()..self.case.search.1.minutes() {
            let Ok(instant) = Instant::from_minutes(minute) else {
                continue;
            };
            let local = instant.zoned(&self.grid_zone);
            let minute_of_day = i64::from(local.time().minute_of_day());
            if (minute_of_day - self.grid_anchor).rem_euclid(self.grid_step) == 0 {
                starts.push(instant);
            }
        }
        starts
    }

    /// The reference model's decision for every candidate start, in start order.
    #[must_use]
    pub fn decisions(&self) -> Vec<(Instant, Decision)> {
        self.candidate_starts()
            .into_iter()
            .map(|start| (start, self.decide(start)))
            .collect()
    }

    /// The reference model's decision for `start`.
    #[must_use]
    pub fn decide(&self, start: Instant) -> Decision {
        let first: Vec<Option<ReasonCode>> = (0..self.members.len())
            .map(|index| self.first_reason(index, start))
            .collect();
        let eligible = self.eligible_hosts(&first);
        let short = self
            .case
            .terms
            .iter()
            .zip(&eligible)
            .any(|(term, hosts)| hosts.len() < term.count());
        let assignment = if short {
            None
        } else {
            self.assignment(&eligible)
        };
        let attendee_unavailable =
            self.members.iter().enumerate().any(|(index, member)| {
                member.plan.role == Role::Attendee && first[index].is_some()
            });

        // A term short of eligible hosts is already explained by each blocked
        // host's own reason, so the start-level reason appears only when no
        // participant's reason says it: the joint case, where every term has
        // enough eligible hosts and none of them can fill the terms together, and
        // the case where the requirement fills but an attendee is not free.
        let start_reason = if short {
            None
        } else {
            match (&assignment, attendee_unavailable) {
                (None, _) => Some(ReasonCode::TermUnfillable),
                (Some(_), true) => Some(ReasonCode::AttendeeUnavailable),
                (Some(_), false) => None,
            }
        };

        match (&assignment, start_reason) {
            (Some(assignment), None) => Decision::Offered {
                eligible: eligible
                    .iter()
                    .map(|hosts| {
                        hosts
                            .iter()
                            .map(|index| self.id(*index).to_owned())
                            .collect()
                    })
                    .collect(),
                assignment: assignment.clone(),
                reserve: self.reserve(start, assignment),
                calendar_unknown: assignment.iter().flatten().any(|id| {
                    self.members
                        .iter()
                        .find(|member| member.plan.id == *id)
                        .is_some_and(|member| member.plan.calendar.is_none())
                }),
            },
            (_, start_reason) => {
                let mut reasons: Vec<(Option<String>, ReasonCode)> = self
                    .reported_order()
                    .into_iter()
                    .filter_map(|index| {
                        first[index].map(|code| (Some(self.id(index).to_owned()), code))
                    })
                    .collect();
                if let Some(code) = start_reason {
                    reasons.push((None, code));
                }
                Decision::Excluded(reasons)
            }
        }
    }

    /// The first failing reason for the participant with this ID at `start`.
    #[must_use]
    pub fn first_reason_of(&self, id: &str, start: Instant) -> Option<ReasonCode> {
        let index = self
            .members
            .iter()
            .position(|member| member.plan.id == id)?;
        self.first_reason(index, start)
    }

    /// The ids an exclusion entry reports: every attendee and every host at least
    /// one term selects, ascending by ID.
    fn reported_order(&self) -> Vec<usize> {
        let mut reported: Vec<usize> = (0..self.members.len())
            .filter(|index| {
                self.members[*index].plan.role == Role::Attendee
                    || self
                        .case
                        .terms
                        .iter()
                        .any(|term| term.filled_by(self.members[*index].plan))
            })
            .collect();
        reported.sort_by(|left, right| self.id(*left).cmp(self.id(*right)));
        reported
    }

    /// Each term's eligible hosts, ascending by ID.
    fn eligible_hosts(&self, first: &[Option<ReasonCode>]) -> Vec<Vec<usize>> {
        self.case
            .terms
            .iter()
            .map(|term| {
                let mut hosts: Vec<usize> = (0..self.members.len())
                    .filter(|index| term.filled_by(self.members[*index].plan))
                    .filter(|index| first[*index].is_none())
                    .collect();
                hosts.sort_by(|left, right| self.id(*left).cmp(self.id(*right)));
                hosts
            })
            .collect()
    }

    /// "The assignment with the highest total weight and, among those, the
    /// lexicographically smallest per-term sorted ID list, decided by brute force.
    fn assignment(&self, eligible: &[Vec<usize>]) -> Option<Vec<Vec<String>>> {
        let terms: Vec<BruteTerm> = self
            .case
            .terms
            .iter()
            .zip(eligible)
            .map(|(term, hosts)| BruteTerm {
                count: term.count(),
                candidates: hosts
                    .iter()
                    .map(|index| (self.id(*index).to_owned(), self.members[*index].plan.weight))
                    .collect(),
            })
            .collect();
        brute_assign(&terms)
    }

    /// The travel every participant the booking needs occupies: the assigned
    /// hosts and every attendee, sorted by participant ID then start.
    fn reserve(&self, start: Instant, assignment: &[Vec<String>]) -> Vec<(String, (i64, i64))> {
        let end = start.minutes() + i64::from(self.case.duration);
        let mut reserve = Vec::new();
        let mut ids: Vec<&str> = assignment.iter().flatten().map(String::as_str).collect();
        ids.extend(
            self.members
                .iter()
                .filter(|member| member.plan.role == Role::Attendee)
                .map(|member| member.plan.id.as_str()),
        );
        ids.sort_unstable();
        for id in ids {
            let (before, after) = self.case.travel.into_pair();
            if before > 0 {
                reserve.push((id.to_owned(), (start.minutes() - before, start.minutes())));
            }
            if after > 0 {
                reserve.push((id.to_owned(), (end, end + after)));
            }
        }
        reserve
    }

    fn id(&self, index: usize) -> &str {
        &self.members[index].plan.id
    }

    /// The participant's own span, without travel, in Unix minutes.
    fn own_span(&self, start: Instant) -> (i64, i64) {
        (
            start.minutes(),
            start.minutes() + i64::from(self.case.duration),
        )
    }

    /// The booking's occupied span, with travel, in Unix minutes.
    fn occupied_span(&self, start: Instant) -> (i64, i64) {
        let own = self.own_span(start);
        let (before, after) = self.case.travel.into_pair();
        (own.0 - before, own.1 + after)
    }

    /// The first reason the participant cannot take `start`, in the response
    /// contract's precedence order.
    fn first_reason(&self, index: usize, start: Instant) -> Option<ReasonCode> {
        let member = &self.members[index];
        let own = self.own_span(start);
        let occupied = self.occupied_span(start);
        if occupied.0 < self.case.now.minutes() + member.lead_min {
            return Some(ReasonCode::LeadTimeMin);
        }
        if member
            .bounds
            .iter()
            .any(|bound| !bound.allows(start, &self.event))
        {
            return Some(ReasonCode::LeadTimeMax);
        }
        if !member.windows.iter().all(|flags| flags.contains_span(own)) {
            return Some(ReasonCode::OutsideWindow);
        }
        if member.blackouts.iter().any(|flags| flags.overlaps(own)) {
            return Some(ReasonCode::Blackout);
        }
        if block_overlap(member, own).is_some() {
            return Some(ReasonCode::Blocked);
        }
        // Travel is judged only once the booking's own span is clear, on the
        // travel spans either side of it.
        let (before, after) = self.case.travel.into_pair();
        for (minutes, span) in [(before, (occupied.0, own.0)), (after, (own.1, occupied.1))] {
            if minutes == 0 {
                continue;
            }
            if !member.windows.iter().all(|flags| flags.contains_span(span))
                || member.blackouts.iter().any(|flags| flags.overlaps(span))
                || block_overlap(member, span).is_some()
            {
                return Some(ReasonCode::Travel);
            }
        }
        if clearance_overlap(member, occupied).is_some() {
            return Some(ReasonCode::Clearance);
        }
        if member
            .rests
            .iter()
            .any(|(rest, days)| rest_excludes(rest, &member.zones, days, occupied))
        {
            return Some(ReasonCode::Rest);
        }
        if member.caps.iter().any(|cap| {
            cap.excluded(start, Some(&member.zones), &self.event, self.case.duration)
                .is_some()
        }) {
            return Some(ReasonCode::CapReached);
        }
        None
    }

    /// The `cap_reached` parameters for the first cap that excludes the
    /// participant at `start`, if one does.
    #[must_use]
    pub fn first_cap_params_of(&self, id: &str, start: Instant) -> Option<(u64, u32, String)> {
        let index = self
            .members
            .iter()
            .position(|member| member.plan.id == id)?;
        let member = &self.members[index];
        member.caps.iter().find_map(|cap| {
            cap.excluded(start, Some(&member.zones), &self.event, self.case.duration)
        })
    }
}

/// A blocking event whose occupied span overlaps `span`.
fn block_overlap<'a>(member: &Member<'a>, span: (i64, i64)) -> Option<&'a BlockPlan> {
    member
        .blocks
        .iter()
        .find(|blocked| overlaps(blocked.occupied, span))
        .map(|blocked| blocked.plan)
}

/// A blocking event whose clearance-widened span overlaps `span` without its own
/// occupied span doing so.
fn clearance_overlap<'a>(member: &Member<'a>, span: (i64, i64)) -> Option<&'a BlockPlan> {
    member
        .blocks
        .iter()
        .find(|blocked| overlaps(blocked.widened, span) && !overlaps(blocked.occupied, span))
        .map(|blocked| blocked.plan)
}

impl TravelPlan {
    fn into_pair(self) -> (i64, i64) {
        (i64::from(self.before), i64::from(self.after))
    }
}

fn applicable_policies(case: &A2Case) -> Vec<Vec<&StatusPolicyPlan>> {
    case.participants
        .iter()
        .enumerate()
        .map(|(index, _)| {
            case.rules
                .iter()
                .filter(|rule| rule.applies_to(index))
                .filter_map(|rule| match &rule.plan {
                    RulePlan::StatusPolicy(plan) => Some(plan),
                    _ => None,
                })
                .collect()
        })
        .collect()
}

/// The request-wide pieces every member is resolved against.
struct OracleContext<'a> {
    case: &'a A2Case,
    judged: (i64, i64),
    policies: Vec<Vec<&'a StatusPolicyPlan>>,
    events: Vec<crate::oracle_support::CountedPlanEvent>,
    pooled: Vec<(String, CapModel)>,
}

fn resolve_member<'a>(
    context: &OracleContext<'a>,
    db: &TimeZoneDatabase,
    event: &Zone,
    index: usize,
) -> Member<'a> {
    let OracleContext {
        case,
        judged,
        policies,
        events,
        pooled,
    } = context;
    let participant = &case.participants[index];
    let applies = |rule: &A2Rule| rule.applies_to(index);
    let zones = participant.zones.resolve(db);

    let mut windows = Vec::new();
    let mut blackouts = Vec::new();
    let mut rests = Vec::new();
    let mut caps = Vec::new();
    let mut lead_min = 0;
    let mut bounds = Vec::new();
    for rule in case.rules.iter().filter(|rule| applies(rule)) {
        match &rule.plan {
            RulePlan::Window(plan) => {
                windows.push(window_flags(
                    case,
                    db,
                    participant,
                    &zones,
                    event,
                    *judged,
                    plan,
                ));
            }
            RulePlan::Blackout(plan) => {
                blackouts.push(blackout_flags(
                    case,
                    db,
                    participant,
                    &zones,
                    event,
                    *judged,
                    plan,
                ));
            }
            RulePlan::LeadTime(plan) => {
                lead_min = lead_min.max(i64::from(plan.min));
                if let Some(max) = plan.max {
                    bounds.push(resolve_bound(max, case.now, event));
                }
            }
            RulePlan::Rest(plan) => rests.push(plan),
            RulePlan::Cap(plan) => {
                caps.extend(member_cap(
                    plan,
                    &case.part_tags,
                    participant,
                    &zones,
                    db,
                    event,
                    events,
                    pooled,
                ));
            }
            RulePlan::Clearance { .. } | RulePlan::StatusPolicy(_) => {}
        }
    }

    let (blocks, rests) = member_events(participant, rests, case, applies, policies, &zones, index);

    Member {
        plan: participant,
        zones,
        windows,
        blackouts,
        blocks,
        rests,
        caps,
        lead_min,
        bounds,
    }
}

/// One member's blocking events and the activity dates each rest rule reads.
fn member_events<'a>(
    participant: &'a A2Participant,
    rests: Vec<&'a RestPlan>,
    case: &'a A2Case,
    applies: impl Fn(&A2Rule) -> bool,
    policies: &[Vec<&'a StatusPolicyPlan>],
    zones: &ResolvedZones,
    index: usize,
) -> (Vec<ResolvedBlock<'a>>, Vec<(&'a RestPlan, RestDays)>) {
    let clearance = member_clearance(case, applies);
    let at_start = i64::from(clearance.after.max(clearance.min_gap));
    let at_end = i64::from(clearance.before.max(clearance.min_gap));
    let empty: &[&StatusPolicyPlan] = &[];
    let policies = policies.get(index).map_or(empty, Vec::as_slice);
    let excluded: std::collections::BTreeSet<&str> =
        case.exclude_event_keys.iter().map(String::as_str).collect();
    let blocks: Vec<ResolvedBlock<'_>> = participant
        .calendar
        .iter()
        .flatten()
        .filter(|block| {
            !block
                .event_key
                .as_deref()
                .is_some_and(|key| excluded.contains(key))
                && blocks(block, policies)
        })
        .map(|block| {
            let occupied = (
                block.span.0.minutes() - i64::from(block.travel_before),
                block.span.1.minutes() + i64::from(block.travel_after),
            );
            ResolvedBlock {
                occupied,
                widened: (occupied.0 - at_start, occupied.1 + at_end),
                plan: block,
            }
        })
        .collect();
    // Rest activity is the occupied spans of blocking, non-all-day blocks; the
    // booking is added by the model itself.
    let activity: Vec<(i64, i64)> = blocks
        .iter()
        .filter(|block| !block.plan.all_day)
        .map(|block| block.occupied)
        .collect();
    let rests = rests
        .into_iter()
        .map(|plan| (plan, rest_activity_days(plan, zones, &activity)))
        .collect();
    (blocks, rests)
}

/// The cap models one member contributes: its own usage for an `each` cap, and
/// the shared model for a pooled cap the context already resolved.
#[allow(clippy::too_many_arguments)] // one cap's inputs, each named at the call site
fn member_cap(
    plan: &CapPlan,
    part_tags: &[String],
    participant: &A2Participant,
    zones: &ResolvedZones,
    db: &TimeZoneDatabase,
    event: &Zone,
    events: &[crate::oracle_support::CountedPlanEvent],
    pooled: &[(String, CapModel)],
) -> Option<CapModel> {
    // A cap applies to a booking only when the part's tags match its `count`.
    if !plan.counts_any(part_tags) {
        return None;
    }
    let fixed = cap_zone(plan, db, event);
    Some(match plan.per {
        opportune::model::CapPer::Each => {
            let id = participant.id.as_str();
            let usage = cap_usage(
                plan,
                events,
                Some(id),
                &[id],
                fixed.as_ref(),
                Some(zones),
                event,
            );
            CapModel::new(plan.clone(), fixed, usage)
        }
        opportune::model::CapPer::Pooled => pooled
            .iter()
            .find(|(rule_id, _)| rule_id == &plan.id)
            .map(|(_, model)| model.clone())?,
    })
}

/// Every pooled cap that applies to the request, with the usage it already
/// holds. One model serves every covered participant: a pool shares its bucket.
fn pooled_caps(
    case: &A2Case,
    db: &TimeZoneDatabase,
    event: &Zone,
    events: &[crate::oracle_support::CountedPlanEvent],
) -> Vec<(String, CapModel)> {
    let mut models = Vec::new();
    for rule in &case.rules {
        let RulePlan::Cap(plan) = &rule.plan else {
            continue;
        };
        if !rule.applies_to_part
            || plan.per != opportune::model::CapPer::Pooled
            || !plan.counts_any(&case.part_tags)
        {
            continue;
        }
        let covered: Vec<&str> = case
            .participants
            .iter()
            .enumerate()
            .filter(|(index, _)| rule.who.get(*index).copied().unwrap_or(false))
            .map(|(_, participant)| participant.id.as_str())
            .collect();
        if covered.is_empty() {
            continue;
        }
        let fixed = cap_zone(plan, db, event);
        let usage = cap_usage(plan, events, None, &covered, fixed.as_ref(), None, event);
        models.push((plan.id.clone(), CapModel::new(plan.clone(), fixed, usage)));
    }
    models
}

/// The fixed zone a cap reads its buckets in, or `None` for the participant's
/// own zone.
fn cap_zone(plan: &CapPlan, db: &TimeZoneDatabase, event: &Zone) -> Option<Zone> {
    match &plan.zone {
        crate::a1::RuleZonePlan::Participant => None,
        crate::a1::RuleZonePlan::Event => Some(event.clone()),
        crate::a1::RuleZonePlan::Named(name) => Some(resolved_named(db, name)),
    }
}

/// Clearance combines across the rules that apply to one participant, not across
/// the request: each gap is the largest asked for, never a sum.
fn member_clearance(case: &A2Case, applies: impl Fn(&A2Rule) -> bool) -> ClearancePlan {
    let mut clearance = ClearancePlan::default();
    for rule in case.rules.iter().filter(|rule| applies(rule)) {
        let RulePlan::Clearance {
            clearance: plan, ..
        } = &rule.plan
        else {
            continue;
        };
        clearance.before = clearance.before.max(plan.before);
        clearance.after = clearance.after.max(plan.after);
        clearance.min_gap = clearance.min_gap.max(plan.min_gap);
    }
    clearance
}

/// Minute membership of one window rule, read through the participant's zones.
fn window_flags(
    case: &A2Case,
    db: &TimeZoneDatabase,
    participant: &A2Participant,
    zones: &ResolvedZones,
    event: &Zone,
    judged: (i64, i64),
    plan: &crate::a1::WindowPlan,
) -> MinuteFlags {
    let named = plan.zone.zone_name().unwrap_or(&case.event);
    let named_zone = resolved_named(db, named);
    let membership = WindowCase {
        zone: plan.zone.plan_zone(),
        zones: participant.zones.clone(),
        event: case.event.clone(),
        named: named.to_owned(),
        weekly: plan.weekly.clone(),
        dates: plan.dates.clone(),
        from: case.search.0,
        until: case.search.1,
    };
    MinuteFlags::over(judged.0, judged.1, |minute| {
        Instant::from_minutes(minute)
            .is_ok_and(|instant| membership.contains(instant, zones, event, &named_zone))
    })
}

/// Minute membership of one blackout rule, read through the participant's zones.
fn blackout_flags(
    case: &A2Case,
    db: &TimeZoneDatabase,
    participant: &A2Participant,
    zones: &ResolvedZones,
    event: &Zone,
    judged: (i64, i64),
    plan: &crate::a1::BlackoutPlan,
) -> MinuteFlags {
    let named = plan.zone.zone_name().unwrap_or(&case.event);
    let named_zone = resolved_named(db, named);
    let membership = BlackoutCase {
        zone: plan.zone.plan_zone(),
        zones: participant.zones.clone(),
        event: case.event.clone(),
        named: named.to_owned(),
        spans: plan
            .spans
            .iter()
            .map(|((start_date, start), (end_date, end))| {
                (
                    (*start_date, minute_of_day(*start)),
                    (*end_date, minute_of_day(*end)),
                )
            })
            .collect(),
        from: case.search.0,
        until: case.search.1,
    };
    MinuteFlags::over(judged.0, judged.1, |minute| {
        Instant::from_minutes(minute)
            .is_ok_and(|instant| membership.contains(instant, zones, event, &named_zone))
    })
}

fn resolved_named(db: &TimeZoneDatabase, name: &str) -> Zone {
    db.zone(name)
        .unwrap_or_else(|err| panic!("`{name}` is in the bundled database: {err}"))
}

fn resolve_grid(case: &A2Case, db: &TimeZoneDatabase, event: &Zone) -> (i64, i64, Zone) {
    match &case.grid {
        Some(grid) => {
            let zone = match &grid.zone {
                GridZonePlan::Event => event.clone(),
                GridZonePlan::Named(name) => resolved_named(db, name),
            };
            (i64::from(grid.step), i64::from(grid.anchor), zone)
        }
        None => (i64::from(case.duration.clamp(1, 30)), 0, event.clone()),
    }
}

/// Compares the engine's answer for `case` with the oracle's over the whole
/// search window: every offered start with its eligible pools, assignment,
/// reserve and flag; every exclusion with its reasons, participant by
/// participant; and `check_slot`'s verdict on the offers and on the first few
/// exclusions.
///
/// A requirement the case cannot fill jointly is structurally impossible, so the
/// comparison pins the engine's rejection of it rather than an answer.
///
/// The comparison returns its first disagreement as a [`TestCaseError`], so a
/// property test shrinks it and a stress run reports it. It is the one
/// implementation of this comparison: the property gate runs it on a few hundred
/// generated cases and the stress run on tens of thousands.
///
/// # Errors
/// Returns the first disagreement between the engine and the oracle as a
/// `TestCaseError` carrying both sides of the comparison.
///
/// # Panics
/// Panics when the engine fails on a request the generator emitted, which is
/// itself a disagreement the caller must not read past.
pub fn compare_with_oracle(
    case: &A2Case,
    db: &TimeZoneDatabase,
    engine: &Engine,
) -> Result<(), TestCaseError> {
    let request = case.to_request();
    let oracle = A2Oracle::new(case, db);
    let expected = oracle.decisions();
    let diagnostics = engine.validate(&request);

    if !case.statically_fillable() {
        // A request whose terms cannot be filled jointly is structurally
        // impossible, so the engine rejects it rather than answering with a
        // partial assignment.
        prop_assert!(
            !diagnostics.is_valid(),
            "an unfillable requirement passed validation"
        );
        for error in &diagnostics.errors {
            prop_assert_eq!(error.code(), "requirement_unfillable");
        }
        prop_assert_eq!(
            engine.find_availability(&request).unwrap_err().code(),
            "invalid_request"
        );
        return Ok(());
    }

    prop_assert!(
        diagnostics.is_valid(),
        "the generator emitted an invalid request: {:?}",
        diagnostics.errors
    );
    let response = engine.find_availability(&request).unwrap();
    prop_assert!(!response.truncated, "the case's offer limit truncated");

    let offered: Vec<&(Instant, Decision)> = expected
        .iter()
        .filter(|(_, decision)| matches!(decision, Decision::Offered { .. }))
        .collect();
    let excluded: Vec<&(Instant, Decision)> = expected
        .iter()
        .filter(|(_, decision)| matches!(decision, Decision::Excluded(_)))
        .collect();

    prop_assert_eq!(
        offered_starts(&response),
        offered.iter().map(|(start, _)| *start).collect::<Vec<_>>(),
        "offered starts"
    );
    prop_assert_eq!(
        response
            .excluded
            .iter()
            .map(|entry| entry.start)
            .collect::<Vec<_>>(),
        excluded.iter().map(|(start, _)| *start).collect::<Vec<_>>(),
        "excluded starts"
    );
    for offer in &response.offers {
        let start = offer.start().expect("an offer has an event");
        prop_assert!(
            !response.excluded.iter().any(|entry| entry.start == start),
            "{} is both offered and excluded",
            start
        );
    }

    assert_offers(engine, &request, &response, &offered, &oracle)?;
    assert_exclusions(&response, &excluded, &oracle)?;
    assert_check_slot_agrees(engine, case, &request, &oracle, &excluded)
}

/// Every offer's start, in order.
///
/// # Panics
/// Panics when an offer carries no event, which only a hand-built `Response`
/// does: every offer the engine produces has at least one event.
#[must_use]
pub fn offered_starts(response: &Response) -> Vec<Instant> {
    response
        .offers
        .iter()
        .map(|offer| offer.start().expect("an offer has an event"))
        .collect()
}

/// The reserve spans as `(participant, (from, until))` in Unix minutes, which is
/// how the oracle carries them.
fn reserve_spans(offer: &Offer) -> Vec<(String, (i64, i64))> {
    offer.events[0]
        .reserve
        .iter()
        .map(|span| {
            (
                span.participant.clone(),
                (span.span.start().minutes(), span.span.end().minutes()),
            )
        })
        .collect()
}

/// The `(participant, code)` pairs an exclusion entry reports, in order.
#[must_use]
pub fn excluded_reasons(entry: &opportune::reasons::Excluded) -> Vec<(Option<String>, ReasonCode)> {
    entry
        .reasons
        .iter()
        .map(|reason| (reason.participant.clone(), reason.code))
        .collect()
}

/// The booking a multi-participant case proposes at `start`.
fn proposal(start: Instant, assignment: Vec<Vec<String>>) -> Proposal {
    Proposal {
        events: vec![ProposalEvent {
            part: "main".to_owned(),
            start,
            assignment,
        }],
    }
}

/// Every offer carries the oracle's eligible pools, assignment, reserve and flag,
/// confirms with `check_slot`, and leaves no applicable cap exceeded once the
/// booking's contribution is added.
fn assert_offers(
    engine: &Engine,
    request: &Request,
    response: &Response,
    offered: &[&(Instant, Decision)],
    oracle: &A2Oracle<'_>,
) -> Result<(), TestCaseError> {
    for (offer, (start, decision)) in response.offers.iter().zip(offered) {
        let Decision::Offered {
            eligible,
            assignment,
            reserve,
            calendar_unknown,
        } = decision
        else {
            unreachable!("filtered to offered decisions");
        };
        let event = &offer.events[0];
        prop_assert_eq!(&event.start, start);
        prop_assert_eq!(&event.eligible, eligible, "eligible hosts at {}", start);
        prop_assert_eq!(&event.assignment, assignment, "assignment at {}", start);
        prop_assert_eq!(&reserve_spans(offer), reserve, "reserve at {}", start);
        prop_assert_eq!(
            offer.calendar_unknown(),
            *calendar_unknown,
            "calendar_unknown at {}",
            start
        );

        let check = engine
            .check_slot(request, &proposal(*start, event.assignment.clone()))
            .unwrap();
        prop_assert!(
            check.valid,
            "an offered start failed check_slot at {}: {:?}",
            start,
            check.reasons
        );
        prop_assert!(check.reasons.is_empty());
        prop_assert_eq!(&check.reserve, &event.reserve);
        prop_assert_eq!(&check.flags, &offer.flags);

        // The oracle computes each cap's usage independently, so an offer it
        // says is over a limit is a real breach of the workload invariant rather
        // than a shared blind spot.
        let attendees = request
            .participants
            .iter()
            .filter(|participant| participant.role == Role::Attendee)
            .map(|participant| participant.id.as_str());
        for id in event
            .assignment
            .iter()
            .flatten()
            .map(String::as_str)
            .chain(attendees)
        {
            prop_assert_eq!(
                oracle.first_cap_params_of(id, *start),
                None,
                "an offered start leaves a cap exceeded for {}",
                id
            );
        }
    }
    Ok(())
}

/// Every exclusion carries exactly the reasons the oracle reports, participant by
/// participant and in the same order, and a capped start reports the bucket usage
/// the oracle computes independently.
fn assert_exclusions(
    response: &Response,
    excluded: &[&(Instant, Decision)],
    oracle: &A2Oracle<'_>,
) -> Result<(), TestCaseError> {
    for (entry, (start, decision)) in response.excluded.iter().zip(excluded) {
        let Decision::Excluded(reasons) = decision else {
            unreachable!("filtered to excluded decisions");
        };
        prop_assert_eq!(
            excluded_reasons(entry),
            reasons.clone(),
            "reasons at {}",
            start
        );
        for reason in &entry.reasons {
            if reason.code != ReasonCode::CapReached {
                continue;
            }
            let Some(id) = reason.participant.as_deref() else {
                continue;
            };
            let expected = oracle.first_cap_params_of(id, entry.start);
            let actual = reason.params.as_ref().and_then(|params| match params {
                ReasonParams::CapReached {
                    used,
                    limit,
                    bucket,
                } => Some((*used, *limit, bucket.clone())),
                _ => None,
            });
            prop_assert_eq!(actual, expected, "cap parameters for {} at {}", id, start);
        }
    }
    Ok(())
}

/// `check_slot` agrees with the search for sampled excluded starts: every
/// participant it judges has the same first failing reason.
fn assert_check_slot_agrees(
    engine: &Engine,
    case: &A2Case,
    request: &Request,
    oracle: &A2Oracle<'_>,
    excluded: &[&(Instant, Decision)],
) -> Result<(), TestCaseError> {
    let probe = case
        .probe_assignment()
        .expect("a statically fillable requirement has a probe assignment");
    let mut judged: Vec<&str> = case
        .participants
        .iter()
        .filter(|participant| participant.role == Role::Attendee)
        .map(|participant| participant.id.as_str())
        .collect();
    judged.extend(probe.iter().flatten().map(String::as_str));
    judged.sort_unstable();
    judged.dedup();

    for (start, _) in excluded.iter().take(3) {
        let check = engine
            .check_slot(request, &proposal(*start, probe.clone()))
            .unwrap();
        prop_assert!(
            !check.valid,
            "an excluded start passed check_slot at {}",
            start
        );
        for id in judged.iter().copied() {
            let confirmed = check
                .reasons
                .iter()
                .find(|reason| reason.participant.as_deref() == Some(id))
                .map(|reason| reason.code);
            let expected = oracle.first_reason_of(id, *start);
            prop_assert_eq!(confirmed, expected, "participant {} at {}", id, start);
        }
    }
    Ok(())
}
