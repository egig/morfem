import init, { Base as WasmBase } from "./gen/base_wasm.js";
import type { BaseOptions, RawRequest, RawResponse, Store } from "./types.js";

export type * from "./types.js";

export interface Base {
  /** Serve a standard Request. Plug into `Bun.serve`, `Deno.serve`, a Worker `fetch`, or call directly. */
  fetch(req: Request): Promise<Response>;
  /** Lower level: framework-free request in, response out. */
  handle(req: RawRequest): Promise<RawResponse>;
}

let loaded: Promise<unknown> | undefined;

async function defaultWasm(): Promise<unknown> {
  const url = new URL("./gen/base_wasm_bg.wasm", import.meta.url);
  if (url.protocol === "file:" && typeof (globalThis as any).process?.versions?.node === "string" && !("Deno" in globalThis) && !("Bun" in globalThis)) {
    // Node's fetch cannot read file: URLs.
    const spec = "node:fs/promises";
    const { readFile } = (await import(/* @vite-ignore */ spec)) as { readFile(u: URL): Promise<Uint8Array> };
    return readFile(url);
  }
  return url;
}

function load(wasm: BaseOptions["wasm"]): Promise<unknown> {
  // One instantiation per module instance. An explicit `wasm` wins on first call.
  loaded ??= Promise.resolve(wasm ?? defaultWasm()).then((module_or_path) => init({ module_or_path: module_or_path as any }));
  return loaded;
}

/**
 * Create a database. Applies the schema (idempotent) before returning.
 *
 * ```ts
 * const db = await createBase({ store: sqliteStore(new DatabaseSync("app.db")), keys: [{ key: "devkey-12345", access: "admin" }] });
 * const res = await db.fetch(new Request("http://x/notes", { method: "PUT", headers: { authorization: "Bearer devkey-12345" } }));
 * ```
 */
export async function createBase(opts: BaseOptions): Promise<Base> {
  if (!opts.noAuth && !opts.keys?.length) {
    throw new Error("createBase: pass `keys` or `noAuth: true` (without either, every request is 401)");
  }
  await load(opts.wasm);
  const store: Store = opts.store;
  const batch = async (json: string): Promise<string> => JSON.stringify(await store.batch(JSON.parse(json)));
  const core = new WasmBase(batch, opts.noAuth ? undefined : JSON.stringify(opts.keys), !!opts.noAuth);
  await core.init();

  const handle = async (r: RawRequest): Promise<RawResponse> =>
    (await core.handle(r.method, r.path, r.query ?? "", r.bearer ?? undefined, r.body ?? new Uint8Array())) as RawResponse;

  return {
    handle,
    async fetch(req) {
      const url = new URL(req.url);
      const auth = req.headers.get("authorization");
      const m = auth && /^Bearer\s+(.+)$/i.exec(auth);
      const out = await handle({
        method: req.method,
        path: url.pathname,
        query: url.search.slice(1),
        bearer: m ? m[1] : null,
        body: new Uint8Array(await req.arrayBuffer()),
      });
      return new Response(out.body as BodyInit, { status: out.status, headers: { "content-type": out.contentType } });
    },
  };
}

export { openBase, BaseError, Collection } from "./sdk.js";
export type * from "./sdk.js";
