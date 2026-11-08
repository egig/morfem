---
title: base
---

# base

Schemaless, transactional document database with a CouchDB-shaped HTTP API.
SQLite underneath: `rusqlite` natively, D1 on Cloudflare Workers.

- JSON documents, per-document integer revisions (`_rev`).
- Mango queries (`_find`) with optional indexes.
- Multi-document, multi-database transactions (`/_txn`) using optimistic concurrency.
- Change feed (`_changes`) ordered by a global `seq`.
- Bearer-token auth with `read`, `write`, `admin` roles, optionally scoped to databases.

## Pages

| page | covers |
|---|---|
| [Getting started](getting-started) | run locally, deploy to Cloudflare, first requests |
| [HTTP API](api) | every endpoint, request and response shapes, errors |
| [Queries](queries) | `_all_docs`, Mango `_find`, indexes, `_changes` |
| [Transactions](transactions) | `/_txn`, reads, guards, conflicts |
| [Auth and limits](auth-and-limits) | keys, roles, limits, naming rules |

## For LLMs and agents

- [llms.txt](llms.txt): short index.
- [llms-full.txt](llms-full.txt): all documentation in one plain-text file.
