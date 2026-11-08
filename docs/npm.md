---
title: npm package
---

# npm package

`@egig/base` runs the database inside your own process. The engine ships as prebuilt WebAssembly, so you do not clone the repo or install Rust. You supply the SQLite or D1 handle. The HTTP API is the same as the server's: see [HTTP API](api).

```sh
npm i @egig/base
```

## Node (22.5 or newer)

```ts
import { DatabaseSync } from "node:sqlite";
import { createBase } from "@egig/base";
import { sqliteStore } from "@egig/base/stores/sqlite";

const db = await createBase({
  store: sqliteStore(new DatabaseSync("app.db")),
  keys: [{ key: "devkey-12345", access: "admin" }],
});

const headers = { authorization: "Bearer devkey-12345", "content-type": "application/json" };
await db.fetch(new Request("http://x/notes", { method: "PUT", headers }));
const res = await db.fetch(
  new Request("http://x/notes/a", { method: "PUT", headers, body: JSON.stringify({ title: "hi" }) }),
);
console.log(await res.json()); // { ok: true, id: "a", rev: "1" }
```

The host in the URL is ignored; only the path and query matter.

`sqliteStore` accepts a `node:sqlite` `DatabaseSync`, a `better-sqlite3` database, or a `bun:sqlite` `Database`.

## SDK (no HTTP)

`openBase` skips Request/Response and gives typed methods. The path is TS, then SDK, then the wasm engine, then SQLite. The SDK is trusted: there are no keys and everything is admin.

```ts
import { openFile } from "@egig/base/node";

const base = await openFile("app.db"); // sets WAL and a 5 s busy timeout
const notes = await base.createDb<{ title: string; n: number }>("notes");

const { rev } = await notes.put("a", { title: "hi", n: 1 });   // create-only: throws a conflict if "a" exists
await notes.put("a", { title: "hello", n: 2 }, { rev });       // update from rev
await notes.get("a");                                          // { _id, _rev, title, n } or null
await notes.update("a", (d) => ({ ...d!, n: d!.n + 1 }));      // read-modify-write, retries on conflict
await notes.createIndex(["n"]);
const { docs } = await notes.find({ selector: { n: { $gte: 1 } }, sort: [{ n: "desc" }], limit: 10 });
await notes.delete("a", (await notes.get("a"))!._rev);
base.close();
```

With your own store (Workers, Bun, better-sqlite3) use `openBase({ store })` from `@egig/base` or `@egig/base/workers`.

| method | notes |
|---|---|
| `base.createDb(name)` | returns the handle; fine if it exists |
| `base.db(name)` | handle, does not create |
| `base.dropDb(name)`, `base.listDbs()` | |
| `base.txn({ reads?, guards?, ops })` | see [Transactions](transactions) |
| `base.raw` | the `fetch`/`handle` object, auth off. Do not expose it publicly |
| `db.get(id)` | doc or `null` |
| `db.put(id, doc, { rev? })` | no `rev` = create only |
| `db.post(doc)` | generated id |
| `db.delete(id, rev)` | |
| `db.update(id, fn, retries = 5)` | return `undefined` from `fn` to skip the write |
| `db.find({ selector, sort?, limit?, skip? })` | see [Queries](queries) |
| `db.allDocs({ startkey?, endkey?, limit?, skip?, descending?, includeDocs? })` | |
| `db.changes({ since?, limit?, includeDocs? })` | |
| `db.info()` | |
| `db.createIndex(fields, name?)`, `db.indexes()`, `db.dropIndex(name)` | |

Failures throw `BaseError` with `status`, `error`, `reason` (the same as the [HTTP errors](api)) and `isConflict`. `get` returns `null` instead of throwing on 404.

## Serve over HTTP

`db.fetch` has the standard `(Request) => Promise<Response>` shape.

```ts
Bun.serve({ port: 5984, fetch: (req) => db.fetch(req) });
Deno.serve({ port: 5984 }, (req) => db.fetch(req));
```

On Node, use any Request/Response adapter, for example `@hono/node-server`.

## Cloudflare Workers (D1)

```ts
import { createBase, d1Store } from "@egig/base/workers";

let db: ReturnType<typeof createBase> | undefined;

export default {
  fetch(req: Request, env: { DB: D1Database; BASE_KEYS: string }) {
    db ??= createBase({ store: d1Store(env.DB), keys: JSON.parse(env.BASE_KEYS) });
    return db.then((b) => b.fetch(req));
  },
};
```

Use the `@egig/base/workers` entry. It imports the `.wasm` as a module, which wrangler bundles; Workers cannot compile wasm from bytes at runtime. The schema is applied on first use.

Set the keys with `npx wrangler secret put BASE_KEYS`. Add a D1 binding named `DB` in `wrangler.toml`.

## Browser

Pass any `Store` (for example one over sql.js or wa-sqlite). The wasm loads from `import.meta.url`. If your bundler moves the file, pass it yourself:

```ts
import wasmUrl from "@egig/base/base.wasm?url"; // Vite
await createBase({ store, noAuth: true, wasm: wasmUrl });
```

## Options

| option | meaning |
|---|---|
| `store` | required. Where data lives. |
| `keys` | array of `{ key, access, dbs? }`. Same meaning as `BASE_KEYS`: see [Auth and limits](auth-and-limits). |
| `noAuth` | `true` makes everyone admin. Local development only. Pass `keys` or `noAuth`; `createBase` throws if you pass neither. |
| `wasm` | `WebAssembly.Module`, bytes, `Response`, or URL. Default: the file shipped in the package. |

`createBase` returns `{ fetch(Request), handle({ method, path, query?, bearer?, body? }) }`. `path` is percent-encoded. `handle` returns `{ status, contentType, body: Uint8Array }`.

## Custom store

```ts
interface Store {
  batch(stmts: { sql: string; params: (string | number | null)[] }[]): Promise<(string | number | null)[][][]>;
}
```

Rules:

- Run all statements in order inside one atomic transaction.
- Return, per statement, its rows as arrays of column values (`[]` if none).
- If any statement fails, roll back the whole batch and throw. Do not change the error message: a failed transaction guard is detected from it and becomes a 409.

## Notes

- One process, one writer. If several processes share one SQLite file, each batch still runs in `BEGIN IMMEDIATE`, but set a busy timeout on your handle.
- `createBase` applies the schema on every call. It is idempotent, so restarts are safe.
