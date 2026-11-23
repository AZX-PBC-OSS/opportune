/**
 * The one error a JavaScript caller catches.
 *
 * Every failure this package reports is an {@link OpportuneError}: the engine's
 * own rejection, read from the structured payload the WASM module throws, and the
 * rejections this layer makes before a call is made — a document with no JSON
 * spelling, a non-document argument with no UTF-8 spelling. A caller's handler for
 * {@link OpportuneError} therefore never sees a bare `TypeError` from
 * `JSON.stringify`, a raw string from the module, or a `RuntimeError` from a
 * failed instance.
 *
 * The {@link OpportuneError.code} values fall in three groups:
 *
 * - the wire layer's own codes, such as `invalid_json`, `unknown_field` and
 *   `invalid_value`, with the RFC 6901 {@link OpportuneError.path} of the field at
 *   fault and the findings a rejected request produced. This layer raises
 *   `invalid_json` itself for a document it cannot write — a circular reference, a
 *   `BigInt`, a number JSON spells as `null` — or for one a caller holds as text
 *   that has no UTF-8 spelling;
 * - `argument_not_utf8` and `invalid_type`, for a non-document argument — a zone
 *   name, a rule ID — that holds an unpaired surrogate, or that is not a string at
 *   all: `argument_not_utf8` is the code the Python binding answers for the
 *   surrogate, and a value that is not a string has nothing to send;
 * - the binding's own failures, which are not rejections of the caller's
 *   document: `wasm_unavailable` when the compiled module is missing or stale,
 *   `wasm_error` when a call into it throws something that is not its documented
 *   payload, `engine_disposed` when a freed engine is used again, and
 *   `unsupported_runtime` when a browser or Edge bundle resolved this package.
 *
 * {@link OpportuneError.code} is an open union of those codes and the engine's own
 * diagnostic codes, so a handler can name one without a cast and a code from a later
 * release still arrives.
 */

import type { Diagnostic } from "./generated/types.js";
import type { OpportuneErrorCode } from "./vocabulary.js";

/**
 * A rejected request, document or engine configuration.
 *
 * The cause of a translated failure — the payload string the module threw, or the
 * JavaScript exception this layer caught — is kept as {@link Error.cause}, so a
 * log or a debugger still sees it.
 */
export class OpportuneError extends Error {
  /** The stable, machine-readable code, such as `invalid_json`. */
  readonly code: OpportuneErrorCode;

  /**
   * A sentence naming the value that was rejected.
   *
   * The text may be reworded; match on {@link code} instead.
   */
  override readonly message: string;

  /**
   * The RFC 6901 JSON pointer of the field at fault, or `null` when the failure
   * is about the document as a whole.
   */
  readonly path: string | null;

  /**
   * The findings a rejected request carries, each with its own pointer.
   *
   * Empty when the failure is about an argument or the engine rather than about a
   * request's fields.
   */
  readonly errors: Diagnostic[];

  constructor(
    code: OpportuneErrorCode,
    message: string,
    options: {
      path?: string | null;
      errors?: Diagnostic[];
      cause?: unknown;
    } = {},
  ) {
    super(message, { cause: options.cause });
    this.name = "OpportuneError";
    this.code = code;
    this.message = message;
    this.path = options.path ?? null;
    this.errors = options.errors ?? [];
  }
}

/**
 * A rejection of an argument this package could not read.
 *
 * The engine raises {@link OpportuneError} for everything it refuses, so the
 * checks this layer makes itself raise the same class rather than an exception no
 * handler catches. There is no `path` and no finding list: the argument is at
 * fault, not a field of a document.
 */
export function rejection(code: OpportuneErrorCode, message: string, cause?: unknown): OpportuneError {
  return new OpportuneError(code, message, { cause });
}

/**
 * Error codes naming a server or deployment failure rather than invalid caller input.
 *
 * Each names something no field of a caller's request can fix: engine configuration
 * or limits, the WASM module or runtime, JSON serialization, argument checking, or a
 * provider payload the application fetched itself and could not read. A Graph or
 * Google payload is the server's own integration input — the route asked the
 * provider for it — so a document the adapter cannot read is the deployment's
 * failure rather than the visitor's, which is the line the Python binding draws too.
 */
const SERVER_ERROR_CODES: ReadonlySet<string> = new Set([
  "argument_not_utf8",
  "engine_disposed",
  "invalid_argument",
  "invalid_limits",
  "payload_invalid",
  "serialization_failed",
  "tzdb_bundled_unavailable",
  "tzdb_directory_unusable",
  "tzdb_not_configured",
  "tzdb_version_missing",
  "tzdb_version_without_directory",
  "unsupported_runtime",
  "unsupported_schema_version",
  "wasm_error",
  "wasm_unavailable",
]);

/**
 * The HTTP status code an application should answer `error` with: `422` or `500`.
 *
 * Returns `422` when the caller supplied a document the engine or wire parser
 * refused (a request, proposal, or query), and `500` when the failure belongs to
 * the server or deployment (bad engine limits, unavailable WASM module, disposed
 * engine, a provider payload the application fetched and could not read, or
 * package argument errors).
 */
export function httpStatusForError(error: OpportuneError): 422 | 500 {
  if (error == null || typeof error !== "object" || typeof error.code !== "string") {
    return 500;
  }
  return SERVER_ERROR_CODES.has(error.code) ? 500 : 422;
}

/**
 * Reclassifies a limits failure raised during `Engine` construction as
 * `invalid_limits` so HTTP error middleware answers it with `500` rather than `422`.
 */
export function configurationRejection(failure: OpportuneError): OpportuneError {
  if (httpStatusForError(failure) !== 422) {
    return failure;
  }
  return new OpportuneError(
    "invalid_limits",
    `the limits are not a document the engine reads: ${failure.message}`,
    { path: failure.path, errors: failure.errors, cause: failure },
  );
}

/** `cause` as a sentence, for a message that names what failed. */
export function reason(cause: unknown): string {
  if (cause instanceof Error) {
    return cause.message;
  }
  try {
    return String(cause);
  } catch {
    return Object.prototype.toString.call(cause);
  }
}
