//! Native SQLite backend for morfem-engine (rusqlite).

use morfem_engine::{Error, Result, Row, Stmt, Store, Val};
use rusqlite::types::{ToSqlOutput, ValueRef};
use rusqlite::{params_from_iter, Connection, OpenFlags, TransactionBehavior};
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, TryLockError};

const READERS: usize = 4;

/// One writer connection plus a small pool of read-only connections (file DBs only).
/// WAL lets readers run while the writer commits.
pub struct SqliteStore {
    conn: Mutex<Connection>,
    readers: Vec<Mutex<Connection>>,
    next: AtomicUsize,
}

impl SqliteStore {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let err = |e: rusqlite::Error| Error::Internal(e.to_string());
        let conn = Connection::open(path.as_ref()).map_err(err)?;
        let mut store = Self::init(conn)?;
        for _ in 0..READERS {
            let r = Connection::open_with_flags(
                path.as_ref(),
                OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX | OpenFlags::SQLITE_OPEN_URI,
            )
            .map_err(err)?;
            r.busy_timeout(std::time::Duration::from_secs(5)).map_err(err)?;
            r.set_prepared_statement_cache_capacity(64);
            store.readers.push(Mutex::new(r));
        }
        Ok(store)
    }

    pub fn memory() -> Result<Self> {
        let conn = Connection::open_in_memory().map_err(|e| Error::Internal(e.to_string()))?;
        Self::init(conn)
    }

    fn init(conn: Connection) -> Result<Self> {
        let setup = || -> rusqlite::Result<()> {
            // journal_mode returns a row, so read it instead of executing.
            conn.query_row("PRAGMA journal_mode=WAL", [], |_| Ok(()))?;
            conn.busy_timeout(std::time::Duration::from_secs(5))?;
            // WAL + NORMAL: no fsync per commit; durable across app crashes, may lose last commits on power loss.
            conn.execute_batch("PRAGMA synchronous=NORMAL; PRAGMA temp_store=MEMORY; PRAGMA cache_size=-65536;")?;
            conn.set_prepared_statement_cache_capacity(64);
            Ok(())
        };
        setup().map_err(|e| Error::Internal(e.to_string()))?;
        // Atomic and serialized across processes by the write lock (busy_timeout above): a crash or
        // failure leaves the schema untouched, and two servers booting together cannot interleave.
        let mut conn = conn;
        let apply = |conn: &mut Connection| -> rusqlite::Result<()> {
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            tx.execute_batch(morfem_engine::SCHEMA)?;
            tx.commit()
        };
        apply(&mut conn).map_err(|e| Error::Internal(format!("schema: {e}")))?;
        Ok(SqliteStore { conn: Mutex::new(conn), readers: vec![], next: AtomicUsize::new(0) })
    }
}

fn to_sql(v: &Val) -> ToSqlOutput<'_> {
    ToSqlOutput::Borrowed(match v {
        Val::Null => ValueRef::Null,
        Val::Int(i) => ValueRef::Integer(*i),
        Val::Real(f) => ValueRef::Real(*f),
        Val::Text(s) => ValueRef::Text(s.as_bytes()),
    })
}

fn from_ref(v: ValueRef<'_>) -> rusqlite::Result<Val> {
    Ok(match v {
        ValueRef::Null => Val::Null,
        ValueRef::Integer(i) => Val::Int(i),
        ValueRef::Real(f) => Val::Real(f),
        ValueRef::Text(t) => Val::Text(String::from_utf8_lossy(t).into_owned()),
        ValueRef::Blob(_) => return Err(rusqlite::Error::InvalidColumnType(0, "blob".into(), rusqlite::types::Type::Blob)),
    })
}

fn is_read_only(stmts: &[Stmt]) -> bool {
    stmts.iter().all(|s| s.sql.trim_start().get(..6).is_some_and(|p| p.eq_ignore_ascii_case("select")))
}

fn run(conn: &mut Connection, stmts: Vec<Stmt>, behavior: TransactionBehavior) -> rusqlite::Result<Vec<Vec<Row>>> {
    // Writes use IMMEDIATE: write lock up front, so no read->write upgrade deadlocks.
    // Reads use DEFERRED on a read-only connection: consistent snapshot, no write lock.
    let tx = conn.transaction_with_behavior(behavior)?;
    let mut out = Vec::with_capacity(stmts.len());
    for s in stmts {
        let mut st = tx.prepare_cached(&s.sql)?;
        let ncol = st.column_count();
        let params = params_from_iter(s.params.iter().map(to_sql));
        if ncol == 0 {
            st.execute(params)?;
            out.push(vec![]);
        } else {
            let rows = st
                .query_map(params, |r| (0..ncol).map(|i| from_ref(r.get_ref(i)?)).collect::<rusqlite::Result<Row>>())?
                .collect::<rusqlite::Result<Vec<Row>>>()?;
            out.push(rows);
        }
    }
    tx.commit()?;
    Ok(out)
}

impl Store for SqliteStore {
    async fn batch(&self, stmts: Vec<Stmt>) -> Result<Vec<Vec<Row>>> {
        let poisoned = || Error::Internal("store lock poisoned".into());
        if !self.readers.is_empty() && is_read_only(&stmts) {
            let n = self.readers.len();
            let start = self.next.fetch_add(1, Ordering::Relaxed);
            // Prefer an idle reader; otherwise wait on one.
            let mut guard = None;
            for k in 0..n {
                match self.readers[(start + k) % n].try_lock() {
                    Ok(g) => {
                        guard = Some(g);
                        break;
                    }
                    Err(TryLockError::Poisoned(_)) => return Err(poisoned()),
                    Err(TryLockError::WouldBlock) => {}
                }
            }
            let mut conn = match guard {
                Some(g) => g,
                None => self.readers[start % n].lock().map_err(|_| poisoned())?,
            };
            return run(&mut conn, stmts, TransactionBehavior::Deferred).map_err(|e| Error::from_store(e.to_string()));
        }
        let mut conn = self.conn.lock().map_err(|_| poisoned())?;
        run(&mut conn, stmts, TransactionBehavior::Immediate).map_err(|e| Error::from_store(e.to_string()))
    }
}
