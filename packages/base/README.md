# @egig/base

Schemaless, transactional document database with a CouchDB-shaped HTTP API, as an npm package.
The engine is prebuilt WebAssembly: no Rust toolchain, no clone, no build step.
You bring a SQLite (Node, Bun) or D1 (Cloudflare) handle.

```sh
npm i @egig/base
```

## Node (>= 22.5)

```ts
import { DatabaseSync } from "node:sqlite";
import { createBase } from "@egig/base";
import { sqliteStore } from "@egig/base/stores/sqlite";

const db = await createBase({
  store: sqliteStore(new DatabaseSync("app.db")),
  keys: [{ key: "devkey-12345", access: "admin" }],   // or { noAuth: true } for local dev
});

const auth = { authorization: "Bearer devkey-12345", "content-type": "application/json" };
await db.fetch(new Request("http://x/notes", { method: "PUT", headers: auth }));
await db.fetch(new Request("http://x/notes/a", { method: "PUT", headers: auth, body: '{"title":"hi"}' }));
```

`sqliteStore` also accepts `better-sqlite3` and `bun:sqlite` handles.
`db.fetch` is a standard `(Request) => Promise<Response>`, so it drops into `Bun.serve({ fetch })`, `Deno.serve`, or any Request/Response framework.

## SDK (no HTTP)

```ts
import { openFile } from "@egig/base/node";

const base = await openFile("app.db");
const notes = await base.createDb<{ title: string }>("notes");
await notes.put("a", { title: "hi" });
console.log(await notes.get("a"));
console.log((await notes.find({ selector: { title: "hi" } })).docs);
```

Typed `get/put/post/delete/update/find/allDocs/changes`, indexes, and `base.txn(...)`. No keys: the SDK is trusted. Full list: docs/npm.md.

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

The `/workers` entry imports the `.wasm` file as a module, which wrangler bundles. The schema is applied automatically on first use.

## Browser

Pass any `Store` (for example over sql.js or wa-sqlite). The wasm loads from `import.meta.url`; override with `wasm:` if your bundler moves it.

## Custom store

```ts
interface Store { batch(stmts: { sql: string; params: (string | number | null)[] }[]): Promise<(string | number | null)[][][]> }
```

Run the statements in order in one atomic transaction and return rows as arrays. A throw must roll back everything.
API, auth, transactions and limits: see the repo README.

## Develop

Needs Rust with the `wasm32-unknown-unknown` target and `wasm-bindgen-cli` 0.2.130.

```sh
npm install && npm run build && npm test
```
