// Cloudflare Workers entry: wrangler turns the `.wasm` import into a precompiled WebAssembly.Module
// (Workers forbid compiling wasm from bytes at runtime).
// @ts-ignore - resolved by the bundler
import wasm from "./gen/base_wasm_bg.wasm";
import { createBase as create, type Base } from "./index.js";
import type { BaseOptions } from "./types.js";
import { openBase as open, type Sdk } from "./sdk.js";

export type * from "./types.js";
export type { Base };
export { d1Store } from "./stores/d1.js";

export function createBase(opts: Omit<BaseOptions, "wasm">): Promise<Base> {
  return create({ ...opts, wasm } as BaseOptions);
}

export * from "./sdk.js";

/** SDK over a Store, with the wasm preloaded for Workers. */
export function openBase(opts: { store: import("./types.js").Store }): Promise<Sdk> {
  return open({ ...opts, wasm } as any);
}
