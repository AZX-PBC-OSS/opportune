/// <reference lib="esnext.disposable" preserve="true" />
/**
 * The engine a caller builds once, and the free functions over a shared one.
 *
 * `Engine` holds the compiled module's engine — the bundled time zone database and
 * the request limits — and answers the four queries twice: as the generated
 * document type, and as the JSON text the module already wrote. A server route
 * that returns a `Response` body takes the text (`findAvailabilityJson`) and hands
 * it on, so the answer is never parsed and written back out.
 *
 * The module's engine owns a native resource, so its release is explicit:
 * {@link Engine.free} hands the resource back, `[Symbol.dispose]` makes
 * `using engine = new Engine()` release it when the block ends, and a call on a
 * freed engine raises `engine_disposed` rather than reaching a null pointer. Only
 * the shared engine the free functions answer from is never released: it lives as
 * long as the module does.
 *
 * The reference directive above is what a consumer needs rather than this package:
 * `[Symbol.dispose]` is declared in the `esnext.disposable` library, and a project
 * that compiles with `"lib": ["ES2024"]` and `"skipLibCheck": false` reads this
 * file's declaration in its own program, where this package's `tsconfig.json` counts
 * for nothing. `preserve="true"` is what carries the directive into the emitted
 * declaration — without it the compiler drops a library reference it has already
 * loaded, and the consumer is left with `[Symbol.dispose]` and no `Symbol.dispose`.
 * The test suite compiles the packed artifact under that configuration.
 */

// `#wasm` is the private conditional import `package.json#imports` resolves: one
// accessor for every runtime, with the loader behind it chosen by condition rather than
// in this file. The `node` and `default` conditions select the loader for Node and Bun,
// while `workerd` selects the static precompiled Worker loader and `browser`/`worker`
// select the explicit web initializer. Nothing at or
// below this file names a runtime, so the API above the seam is one implementation rather
// than one per deployment target.
import { wasmModule } from "#wasm";

import { OpportuneError, configurationRejection, reason, rejection } from "./errors.js";
import { asPlainRecord, asRecord, documentText, optionsOf, readDocument, typeName, utf8Text } from "./json.js";
import { translated, type WasmEngine } from "./wasm.js";
import type {
  Diagnostics,
  Limits,
  Proposal,
  RangesQuery,
  Request,
  RequiredRanges,
  Response,
  SlotCheck,
} from "./generated/types.js";

/**
 * The mark every `Engine` carries, and the handle it owns.
 *
 * Both come from the global symbol registry rather than from this module, so an
 * `Engine` built by a *second copy* of this package in one `node_modules` tree — a
 * monorepo with two versions, a pnpm store that did not dedupe — is one this copy's
 * `binding` recognises and can answer from. A module-local symbol or a `WeakMap`
 * would make `engine instanceof Engine` false across the two, and the `{ engine }`
 * option an adapter takes would be refused as `invalid_type` while it already *is*
 * an engine: `Symbol.for` is the one identity two copies can agree on.
 *
 * Neither symbol is exported, so neither is part of the package's API; they are
 * this file's seam to itself, and the handle is an implementation detail a caller
 * reaches only through `free`, `[Symbol.dispose]` and the query methods.
 */
const ENGINE_BRAND = Symbol.for("opportune.engine");
const HANDLE = Symbol.for("opportune.engine.handle");

/** The shape {@link isEngine} reads: the brand and, while it is open, the handle. */
interface Branded {
  readonly [ENGINE_BRAND]?: true;
  [HANDLE]?: WasmEngine;
}

/**
 * Whether `value` is an `Engine`, including one built by another copy of the
 * package.
 *
 * Two arms, because there are two ways to be an engine. `instanceof` is this copy's
 * class, and is what an object built here answers. The brand is what answers for an
 * `Engine` a *second* copy built — two installations do not share a constructor, so
 * `instanceof` is false across them, while `Symbol.for` is one symbol per process.
 * A duck-typed object that merely looks like an engine is neither, and is refused.
 *
 * This is the package's own seam, like {@link binding}: it is exported for the
 * adapter submodules and is not part of the package's entry point.
 */
export function isEngine(value: unknown): value is Engine {
  if (value instanceof Engine) {
    return true;
  }
  return (
    typeof value === "object" &&
    value !== null &&
    (value as Branded)[ENGINE_BRAND] === true
  );
}

/**
 * An engine: the bundled time zone database and the limits, built once and
 * shared.
 *
 * ```ts
 * using engine = new Engine();
 * const response = engine.findAvailability(request);
 * ```
 */
export class Engine {
  /** Marks this instance as an engine; see {@link isEngine}. Not part of the API. */
  readonly [ENGINE_BRAND]: true = true;

  /** The compiled module's engine, while this one is open. Not part of the API. */
  [HANDLE]?: WasmEngine;

  /**
   * Builds an engine over the bundled database.
   *
   * @param limits the request-size limits, or `undefined` or `null` for the engine's
   *   own defaults — start from {@link defaultLimits} and change what you need. A
   *   `null` is the "no argument" a caller reaches for from a variable that holds
   *   nothing, so it reads as the defaults rather than as a document the module
   *   would refuse.
   * @throws OpportuneError with code `invalid_limits` when the limits are not a
   *   document the engine reads, naming the field at fault in `.path` and chaining
   *   the underlying wire rejection as `.cause`.
   */
  constructor(limits?: Limits | null) {
    try {
      const limitsJson =
        limits == null ? null : documentText(limitsDocument(limits), "the limits");
      const { WasmEngine } = wasmModule();
      this[HANDLE] = translated(() => new WasmEngine(limitsJson));
    } catch (thrown) {
      if (thrown instanceof OpportuneError) {
        throw configurationRejection(thrown);
      }
      throw thrown;
    }
  }

  /** The version of the time zone database, as a response's `meta.tzdb` reports it. */
  get tzdbVersion(): string {
    return translated(() => binding(this).tzdb_version());
  }

  /**
   * Every candidate start the request's calendars, rules and caps allow.
   *
   * @throws OpportuneError when the request is rejected. Its findings land in
   *   `.errors`, each with the pointer of the field it is about.
   */
  findAvailability(request: Request): Response {
    return readDocument<Response>(this.findAvailabilityJson(request));
  }

  /**
   * The same answer as {@link Engine.findAvailability}, as its JSON text.
   *
   * @throws OpportuneError when the request is rejected.
   */
  findAvailabilityJson(request: Request): string {
    const requestJson = documentText(request, "the request");
    return translated(() => binding(this).find_availability_json(requestJson));
  }

  /**
   * Judges one proposed booking against the same request.
   *
   * @throws OpportuneError when either document is rejected. A proposal that does
   *   not fit the request — an unknown part, the wrong number of terms or hosts —
   *   names the proposal field at fault in `.path`.
   */
  checkSlot(request: Request, proposal: Proposal): SlotCheck {
    return readDocument<SlotCheck>(this.checkSlotJson(request, proposal));
  }

  /**
   * The same judgement as {@link Engine.checkSlot}, as its JSON text.
   *
   * @throws OpportuneError when either document is rejected.
   */
  checkSlotJson(request: Request, proposal: Proposal): string {
    const requestJson = documentText(request, "the request");
    const proposalJson = documentText(proposal, "the proposal");
    return translated(() => binding(this).check_slot_json(requestJson, proposalJson));
  }

  /**
   * The request's static errors and warnings.
   *
   * @throws OpportuneError when the request is rejected.
   */
  validate(request: Request): Diagnostics {
    return readDocument<Diagnostics>(this.validateJson(request));
  }

  /**
   * The same diagnostics as {@link Engine.validate}, as their JSON text.
   *
   * @throws OpportuneError when the request is rejected.
   */
  validateJson(request: Request): string {
    const requestJson = documentText(request, "the request");
    return translated(() => binding(this).validate_json(requestJson));
  }

  /**
   * The calendar and occurrence data a request needs, before fetching any.
   *
   * `query` may also be the whole request the ranges are for: a `RangesQuery` is a
   * `Request` without the six fields the engine does not read here, so the request
   * a caller already holds is one argument rather than a second document to build by
   * hand. Fields outside those six are still sent, and still rejected.
   *
   * @throws OpportuneError when the query is rejected.
   */
  requiredRanges(query: RangesQuery | Request): RequiredRanges {
    return readDocument<RequiredRanges>(this.requiredRangesJson(query));
  }

  /**
   * The same ranges as {@link Engine.requiredRanges}, as their JSON text.
   *
   * @throws OpportuneError when the query is rejected.
   */
  requiredRangesJson(query: RangesQuery | Request): string {
    return translated(() => binding(this).required_ranges_json(rangesQueryText(query)));
  }

  /**
   * Releases the engine's native resource.
   *
   * Idempotent, and the engine makes no further call after it: the handle is
   * removed even when the release itself throws, so a second {@link Engine.free}
   * releases nothing, and a query that follows raises `engine_disposed` rather than
   * reaching a freed pointer. `[Symbol.dispose]` calls this, so a `using` block needs
   * no `try`/`finally`.
   *
   * The module also registers a finalizer for a handle that is dropped without
   * this, but finalizer timing is the collector's to decide: a long-lived process
   * that builds engines per request has to release them or its native memory grows
   * with every request.
   */
  free(): void {
    const handle = this[HANDLE];
    if (handle === undefined) {
      return;
    }
    try {
      translated(() => handle.free());
    } finally {
      // Removed in a `finally` so a release that throws still leaves the engine
      // disposed: the handle is destroyed either way, and a second call must not
      // reach it.
      delete this[HANDLE];
    }
  }

  /** Releases the engine's native resource; see {@link Engine.free}. */
  [Symbol.dispose](): void {
    this.free();
  }
}

/**
 * `limits` as the plain document the module reads.
 *
 * Every field of a limits document is optional, so `{}` is a document the wire reads as
 * "every field default". A `Map`, a `Set` or an instance of a class carries no own
 * enumerable member, so `JSON.stringify` writes exactly that and the engine would run
 * with none of the limits the caller asked for — a bound tightened to one participant
 * silently becomes the engine's own maximum. Only a plain mapping is written, the shape
 * {@link limitsWith} and the adapters' options already require, so the mistake leaves as
 * the rejection a caller can act on rather than as a silent widening of every bound.
 *
 * @throws OpportuneError with code `invalid_type`; `Engine` reclassifies it as
 *   `invalid_limits`, the code its own `@throws` states.
 */
function limitsDocument(limits: Limits): Limits {
  if (asPlainRecord(limits) === undefined) {
    throw rejection(
      "invalid_type",
      `the limits are a document of fields, not a value of type ${typeName(limits)}`,
    );
  }
  return limits;
}

/**
 * The binding handle `engine` answers with, or the shared engine's when it is
 * `undefined` or `null`.
 *
 * This is the package's own seam between the typed surface and the adapter
 * submodules, which convert payloads over one engine's database. It is not
 * exported from the package's entry point.
 *
 * @throws OpportuneError with code `invalid_type` when `engine` is neither absent
 *   nor an {@link Engine}, and `engine_disposed` when an {@link Engine} has been
 *   freed.
 */
export function binding(engine?: Engine): WasmEngine {
  if (engine !== undefined && engine !== null && !isEngine(engine)) {
    throw rejection(
      "invalid_type",
      `\`engine\` must be an Engine instance, not \`${reason(engine)}\``,
    );
  }
  const target = engine ?? shared();
  const handle = target[HANDLE];
  if (handle === undefined) {
    throw rejection(
      "engine_disposed",
      "the engine was freed, so it can answer nothing else: build another one, or reuse one that is still open",
    );
  }
  return handle;
}

/**
 * The six `Request` fields a `RangesQuery` has no place for.
 *
 * The wire layer reads a query with `deny_unknown_fields`, so a request handed
 * straight to {@link Engine.requiredRanges} is rejected for carrying fields the
 * ranges do not depend on. These are the whole of that difference: what to look for,
 * when, and on whose calendars is all a query reads, and everything else in a request
 * is about the search that follows.
 */
const REQUEST_ONLY: readonly string[] = [
  "occurrences",
  "exclude_event_keys",
  "calendars",
  "weights",
  "limit",
  "explain",
];

/**
 * The `RangesQuery` inside `query`, as the JSON text the module reads.
 *
 * Only the six fields above are dropped. Anything else the caller added is sent as
 * it is, so a misspelled field is still `unknown_field` at the wire rather than a
 * silently narrower query: a request with a typo has to fail the same way a query
 * with one does.
 *
 * Each member is defined on the copy rather than assigned to it, which is what keeps
 * a `__proto__` member — the one name `JSON.parse` gives a body a caller sent — a
 * member the wire sees and rejects, instead of the copy's own prototype, which no
 * text documents.
 */
function rangesQueryText(query: RangesQuery | Request): string {
  let projected: unknown;
  try {
    const stated = asRecord(query);
    projected = stated === undefined
      // Not a document at all: written as it stands, so the wire parser answers for
      // it with the code it uses for every other unreadable document.
      ? query
      : Object.fromEntries(
          Object.entries(stated).filter(([key]) => !REQUEST_ONLY.includes(key)),
        );
  } catch (cause) {
    // Reading a member is part of writing the document. Keep a throwing getter or
    // proxy on the caller's side of the boundary, as JSON.stringify's own read does.
    throw rejection(
      "invalid_json",
      `the query has no JSON spelling: ${reason(cause)}`,
      cause,
    );
  }
  return documentText(projected, "the query");
}

/** The engine the module-level functions share, built on first use. */
let sharedEngine: Engine | undefined;

function shared(): Engine {
  sharedEngine ??= new Engine();
  return sharedEngine;
}

/**
 * `options` as the options object an adapter reads, rejecting an {@link Engine}
 * passed in its place.
 *
 * The adapters take one options object rather than positional optional arguments.
 * An `Engine` *is* an object, so without this check passing an `Engine` directly
 * where the options belong would read as an options object with every field absent
 * and silently answer from the shared engine. `{ engine }` is where one belongs.
 * The check is {@link isEngine}, so an `Engine` from a second copy of the package is
 * recognised rather than read as an options object that happens to carry no fields.
 *
 * This is the package's own seam between the typed surface and the adapter
 * submodules, like {@link binding}; it is not exported from the package's entry
 * point.
 *
 * @param what the call the options belong to, named in the message.
 * @throws OpportuneError with code `invalid_type` when `options` is not an object,
 *   or is an {@link Engine}.
 */
export function adapterOptions<T extends object>(options: T, what: string): T {
  if (isEngine(options)) {
    throw rejection(
      "invalid_type",
      `\`${what}\` takes an object of optional fields here, not an Engine: pass it as \`{ engine }\``,
    );
  }
  return optionsOf(options, what);
}

/**
 * The engine's own default request-size limits.
 *
 * These are the limits an engine built with no `limits` argument applies, read from
 * the crate's own defaults rather than restated here, so
 * `new Engine(limitsWith(...))` starts from exactly what it replaces.
 */
export function defaultLimits(): Limits {
  return readDocument<Limits>(translated(() => wasmModule().default_limits_json()));
}

/**
 * The default limits with `overrides` applied.
 *
 * ```ts
 * const engine = new Engine(limitsWith({ max_search_window_days: 120 }));
 * ```
 *
 * The override fields are not validated here: the engine reads the document when it
 * is built, so a value it cannot mean is rejected there with the wire parser's own
 * code — `invalid_type`, `invalid_value`, `missing_field` or `unknown_field` — and
 * the pointer of the field. Passing a non-object `overrides` argument is rejected
 * here with `invalid_type`.
 */
export function limitsWith(overrides: Partial<Limits>): Limits {
  const applied = optionsOf(overrides, "limitsWith");
  const defined = Object.fromEntries(
    Object.entries(applied).filter(([, value]) => value !== undefined),
  );
  return { ...defaultLimits(), ...defined };
}

/** Every candidate start the shared engine allows for `request`. */
export function findAvailability(request: Request): Response {
  return shared().findAvailability(request);
}

/** The same answer as {@link findAvailability}, as its JSON text. */
export function findAvailabilityJson(request: Request): string {
  return shared().findAvailabilityJson(request);
}

/** Judges one proposed booking against `request`, from the shared engine. */
export function checkSlot(request: Request, proposal: Proposal): SlotCheck {
  return shared().checkSlot(request, proposal);
}

/** The same judgement as {@link checkSlot}, as its JSON text. */
export function checkSlotJson(request: Request, proposal: Proposal): string {
  return shared().checkSlotJson(request, proposal);
}

/** The request's static errors and warnings, from the shared engine. */
export function validate(request: Request): Diagnostics {
  return shared().validate(request);
}

/** The same diagnostics as {@link validate}, as their JSON text. */
export function validateJson(request: Request): string {
  return shared().validateJson(request);
}

/** The data `query` needs, from the shared engine; `query` may be a whole request. */
export function requiredRanges(query: RangesQuery | Request): RequiredRanges {
  return shared().requiredRanges(query);
}

/** The same ranges as {@link requiredRanges}, as their JSON text. */
export function requiredRangesJson(query: RangesQuery | Request): string {
  return shared().requiredRangesJson(query);
}

/** The largest schema version the WASM `u32` parameter can represent. */
const MAX_SCHEMA_VERSION = 0xffff_ffff;

/**
 * `fromVersion` as an integer within the WASM `u32` range.
 *
 * WebAssembly coerces `i32` parameters with JS `ToInt32`, so without this guard
 * `1.5`, `2**32 + 1`, `"1"` and `true` would silently run as schema version `1`,
 * and `-1` would report version `4294967295`.
 */
function schemaVersion(fromVersion: number): number {
  if (typeof fromVersion !== "number" || !Number.isInteger(fromVersion)) {
    throw rejection(
      "invalid_type",
      `the schema version must be an integer, not \`${reason(fromVersion)}\``,
    );
  }
  if (fromVersion < 0 || fromVersion > MAX_SCHEMA_VERSION) {
    throw rejection(
      "invalid_value",
      `the schema version must be between 0 and ${MAX_SCHEMA_VERSION}, not ${fromVersion}`,
    );
  }
  return fromVersion;
}

/**
 * Rewrites a stored rule as the current schema version's canonical JSON.
 *
 * @param rule the rule as the JSON text it was stored as.
 * @param fromVersion the schema version it was written with.
 * @throws OpportuneError with code `unsupported_schema_version` when this build
 *   does not read that version, `invalid_json` when the text is not a rule of it or
 *   holds no UTF-8 spelling, `invalid_type` when `rule` is not a string or
 *   `fromVersion` is not an integer, and `invalid_value` when `fromVersion` is
 *   outside `0..=4294967295`.
 */
export function upgradeRule(rule: string, fromVersion: number): string {
  const ruleJson = utf8Text(rule, "the rule");
  const version = schemaVersion(fromVersion);
  return translated(() => wasmModule().upgrade_rule_json(ruleJson, version));
}

/**
 * Rewrites a stored working calendar as the current schema version's canonical
 * JSON.
 *
 * @throws OpportuneError for the same reasons {@link upgradeRule} raises.
 */
export function upgradeCalendar(calendar: string, fromVersion: number): string {
  const calendarJson = utf8Text(calendar, "the calendar");
  const version = schemaVersion(fromVersion);
  return translated(() => wasmModule().upgrade_calendar_json(calendarJson, version));
}

/**
 * Rewrites a stored goal as the current schema version's canonical JSON.
 *
 * @throws OpportuneError for the same reasons {@link upgradeRule} raises.
 */
export function upgradeGoal(goal: string, fromVersion: number): string {
  const goalJson = utf8Text(goal, "the goal");
  const version = schemaVersion(fromVersion);
  return translated(() => wasmModule().upgrade_goal_json(goalJson, version));
}
