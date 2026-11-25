/* Generated from opportune JSON Schema — do not edit by hand; run make codegen-ts. */

/**
 * A two-element array of strings: a span's start and end, or a blackout's
 * local date-times.
 *
 * The JSON spells both as an array of two strings, so the wire reads both into
 * one shape and leaves the meaning to the field that holds it. The array is
 * read by hand rather than derived so that an array of the wrong length is one
 * `invalid_length` failure naming the shape a caller can write, rather than
 * serde's two spellings of the same mistake — one of which reports a
 * three-element array as a JSON syntax error.
 *
 * @minItems 2
 * @maxItems 2
 *
 * This interface was referenced by `Schema`'s JSON-Schema
 * via the `definition` "Span".
 */
export type Span = [string, string];
/**
 * This interface was referenced by `Schema`'s JSON-Schema
 * via the `definition` "Selector".
 */
export type Selector =
  MatchAll | TagSelector | AnySelector | AllSelector | NotSelector | IdsSelector | RoleSelector | KindSelector;
/**
 * This interface was referenced by `Schema`'s JSON-Schema
 * via the `definition` "MatchAll".
 */
export type MatchAll = true;
/**
 * A participant's role in a request.
 *
 * This interface was referenced by `Schema`'s JSON-Schema
 * via the `definition` "Role".
 */
export type Role = "host" | "attendee";
/**
 * Whether a participant is a person or a resource.
 *
 * This interface was referenced by `Schema`'s JSON-Schema
 * via the `definition` "ParticipantKind".
 */
export type ParticipantKind = "person" | "resource";
/**
 * What one participant's calendar converted into.
 *
 * This interface was referenced by `Schema`'s JSON-Schema
 * via the `definition` "CalendarOutcome".
 */
export type CalendarOutcome = AvailableCalendarOutcome | UnavailableCalendarOutcome;
/**
 * What a `cap` measures.
 *
 * This interface was referenced by `Schema`'s JSON-Schema
 * via the `definition` "CapMeasure".
 */
export type CapMeasure = "count" | "minutes";
/**
 * Whether a `cap` limits each participant or the pool together.
 *
 * This interface was referenced by `Schema`'s JSON-Schema
 * via the `definition` "CapPer".
 */
export type CapPer = "each" | "pooled";
/**
 * The civil period a `cap` bucket covers.
 *
 * This interface was referenced by `Schema`'s JSON-Schema
 * via the `definition` "CapPeriod".
 */
export type CapPeriod = "day" | "week" | "month";
/**
 * One clause of a requirement.
 *
 * This interface was referenced by `Schema`'s JSON-Schema
 * via the `definition` "Term".
 */
export type Term = PersonTerm | AtLeastTerm;
/**
 * One mailbox's working hours, as a rule or as the reason it could not become
 * one.
 *
 * Working hours are Graph-only, which is why the generated model is named
 * `GraphWorkingHoursOutcome`. The failure is a value rather than an exception so
 * a caller can fall back to its own window without catching one.
 *
 * This interface was referenced by `Schema`'s JSON-Schema
 * via the `definition` "GraphWorkingHoursOutcome".
 */
export type GraphWorkingHoursOutcome = AvailableWorkingHoursOutcome | UnavailableWorkingHoursOutcome;
/**
 * A `lead_time` maximum: how far ahead a start may be.
 *
 * This interface was referenced by `Schema`'s JSON-Schema
 * via the `definition` "LeadTimeMax".
 */
export type LeadTimeMax = DurationMax | DaysMax | WorkingDaysMax | DateMax;
/**
 * One rule: an ID, which participants it constrains, which bookings it applies
 * to, and the kind-specific parameters.
 *
 * This interface was referenced by `Schema`'s JSON-Schema
 * via the `definition` "Rule".
 */
export type Rule = WindowRule | BlackoutRule | ClearanceRule | LeadTimeRule | StatusPolicyRule | RestRule | CapRule;
/**
 * What a matching status-policy entry does to a block.
 *
 * This interface was referenced by `Schema`'s JSON-Schema
 * via the `definition` "StatusEffect".
 */
export type StatusEffect = "ignore" | "block";
/**
 * The statuses a status-policy entry may name.
 *
 * A block's status is read as an open string, because a provider may report a
 * value this layer has never seen and `busy` is the safe reading of it. An entry
 * is the caller's own configuration, so a value outside this set is rejected
 * where it is written rather than silently matched against something else.
 *
 * This interface was referenced by `Schema`'s JSON-Schema
 * via the `definition` "Status".
 */
export type Status = "free" | "tentative" | "busy";

/**
 * One per-event finding as JSON.
 *
 * This interface was referenced by `Schema`'s JSON-Schema
 * via the `definition` "AdapterDiagnostic".
 */
export interface AdapterDiagnostic {
  /**
   * The stable, machine-readable code.
   */
  code: string;
  /**
   * The provider's ID for the event the finding is about, when there is one.
   */
  event_id?: string | null;
  /**
   * A sentence naming the value and why it was left out.
   */
  message: string;
}
/**
 * One converted block as JSON, with the provider's event ID beside it.
 *
 * This interface was referenced by `Schema`'s JSON-Schema
 * via the `definition` "AnnotatedBlock".
 */
export interface AnnotatedBlock {
  /**
   * The block, in the wire layer's own shape.
   */
  block: Block;
  /**
   * The provider's ID for the event, or `null` when the source reports spans
   * rather than events.
   */
  event_id?: string | null;
}
/**
 * One span on a participant's calendar.
 *
 * This interface was referenced by `Schema`'s JSON-Schema
 * via the `definition` "Block".
 */
export interface Block {
  /**
   * Whether the source marked the block all day.
   */
  all_day?: boolean | null;
  /**
   * The caller's ID for the event this block is one instance of.
   */
  event_key?: string | null;
  /**
   * The block's start and end instants, which round outward.
   */
  span: Span;
  /**
   * How the block affects availability. A provider may report a status this
   * crate does not know, and an unrecognised value reads as `busy`; absent
   * means `busy`.
   */
  status?: string | null;
  /**
   * Everything the block is tagged with.
   */
  tags?: string[] | null;
  /**
   * Travel the caller computed to leave the block.
   */
  travel_after?: string | null;
  /**
   * Travel the caller computed to reach the block.
   */
  travel_before?: string | null;
}
/**
 * A term needing a count of hosts among the participants a selector matches.
 *
 * This interface was referenced by `Schema`'s JSON-Schema
 * via the `definition` "AtLeastTerm".
 */
export interface AtLeastTerm {
  /**
   * How many hosts the term needs.
   */
  at_least: number;
  /**
   * Which participants can fill it.
   */
  of: Selector;
}
export interface TagSelector {
  tag: string;
}
export interface AnySelector {
  any: Selector[];
}
export interface AllSelector {
  all: Selector[];
}
export interface NotSelector {
  not: Selector;
}
export interface IdsSelector {
  ids: string[];
}
export interface RoleSelector {
  role: Role;
}
export interface KindSelector {
  kind: ParticipantKind;
}
/**
 * The parameters of a `blackout` rule.
 *
 * This interface was referenced by `Schema`'s JSON-Schema
 * via the `definition` "BlackoutParams".
 */
export interface BlackoutParams {
  /**
   * Local date-time spans inside which nothing may be booked.
   */
  spans: Span[];
  /**
   * `participant`, `event`, or an IANA zone name.
   */
  zone: string;
}
/**
 * The calendar converted.
 */
export interface AvailableCalendarOutcome {
  /**
   * The converted blocks, in provider order.
   */
  blocks: AnnotatedBlock[];
  /**
   * Per-event findings that did not stop the conversion.
   */
  diagnostics: AdapterDiagnostic[];
  status: "available";
}
/**
 * The calendar could not be converted, so availability is unknown rather
 * than answered with no blocks.
 */
export interface UnavailableCalendarOutcome {
  /**
   * A stable, machine-readable code.
   */
  code: string;
  /**
   * A sentence naming what could not be converted and why.
   */
  message: string;
  status: "unavailable";
}
/**
 * A named working calendar.
 *
 * This interface was referenced by `Schema`'s JSON-Schema
 * via the `definition` "CalendarSpec".
 */
export interface CalendarSpec {
  /**
   * Dates that are not worked whatever the weekday.
   */
  holidays?: string[] | null;
  /**
   * The weekdays the calendar works, written as day keys such as `mon-fri` or
   * `mon,wed`.
   *
   * A count of working days needs a calendar that works at least one weekday:
   * an absent list is the day-list syntax's spelling of "no working day",
   * which the engine rejects with `working_days_empty`.
   */
  working_days?: string | null;
  /**
   * The zone the calendar's civil dates are read in.
   */
  zone: string;
}
/**
 * The parameters of a `cap` rule.
 *
 * This interface was referenced by `Schema`'s JSON-Schema
 * via the `definition` "CapParams".
 */
export interface CapParams {
  /**
   * Which events the cap measures, matched against their tags.
   */
  count: Selector;
  /**
   * The most the period may hold.
   */
  limit: number;
  /**
   * Whether events or their minutes count.
   */
  measure: CapMeasure;
  /**
   * Whether the limit applies to each participant or to the pool together.
   */
  per: CapPer;
  /**
   * The civil period a bucket covers.
   */
  period: CapPeriod;
  /**
   * The weekday a week bucket starts on, or absent for Monday.
   */
  week_starts?: string | null;
  /**
   * The zone the bucket labels are read in.
   */
  zone: string;
}
/**
 * The parameters of a `clearance` rule.
 *
 * This interface was referenced by `Schema`'s JSON-Schema
 * via the `definition` "ClearanceParams".
 */
export interface ClearanceParams {
  /**
   * The gap the booking needs after it.
   */
  after: string;
  /**
   * The gap the booking needs before it.
   */
  before: string;
  /**
   * The least gap between any two events.
   */
  min_gap: string;
}
/**
 * How many offers fall on one civil date.
 *
 * This interface was referenced by `Schema`'s JSON-Schema
 * via the `definition` "DateCount".
 */
export interface DateCount {
  /**
   * The date in the display zone.
   */
  date: string;
  /**
   * How many offers start on it.
   */
  offers: number;
}
/**
 * A `lead_time` maximum written as a date in the event zone.
 *
 * This interface was referenced by `Schema`'s JSON-Schema
 * via the `definition` "DateMax".
 */
export interface DateMax {
  /**
   * The last local date a start may fall on.
   */
  date: string;
}
/**
 * A `lead_time` maximum written as a count of calendar days.
 *
 * This interface was referenced by `Schema`'s JSON-Schema
 * via the `definition` "DaysMax".
 */
export interface DaysMax {
  /**
   * How many calendar days after today a start may fall on.
   */
  days: number;
}
/**
 * One validation finding.
 *
 * This interface was referenced by `Schema`'s JSON-Schema
 * via the `definition` "Diagnostic".
 */
export interface Diagnostic {
  /**
   * The stable, machine-readable code.
   */
  code: string;
  /**
   * The human-readable message.
   */
  message: string;
  /**
   * The RFC 6901 pointer of the request field this finding is about.
   */
  path?: string | null;
}
/**
 * What `validate` found in a request.
 *
 * This interface was referenced by `Schema`'s JSON-Schema
 * via the `definition` "Diagnostics".
 */
export interface Diagnostics {
  /**
   * Everything that makes the request impossible.
   */
  errors: Diagnostic[];
  /**
   * Everything the caller should know that does not make it impossible.
   */
  warnings: Diagnostic[];
}
/**
 * A `lead_time` maximum written as a duration.
 *
 * This interface was referenced by `Schema`'s JSON-Schema
 * via the `definition` "DurationMax".
 */
export interface DurationMax {
  /**
   * How far ahead a start may be.
   */
  duration: string;
}
/**
 * One candidate start that was not offered.
 *
 * This interface was referenced by `Schema`'s JSON-Schema
 * via the `definition` "Excluded".
 */
export interface Excluded {
  /**
   * The goal part the start belonged to.
   */
  part: string;
  /**
   * Why the start was not offered.
   */
  reasons: Reason[];
  /**
   * The start that was not offered.
   */
  start: string;
}
/**
 * One reason a participant cannot take a candidate start.
 *
 * This interface was referenced by `Schema`'s JSON-Schema
 * via the `definition` "Reason".
 */
export interface Reason {
  /**
   * Why the start was excluded.
   */
  code: string;
  /**
   * The event key of the block that caused the exclusion, when a block did.
   */
  event_key?: string | null;
  /**
   * The structured parameters the code carries, when it carries any.
   */
  params?: ReasonParams | null;
  /**
   * The participant the reason applies to, or absent for a start-level reason.
   */
  participant?: string | null;
  /**
   * The rule that produced the exclusion, when an explicit rule applies.
   */
  rule?: string | null;
  /**
   * The tags of the block that caused the exclusion, when a block did.
   */
  tags: string[];
}
/**
 * The structured parameters a [`WireReason`] carries.
 *
 * This interface was referenced by `Schema`'s JSON-Schema
 * via the `definition` "ReasonParams".
 */
export interface ReasonParams {
  /**
   * The bucket a cap was measured over.
   */
  bucket: string;
  /**
   * The cap's limit.
   */
  limit: number;
  /**
   * How much the bucket holds before the booking's contribution.
   */
  used: number;
}
/**
 * How much exclusion detail a search returns.
 *
 * This interface was referenced by `Schema`'s JSON-Schema
 * via the `definition` "Explain".
 */
export interface Explain {
  /**
   * The maximum number of reasons returned across every excluded start.
   */
  limit: number;
}
/**
 * What is being booked.
 *
 * This interface was referenced by `Schema`'s JSON-Schema
 * via the `definition` "Goal".
 */
export interface Goal {
  /**
   * Caller-supplied candidate start instants, instead of a grid.
   */
  candidates?: string[] | null;
  /**
   * The zone offers are grouped by date in, or absent for the event zone.
   */
  display_zone?: string | null;
  /**
   * The grid candidate starts are drawn from, or absent for the default grid.
   */
  grid?: Grid | null;
  /**
   * The parts of the booking. Version 1 requires exactly one.
   */
  parts: Part[];
  /**
   * The range of starts to consider.
   */
  search_window: Span;
  /**
   * The event zone.
   */
  zone: string;
}
/**
 * A local clock grid of candidate starts.
 *
 * This interface was referenced by `Schema`'s JSON-Schema
 * via the `definition` "Grid".
 */
export interface Grid {
  /**
   * The local minute each day's grid is anchored on, or absent for `00:00`.
   */
  anchor?: string | null;
  /**
   * The step between candidate local minutes.
   */
  step: string;
  /**
   * The zone the grid's clock is read in, or absent for the event zone.
   */
  zone?: string | null;
}
/**
 * One event within a goal.
 *
 * This interface was referenced by `Schema`'s JSON-Schema
 * via the `definition` "Part".
 */
export interface Part {
  /**
   * The length of the event.
   */
  duration: string;
  /**
   * The part's ID, or absent for `main`.
   */
  id?: string | null;
  /**
   * The terms that decide who can take it.
   */
  requirement: Requirement;
  /**
   * What is being booked.
   */
  tags?: string[] | null;
  /**
   * Travel the new booking needs, or absent for none.
   */
  travel?: Travel | null;
}
/**
 * Who can take a part: every named term, filled by distinct hosts.
 *
 * This interface was referenced by `Schema`'s JSON-Schema
 * via the `definition` "Requirement".
 */
export interface Requirement {
  /**
   * The terms every booking must satisfy.
   */
  all: Term[];
}
/**
 * A term filled by the participant with this ID.
 *
 * This interface was referenced by `Schema`'s JSON-Schema
 * via the `definition` "PersonTerm".
 */
export interface PersonTerm {
  /**
   * The participant's ID.
   */
  person: string;
}
/**
 * Travel the new booking needs, in each direction, with per-participant
 * overrides.
 *
 * This interface was referenced by `Schema`'s JSON-Schema
 * via the `definition` "Travel".
 */
export interface Travel {
  /**
   * Travel needed after the event, for participants without an override.
   */
  after: string;
  /**
   * Travel needed before the event, for participants without an override.
   */
  before: string;
  /**
   * Per-participant travel, keyed by participant ID.
   */
  by_participant?: {
    [k: string]: ParticipantTravel;
  } | null;
}
/**
 * One participant's travel, replacing the part's travel in both directions.
 *
 * This interface was referenced by `Schema`'s JSON-Schema
 * via the `definition` "ParticipantTravel".
 */
export interface ParticipantTravel {
  /**
   * Travel needed after the event.
   */
  after: string;
  /**
   * Travel needed before the event.
   */
  before: string;
}
/**
 * The working hours became a `window` rule.
 */
export interface AvailableWorkingHoursOutcome {
  /**
   * The `window` rule, in the wire layer's own shape.
   */
  rule: WindowRule;
  status: "available";
}
/**
 * When bookings may happen.
 *
 * This interface was referenced by `Schema`'s JSON-Schema
 * via the `definition` "WindowRule".
 */
export interface WindowRule {
  /**
   * The rule's ID, unique in the request.
   */
  id: string;
  kind: "window";
  /**
   * The window specification.
   */
  params: WindowParams;
  /**
   * Which bookings it applies to, or all of them by default.
   */
  when?: Selector | null;
  /**
   * Which participants the rule constrains, or every host by default.
   */
  who?: Selector | null;
}
/**
 * The parameters of a `window` rule.
 *
 * This interface was referenced by `Schema`'s JSON-Schema
 * via the `definition` "WindowParams".
 */
export interface WindowParams {
  /**
   * Hours that override the weekly ones, keyed by a date or date range.
   */
  dates?: {
    [k: string]: string[];
  } | null;
  /**
   * The hours each day key shares, keyed by a weekday list or range.
   */
  weekly?: {
    [k: string]: string[];
  } | null;
  /**
   * `participant`, `event`, or an IANA zone name.
   */
  zone: string;
}
/**
 * The working hours could not become a rule.
 */
export interface UnavailableWorkingHoursOutcome {
  /**
   * A stable, machine-readable code.
   */
  code: string;
  /**
   * A sentence naming what could not be converted and why.
   */
  message: string;
  status: "unavailable";
}
/**
 * A `lead_time` maximum written as a count of working days on a calendar.
 *
 * This interface was referenced by `Schema`'s JSON-Schema
 * via the `definition` "WorkingDaysMax".
 */
export interface WorkingDaysMax {
  /**
   * The request calendar that decides which days are working days.
   */
  calendar: string;
  /**
   * How many working days after today remain inside the horizon.
   */
  working_days: number;
}
/**
 * The parameters of a `lead_time` rule.
 *
 * This interface was referenced by `Schema`'s JSON-Schema
 * via the `definition` "LeadTimeParams".
 */
export interface LeadTimeParams {
  /**
   * The latest start allowed, or absent for none beyond the search window.
   */
  max?: LeadTimeMax | null;
  /**
   * The minimum notice, measured from the booking's occupied start.
   */
  min: string;
}
/**
 * Configurable caps on request size.
 *
 * Every field is optional: an absent field — or an explicit `null`, which a
 * binding that writes every key emits — is the engine's default for that cap,
 * `Limits::default`. Writing a `Limits` back out states all ten, so one
 * document carries the engine's whole configuration.
 *
 * This interface was referenced by `Schema`'s JSON-Schema
 * via the `definition` "Limits".
 */
export interface Limits {
  /**
   * Maximum number of blocks per participant, or absent for the engine
   * default.
   */
  max_blocks_per_participant?: number | null;
  /**
   * Maximum number of candidate starts in one search, or absent for the engine
   * default.
   */
  max_candidate_starts?: number | null;
  /**
   * Maximum length of a booking, a booking or block's travel or a clearance in
   * minutes, or absent for the engine default.
   */
  max_duration_minutes?: number | null;
  /**
   * Maximum `explain.limit` a request may ask for, or absent for the engine
   * default.
   */
  max_explain_limit?: number | null;
  /**
   * Maximum number of occurrences in one request, or absent for the engine
   * default.
   */
  max_occurrences?: number | null;
  /**
   * Maximum number of participants in one request, or absent for the engine
   * default.
   */
  max_participants?: number | null;
  /**
   * Maximum number of host slots one requirement may ask for, and the number of
   * terms it may name, or absent for the engine default.
   */
  max_requirement_slots?: number | null;
  /**
   * Maximum number of rules in one request, or absent for the engine default.
   */
  max_rules?: number | null;
  /**
   * Maximum search-window length in days, or absent for the engine default.
   *
   * A day count is already `u32` in the domain and no pointer width is
   * involved, so it is read as one here too: a larger value is rejected rather
   * than saturated into a limit no calendar could satisfy.
   */
  max_search_window_days?: number | null;
  /**
   * Maximum number of zone periods one participant may carry, or absent for the
   * engine default.
   */
  max_zone_periods_per_participant?: number | null;
}
/**
 * The versions an answer was produced from.
 *
 * This interface was referenced by `Schema`'s JSON-Schema
 * via the `definition` "Meta".
 */
export interface Meta {
  /**
   * The engine's version.
   */
  engine: string;
  /**
   * The schema version of the response shape.
   */
  schema: number;
  /**
   * The time zone database's version.
   */
  tzdb: string;
}
/**
 * One event the caller can identify, already expanded.
 *
 * This interface was referenced by `Schema`'s JSON-Schema
 * via the `definition` "Occurrence".
 */
export interface Occurrence {
  /**
   * The caller's ID for this event instance.
   */
  event_key: string;
  /**
   * The participants the event involves.
   */
  participants: string[];
  /**
   * When the event happens.
   */
  span: Span;
  /**
   * Everything the event is tagged with.
   */
  tags?: string[] | null;
}
/**
 * One bookable time.
 *
 * This interface was referenced by `Schema`'s JSON-Schema
 * via the `definition` "Offer".
 */
export interface Offer {
  /**
   * One event per goal part.
   */
  events: OfferEvent[];
  /**
   * What the caller should know about the offer.
   */
  flags: string[];
}
/**
 * One booked event: its part, its span, who could take it and who was chosen.
 *
 * This interface was referenced by `Schema`'s JSON-Schema
 * via the `definition` "OfferEvent".
 */
export interface OfferEvent {
  /**
   * The chosen assignment, one list per term.
   */
  assignment: string[][];
  /**
   * Who could fill each term, one list per term.
   */
  eligible: string[][];
  /**
   * The booking's end.
   */
  end: string;
  /**
   * The goal part's ID.
   */
  part: string;
  /**
   * Travel every participant the booking needs occupies.
   */
  reserve: ReserveSpan[];
  /**
   * The booking's start.
   */
  start: string;
}
/**
 * One span of time the caller should hold on a participant's calendar.
 *
 * This interface was referenced by `Schema`'s JSON-Schema
 * via the `definition` "ReserveSpan".
 */
export interface ReserveSpan {
  /**
   * What the hold is for.
   */
  kind: string;
  /**
   * The participant to hold it for.
   */
  participant: string;
  /**
   * The span to hold.
   */
  span: Span;
}
/**
 * One participant and what is already on their calendar.
 *
 * This interface was referenced by `Schema`'s JSON-Schema
 * via the `definition` "Participant".
 */
export interface Participant {
  /**
   * The participant's blocks, or absent when no calendar was supplied.
   */
  calendar?: Block[] | null;
  /**
   * The participant's opaque, stable ID.
   */
  id: string;
  /**
   * Person or resource, or absent for a person.
   */
  kind?: ParticipantKind | null;
  /**
   * Host or attendee, or absent for a host.
   */
  role?: Role | null;
  /**
   * Everything the participant is tagged with.
   */
  tags?: string[] | null;
  /**
   * The participant's home zone.
   */
  zone: string;
  /**
   * Spans in other zones.
   */
  zone_periods?: ZonePeriod[] | null;
}
/**
 * A span of instants during which a participant is in another zone.
 *
 * This interface was referenced by `Schema`'s JSON-Schema
 * via the `definition` "ZonePeriod".
 */
export interface ZonePeriod {
  /**
   * The first instant of the period.
   */
  from: string;
  /**
   * One past the last instant of the period.
   */
  until: string;
  /**
   * The zone the participant is in during the period.
   */
  zone: string;
}
/**
 * A booking a caller proposes to confirm.
 *
 * This interface was referenced by `Schema`'s JSON-Schema
 * via the `definition` "Proposal".
 */
export interface Proposal {
  /**
   * One proposed event per goal part.
   */
  events: ProposalEvent[];
}
/**
 * One event of a [`WireProposal`].
 *
 * This interface was referenced by `Schema`'s JSON-Schema
 * via the `definition` "ProposalEvent".
 */
export interface ProposalEvent {
  /**
   * The hosts to assign, one list per term.
   */
  assignment: string[][];
  /**
   * The goal part's ID.
   */
  part: string;
  /**
   * The proposed start.
   */
  start: string;
}
/**
 * What calendar and occurrence data a request needs.
 *
 * This interface was referenced by `Schema`'s JSON-Schema
 * via the `definition` "RangesQuery".
 */
export interface RangesQuery {
  /**
   * What is being booked.
   */
  goal: Goal;
  /**
   * The current instant, which the wire rounds up to the next minute.
   */
  now: string;
  /**
   * The participants, without their calendars.
   */
  participants?: Participant[] | null;
  /**
   * The final rule list.
   */
  rules?: Rule[] | null;
}
/**
 * When bookings may not happen.
 */
export interface BlackoutRule {
  /**
   * The rule's ID, unique in the request.
   */
  id: string;
  kind: "blackout";
  /**
   * The blackout specification.
   */
  params: BlackoutParams;
  /**
   * Which bookings it applies to, or all of them by default.
   */
  when?: Selector | null;
  /**
   * Which participants the rule constrains, or every host by default.
   */
  who?: Selector | null;
}
/**
 * The gaps the new booking needs between meetings.
 */
export interface ClearanceRule {
  /**
   * The rule's ID, unique in the request.
   */
  id: string;
  kind: "clearance";
  /**
   * The clearance parameters.
   */
  params: ClearanceParams;
  /**
   * Which bookings it applies to, or all of them by default.
   */
  when?: Selector | null;
  /**
   * Which participants the rule constrains, or every host by default.
   */
  who?: Selector | null;
}
/**
 * How soon and how far ahead a booking may start.
 */
export interface LeadTimeRule {
  /**
   * The rule's ID, unique in the request.
   */
  id: string;
  kind: "lead_time";
  /**
   * The lead-time parameters.
   */
  params: LeadTimeParams;
  /**
   * Which bookings it applies to, or all of them by default.
   */
  when?: Selector | null;
  /**
   * Which participants the rule constrains, or every host by default.
   */
  who?: Selector | null;
}
/**
 * How statuses and tags affect availability.
 */
export interface StatusPolicyRule {
  /**
   * The rule's ID, unique in the request.
   */
  id: string;
  kind: "status_policy";
  /**
   * The policy parameters.
   */
  params: StatusPolicyParams;
  /**
   * Which bookings it applies to, or all of them by default.
   */
  when?: Selector | null;
  /**
   * Which participants the rule constrains, or every host by default.
   */
  who?: Selector | null;
}
/**
 * The parameters of a `status_policy` rule.
 *
 * This interface was referenced by `Schema`'s JSON-Schema
 * via the `definition` "StatusPolicyParams".
 */
export interface StatusPolicyParams {
  /**
   * The entries, in the order the caller wrote them.
   */
  entries: StatusPolicyEntry[];
}
/**
 * One status-policy entry: what it matches and what it decides.
 *
 * This interface was referenced by `Schema`'s JSON-Schema
 * via the `definition` "StatusPolicyEntry".
 */
export interface StatusPolicyEntry {
  /**
   * What happens to a block the entry matches.
   */
  effect: StatusEffect;
  /**
   * The tag selector this entry matches, if it matches on tags.
   */
  match?: Selector | null;
  /**
   * The block status this entry matches, if it matches on status.
   *
   * Unlike a block's `status`, this one is a closed set. A block's status is
   * provider data, where an unrecognised value has to fail safe toward busy;
   * an entry is caller-authored configuration, where an unrecognised value is
   * a mistake. Reading one as `busy` would turn an entry meant to ignore one
   * kind of event into one that ignores every busy block.
   */
  status?: Status | null;
}
/**
 * A late finish blocks an early start.
 */
export interface RestRule {
  /**
   * The rule's ID, unique in the request.
   */
  id: string;
  kind: "rest";
  /**
   * The rest parameters.
   */
  params: RestParams;
  /**
   * Which bookings it applies to, or all of them by default.
   */
  when?: Selector | null;
  /**
   * Which participants the rule constrains, or every host by default.
   */
  who?: Selector | null;
}
/**
 * The parameters of a `rest` rule.
 *
 * This interface was referenced by `Schema`'s JSON-Schema
 * via the `definition` "RestParams".
 */
export interface RestParams {
  /**
   * The local time from which a finish counts as late.
   */
  late_after: string;
  /**
   * The local time before which a start counts as early.
   */
  not_before: string;
}
/**
 * A limit on how much of a period may be booked.
 */
export interface CapRule {
  /**
   * The rule's ID, unique in the request.
   */
  id: string;
  kind: "cap";
  /**
   * The cap parameters.
   */
  params: CapParams;
  /**
   * Which bookings it applies to, or all of them by default.
   */
  when?: Selector | null;
  /**
   * Which participants the rule constrains, or every host by default.
   */
  who?: Selector | null;
}
/**
 * A request to search for availability.
 *
 * This interface was referenced by `Schema`'s JSON-Schema
 * via the `definition` "Request".
 */
export interface Request {
  /**
   * Named working calendars, by name.
   */
  calendars?: {
    [k: string]: CalendarSpec;
  } | null;
  /**
   * The caller's event keys for the events it is moving, which the engine
   * ignores wherever an existing event would otherwise block time, trigger
   * `rest` activity or count toward a cap.
   */
  exclude_event_keys?: string[] | null;
  /**
   * Whether to report why candidate starts were not offered.
   */
  explain?: Explain | null;
  /**
   * What is being booked.
   */
  goal: Goal;
  /**
   * The maximum number of offers to return, or absent for the engine's default.
   */
  limit?: number | null;
  /**
   * The current instant, which the wire rounds up to the next minute.
   */
  now: string;
  /**
   * The caller's bookings and sessions.
   */
  occurrences?: Occurrence[] | null;
  /**
   * Everyone the request knows about.
   */
  participants?: Participant[] | null;
  /**
   * The final rule list.
   */
  rules?: Rule[] | null;
  /**
   * The weight each participant's assignment carries, by participant ID.
   */
  weights?: {
    [k: string]: number;
  } | null;
}
/**
 * One UTC envelope of data a request needs.
 *
 * Ask for this before a search or a confirmation when the request can be capped
 * or calendars come from a provider. `calendar` is the hull the search window's
 * civil days cover; `occurrences` is the hull of the cap buckets the window
 * touches, or absent when no cap applies.
 *
 * The hulls start a two-phase fetch, not a one-pass query: a cap merges every
 * record sharing an event key across all supplied time ranges. **Initial fetch:**
 * every block whose occupied span — its own span widened by its travel — overlaps
 * `calendar`, and every occurrence and block whose own span overlaps
 * `occurrences`. **Event-key closure:** collect the event keys observed on those
 * own-span occurrence-range records, then fetch every record carrying one of those
 * keys on every participant, all-day and otherwise nonmatching records included,
 * and judge that closed request against a consistent snapshot. `occurrences` is one
 * request-global list: fetch every occurrence, never filtering by the participants
 * one names.
 *
 * **Confirmation outside the search window:** the hulls cover only instants inside
 * the goal's `search_window`, but `check_slot` ignores that window. Confirming a
 * start outside it needs a new ranges query whose window contains the start,
 * followed by the same initial fetch and event-key closure.
 *
 * **Exact closure unavailable:** a one-pass fetch is key-complete only when every
 * copy of an event has the identical own span; otherwise fetch the full supported
 * time domain with limits raised to cover it (`max_blocks_per_participant` and
 * `max_occurrences`), or fail closed by returning an error rather than evaluating
 * narrowed data. See the Rust `RequiredRanges` documentation for the whole
 * contract.
 *
 * This interface was referenced by `Schema`'s JSON-Schema
 * via the `definition` "RequiredRanges".
 */
export interface RequiredRanges {
  /**
   * The UTC range every participant's calendar must cover, planned from the
   * goal's `search_window`.
   *
   * Fetch every block whose occupied span — the block's own span widened by its
   * `travel_before` and `travel_after` — overlaps this range, not only blocks
   * whose own span does: a block ending before the range can still occupy a
   * minute inside it. A confirmation outside the search window needs a new
   * ranges query whose window contains the confirmed start.
   */
  calendar: Span;
  /**
   * The UTC range of the initial occurrence-range fetch, or absent when no cap
   * applies.
   *
   * Fetch every occurrence and every calendar block whose own span overlaps it,
   * on every participant, and never filter occurrences by the participants
   * they name: the list is request-global. Then perform the event-key closure:
   * fetch every record carrying a key observed on those own-span
   * occurrence-range records, on every participant, and evaluate the closed
   * request. An omitted field and an explicit `null` both read as no occurrence
   * range; the field is omitted rather than written as `null`, matching every
   * other optional response field. When that closure cannot be completed, fetch
   * the full supported time domain with `Limits` raised to cover it — raise
   * `max_blocks_per_participant` and `max_occurrences`, because the published
   * defaults can refuse that fetch — or fail closed: the caller's own entry
   * point returns an error instead of evaluating narrowed data.
   */
  occurrences?: Span | null;
}
/**
 * What a search returns.
 *
 * This interface was referenced by `Schema`'s JSON-Schema
 * via the `definition` "Response".
 */
export interface Response {
  /**
   * Every civil date the search window touches, with its offer count.
   */
  dates: DateCount[];
  /**
   * The candidate starts that were not offered.
   */
  excluded: Excluded[];
  /**
   * Whether the explanation limit cut the exclusion list short.
   */
  excluded_truncated: boolean;
  /**
   * The versions the answer was produced from.
   */
  meta: Meta;
  /**
   * The bookable times, in start order.
   */
  offers: Offer[];
  /**
   * Whether the offer limit cut the list short.
   */
  truncated: boolean;
  /**
   * Request warnings.
   */
  warnings: Diagnostic[];
}
/**
 * The answer to checking one proposed booking.
 *
 * This interface was referenced by `Schema`'s JSON-Schema
 * via the `definition` "SlotCheck".
 */
export interface SlotCheck {
  /**
   * What the caller should know that does not make it invalid.
   */
  flags: string[];
  /**
   * The versions the answer was produced from.
   */
  meta: Meta;
  /**
   * Every reason the booking fails.
   */
  reasons: Reason[];
  /**
   * Travel every participant the booking needs occupies.
   */
  reserve: ReserveSpan[];
  /**
   * Whether the booking can be made as proposed.
   */
  valid: boolean;
  /**
   * The request's validation warnings.
   */
  warnings: Diagnostic[];
}
