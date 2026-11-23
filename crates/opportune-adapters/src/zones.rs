//! Zone resolution and category slugging: the two pieces every provider adapter
//! shares.
//!
//! A provider names time zones its own way. Graph carries Windows IDs
//! (`Pacific Standard Time`), Google carries IANA names, and payloads in the wild
//! carry aliases neither provider documents (`Coordinated Universal Time`).
//! [`resolve_zone`] resolves a name in the order the adapter contract fixes:
//! IANA first, because that is what a payload means when it names a zone the
//! database knows; then — with the `windows-zones` feature on — Windows IDs
//! through the CLDR mapping; then a small alias table for the names neither
//! covers. A name no step resolves is an error, never a silent fallback to UTC,
//! because reading an event's wall-clock times in the wrong zone moves it by
//! hours.
//!
//! The returned [`Zone`] carries the database name it resolved to, so a rule built
//! from a payload names a zone the engine can resolve again: a Windows ID becomes
//! the IANA zone behind it.

use std::collections::BTreeMap;

use opportune::tags::Tag;
use opportune::time::{TimeZoneDatabase, Zone};
#[cfg(feature = "windows-zones")]
use windows_timezones::WindowsTimezone;

use crate::types::{AdapterDiagnostic, AdapterZoneError};

/// Names that are neither IANA nor in the CLDR Windows mapping, mapped to the IANA
/// zone they mean.
///
/// The two deprecated Windows IDs are here because CLDR dropped them and real
/// tenants still send them: `Mid-Atlantic Standard Time` was UTC-02:00 with
/// daylight saving, and `Kamchatka Standard Time` was UTC+12 with daylight saving.
/// Neither observes daylight saving under the IANA zones they map to any more, so
/// a historical event in them reads an hour off; a live one reads correctly, and
/// there is no other name for them.
const ALIASES: [(&str, &str); 3] = [
    ("Coordinated Universal Time", "UTC"),
    ("Mid-Atlantic Standard Time", "Etc/GMT+2"),
    ("Kamchatka Standard Time", "Asia/Kamchatka"),
];

/// Resolves a zone name a provider payload carries.
///
/// The order is IANA, then the CLDR Windows mapping (with `windows-zones`), then
/// the alias table. The returned zone is named after the database entry it
/// resolved to, which is what a rule built from the payload must carry.
///
/// ```
/// # #[cfg(feature = "bundled-tzdb")] {
/// use opportune::time::TimeZoneDatabase;
/// use opportune_adapters::zones::resolve_zone;
///
/// let db = TimeZoneDatabase::bundled().unwrap();
/// let zone = resolve_zone(&db, "America/New_York").unwrap();
/// assert_eq!(zone.name(), "America/New_York");
///
/// // The alias resolves to the IANA zone behind it, so the engine can resolve
/// // the same name later.
/// let alias = resolve_zone(&db, "Coordinated Universal Time").unwrap();
/// assert_eq!(alias.name(), "UTC");
///
/// assert_eq!(
///     resolve_zone(&db, "Not A Real Zone").unwrap_err().code(),
///     "zone_unknown",
/// );
/// # }
/// ```
///
/// # Errors
/// Returns [`AdapterZoneError::Unresolved`] when no step resolves the name.
pub fn resolve_zone(db: &TimeZoneDatabase, name: &str) -> Result<Zone, AdapterZoneError> {
    let name = name.trim();
    if let Ok(zone) = db.zone(name) {
        return Ok(zone);
    }
    #[cfg(feature = "windows-zones")]
    if let Ok(windows) = name.parse::<WindowsTimezone>()
        && let Ok(zone) = db.zone(windows.tzdb_id())
    {
        return Ok(zone);
    }
    if let Some((_, iana)) = ALIASES.iter().find(|(alias, _)| *alias == name)
        && let Ok(zone) = db.zone(iana)
    {
        return Ok(zone);
    }
    Err(AdapterZoneError::Unresolved {
        name: name.to_owned(),
    })
}

/// The tag a category name means: the caller's mapped tag when the map holds one,
/// otherwise `category/<slug>`.
///
/// The slug is the name lowercased, with a Latin accent folded to its ASCII letter
/// and every run of the remaining characters outside `a-z 0-9` replaced by `-`, and
/// no leading or trailing `-`: `Client Demo (Q3)!` becomes
/// `category/client-demo-q3`, `Café` becomes `category/cafe`, and `Müller` becomes
/// `category/muller`. A name that slugs to nothing — `---`, `   `, `🎉` — has no
/// tag, because `category/` is not a tag path, and it is dropped.
///
/// ```
/// use std::collections::BTreeMap;
///
/// use opportune::tags::Tag;
/// use opportune_adapters::zones::category_to_tag;
///
/// let map = BTreeMap::from([("Interview".to_owned(), Tag::new("interview").unwrap())]);
/// assert_eq!(
///     category_to_tag("Interview", Some(&map)).unwrap().as_str(),
///     "interview",
/// );
/// assert_eq!(
///     category_to_tag("Client Demo (Q3)!", Some(&map)).unwrap().as_str(),
///     "category/client-demo-q3",
/// );
/// assert_eq!(category_to_tag("🎉", None), None);
/// ```
#[must_use]
pub fn category_to_tag(name: &str, map: Option<&BTreeMap<String, Tag>>) -> Option<Tag> {
    if let Some(tag) = map.and_then(|map| map.get(name)) {
        return Some(tag.clone());
    }
    let slug = slug(name);
    if slug.is_empty() {
        return None;
    }
    Tag::new(&format!("category/{slug}")).ok()
}

/// The stable code for a category name that makes no tag.
#[cfg(feature = "graph")]
const CATEGORY_TAG_UNMAPPABLE: &str = "category_tag_unmappable";

/// The most invalid tokens one marker list reports in full. Beyond it a single
/// summary diagnostic names how many more were omitted, so a hostile value cannot
/// flood the findings a caller carries.
const MAX_MARKER_DIAGNOSTICS: usize = 16;

/// The most categories one event reports in full. Beyond it a single summary
/// diagnostic names how many more were omitted, matching the marker cap so one
/// hostile event cannot flood the findings a caller carries.
#[cfg(feature = "graph")]
const MAX_CATEGORY_DIAGNOSTICS: usize = 16;

/// The marker tag list a provider event carries, split and validated.
///
/// A token that is not a valid tag path is reported in `diagnostics` and left out,
/// rather than failing the event: the event's times are still what the caller
/// needs from it, and the diagnostic names the token to fix. At most 16 invalid
/// tokens are reported individually; the rest are folded into one summary diagnostic.
pub fn parse_marker_tags(
    raw: &str,
    event_id: Option<&str>,
    diagnostics: &mut Vec<AdapterDiagnostic>,
) -> Vec<Tag> {
    let mut tags = Vec::new();
    let mut reported = 0usize;
    let mut omitted = 0usize;
    for token in raw.split_whitespace() {
        match Tag::new(token) {
            Ok(tag) => tags.push(tag),
            Err(err) => {
                if reported < MAX_MARKER_DIAGNOSTICS {
                    diagnostics.push(AdapterDiagnostic {
                        code: "marker_tag_invalid",
                        event_id: event_id.map(str::to_owned),
                        message: format!("the marker tag `{token}` is not a tag path: {err}"),
                    });
                    reported += 1;
                } else {
                    omitted += 1;
                }
            }
        }
    }
    if omitted > 0 {
        diagnostics.push(AdapterDiagnostic {
            code: "marker_tag_invalid",
            event_id: event_id.map(str::to_owned),
            message: format!(
                "{omitted} more marker token(s) are not tag paths and were omitted from the \
                 findings"
            ),
        });
    }
    tags
}

/// The tags a category list yields, and a diagnostic for each real name that
/// yields none.
///
/// A category the caller's map names takes the mapped tag; an unmapped name
/// becomes `category/<slug>`; a name that slugs to nothing makes no tag at all. A
/// non-blank name that makes no tag is reported in `diagnostics`, so a caller whose
/// category is written in a script with no ASCII spelling can see why its
/// `category/...` policy never matched. A blank name is not a category and is
/// dropped silently. At most 16 names are reported individually; the rest are
/// folded into one summary diagnostic.
#[cfg(feature = "graph")]
pub(crate) fn categories_to_tags(
    categories: &[String],
    map: Option<&BTreeMap<String, Tag>>,
    event_id: Option<&str>,
    diagnostics: &mut Vec<AdapterDiagnostic>,
) -> Vec<Tag> {
    let mut tags = Vec::with_capacity(categories.len());
    let mut reported = 0usize;
    let mut omitted = 0usize;
    for category in categories {
        if let Some(tag) = category_to_tag(category, map) {
            tags.push(tag);
        } else if !category.trim().is_empty() {
            if reported < MAX_CATEGORY_DIAGNOSTICS {
                diagnostics.push(AdapterDiagnostic {
                    code: CATEGORY_TAG_UNMAPPABLE,
                    event_id: event_id.map(str::to_owned),
                    message: format!(
                        "the category `{category}` maps to no tag: the category map has no entry \
                         for it and its name has no ASCII slug, so no `category/...` tag was added"
                    ),
                });
                reported += 1;
            } else {
                omitted += 1;
            }
        }
    }
    if omitted > 0 {
        diagnostics.push(AdapterDiagnostic {
            code: CATEGORY_TAG_UNMAPPABLE,
            event_id: event_id.map(str::to_owned),
            message: format!(
                "{omitted} more categories map to no tag and were omitted from the findings"
            ),
        });
    }
    tags
}

/// The ASCII slug a category name reduces to: lowercased, Latin accents folded to
/// their ASCII letter, with runs of the remaining characters outside `a-z 0-9`
/// collapsed to one `-` and no `-` at either end.
fn slug(name: &str) -> String {
    let mut slug = String::with_capacity(name.len());
    for character in name.chars() {
        let character = character.to_lowercase().next().unwrap_or(character);
        if character.is_ascii_alphanumeric() {
            slug.push(character);
        } else if let Some(folded) = fold(character) {
            slug.push(folded);
        } else if !slug.is_empty() && !slug.ends_with('-') {
            slug.push('-');
        }
    }
    while slug.ends_with('-') {
        slug.pop();
    }
    slug
}

/// The ASCII letter a Latin letter stands for when its accent can be dropped
/// without changing the word.
///
/// This is a small hand-written fold rather than a normalization pass: the crate
/// carries no Unicode data table, and calendar categories are Latin at worst. A
/// letter outside the table stays a separator, so a script with no ASCII spelling
/// still slugs to nothing rather than to a wrong ASCII word.
fn fold(character: char) -> Option<char> {
    let folded = match character {
        'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' | 'ā' | 'ă' | 'ą' => 'a',
        'ç' | 'ć' | 'č' => 'c',
        'ď' | 'đ' => 'd',
        'è' | 'é' | 'ê' | 'ë' | 'ē' | 'ĕ' | 'ė' | 'ę' | 'ě' => 'e',
        'ĝ' | 'ğ' | 'ġ' | 'ģ' => 'g',
        'ĥ' | 'ħ' => 'h',
        'ì' | 'í' | 'î' | 'ï' | 'ĩ' | 'ī' | 'ĭ' | 'į' | 'ı' => 'i',
        'ĵ' => 'j',
        'ķ' => 'k',
        'ĺ' | 'ļ' | 'ľ' | 'ł' => 'l',
        'ñ' | 'ń' | 'ņ' | 'ň' => 'n',
        'ò' | 'ó' | 'ô' | 'õ' | 'ö' | 'ø' | 'ō' | 'ŏ' | 'ő' => 'o',
        'ŕ' | 'ŗ' | 'ř' => 'r',
        'ś' | 'ŝ' | 'ş' | 'š' => 's',
        'ţ' | 'ť' | 'ŧ' => 't',
        'ù' | 'ú' | 'û' | 'ü' | 'ũ' | 'ū' | 'ŭ' | 'ů' | 'ű' | 'ų' => 'u',
        'ŵ' => 'w',
        'ý' | 'ÿ' | 'ŷ' => 'y',
        'ź' | 'ż' | 'ž' => 'z',
        _ => return None,
    };
    Some(folded)
}

#[cfg(test)]
mod tests {
    use super::slug;

    #[test]
    fn a_slug_is_ascii_lowercase_with_single_separators() {
        assert_eq!(slug("Interview"), "interview");
        assert_eq!(slug("Client Demo (Q3)!"), "client-demo-q3");
        assert_eq!(slug("1:1 Sync!"), "1-1-sync");
        assert_eq!(slug("  spaced  out  "), "spaced-out");
        assert_eq!(slug("---"), "");
        assert_eq!(slug("   "), "");
        assert_eq!(slug("🎉"), "");
        assert_eq!(slug("a.b_c"), "a-b-c");
    }

    #[test]
    fn a_slug_folds_latin_accents_to_their_ascii_letter() {
        assert_eq!(slug("Café"), "cafe");
        assert_eq!(slug("Müller"), "muller");
        assert_eq!(slug("Séoul"), "seoul");
        assert_eq!(slug("Réunion"), "reunion");
        assert_eq!(slug("Niño"), "nino");
        assert_eq!(slug("Über"), "uber");
        assert_eq!(slug("Łódź"), "lodz");
        // A script with no ASCII spelling still slugs to nothing rather than to a
        // wrong ASCII word.
        assert_eq!(slug("会議"), "");
    }
}
