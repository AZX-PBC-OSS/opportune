//! Adversarial probes for the public surface a downstream crate builds against.
//!
//! Each probe targets one assumption an external caller makes about this crate's
//! own public surface: that a non-exhaustive enum is still constructible outside
//! the crate, that the types an embedding service's runtime shares across tasks
//! carry the traits it needs, and that the sugar the crate documents is reachable
//! from `opportune::model` alone. It is compiled against the crate's public API
//! exactly as an external caller sees it; the rounding constructors' range
//! behaviour is probed in `time_contract.rs`, which reaches them the same way.

use opportune::model::{
    CapPer, CapPeriod, Goal, Part, Participant, Requirement, Role, Rule, RuleKind, SCHEMA_VERSION,
    Selector, Span, StatusPolicy, Tag, Term, Travel,
};
use opportune::presets;
use opportune::reasons::{Reason, ReasonCode, ReasonParams};
use opportune::time::{
    BlackoutSpan, Date, Duration, Instant, LocalDateTime, RuleZone, TimeError, TimeOfDay,
    TimeZoneDatabase, ZoneName,
};
use opportune::{Engine, Error};

/// `Reason::with_params` takes [`ReasonParams`], so a caller outside the crate
/// has to be able to build the enum. `#[non_exhaustive]` restricts exhaustive
/// matching; it must not restrict constructing the variants that exist, or the
/// constructor is unusable from the only place it matters — a caller, a binding
/// or `opportune-json` building a reason of its own. Nothing inside the crate
/// can prove this, because the attribute has no effect on in-crate code.
#[test]
fn a_downstream_caller_can_build_cap_parameters() {
    let params = ReasonParams::CapReached {
        used: 2,
        limit: 2,
        bucket: "2027-03-09".to_owned(),
    };
    let reason = Reason::new("ana", ReasonCode::CapReached).with_params(params.clone());
    assert_eq!(reason.params.as_ref(), Some(&params));
    assert!(
        reason.to_string().contains("used 2 of 2 in 2027-03-09"),
        "the parameters render in the reason's text"
    );
}

/// An `Engine` is built once and shared: an `Arc<Engine>` moves into every
/// handler, and a server spawns one task per request over it. `TimeZoneDatabase`
/// is cloneable so a caller can hold one beside the engine it built. Every error
/// crosses a `Box<dyn Error + Send + Sync>` boundary — `anyhow`, `thiserror`'s
/// `#[source]`, `tower`'s `BoxError` — so it has to carry those bounds itself.
///
/// The assertions are compile-time: if a future field makes any of these types
/// fail a bound, this test does not build.
#[test]
fn the_public_types_carry_the_traits_an_embedding_service_needs() {
    fn send_sync_clone<T: Send + Sync + Clone>() {}
    fn error<T: std::error::Error + Send + Sync + 'static>() {}

    send_sync_clone::<Engine>();
    send_sync_clone::<TimeZoneDatabase>();
    error::<Error>();
    error::<TimeError>();
}

/// A one-part goal that needs the part's own ID, tags or travel. `Goal::single`
/// builds a fresh `Part::new`, so it drops all three; a caller outside the crate
/// has to be able to keep them without adopting the multi-part list shape.
#[test]
fn a_single_part_goal_keeps_the_parts_own_id_tags_and_travel() {
    let zone = ZoneName::utc();
    let part = Part::new(
        Duration::from_minutes(45),
        Requirement::new(vec![Term::person("ana")]),
    )
    .with_id("round-1")
    .with_tags(vec![Tag::new("interview").unwrap()])
    .with_travel(Travel::new(
        Duration::from_minutes(15),
        Duration::from_minutes(10),
    ));

    let goal = Goal::single_part(zone.clone(), Span::EVERYTHING, part);
    assert_eq!(goal.parts.len(), 1);
    assert_eq!(goal.parts[0].id, "round-1");
    assert_eq!(goal.parts[0].tags, [Tag::new("interview").unwrap()]);
    assert_eq!(goal.parts[0].travel.for_participant("ana").0.minutes(), 15);
    assert_eq!(goal.display_zone, zone);
    assert!(goal.grid.is_none());
}

/// The zone a fixed-time request needs, without a `Result` the caller cannot
/// fail. The crate's own presets use it internally, so it has to be reachable
/// from outside.
#[test]
fn the_utc_zone_name_needs_neither_a_result_nor_an_unwrap() {
    assert_eq!(ZoneName::utc().as_str(), "UTC");
    assert_eq!(ZoneName::new("UTC").unwrap(), ZoneName::utc());
}

/// The whole supported range, as one value instead of a `Span::new` the caller
/// has to unwrap. It is still the same span the fallible constructor builds.
#[test]
fn span_everything_is_the_whole_supported_range() {
    assert_eq!(Span::EVERYTHING.start(), Instant::MIN);
    assert_eq!(Span::EVERYTHING.end(), Instant::MAX);
    assert_eq!(
        Span::new(Instant::MIN, Instant::MAX).unwrap(),
        Span::EVERYTHING
    );
    assert!(Span::EVERYTHING.contains(Instant::MIN));
    assert!(!Span::EVERYTHING.contains(Instant::MAX));
}

/// Every rule preset is reachable from `opportune::presets`, and a blackout is
/// the one the list used to be missing: it needs no rule list, no `Rule::new` and
/// no `RuleKind` to write. `Rule::new`'s defaults are what make it host-scoped
/// and unconditional.
#[test]
fn the_blackout_preset_is_host_scoped_and_unconditional() {
    let shutdown = BlackoutSpan::new(
        LocalDateTime::new(Date::new(2026, 12, 24).unwrap(), TimeOfDay::MIDNIGHT),
        LocalDateTime::new(Date::new(2027, 1, 2).unwrap(), TimeOfDay::MIDNIGHT),
    )
    .unwrap();

    let rule = presets::blackout(RuleZone::Named(ZoneName::utc()), vec![shutdown]);
    assert_eq!(rule.id, "blackout");
    assert_eq!(rule.who, Selector::Role(Role::Host));
    assert_eq!(rule.when, Selector::True);
    let spans = rule.blackout().expect("a blackout rule").spans();
    assert_eq!(spans.len(), 1);
    assert_eq!(spans[0], shutdown);
}

/// A pooled daily cap, or one bucketed in the event zone, without the seven
/// positional arguments of `Cap::new` — and the two-argument preset keeps the
/// scope it always had.
#[test]
fn the_daily_cap_preset_exposes_per_and_zone() {
    let pooled = presets::cap_per_day_with(
        Selector::tag(Tag::new("interview").unwrap()),
        4,
        CapPer::Pooled,
        RuleZone::Event,
    );
    let cap = pooled.cap().expect("a cap rule");
    assert_eq!(pooled.id, "cap_per_day");
    assert_eq!(
        (cap.limit, cap.period, cap.per),
        (4, CapPeriod::Day, CapPer::Pooled)
    );
    assert_eq!(cap.zone, RuleZone::Event);

    let each = presets::cap_per_day(Selector::True, 2);
    let cap = each.cap().expect("a cap rule");
    assert_eq!(cap.per, CapPer::Each);
    assert_eq!(cap.zone, RuleZone::Participant);
}

/// `opportune::model` has to be self-contained for the types its own values
/// expose: `Participant.role`, `Rule.who` and `StatusPolicyEntry.tags` are field
/// types, so a caller holding a model value must be able to name them from the
/// same module. `SCHEMA_VERSION` is the response contract's version and belongs
/// beside `DEFAULT_LIMIT` at the crate root.
#[test]
fn model_names_every_field_type_of_its_own_values() {
    let participant = Participant::new("ana", ZoneName::utc()).with_role(Role::Attendee);
    assert_eq!(participant.role, Role::Attendee);

    let selector = Selector::tag(Tag::new("team/sales").unwrap());
    let rule = Rule::new("r_1", RuleKind::StatusPolicy(StatusPolicy::default()))
        .with_who(selector.clone());
    assert_eq!(rule.who, selector);

    assert_eq!(opportune::SCHEMA_VERSION, SCHEMA_VERSION);
    assert_eq!(SCHEMA_VERSION, 1);
}
