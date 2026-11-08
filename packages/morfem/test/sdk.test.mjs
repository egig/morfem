import { test } from "node:test";
import assert from "node:assert/strict";
import { openFile, MorfemError } from "../dist/node.js";

test("sdk crud, find, update, txn, changes", async () => {
  const morfem = await openFile(":memory:");
  const notes = await morfem.createDb("notes");
  await morfem.createDb("notes"); // idempotent

  assert.equal(await notes.get("nope"), null);
  const { rev } = await notes.put("a", { title: "hi", n: 1 });
  assert.equal(rev, "1");
  await assert.rejects(notes.put("a", { title: "x" }), (e) => e instanceof MorfemError && e.isConflict);
  assert.equal((await notes.put("a", { title: "hi2", n: 2 }, { rev })).rev, "2");
  assert.deepEqual(await notes.get("a"), { _id: "a", _rev: "2", title: "hi2", n: 2 });

  await notes.update("a", (d) => ({ ...d, n: d.n + 1 }));
  assert.equal((await notes.get("a")).n, 3);
  await notes.update("counter", (d) => ({ n: (d?.n ?? 0) + 1 }));

  await notes.createIndex(["n"]);
  const found = await notes.find({ selector: { n: { $gte: 3 } }, sort: [{ n: "desc" }] });
  assert.equal(found.docs.length, 1);
  assert.equal((await notes.allDocs({ includeDocs: true })).rows.length, 2);

  const a = await notes.get("a");
  await assert.rejects(
    morfem.txn({ ops: [{ op: "put", db: "notes", id: "a", rev: "99", doc: {} }] }),
    (e) => e.isConflict,
  );
  const t = await morfem.txn({ ops: [{ op: "put", db: "notes", id: "a", rev: a._rev, doc: { n: 10 } }, { op: "delete", db: "notes", id: "counter", rev: "1" }] });
  assert.equal(t.results.length, 2);

  const ch = await notes.changes({ includeDocs: true });
  assert.ok(ch.results.some((r) => r.id === "counter" && r.deleted));
  assert.deepEqual(await morfem.listDbs(), ["notes"]);
  await morfem.dropDb("notes");
  assert.deepEqual(await morfem.listDbs(), []);
  morfem.close();
});
