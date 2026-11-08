---
title: Auth and limits
---

# Auth and limits

## Keys

Send `Authorization: Bearer <key>`. Keys come from `MORFEM_KEYS`, a JSON array:

```json
[{"key":"admin-key-1234","access":"admin"},
 {"key":"app-key-12345","access":"write","dbs":["notes"]},
 {"key":"ro-key-12345","access":"read","dbs":["*"]}]
```

- `key`: 8 or more characters.
- `access`: `read` < `write` < `admin`. Higher roles include lower ones.
- `dbs`: databases the key may touch. Omitted or containing `"*"` = all.
- No keys configured = every request is 401 (except `GET /` and `GET /_ui`).
- Key comparison is constant-time.

| role | can |
|---|---|
| `read` | `GET` docs, `_all_docs`, `_find`, `_changes`, `_index` (list), db info, `_all_dbs` |
| `write` | read, plus create/update/delete docs, `/_txn` |
| `admin` | write, plus create/delete databases, create/delete indexes |

A key outside its role or `dbs` scope gets 403. `_all_dbs` only lists dbs the key can read.

## Naming rules

- Database: `[a-z0-9_-]`, 1 to 64 chars, must not start with `_` or `-`.
- Document id: 1 to 256 chars, no control characters, must not start with `_`.
- Index name: `[A-Za-z0-9_]`, max 64.
- Field path (indexes, selectors, sort): dotted segments of `[A-Za-z0-9_]`, max 128.
- Top-level doc fields starting with `_` are reserved (`_id`, `_rev` only).

## Limits

| | |
|---|---|
| request body | 2,000,000 bytes |
| document | 1,000,000 bytes |
| ops / reads / guards per `/_txn` | 50 / 100 / 20 |
| `limit` on `_all_docs`, `_find` | max 1000 (default 25) |
| `limit` on `_changes` | max 1000 (default 100) |
| index fields | 1 to 8 |

## Known limits

- Only the latest revision of a doc is stored. Deletes leave tombstones; no compaction yet.
- `_find` without an index scans the whole db.
- `DELETE /{db}` on a very large db may exceed D1 per-request limits.
- Queries inside a `/_txn` are not re-run at commit; use `db` guards.
- No long-poll `_changes`, attachments, or replication.
