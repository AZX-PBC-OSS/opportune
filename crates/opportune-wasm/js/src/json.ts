/**
 * Text at the package boundary: documents going to the module and documents
 * coming back.
 *
 * The engine reads and writes JSON documents, so each call has to turn a
 * TypeScript value into text and the answer back into a value. Both directions
 * have a failure the caller must see as an {@link OpportuneError}: a value with no
 * JSON spelling (a circular object, a `BigInt`, a function) and an answer that is
 * not the document the generated types describe.
 *
 * A caller may also hold a document as text already — a provider payload an HTTP
 * client did not parse, a rule read out of storage — and that text crosses the
 * boundary as it is, so {@link utf8Text} reads it before the module does.
 */

import { OpportuneError, reason, rejection } from "./errors.js";

/**
 * `value` as a JSON object, or `undefined` when it is any other JSON value.
 */
export function asRecord(value: unknown): Record<string, unknown> | undefined {
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    return undefined;
  }
  // Narrowed by the guard above: an object that is neither `null` nor an array is
  // a JSON object, whose members are all `unknown` until a caller narrows them.
  return value as Record<string, unknown>;
}

/**
 * `value` as a plain JSON object — a mapping of keys — or `undefined` when it is
 * anything else.
 *
 * A mapping is an object literal, one read back from a document, or one built with
 * `Object.create(null)` to keep a caller's own `__proto__` key as a key. The test is
 * the shape of the prototype chain rather than identity with this realm's
 * `Object.prototype`: a browser iframe or a Node `vm` context has a different
 * `Object.prototype`, and its object literals are mappings too, which `JSON.stringify`
 * writes exactly as it writes this realm's. Every other object has a prototype of its
 * own — a `Date`, a `RegExp`, an instance of a class — and none of its own enumerable
 * members are the keys a caller meant, so a value read as one is a mapping of no keys
 * rather than the mistake it is.
 */
export function asPlainRecord(value: unknown): Record<string, unknown> | undefined {
  const record = asRecord(value);
  if (record === undefined) {
    return undefined;
  }
  const prototype = Object.getPrototypeOf(record);
  if (prototype === null) {
    return record;
  }
  // A plain object's prototype is some realm's `Object.prototype`, whose own prototype
  // is `null`. A class instance's prototype chain is one level deeper, so it is still
  // refused.
  return Object.getPrototypeOf(prototype) === null ? record : undefined;
}

/**
 * `value` as the noun a message calls it: `null`, `list`, `map`, `string`, `date`.
 *
 * `typeof null` is `"object"` and an array is an object too, so both are named as the
 * caller wrote them. An instance is named by its class, which is the one fact about it
 * a caller recognises: `object` tells a caller nothing about the `Date` it passed where
 * a mapping belonged.
 */
export function typeName(value: unknown): string {
  if (value === null) {
    return "null";
  }
  if (Array.isArray(value)) {
    return "list";
  }
  if (typeof value === "object") {
    const prototype: { constructor?: { name?: unknown } } | null = Object.getPrototypeOf(value);
    const name = prototype?.constructor?.name;
    return typeof name === "string" && name !== "" ? name.toLowerCase() : "object";
  }
  return typeof value;
}

/**
 * The JSON text `value` is written as.
 *
 * A number JSON cannot spell is refused rather than written: `JSON.stringify`
 * turns `Infinity`, `-Infinity` and `NaN` into `null`, so a request carrying
 * `limit: Infinity` would reach the engine as `limit: null` — the default, an
 * unbounded search — and the engine would answer a document the caller never
 * wrote. The wire layer reads no non-finite number either, so the rejection is
 * this layer's to make.
 *
 * @param what the document being written, named in the message when it has no
 *   spelling — a request, a proposal, a limits document.
 * @throws OpportuneError with code `invalid_json` when `value` holds something
 *   JSON cannot write, such as a circular reference, a `BigInt` or `NaN`.
 */
export function documentText(value: unknown, what = "the document"): string {
  let text: string | undefined;
  try {
    text = JSON.stringify(value, finite(what));
  } catch (cause) {
    if (cause instanceof OpportuneError) {
      // The replacer's own rejection: it names the member the number sits under,
      // which the message below could only report as "no JSON spelling".
      throw cause;
    }
    throw rejection(
      "invalid_json",
      `${what} has no JSON spelling: ${reason(cause)}`,
      cause,
    );
  }
  if (text === undefined) {
    throw rejection("invalid_json", `${what} has no JSON spelling: JSON writes no value for it`);
  }
  return text;
}

/**
 * The replacer that refuses a number JSON writes as `null`.
 *
 * `JSON.stringify`'s own spelling of a non-finite number is `null`, which the
 * wire layer reads as "absent" for a field that is optional and as a value it
 * rejects for one that is not — either way not the number the caller wrote.
 * `key` is the member the number sits under, so the message names it; the
 * document as a whole arrives under the empty key.
 */
function finite(what: string): (this: unknown, key: string, value: unknown) => unknown {
  return function replacer(this: unknown, key: string, value: unknown): unknown {
    if (typeof value === "number" && !Number.isFinite(value)) {
      const field = key === "" ? "" : ` at \`${key}\``;
      throw rejection(
        "invalid_json",
        `${what} holds ${String(value)}${field}, which JSON writes as \`null\` rather than as ` +
          "the number the caller wrote; send a finite number or leave the field out",
      );
    }
    return value;
  };
}

/**
 * `value`, or the rejection a selector the wire writes as no selector raises.
 *
 * A selector member has to survive `JSON.stringify` as something the wire reads: `true`,
 * `false`, `null`, or an object with at least one member. Three spellings are not one, and
 * each is silent — the field reaches the engine as if the caller had left it out, so a
 * rule's `who` becomes its own `{ role: "host" }` default and its `when` every booking:
 *
 * - a function or a symbol, which `JSON.stringify` drops, so the member is gone;
 * - `{}`, which a `Map`, a `Set`, a `RegExp` or an empty plain object is written as,
 *   because none of them carries a member the wire can read;
 * - `null` or no value at all, which an invalid `Date` or an object whose `toJSON` answers
 *   that way is written as — `who: null` is the wire's default, not a selector.
 *
 * The spelling decides, not the type or its keys: a class instance whose own fields the
 * wire can read is those members and stays a selector, `new Boolean(true)` spells `true`,
 * and an object with a `toJSON` is whatever that returns. Every other malformed selector
 * survives as itself — a string, a number, an array, or the `null` a dropped list member
 * becomes — so the engine rejects it at the field the caller wrote, with its pointer.
 *
 * @param value the selector the caller supplied.
 * @param what the argument being read, named in the message — `who`, `when`, `selector`.
 * @throws OpportuneError with code `invalid_type` when `value` is a function or a symbol,
 *   or an object whose JSON spelling is `{}`, `null` or no value.
 */
export function assertCarryable(value: unknown, what: string): void {
  if (typeof value === "function" || typeof value === "symbol") {
    throw rejection(
      "invalid_type",
      `\`${what}\` is a selector the wire can carry, not a value of type ${typeName(value)}`,
    );
  }
  if (typeof value !== "object" || value === null) {
    return;
  }
  let spelled: string | undefined;
  try {
    spelled = JSON.stringify(value);
  } catch {
    // A value with no JSON spelling at all is the document writer's to report, at the
    // field it sits under: this check is about a selector the wire writes as none, not
    // about a document that cannot be written.
    return;
  }
  if (spelled === "{}" || spelled === "null" || spelled === undefined) {
    throw rejection(
      "invalid_type",
      `\`${what}\` is a selector the wire can carry, not a value of type ${typeName(value)} ` +
        `whose JSON spelling is \`${spelled ?? "nothing"}\`: the engine would read the ` +
        "field's own default instead",
    );
  }
}

/**
 * The JSON text of a provider payload, which a caller may hold as text or as the
 * parsed body an HTTP client gave it.
 *
 * The object form is `object` rather than `Record<string, unknown>` so the official
 * SDK types cross it unchanged: `googleapis`' and `@microsoft/microsoft-graph-types`'
 * interfaces declare their fields and no index signature, and an interface without one
 * is not assignable to a string-keyed record. What arrives is written as JSON as it
 * stands, so a value that is not the provider's document is the wire layer's to
 * reject, with the code and pointer it rejects every other document with.
 *
 * @throws OpportuneError with code `invalid_json` when the payload has no JSON
 *   spelling, or holds text with no UTF-8 spelling.
 */
export function payloadText(payload: string | object): string {
  return typeof payload === "string"
    ? utf8Text(payload, "the payload")
    : documentText(payload, "the payload");
}

/**
 * The document `text` holds, as the generated type that describes it.
 *
 * The module wrote this document from the same schema the generated type is
 * generated from — one Rust type, one JSON Schema, one TypeScript declaration — so
 * the cast is the compile-time form of a guarantee the producer owns. What is
 * checked here is what a caller can be wrong about: the answer is a JSON object,
 * not `null`, an array or a bare value.
 *
 * @throws OpportuneError with code `wasm_error` when the answer is not a document,
 *   which is a failure of the compiled module rather than of the caller's input.
 */
export function readDocument<T extends object>(text: string): T {
  let parsed: unknown;
  try {
    parsed = JSON.parse(text);
  } catch (cause) {
    throw rejection(
      "wasm_error",
      `the module answered with text that is not JSON: ${reason(cause)}`,
      cause,
    );
  }
  if (asRecord(parsed) === undefined) {
    throw rejection("wasm_error", "the module answered with a JSON value that is not a document");
  }
  return parsed as T;
}

/**
 * `value`, or the rejection a non-document argument that is not a string, or has
 * no UTF-8 spelling, raises.
 *
 * A zone name or a rule ID is not a JSON document, so the wire layer never sees
 * it: neither a value that is the wrong type nor an unpaired surrogate in it has
 * an answer the module could give. {@link utf8Text} makes the same check for a
 * document a caller holds as text, and says why it has to be made here.
 *
 * @param what the argument being read, named in the message — a participant zone,
 *   a rule ID.
 * @throws OpportuneError with code `invalid_type` when `value` is not a string,
 *   and `argument_not_utf8` when it is not well-formed UTF-16, which is the code the
 *   Python binding answers for the same argument.
 */
export function argument(value: string, what: string): string {
  return utf8(value, "argument_not_utf8", what);
}

/**
 * `text`, or the rejection a document a caller already holds as text raises when
 * it is not a string, or has no UTF-8 spelling.
 *
 * A stored rule, a payload an HTTP client did not parse: these cross the boundary
 * as the caller's own text rather than through {@link documentText}, so nothing
 * re-spells them. `wasm-bindgen` encodes every string argument with `TextEncoder`,
 * which replaces an unpaired surrogate with U+FFFD instead of failing, so without
 * this check the module reads a document that differs from the one the caller
 * holds and answers for the difference: an event ID that no longer names the
 * provider's event, an outcome keyed by a calendar ID no lookup can reproduce, a
 * rule rewritten under an ID its owner never gave it. By the time the bytes arrive
 * the module cannot tell, so the check is here.
 *
 * The code is `invalid_json` — the wire layer's own, and the one the Python
 * binding answers for the same argument — because the text is a document: it is
 * the document that cannot be read, not an argument beside it.
 *
 * @param what the document being read, named in the message — a payload, a rule.
 * @throws OpportuneError with code `invalid_type` when `text` is not a string,
 *   and `invalid_json` when it holds an unpaired surrogate.
 */
export function utf8Text(text: string, what: string): string {
  return utf8(text, "invalid_json", what);
}

/**
 * `value` as the text the module reads, or the rejection a value that is not a
 * string, or holds an unpaired surrogate, raises with `code`.
 *
 * JavaScript callers are not type-checked, so both are checked here rather than
 * left to throw from `String.prototype.isWellFormed` or from the call itself.
 */
function utf8(value: string, code: string, what: string): string {
  if (typeof value !== "string") {
    throw rejection(
      "invalid_type",
      `${what} is not a string, so there is nothing to send: it is \`${reason(value)}\``,
    );
  }
  if (!value.isWellFormed()) {
    throw rejection(code, `${what} is not UTF-8 text: it holds an unpaired surrogate`);
  }
  return value;
}

/**
 * `options`, or the rejection an options argument that is not a plain mapping raises.
 *
 * Every options object here is all-optional fields, so a caller with nothing to
 * configure leaves the argument out. A value of another type is a mistake this
 * layer has to report itself: `fn(a, b, null)` is the JavaScript idiom for
 * "defaults", and reading `ruleId` off `null` is a raw `TypeError` from this
 * package's own code rather than a rejection its caller can answer.
 *
 * The shape checked is the one {@link asPlainRecord} names, not merely "an object":
 * a `Map`, a `Set`, a `Date` or an instance of a class carries no own enumerable
 * member, so reading one as an options object would answer from every field's
 * default instead. `limitsWith(new Map([["max_search_window_days", 5]]))` would
 * then silently run with the limits it was asked to tighten, and
 * `businessHours("mon-fri", "09:00-17:00", new Map([["zone", "UTC"]]))` would
 * silently read the hours in the participant's zone. Both are the rejection the
 * caller can act on instead.
 *
 * @param what the call the options belong to, named in the message.
 * @throws OpportuneError with code `invalid_type` when `options` is not a plain
 *   object: an array, `null`, a primitive, or an instance whose members are not
 *   its keys.
 */
export function optionsOf<T extends object>(options: T, what: string): T {
  if (asPlainRecord(options) === undefined) {
    throw rejection(
      "invalid_type",
      `${what} options are an object of optional fields, not ${shownOptions(options)}`,
    );
  }
  return options;
}

/**
 * `options` as the message names it: a list, a null-prototype-safe value, or the
 * class of an instance that is an object but not a mapping.
 *
 * An instance is named by its class — `a map`, `a date` — because "an object" is
 * exactly what a caller who passed one believes it is; the name is the one fact
 * that tells them which value the call refused.
 */
function shownOptions(options: unknown): string {
  if (Array.isArray(options)) {
    return "a list";
  }
  if (asRecord(options) !== undefined) {
    const name = typeName(options);
    return name === "object" ? "an instance" : `a ${name}`;
  }
  return `\`${reason(options)}\``;
}
