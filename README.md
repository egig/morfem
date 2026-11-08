# base

Schemaless, transactional document database with a CouchDB-shaped HTTP API.
SQLite under the hood: `rusqlite` natively, D1 on Cloudflare Workers.

## Layout

| crate | role |
|---|---|
| `crates/engine` | Engine, router, Mango compiler, auth. No IO deps; builds for wasm. Owns the SQL schema (`schema.sql`, applied on every boot). |
| `crates/sqlite` | `Store` over rusqlite. Integration tests live here. |
| `crates/server` | Native axum binary (dev, self-host). |
| `crates/worker` | Cloudflare Worker + D1 `Store`. |

## Run

```sh
# native
BASE_KEYS='[{"key":"devkey-12345","access":"admin"}]' cargo run -p base-server
# or: cargo run -p base-server -- --no-auth

# Worker, local D1
cd crates/worker
npx wrangler dev          # keys come from .dev.vars

# deploy
npx wrangler d1 create base            # put the id in wrangler.toml
npx wrangler secret put BASE_KEYS
npx wrangler deploy
```

The schema is applied automatically on boot (native open, Worker cold start); there is no migration step. Needs `cargo install worker-build` and `rustup target add wasm32-unknown-unknown`.

## Auth

`Authorization: Bearer <key>`. `BASE_KEYS` is a JSON array:
`[{"key":"...","access":"admin|write|read","dbs":["notes"]}]` (`dbs` omitted = all).
Keys must be 8+ chars. No keys configured = every request is 401.
`admin` creates/deletes dbs and indexes; `write` writes docs; `read` reads.

## API

| | |
|---|---|
| `PUT /{db}` `DELETE /{db}` `GET /{db}` | create / delete / info (`doc_count`, `update_seq`) |
| `GET /_all_dbs` | |
| `POST /{db}` | create doc with generated id |
| `GET\|PUT\|DELETE /{db}/{id}` | `PUT` without `_rev` = create only; update needs `_rev` (body) or `?rev=`; `DELETE` needs `?rev=` |
| `GET /{db}/_all_docs` | `startkey endkey limit skip descending include_docs` (plain strings, not JSON-encoded) |
| `POST /{db}/_find` | Mango: `{"selector":{...},"sort":[...],"limit":n,"skip":n}` |
| `GET\|POST /{db}/_index`, `DELETE /{db}/_index/{name}` | `{"fields":["age"],"name":"opt"}` |
| `GET /{db}/_changes` | `since limit include_docs`. Latest state per doc, ordered by `seq`. |
| `POST /_txn` | multi-doc, multi-db transaction |

`_rev` is a per-doc integer rendered as a string. Only the latest revision is stored; deletes leave tombstones.
Names starting with `_` are reserved for ids and top-level fields. Db names: `[a-z0-9_-]`, max 64.

Mango operators: implicit eq, nested subfields, `$eq $ne $gt $gte $lt $lte $in $nin $exists $and $or $nor $not`.
`_find` and `_all_docs` return `update_seq`. `_find` warns when no index covers the selector.

## Transactions

D1 has no interactive transactions, so base uses optimistic concurrency:

```jsonc
POST /_txn
{
  "reads":  [{"db":"acct","id":"a","rev":"1"}, {"db":"acct","id":"new"}],  // rev omitted = must be absent
  "guards": [{"kind":"db","db":"acct","seq":42}],   // fail if anything in db changed after seq 42
  "ops": [
    {"op":"put","db":"acct","id":"a","rev":"1","doc":{"bal":50}},          // rev omitted = create only
    {"op":"delete","db":"acct","id":"b","rev":"3"}
  ]
}
// 200 {"ok":true,"seq":43,"results":[{"db","id","rev"}]}   409 on any stale rev; nothing applied
```

Reads are validated per document. For query results (phantoms), pass the `update_seq` the query returned as a `db` guard.
Commit = one atomic batch: guard statements abort and roll back on mismatch, then one global `seq` bump stamps every write.
Limits: 50 ops, 100 reads, 20 guards, 1 MB per doc, `skip` max 100000 (400 above; page with `startkey` / `since`).

## Reactiveness (not built)

Every commit has a global `seq`, every doc stores the `seq` of its last write, and `_changes?since=` is the feed.
Subscriptions can later be a long-poll/SSE layer, or a Durable Object fan-out, on top of that.

## Known limits

- Cloudflare Free plan caps queries per Worker invocation (50), so large `/_txn` calls need the Paid plan. Each op costs about 2 statements.
- `_find` without an index scans the db. Create indexes with `_index`. Indexes with the same field list are shared across dbs (one physical index, `db` leading), so write cost grows with distinct field lists, not db count. Index DDL on big dbs is slow on D1.
- `DELETE /{db}` deletes docs in 5000-row chunks (several D1 round trips on a big db). A failure midway leaves the db partly emptied; retry finishes it.
- Queries inside a `/_txn` are not re-run at commit; use `db` guards.
- No long-poll `_changes`, attachments, replication, or tombstone compaction yet.
