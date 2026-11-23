//! Calendar availability and scheduling calculations across constraints.
//!
//! A [`Request`] goes in — the current instant, what is to be booked, the
//! participants and their calendars, and the caller's final rule list — and a
//! [`Response`] comes out: the times that can be booked, who could take each one,
//! and the travel to reserve. The engine is a pure function of its input: `now`
//! is an argument, there is no clock and no I/O, and the same request against the
//! same versions gives the same answer everywhere.
//!
//! [`Engine`] holds the time zone database and the request limits. It is built
//! once and reused, and it is the only thing that resolves a request's zone and
//! calendar names, which is what keeps a request a plain value a caller can build
//! and store without either.
//!
//! # Quickstart
//!
//! One person, one 30-minute booking, over a week of weekdays with ten-minute
//! buffers and four hours' notice:
//!
//! ```
//! use opportune::Engine;
//! use opportune::model::{Goal, Participant, Request, Requirement, Span, Term};
//! use opportune::presets;
//! use opportune::time::{
//!     Date, Duration, Instant, RuleZone, TimeOfDay, Weekday, Weekdays, ZoneName,
//! };
//!
//! # #[cfg(feature = "bundled-tzdb")] {
//! let zone = ZoneName::new("Europe/London").unwrap();
//! let now = Instant::from_utc(Date::new(2027, 6, 14).unwrap(), TimeOfDay::new(7, 0).unwrap()).unwrap();
//! let until = Instant::from_utc(Date::new(2027, 6, 19).unwrap(), TimeOfDay::new(0, 0).unwrap()).unwrap();
//! let mut request = Request::new(
//!     now,
//!     Goal::single(
//!         zone.clone(),
//!         Span::new(now, until).unwrap(),
//!         Duration::from_minutes(30),
//!         Requirement::new(vec![Term::person("rich")]),
//!     ),
//! );
//! request
//!     .participants
//!     .push(Participant::new("rich", zone).with_calendar(Vec::new()));
//! request.rules.push(presets::business_hours(
//!     RuleZone::Participant,
//!     Weekdays::range(Weekday::Monday, Weekday::Friday),
//!     TimeOfDay::new(9, 0).unwrap(),
//!     TimeOfDay::new(17, 0).unwrap(),
//! ).unwrap());
//! request.rules.push(presets::buffers(Duration::from_minutes(10), Duration::from_minutes(10)));
//! request.rules.push(presets::min_notice(Duration::from_hours_minutes(4, 0)));
//!
//! let response = Engine::builder().build().unwrap().find_availability(&request).unwrap();
//! assert!(!response.offers.is_empty());
//! # }
//! ```
//!
//! # Entry points
//!
//! [`Engine::find_availability`] books forwards: every candidate start that the
//! participants' calendars, rules, caps and rest allow, each with who could take
//! it. [`Engine::check_slot`] judges one proposed booking at confirmation time,
//! against the same rules and calendars, reporting every failing reason.
//! [`Engine::validate`] reports the request's structural errors and its warnings
//! before any availability is computed, and [`Engine::required_ranges`] says which
//! calendar and occurrence data the caller has to fetch. With the default
//! `bundled-tzdb` feature the same four are also free functions using a default
//! engine.
//!
//! # Time zone databases
//!
//! The database is always an explicit choice, never the host's: results that
//! depend on the machine are not reproducible. `EngineBuilder::tzdb_bundled`
//! uses the database compiled into the binary (the default with the
//! `bundled-tzdb` feature), and `EngineBuilder::tzdb_dir` reads a directory of
//! `TZif` files, labelling it from its `+VERSION` file or from the label the
//! caller passes. Without `bundled-tzdb` there is no default engine and building
//! one fails with `tzdb_not_configured` until a directory is given.
//!
//! # Rule kinds
//!
//! Rules are typed data with fixed meaning, and the caller supplies the final
//! list: `window`, `blackout`, `clearance`, `lead_time`, `status_policy`, `rest`
//! and `cap`. [`presets`] builds the common ones into [`Rule`] values, and the
//! builders in [`model`] and [`time`] cover the rest.
//!
//! # Reasons and errors
//!
//! Every excluded start carries structured [`reasons`] when the request asks for
//! an explanation, each with a stable [`ReasonCode`]; [`ReasonCode::ALL`] is the
//! registry, in the precedence order a participant's first failing reason
//! follows. Every failure is a typed [`Error`] whose `code()` is stable across
//! releases, and a request that [`Engine::validate`] rejects comes back with
//! [`Diagnostic`] findings that name the request field at fault.
//!
//! # Feature flags
//!
//! `bundled-tzdb` (default) compiles the time zone database into the crate and
//! provides the free functions. Turning it off leaves the core with a
//! caller-supplied database only, which is what a strict license build or a
//! `wasm32` build with its own data uses.
//!
//! This crate is pure Rust: the Python bindings live in `opportune-py`, which
//! depends on this crate by path and re-exports [`VERSION`] as
//! `opportune.__version__`.

pub mod error;
pub mod intervals;
// Exposed for `opportune-testkit`'s brute-force oracle, which compares its own
// assignment against the engine's. Hidden from the documented surface so its
// `Term` does not read as a second, competing definition beside `model::Term`;
// its assignments are an implementation detail of the engine, not a contract a
// caller builds against.
#[doc(hidden)]
pub mod matching;
pub mod model;
pub mod presets;
pub mod reasons;
pub mod tags;
pub mod time;

mod caps;
mod engine;
mod eval;
mod exclude;
mod limits;
mod occupancy;
mod output;
mod ranges;
mod rest;
mod rules;
mod validate;

pub use engine::{Engine, EngineBuilder};
#[cfg(feature = "bundled-tzdb")]
pub use engine::{check_slot, find_availability, required_ranges, validate};
pub use error::{Diagnostic, Diagnostics, Error};
pub use limits::Limits;
pub use model::{
    Block, BlockStatus, CalendarSpec, Cap, CapMeasure, CapPer, CapPeriod, Clearance, DEFAULT_LIMIT,
    DateCount, Explain, Goal, LeadTime, LeadTimeMax, Meta, Occurrence, Offer, OfferEvent,
    OfferFlag, Part, Participant, ParticipantKind, ParticipantTravel, Proposal, ProposalEvent,
    RangesQuery, Request, RequiredRanges, Requirement, ReserveKind, ReserveSpan, Response, Rest,
    Rule, RuleKind, SCHEMA_VERSION, SlotCheck, Span, StatusEffect, StatusPolicy, StatusPolicyEntry,
    Term, Travel, ZonePeriod,
};
pub use reasons::{Excluded, Reason, ReasonCode, ReasonParams};

/// The crate version, re-exported by the `opportune-py` bindings as
/// `opportune.__version__`.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
