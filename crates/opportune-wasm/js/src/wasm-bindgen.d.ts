/**
 * Ambient types for the `.wasm` asset imported by `src/wasm-workerd.ts`.
 *
 * `tsc` does not emit `.d.ts` files for input `.d.ts` files, so this stays
 * compile-time only and never leaks DOM/Worker globals into consumer
 * declarations.
 *
 * The interface names the members `lib.dom` would give it. With an empty body,
 * `WebAssembly.Module` is `{}` — a type every non-nullish value satisfies — so a
 * union such as `SyncInitInput` collapses to `unknown` and accepts a `string` the
 * runtime would reject. The two members are what make the type the compiled-module
 * value it stands for.
 */

declare namespace WebAssembly {
  interface Module {
    readonly imports: readonly unknown[];
    readonly exports: readonly unknown[];
  }
}

declare module "*.wasm" {
  const module: WebAssembly.Module;
  export default module;
}
