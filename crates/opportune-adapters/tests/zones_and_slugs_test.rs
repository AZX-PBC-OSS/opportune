//! Zone resolution, the Windows zone sweep, category slugging and marker tags.

// Every case resolves zone names against a real database, so this target runs
// where the crate compiles one in: `make test-matrix`'s `graph` build keeps
// `bundled-tzdb` on, and checks the bundle-free build separately.
#![cfg(feature = "bundled-tzdb")]

use std::collections::BTreeMap;

use opportune::tags::Tag;
use opportune::time::{TimeZoneDatabase, ZoneName};
use opportune_adapters::zones::{category_to_tag, parse_marker_tags, resolve_zone};
use opportune_adapters::{AdapterDiagnostic, AdapterZoneError};
#[cfg(feature = "windows-zones")]
use windows_timezones::WindowsTimezone;

/// The bundled database every conversion resolves names against.
fn db() -> TimeZoneDatabase {
    TimeZoneDatabase::bundled().expect("the bundled database is compiled in")
}

/// The IANA name a payload zone name resolves to.
fn resolved(name: &str) -> String {
    resolve_zone(&db(), name)
        .unwrap_or_else(|err| panic!("`{name}` should resolve: {err}"))
        .name()
        .to_owned()
}

#[test]
fn iana_names_are_tried_first() {
    let db = db();
    for name in [
        "UTC",
        "Etc/UTC",
        "America/New_York",
        "Europe/London",
        "Australia/Lord_Howe",
        "Asia/Kathmandu",
        "US/Eastern",
    ] {
        let zone = resolve_zone(&db, name).unwrap();
        assert_eq!(zone.name(), name);
        // The name the adapter returns is one the engine can resolve again: a
        // rule built from a payload has to survive being written to a request.
        assert!(db.zone(zone.name()).is_ok(), "{name}");
    }
}

#[test]
fn the_alias_table_covers_the_names_neither_database_does() {
    assert_eq!(resolved("Coordinated Universal Time"), "UTC");
    assert_eq!(resolved("  Coordinated Universal Time  "), "UTC");
    assert_eq!(resolved("Mid-Atlantic Standard Time"), "Etc/GMT+2");
    assert_eq!(resolved("Kamchatka Standard Time"), "Asia/Kamchatka");
}

#[test]
fn an_unresolvable_name_is_an_error_rather_than_utc() {
    let error = resolve_zone(&db(), "Not A Real Zone").unwrap_err();
    assert_eq!(error.code(), "zone_unknown");
    assert_eq!(
        error,
        AdapterZoneError::Unresolved {
            name: "Not A Real Zone".to_owned()
        }
    );
    assert_eq!(resolve_zone(&db(), "").unwrap_err().code(), "zone_unknown");
}

#[cfg(feature = "windows-zones")]
#[test]
fn windows_names_resolve_to_the_iana_zone_behind_them() {
    for (windows, iana) in [
        ("Pacific Standard Time", "America/Los_Angeles"),
        ("Eastern Standard Time", "America/New_York"),
        ("GMT Standard Time", "Europe/London"),
        ("AUS Eastern Standard Time", "Australia/Sydney"),
        ("W. Europe Standard Time", "Europe/Berlin"),
    ] {
        assert_eq!(resolved(windows), iana, "{windows}");
    }
    // CLDR maps India to the zone's legacy alias, which the database still holds.
    let india = resolve_zone(&db(), "India Standard Time").unwrap();
    assert_eq!(india.name(), "Asia/Calcutta");
    assert!(db().zone(india.name()).is_ok());
}

/// Every name `windows-timezones` 0.5.1 parses, as published by its `FromStr`
/// table. A version bump that adds or renames a zone updates this list with it.
#[cfg(feature = "windows-zones")]
const WINDOWS_NAMES: [&str; 139] = [
    "Dateline Standard Time",
    "UTC-11",
    "Aleutian Standard Time",
    "Hawaiian Standard Time",
    "Marquesas Standard Time",
    "Alaskan Standard Time",
    "UTC-09",
    "Pacific Standard Time (Mexico)",
    "UTC-08",
    "Pacific Standard Time",
    "US Mountain Standard Time",
    "Mountain Standard Time (Mexico)",
    "Mountain Standard Time",
    "Yukon Standard Time",
    "Central America Standard Time",
    "Central Standard Time",
    "Easter Island Standard Time",
    "Central Standard Time (Mexico)",
    "Canada Central Standard Time",
    "SA Pacific Standard Time",
    "Eastern Standard Time (Mexico)",
    "Eastern Standard Time",
    "Haiti Standard Time",
    "Cuba Standard Time",
    "US Eastern Standard Time",
    "Turks And Caicos Standard Time",
    "Paraguay Standard Time",
    "Atlantic Standard Time",
    "Venezuela Standard Time",
    "Central Brazilian Standard Time",
    "SA Western Standard Time",
    "Pacific SA Standard Time",
    "Newfoundland Standard Time",
    "Tocantins Standard Time",
    "E. South America Standard Time",
    "SA Eastern Standard Time",
    "Argentina Standard Time",
    "Greenland Standard Time",
    "Montevideo Standard Time",
    "Magallanes Standard Time",
    "Saint Pierre Standard Time",
    "Bahia Standard Time",
    "UTC-02",
    "Azores Standard Time",
    "Cape Verde Standard Time",
    "UTC",
    "GMT Standard Time",
    "Greenwich Standard Time",
    "Sao Tome Standard Time",
    "Morocco Standard Time",
    "W. Europe Standard Time",
    "Central Europe Standard Time",
    "Romance Standard Time",
    "Central European Standard Time",
    "W. Central Africa Standard Time",
    "Jordan Standard Time",
    "GTB Standard Time",
    "Middle East Standard Time",
    "Egypt Standard Time",
    "E. Europe Standard Time",
    "Syria Standard Time",
    "West Bank Standard Time",
    "South Africa Standard Time",
    "FLE Standard Time",
    "Israel Standard Time",
    "South Sudan Standard Time",
    "Kaliningrad Standard Time",
    "Sudan Standard Time",
    "Libya Standard Time",
    "Namibia Standard Time",
    "Arabic Standard Time",
    "Turkey Standard Time",
    "Arab Standard Time",
    "Belarus Standard Time",
    "Russian Standard Time",
    "E. Africa Standard Time",
    "Iran Standard Time",
    "Arabian Standard Time",
    "Astrakhan Standard Time",
    "Azerbaijan Standard Time",
    "Russia Time Zone 3",
    "Mauritius Standard Time",
    "Saratov Standard Time",
    "Georgian Standard Time",
    "Volgograd Standard Time",
    "Caucasus Standard Time",
    "Afghanistan Standard Time",
    "West Asia Standard Time",
    "Ekaterinburg Standard Time",
    "Pakistan Standard Time",
    "Qyzylorda Standard Time",
    "India Standard Time",
    "Sri Lanka Standard Time",
    "Nepal Standard Time",
    "Central Asia Standard Time",
    "Bangladesh Standard Time",
    "Omsk Standard Time",
    "Myanmar Standard Time",
    "SE Asia Standard Time",
    "Altai Standard Time",
    "W. Mongolia Standard Time",
    "North Asia Standard Time",
    "N. Central Asia Standard Time",
    "Tomsk Standard Time",
    "China Standard Time",
    "North Asia East Standard Time",
    "Singapore Standard Time",
    "W. Australia Standard Time",
    "Taipei Standard Time",
    "Ulaanbaatar Standard Time",
    "Aus Central W. Standard Time",
    "Transbaikal Standard Time",
    "Tokyo Standard Time",
    "North Korea Standard Time",
    "Korea Standard Time",
    "Yakutsk Standard Time",
    "Cen. Australia Standard Time",
    "AUS Central Standard Time",
    "E. Australia Standard Time",
    "AUS Eastern Standard Time",
    "West Pacific Standard Time",
    "Tasmania Standard Time",
    "Vladivostok Standard Time",
    "Lord Howe Standard Time",
    "Bougainville Standard Time",
    "Russia Time Zone 10",
    "Magadan Standard Time",
    "Norfolk Standard Time",
    "Sakhalin Standard Time",
    "Central Pacific Standard Time",
    "Russia Time Zone 11",
    "New Zealand Standard Time",
    "UTC+12",
    "Fiji Standard Time",
    "Chatham Islands Standard Time",
    "UTC+13",
    "Tonga Standard Time",
    "Samoa Standard Time",
    "Line Islands Standard Time",
];

#[cfg(feature = "windows-zones")]
#[test]
fn every_windows_name_resolves_to_the_variant_this_file_names_it_with() {
    for name in WINDOWS_NAMES {
        let zone: WindowsTimezone = name
            .parse()
            .unwrap_or_else(|_| panic!("`{name}` is a name this crate parses"));
        assert_eq!(windows_name(zone), name);
        assert_eq!(zone.name(), name);
    }
}

/// The name a payload spells each `WindowsTimezone` with.
///
/// The match names every variant and has no wildcard arm, so a `windows-timezones`
/// upgrade that adds a zone fails this file to compile until the new zone is named
/// here — and the sweep beside it then proves that name resolves against the
/// bundled database.
#[cfg(feature = "windows-zones")]
#[expect(
    clippy::too_many_lines,
    reason = "one arm per zone is the exhaustive coverage this match exists to state"
)]
fn windows_name(zone: WindowsTimezone) -> &'static str {
    match zone {
        WindowsTimezone::DatelineStandardTime => "Dateline Standard Time",
        WindowsTimezone::UtcMinus11 => "UTC-11",
        WindowsTimezone::AleutianStandardTime => "Aleutian Standard Time",
        WindowsTimezone::HawaiianStandardTime => "Hawaiian Standard Time",
        WindowsTimezone::MarquesasStandardTime => "Marquesas Standard Time",
        WindowsTimezone::AlaskanStandardTime => "Alaskan Standard Time",
        WindowsTimezone::UtcMinus09 => "UTC-09",
        WindowsTimezone::PacificStandardTimeMexico => "Pacific Standard Time (Mexico)",
        WindowsTimezone::UtcMinus08 => "UTC-08",
        WindowsTimezone::PacificStandardTime => "Pacific Standard Time",
        WindowsTimezone::UsMountainStandardTime => "US Mountain Standard Time",
        WindowsTimezone::MountainStandardTimeMexico => "Mountain Standard Time (Mexico)",
        WindowsTimezone::MountainStandardTime => "Mountain Standard Time",
        WindowsTimezone::YukonStandardTime => "Yukon Standard Time",
        WindowsTimezone::CentralAmericaStandardTime => "Central America Standard Time",
        WindowsTimezone::CentralStandardTime => "Central Standard Time",
        WindowsTimezone::EasterIslandStandardTime => "Easter Island Standard Time",
        WindowsTimezone::CentralStandardTimeMexico => "Central Standard Time (Mexico)",
        WindowsTimezone::CanadaCentralStandardTime => "Canada Central Standard Time",
        WindowsTimezone::SaPacificStandardTime => "SA Pacific Standard Time",
        WindowsTimezone::EasternStandardTimeMexico => "Eastern Standard Time (Mexico)",
        WindowsTimezone::EasternStandardTime => "Eastern Standard Time",
        WindowsTimezone::HaitiStandardTime => "Haiti Standard Time",
        WindowsTimezone::CubaStandardTime => "Cuba Standard Time",
        WindowsTimezone::UsEasternStandardTime => "US Eastern Standard Time",
        WindowsTimezone::TurksAndCaicosStandardTime => "Turks And Caicos Standard Time",
        WindowsTimezone::ParaguayStandardTime => "Paraguay Standard Time",
        WindowsTimezone::AtlanticStandardTime => "Atlantic Standard Time",
        WindowsTimezone::VenezuelaStandardTime => "Venezuela Standard Time",
        WindowsTimezone::CentralBrazilianStandardTime => "Central Brazilian Standard Time",
        WindowsTimezone::SaWesternStandardTime => "SA Western Standard Time",
        WindowsTimezone::PacificSaStandardTime => "Pacific SA Standard Time",
        WindowsTimezone::NewfoundlandStandardTime => "Newfoundland Standard Time",
        WindowsTimezone::TocantinsStandardTime => "Tocantins Standard Time",
        WindowsTimezone::ESouthAmericaStandardTime => "E. South America Standard Time",
        WindowsTimezone::SaEasternStandardTime => "SA Eastern Standard Time",
        WindowsTimezone::ArgentinaStandardTime => "Argentina Standard Time",
        WindowsTimezone::GreenlandStandardTime => "Greenland Standard Time",
        WindowsTimezone::MontevideoStandardTime => "Montevideo Standard Time",
        WindowsTimezone::MagallanesStandardTime => "Magallanes Standard Time",
        WindowsTimezone::SaintPierreStandardTime => "Saint Pierre Standard Time",
        WindowsTimezone::BahiaStandardTime => "Bahia Standard Time",
        WindowsTimezone::UtcMinus02 => "UTC-02",
        WindowsTimezone::AzoresStandardTime => "Azores Standard Time",
        WindowsTimezone::CapeVerdeStandardTime => "Cape Verde Standard Time",
        WindowsTimezone::Utc => "UTC",
        WindowsTimezone::GmtStandardTime => "GMT Standard Time",
        WindowsTimezone::GreenwichStandardTime => "Greenwich Standard Time",
        WindowsTimezone::SaoTomeStandardTime => "Sao Tome Standard Time",
        WindowsTimezone::MoroccoStandardTime => "Morocco Standard Time",
        WindowsTimezone::WEuropeStandardTime => "W. Europe Standard Time",
        WindowsTimezone::CentralEuropeStandardTime => "Central Europe Standard Time",
        WindowsTimezone::RomanceStandardTime => "Romance Standard Time",
        WindowsTimezone::CentralEuropeanStandardTime => "Central European Standard Time",
        WindowsTimezone::WCentralAfricaStandardTime => "W. Central Africa Standard Time",
        WindowsTimezone::JordanStandardTime => "Jordan Standard Time",
        WindowsTimezone::GtbStandardTime => "GTB Standard Time",
        WindowsTimezone::MiddleEastStandardTime => "Middle East Standard Time",
        WindowsTimezone::EgyptStandardTime => "Egypt Standard Time",
        WindowsTimezone::EEuropeStandardTime => "E. Europe Standard Time",
        WindowsTimezone::SyriaStandardTime => "Syria Standard Time",
        WindowsTimezone::WestBankStandardTime => "West Bank Standard Time",
        WindowsTimezone::SouthAfricaStandardTime => "South Africa Standard Time",
        WindowsTimezone::FleStandardTime => "FLE Standard Time",
        WindowsTimezone::IsraelStandardTime => "Israel Standard Time",
        WindowsTimezone::SouthSudanStandardTime => "South Sudan Standard Time",
        WindowsTimezone::KaliningradStandardTime => "Kaliningrad Standard Time",
        WindowsTimezone::SudanStandardTime => "Sudan Standard Time",
        WindowsTimezone::LibyaStandardTime => "Libya Standard Time",
        WindowsTimezone::NamibiaStandardTime => "Namibia Standard Time",
        WindowsTimezone::ArabicStandardTime => "Arabic Standard Time",
        WindowsTimezone::TurkeyStandardTime => "Turkey Standard Time",
        WindowsTimezone::ArabStandardTime => "Arab Standard Time",
        WindowsTimezone::BelarusStandardTime => "Belarus Standard Time",
        WindowsTimezone::RussianStandardTime => "Russian Standard Time",
        WindowsTimezone::EAfricaStandardTime => "E. Africa Standard Time",
        WindowsTimezone::IranStandardTime => "Iran Standard Time",
        WindowsTimezone::ArabianStandardTime => "Arabian Standard Time",
        WindowsTimezone::AstrakhanStandardTime => "Astrakhan Standard Time",
        WindowsTimezone::AzerbaijanStandardTime => "Azerbaijan Standard Time",
        WindowsTimezone::RussiaTimeZone3 => "Russia Time Zone 3",
        WindowsTimezone::MauritiusStandardTime => "Mauritius Standard Time",
        WindowsTimezone::SaratovStandardTime => "Saratov Standard Time",
        WindowsTimezone::GeorgianStandardTime => "Georgian Standard Time",
        WindowsTimezone::VolgogradStandardTime => "Volgograd Standard Time",
        WindowsTimezone::CaucasusStandardTime => "Caucasus Standard Time",
        WindowsTimezone::AfghanistanStandardTime => "Afghanistan Standard Time",
        WindowsTimezone::WestAsiaStandardTime => "West Asia Standard Time",
        WindowsTimezone::EkaterinburgStandardTime => "Ekaterinburg Standard Time",
        WindowsTimezone::PakistanStandardTime => "Pakistan Standard Time",
        WindowsTimezone::QyzylordaStandardTime => "Qyzylorda Standard Time",
        WindowsTimezone::IndiaStandardTime => "India Standard Time",
        WindowsTimezone::SriLankaStandardTime => "Sri Lanka Standard Time",
        WindowsTimezone::NepalStandardTime => "Nepal Standard Time",
        WindowsTimezone::CentralAsiaStandardTime => "Central Asia Standard Time",
        WindowsTimezone::BangladeshStandardTime => "Bangladesh Standard Time",
        WindowsTimezone::OmskStandardTime => "Omsk Standard Time",
        WindowsTimezone::MyanmarStandardTime => "Myanmar Standard Time",
        WindowsTimezone::SeAsiaStandardTime => "SE Asia Standard Time",
        WindowsTimezone::AltaiStandardTime => "Altai Standard Time",
        WindowsTimezone::WMongoliaStandardTime => "W. Mongolia Standard Time",
        WindowsTimezone::NorthAsiaStandardTime => "North Asia Standard Time",
        WindowsTimezone::NCentralAsiaStandardTime => "N. Central Asia Standard Time",
        WindowsTimezone::TomskStandardTime => "Tomsk Standard Time",
        WindowsTimezone::ChinaStandardTime => "China Standard Time",
        WindowsTimezone::NorthAsiaEastStandardTime => "North Asia East Standard Time",
        WindowsTimezone::SingaporeStandardTime => "Singapore Standard Time",
        WindowsTimezone::WAustraliaStandardTime => "W. Australia Standard Time",
        WindowsTimezone::TaipeiStandardTime => "Taipei Standard Time",
        WindowsTimezone::UlaanbaatarStandardTime => "Ulaanbaatar Standard Time",
        WindowsTimezone::AusCentralWStandardTime => "Aus Central W. Standard Time",
        WindowsTimezone::TransbaikalStandardTime => "Transbaikal Standard Time",
        WindowsTimezone::TokyoStandardTime => "Tokyo Standard Time",
        WindowsTimezone::NorthKoreaStandardTime => "North Korea Standard Time",
        WindowsTimezone::KoreaStandardTime => "Korea Standard Time",
        WindowsTimezone::YakutskStandardTime => "Yakutsk Standard Time",
        WindowsTimezone::CenAustraliaStandardTime => "Cen. Australia Standard Time",
        WindowsTimezone::AusCentralStandardTime => "AUS Central Standard Time",
        WindowsTimezone::EAustraliaStandardTime => "E. Australia Standard Time",
        WindowsTimezone::AusEasternStandardTime => "AUS Eastern Standard Time",
        WindowsTimezone::WestPacificStandardTime => "West Pacific Standard Time",
        WindowsTimezone::TasmaniaStandardTime => "Tasmania Standard Time",
        WindowsTimezone::VladivostokStandardTime => "Vladivostok Standard Time",
        WindowsTimezone::LordHoweStandardTime => "Lord Howe Standard Time",
        WindowsTimezone::BougainvilleStandardTime => "Bougainville Standard Time",
        WindowsTimezone::RussiaTimeZone10 => "Russia Time Zone 10",
        WindowsTimezone::MagadanStandardTime => "Magadan Standard Time",
        WindowsTimezone::NorfolkStandardTime => "Norfolk Standard Time",
        WindowsTimezone::SakhalinStandardTime => "Sakhalin Standard Time",
        WindowsTimezone::CentralPacificStandardTime => "Central Pacific Standard Time",
        WindowsTimezone::RussiaTimeZone11 => "Russia Time Zone 11",
        WindowsTimezone::NewZealandStandardTime => "New Zealand Standard Time",
        WindowsTimezone::UtcPlus12 => "UTC+12",
        WindowsTimezone::FijiStandardTime => "Fiji Standard Time",
        WindowsTimezone::ChathamIslandsStandardTime => "Chatham Islands Standard Time",
        WindowsTimezone::UtcPlus13 => "UTC+13",
        WindowsTimezone::TongaStandardTime => "Tonga Standard Time",
        WindowsTimezone::SamoaStandardTime => "Samoa Standard Time",
        WindowsTimezone::LineIslandsStandardTime => "Line Islands Standard Time",
    }
}

#[cfg(feature = "windows-zones")]
#[test]
fn every_windows_zone_resolves_against_the_bundled_database() {
    let db = db();
    let mut unresolved = Vec::new();
    for name in WINDOWS_NAMES {
        match resolve_zone(&db, name) {
            Ok(zone) => {
                // A rule built from a payload names this zone, so it must resolve
                // again through the same database the engine reads.
                assert!(db.zone(zone.name()).is_ok(), "{name} -> {}", zone.name());
            }
            Err(err) => unresolved.push(format!("{name}: {err}")),
        }
    }
    assert!(
        unresolved.is_empty(),
        "{} of {} Windows zones do not resolve: {unresolved:#?}",
        unresolved.len(),
        WINDOWS_NAMES.len()
    );
}

#[cfg(not(feature = "windows-zones"))]
#[test]
fn without_the_cldr_mapping_only_iana_and_alias_names_resolve() {
    // IANA names and the alias table are unaffected by compiling the CLDR data out.
    assert_eq!(resolved("UTC"), "UTC");
    assert_eq!(resolved("Etc/UTC"), "Etc/UTC");
    assert_eq!(resolved("America/New_York"), "America/New_York");
    assert_eq!(resolved("Coordinated Universal Time"), "UTC");
    // A Windows name is a CLDR-only name, so it is unresolved rather than guessed.
    let error = resolve_zone(&db(), "Pacific Standard Time").unwrap_err();
    assert_eq!(
        error,
        AdapterZoneError::Unresolved {
            name: "Pacific Standard Time".to_owned()
        }
    );
}

#[test]
fn a_mapped_category_takes_the_callers_tag() {
    let map = BTreeMap::from([
        ("Interview".to_owned(), Tag::new("interview").unwrap()),
        ("1:1 Sync!".to_owned(), Tag::new("one-on-one").unwrap()),
    ]);
    assert_eq!(
        category_to_tag("Interview", Some(&map)).unwrap().as_str(),
        "interview"
    );
    assert_eq!(
        category_to_tag("1:1 Sync!", Some(&map)).unwrap().as_str(),
        "one-on-one"
    );
    // A mapped name is used as written, not slugged.
    assert_eq!(
        category_to_tag("Interview", Some(&map)).unwrap().as_str(),
        "interview"
    );
}

#[test]
fn an_unmapped_category_becomes_a_slug() {
    assert_eq!(
        category_to_tag("Client Demo (Q3)!", None).unwrap().as_str(),
        "category/client-demo-q3"
    );
    assert_eq!(
        category_to_tag("Interview", None).unwrap().as_str(),
        "category/interview"
    );
    let map = BTreeMap::from([("Interview".to_owned(), Tag::new("interview").unwrap())]);
    assert_eq!(
        category_to_tag("Team Offsite", Some(&map))
            .unwrap()
            .as_str(),
        "category/team-offsite"
    );
}

#[test]
fn a_category_that_slugs_to_nothing_is_dropped() {
    for empty in ["---", "   ", "🎉", "", "///"] {
        assert_eq!(category_to_tag(empty, None), None, "{empty:?}");
    }
}

#[test]
fn marker_tags_are_split_and_invalid_tokens_are_reported() {
    let mut diagnostics = Vec::<AdapterDiagnostic>::new();
    let tags = parse_marker_tags(
        "team/sales  interview/platform not:a:tag",
        Some("evt_1"),
        &mut diagnostics,
    );
    let names: Vec<&str> = tags.iter().map(Tag::as_str).collect();
    assert_eq!(names, ["team/sales", "interview/platform"]);
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0].code, "marker_tag_invalid");
    assert_eq!(diagnostics[0].event_id.as_deref(), Some("evt_1"));
    assert!(diagnostics[0].message.contains("not:a:tag"));
}

#[test]
fn a_zone_name_is_a_zone_name_whatever_whitespace_it_carries() {
    assert_eq!(
        resolve_zone(&db(), "  America/New_York ").unwrap().name(),
        "America/New_York"
    );
    assert_eq!(ZoneName::new("UTC").unwrap().as_str(), "UTC");
}

#[test]
fn a_hostile_marker_list_caps_its_invalid_token_diagnostics() {
    // A marker value is provider-supplied text, so it must not be able to blow up
    // the findings a caller carries. Sixteen tokens are reported in full and the
    // rest become one summary naming the count.
    let raw = std::iter::repeat_n("not:a:tag", 1_000)
        .collect::<Vec<_>>()
        .join(" ");
    let mut diagnostics = Vec::<AdapterDiagnostic>::new();
    let tags = parse_marker_tags(&raw, Some("evt_1"), &mut diagnostics);
    assert!(tags.is_empty());
    assert_eq!(diagnostics.len(), 17);
    assert!(
        diagnostics[..16]
            .iter()
            .all(|finding| finding.code == "marker_tag_invalid")
    );
    assert!(diagnostics[15].message.contains("not:a:tag"));
    let summary = &diagnostics[16];
    assert_eq!(summary.code, "marker_tag_invalid");
    assert_eq!(summary.event_id.as_deref(), Some("evt_1"));
    assert!(summary.message.contains("984"), "{summary:?}");
    assert!(!summary.message.contains("not:a:tag"), "{summary:?}");

    // A list under the cap is unchanged.
    let mut diagnostics = Vec::new();
    let tags = parse_marker_tags("team/sales not:a:tag", Some("evt_2"), &mut diagnostics);
    assert_eq!(tags.len(), 1);
    assert_eq!(diagnostics.len(), 1);
}

#[test]
fn a_latin_accented_category_folds_to_its_ascii_letters() {
    for (name, slug) in [
        ("Café", "category/cafe"),
        ("Müller", "category/muller"),
        ("Séoul", "category/seoul"),
        ("Réunion", "category/reunion"),
        ("Niño", "category/nino"),
        ("Über", "category/uber"),
    ] {
        assert_eq!(
            category_to_tag(name, None).unwrap().as_str(),
            slug,
            "{name}"
        );
    }
    // A script with no ASCII spelling still has no tag rather than a wrong one.
    assert_eq!(category_to_tag("会議", None), None);
}
