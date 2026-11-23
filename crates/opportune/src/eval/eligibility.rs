//! Eligibility and reasons: which participants can take a start, and why the
//! others cannot.
//!
//! One function decides whether a participant can take a start, and both the
//! search's answer and the reason it reports read the same checks, so an offer
//! and a `check_slot` cannot disagree. The eligibility pass itself answers from
//! the precomputed `usable` and `cap_blocked` sets, and the allocating reason path
//! runs only for a participant that failed.

use std::cmp::Ordering;

use crate::intervals::Span as MinuteSpan;
use crate::matching;
use crate::reasons::{Reason, ReasonCode, ReasonParams};
use crate::time::{Date, Instant};

use super::ParticipantSettings;
use super::Settings;
use super::allowed::representable;
use super::blocked::{BlockedSpan, blackout_overlapping, block_overlapping, overlaps};

/// How many reasons a caller needs for one participant at one start.
///
/// An excluded start reports the first failing reason; `check_slot` reports
/// every one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Wanted {
    /// The first failing reason, which is what one excluded start reports.
    First,
    /// Every failing reason, in precedence order, which is what `check_slot`
    /// reports.
    All,
}

impl Wanted {
    /// Whether a failure is enough to stop judging.
    const fn stops(self) -> bool {
        !matches!(self, Self::All)
    }
}

/// Whether `participant` cannot book a start at `start`, collecting that
/// participant's reasons in the canonical precedence order of
/// [`ReasonCode::ALL`].
///
/// `out` is empty on entry: `First` leaves at the first failure it finds, and
/// `All` judges every check and answers from what it collected.
///
/// `own` is the booking's own span, which every participant shares and the
/// caller builds once per start.
///
/// The booking's own span decides `outside_window`, `blackout` and `blocked`.
/// `travel` is judged only once that span is clear, on the travel spans either
/// side of it. `clearance` is judged on the whole occupied span, and only when
/// the conflict is the gap rather than an actual overlap, which `blocked` or
/// `travel` already explains.
pub(crate) fn failing_reasons(
    participant: &ParticipantSettings<'_>,
    start: Instant,
    own: &MinuteSpan,
    wanted: Wanted,
    out: &mut Vec<Reason>,
) -> bool {
    let id = participant.participant.id.as_str();

    let occupied_start = own.start().saturating_sub(participant.travel.0);
    if occupied_start < participant.lead_min_at {
        out.push(cite(
            Reason::new(id, ReasonCode::LeadTimeMin),
            participant.rules.lead_min_rule.map(|(rule, _)| rule),
        ));
        if wanted.stops() {
            return true;
        }
    }
    if let Some((rule, bound)) = &participant.lead_bound
        && !bound.allows(start)
    {
        out.push(Reason::new(id, ReasonCode::LeadTimeMax).with_rule(*rule));
        if wanted.stops() {
            return true;
        }
    }

    let own_allowed = participant.inside_windows(own);
    if !own_allowed {
        out.push(cite(
            Reason::new(id, ReasonCode::OutsideWindow),
            participant.excluding_window(own),
        ));
        if wanted.stops() {
            return true;
        }
    }
    let own_blackout = blackout_overlapping(&participant.blackouts, own);
    if let Some(rule) = own_blackout {
        out.push(Reason::new(id, ReasonCode::Blackout).with_rule(rule));
        if wanted.stops() {
            return true;
        }
    }
    let own_block = block_overlapping(&participant.blocked, own);
    if let Some(blocked) = own_block {
        out.push(Reason::new(id, ReasonCode::Blocked).with_block(blocked.block));
        if wanted.stops() {
            return true;
        }
    }

    if own_allowed
        && own_blackout.is_none()
        && own_block.is_none()
        && let Some(cause) = travel_conflict(participant, start, own.end())
    {
        out.push(cause.reason(id));
        if wanted.stops() {
            return true;
        }
    }

    let Some(occupied) = occupied_span(participant, own) else {
        return !out.is_empty();
    };
    if let Some((blocked, rule)) = clearance_conflict(participant, &occupied) {
        out.push(cite(
            Reason::new(id, ReasonCode::Clearance).with_block(blocked.block),
            rule,
        ));
        if wanted.stops() {
            return true;
        }
    }
    if let Some(rule) = crate::rest::rest_conflict(
        &participant.rests,
        &participant.schedule,
        &participant.date_jumps,
        &occupied,
    ) {
        out.push(Reason::new(id, ReasonCode::Rest).with_rule(rule));
        if wanted.stops() {
            return true;
        }
    }
    if let Some(conflict) = cap_conflict(participant, start) {
        out.push(conflict.reason(id));
        return true;
    }
    !out.is_empty()
}

/// The earliest applicable `cap` the booking's start would take past its limit.
///
/// Caps are judged after rest, so a start excluded by both reports rest: the
/// canonical precedence decides which reason a caller sees first.
fn cap_conflict<'a>(
    participant: &'a ParticipantSettings<'_>,
    start: Instant,
) -> Option<CapConflict<'a>> {
    participant.caps.iter().find_map(|cap| {
        cap.exceeded(&participant.schedule, start)
            .map(|(used, bucket)| CapConflict {
                rule: cap.rule,
                used,
                limit: cap.limit,
                bucket,
            })
    })
}

/// A cap the booking would take past its limit, before the reason is built.
struct CapConflict<'a> {
    /// The rule that supplied the exclusion.
    rule: &'a str,
    /// How much the bucket already holds.
    used: u64,
    /// The cap's limit.
    limit: u32,
    /// The bucket's label.
    bucket: Date,
}

impl CapConflict<'_> {
    /// The exclusion reason, naming the participant it applies to.
    fn reason(&self, id: &str) -> Reason {
        Reason::new(id, ReasonCode::CapReached)
            .with_rule(self.rule)
            .with_params(ReasonParams::CapReached {
                used: self.used,
                limit: self.limit,
                bucket: self.bucket.to_string(),
            })
    }
}

/// The travel cause for a booking whose own span is clear, if either travel span
/// is not: the earliest cause in the same order the own span is judged.
fn travel_conflict<'a>(
    participant: &'a ParticipantSettings<'_>,
    start: Instant,
    own_end: i64,
) -> Option<TravelConflict<'a>> {
    let start_minutes = start.minutes();
    let (before, after) = participant.travel;
    let spans = [
        (before, start_minutes.saturating_sub(before), start_minutes),
        (after, own_end, own_end.saturating_add(after)),
    ];
    for (minutes, from, to) in spans {
        if minutes == 0 {
            continue;
        }
        let Ok(span) = MinuteSpan::new(from, to) else {
            continue;
        };
        if !participant.inside_windows(&span) {
            return Some(TravelConflict::OutsideWindow(
                participant.excluding_window(&span),
            ));
        }
        if let Some(rule) = blackout_overlapping(&participant.blackouts, &span) {
            return Some(TravelConflict::Blackout(rule));
        }
        if let Some(blocked) = block_overlapping(&participant.blocked, &span) {
            return Some(TravelConflict::Blocked(blocked));
        }
    }
    None
}

/// Why travel in one direction is not clear, before the reason is built.
enum TravelConflict<'a> {
    /// The travel falls outside a window, citing the window when one applies.
    OutsideWindow(Option<&'a str>),
    /// The travel overlaps a blackout.
    Blackout(&'a str),
    /// The travel overlaps an event on the participant's calendar.
    Blocked(&'a BlockedSpan<'a>),
}

impl TravelConflict<'_> {
    /// The exclusion reason, naming the participant it applies to.
    fn reason(&self, id: &str) -> Reason {
        let reason = Reason::new(id, ReasonCode::Travel);
        match self {
            Self::OutsideWindow(rule) => cite(reason, *rule),
            Self::Blackout(rule) => reason.with_rule(*rule),
            Self::Blocked(blocked) => reason.with_block(blocked.block),
        }
    }
}

/// The blocking event and clearance rule to cite for a booking judged too close to
/// an event: the widest conflicting evidence is one event, but several can be in
/// the way, and the reason must cite the earliest rule in request order among the
/// ones that excluded the booking rather than the first event on the calendar.
///
/// A conflict is an event whose clearance-widened span the occupied span crosses
/// while its own span stays clear: the evidence that the booking is too close
/// rather than on top of it, which `blocked` already explains.
fn clearance_conflict<'a, 'b>(
    participant: &'b ParticipantSettings<'a>,
    occupied: &MinuteSpan,
) -> Option<(&'b BlockedSpan<'a>, Option<&'a str>)> {
    let mut best: Option<(usize, &'b BlockedSpan<'a>, Option<&'a str>)> = None;
    for blocked in participant.blocked.widened_candidates(occupied) {
        if !overlaps(&blocked.widened, occupied) || overlaps(&blocked.occupied, occupied) {
            continue;
        }
        let supplier = clearance_rule(participant, blocked, occupied);
        let position = supplier.map_or(usize::MAX, |(_, position)| position);
        // `blocked` is in canonical order, so the first event a rule excludes
        // keeps the citation when several rules are equally early.
        match best {
            Some((seen, _, _)) if seen <= position => {}
            _ => best = Some((position, blocked, supplier.map(|(rule, _)| rule))),
        }
    }
    best.map(|(_, blocked, rule)| (blocked, rule))
}

/// The clearance rule that supplied the gap the booking crossed: the larger of the
/// booking's own gap and the least gap, on the side of the event it met, and the
/// earliest in request order when both asked for the same. The rule's position is
/// returned with it so competing conflicts can be ordered by request order.
fn clearance_rule<'a>(
    participant: &ParticipantSettings<'a>,
    blocked: &BlockedSpan<'a>,
    occupied: &MinuteSpan,
) -> crate::rules::Supplier<'a> {
    let rules = participant.rules.clearance_rules;
    let clearance = participant.rules.clearance;
    let (gap, gap_rule) = if occupied.end() <= blocked.occupied.start() {
        // The booking finishes before the event, so the gap it needs after
        // itself is the one it crossed.
        (clearance.after, rules.after)
    } else {
        (clearance.before, rules.before)
    };
    match gap.cmp(&clearance.min_gap) {
        Ordering::Greater => gap_rule,
        Ordering::Less => rules.min_gap,
        // Both asked for the same gap: the earliest rule in request order
        // supplied the value the reason cites.
        Ordering::Equal => [gap_rule, rules.min_gap]
            .into_iter()
            .flatten()
            .min_by_key(|(_, position)| *position),
    }
}

/// The reason with the rule that produced it, or without one when no explicit
/// rule applies.
fn cite(reason: Reason, rule: Option<&str>) -> Reason {
    match rule {
        Some(rule) => reason.with_rule(rule),
        None => reason,
    }
}

/// Whether `participant` can take `start` without building any reason, using
/// the precomputed `usable` and `cap_blocked` interval sets.
pub(crate) fn is_participant_eligible(
    participant: &ParticipantSettings<'_>,
    start: Instant,
    own: &MinuteSpan,
) -> bool {
    let occupied_start = own.start().saturating_sub(participant.travel.0);
    if occupied_start < participant.lead_min_at {
        return false;
    }
    if let Some((_, bound)) = &participant.lead_bound
        && !bound.allows(start)
    {
        return false;
    }
    let Some(occupied) = occupied_span(participant, own) else {
        return true;
    };
    if !participant.usable.contains_span(&occupied) {
        return false;
    }
    if participant.cap_always_blocked || participant.cap_blocked.contains_minute(start.minutes()) {
        return false;
    }
    if occupied.end() - occupied.start() == participant.occupied_len {
        if participant
            .rest_self_blocked
            .contains_minute(occupied.start())
        {
            return false;
        }
    } else {
        for rest in &participant.rests {
            if rest.late.overlaps(&occupied)
                && rest.early.overlaps(&occupied)
                && crate::rest::rest_self_conflict(
                    rest,
                    &participant.schedule,
                    &participant.date_jumps,
                    &occupied,
                )
            {
                return false;
            }
        }
    }
    true
}

/// The minutes a booking occupies for `participant`: its own span widened by the
/// travel either side, narrowed to the minutes the model can represent.
///
/// `None` when none of those minutes is representable, which no booking inside the
/// supported range can produce, because its own span always holds at least one.
fn occupied_span(participant: &ParticipantSettings<'_>, own: &MinuteSpan) -> Option<MinuteSpan> {
    let from = own.start().saturating_sub(participant.travel.0);
    let until = own.end().saturating_add(participant.travel.1);
    representable(MinuteSpan::new(from, until).ok()?)
}

/// Whether each participant can take `start`, in `eligible` indexed like
/// `Settings::participants`, with the ones that cannot in `failed` in the same
/// order.
///
/// Every participant an offer depends on is judged once per start, because one
/// participant may fill several terms and an attendee gates the whole offer. The
/// answers are indexed like `Settings::participants` so a term's eligible list is
/// a filter rather than a second walk over the request. Only the answer is built
/// here: most starts of a real request are offered, and an offer needs nobody's
/// reason, so the allocating reason path runs only for the participants `failed`
/// names, and only when the start is excluded and the request asked to know why.
pub(crate) fn start_eligibility(
    settings: &Settings<'_>,
    start: Instant,
    own: &MinuteSpan,
    eligible: &mut Vec<bool>,
    failed: &mut Vec<usize>,
) {
    eligible.clear();
    eligible.resize(settings.participants.len(), false);
    failed.clear();
    for &index in &settings.reported {
        let participant = &settings.participants[index];
        if is_participant_eligible(participant, start, own) {
            eligible[index] = true;
        } else {
            failed.push(index);
        }
    }
}

/// The first failing reason of each participant `start_eligibility` recorded as
/// ineligible, in the order it recorded them — which is ascending participant ID
/// order, the order an exclusion reports.
pub(crate) fn exclusion_reasons(
    settings: &Settings<'_>,
    start: Instant,
    own: &MinuteSpan,
    failed: &[usize],
) -> Vec<Reason> {
    let mut reasons = Vec::new();
    let mut scratch = Vec::new();
    for &index in failed {
        scratch.clear();
        let still_fails = failing_reasons(
            &settings.participants[index],
            start,
            own,
            Wanted::First,
            &mut scratch,
        );
        // Both passes judge equivalent predicates, so a participant the search
        // found ineligible always has a reason to report. Should they ever
        // disagree, an answer and its explanation would contradict each other:
        // fail loudly here rather than publish an exclusion with nothing to say.
        debug_assert!(still_fails, "an ineligible participant reports no reason");
        reasons.append(&mut scratch);
    }
    reasons
}

/// Whether some term holds fewer eligible hosts than it needs.
pub(crate) fn any_term_short(settings: &Settings<'_>, eligible: &[bool]) -> bool {
    settings.terms.iter().any(|term| {
        term.hosts
            .iter()
            .filter(|index| eligible[**index])
            .take(term.count)
            .count()
            < term.count
    })
}

/// Each term's eligible hosts at one start, ascending by ID.
pub(crate) fn eligible_per_term(settings: &Settings<'_>, eligible: &[bool]) -> Vec<Vec<String>> {
    settings
        .terms
        .iter()
        .map(|term| {
            term.hosts
                .iter()
                .filter(|index| eligible[**index])
                .map(|index| settings.participants[*index].participant.id.clone())
                .collect()
        })
        .collect()
}

/// The matching outcome for the last eligible-host shape a search asked about.
///
/// Consecutive candidate starts usually leave the same hosts eligible — nobody's
/// calendar changes between them — so every start of such a run would otherwise
/// re-solve the one assignment its shape describes. One shape is retained: the
/// previous one. A start whose eligible hosts differ, as an adversarial calendar
/// can arrange for every start, costs the solve it always did and retains no more
/// memory than the shape before it, so peak memory is one shape whatever the
/// candidate count.
///
/// Only a requirement whose pools overlap is solved as a joint assignment, so only
/// one has an answer worth retaining: when no participant fills two terms both
/// questions are read straight off the pre-ranked lists and never reach this cache.
///
/// A cache answers only the request that built it: the requirement, the weights it
/// ranks hosts by and the cap on how many hosts can be used are fixed for the
/// whole response, so the shape is the one thing that varies between its queries.
#[derive(Default)]
pub(crate) struct MatchingCache {
    /// The shape the answer belongs to, and the answers computed for it.
    shape: Shape,
    /// Whether `shape` describes a start already asked about, so an empty shape
    /// is told apart from the state before the first query.
    known: bool,
}

/// One eligible-host shape and the answers computed for it.
#[derive(Default)]
struct Shape {
    /// Each term's eligible hosts in the matcher's rank order, cut to `slots`:
    /// the only hosts any optimal assignment can use.
    hosts: Vec<Vec<usize>>,
    /// The requirement this shape answers: the total hosts it asks for and the
    /// per-term counts. Part of the key, so a cache can never answer a
    /// requirement whose terms it did not resolve.
    slots: usize,
    /// Each term's demand, in request order.
    counts: Vec<usize>,
    /// What has been answered about this shape.
    answer: ShapeAnswer,
}

/// What is known about one eligible-host shape.
///
/// The variants make "not fillable but here is an assignment" unrepresentable:
/// an assignment is only ever stored when one was built for this shape, and a
/// shape whose fillability is known without one has no assignment at all.
#[derive(Default)]
enum ShapeAnswer {
    /// The shape is loaded but nothing has been asked about it yet.
    #[default]
    Unasked,
    /// No assignment fills every term.
    Unfillable,
    /// Some assignment fills every term, but none has been built.
    Fillable,
    /// The assignment that fills every term.
    Assigned(Vec<Vec<String>>),
}

impl ShapeAnswer {
    /// Whether the shape can be filled, if that has been decided.
    const fn fillable(&self) -> Option<bool> {
        match self {
            Self::Unasked => None,
            Self::Unfillable => Some(false),
            Self::Fillable | Self::Assigned(_) => Some(true),
        }
    }
}

impl MatchingCache {
    /// Whether `eligible` matches the cached shape without allocating.
    fn matches_shape(&self, settings: &Settings<'_>, eligible: &[bool]) -> bool {
        self.known
            && self.shape.slots == settings.slots
            && self.shape.hosts.len() == settings.terms.len()
            && self.shape.counts.len() == settings.terms.len()
            && settings
                .terms
                .iter()
                .zip(&self.shape.counts)
                .all(|(term, count)| term.count == *count)
            && settings
                .terms
                .iter()
                .zip(&self.shape.hosts)
                .all(|(term, cached)| {
                    term.hosts_by_weight
                        .iter()
                        .copied()
                        .filter(|&index| eligible[index])
                        .take(settings.slots)
                        .eq(cached.iter().copied())
                })
    }

    /// Points the cache at the shape `eligible` describes, discarding the previous
    /// answer when it differs.
    fn load(&mut self, settings: &Settings<'_>, eligible: &[bool]) -> &mut Shape {
        debug_assert_eq!(
            eligible.len(),
            settings.participants.len(),
            "eligibility is judged for every participant the settings carry"
        );
        if !self.matches_shape(settings, eligible) {
            self.shape.hosts.resize_with(settings.terms.len(), Vec::new);
            for (term, slot) in settings.terms.iter().zip(&mut self.shape.hosts) {
                slot.clear();
                slot.extend(
                    term.hosts_by_weight
                        .iter()
                        .copied()
                        .filter(|&index| eligible[index])
                        .take(settings.slots),
                );
            }
            self.shape.slots = settings.slots;
            self.shape.counts.clear();
            self.shape
                .counts
                .extend(settings.terms.iter().map(|term| term.count));
            self.shape.answer = ShapeAnswer::Unasked;
            self.known = true;
        }
        &mut self.shape
    }

    /// The highest-weight assignment of distinct eligible hosts to the
    /// requirement's terms at one start, using term-ordered lexicographic
    /// tie-breaking, or `None` when no assignment fills every term.
    ///
    /// When no participant fills two terms, each term's choice constrains no other
    /// and the answer is each term's heaviest eligible hosts, so it is read
    /// straight off the pre-ranked lists instead of being solved as a joint
    /// assignment — and there is no solve to reuse, so that read is not keyed.
    pub(crate) fn assignment_at(
        &mut self,
        settings: &Settings<'_>,
        eligible: &[bool],
    ) -> Option<Vec<Vec<String>>> {
        if settings.terms_disjoint {
            return disjoint_assignment(settings, eligible);
        }
        let shape = self.load(settings, eligible);
        match &shape.answer {
            ShapeAnswer::Assigned(assignment) => return Some(assignment.clone()),
            ShapeAnswer::Unfillable => return None,
            ShapeAnswer::Unasked | ShapeAnswer::Fillable => {}
        }
        let candidates = candidates_of(settings, &shape.hosts);
        let terms = terms(settings, &candidates);
        let assignment = matching::assign(&terms).map(|assignment| assignment.terms);
        shape.answer = match &assignment {
            Some(assignment) => ShapeAnswer::Assigned(assignment.clone()),
            None => ShapeAnswer::Unfillable,
        };
        assignment
    }

    /// Whether the requirement can be filled at one start when the caller no
    /// longer needs the concrete assignment (past `request.limit()`).
    ///
    /// A disjoint requirement answers this with the same scan the caller already
    /// made, so it is not keyed either: building the shape to key it on would cost
    /// an allocation per start to save that scan.
    pub(crate) fn can_fill(&mut self, settings: &Settings<'_>, eligible: &[bool]) -> bool {
        if settings.terms_disjoint {
            return !any_term_short(settings, eligible);
        }
        let shape = self.load(settings, eligible);
        if let Some(fillable) = shape.answer.fillable() {
            return fillable;
        }
        let candidates = candidates_of(settings, &shape.hosts);
        let terms = terms(settings, &candidates);
        let fillable = matching::jointly_fillable(&terms);
        shape.answer = if fillable {
            ShapeAnswer::Fillable
        } else {
            ShapeAnswer::Unfillable
        };
        fillable
    }
}

/// Each term's choices when no participant fills two terms: its heaviest eligible
/// hosts, reported in ascending participant ID order.
fn disjoint_assignment(settings: &Settings<'_>, eligible: &[bool]) -> Option<Vec<Vec<String>>> {
    settings
        .terms
        .iter()
        .map(|term| {
            let mut chosen: Vec<usize> = term
                .hosts_by_weight
                .iter()
                .copied()
                .filter(|index| eligible[*index])
                .take(term.count)
                .collect();
            if chosen.len() < term.count {
                return None;
            }
            // Output within a term is in ascending participant ID order.
            chosen.sort_by(|left, right| {
                settings.participants[*left]
                    .participant
                    .id
                    .cmp(&settings.participants[*right].participant.id)
            });
            Some(
                chosen
                    .into_iter()
                    .map(|index| settings.participants[index].participant.id.clone())
                    .collect(),
            )
        })
        .collect()
}

/// Each term's eligible hosts as matching candidates, in the order the matcher
/// ranks them.
fn candidates_of<'a>(
    settings: &'a Settings<'_>,
    hosts: &'a [Vec<usize>],
) -> Vec<Vec<matching::Candidate<'a>>> {
    hosts
        .iter()
        .map(|term| {
            term.iter()
                .map(|index| matching::Candidate {
                    id: settings.participants[*index].participant.id.as_str(),
                    weight: settings.weights[*index],
                })
                .collect()
        })
        .collect()
}

/// The matching terms over borrowed candidate lists.
fn terms<'a>(
    settings: &Settings<'_>,
    candidates: &'a [Vec<matching::Candidate<'a>>],
) -> Vec<matching::Term<'a>> {
    settings
        .terms
        .iter()
        .zip(candidates)
        .map(|(term, candidates)| matching::Term {
            count: term.count,
            candidates: candidates.as_slice(),
        })
        .collect()
}

/// Every mode answers the same question. `First` leaves at the first failure, but
/// `All` judges every check and so cannot return there: it has to answer from
/// what it collected. A booking in the past is excluded by `lead_time_min`, which
/// the last check — the cap — does not repeat, so an answer read off that check
/// alone would call an excluded participant eligible. `check_slot` reads the
/// reasons rather than the answer, so nothing else in the crate would catch it.
#[cfg(all(test, feature = "bundled-tzdb"))]
mod tests {
    use super::{Wanted, failing_reasons, is_participant_eligible};
    use crate::Engine;
    use crate::eval::slot_settings;
    use crate::intervals::Span as MinuteSpan;
    use crate::model::{Goal, Part, Participant, Request, Requirement, Span, Term};
    use crate::reasons::ReasonCode;
    use crate::tags::{Role, Selector};
    use crate::time::{Duration, Instant, ZoneName};

    #[test]
    fn every_wanted_mode_answers_whether_the_participant_can_book() {
        let engine = Engine::builder().build().unwrap();
        let zone = ZoneName::new("UTC").unwrap();
        let at = |count: i64| Instant::from_minutes(Instant::MIN.minutes() + count).unwrap();
        let start = at(10 * 60);
        let mut participant = Participant::new("rich", zone.clone());
        participant.calendar = Some(Vec::new());
        let mut request = Request::new(
            // `now` is an hour after the proposed start, so the booking is late
            // rather than blocked: one failure, and no cap to repeat it.
            at(11 * 60),
            Goal::new(
                zone,
                Span::new(at(0), at(24 * 60)).unwrap(),
                vec![Part::new(
                    Duration::from_minutes(30),
                    Requirement::new(vec![Term::at_least(1, Selector::role(Role::Host))]),
                )],
            ),
        );
        request.participants = vec![participant];

        let settings = slot_settings(&engine, &request, start).unwrap();
        let own = MinuteSpan::new(start.minutes(), start.minutes() + 30).unwrap();
        assert!(!is_participant_eligible(
            &settings.participants[0],
            start,
            &own
        ));
        for wanted in [Wanted::First, Wanted::All] {
            let mut reasons = Vec::new();
            let failed =
                failing_reasons(&settings.participants[0], start, &own, wanted, &mut reasons);
            assert!(
                failed,
                "{wanted:?}: a start an hour in the past is excluded"
            );
            assert_eq!(reasons.len(), 1);
            assert_eq!(reasons[0].code, ReasonCode::LeadTimeMin);
        }
    }
}
