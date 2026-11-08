---
title: Getting started
---

# Getting started

No Rust? Use the [npm package](npm) instead: `npm i @egig/base`.

## Run natively

```sh
BASE_KEYS='[{"key":"devkey-12345","access":"admin"}]' cargo run -p base-server
# or, no auth (everyone is admin; local development only):
cargo run -p base-server -- --no-auth
```

Server options:

| flag | env | default |
|---|---|---|
| `--db` | `BASE_DB` | `base.db` (SQLite file) |
| `--listen` | `BASE_LISTEN` | `127.0.0.1:5984` |
| `--keys` | `BASE_KEYS` | none (every request is 401) |
| `--no-auth` | | off |

## Run on Cloudflare Workers

Needs `cargo install worker-build` and `rustup target add wasm32-unknown-unknown`.

```sh
cd crates/worker
npx wrangler dev          # keys come from .dev.vars
```

Deploy:

```sh
npx wrangler d1 create base            # put the id in wrangler.toml
npx wrangler secret put BASE_KEYS
npx wrangler deploy
```

`.dev.vars` example:

```
BASE_KEYS=[{"key":"devkey-12345","access":"admin"}]
```

## First requests

```sh
H='Authorization: Bearer devkey-12345'
B=http://127.0.0.1:5984

curl -X PUT  $B/notes -H "$H"                                  # {"ok":true}
curl -X PUT  $B/notes/n1 -H "$H" -d '{"title":"hi","n":1}'     # {"ok":true,"id":"n1","rev":"1"}
curl         $B/notes/n1 -H "$H"                               # {"_id":"n1","_rev":"1","title":"hi","n":1}
curl -X PUT  $B/notes/n1 -H "$H" -d '{"_rev":"1","title":"hello","n":2}'   # rev "2"
curl -X POST $B/notes/_find -H "$H" -d '{"selector":{"n":{"$gte":1}}}'
curl -X DELETE "$B/notes/n1?rev=2" -H "$H"
```

## Admin page

`GET /_ui` serves a static admin page. It is public and holds no data; it sends your token with each API call.
`GET /` is also public and returns `{"name":"base","version":"..."}`.
