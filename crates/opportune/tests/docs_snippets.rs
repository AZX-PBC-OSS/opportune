//! The Rust code the documentation shows, compiled and run against this crate.
//!
//! Each snippet is a file under `tests/snippets/` that is also, byte for byte, a
//! code block in the adopter documentation. The modules below compile them and
//! run the `main` each one is written as, asserting the scheduling outcome it
//! promises, and [`the_documentation_carries_the_compiled_snippets`] proves the
//! documentation still quotes them. Neither half can move without the other
//! failing.
//!
//! Adding a recipe is one tuple below and its block in `docs/recipes.md`: the
//! tuple generates both the module that runs it and its entry in the fence list,
//! so a snippet cannot be compiled without its documentation being checked.
//!
//! The provider-adapter recipe needs `opportune-adapters`, which this crate does
//! not depend on; its snippet is compiled by that crate's own
//! `tests/docs_snippets.rs`. Its page block is still checked here, by
//! [`PROVIDER_RECIPE`].
//!
//! The page also has to be reachable: `mkdocs build` reports a docs page that no
//! `nav` entry names as information rather than as a warning, so
//! `mkdocs build --strict` stays green with the recipes page orphaned in the site
//! tree. [`the_recipes_page_is_in_the_navigation`] is the check that notices.

#![cfg(feature = "bundled-tzdb")]

/// A snippet module: the file compiled, and a test that runs the `main` it is
/// written as.
macro_rules! snippet {
    ($module:ident, $file:literal) => {
        mod $module {
            include!($file);

            #[test]
            fn runs() -> Result<(), Box<dyn std::error::Error>> {
                main()
            }
        }
    };
}

/// The recipes this crate compiles, each becoming a [`snippet!`] module and an
/// entry in the fence list the documentation test compares against.
macro_rules! recipes {
    ($(($module:ident, $file:literal)),* $(,)?) => {
        $( snippet!($module, $file); )*
        const COMPILED_RECIPES: &[&str] = &[$(include_str!($file)),*];
    };
}

snippet!(api_free_functions, "snippets/api_free_functions.rs");
snippet!(quickstart_index, "snippets/quickstart_index.rs");
snippet!(quickstart_readme, "snippets/quickstart_readme.rs");

recipes![
    (recipe_personal_link, "snippets/recipe_personal_link.rs"),
    (recipe_panel_caps, "snippets/recipe_panel_caps.rs"),
    (
        recipe_rest_horizon_notice,
        "snippets/recipe_rest_horizon_notice.rs"
    ),
    (
        recipe_conference_booth,
        "snippets/recipe_conference_booth.rs"
    ),
    (recipe_travel_reserve, "snippets/recipe_travel_reserve.rs"),
    (recipe_layered_policy, "snippets/recipe_layered_policy.rs"),
    (recipe_team_room, "snippets/recipe_team_room.rs"),
    (
        recipe_panel_reschedule,
        "snippets/recipe_panel_reschedule.rs"
    ),
];

/// The provider recipe, compiled by `opportune-adapters`' own docs test.
const PROVIDER_RECIPE: &str = include_str!("snippets/recipe_provider_adapters.rs");

const MKDOCS: &str = include_str!("../../../mkdocs.yml");
const README: &str = include_str!("../../../README.md");
const INDEX: &str = include_str!("../../../docs/index.md");
const API: &str = include_str!("../../../docs/api.md");
const RECIPES: &str = include_str!("../../../docs/recipes.md");

/// The `(label, page)` pairs `mkdocs.yml`'s `nav` names, in the order the site
/// lists them. The block runs from `nav:` to the next top-level key.
fn nav_entries(config: &str) -> Vec<(&str, &str)> {
    let mut entries = Vec::new();
    let mut in_nav = false;
    for line in config.lines() {
        if !line.starts_with(' ') {
            in_nav = line.trim_end() == "nav:";
            continue;
        }
        let Some(rest) = line.trim_start().strip_prefix("- ") else {
            continue;
        };
        if let (true, Some((label, page))) = (in_nav, rest.split_once(':')) {
            entries.push((label.trim(), page.trim()));
        }
    }
    entries
}

#[test]
fn the_recipes_page_is_in_the_navigation() {
    // The label is the site's own wording and may change; being in `nav` at all is
    // what makes the page reachable, and it is the part no build checks.
    assert!(
        nav_entries(MKDOCS)
            .iter()
            .any(|(_, page)| *page == "recipes.md"),
        "docs/recipes.md is not an entry in `mkdocs.yml`'s `nav`: `mkdocs build` \
         reports an unreferenced page as information, so `mkdocs build --strict` stays \
         green while the page drops out of the site navigation"
    );
}

/// Every `## ` section of `document`, with the language of each code fence it opens.
/// A fence's closing marker carries no language, so only openeners are recorded.
fn sections(document: &str) -> Vec<(&str, Vec<&str>)> {
    let mut sections: Vec<(&str, Vec<&str>)> = Vec::new();
    let mut current: Option<(&str, Vec<&str>)> = None;
    for line in document.lines() {
        if let Some(heading) = line.strip_prefix("## ") {
            sections.extend(current.take());
            current = Some((heading, Vec::new()));
            continue;
        }
        let Some((_, languages)) = current.as_mut() else {
            continue;
        };
        if let Some(language) = line.strip_prefix("```").filter(|it| !it.is_empty()) {
            languages.push(language);
        }
    }
    sections.extend(current);
    sections
}

#[test]
fn every_recipe_is_worked_in_all_three_languages() {
    // The page's contract is three surfaces per scenario. Only the Rust blocks are
    // compiled, so without this a deleted or never-written Python or TypeScript half
    // leaves every gate green while the page stops being the cross-surface reference it
    // is supposed to be. A section is a recipe when it works the scenario in Rust — the
    // fence test above fixes which nine those are — so a later section that carries some
    // other kind of block is not held to the three-surface rule.
    let recipes: Vec<(&str, Vec<&str>)> = sections(RECIPES)
        .into_iter()
        .filter(|(_, languages)| languages.contains(&"rust"))
        .collect();
    assert_eq!(
        recipes.len(),
        9,
        "docs/recipes.md works {} recipes, not the nine the page promises",
        recipes.len()
    );
    for (heading, languages) in recipes {
        for language in ["rust", "python", "ts"] {
            assert!(
                languages.contains(&language),
                "the `{heading}` recipe has no `{language}` block, so the page is not \
                 worked in Rust, Python and TypeScript for that scenario"
            );
        }
    }
}

/// The count `page` states immediately before `phrase` — `nine` in "nine worked
/// scenarios" — as a number.
///
/// Panics when the page no longer carries the phrase or puts something other than a
/// count word in front of it: a page that stops stating its count is one this gate can
/// no longer hold, which is a reason to look at the page rather than to pass.
fn stated_count(page: &str, phrase: &str) -> usize {
    const WORDS: &[&str] = &[
        "zero", "one", "two", "three", "four", "five", "six", "seven", "eight", "nine", "ten",
        "eleven", "twelve",
    ];
    let at = page
        .find(phrase)
        .unwrap_or_else(|| panic!("the page no longer says `{phrase}`, so it states no count"));
    let word = page[..at]
        .split_whitespace()
        .next_back()
        .expect("a phrase is preceded by the count that introduces it")
        .to_lowercase();
    WORDS
        .iter()
        .position(|candidate| *candidate == word)
        .unwrap_or_else(|| panic!("`{word}` in front of `{phrase}` is not a count word"))
}

#[test]
fn the_pages_that_introduce_the_recipes_state_the_number_the_page_works() {
    // Three pages tell a reader how many recipes there are: the recipes page itself,
    // the docs landing page's "Where to go next" entry and the repository README. Only
    // the recipes page is generated from the sections, so the other two are prose that
    // nothing recomputes — adding a recipe moved the page's own count while the two
    // that send the reader there kept the old one, and no gate noticed. Each stated
    // count is compared with the sections this page actually works, so all three move
    // together or this fails.
    let worked = sections(RECIPES)
        .into_iter()
        .filter(|(_, languages)| languages.contains(&"rust"))
        .count();
    for (page, phrase) in [
        (RECIPES, " worked scenarios"),
        (INDEX, " worked scenarios"),
        (README, " scenarios worked through"),
    ] {
        assert_eq!(
            stated_count(page, phrase),
            worked,
            "the page says a different number of recipes in front of `{phrase}` than \
             docs/recipes.md works ({worked}), so a reader is sent to a page that is not \
             the size they were told"
        );
    }
}

/// Every Rust code fence in `document`, with the text between the fences.
fn rust_fences(document: &str) -> Vec<&str> {
    let mut fences = Vec::new();
    let mut rest = document;
    while let Some(start) = rest.find("```rust\n") {
        let body = &rest[start + "```rust\n".len()..];
        let end = body
            .find("```")
            .expect("every Rust code fence in the documentation is closed");
        fences.push(&body[..end]);
        rest = &body[end + 3..];
    }
    fences
}

#[test]
fn the_documentation_carries_the_compiled_snippets() {
    assert_eq!(
        rust_fences(README),
        [include_str!("snippets/quickstart_readme.rs")],
        "README.md's one Rust block is the quickstart this test runs"
    );
    assert_eq!(
        rust_fences(INDEX),
        [include_str!("snippets/quickstart_index.rs")],
        "docs/index.md's one Rust block is the quickstart this test runs"
    );
    assert!(
        rust_fences(API).contains(&include_str!("snippets/api_free_functions.rs")),
        "docs/api.md still quotes the free-function example this test runs"
    );

    let expected: Vec<&str> = COMPILED_RECIPES
        .iter()
        .copied()
        .chain([PROVIDER_RECIPE])
        .collect();
    assert_eq!(
        rust_fences(RECIPES),
        expected,
        "every Rust block in docs/recipes.md is one of the compiled recipe snippets, \
         in the order the page presents them"
    );
}
