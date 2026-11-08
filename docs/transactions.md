---
title: Transactions
---

# Transactions

D1 has no interactive transactions, so morfem uses optimistic concurrency. You send reads you depended on, guards, and writes in one request. morfem validates and applies them in one atomic batch.

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
// 200 {"ok":true,"seq":43,"results":[{"db":"acct","id":"a","rev":"2"}, ...]}
// 409 on any stale rev or failed guard; nothing applied
```

Fields (all arrays optional):

- `reads`: `{db, id, rev?}`. The doc must be at `rev`, or absent/deleted when `rev` is omitted. Checked per document.
- `guards`: `{"kind":"db","db","seq"}`. Fails if any doc in `db` was written after `seq`. Use the `update_seq` returned by `_find` or `_all_docs` to protect a query result from phantoms.
- `ops`: `put` (`rev` omitted = create only, else update from `rev`) and `delete` (`rev` required).

Rules:

- Max 50 ops, 100 reads, 20 guards. 1 MB per doc.
- One op per document per transaction (duplicates are 400).
- Writes need `write` on every db touched; reads and guards need `read`.
- All writes share one global `seq`, returned as `seq`. `results` has one entry per op, in order.
- Missing db: 404, nothing applied.

## Retry pattern

1. `GET` the docs, remember each `_rev`.
2. Compute new values.
3. `POST /_txn` with those revs in `ops` (and `reads` for docs you only read).
4. On 409, go to step 1.

## Cloudflare Free plan

Free caps D1 queries per Worker invocation at 50. Each op costs about 3 statements, so large transactions need the Paid plan.
