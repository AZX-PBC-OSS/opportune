//! Reference oracle, generators, scenarios and benchmark inputs for testing the
//! [`opportune`] engine.
//!
//! The oracle deliberately re-implements the engine's semantics without the
//! engine's in-house interval and matching code, so a bug in either surfaces as
//! a disagreement with the oracle rather than a shared blind spot.
//!
//! This crate is a testing aid and is never published.
//!
//! [`opportune`]: https://docs.rs/opportune

pub mod a1;
pub mod a2;
pub mod canonical;
pub mod generators;
pub mod json_goldens;
pub mod matching;
pub mod oracle;
mod oracle_support;
pub mod scenarios;
pub mod scenarios_a2;
pub mod specs;
pub mod workloads;

pub use oracle::MinuteSet;
