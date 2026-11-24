/**
 * Ambient types for the `.wasm` asset imported by `src/wasm-workerd.ts`.
 *
 * `tsc` does not emit `.d.ts` files for input `.d.ts` files, so this stays
 * compile-time only and never leaks DOM/Worker globals into consumer
 * declarations.
 */

declare namespace WebAssembly {
  interface Module {}
}

declare module "*.wasm" {
  const module: WebAssembly.Module;
  export default module;
}
