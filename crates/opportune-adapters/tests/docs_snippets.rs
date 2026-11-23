//! The provider-adapter recipe from `docs/recipes.md`, compiled and run against
//! this crate.
//!
//! The snippet file lives beside the core crate's snippet corpus so the
//! documentation has one source for its Rust blocks; this test is the half that
//! compiles it, because `opportune` does not depend on this crate.
//!
//! The recipe converts both providers and builds an engine from the bundled
//! database, so this target compiles only when all three features are on. The
//! feature matrix builds this crate with `graph` alone to prove `google` (and
//! `windows-zones`) stay optional, and the snippet's imports must not force
//! `google` into that configuration.

#![cfg(all(feature = "graph", feature = "google", feature = "bundled-tzdb"))]

mod recipe_provider_adapters {
    include!("../../opportune/tests/snippets/recipe_provider_adapters.rs");

    #[test]
    fn runs() -> Result<(), Box<dyn std::error::Error>> {
        main()
    }

    #[test]
    fn custom_zone_fallback_keeps_the_mailbox_scope() -> Result<(), Box<dyn std::error::Error>> {
        let engine = opportune::Engine::builder().build()?;
        let zone = opportune::time::ZoneName::new("America/New_York")?;
        let custom = r##"{
            "daysOfWeek": ["monday"],
            "startTime": "09:00:00.0000000",
            "endTime": "17:00:00.0000000",
            "timeZone": {
                "@odata.type": "#microsoft.graph.customTimeZone",
                "bias": 60
            }
        }"##;
        let rule = working_hours_window(&engine, custom, &zone)?;
        assert_eq!(rule.who, opportune::tags::Selector::ids(["ana"]));
        Ok(())
    }
}
