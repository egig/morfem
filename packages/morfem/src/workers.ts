// Cloudflare Workers entry: wrangler turns the `.wasm` import into a precompiled WebAssembly.Module
// (Workers forbid compiling wasm from bytes at runtime).
// @ts-ignore - resolved by the bundler
import wasm from "./gen/morfem_wasm_bg.wasm";
import { createMorfem as create, type Morfem } from "./index.js";
import type { MorfemOptions } from "./types.js";
import { openMorfem as open, type Sdk } from "./sdk.js";

export type * from "./types.js";
export type { Morfem };
export { d1Store } from "./stores/d1.js";

export function createMorfem(opts: Omit<MorfemOptions, "wasm">): Promise<Morfem> {
  return create({ ...opts, wasm } as MorfemOptions);
}

export * from "./sdk.js";

/** SDK over a Store, with the wasm preloaded for Workers. */
export function openMorfem(opts: { store: import("./types.js").Store }): Promise<Sdk> {
  return open({ ...opts, wasm } as any);
}
