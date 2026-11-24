/**
 * Ambient type for the `.wasm` asset imported by `src/wasm-workerd.ts`.
 *
 * The bundler owns what a static `.wasm` import produces. `object` is the honest local
 * constraint: the generated glue's `SyncInitInput` accepts it, and no DOM/WebWorker
 * `WebAssembly` namespace is needed on this Node-targeted typecheck. This input `.d.ts`
 * is not emitted, so the declaration never leaks into a consumer's program.
 */

declare module "*.wasm" {
  const module: object;
  export default module;
}
