import { test } from "node:test";
import assert from "node:assert/strict";
import { DatabaseSync } from "node:sqlite";
import { createMorfem } from "../dist/index.js";
import { sqliteStore } from "../dist/stores/sqlite.js";

const auth = { authorization: "Bearer devkey-12345", "content-type": "application/json" };
const req = (db, method, path, body, headers = auth) =>
  db.fetch(new Request("http://x" + path, { method, headers, body: body && JSON.stringify(body) }));

test("crud, find, txn conflict", async () => {
  const db = await createMorfem({ store: sqliteStore(new DatabaseSync(":memory:")), keys: [{ key: "devkey-12345", access: "admin" }] });

  assert.equal((await req(db, "GET", "/_all_dbs", undefined, {})).status, 401);
  assert.equal((await req(db, "PUT", "/notes")).status, 201);

  const put = await req(db, "PUT", "/notes/a", { title: "hi", n: 1 });
  assert.equal(put.status, 201);
  const { rev } = await put.json();
  assert.equal(rev, "1");

  assert.deepEqual(await (await req(db, "GET", "/notes/a")).json(), { _id: "a", _rev: "1", title: "hi", n: 1 });

  const found = await (await req(db, "POST", "/notes/_find", { selector: { n: { $gte: 1 } } })).json();
  assert.equal(found.docs.length, 1);

  const bad = await req(db, "POST", "/_txn", { ops: [{ op: "put", db: "notes", id: "a", rev: "99", doc: {} }] });
  assert.equal(bad.status, 409);
  const ok = await req(db, "POST", "/_txn", { ops: [{ op: "put", db: "notes", id: "a", rev: "1", doc: { n: 2 } }] });
  assert.equal(ok.status, 200);
});

test("schema init is idempotent across restarts", async () => {
  const sqlite = new DatabaseSync(":memory:");
  const opts = { store: sqliteStore(sqlite), noAuth: true };
  const a = await createMorfem(opts);
  await a.fetch(new Request("http://x/d", { method: "PUT" }));
  const b = await createMorfem(opts);
  assert.equal((await b.fetch(new Request("http://x/d"))).status, 200);
});

test("requires auth config", async () => {
  await assert.rejects(createMorfem({ store: sqliteStore(new DatabaseSync(":memory:")) }));
});
