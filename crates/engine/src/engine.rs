use crate::auth::{Access, Authenticator, Principal};
use crate::error::{Error, Result};
use crate::http::{Request, Response};
use crate::mango;
use crate::store::{Row, Stmt, Store, Val};
use crate::validate::{self, json_expr, MAX_DOC_BYTES};
use serde::ser::{Serialize, SerializeMap, Serializer};
use serde::{Deserialize, Deserializer};
use serde_json::{json, Map, Value};

const UI_HTML: &str = include_str!("ui.html");
const MAX_BODY: usize = 2_000_000;
const MAX_OPS: usize = 50;
const MAX_READS: usize = 100;
const MAX_GUARDS: usize = 20;
/// `skip` walks and discards rows; deep offsets are O(n).
const MAX_SKIP: u64 = 100_000;
const DELETE_CHUNK: i64 = 5000;

/// Revision: per-document counter. JSON form is a string (`"3"`); numbers are accepted on input.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rev(pub u64);

impl<'de> Deserialize<'de> for Rev {
    fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Repr {
            N(u64),
            S(String),
        }
        match Repr::deserialize(d)? {
            Repr::N(n) => Ok(Rev(n)),
            Repr::S(s) => s.parse().map(Rev).map_err(serde::de::Error::custom),
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct ReadSpec {
    pub db: String,
    pub id: String,
    /// `None` asserts the doc is absent (or deleted).
    #[serde(default)]
    pub rev: Option<Rev>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Guard {
    /// Fail the commit if any doc in `db` was written after `seq` (phantom protection for queries).
    Db { db: String, seq: u64 },
}

#[derive(Debug, Deserialize)]
#[serde(tag = "op", rename_all = "lowercase")]
pub enum Op {
    /// `rev` absent = create only; present = update from that revision.
    Put {
        db: String,
        id: String,
        doc: Value,
        #[serde(default)]
        rev: Option<Rev>,
    },
    Delete { db: String, id: String, rev: Rev },
}

impl Op {
    fn target(&self) -> (&str, &str) {
        match self {
            Op::Put { db, id, .. } | Op::Delete { db, id, .. } => (db, id),
        }
    }
}

#[derive(Debug, Default, Deserialize)]
pub struct Txn {
    #[serde(default)]
    pub reads: Vec<ReadSpec>,
    #[serde(default)]
    pub guards: Vec<Guard>,
    #[serde(default)]
    pub ops: Vec<Op>,
}

pub struct Committed {
    pub seq: i64,
    pub results: Vec<(String, String, i64)>,
}

pub struct Engine<S: Store> {
    store: S,
    auth: Box<dyn Authenticator + Send + Sync>,
    new_id: fn() -> String,
}

fn int(row: &Row, i: usize) -> Result<i64> {
    row.get(i).and_then(Val::as_i64).ok_or_else(|| Error::Internal(format!("column {i} not an integer")))
}

fn text(row: &Row, i: usize) -> Result<String> {
    row.get(i)
        .and_then(Val::as_str)
        .map(str::to_string)
        .ok_or_else(|| Error::Internal(format!("column {i} not text")))
}

/// Reply body: a parsed value, or JSON already serialized (docs are spliced from stored text).
enum Out {
    V(Value),
    Raw(String),
}

fn val(r: Result<(u16, Value)>) -> Result<(u16, Out)> {
    r.map(|(s, v)| (s, Out::V(v)))
}

fn json_str(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| "\"\"".into())
}

/// `{"_id":..,"_rev":"n",<stored fields>}` without parsing the stored body.
/// Stored bodies are compact objects written by `store_body`, with `_` keys already stripped.
fn render_raw(id: &str, rev: i64, body: &str) -> Result<String> {
    let inner = match body.strip_prefix('{') {
        Some(rest) if rest.ends_with('}') => rest,
        _ => return Err(Error::Internal("stored body not an object".into())),
    };
    let mut s = String::with_capacity(body.len() + id.len() + 32);
    s.push_str("{\"_id\":");
    s.push_str(&json_str(id));
    s.push_str(",\"_rev\":\"");
    s.push_str(&rev.to_string());
    s.push('"');
    if inner.len() > 1 {
        s.push(',');
    }
    s.push_str(inner);
    Ok(s)
}

fn parse_body(req: &Request) -> Result<Value> {
    serde_json::from_slice(&req.body).map_err(|e| Error::BadRequest(format!("invalid JSON body: {e}")))
}

fn parse_obj(req: &Request) -> Result<Map<String, Value>> {
    match parse_body(req)? {
        Value::Object(m) => Ok(m),
        _ => Err(Error::BadRequest("body must be a JSON object".into())),
    }
}

struct Query(Vec<(String, String)>);

impl Query {
    fn get(&self, k: &str) -> Option<&str> {
        self.0.iter().find(|(key, _)| key == k).map(|(_, v)| v.as_str())
    }
    fn first(&self, ks: &[&str]) -> Option<&str> {
        ks.iter().find_map(|k| self.get(k))
    }
    fn num(&self, k: &str, default: u64, max: u64) -> Result<u64> {
        match self.get(k) {
            None => Ok(default),
            Some(s) => s
                .parse::<u64>()
                .map(|n| n.min(max))
                .map_err(|_| Error::BadRequest(format!("query param {k} must be a non-negative integer"))),
        }
    }
    fn flag(&self, k: &str) -> bool {
        self.get(k) == Some("true")
    }
}

fn seq_stmt() -> Stmt {
    Stmt::new("SELECT v AS seq FROM meta WHERE k='seq'", vec![])
}

fn db_exists_stmt(db: &str) -> Stmt {
    Stmt::new("SELECT 1 AS x FROM dbs WHERE name=?", vec![db.into()])
}

fn guard_stmt(cond: &str, params: Vec<Val>) -> Stmt {
    Stmt::new(format!("INSERT INTO _guard(ok) SELECT CASE WHEN {cond} THEN 1 ELSE 0 END"), params)
}

fn guard_live_rev(db: &str, id: &str, rev: Option<Rev>) -> Stmt {
    match rev {
        Some(Rev(r)) => guard_stmt(
            "EXISTS(SELECT 1 FROM docs WHERE db=? AND id=? AND deleted=0 AND rev=?)",
            vec![db.into(), id.into(), Val::Int(r as i64)],
        ),
        None => guard_stmt(
            "NOT EXISTS(SELECT 1 FROM docs WHERE db=? AND id=? AND deleted=0)",
            vec![db.into(), id.into()],
        ),
    }
}

/// Physical index, shared by every db that declares the same field list. Leading `db` column
/// keeps lookups per-db; no per-db partial predicate, so write cost scales with distinct
/// field lists rather than with the number of dbs.
fn index_ident(fields: &[String]) -> String {
    format!("\"base:idx:{}\"", fields.join(","))
}

/// Pre-shared-index name (partial index per db). May still exist in older databases.
fn legacy_index_ident(db: &str, name: &str) -> String {
    format!("\"base:{db}:{name}\"")
}

impl<S: Store> Engine<S> {
    pub fn new(store: S, auth: Box<dyn Authenticator + Send + Sync>, new_id: fn() -> String) -> Self {
        Engine { store, auth, new_id }
    }

    /// Apply the schema as one atomic batch. Idempotent and safe to run concurrently; call once per
    /// process / isolate before serving. Stores that apply the schema themselves on open need not call it.
    pub async fn init(&self) -> Result<()> {
        let stmts = crate::schema_statements().map(|sql| Stmt::new(sql, vec![])).collect();
        self.store.batch(stmts).await.map(|_| ())
    }

    pub async fn handle(&self, req: Request) -> Response {
        // Static admin page. Public: it holds no data and sends the user's token with each API call.
        if req.method.eq_ignore_ascii_case("GET") && req.path.trim_end_matches('/') == "/_ui" {
            return Response::html(UI_HTML);
        }
        match self.route(req).await {
            Ok((status, Out::V(v))) => Response::json(status, &v),
            Ok((status, Out::Raw(s))) => Response::json_raw(status, s),
            Err(e) => Response::json(e.status(), &e.to_json()),
        }
    }

    fn require(&self, p: &Principal, db: &str, need: Access) -> Result<()> {
        if p.can(db, need) {
            Ok(())
        } else {
            Err(Error::Forbidden)
        }
    }

    async fn route(&self, req: Request) -> Result<(u16, Out)> {
        if req.body.len() > MAX_BODY {
            return Err(Error::PayloadTooLarge);
        }
        let segs: Vec<String> = req
            .path
            .split('/')
            .filter(|s| !s.is_empty())
            .map(|s| {
                percent_encoding::percent_decode_str(s)
                    .decode_utf8()
                    .map(|c| c.into_owned())
                    .map_err(|_| Error::BadRequest("path is not valid UTF-8".into()))
            })
            .collect::<Result<_>>()?;
        let s: Vec<&str> = segs.iter().map(String::as_str).collect();
        let q = Query(form_urlencoded::parse(req.query.as_bytes()).into_owned().collect());
        let m = req.method.to_ascii_uppercase();

        if m == "GET" && s.is_empty() {
            return Ok((200, Out::V(json!({"name": "base", "version": env!("CARGO_PKG_VERSION")}))));
        }
        let p = self.auth.authenticate(req.bearer.as_deref()).ok_or(Error::Unauthorized)?;

        match (m.as_str(), s.as_slice()) {
            ("GET", ["_all_dbs"]) => val(self.all_dbs(&p).await),
            ("POST", ["_txn"]) => val(self.txn_endpoint(&p, &req).await),
            (_, [] | ["_all_dbs"] | ["_txn"]) => Err(Error::MethodNotAllowed),

            ("GET", [db, "_all_docs"]) => self.all_docs(&p, db, &q).await,
            ("POST", [db, "_find"]) => self.find(&p, db, &req).await,
            ("GET", [db, "_changes"]) => self.changes(&p, db, &q).await,
            ("GET", [db, "_index"]) => val(self.list_indexes(&p, db).await),
            ("POST", [db, "_index"]) => val(self.create_index(&p, db, &req).await),
            ("DELETE", [db, "_index", name]) => val(self.delete_index(&p, db, name).await),
            (_, [_, "_all_docs" | "_find" | "_changes" | "_index"] | [_, "_index", _]) => {
                Err(Error::MethodNotAllowed)
            }
            (_, [_, id, ..]) if id.starts_with('_') => Err(Error::NotFound("Unknown endpoint.".into())),

            ("PUT", [db]) => val(self.create_db(&p, db).await),
            ("DELETE", [db]) => val(self.delete_db(&p, db).await),
            ("GET", [db]) => val(self.db_info(&p, db).await),
            ("POST", [db]) => val(self.post_doc(&p, db, &req).await),
            (_, [_]) => Err(Error::MethodNotAllowed),

            ("GET", [db, id]) => self.get_doc(&p, db, id).await,
            ("PUT", [db, id]) => val(self.put_doc(&p, db, id, &q, &req).await),
            ("DELETE", [db, id]) => val(self.delete_doc(&p, db, id, &q).await),
            (_, [_, _]) => Err(Error::MethodNotAllowed),
            _ => Err(Error::NotFound("Unknown endpoint.".into())),
        }
    }

    // ---- databases -------------------------------------------------------

    async fn all_dbs(&self, p: &Principal) -> Result<(u16, Value)> {
        let r = self.store.batch(vec![Stmt::new("SELECT name FROM dbs ORDER BY name", vec![])]).await?;
        let names: Vec<Value> = r[0]
            .iter()
            .filter_map(|row| row[0].as_str())
            .filter(|n| p.can(n, Access::Read))
            .map(|n| json!(n))
            .collect();
        Ok((200, Value::Array(names)))
    }

    async fn create_db(&self, p: &Principal, db: &str) -> Result<(u16, Value)> {
        validate::db_name(db)?;
        self.require(p, db, Access::Admin)?;
        let r = self
            .store
            .batch(vec![
                Stmt::new(
                    "INSERT OR IGNORE INTO dbs(name, created_seq) SELECT ?, v FROM meta WHERE k='seq'",
                    vec![db.into()],
                ),
                Stmt::new("SELECT changes() AS n", vec![]),
            ])
            .await?;
        if int(&r[1][0], 0)? == 0 {
            return Err(Error::PreconditionFailed("The database could not be created, it already exists.".into()));
        }
        Ok((201, json!({"ok": true})))
    }

    async fn delete_db(&self, p: &Principal, db: &str) -> Result<(u16, Value)> {
        validate::db_name(db)?;
        self.require(p, db, Access::Admin)?;
        let idx = self
            .store
            .batch(vec![Stmt::new("SELECT name, fields FROM indexes WHERE db=?", vec![db.into()])])
            .await?;
        let mut stmts = vec![];
        let mut field_sets: Vec<Vec<String>> = vec![];
        for row in &idx[0] {
            stmts.push(Stmt::new(format!("DROP INDEX IF EXISTS {}", legacy_index_ident(db, &text(row, 0)?)), vec![]));
            field_sets.push(serde_json::from_str(&text(row, 1)?).unwrap_or_default());
        }
        stmts.push(Stmt::new("DELETE FROM docs WHERE db=?", vec![db.into()]));
        stmts.push(Stmt::new("DELETE FROM db_counts WHERE db=?", vec![db.into()]));
        stmts.push(Stmt::new("DELETE FROM indexes WHERE db=?", vec![db.into()]));
        stmts.push(Stmt::new("DELETE FROM dbs WHERE name=?", vec![db.into()]));
        stmts.push(Stmt::new("SELECT changes() AS n", vec![]));
        // Delete docs in chunks so one huge db does not hold the write lock (or exceed D1 limits) in a
        // single statement. The db row goes last, in the batch below, so the name stays taken until empty.
        loop {
            let r = self
                .store
                .batch(vec![
                    Stmt::new(
                        "DELETE FROM docs WHERE db=? AND id IN (SELECT id FROM docs WHERE db=? LIMIT ?)",
                        vec![db.into(), db.into(), Val::Int(DELETE_CHUNK)],
                    ),
                    Stmt::new("SELECT changes() AS n", vec![]),
                ])
                .await?;
            if int(&r[1][0], 0)? < DELETE_CHUNK {
                break;
            }
        }
        let r = self.store.batch(stmts).await?;
        if int(&r.last().unwrap()[0], 0)? == 0 {
            return Err(Error::NotFound("Database does not exist.".into()));
        }
        for f in field_sets {
            self.drop_unused_index(&f).await?;
        }
        Ok((200, json!({"ok": true})))
    }

    /// Drop the shared physical index once no db declares this field list. A concurrent
    /// `_index` create can race this and lose the index; queries then scan, results stay correct.
    async fn drop_unused_index(&self, fields: &[String]) -> Result<()> {
        if fields.is_empty() {
            return Ok(());
        }
        let fields_json = serde_json::to_string(fields).unwrap();
        let r = self
            .store
            .batch(vec![Stmt::new("SELECT COUNT(*) AS n FROM indexes WHERE fields=?", vec![Val::Text(fields_json)])])
            .await?;
        if int(&r[0][0], 0)? == 0 {
            self.store.batch(vec![Stmt::new(format!("DROP INDEX IF EXISTS {}", index_ident(fields)), vec![])]).await?;
        }
        Ok(())
    }

    async fn db_info(&self, p: &Principal, db: &str) -> Result<(u16, Value)> {
        validate::db_name(db)?;
        self.require(p, db, Access::Read)?;
        let r = self
            .store
            .batch(vec![
                db_exists_stmt(db),
                Stmt::new("SELECT COALESCE((SELECT n FROM db_counts WHERE db=?), 0) AS n", vec![db.into()]),
                seq_stmt(),
            ])
            .await?;
        if r[0].is_empty() {
            return Err(Error::NotFound("Database does not exist.".into()));
        }
        Ok((200, json!({"db_name": db, "doc_count": int(&r[1][0], 0)?, "update_seq": int(&r[2][0], 0)?})))
    }

    // ---- documents -------------------------------------------------------

    async fn get_doc(&self, p: &Principal, db: &str, id: &str) -> Result<(u16, Out)> {
        validate::db_name(db)?;
        validate::doc_id(id)?;
        self.require(p, db, Access::Read)?;
        let r = self
            .store
            .batch(vec![
                db_exists_stmt(db),
                Stmt::new("SELECT rev, deleted, body FROM docs WHERE db=? AND id=?", vec![db.into(), id.into()]),
            ])
            .await?;
        if r[0].is_empty() {
            return Err(Error::NotFound("Database does not exist.".into()));
        }
        match r[1].first() {
            Some(row) if int(row, 1)? == 0 => Ok((200, Out::Raw(render_raw(id, int(row, 0)?, &text(row, 2)?)?))),
            Some(_) => Err(Error::NotFound("deleted".into())),
            None => Err(Error::NotFound("missing".into())),
        }
    }

    async fn put_doc(&self, p: &Principal, db: &str, id: &str, q: &Query, req: &Request) -> Result<(u16, Value)> {
        let doc = parse_obj(req)?;
        let rev = match (q.get("rev"), doc.get("_rev")) {
            (Some(s), _) => Some(parse_rev(s)?),
            (None, Some(Value::String(s))) => Some(parse_rev(s)?),
            (None, Some(Value::Number(n))) => n.as_u64().map(Rev),
            (None, Some(_)) => return Err(Error::BadRequest("_rev must be a string".into())),
            (None, None) => None,
        };
        let c = self.commit(p, Txn {
            ops: vec![Op::Put { db: db.into(), id: id.into(), doc: Value::Object(doc), rev }],
            ..Default::default()
        }).await?;
        Ok((201, json!({"ok": true, "id": id, "rev": c.results[0].2.to_string()})))
    }

    async fn post_doc(&self, p: &Principal, db: &str, req: &Request) -> Result<(u16, Value)> {
        let doc = parse_obj(req)?;
        let id = match doc.get("_id") {
            Some(Value::String(s)) => s.clone(),
            Some(_) => return Err(Error::BadRequest("_id must be a string".into())),
            None => (self.new_id)(),
        };
        let c = self.commit(p, Txn {
            ops: vec![Op::Put { db: db.into(), id: id.clone(), doc: Value::Object(doc), rev: None }],
            ..Default::default()
        }).await?;
        Ok((201, json!({"ok": true, "id": id, "rev": c.results[0].2.to_string()})))
    }

    async fn delete_doc(&self, p: &Principal, db: &str, id: &str, q: &Query) -> Result<(u16, Value)> {
        let rev = q.get("rev").map(parse_rev).transpose()?.ok_or_else(|| {
            Error::Conflict("Document update conflict: ?rev= is required.".into())
        })?;
        let c = self.commit(p, Txn {
            ops: vec![Op::Delete { db: db.into(), id: id.into(), rev }],
            ..Default::default()
        }).await?;
        Ok((200, json!({"ok": true, "id": id, "rev": c.results[0].2.to_string()})))
    }

    // ---- transactions ----------------------------------------------------

    async fn txn_endpoint(&self, p: &Principal, req: &Request) -> Result<(u16, Value)> {
        let txn: Txn = serde_json::from_slice(&req.body)
            .map_err(|e| Error::BadRequest(format!("invalid transaction: {e}")))?;
        let c = self.commit(p, txn).await?;
        let results: Vec<Value> = c
            .results
            .iter()
            .map(|(db, id, rev)| json!({"db": db, "id": id, "rev": rev.to_string()}))
            .collect();
        Ok((200, json!({"ok": true, "seq": c.seq, "results": results})))
    }

    /// Validate the read set and guards, then apply all writes atomically.
    pub async fn commit(&self, p: &Principal, txn: Txn) -> Result<Committed> {
        if txn.ops.len() > MAX_OPS || txn.reads.len() > MAX_READS || txn.guards.len() > MAX_GUARDS {
            return Err(Error::BadRequest(format!(
                "transaction too large (max {MAX_OPS} ops, {MAX_READS} reads, {MAX_GUARDS} guards)"
            )));
        }

        // Validate + authorize.
        let mut dbs: Vec<&str> = vec![];
        for r in &txn.reads {
            validate::db_name(&r.db)?;
            validate::doc_id(&r.id)?;
            self.require(p, &r.db, Access::Read)?;
            dbs.push(&r.db);
        }
        for g in &txn.guards {
            let Guard::Db { db, .. } = g;
            validate::db_name(db)?;
            self.require(p, db, Access::Read)?;
            dbs.push(db);
        }
        let mut seen = std::collections::HashSet::new();
        let mut bodies: Vec<Option<String>> = vec![];
        for op in &txn.ops {
            let (db, id) = op.target();
            validate::db_name(db)?;
            validate::doc_id(id)?;
            self.require(p, db, Access::Write)?;
            if !seen.insert((db, id)) {
                return Err(Error::BadRequest(format!("duplicate op for {db}/{id}")));
            }
            dbs.push(db);
            bodies.push(match op {
                Op::Put { id, doc, .. } => Some(store_body(id, doc)?),
                Op::Delete { .. } => None,
            });
        }
        dbs.sort_unstable();
        dbs.dedup();

        let mut stmts: Vec<Stmt> = vec![];
        for d in &dbs {
            stmts.push(guard_stmt("EXISTS(SELECT 1 FROM dbs WHERE name=?)", vec![(*d).into()]));
        }
        for r in &txn.reads {
            stmts.push(guard_live_rev(&r.db, &r.id, r.rev));
        }
        for g in &txn.guards {
            let Guard::Db { db, seq } = g;
            stmts.push(guard_stmt(
                "NOT EXISTS(SELECT 1 FROM docs WHERE db=? AND seq>?)",
                vec![db.as_str().into(), Val::Int(*seq as i64)],
            ));
        }
        for op in &txn.ops {
            match op {
                Op::Put { db, id, rev, .. } => stmts.push(guard_live_rev(db, id, *rev)),
                Op::Delete { db, id, rev } => stmts.push(guard_live_rev(db, id, Some(*rev))),
            }
        }
        let has_guards = !stmts.is_empty();
        let mut upsert_idx: Vec<usize> = vec![];

        if !txn.ops.is_empty() {
            stmts.push(Stmt::new("UPDATE meta SET v=v+1 WHERE k='seq'", vec![]));
            for (op, body) in txn.ops.iter().zip(bodies) {
                let (db, id) = op.target();
                let (deleted, body) = match body {
                    Some(b) => (0, b),
                    None => (1, "{}".to_string()),
                };
                upsert_idx.push(stmts.len());
                stmts.push(Stmt::new(
                    "INSERT INTO docs(db,id,rev,seq,deleted,body) \
                     VALUES(?,?,1,(SELECT v FROM meta WHERE k='seq'),?,?) \
                     ON CONFLICT(db,id) DO UPDATE SET rev=docs.rev+1, seq=excluded.seq, \
                     deleted=excluded.deleted, body=excluded.body \
                     RETURNING rev AS rev",
                    vec![db.into(), id.into(), Val::Int(deleted), Val::Text(body)],
                ));
            }
        }
        let seq_idx = stmts.len();
        stmts.push(seq_stmt());
        if has_guards {
            stmts.push(Stmt::new("DELETE FROM _guard", vec![]));
        }

        let r = match self.store.batch(stmts).await {
            // A missing db trips the same guard as a stale rev. Only on failure, pay one
            // extra round trip to tell 404 from 409.
            Err(Error::Conflict(m)) => {
                if !dbs.is_empty() {
                    let ex = self.store.batch(dbs.iter().map(|d| db_exists_stmt(d)).collect()).await?;
                    if let Some(i) = ex.iter().position(|rows| rows.is_empty()) {
                        return Err(Error::NotFound(format!("Database '{}' does not exist.", dbs[i])));
                    }
                }
                return Err(Error::Conflict(m));
            }
            other => other?,
        };
        let mut results = vec![];
        for (i, op) in txn.ops.iter().enumerate() {
            let (db, id) = op.target();
            results.push((db.to_string(), id.to_string(), int(&r[upsert_idx[i]][0], 0)?));
        }
        Ok(Committed { seq: int(&r[seq_idx][0], 0)?, results })
    }

    // ---- queries ---------------------------------------------------------

    async fn all_docs(&self, p: &Principal, db: &str, q: &Query) -> Result<(u16, Out)> {
        validate::db_name(db)?;
        self.require(p, db, Access::Read)?;
        let limit = q.num("limit", 25, 1000)?;
        let skip = q.num("skip", 0, u64::MAX >> 2)?;
        check_skip(skip)?;
        let include = q.flag("include_docs");
        let desc = q.flag("descending");

        let cols = if include { "id, rev, body" } else { "id, rev, NULL AS body" };
        let mut sql = format!("SELECT {cols} FROM docs WHERE db=? AND deleted=0");
        let mut params: Vec<Val> = vec![db.into()];
        if let Some(k) = q.first(&["startkey", "start_key"]) {
            sql.push_str(" AND id>=?");
            params.push(k.into());
        }
        if let Some(k) = q.first(&["endkey", "end_key"]) {
            sql.push_str(" AND id<=?");
            params.push(k.into());
        }
        sql.push_str(if desc { " ORDER BY id DESC" } else { " ORDER BY id ASC" });
        sql.push_str(" LIMIT ? OFFSET ?");
        params.push(Val::Int(limit as i64));
        params.push(Val::Int(skip as i64));

        let r = self.store.batch(vec![seq_stmt(), db_exists_stmt(db), Stmt::new(sql, params)]).await?;
        if r[1].is_empty() {
            return Err(Error::NotFound("Database does not exist.".into()));
        }
        let mut out = format!("{{\"offset\":{skip},\"rows\":[");
        for (i, row) in r[2].iter().enumerate() {
            let (id, rev) = (text(row, 0)?, int(row, 1)?);
            if i > 0 {
                out.push(',');
            }
            let j = json_str(&id);
            out.push_str(&format!("{{\"id\":{j},\"key\":{j},\"value\":{{\"rev\":\"{rev}\"}}"));
            if include {
                out.push_str(",\"doc\":");
                out.push_str(&render_raw(&id, rev, &text(row, 2)?)?);
            }
            out.push('}');
        }
        out.push_str(&format!("],\"update_seq\":{}}}", int(&r[0][0], 0)?));
        Ok((200, Out::Raw(out)))
    }

    async fn find(&self, p: &Principal, db: &str, req: &Request) -> Result<(u16, Out)> {
        validate::db_name(db)?;
        self.require(p, db, Access::Read)?;
        let body = parse_obj(req)?;
        let sel = body.get("selector").ok_or_else(|| Error::BadRequest("selector is required".into()))?;
        let c = mango::compile(sel)?;
        let limit = body.get("limit").map_or(Ok(25), |v| as_count(v, "limit"))?.min(1000);
        let skip = body.get("skip").map_or(Ok(0), |v| as_count(v, "skip"))?;
        check_skip(skip)?;
        let order = match body.get("sort") {
            Some(s) => format!("{},id ASC", mango::order_by(s)?),
            None => "id ASC".to_string(),
        };

        // db is validated [a-z0-9_-] and inlined; shared indexes lead with `db`, so a literal lets the planner seek.
        let sql = format!(
            "SELECT id, rev, body FROM docs WHERE db='{db}' AND deleted=0 AND {} ORDER BY {order} LIMIT ? OFFSET ?",
            c.sql
        );
        let mut params = c.params.clone();
        params.push(Val::Int(limit as i64));
        params.push(Val::Int(skip as i64));

        let r = self
            .store
            .batch(vec![
                seq_stmt(),
                db_exists_stmt(db),
                Stmt::new("SELECT fields FROM indexes WHERE db=?", vec![db.into()]),
                Stmt::new(sql, params),
            ])
            .await?;
        if r[1].is_empty() {
            return Err(Error::NotFound("Database does not exist.".into()));
        }
        let mut out = String::from("{\"docs\":[");
        for (i, row) in r[3].iter().enumerate() {
            if i > 0 {
                out.push(',');
            }
            out.push_str(&render_raw(&text(row, 0)?, int(row, 1)?, &text(row, 2)?)?);
        }
        out.push_str(&format!("],\"update_seq\":{}", int(&r[0][0], 0)?));
        // `{}` compiles to "1": nothing to index. Any other selector that no index covers is a scan.
        if c.sql != "1" {
            let mut covered = false;
            for row in &r[2] {
                let f: Vec<String> = serde_json::from_str(&text(row, 0)?).unwrap_or_default();
                covered |= f.first().is_some_and(|first| c.fields.contains(first));
            }
            if !covered {
                out.push_str(",\"warning\":");
                out.push_str(&json_str("No matching index found, falling back to a scan of the database."));
            }
        }
        out.push('}');
        Ok((200, Out::Raw(out)))
    }

    async fn changes(&self, p: &Principal, db: &str, q: &Query) -> Result<(u16, Out)> {
        validate::db_name(db)?;
        self.require(p, db, Access::Read)?;
        let since = q.num("since", 0, i64::MAX as u64)? as i64;
        let limit = q.num("limit", 100, 1000)?;
        let include = q.flag("include_docs");

        // One query: take the first `limit` rows, then extend to the end of the last row's commit
        // (a commit can touch many docs under one seq; never split one across pages).
        let body_col = if include { "body" } else { "NULL AS body" };
        let sql = format!(
            "SELECT seq, id, rev, deleted, {body_col} FROM docs WHERE db=?1 AND seq>?2 \
             AND seq <= COALESCE((SELECT seq FROM docs WHERE db=?1 AND seq>?2 ORDER BY seq LIMIT 1 OFFSET ?3), 9223372036854775807) \
             ORDER BY seq, id"
        );
        let offset = limit.max(1) as i64 - 1;
        let r = self
            .store
            .batch(vec![
                db_exists_stmt(db),
                Stmt::new(sql, vec![db.into(), Val::Int(since), Val::Int(offset)]),
                seq_stmt(),
            ])
            .await?;
        if r[0].is_empty() {
            return Err(Error::NotFound("Database does not exist.".into()));
        }
        let rows: &[Row] = if limit == 0 { &[] } else { &r[1] };
        let mut out = String::from("{\"results\":[");
        for (i, row) in rows.iter().enumerate() {
            let (seq, id, rev, deleted) = (int(row, 0)?, text(row, 1)?, int(row, 2)?, int(row, 3)? != 0);
            if i > 0 {
                out.push(',');
            }
            out.push_str(&format!("{{\"seq\":{seq},\"id\":{},\"changes\":[{{\"rev\":\"{rev}\"}}]", json_str(&id)));
            if deleted {
                out.push_str(",\"deleted\":true");
            } else if include {
                out.push_str(",\"doc\":");
                out.push_str(&render_raw(&id, rev, &text(row, 4)?)?);
            }
            out.push('}');
        }
        let last_seq = match rows.last() {
            Some(row) => int(row, 0)?,
            None => int(&r[2][0], 0)?.max(since),
        };
        out.push_str(&format!("],\"last_seq\":{last_seq}}}"));
        Ok((200, Out::Raw(out)))
    }

    // ---- indexes ---------------------------------------------------------

    async fn list_indexes(&self, p: &Principal, db: &str) -> Result<(u16, Value)> {
        validate::db_name(db)?;
        self.require(p, db, Access::Read)?;
        let r = self
            .store
            .batch(vec![
                db_exists_stmt(db),
                Stmt::new("SELECT name, fields FROM indexes WHERE db=? ORDER BY name", vec![db.into()]),
            ])
            .await?;
        if r[0].is_empty() {
            return Err(Error::NotFound("Database does not exist.".into()));
        }
        let mut out = vec![];
        for row in &r[1] {
            let fields: Value = serde_json::from_str(&text(row, 1)?).unwrap_or(Value::Null);
            out.push(json!({"name": text(row, 0)?, "fields": fields}));
        }
        Ok((200, json!({"indexes": out})))
    }

    async fn create_index(&self, p: &Principal, db: &str, req: &Request) -> Result<(u16, Value)> {
        validate::db_name(db)?;
        self.require(p, db, Access::Admin)?;
        let body = parse_obj(req)?;
        let fields: Vec<String> = body
            .get("fields")
            .and_then(Value::as_array)
            .filter(|a| !a.is_empty() && a.len() <= 8)
            .and_then(|a| a.iter().map(|v| v.as_str().map(str::to_string)).collect())
            .ok_or_else(|| Error::BadRequest("fields must be an array of 1-8 strings".into()))?;
        for f in &fields {
            validate::field_path(f)?;
        }
        let name = match body.get("name") {
            Some(Value::String(n)) => n.clone(),
            Some(_) => return Err(Error::BadRequest("name must be a string".into())),
            None => fields.join("_").replace('.', "_"),
        };
        validate::index_name(&name)?;
        let fields_json = serde_json::to_string(&fields).unwrap();

        let r = self
            .store
            .batch(vec![
                db_exists_stmt(db),
                Stmt::new("SELECT fields FROM indexes WHERE db=? AND name=?", vec![db.into(), name.as_str().into()]),
            ])
            .await?;
        if r[0].is_empty() {
            return Err(Error::NotFound("Database does not exist.".into()));
        }
        if let Some(row) = r[1].first() {
            return if text(row, 0)? == fields_json {
                Ok((200, json!({"result": "exists", "name": name})))
            } else {
                Err(Error::Conflict("An index with this name and different fields exists.".into()))
            };
        }
        let mut exprs: Vec<String> = vec!["db".into()];
        exprs.extend(fields.iter().map(|f| json_expr(f)));
        self.store
            .batch(vec![
                Stmt::new(
                    format!(
                        "CREATE INDEX IF NOT EXISTS {} ON docs({}) WHERE deleted=0",
                        index_ident(&fields),
                        exprs.join(",")
                    ),
                    vec![],
                ),
                Stmt::new(
                    "INSERT OR REPLACE INTO indexes(db,name,fields) VALUES(?,?,?)",
                    vec![db.into(), name.as_str().into(), Val::Text(fields_json)],
                ),
            ])
            .await?;
        Ok((201, json!({"result": "created", "name": name})))
    }

    async fn delete_index(&self, p: &Principal, db: &str, name: &str) -> Result<(u16, Value)> {
        validate::db_name(db)?;
        validate::index_name(name)?;
        self.require(p, db, Access::Admin)?;
        let r = self
            .store
            .batch(vec![
                Stmt::new("SELECT fields FROM indexes WHERE db=? AND name=?", vec![db.into(), name.into()]),
                Stmt::new(format!("DROP INDEX IF EXISTS {}", legacy_index_ident(db, name)), vec![]),
                Stmt::new("DELETE FROM indexes WHERE db=? AND name=?", vec![db.into(), name.into()]),
            ])
            .await?;
        let Some(row) = r[0].first() else {
            return Err(Error::NotFound("Index does not exist.".into()));
        };
        let fields: Vec<String> = serde_json::from_str(&text(row, 0)?).unwrap_or_default();
        self.drop_unused_index(&fields).await?;
        Ok((200, json!({"ok": true})))
    }
}

fn parse_rev(s: &str) -> Result<Rev> {
    s.parse::<u64>().map(Rev).map_err(|_| Error::BadRequest("rev must be a positive integer string".into()))
}

fn as_count(v: &Value, name: &str) -> Result<u64> {
    v.as_u64().ok_or_else(|| Error::BadRequest(format!("{name} must be a non-negative integer")))
}

/// Serializes an object's fields minus `_id`/`_rev`, borrowing instead of cloning.
struct Clean<'a>(&'a Map<String, Value>);

impl Serialize for Clean<'_> {
    fn serialize<S: Serializer>(&self, ser: S) -> std::result::Result<S::Ok, S::Error> {
        let mut m = ser.serialize_map(None)?;
        for (k, v) in self.0 {
            if !k.starts_with('_') {
                m.serialize_entry(k, v)?;
            }
        }
        m.end()
    }
}

fn check_skip(skip: u64) -> Result<()> {
    if skip > MAX_SKIP {
        return Err(Error::BadRequest(format!("skip too large (max {MAX_SKIP}); page with startkey or since")));
    }
    Ok(())
}

/// Validate reserved keys, enforce size, serialize without `_id`/`_rev`.
fn store_body(id: &str, doc: &Value) -> Result<String> {
    let obj = doc.as_object().ok_or_else(|| Error::BadRequest("doc must be a JSON object".into()))?;
    for (k, v) in obj {
        match k.as_str() {
            "_id" => {
                if v.as_str() != Some(id) {
                    return Err(Error::BadRequest("_id in body does not match document id".into()));
                }
            }
            "_rev" => {}
            k if k.starts_with('_') => {
                return Err(Error::BadRequest(format!("Field '{k}' is reserved: top-level names must not start with _.")));
            }
            _ => {}
        }
    }
    let s = serde_json::to_string(&Clean(obj)).map_err(|e| Error::Internal(e.to_string()))?;
    if s.len() > MAX_DOC_BYTES {
        return Err(Error::PayloadTooLarge);
    }
    Ok(s)
}
