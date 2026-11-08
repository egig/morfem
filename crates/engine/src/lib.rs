//! morfem-engine: schemaless, transactional document engine over a SQLite-like store.
//! No runtime or IO dependencies; builds for native and `wasm32-unknown-unknown`.

pub mod auth;
mod engine;
pub mod error;
pub mod http;
pub mod mango;
pub mod store;
pub mod validate;

pub use engine::{Committed, Engine, Guard, Op, ReadSpec, Rev, Txn};
pub use error::{Error, Result};
pub use http::{Request, Response, CONTENT_TYPE};
pub use store::{Row, Stmt, Store, Val};

/// SQL schema, applied on every boot by every store. Idempotent; see the header of `schema.sql`.
pub const SCHEMA: &str = include_str!("../schema.sql");

/// `SCHEMA` split into single statements (separator line: `--;;`), for stores that cannot run a script (D1).
pub fn schema_statements() -> impl Iterator<Item = &'static str> {
    SCHEMA.split("\n--;;\n").map(str::trim).filter(|s| !s.is_empty())
}
