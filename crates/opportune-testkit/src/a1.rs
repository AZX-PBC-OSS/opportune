//! A whole-request reference model for the single-participant engine.
//!
//! An [`A1Case`] is the plain description a generator produces: one host with a
//! zone and a calendar, a goal with a grid or supplied candidates, travel, and a
//! list of the seven rule kinds this version evaluates. [`A1Case::to_request`]
//! turns that description into the engine's domain types, so a test can feed the
//! same case to both sides. A cap here measures the host's own blocks, because an
//! `A1Case` carries no occurrences; the multi-participant model carries those.
//!
//! [`A1Oracle`] decides each candidate start straight from the specification
//! text: it converts every minute of interest to local time with `jiff` (through
//! [`Zone::zoned`](opportune::time::Zone::zoned)) and applies the documented
//! membership rules, sharing none of the engine's interval-set or breakpoint
//! code. The eligibility rules are applied in the response contract's precedence
//! order, so a disagreement shows up as a different decision, not just a
//! different list of offers.
//!
//! The oracle deliberately does not mirror the engine's structure: it walks
//! minutes and keeps booleans, where the engine coalesces half-open spans.

use std::collections::BTreeMap;

use opportune::Engine;
use opportune::model::{
    Block, BlockStatus, Cap, CapMeasure, CapPer, CapPeriod, Clearance, Explain, Goal, LeadTime,
    LeadTimeMax, Occurrence, Part, Participant, Proposal, ProposalEvent, Request, Requirement,
    Rule, RuleKind, Span, StatusEffect, StatusPolicy, StatusPolicyEntry, Term, Travel, ZonePeriod,
};
use opportune::reasons::ReasonCode;
use opportune::tags::{Role, Selector, Tag};
use opportune::time::{
    BlackoutSpan, Date, DateKey, DateRange, Duration, Grid, GridZone, Instant, LocalDateTime,
    RuleZone, TimeOfDay, TimeZoneDatabase, WallEnd, WallRange, Weekdays, WeeklyKey, WindowSpec,
    Zone, ZoneName,
};
use proptest::prelude::{prop_assert, prop_assert_eq};
use proptest::test_runner::TestCaseError;

use crate::oracle_support::{
    CapModel, LeadBound, MinuteFlags, RestDays, blocks, cap_usage, counted_events, minute_of_day,
    overlaps, resolve_bound, rest_activity_days, rest_excludes,
};
use crate::specs::{BlackoutCase, DateRangePlan, PlanZone, ResolvedZones, WindowCase, ZonePlan};

/// The offer limit a converted case asks for, high enough that no generated case
/// is truncated. Truncation is exercised deliberately by its own property test.
const OFFER_LIMIT: usize = 100_000;

/// The explanation limit a converted case asks for, the engine's default maximum.
/// Generated cases report far fewer reasons than this, so exclusions are whole.
const EXPLAIN_LIMIT: usize = 10_000;

/// Which zone a generated rule is read in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RuleZonePlan {
    /// The participant's zone at each instant.
    Participant,
    /// The event's zone.
    Event,
    /// A named IANA zone.
    Named(String),
}

impl RuleZonePlan {
    pub(crate) fn plan_zone(&self) -> PlanZone {
        match self {
            Self::Participant => PlanZone::Participant,
            Self::Event => PlanZone::Event,
            Self::Named(_) => PlanZone::Named,
        }
    }

    pub(crate) fn zone_name(&self) -> Option<&str> {
        match self {
            Self::Named(name) => Some(name),
            Self::Participant | Self::Event => None,
        }
    }
}

/// The zone a candidate grid is read in. A grid in the participant's zone is not
/// expressible, because a participant's zone is piecewise per instant.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GridZonePlan {
    /// The event's zone.
    Event,
    /// A named IANA zone.
    Named(String),
}

/// A generated candidate grid.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GridPlan {
    /// Step in minutes, 1 to 1440.
    pub step: u32,
    /// Local minute of day the grid is anchored at.
    pub anchor: u16,
    /// The zone the local clock is read in.
    pub zone: GridZonePlan,
}

/// Travel the new booking needs, in each direction.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TravelPlan {
    /// Minutes before the booking.
    pub before: u32,
    /// Minutes after the booking.
    pub after: u32,
}

/// One generated block on the participant's calendar.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlockPlan {
    /// The block's own half-open span.
    pub span: (Instant, Instant),
    /// How the block affects availability.
    pub status: BlockStatus,
    /// Whether the source marked it all day.
    pub all_day: bool,
    /// Everything the block is tagged with.
    pub tags: Vec<String>,
    /// The caller's ID for the event this block is one instance of.
    pub event_key: Option<String>,
    /// Travel to reach the block.
    pub travel_before: u32,
    /// Travel to leave the block.
    pub travel_after: u32,
}

/// One event the caller can identify, as a plan describes it.
///
/// An occurrence is authoritative for the span and participants of the event it
/// shares an event key with, and a cap counts it once however many calendars
/// carry a matching block.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OccurrencePlan {
    /// The caller's ID for this event instance.
    pub event_key: String,
    /// The event's own span, without travel.
    pub span: (Instant, Instant),
    /// Everything the event is tagged with.
    pub tags: Vec<String>,
    /// The participants the event involves, at least one.
    pub participants: Vec<String>,
}

impl OccurrencePlan {
    pub(crate) fn to_occurrence(&self) -> Occurrence {
        let mut occurrence = Occurrence::new(
            self.event_key.clone(),
            Span::new(self.span.0, self.span.1).expect("an occurrence span is non-empty"),
            self.participants.clone(),
        )
        .expect("an occurrence names at least one participant");
        occurrence.tags = self
            .tags
            .iter()
            .map(|tag| Tag::new(tag).expect("a generated tag is valid"))
            .collect();
        occurrence
    }
}

/// The participant a generated case books.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParticipantPlan {
    /// The participant's ID.
    pub id: String,
    /// The participant's home zone and disjoint travel legs.
    pub zones: ZonePlan,
    /// The participant's blocks, or `None` when no calendar was supplied.
    pub calendar: Option<Vec<BlockPlan>>,
}

/// A generated `window` rule: a weekly and date specification.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WindowPlan {
    /// The rule's ID.
    pub id: String,
    /// The zone the window is read in.
    pub zone: RuleZonePlan,
    /// Weekly keys: a Monday-first day mask and the ranges those days share.
    pub weekly: Vec<(u8, Vec<(u16, u16)>)>,
    /// Date overrides, inclusive and disjoint.
    pub dates: Vec<DateRangePlan>,
    /// Whether the rule's `who` selects this case's host.
    pub applies_to_host: bool,
    /// Whether the rule's `when` matches this case's part tags.
    pub applies_to_part: bool,
}

/// A generated `blackout` rule: local date-time spans.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlackoutPlan {
    /// The rule's ID.
    pub id: String,
    /// The zone the spans are read in.
    pub zone: RuleZonePlan,
    /// Half-open local date-time spans, as `((date, minute), (date, minute))`.
    pub spans: Vec<((Date, u16), (Date, u16))>,
    /// Whether the rule's `who` selects this case's host.
    pub applies_to_host: bool,
    /// Whether the rule's `when` matches this case's part tags.
    pub applies_to_part: bool,
}

/// A generated `clearance` rule.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ClearancePlan {
    /// The gap the booking needs before it.
    pub before: u32,
    /// The gap the booking needs after it.
    pub after: u32,
    /// The least gap between any two events.
    pub min_gap: u32,
    /// Whether the rule's `who` selects this case's host.
    pub applies_to_host: bool,
    /// Whether the rule's `when` matches this case's part tags.
    pub applies_to_part: bool,
}

/// A generated `lead_time` maximum.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LeadMaxPlan {
    /// At most `now + minutes`.
    Duration(u32),
    /// At most `today + days` calendar days in the event zone.
    Days(u32),
    /// At most this local date in the event zone.
    Date(Date),
}

/// A generated `lead_time` rule.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LeadTimePlan {
    /// The rule's ID.
    pub id: String,
    /// Minimum notice in minutes.
    pub min: u32,
    /// The optional latest start.
    pub max: Option<LeadMaxPlan>,
    /// Whether the rule's `who` selects this case's host.
    pub applies_to_host: bool,
    /// Whether the rule's `when` matches this case's part tags.
    pub applies_to_part: bool,
}

/// One generated `status_policy` entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StatusEntryPlan {
    /// The status this entry matches, if it matches on status.
    pub status: Option<BlockStatus>,
    /// The tag this entry matches, if it matches on tags.
    pub tag: Option<String>,
    /// What happens to a matching block.
    pub effect: StatusEffect,
}

/// A generated `status_policy` rule.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StatusPolicyPlan {
    /// The rule's ID.
    pub id: String,
    /// The entries, in the order the caller wrote them.
    pub entries: Vec<StatusEntryPlan>,
    /// Whether the rule's `who` selects this case's host.
    pub applies_to_host: bool,
    /// Whether the rule's `when` matches this case's part tags.
    pub applies_to_part: bool,
}

/// A generated `rest` rule.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RestPlan {
    /// The rule's ID.
    pub id: String,
    /// The local time from which a finish counts as late, in minutes after
    /// midnight.
    pub late_after: u16,
    /// The local time before which a start counts as early, in minutes after
    /// midnight.
    pub not_before: u16,
    /// Whether the rule's `who` selects this case's host.
    pub applies_to_host: bool,
    /// Whether the rule's `when` matches this case's part tags.
    pub applies_to_part: bool,
}

/// A generated `cap` rule.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CapPlan {
    /// The rule's ID.
    pub id: String,
    /// The tags the cap's `count` selector matches. Empty matches every event.
    pub count: Vec<String>,
    /// Whether events or their minutes count.
    pub measure: CapMeasure,
    /// The most a period may hold.
    pub limit: u32,
    /// The civil period a bucket covers.
    pub period: CapPeriod,
    /// Whether the limit applies to each participant or the pool together.
    pub per: CapPer,
    /// The zone the bucket labels are read in.
    pub zone: RuleZonePlan,
    /// The weekday a week bucket starts on.
    pub week_starts: opportune::time::Weekday,
    /// Whether the rule's `who` selects this case's host.
    pub applies_to_host: bool,
    /// Whether the rule's `when` matches this case's part tags.
    pub applies_to_part: bool,
}

impl CapPlan {
    /// Whether the cap's `count` selector matches any of `tags`.
    ///
    /// The selector matches a tag when a named tag is that tag or an ancestor of
    /// it, so `interview` matches `interview/front-end`.
    pub(crate) fn counts_any(&self, tags: &[String]) -> bool {
        self.count.is_empty()
            || self.count.iter().any(|named| {
                tags.iter().any(|tag| {
                    tag == named
                        || tag
                            .strip_prefix(named.as_str())
                            .is_some_and(|rest| rest.starts_with('/'))
                })
            })
    }
}

/// One generated rule of any of the seven kinds. The single-participant oracle
/// models every kind: a cap here counts the lone participant's blocks, because
/// an [`A1Case`] carries no occurrences.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RulePlan {
    /// A window rule.
    Window(WindowPlan),
    /// A blackout rule.
    Blackout(BlackoutPlan),
    /// A clearance rule with its own ID.
    Clearance {
        /// The rule's ID.
        id: String,
        /// The clearance gaps.
        clearance: ClearancePlan,
    },
    /// A lead-time rule.
    LeadTime(LeadTimePlan),
    /// A status-policy rule.
    StatusPolicy(StatusPolicyPlan),
    /// A rest rule.
    Rest(RestPlan),
    /// A cap rule.
    Cap(CapPlan),
}

impl RulePlan {
    /// The rule kind's name, matching the label [`crate::scenarios::rule_kind_scenarios`]
    /// gives a fixture of this kind.
    ///
    /// One spelling of the name for every consumer: the canonical report's header,
    /// the stress run's coverage tally, and anything else that has to say which
    /// kind a generated rule is.
    #[must_use]
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Window(_) => "window",
            Self::Blackout(_) => "blackout",
            Self::Clearance { .. } => "clearance",
            Self::LeadTime(_) => "lead_time",
            Self::StatusPolicy(_) => "status_policy",
            Self::Rest(_) => "rest",
            Self::Cap(_) => "cap",
        }
    }

    pub(crate) fn set_id(&mut self, id: String) {
        match self {
            Self::Window(plan) => plan.id = id,
            Self::Blackout(plan) => plan.id = id,
            Self::Clearance { id: current, .. } => *current = id,
            Self::LeadTime(plan) => plan.id = id,
            Self::StatusPolicy(plan) => plan.id = id,
            Self::Rest(plan) => plan.id = id,
            Self::Cap(plan) => plan.id = id,
        }
    }

    fn applies_to_host(&self) -> bool {
        match self {
            Self::Window(plan) => plan.applies_to_host,
            Self::Blackout(plan) => plan.applies_to_host,
            Self::Clearance { clearance, .. } => clearance.applies_to_host,
            Self::LeadTime(plan) => plan.applies_to_host,
            Self::StatusPolicy(plan) => plan.applies_to_host,
            Self::Rest(plan) => plan.applies_to_host,
            Self::Cap(plan) => plan.applies_to_host,
        }
    }

    fn applies_to_part(&self) -> bool {
        match self {
            Self::Window(plan) => plan.applies_to_part,
            Self::Blackout(plan) => plan.applies_to_part,
            Self::Clearance { clearance, .. } => clearance.applies_to_part,
            Self::LeadTime(plan) => plan.applies_to_part,
            Self::StatusPolicy(plan) => plan.applies_to_part,
            Self::Rest(plan) => plan.applies_to_part,
            Self::Cap(plan) => plan.applies_to_part,
        }
    }

    fn applies(&self) -> bool {
        self.applies_to_host() && self.applies_to_part()
    }
}

/// One generated single-participant request, in plan form.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct A1Case {
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
    /// The host being booked.
    pub participant: ParticipantPlan,
    /// The request's final rule list.
    pub rules: Vec<RulePlan>,
    /// Event keys ignored across blocking, rest, and caps (e.g. a booking being moved).
    pub exclude_event_keys: Vec<String>,
}

impl A1Case {
    /// The engine request this plan describes.
    ///
    /// The conversion is deliberately mechanical: the oracle reads the plan and
    /// the engine reads the request, so a conversion mistake shows up as a
    /// disagreement rather than a shared blind spot.
    ///
    /// # Panics
    /// Panics when a plan is not a valid request — an unparseable tag, zone or
    /// grid, an inverted span, or a rule ID that repeats. Generators emit valid
    /// plans, and scenarios build the domain types directly.
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

        let mut participant = Participant::new(
            &self.participant.id,
            ZoneName::new(&self.participant.zones.home).expect("the home zone is a zone name"),
        );
        participant.zone_periods = self
            .participant
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
            .participant
            .calendar
            .as_ref()
            .map(|blocks| blocks.iter().map(BlockPlan::to_block).collect());

        let mut request = Request::new(self.now, goal);
        request.participants.push(participant);
        request.rules = self.rules.iter().map(RulePlan::to_rule).collect();
        request
            .exclude_event_keys
            .clone_from(&self.exclude_event_keys);
        request.limit = Some(OFFER_LIMIT);
        request.explain = Some(Explain::new(EXPLAIN_LIMIT));
        request
    }

    fn part(&self) -> Part {
        let mut part = Part::new(
            Duration::from_minutes(self.duration),
            Requirement::new(vec![Term::person(self.participant.id.clone())]),
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
}

impl GridPlan {
    pub(crate) fn to_grid(&self) -> Grid {
        Grid::new(
            Duration::from_minutes(self.step),
            TimeOfDay::from_minute_of_day(self.anchor).expect("an anchor below 1440"),
            match &self.zone {
                GridZonePlan::Event => GridZone::Event,
                GridZonePlan::Named(name) => {
                    GridZone::Named(ZoneName::new(name).expect("the grid names a zone"))
                }
            },
        )
        .expect("the generator emits a step between 1 and 1440 minutes")
    }
}

impl BlockPlan {
    pub(crate) fn to_block(&self) -> Block {
        Block {
            span: Span::new(self.span.0, self.span.1).expect("a block span is non-empty"),
            status: self.status,
            all_day: self.all_day,
            tags: self
                .tags
                .iter()
                .map(|tag| Tag::new(tag).expect("a generated tag is valid"))
                .collect(),
            event_key: self.event_key.clone(),
            travel_before: Duration::from_minutes(self.travel_before),
            travel_after: Duration::from_minutes(self.travel_after),
        }
    }
}

impl RulePlan {
    pub(crate) fn to_rule(&self) -> Rule {
        match self {
            Self::Window(plan) => Rule {
                id: plan.id.clone(),
                kind: RuleKind::Window(plan.to_spec()),
                who: who_selector(plan.applies_to_host),
                when: when_selector(plan.applies_to_part),
            },
            Self::Blackout(plan) => Rule {
                id: plan.id.clone(),
                kind: RuleKind::Blackout(plan.to_spec()),
                who: who_selector(plan.applies_to_host),
                when: when_selector(plan.applies_to_part),
            },
            Self::Clearance { id, clearance } => Rule {
                id: id.clone(),
                kind: RuleKind::Clearance(Clearance {
                    before: Duration::from_minutes(clearance.before),
                    after: Duration::from_minutes(clearance.after),
                    min_gap: Duration::from_minutes(clearance.min_gap),
                }),
                who: who_selector(clearance.applies_to_host),
                when: when_selector(clearance.applies_to_part),
            },
            Self::LeadTime(plan) => Rule {
                id: plan.id.clone(),
                kind: RuleKind::LeadTime(LeadTime {
                    min: Duration::from_minutes(plan.min),
                    max: plan.max.map(|max| match max {
                        LeadMaxPlan::Duration(minutes) => {
                            LeadTimeMax::Duration(Duration::from_minutes(minutes))
                        }
                        LeadMaxPlan::Days(days) => LeadTimeMax::Days(days),
                        LeadMaxPlan::Date(date) => LeadTimeMax::Date(date),
                    }),
                }),
                who: who_selector(plan.applies_to_host),
                when: when_selector(plan.applies_to_part),
            },
            Self::StatusPolicy(plan) => Rule {
                id: plan.id.clone(),
                kind: RuleKind::StatusPolicy(StatusPolicy {
                    entries: plan
                        .entries
                        .iter()
                        .map(|entry| StatusPolicyEntry {
                            status: entry.status,
                            tags: entry.tag.as_ref().map(|tag| {
                                Selector::Tag(Tag::new(tag).expect("a generated tag is valid"))
                            }),
                            effect: entry.effect,
                        })
                        .collect(),
                }),
                who: who_selector(plan.applies_to_host),
                when: when_selector(plan.applies_to_part),
            },
            Self::Rest(plan) => Rule {
                id: plan.id.clone(),
                kind: RuleKind::Rest(opportune::model::Rest::new(
                    minute_of_day(plan.late_after),
                    minute_of_day(plan.not_before),
                )),
                who: who_selector(plan.applies_to_host),
                when: when_selector(plan.applies_to_part),
            },
            Self::Cap(plan) => Rule {
                id: plan.id.clone(),
                kind: RuleKind::Cap(Cap::new(
                    if plan.count.is_empty() {
                        Selector::True
                    } else {
                        Selector::Any(
                            plan.count
                                .iter()
                                .map(|tag| {
                                    Selector::Tag(Tag::new(tag).expect("a generated tag is valid"))
                                })
                                .collect(),
                        )
                    },
                    plan.measure,
                    plan.limit,
                    plan.period,
                    plan.per,
                    rule_zone(&plan.zone),
                    plan.week_starts,
                )),
                who: who_selector(plan.applies_to_host),
                when: when_selector(plan.applies_to_part),
            },
        }
    }
}

impl WindowPlan {
    fn to_spec(&self) -> WindowSpec {
        WindowSpec::new(
            rule_zone(&self.zone),
            self.weekly
                .iter()
                .map(|(mask, hours)| {
                    WeeklyKey::new(weekdays(*mask), ranges(hours))
                        .expect("the generator emits disjoint weekly keys")
                })
                .collect(),
            self.dates
                .iter()
                .map(|(start, end, hours)| {
                    DateKey::new(
                        DateRange::new(*start, *end).expect("the generator emits ordered dates"),
                        ranges(hours),
                    )
                    .expect("the generator emits non-overlapping ranges")
                })
                .collect(),
        )
        .expect("the generator emits disjoint date keys")
    }
}

impl BlackoutPlan {
    fn to_spec(&self) -> opportune::time::BlackoutSpec {
        opportune::time::BlackoutSpec::new(
            rule_zone(&self.zone),
            self.spans
                .iter()
                .map(|(start, end)| {
                    BlackoutSpan::new(
                        LocalDateTime::new(start.0, minute_of_day(start.1)),
                        LocalDateTime::new(end.0, minute_of_day(end.1)),
                    )
                    .expect("the generator emits ascending spans")
                })
                .collect(),
        )
    }
}

fn rule_zone(zone: &RuleZonePlan) -> RuleZone {
    match zone {
        RuleZonePlan::Participant => RuleZone::Participant,
        RuleZonePlan::Event => RuleZone::Event,
        RuleZonePlan::Named(name) => {
            RuleZone::Named(ZoneName::new(name).expect("a rule zone names a zone"))
        }
    }
}

/// A `who` selector that selects a host, or one that selects no host at all.
fn who_selector(applies: bool) -> Selector {
    if applies {
        Selector::Role(Role::Host)
    } else {
        Selector::Not(Box::new(Selector::Role(Role::Host)))
    }
}

/// A `when` selector that matches every part, or no part at all.
fn when_selector(applies: bool) -> Selector {
    if applies {
        Selector::True
    } else {
        Selector::Not(Box::new(Selector::True))
    }
}

fn weekdays(mask: u8) -> Weekdays {
    const WEEK: [opportune::time::Weekday; 7] = [
        opportune::time::Weekday::Monday,
        opportune::time::Weekday::Tuesday,
        opportune::time::Weekday::Wednesday,
        opportune::time::Weekday::Thursday,
        opportune::time::Weekday::Friday,
        opportune::time::Weekday::Saturday,
        opportune::time::Weekday::Sunday,
    ];
    Weekdays::list(
        WEEK.into_iter()
            .enumerate()
            .filter(|(index, _)| mask & (1 << index) != 0)
            .map(|(_, day)| day),
    )
}

fn ranges(hours: &[(u16, u16)]) -> Vec<WallRange> {
    hours
        .iter()
        .map(|&(start, end)| {
            let start = minute_of_day(start);
            let end = if end == TimeOfDay::MINUTES_PER_DAY {
                WallEnd::Midnight
            } else {
                WallEnd::Time(minute_of_day(end))
            };
            WallRange::new(start, end).expect("the generator emits non-empty ranges")
        })
        .collect()
}

/// The oracle's decision for one candidate start.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decision {
    /// The start is bookable.
    Offered,
    /// The start is excluded for this reason: the first in the response
    /// contract's precedence order.
    Excluded(ReasonCode),
}

/// A minute-by-minute reference model of one [`A1Case`].
///
/// The model resolves the case once — every rule's minute membership over the
/// judged range, the combined clearance, the lead-time bounds and the blocking
/// events — and then answers one start at a time. Membership is built by
/// converting each minute to local time and applying the specification text, so
/// the model shares no interval or breakpoint code with the engine.
pub struct A1Oracle<'a> {
    case: &'a A1Case,
    event: Zone,
    zones: ResolvedZones,
    windows: Vec<ResolvedWindow>,
    blackouts: Vec<ResolvedBlackout<'a>>,
    blocks: Vec<ResolvedBlock<'a>>,
    rests: Vec<(&'a RestPlan, RestDays)>,
    caps: Vec<CapModel>,
    lead_min: i64,
    bounds: Vec<LeadBound>,
    grid_step: i64,
    grid_anchor: i64,
    grid_zone: Zone,
}

struct ResolvedWindow {
    flags: MinuteFlags,
}

struct ResolvedBlackout<'a> {
    plan: &'a BlackoutPlan,
    flags: MinuteFlags,
}

struct ResolvedBlock<'a> {
    plan: &'a BlockPlan,
    occupied: (i64, i64),
    widened: (i64, i64),
}

impl<'a> A1Oracle<'a> {
    /// Resolves `case` against `db`.
    ///
    /// # Panics
    /// Panics when a zone the plan names is not in the database. Generators draw
    /// zone names from the bundled database's matrix.
    #[must_use]
    pub fn new(case: &'a A1Case, db: &TimeZoneDatabase) -> Self {
        let zones = case.participant.zones.resolve(db);
        let event = resolved_event(case, db);
        let judged = case.judged_range();
        let clearance = combined_clearance(case);
        let policies = applicable_policies(case);
        let windows = resolve_windows(case, db, &zones, &event, judged);
        let blackouts = resolve_blackouts(case, db, &zones, &event, judged);
        let blocks = resolve_blocks(
            &case.participant,
            clearance,
            &policies,
            &case.exclude_event_keys,
        );
        // Rest activity is the occupied spans of blocking, non-all-day blocks;
        // the booking is added by the model itself.
        let activity: Vec<(i64, i64)> = blocks
            .iter()
            .filter(|block| !block.plan.all_day)
            .map(|block| block.occupied)
            .collect();
        let rests: Vec<(&RestPlan, RestDays)> = case
            .rules
            .iter()
            .filter(|rule| rule.applies())
            .filter_map(|rule| match rule {
                RulePlan::Rest(plan) => Some(plan),
                _ => None,
            })
            .map(|plan| (plan, rest_activity_days(plan, &zones, &activity)))
            .collect();
        let caps = resolve_caps(case, db, &event, &zones);
        let (lead_min, bounds) = resolve_lead(case, &event);
        let (grid_step, grid_anchor, grid_zone) = resolve_grid(case, db, &event);

        Self {
            case,
            event,
            zones,
            windows,
            blackouts,
            blocks,
            rests,
            caps,
            lead_min,
            bounds,
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

    /// The oracle's decision for `start`, applying the response contract's
    /// precedence order.
    #[must_use]
    pub fn decide(&self, start: Instant) -> Decision {
        let own = self.own_span(start);
        let occupied = self.occupied_span(start);
        let now = self.case.now.minutes();
        if occupied.0 < now + self.lead_min {
            return Decision::Excluded(ReasonCode::LeadTimeMin);
        }
        if self
            .bounds
            .iter()
            .any(|bound| !bound.allows(start, &self.event))
        {
            return Decision::Excluded(ReasonCode::LeadTimeMax);
        }
        if !self.inside_windows(own) {
            return Decision::Excluded(ReasonCode::OutsideWindow);
        }
        if self.blackout_overlap(own).is_some() {
            return Decision::Excluded(ReasonCode::Blackout);
        }
        if self.block_overlap(own).is_some() {
            return Decision::Excluded(ReasonCode::Blocked);
        }
        if let Some(code) = self.travel_reason(own, occupied) {
            return Decision::Excluded(code);
        }
        if self.clearance_overlap(occupied).is_some() {
            return Decision::Excluded(ReasonCode::Clearance);
        }
        if self
            .rests
            .iter()
            .any(|(rest, days)| rest_excludes(rest, &self.zones, days, occupied))
        {
            return Decision::Excluded(ReasonCode::Rest);
        }
        if self.caps.iter().any(|cap| {
            cap.excluded(start, Some(&self.zones), &self.event, self.case.duration)
                .is_some()
        }) {
            return Decision::Excluded(ReasonCode::CapReached);
        }
        Decision::Offered
    }

    /// The decision for every candidate start, in start order.
    #[must_use]
    pub fn decisions(&self) -> Vec<(Instant, Decision)> {
        self.candidate_starts()
            .into_iter()
            .map(|start| (start, self.decide(start)))
            .collect()
    }

    /// The booking's own span, without travel, in Unix minutes.
    fn own_span(&self, start: Instant) -> (i64, i64) {
        (
            start.minutes(),
            start.minutes() + i64::from(self.case.duration),
        )
    }

    /// The booking's occupied span, with travel, in Unix minutes.
    #[must_use]
    pub fn occupied_span(&self, start: Instant) -> (i64, i64) {
        let own = self.own_span(start);
        (
            own.0 - i64::from(self.case.travel.before),
            own.1 + i64::from(self.case.travel.after),
        )
    }

    /// Whether every applicable window covers `span`.
    #[must_use]
    pub fn inside_windows(&self, span: (i64, i64)) -> bool {
        self.windows
            .iter()
            .all(|window| window.flags.contains_span(span))
    }

    /// The ID of a blackout overlapping `span`, if one does.
    #[must_use]
    pub fn blackout_overlap(&self, span: (i64, i64)) -> Option<&str> {
        self.blackouts
            .iter()
            .find(|blackout| blackout.flags.overlaps(span))
            .map(|blackout| blackout.plan.id.as_str())
    }

    /// A blocking event whose occupied span overlaps `span`, if one does.
    #[must_use]
    pub fn block_overlap(&self, span: (i64, i64)) -> Option<&BlockPlan> {
        self.blocks
            .iter()
            .find(|block| overlaps(block.occupied, span))
            .map(|block| block.plan)
    }

    /// A blocking event whose clearance-widened span overlaps `span` without its
    /// own occupied span doing so: the evidence that the booking is too close
    /// rather than on top of the event.
    #[must_use]
    pub fn clearance_overlap(&self, span: (i64, i64)) -> Option<&BlockPlan> {
        self.blocks
            .iter()
            .find(|block| overlaps(block.widened, span) && !overlaps(block.occupied, span))
            .map(|block| block.plan)
    }

    /// Whether the assigned host has no calendar, so an offer is flagged
    /// `calendar_unknown`.
    #[must_use]
    pub fn calendar_unknown(&self) -> bool {
        self.case.participant.calendar.is_none()
    }

    fn travel_reason(&self, own: (i64, i64), occupied: (i64, i64)) -> Option<ReasonCode> {
        let spans = [
            (self.case.travel.before, (occupied.0, own.0)),
            (self.case.travel.after, (own.1, occupied.1)),
        ];
        for (minutes, span) in spans {
            if minutes == 0 {
                continue;
            }
            if !self.inside_windows(span) {
                return Some(ReasonCode::Travel);
            }
            if self.blackout_overlap(span).is_some() {
                return Some(ReasonCode::Travel);
            }
            if self.block_overlap(span).is_some() {
                return Some(ReasonCode::Travel);
            }
        }
        None
    }
}

impl A1Case {
    /// The range the case is judged over: the search window widened by the
    /// largest travel before it and by the duration plus the largest travel
    /// after it, clamped to the supported instants.
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

    fn in_window(&self, start: Instant) -> bool {
        self.search.0 <= start && start < self.search.1
    }
}

fn resolved_event(case: &A1Case, db: &TimeZoneDatabase) -> Zone {
    db.zone(&case.event)
        .unwrap_or_else(|err| panic!("`{}` is in the bundled database: {err}", case.event))
}

fn resolved_named(db: &TimeZoneDatabase, name: &str) -> Zone {
    db.zone(name)
        .unwrap_or_else(|err| panic!("`{name}` is in the bundled database: {err}"))
}

fn applicable_policies(case: &A1Case) -> Vec<&StatusPolicyPlan> {
    case.rules
        .iter()
        .filter_map(|rule| match rule {
            RulePlan::StatusPolicy(plan) if rule.applies() => Some(plan),
            _ => None,
        })
        .collect()
}

fn resolve_windows(
    case: &A1Case,
    db: &TimeZoneDatabase,
    zones: &ResolvedZones,
    event: &Zone,
    judged: (i64, i64),
) -> Vec<ResolvedWindow> {
    case.rules
        .iter()
        .filter_map(|rule| match rule {
            RulePlan::Window(plan) if rule.applies() => Some(plan),
            _ => None,
        })
        .map(|plan| {
            let named = plan.zone.zone_name().unwrap_or(&case.event);
            let named_zone = resolved_named(db, named);
            let membership = WindowCase {
                zone: plan.zone.plan_zone(),
                zones: case.participant.zones.clone(),
                event: case.event.clone(),
                named: named.to_owned(),
                weekly: plan.weekly.clone(),
                dates: plan.dates.clone(),
                from: case.search.0,
                until: case.search.1,
            };
            let flags = MinuteFlags::over(judged.0, judged.1, |minute| {
                Instant::from_minutes(minute)
                    .is_ok_and(|instant| membership.contains(instant, zones, event, &named_zone))
            });
            ResolvedWindow { flags }
        })
        .collect()
}

fn resolve_blackouts<'a>(
    case: &'a A1Case,
    db: &TimeZoneDatabase,
    zones: &ResolvedZones,
    event: &Zone,
    judged: (i64, i64),
) -> Vec<ResolvedBlackout<'a>> {
    case.rules
        .iter()
        .filter_map(|rule| match rule {
            RulePlan::Blackout(plan) if rule.applies() => Some(plan),
            _ => None,
        })
        .map(|plan| {
            let named = plan.zone.zone_name().unwrap_or(&case.event);
            let named_zone = resolved_named(db, named);
            let membership = BlackoutCase {
                zone: plan.zone.plan_zone(),
                zones: case.participant.zones.clone(),
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
            let flags = MinuteFlags::over(judged.0, judged.1, |minute| {
                Instant::from_minutes(minute)
                    .is_ok_and(|instant| membership.contains(instant, zones, event, &named_zone))
            });
            ResolvedBlackout { plan, flags }
        })
        .collect()
}

fn resolve_blocks<'a>(
    participant: &'a ParticipantPlan,
    clearance: ClearancePlan,
    policies: &[&StatusPolicyPlan],
    exclude_event_keys: &[String],
) -> Vec<ResolvedBlock<'a>> {
    let excluded: std::collections::BTreeSet<&str> =
        exclude_event_keys.iter().map(String::as_str).collect();
    let at_start = i64::from(clearance.after.max(clearance.min_gap));
    let at_end = i64::from(clearance.before.max(clearance.min_gap));
    participant
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
                plan: block,
                occupied,
                widened: (occupied.0 - at_start, occupied.1 + at_end),
            }
        })
        .collect()
}

fn resolve_lead(case: &A1Case, event: &Zone) -> (i64, Vec<LeadBound>) {
    let mut lead_min = 0;
    let mut bounds = Vec::new();
    for plan in case.rules.iter().filter_map(|rule| match rule {
        RulePlan::LeadTime(plan) if rule.applies() => Some(plan),
        _ => None,
    }) {
        lead_min = lead_min.max(i64::from(plan.min));
        if let Some(max) = plan.max {
            bounds.push(resolve_bound(max, case.now, event));
        }
    }
    (lead_min, bounds)
}

/// Every cap that applies to the single participant, with the usage each holds.
///
/// An `A1Case` carries no occurrences, so a cap here measures the participant's
/// own blocks, all-day ones excepted: keyless blocks count one by one and keyed
/// ones merge into one event.
fn resolve_caps(
    case: &A1Case,
    db: &TimeZoneDatabase,
    event: &Zone,
    zones: &ResolvedZones,
) -> Vec<CapModel> {
    let blocks = case.participant.calendar.as_deref().unwrap_or(&[]);
    let events = counted_events(
        &[],
        &[(case.participant.id.as_str(), blocks)],
        &case.exclude_event_keys,
    );
    let id = case.participant.id.as_str();
    let mut caps = Vec::new();
    for rule in &case.rules {
        let RulePlan::Cap(plan) = rule else {
            continue;
        };
        if !rule.applies() || !plan.counts_any(&case.part_tags) {
            continue;
        }
        let fixed = match &plan.zone {
            RuleZonePlan::Participant => None,
            RuleZonePlan::Event => Some(event.clone()),
            RuleZonePlan::Named(name) => Some(resolved_named(db, name)),
        };
        let subject = (plan.per == opportune::model::CapPer::Each).then_some(id);
        let usage = cap_usage(
            plan,
            &events,
            subject,
            &[id],
            fixed.as_ref(),
            Some(zones),
            event,
        );
        caps.push(CapModel::new(plan.clone(), fixed, usage));
    }
    caps
}

fn resolve_grid(case: &A1Case, db: &TimeZoneDatabase, event: &Zone) -> (i64, i64, Zone) {
    match &case.grid {
        Some(grid) => {
            let zone = match &grid.zone {
                GridZonePlan::Event => event.clone(),
                GridZonePlan::Named(name) => resolved_named(db, name),
            };
            (i64::from(grid.step), i64::from(grid.anchor), zone)
        }
        // The engine's default: the duration capped at 30 minutes, anchored at
        // local midnight in the event zone.
        None => (i64::from(case.duration.clamp(1, 30)), 0, event.clone()),
    }
}

fn combined_clearance(case: &A1Case) -> ClearancePlan {
    let mut combined = ClearancePlan::default();
    for rule in &case.rules {
        let RulePlan::Clearance { clearance, .. } = rule else {
            continue;
        };
        if !rule.applies() {
            continue;
        }
        combined.before = combined.before.max(clearance.before);
        combined.after = combined.after.max(clearance.after);
        combined.min_gap = combined.min_gap.max(clearance.min_gap);
    }
    combined
}

/// Compares the engine's answer for `case` with the oracle's: every candidate
/// start the oracle admits must be offered, every start it excludes must be
/// excluded for the same reason, and `check_slot` must confirm every offer and
/// reject every probed exclusion the same way.
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
    case: &A1Case,
    db: &TimeZoneDatabase,
    engine: &Engine,
) -> Result<(), TestCaseError> {
    let request = case.to_request();
    let diagnostics = engine.validate(&request);
    prop_assert!(
        diagnostics.is_valid(),
        "the generator emitted an invalid request: {:?}",
        diagnostics.errors
    );
    let response = engine.find_availability(&request).unwrap();
    let oracle = A1Oracle::new(case, db);
    let mut offered = Vec::new();
    let mut excluded = Vec::new();
    for (start, decision) in oracle.decisions() {
        match decision {
            Decision::Offered => offered.push(start),
            Decision::Excluded(_) => excluded.push((start, decision)),
        }
    }

    prop_assert!(!response.truncated, "the case's offer limit truncated");
    let response_offers: Vec<Instant> = response
        .offers
        .iter()
        .map(|offer| offer.start().expect("an offer has an event"))
        .collect();
    prop_assert_eq!(&response_offers, &offered, "offered starts");

    let response_excluded: Vec<Instant> =
        response.excluded.iter().map(|entry| entry.start).collect();
    let oracle_excluded: Vec<Instant> = excluded.iter().map(|(start, _)| *start).collect();
    prop_assert_eq!(&response_excluded, &oracle_excluded, "excluded starts");
    for (entry, (start, decision)) in response.excluded.iter().zip(&excluded) {
        prop_assert_eq!(
            entry.reasons.len(),
            1,
            "one host yields one reason at {}",
            start
        );
        prop_assert_eq!(
            entry.reasons[0].code,
            decision_code(*decision).expect("an excluded start has a code"),
            "engine reason at {}",
            start
        );
    }

    // `excluded` never contains an offered start.
    for start in &response_excluded {
        prop_assert!(
            !response_offers.contains(start),
            "{} is both offered and excluded",
            start
        );
    }

    // `check_slot` agrees with the search for every offer and for sampled
    // excluded starts. `check_slot` ignores the grid and the search window, and
    // every candidate is inside the window, so a difference is a real split
    // between the two entry points.
    for offer in &response.offers {
        let start = offer.start().expect("an offer has an event");
        let mut check = proposal(case, start);
        check.events[0]
            .assignment
            .clone_from(&offer.events[0].assignment);
        let slot = engine.check_slot(&request, &check).unwrap();
        prop_assert!(
            slot.valid,
            "an offered start failed check_slot at {}",
            start
        );
        prop_assert!(slot.reasons.is_empty());
        prop_assert_eq!(slot.reserve, offer.events[0].reserve.clone());
        prop_assert_eq!(slot.flags, offer.flags.clone());
    }
    for (start, decision) in excluded.iter().take(5) {
        let slot = engine
            .check_slot(&request, &proposal(case, *start))
            .unwrap();
        prop_assert!(
            !slot.valid,
            "an excluded start passed check_slot at {}",
            start
        );
        prop_assert_eq!(
            slot.reasons.first().map(|reason| reason.code),
            decision_code(*decision),
            "check_slot's first reason at {}",
            start
        );
    }
    Ok(())
}

/// The reason code an oracle decision excludes for, or `None` when it offers.
fn decision_code(decision: Decision) -> Option<ReasonCode> {
    match decision {
        Decision::Offered => None,
        Decision::Excluded(code) => Some(code),
    }
}

/// The booking a single-participant case would propose at `start`.
fn proposal(case: &A1Case, start: Instant) -> Proposal {
    Proposal {
        events: vec![ProposalEvent {
            part: "main".to_owned(),
            start,
            assignment: vec![vec![case.participant.id.clone()]],
        }],
    }
}

#[cfg(test)]
mod tests {
    use super::{
        A1Case, A1Oracle, BlackoutPlan, BlockPlan, BlockStatus, ClearancePlan, Decision,
        LeadMaxPlan, LeadTimePlan, ParticipantPlan, RulePlan, RuleZonePlan, StatusEffect,
        StatusEntryPlan, StatusPolicyPlan, TravelPlan, WindowPlan, ZonePlan,
    };
    use opportune::reasons::ReasonCode;
    use opportune::time::{Date, Instant, LocalResolution, TimeOfDay, TimeZoneDatabase, Zone};

    fn instant(day: i8, hour: u8, minute: u8) -> Instant {
        let resolved = Zone::utc()
            .resolve_local(
                Date::new(2027, 6, day).expect("a fixture date"),
                TimeOfDay::new(hour, minute).expect("a fixture time"),
            )
            .expect("UTC has no transitions");
        match resolved {
            LocalResolution::Unique(instant) => instant,
            LocalResolution::Skipped | LocalResolution::Repeated(_, _) => {
                panic!("UTC has no transitions")
            }
        }
    }

    fn case() -> A1Case {
        A1Case {
            now: instant(14, 6, 0),
            search: (instant(14, 0, 0), instant(15, 0, 0)),
            event: "UTC".to_owned(),
            display: "UTC".to_owned(),
            duration: 30,
            grid: None,
            candidates: None,
            travel: TravelPlan::default(),
            part_tags: Vec::new(),
            participant: ParticipantPlan {
                id: "host".to_owned(),
                zones: ZonePlan {
                    home: "UTC".to_owned(),
                    periods: Vec::new(),
                },
                calendar: Some(Vec::new()),
            },
            rules: Vec::new(),
            exclude_event_keys: Vec::new(),
        }
    }

    fn decide(case: &A1Case, start: Instant) -> Decision {
        let db = TimeZoneDatabase::bundled().expect("the bundled database is present");
        A1Oracle::new(case, &db).decide(start)
    }

    fn window(id: &str, from: (u8, u8), until: (u8, u8)) -> RulePlan {
        RulePlan::Window(WindowPlan {
            id: id.to_owned(),
            zone: RuleZonePlan::Event,
            weekly: vec![(
                0b111_1111,
                vec![(
                    u16::from(from.0) * 60 + u16::from(from.1),
                    u16::from(until.0) * 60 + u16::from(until.1),
                )],
            )],
            dates: Vec::new(),
            applies_to_host: true,
            applies_to_part: true,
        })
    }

    fn blackout(id: &str, from: (u8, u8), until: (u8, u8)) -> RulePlan {
        RulePlan::Blackout(BlackoutPlan {
            id: id.to_owned(),
            zone: RuleZonePlan::Event,
            spans: vec![(
                (
                    Date::new(2027, 6, 14).unwrap(),
                    u16::from(from.0) * 60 + u16::from(from.1),
                ),
                (
                    Date::new(2027, 6, 14).unwrap(),
                    u16::from(until.0) * 60 + u16::from(until.1),
                ),
            )],
            applies_to_host: true,
            applies_to_part: true,
        })
    }

    fn block(from: (u8, u8), until: (u8, u8), status: BlockStatus, travel: u32) -> BlockPlan {
        BlockPlan {
            span: (instant(14, from.0, from.1), instant(14, until.0, until.1)),
            status,
            all_day: false,
            tags: Vec::new(),
            event_key: None,
            travel_before: travel,
            travel_after: 0,
        }
    }

    #[test]
    fn a_window_excludes_a_start_that_leaves_it() {
        let mut case = case();
        case.rules = vec![window("hours", (9, 0), (17, 0))];
        assert_eq!(
            decide(&case, instant(14, 8, 30)),
            Decision::Excluded(ReasonCode::OutsideWindow)
        );
        assert_eq!(decide(&case, instant(14, 9, 30)), Decision::Offered);
        assert_eq!(decide(&case, instant(14, 16, 30)), Decision::Offered);
        assert_eq!(
            decide(&case, instant(14, 17, 0)),
            Decision::Excluded(ReasonCode::OutsideWindow)
        );
    }

    #[test]
    fn a_blackout_excludes_an_overlap_and_loses_to_a_closed_window() {
        let mut case = case();
        case.rules = vec![
            window("open", (0, 0), (24, 0)),
            blackout("freeze", (10, 0), (11, 0)),
        ];
        assert_eq!(decide(&case, instant(14, 9, 30)), Decision::Offered);
        assert_eq!(
            decide(&case, instant(14, 10, 30)),
            Decision::Excluded(ReasonCode::Blackout)
        );

        // Both fail, and the contract's order reports the window first.
        case.rules = vec![
            window("morning", (9, 0), (12, 0)),
            blackout("afternoon", (13, 0), (14, 0)),
        ];
        assert_eq!(
            decide(&case, instant(14, 13, 30)),
            Decision::Excluded(ReasonCode::OutsideWindow)
        );
    }

    #[test]
    fn clearance_excludes_a_start_too_close_but_not_one_that_clears_it() {
        let mut case = case();
        case.rules = vec![RulePlan::Clearance {
            id: "gap".to_owned(),
            clearance: ClearancePlan {
                before: 15,
                after: 15,
                min_gap: 0,
                applies_to_host: true,
                applies_to_part: true,
            },
        }];
        case.participant.calendar = Some(vec![block((10, 0), (11, 0), BlockStatus::Busy, 0)]);
        assert_eq!(decide(&case, instant(14, 9, 0)), Decision::Offered);
        assert_eq!(
            decide(&case, instant(14, 9, 30)),
            Decision::Excluded(ReasonCode::Clearance)
        );
        assert_eq!(
            decide(&case, instant(14, 10, 30)),
            Decision::Excluded(ReasonCode::Blocked)
        );
    }

    #[test]
    fn lead_time_bounds_the_occupied_start_and_the_local_date() {
        let mut case = case();
        case.now = instant(14, 8, 0);
        case.rules = vec![RulePlan::LeadTime(LeadTimePlan {
            id: "notice".to_owned(),
            min: 120,
            max: None,
            applies_to_host: true,
            applies_to_part: true,
        })];
        assert_eq!(
            decide(&case, instant(14, 9, 30)),
            Decision::Excluded(ReasonCode::LeadTimeMin)
        );
        assert_eq!(decide(&case, instant(14, 10, 0)), Decision::Offered);

        case.rules = vec![RulePlan::LeadTime(LeadTimePlan {
            id: "horizon".to_owned(),
            min: 0,
            max: Some(LeadMaxPlan::Days(0)),
            applies_to_host: true,
            applies_to_part: true,
        })];
        assert_eq!(decide(&case, instant(14, 9, 0)), Decision::Offered);
        assert_eq!(
            decide(&case, instant(15, 9, 0)),
            Decision::Excluded(ReasonCode::LeadTimeMax)
        );
    }

    #[test]
    fn travel_is_judged_only_once_the_own_span_is_clear() {
        let mut case = case();
        case.participant.calendar = Some(vec![block((9, 0), (10, 0), BlockStatus::Busy, 0)]);
        case.travel = TravelPlan {
            before: 15,
            after: 0,
        };
        assert_eq!(
            decide(&case, instant(14, 9, 0)),
            Decision::Excluded(ReasonCode::Blocked)
        );
        assert_eq!(
            decide(&case, instant(14, 10, 0)),
            Decision::Excluded(ReasonCode::Travel)
        );
        assert_eq!(decide(&case, instant(14, 10, 30)), Decision::Offered);

        // Travel outside a window is a travel failure, not a window failure.
        case.participant.calendar = Some(Vec::new());
        case.rules = vec![window("hours", (9, 0), (17, 0))];
        case.travel = TravelPlan {
            before: 30,
            after: 0,
        };
        assert_eq!(
            decide(&case, instant(14, 9, 15)),
            Decision::Excluded(ReasonCode::Travel)
        );
    }

    #[test]
    fn a_status_policy_decides_a_block_and_its_travel_with_it() {
        let mut case = case();
        case.participant.calendar = Some(vec![block((10, 0), (11, 0), BlockStatus::Tentative, 30)]);
        assert_eq!(
            decide(&case, instant(14, 9, 45)),
            Decision::Excluded(ReasonCode::Blocked)
        );

        case.rules = vec![RulePlan::StatusPolicy(StatusPolicyPlan {
            id: "policy".to_owned(),
            entries: vec![StatusEntryPlan {
                status: Some(BlockStatus::Tentative),
                tag: None,
                effect: StatusEffect::Ignore,
            }],
            applies_to_host: true,
            applies_to_part: true,
        })];
        assert_eq!(decide(&case, instant(14, 9, 45)), Decision::Offered);
        assert_eq!(decide(&case, instant(14, 10, 30)), Decision::Offered);
    }
}
