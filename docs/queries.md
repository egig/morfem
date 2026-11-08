---
title: Queries
---

# Queries

## `GET /{db}/_all_docs`

Query parameters (plain strings, not JSON-encoded):

| param | default | notes |
|---|---|---|
| `startkey` / `start_key` | | inclusive lower bound on id |
| `endkey` / `end_key` | | inclusive upper bound on id |
| `limit` | 25 | max 1000 |
| `skip` | 0 | |
| `descending` | false | |
| `include_docs` | false | adds `doc` to each row |

```json
{"offset":0,"rows":[{"id":"n1","key":"n1","value":{"rev":"1"},"doc":{"_id":"n1","_rev":"1"}}],"update_seq":7}
```

Deleted docs are not listed.

## `POST /{db}/_find` (Mango)

```json
{"selector":{"age":{"$gte":18},"addr":{"city":"Oslo"}},
 "sort":["age",{"name":"desc"}],
 "limit":10,"skip":0}
```

Response:

```json
{"docs":[{"_id":"...","_rev":"1","age":20}],"update_seq":7,"warning":"No matching index found, falling back to a scan of the database."}
```

- `selector` is required. `limit` defaults to 25, max 1000.
- Results are ordered by `sort`, then by `id`. No `sort` = by id.
- Field names are dotted paths of `[A-Za-z0-9_]` segments, e.g. `addr.city`. A plain nested object is a subfield match.
- Operators: implicit `$eq`, `$eq $ne $gt $gte $lt $lte $in $nin $exists`, and `$and $or $nor $not`.
- `warning` appears when no index covers the selector: the query scans the whole db.
- `update_seq` can be used as a `db` guard in a [transaction](transactions) to protect against phantom reads.

## Indexes

```sh
curl -X POST $B/notes/_index -d '{"fields":["age"],"name":"by_age"}'   # 201 {"result":"created","name":"by_age"}
curl $B/notes/_index                                                    # {"indexes":[{"name":"by_age","fields":["age"]}]}
curl -X DELETE $B/notes/_index/by_age                                   # {"ok":true}
```

- `fields`: 1 to 8 dotted paths. `name` optional (default: fields joined with `_`); `[A-Za-z0-9_]`, max 64.
- Same name and same fields again: 200 `{"result":"exists"}`. Same name, different fields: 409.
- A query is covered when its first index field appears in the selector.
- Index creation on a large db is slow on D1.

## `GET /{db}/_changes`

| param | default | notes |
|---|---|---|
| `since` | 0 | return changes with `seq` greater than this |
| `limit` | 100 | max 1000 |
| `include_docs` | false | |

```json
{"results":[{"seq":5,"id":"n1","changes":[{"rev":"2"}],"doc":{"_id":"n1","_rev":"2"}},
            {"seq":6,"id":"n2","changes":[{"rev":"3"}],"deleted":true}],
 "last_seq":6}
```

- Latest state per doc only (not a history), ordered by `seq` then id.
- Deleted docs appear with `"deleted":true` and no `doc`.
- A transaction stamps all its writes with one `seq`; a page is never split in the middle of one `seq`, so it can exceed `limit`.
- Poll with `since=<last_seq>`. No long-poll or SSE yet.
