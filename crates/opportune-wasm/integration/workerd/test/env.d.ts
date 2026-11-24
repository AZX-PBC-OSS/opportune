/**
 * The Worker entry `wrangler.jsonc` names, so a typed import of it — the shape
 * `cloudflare:workers` exports — resolves to this harness's module rather than `unknown`.
 */
declare namespace Cloudflare {
  interface GlobalProps {
    mainModule: typeof import("../src/worker");
  }
}
