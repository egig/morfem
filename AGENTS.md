# AGENTS.md

Guidance for AI coding agents working in this repo. User-facing docs (API, auth, transactions, limits) are in [README.md](README.md). Read it first.

## What this is

Schemaless, transactional document DB with a CouchDB-shaped HTTP API. SQLite underneath: `rusqlite` natively, D1 on Cloudflare Workers.

## Crates

| crate | role |
|---|---|
| `crates/engine` | All logic: `Engine`, routing, Mango compiler (`mango.rs`), auth, validation. No IO deps. Must build for `wasm32-unknown-unknown`. Owns the SQL schema in `schema.sql` (exported as `SCHEMA`, split by `schema_statements()`). |
| `crates/sqlite` | `Store` impl over rusqlite. Integration tests: `crates/sqlite/tests/api.rs`. |
| `crates/server` | Native axum binary. |
| `crates/worker` | Cloudflare Worker + D1 `Store` impl. |

Dependency direction: `server`, `sqlite`, `worker` -> `engine`. Never the reverse.

## Architecture rules

- `engine` talks to storage only through the `Store` trait (`crates/engine/src/store.rs`). It has one method, `batch(Vec<Stmt>)`: ordered statements, one atomic transaction, per-statement rows back. No interactive transactions, because D1 has none.
- Concurrency is optimistic. `/_txn` validates reads (`rev`) and guards (`seq`) inside the same batch as the writes. A failed guard hits the `_guard` CHECK constraint (`morfem_conflict`), rolls back the batch, and maps to `Error::Conflict` (409). Use `Error::from_store` in any new `Store` impl.
- SELECT columns must have unique names (D1 returns rows as objects).
- Do not add IO, threads, `std::time`, or native-only crates to `engine`. Keep IDs/time injected (see `Engine::new(..., new_id)`).
- Schema change: edit `crates/engine/schema.sql`. It runs on every boot (native `SqliteStore::open`, Worker via `Engine::init` once per isolate), atomically, so every statement must be idempotent (`IF NOT EXISTS`, `INSERT OR IGNORE`), and an existing table must only gain things (new tables, indexes, triggers); never rewrite or drop what a deployed DB already has. Separate statements with a `--;;` line. There is no migration step or `wrangler d1 migrations`.
- Reserved: names starting with `_` for ids and top-level fields. Db names `[a-z0-9_-]`, max 64.
- Limits live as consts in `engine.rs` (`MAX_OPS`, `MAX_READS`, `MAX_GUARDS`) and `validate.rs` (`MAX_DOC_BYTES`). If you change one, update the README.

## Build and test

```sh
cargo test --workspace                          # native, includes sqlite integration tests
cargo check -p morfem-engine --target wasm32-unknown-unknown   # engine must stay wasm-clean
cargo run -p morfem-server -- --no-auth           # local server, no keys
```

Worker (needs `cargo install worker-build`, `rustup target add wasm32-unknown-unknown`):

```sh
cd crates/worker
npx wrangler dev        # keys from .dev.vars
```

## Conventions

- Behavior changes get an integration test in `crates/sqlite/tests/api.rs`. It drives the engine over HTTP-shaped requests against a real SQLite store.
- API changes: update the README tables in the same change.
- Error responses are CouchDB-shaped; add variants in `crates/engine/src/error.rs`, not ad-hoc responses.
- Prefer small, surgical diffs. No new dependencies without need; `engine` deps especially must compile to wasm.

## Do not touch / secrets

- `crates/worker/.dev.vars` holds local API keys. Never read it into output, commit it, or paste it anywhere. It is gitignored.
- Ignored build output: `target/`, `build/`, `.wrangler/`, `*.db*`. Do not edit.
- Never run `wrangler deploy`, `wrangler secret put`, or `d1 ... --remote` without explicit user approval.

## Not built yet

Long-poll/SSE `_changes`, attachments, replication, tombstone compaction. Don't assume they exist.
