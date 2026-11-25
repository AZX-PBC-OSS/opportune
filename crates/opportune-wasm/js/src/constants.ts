/**
 * The strings the package states without loading the compiled module.
 *
 * A caller reads these to stamp and to `$expand` its own tags and event keys, so the
 * engine's version and the marker names are facts about the package rather than
 * answers it computes: importing `opportune` for one of them, or
 * `opportune/adapters/graph` for the `$expand` value, must not instantiate the 1.5 MB
 * WebAssembly module. This module imports nothing at all, so it reaches no loader,
 * and the package entry and both adapters read their copy from here.
 *
 * Each constant mirrors the value compiled into the WebAssembly module, and the
 * test suite verifies that all six stay in sync across releases.
 */

/**
 * The version of the `opportune` engine this package was built from, matching
 * `package.json` and the Rust crate version.
 */
export const VERSION: string = "0.2.2"; // x-release-please-version

/** The extended property a caller's own space-separated tags are read from. */
export const MARKER_TAGS: string = "opportune.tags";

/** The extended property carrying a caller's own event key. */
export const MARKER_EVENT_KEY: string = "opportune.event_key";

/** The GUID of opportune's permanent Graph extended-property set. */
export const EXTENDED_PROPERTY_NAMESPACE_GUID: string =
  "a5d0c4e2-7b1f-4c9a-9e3d-2f8b6c1a4e70";

/** The Graph extended-property ID that carries {@link MARKER_TAGS}. */
export const EXTENDED_PROPERTY_TAGS_ID: string =
  `String {${EXTENDED_PROPERTY_NAMESPACE_GUID}} Name ${MARKER_TAGS}`;

/** The Graph extended-property ID that carries {@link MARKER_EVENT_KEY}. */
export const EXTENDED_PROPERTY_EVENT_KEY_ID: string =
  `String {${EXTENDED_PROPERTY_NAMESPACE_GUID}} Name ${MARKER_EVENT_KEY}`;

/** The Graph `$expand` value that fetches both markers with an event. */
export const EXTENDED_PROPERTY_EXPAND: string =
  `singleValueExtendedProperties($filter=id eq '${EXTENDED_PROPERTY_TAGS_ID}' ` +
  `or id eq '${EXTENDED_PROPERTY_EVENT_KEY_ID}')`;
