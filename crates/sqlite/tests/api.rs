use morfem_engine::auth::{Open, StaticKeys};
use morfem_engine::{Engine, Request};
use morfem_sqlite::SqliteStore;
use serde_json::{json, Value};
use std::sync::atomic::{AtomicU64, Ordering};

fn gen_id() -> String {
    static N: AtomicU64 = AtomicU64::new(0);
    format!("gen{}", N.fetch_add(1, Ordering::Relaxed))
}

fn engine() -> Engine<SqliteStore> {
    Engine::new(SqliteStore::memory().unwrap(), Box::new(Open), gen_id)
}

fn call(e: &Engine<SqliteStore>, method: &str, url: &str, body: Option<Value>) -> (u16, Value) {
    call_as(e, None, method, url, body)
}

fn call_as(e: &Engine<SqliteStore>, token: Option<&str>, method: &str, url: &str, body: Option<Value>) -> (u16, Value) {
    let (path, query) = url.split_once('?').unwrap_or((url, ""));
    let r = pollster::block_on(e.handle(Request {
        method: method.into(),
        path: path.into(),
        query: query.into(),
        bearer: token.map(str::to_string),
        body: body.map(|b| serde_json::to_vec(&b).unwrap()).unwrap_or_default(),
    }));
    (r.status, serde_json::from_slice(&r.body).unwrap_or(Value::Null))
}

fn setup() -> Engine<SqliteStore> {
    let e = engine();
    assert_eq!(call(&e, "PUT", "/notes", None).0, 201);
    e
}

#[test]
fn db_lifecycle() {
    let e = engine();
    assert_eq!(call(&e, "PUT", "/a", None).0, 201);
    assert_eq!(call(&e, "PUT", "/a", None).0, 412);
    assert_eq!(call(&e, "PUT", "/Bad Name", None).0, 400);
    assert_eq!(call(&e, "GET", "/_all_dbs", None).1, json!(["a"]));
    let (s, info) = call(&e, "GET", "/a", None);
    assert_eq!((s, info["doc_count"].clone()), (200, json!(0)));
    assert_eq!(call(&e, "DELETE", "/a", None).0, 200);
    assert_eq!(call(&e, "DELETE", "/a", None).0, 404);
    assert_eq!(call(&e, "GET", "/a", None).0, 404);
}

#[test]
fn doc_crud_and_revisions() {
    let e = setup();
    let (s, r) = call(&e, "PUT", "/notes/n1", Some(json!({"title": "hi"})));
    assert_eq!((s, r["rev"].clone()), (201, json!("1")));

    // create-only: second PUT without rev conflicts
    assert_eq!(call(&e, "PUT", "/notes/n1", Some(json!({"title": "x"}))).0, 409);

    let (_, d) = call(&e, "GET", "/notes/n1", None);
    assert_eq!(d, json!({"_id": "n1", "_rev": "1", "title": "hi"}));

    // update with rev in body
    let (s, r) = call(&e, "PUT", "/notes/n1", Some(json!({"_rev": "1", "title": "v2"})));
    assert_eq!((s, r["rev"].clone()), (201, json!("2")));
    // stale rev via query
    assert_eq!(call(&e, "PUT", "/notes/n1?rev=1", Some(json!({"title": "v3"}))).0, 409);

    // delete
    assert_eq!(call(&e, "DELETE", "/notes/n1", None).0, 409);
    let (s, r) = call(&e, "DELETE", "/notes/n1?rev=2", None);
    assert_eq!((s, r["rev"].clone()), (200, json!("3")));
    let (s, r) = call(&e, "GET", "/notes/n1", None);
    assert_eq!((s, r["reason"].clone()), (404, json!("deleted")));

    // recreate after delete continues the rev counter
    let (s, r) = call(&e, "PUT", "/notes/n1", Some(json!({"title": "again"})));
    assert_eq!((s, r["rev"].clone()), (201, json!("4")));
}

#[test]
fn post_generates_id_and_rejects_reserved() {
    let e = setup();
    let (s, r) = call(&e, "POST", "/notes", Some(json!({"a": 1})));
    assert_eq!((s, r["id"].clone()), (201, json!("gen0")));
    assert_eq!(call(&e, "PUT", "/notes/x", Some(json!({"_bad": 1}))).0, 400);
    assert_eq!(call(&e, "PUT", "/notes/_x", Some(json!({}))).0, 404);
    assert_eq!(call(&e, "PUT", "/nodb/x", Some(json!({}))).0, 404);
    assert_eq!(call(&e, "PUT", "/notes/x", Some(json!([1]))).0, 400);
}

#[test]
fn txn_is_atomic_and_validates_reads() {
    let e = setup();
    call(&e, "PUT", "/notes/a", Some(json!({"bal": 100})));
    call(&e, "PUT", "/notes/b", Some(json!({"bal": 0})));

    // transfer with correct read set
    let (s, r) = call(&e, "POST", "/_txn", Some(json!({
        "reads": [{"db":"notes","id":"a","rev":"1"}, {"db":"notes","id":"b","rev":1}],
        "ops": [
            {"op":"put","db":"notes","id":"a","rev":"1","doc":{"bal":50}},
            {"op":"put","db":"notes","id":"b","rev":"1","doc":{"bal":50}}
        ]
    })));
    assert_eq!(s, 200, "{r}");
    assert_eq!(r["results"][0]["rev"], json!("2"));
    let seq = r["seq"].as_i64().unwrap();

    // stale read set => 409, and nothing is applied
    let (s, _) = call(&e, "POST", "/_txn", Some(json!({
        "reads": [{"db":"notes","id":"a","rev":"1"}],
        "ops": [
            {"op":"put","db":"notes","id":"b","rev":"2","doc":{"bal":999}},
            {"op":"put","db":"notes","id":"c","doc":{"bal":1}}
        ]
    })));
    assert_eq!(s, 409);
    assert_eq!(call(&e, "GET", "/notes/b", None).1["bal"], json!(50));
    assert_eq!(call(&e, "GET", "/notes/c", None).0, 404);

    // failing op in the middle rolls back earlier ops too
    let (s, _) = call(&e, "POST", "/_txn", Some(json!({
        "ops": [
            {"op":"put","db":"notes","id":"a","rev":"2","doc":{"bal":1}},
            {"op":"put","db":"notes","id":"b","rev":"1","doc":{"bal":1}}
        ]
    })));
    assert_eq!(s, 409);
    assert_eq!(call(&e, "GET", "/notes/a", None).1["bal"], json!(50));

    // absent-read assertion
    let (s, _) = call(&e, "POST", "/_txn", Some(json!({
        "reads": [{"db":"notes","id":"a"}], "ops": []
    })));
    assert_eq!(s, 409);

    // db guard: write after `seq` in same db fails the guard
    let (s, _) = call(&e, "POST", "/_txn", Some(json!({
        "guards": [{"kind":"db","db":"notes","seq": seq - 1}],
        "ops": [{"op":"put","db":"notes","id":"z","doc":{}}]
    })));
    assert_eq!(s, 409);
    let (s, _) = call(&e, "POST", "/_txn", Some(json!({
        "guards": [{"kind":"db","db":"notes","seq": seq}],
        "ops": [{"op":"put","db":"notes","id":"z","doc":{}}]
    })));
    assert_eq!(s, 200);

    // duplicate target
    let (s, _) = call(&e, "POST", "/_txn", Some(json!({
        "ops": [{"op":"put","db":"notes","id":"q","doc":{}}, {"op":"delete","db":"notes","id":"q","rev":"1"}]
    })));
    assert_eq!(s, 400);
}

#[test]
fn txn_spans_databases() {
    let e = setup();
    call(&e, "PUT", "/other", None);
    let (s, r) = call(&e, "POST", "/_txn", Some(json!({
        "ops": [
            {"op":"put","db":"notes","id":"x","doc":{"k":1}},
            {"op":"put","db":"other","id":"y","doc":{"k":2}}
        ]
    })));
    assert_eq!(s, 200);
    assert_eq!(r["results"][0]["db"], json!("notes"));
    assert_eq!(call(&e, "GET", "/other/y", None).0, 200);
}

fn seed_people(e: &Engine<SqliteStore>) {
    for (id, name, age) in [("p1", "ann", 30), ("p2", "bob", 25), ("p3", "cy", 41), ("p4", "di", 25)] {
        call(e, "PUT", &format!("/notes/{id}"), Some(json!({"name": name, "age": age, "addr": {"city": "x"}})));
    }
}

#[test]
fn all_docs() {
    let e = setup();
    seed_people(&e);
    let (_, r) = call(&e, "GET", "/notes/_all_docs", None);
    let ids: Vec<_> = r["rows"].as_array().unwrap().iter().map(|x| x["id"].as_str().unwrap()).collect();
    assert_eq!(ids, ["p1", "p2", "p3", "p4"]);
    let (_, r) = call(&e, "GET", "/notes/_all_docs?startkey=p2&endkey=p3&include_docs=true", None);
    assert_eq!(r["rows"].as_array().unwrap().len(), 2);
    assert_eq!(r["rows"][0]["doc"]["name"], json!("bob"));
    let (_, r) = call(&e, "GET", "/notes/_all_docs?descending=true&limit=1", None);
    assert_eq!(r["rows"][0]["id"], json!("p4"));
}

#[test]
fn find_selectors_sort_and_index() {
    let e = setup();
    seed_people(&e);
    let find = |sel: Value| -> Vec<String> {
        let (s, r) = call(&e, "POST", "/notes/_find", Some(json!({"selector": sel, "sort": ["name"]})));
        assert_eq!(s, 200, "{r}");
        r["docs"].as_array().unwrap().iter().map(|d| d["_id"].as_str().unwrap().to_string()).collect()
    };
    assert_eq!(find(json!({"age": 25})), ["p2", "p4"]);
    assert_eq!(find(json!({"age": {"$gt": 25, "$lt": 41}})), ["p1"]);
    assert_eq!(find(json!({"age": {"$gte": 30}})), ["p1", "p3"]);
    assert_eq!(find(json!({"age": {"$in": [25, 41]}})), ["p2", "p3", "p4"]);
    assert_eq!(find(json!({"age": {"$nin": [25, 41]}})), ["p1"]);
    assert_eq!(find(json!({"$or": [{"name": "ann"}, {"name": "cy"}]})), ["p1", "p3"]);
    assert_eq!(find(json!({"$and": [{"age": 25}, {"name": {"$ne": "bob"}}]})), ["p4"]);
    assert_eq!(find(json!({"$not": {"age": 25}})), ["p1", "p3"]);
    assert_eq!(find(json!({"age": {"$not": {"$gt": 25}}})), ["p2", "p4"]);
    assert_eq!(find(json!({"addr": {"city": "x"}})), ["p1", "p2", "p3", "p4"]);
    assert_eq!(find(json!({"addr.city": "x", "age": {"$exists": true}})).len(), 4);
    assert_eq!(find(json!({"nope": {"$exists": true}})).len(), 0);
    assert_eq!(find(json!({})).len(), 4);

    let (_, r) = call(&e, "POST", "/notes/_find", Some(json!({
        "selector": {"age": {"$gte": 0}}, "sort": [{"age": "desc"}], "limit": 2, "skip": 1
    })));
    let ids: Vec<_> = r["docs"].as_array().unwrap().iter().map(|d| d["_id"].as_str().unwrap()).collect();
    assert_eq!(ids, ["p1", "p2"]);

    // injection attempts are rejected, not executed
    for bad in [json!({"a'); DROP TABLE docs;--": 1}), json!({"a": {"$bogus": 1}}), json!({"a": [1]})] {
        assert_eq!(call(&e, "POST", "/notes/_find", Some(json!({"selector": bad}))).0, 400);
    }
    assert_eq!(call(&e, "POST", "/notes/_find", Some(json!({"selector": {"age": 1}, "sort": ["a'b"]}))).0, 400);

    // warning until an index exists
    let (_, r) = call(&e, "POST", "/notes/_find", Some(json!({"selector": {"age": 25}})));
    assert!(r.get("warning").is_some());
    let (s, r) = call(&e, "POST", "/notes/_index", Some(json!({"fields": ["age"]})));
    assert_eq!((s, r["name"].clone()), (201, json!("age")));
    assert_eq!(call(&e, "POST", "/notes/_index", Some(json!({"fields": ["age"]}))).0, 200);
    assert_eq!(call(&e, "POST", "/notes/_index", Some(json!({"name": "age", "fields": ["name"]}))).0, 409);
    let (_, r) = call(&e, "POST", "/notes/_find", Some(json!({"selector": {"age": 25}})));
    assert!(r.get("warning").is_none());
    assert_eq!(r["docs"].as_array().unwrap().len(), 2);
    assert_eq!(call(&e, "GET", "/notes/_index", None).1["indexes"][0]["name"], json!("age"));
    assert_eq!(call(&e, "DELETE", "/notes/_index/age", None).0, 200);
    assert_eq!(call(&e, "DELETE", "/notes/_index/age", None).0, 404);
    // db delete drops its indexes
    call(&e, "POST", "/notes/_index", Some(json!({"fields": ["name", "addr.city"]})));
    assert_eq!(call(&e, "DELETE", "/notes", None).0, 200);
    assert_eq!(call(&e, "PUT", "/notes", None).0, 201);
}

#[test]
fn index_matches_compiler_output() {
    // `INDEXED BY` errors unless the partial expression index is usable for the exact
    // SQL the compiler emits. Deterministic, unlike planner cost choices.
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    conn.execute_batch(morfem_engine::SCHEMA).unwrap();
    conn.execute_batch(
        "CREATE INDEX \"morfem:idx:age\" ON docs(db, json_extract(body,'$.age')) WHERE deleted=0;",
    )
    .unwrap();
    for sel in [json!({"age": 5}), json!({"age": {"$gt": 5}}), json!({"age": {"$in": [1, 2]}})] {
        let c = morfem_engine::mango::compile(&sel).unwrap();
        let sql = format!(
            "EXPLAIN QUERY PLAN SELECT id FROM docs INDEXED BY \"morfem:idx:age\" WHERE db='notes' AND deleted=0 AND {}",
            c.sql
        );
        let n = c.params.len();
        let params = rusqlite::params_from_iter(std::iter::repeat(5).take(n));
        let _ = conn.prepare(&sql).unwrap_or_else(|e| panic!("{sel}: index unusable: {e}")).query(params).unwrap();
    }
}

#[test]
fn changes_feed() {
    let e = setup();
    call(&e, "PUT", "/notes/a", Some(json!({"v": 1})));
    call(&e, "PUT", "/notes/b", Some(json!({"v": 1})));
    call(&e, "PUT", "/notes/a?rev=1", Some(json!({"v": 2})));
    call(&e, "DELETE", "/notes/b?rev=1", None);

    let (_, r) = call(&e, "GET", "/notes/_changes", None);
    let res = r["results"].as_array().unwrap();
    // latest state per doc, ordered by seq
    assert_eq!(res.len(), 2);
    assert_eq!(res[0]["id"], json!("a"));
    assert_eq!(res[1]["id"], json!("b"));
    assert_eq!(res[1]["deleted"], json!(true));
    let last = r["last_seq"].as_i64().unwrap();

    let (_, r) = call(&e, "GET", &format!("/notes/_changes?since={last}"), None);
    assert_eq!(r["results"].as_array().unwrap().len(), 0);
    assert_eq!(r["last_seq"], json!(last));

    let (_, r) = call(&e, "GET", "/notes/_changes?include_docs=true&limit=1", None);
    assert_eq!(r["results"][0]["doc"]["v"], json!(2));
}

#[test]
fn changes_never_split_a_commit_across_pages() {
    let e = setup();
    call(&e, "POST", "/_txn", Some(json!({"ops": [
        {"op":"put","db":"notes","id":"a","doc":{}},
        {"op":"put","db":"notes","id":"b","doc":{}},
        {"op":"put","db":"notes","id":"c","doc":{}}
    ]})));
    let (_, r) = call(&e, "GET", "/notes/_changes?limit=1", None);
    assert_eq!(r["results"].as_array().unwrap().len(), 3);
}

#[test]
fn query_seq_feeds_db_guard() {
    let e = setup();
    call(&e, "PUT", "/notes/a", Some(json!({"n": 1})));
    let (_, r) = call(&e, "POST", "/notes/_find", Some(json!({"selector": {"n": 1}})));
    let seq = r["update_seq"].as_i64().unwrap();
    // concurrent insert that would change the query result
    call(&e, "PUT", "/notes/b", Some(json!({"n": 1})));
    let (s, _) = call(&e, "POST", "/_txn", Some(json!({
        "guards": [{"kind":"db","db":"notes","seq": seq}],
        "ops": [{"op":"put","db":"notes","id":"c","doc":{}}]
    })));
    assert_eq!(s, 409);
}

#[test]
fn auth_scopes() {
    let keys = StaticKeys::from_json(
        r#"[{"key":"adminkey1","access":"admin"},
            {"key":"writekey1","access":"write","dbs":["notes"]},
            {"key":"readkey11","access":"read","dbs":["notes"]}]"#,
    )
    .unwrap();
    let e = Engine::new(SqliteStore::memory().unwrap(), Box::new(keys), gen_id);
    let c = |t: Option<&str>, m: &str, u: &str, b: Option<Value>| call_as(&e, t, m, u, b).0;

    assert_eq!(c(None, "GET", "/", None), 200);
    assert_eq!(c(None, "GET", "/_all_dbs", None), 401);
    assert_eq!(c(Some("wrong"), "GET", "/_all_dbs", None), 401);
    assert_eq!(c(Some("writekey1"), "PUT", "/notes", None), 403);
    assert_eq!(c(Some("adminkey1"), "PUT", "/notes", None), 201);
    assert_eq!(c(Some("adminkey1"), "PUT", "/secret", None), 201);
    assert_eq!(c(Some("writekey1"), "PUT", "/notes/a", Some(json!({}))), 201);
    assert_eq!(c(Some("writekey1"), "PUT", "/secret/a", Some(json!({}))), 403);
    assert_eq!(c(Some("readkey11"), "GET", "/notes/a", None), 200);
    assert_eq!(c(Some("readkey11"), "PUT", "/notes/b", Some(json!({}))), 403);
    assert_eq!(c(Some("readkey11"), "GET", "/secret/_all_docs", None), 403);
    assert_eq!(c(Some("readkey11"), "POST", "/_txn", Some(json!({"ops":[{"op":"put","db":"notes","id":"z","doc":{}}]}))), 403);
    assert_eq!(call_as(&e, Some("readkey11"), "GET", "/_all_dbs", None).1, json!(["notes"]));
    assert!(StaticKeys::from_json(r#"[{"key":"short","access":"admin"}]"#).is_err());
    // no keys configured => deny all
    let e2 = Engine::new(SqliteStore::memory().unwrap(), Box::new(StaticKeys::deny_all()), gen_id);
    assert_eq!(call_as(&e2, Some("anything1"), "GET", "/_all_dbs", None).0, 401);
}

#[test]
fn routing_edges() {
    let e = setup();
    assert_eq!(call(&e, "GET", "/notes/_nope", None).0, 404);
    assert_eq!(call(&e, "PUT", "/notes/_find", None).0, 405);
    assert_eq!(call(&e, "DELETE", "/_txn", None).0, 405);
    assert_eq!(call(&e, "PUT", "/notes/a%20b", Some(json!({"x": 1}))).0, 201);
    assert_eq!(call(&e, "GET", "/notes/a%20b", None).1["_id"], json!("a b"));
}

#[test]
fn ui_is_public_html() {
    let keys = StaticKeys::from_json(r#"[{"key":"adminkey1","access":"admin"}]"#).unwrap();
    let e = Engine::new(SqliteStore::memory().unwrap(), Box::new(keys), gen_id);
    for path in ["/_ui", "/_ui/"] {
        let r = pollster::block_on(e.handle(Request { method: "GET".into(), path: path.into(), ..Default::default() }));
        assert_eq!(r.status, 200);
        assert!(r.content_type.starts_with("text/html"));
        assert!(String::from_utf8(r.body).unwrap().contains("<title>morfem</title>"));
    }
}

#[test]
fn shared_index_survives_other_db_delete() {
    let e = engine();
    for db in ["a", "b"] {
        assert_eq!(call(&e, "PUT", &format!("/{db}"), None).0, 201);
        assert_eq!(call(&e, "POST", &format!("/{db}/_index"), Some(json!({"fields": ["age"]}))).0, 201);
        call(&e, "PUT", &format!("/{db}/x"), Some(json!({"age": 5})));
    }
    assert_eq!(call(&e, "DELETE", "/a/_index/age", None).0, 200);
    // b keeps its index and results; a is unaffected by b's rows
    let (_, r) = call(&e, "POST", "/b/_find", Some(json!({"selector": {"age": 5}})));
    assert!(r.get("warning").is_none());
    assert_eq!(r["docs"].as_array().unwrap().len(), 1);
    assert_eq!(call(&e, "DELETE", "/b", None).0, 200);
    // recreate works after last user dropped the physical index
    assert_eq!(call(&e, "PUT", "/b", None).0, 201);
    assert_eq!(call(&e, "POST", "/b/_index", Some(json!({"fields": ["age"]}))).0, 201);
}

#[test]
fn doc_count_tracks_create_delete_revive_and_db_delete() {
    let e = setup();
    let count = |e: &Engine<SqliteStore>| call(e, "GET", "/notes", None).1["doc_count"].clone();
    assert_eq!(count(&e), json!(0));
    call(&e, "PUT", "/notes/a", Some(json!({"v": 1})));
    call(&e, "PUT", "/notes/b", Some(json!({"v": 1})));
    assert_eq!(count(&e), json!(2));
    call(&e, "PUT", "/notes/a?rev=1", Some(json!({"v": 2}))); // update: no change
    assert_eq!(count(&e), json!(2));
    call(&e, "DELETE", "/notes/b?rev=1", None);
    assert_eq!(count(&e), json!(1));
    assert_eq!(call(&e, "PUT", "/notes/b", Some(json!({"v": 3}))).0, 201); // revive
    assert_eq!(count(&e), json!(2));
    assert_eq!(call(&e, "DELETE", "/notes", None).0, 200);
    assert_eq!(call(&e, "PUT", "/notes", None).0, 201);
    assert_eq!(count(&e), json!(0));
}

#[test]
fn skip_is_capped_and_raw_docs_keep_shape() {
    let e = setup();
    assert_eq!(call(&e, "GET", "/notes/_all_docs?skip=100001", None).0, 400);
    assert_eq!(call(&e, "POST", "/notes/_find", Some(json!({"selector": {}, "skip": 100001}))).0, 400);
    // spliced docs: quote/unicode ids, empty body, nested body
    call(&e, "PUT", "/notes/we%22ird", Some(json!({"t": "é\n\"q\"", "n": {"x": [1, 2]}})));
    call(&e, "PUT", "/notes/empty", Some(json!({})));
    let (s, d) = call(&e, "GET", "/notes/we%22ird", None);
    assert_eq!((s, d["_id"].clone(), d["t"].clone()), (200, json!("we\"ird"), json!("é\n\"q\"")));
    assert_eq!(call(&e, "GET", "/notes/empty", None).1, json!({"_id": "empty", "_rev": "1"}));
    let (_, r) = call(&e, "GET", "/notes/_all_docs?include_docs=true", None);
    assert_eq!(r["rows"][0]["doc"], json!({"_id": "empty", "_rev": "1"}));
    assert_eq!(r["rows"][1]["id"], json!("we\"ird"));
}

#[test]
fn find_warning_ignores_non_indexable_operators() {
    let e = setup();
    call(&e, "POST", "/notes/_index", Some(json!({"fields": ["age"]})));
    let warns = |sel: Value| call(&e, "POST", "/notes/_find", Some(json!({"selector": sel}))).1.get("warning").is_some();
    assert!(!warns(json!({"age": {"$gt": 1}})));
    assert!(warns(json!({"age": {"$ne": 1}})));
    assert!(warns(json!({"$or": [{"age": 1}, {"age": 2}]})));
    assert!(!warns(json!({"$and": [{"age": 1}]})));
}

#[test]
fn changes_limit_extends_to_commit_end_and_zero_is_empty() {
    let e = setup();
    let ops: Vec<Value> = (0..5).map(|i| json!({"op": "put", "db": "notes", "id": format!("k{i}"), "doc": {"v": i}})).collect();
    assert_eq!(call(&e, "POST", "/_txn", Some(json!({"ops": ops}))).0, 200);
    call(&e, "PUT", "/notes/z", Some(json!({"v": 1})));
    let (_, r) = call(&e, "GET", "/notes/_changes?limit=2", None);
    assert_eq!(r["results"].as_array().unwrap().len(), 5);
    let (_, r) = call(&e, "GET", "/notes/_changes?limit=0", None);
    assert_eq!(r["results"].as_array().unwrap().len(), 0);
}

fn tmp_db(tag: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!("morfem-boot-{tag}-{}.db", std::process::id()))
}

fn rm_db(p: &std::path::Path) {
    for ext in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{ext}", p.display()));
    }
}

#[test]
fn init_via_store_batch_is_idempotent() {
    // The Worker path: statements split from SCHEMA and run through Store::batch.
    let e = engine();
    pollster::block_on(e.init()).unwrap();
    pollster::block_on(e.init()).unwrap();
    assert!(morfem_engine::schema_statements().count() > 10);
    assert_eq!(call(&e, "PUT", "/a", None).0, 201);
    call(&e, "PUT", "/a/x", Some(json!({"v": 1})));
    pollster::block_on(e.init()).unwrap();
    assert_eq!(call(&e, "GET", "/a", None).1["doc_count"], json!(1));
}

#[test]
fn boot_upgrades_pre_counts_database() {
    let path = tmp_db("upgrade");
    rm_db(&path);
    {
        // Schema as released before db_counts existed, with data in it.
        let c = rusqlite::Connection::open(&path).unwrap();
        c.execute_batch(
            "CREATE TABLE docs (db TEXT NOT NULL, id TEXT NOT NULL, rev INTEGER NOT NULL, seq INTEGER NOT NULL,
               deleted INTEGER NOT NULL DEFAULT 0, body TEXT NOT NULL, PRIMARY KEY (db, id)) WITHOUT ROWID;
             CREATE TABLE dbs (name TEXT PRIMARY KEY, created_seq INTEGER NOT NULL) WITHOUT ROWID;
             CREATE TABLE meta (k TEXT PRIMARY KEY, v INTEGER NOT NULL) WITHOUT ROWID;
             INSERT INTO meta VALUES ('seq', 3);
             INSERT INTO dbs VALUES ('old', 0);
             INSERT INTO docs VALUES ('old','a',1,1,0,'{}'),('old','b',1,2,0,'{}'),('old','c',2,3,1,'{}');",
        )
        .unwrap();
    }
    let e = Engine::new(SqliteStore::open(&path).unwrap(), Box::new(Open), gen_id);
    assert_eq!(call(&e, "GET", "/old", None).1["doc_count"], json!(2)); // backfilled, tombstone excluded
    call(&e, "PUT", "/old/d", Some(json!({})));
    assert_eq!(call(&e, "GET", "/old", None).1["doc_count"], json!(3)); // triggers live
    drop(e);
    // Reboot: no double backfill, count stays exact.
    let e = Engine::new(SqliteStore::open(&path).unwrap(), Box::new(Open), gen_id);
    assert_eq!(call(&e, "GET", "/old", None).1["doc_count"], json!(3));
    drop(e);
    rm_db(&path);
}

#[test]
fn concurrent_boots_do_not_conflict() {
    let path = tmp_db("race");
    rm_db(&path);
    let hs: Vec<_> = (0..6)
        .map(|_| {
            let p = path.clone();
            std::thread::spawn(move || SqliteStore::open(&p).map(|_| ()).map_err(|e| format!("{e:?}")))
        })
        .collect();
    for h in hs {
        h.join().unwrap().unwrap();
    }
    rm_db(&path);
}
