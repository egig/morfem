---
title: HTTP API
---

# HTTP API

All bodies are JSON. Send `Authorization: Bearer <key>` on every request except `GET /` and `GET /_ui`.

## Endpoints

| method and path | access | notes |
|---|---|---|
| `GET /` | none | `{"name":"morfem","version":"0.1.0"}` |
| `GET /_ui` | none | admin page (HTML) |
| `GET /_all_dbs` | read | array of db names the key can read |
| `PUT /{db}` | admin | create db. 201 `{"ok":true}`. 412 `file_exists` if present |
| `GET /{db}` | read | `{"db_name","doc_count","update_seq"}` |
| `DELETE /{db}` | admin | delete db with all docs and indexes. 200 `{"ok":true}` |
| `POST /{db}` | write | create doc with generated id (or `_id` in body). 201 |
| `GET /{db}/{id}` | read | the doc, with `_id` and `_rev` |
| `PUT /{db}/{id}` | write | create or update. 201 `{"ok":true,"id","rev"}` |
| `DELETE /{db}/{id}?rev=N` | write | delete. 200 `{"ok":true,"id","rev"}`. Leaves a tombstone |
| `GET /{db}/_all_docs` | read | see [Queries](queries) |
| `POST /{db}/_find` | read | see [Queries](queries) |
| `GET /{db}/_changes` | read | see [Queries](queries) |
| `GET /{db}/_index` | read | list indexes |
| `POST /{db}/_index` | admin | create index |
| `DELETE /{db}/_index/{name}` | admin | drop index |
| `POST /_txn` | write | see [Transactions](transactions) |

Wrong method on a known path: 405 `method_not_allowed`. Unknown `/{db}/_name`: 404.

## Documents

A document is a JSON object. `_id` and `_rev` are managed by morfem:

- `_rev` is a per-document integer rendered as a string (`"1"`, `"2"`, ...). Only the latest revision is stored.
- `PUT` without a `_rev` is create-only. If the doc exists, you get 409.
- Update with `_rev` in the body or `?rev=` in the query. `?rev=` wins if both are given. A stale `_rev` gives 409.
- `_id` in a `PUT` body must equal the path id.
- Other top-level fields starting with `_` are rejected.
- Deleting a doc then `PUT`ting it again without `_rev` re-creates it. `GET` of a deleted doc is 404 with reason `deleted`; a never-existing doc is 404 `missing`.

```sh
curl -X PUT $B/notes/n1 -d '{"title":"a"}'                      # 201 {"ok":true,"id":"n1","rev":"1"}
curl -X PUT $B/notes/n1 -d '{"title":"b"}'                      # 409 conflict
curl -X PUT "$B/notes/n1?rev=1" -d '{"title":"b"}'              # 201 rev "2"
curl -X POST $B/notes -d '{"title":"auto id"}'                  # 201 {"ok":true,"id":"<generated>","rev":"1"}
```

## Errors

Every error is `{"error":"<name>","reason":"<text>"}`.

| status | error | meaning |
|---|---|---|
| 400 | `bad_request` | invalid JSON, name, id, selector, or parameter |
| 401 | `unauthorized` | missing or unknown bearer token |
| 403 | `forbidden` | key lacks the role or db scope |
| 404 | `not_found` | db, doc, index, or endpoint missing |
| 405 | `method_not_allowed` | |
| 409 | `conflict` | stale or missing `_rev`, create over existing doc, failed guard, or index name clash |
| 412 | `file_exists` | database already exists |
| 413 | `too_large` | body over 2,000,000 bytes |
| 500 | `internal_error` | details are not exposed |
