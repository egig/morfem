//! Throughput smoke bench. Run: cargo test -p morfem-sqlite --release --test bench -- --ignored --nocapture
use morfem_engine::auth::Open;
use morfem_engine::{Engine, Request};
use morfem_sqlite::SqliteStore;
use serde_json::{json, Value};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

fn gen_id() -> String {
    static N: AtomicU64 = AtomicU64::new(0);
    format!("gen{}", N.fetch_add(1, Ordering::Relaxed))
}

fn call(e: &Engine<SqliteStore>, method: &str, url: &str, body: Option<Value>) -> u16 {
    let (path, query) = url.split_once('?').unwrap_or((url, ""));
    pollster::block_on(e.handle(Request {
        method: method.into(),
        path: path.into(),
        query: query.into(),
        bearer: None,
        body: body.map(|b| serde_json::to_vec(&b).unwrap()).unwrap_or_default(),
    }))
    .status
}

fn report(name: &str, n: usize, t: Instant) {
    let s = t.elapsed().as_secs_f64();
    println!("{name:<28} {n:>6} ops  {s:>7.3}s  {:>9.0} ops/s", n as f64 / s);
}

#[test]
#[ignore]
fn bench() {
    let path = std::env::temp_dir().join(format!("morfem-bench-{}.db", std::process::id()));
    let e = Arc::new(Engine::new(SqliteStore::open(&path).unwrap(), Box::new(Open), gen_id));
    assert_eq!(call(&e, "PUT", "/b", None), 201);

    let n = 2000;
    let t = Instant::now();
    for i in 0..n {
        let s = call(&e, "PUT", &format!("/b/d{i}"), Some(json!({"n": i, "pad": "x".repeat(2000)})));
        assert_eq!(s, 201);
    }
    report("put (serial)", n, t);

    let t = Instant::now();
    for i in 0..n {
        assert_eq!(call(&e, "GET", &format!("/b/d{i}"), None), 200);
    }
    report("get (serial)", n, t);

    let t = Instant::now();
    for _ in 0..200 {
        assert_eq!(call(&e, "GET", "/b/_all_docs?limit=100", None), 200);
    }
    report("_all_docs limit=100", 200, t);

    let t = Instant::now();
    for _ in 0..200 {
        assert_eq!(call(&e, "GET", "/b/_changes?limit=100", None), 200);
    }
    report("_changes limit=100", 200, t);

    let threads = 8;
    let per = 1000;
    let t = Instant::now();
    let hs: Vec<_> = (0..threads)
        .map(|k| {
            let e = e.clone();
            std::thread::spawn(move || {
                for i in 0..per {
                    assert_eq!(call(&e, "GET", &format!("/b/d{}", (i * 7 + k) % 2000), None), 200);
                }
            })
        })
        .collect();
    for h in hs {
        h.join().unwrap();
    }
    report("get (8 threads)", threads * per, t);

    drop(e);
    for ext in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{ext}", path.display()));
    }
}

#[test]
#[ignore]
fn bench_many_indexes() {
    for dbs in [1usize, 100, 500] {
        let path = std::env::temp_dir().join(format!("morfem-bench-idx-{}-{dbs}.db", std::process::id()));
        let e = Engine::new(SqliteStore::open(&path).unwrap(), Box::new(Open), gen_id);
        for d in 0..dbs {
            assert_eq!(call(&e, "PUT", &format!("/d{d}"), None), 201);
            for f in ["a", "b"] {
                assert_eq!(call(&e, "POST", &format!("/d{d}/_index"), Some(json!({"fields": [f]}))), 201);
            }
        }
        let n = 3000;
        let t = Instant::now();
        for i in 0..n {
            assert_eq!(call(&e, "PUT", &format!("/d0/k{i}"), Some(json!({"a": i, "b": i}))), 201);
        }
        report(&format!("put, {} partial indexes", dbs * 2), n, t);
        drop(e);
        for ext in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{ext}", path.display()));
        }
    }
}
