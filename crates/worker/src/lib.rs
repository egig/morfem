//! Cloudflare Worker adapter: morfem-engine over D1.

use morfem_engine::auth::{Authenticator, StaticKeys};
use morfem_engine::{Engine, Error, Row, Stmt, Store, Val};
use serde_json::{Map, Value};
use wasm_bindgen::{JsCast, JsValue};
use worker::*;

struct D1Store(D1Database);

fn store_err(e: impl ToString) -> Error {
    Error::from_store(e.to_string())
}

fn to_js(v: &Val) -> JsValue {
    match v {
        Val::Null => JsValue::NULL,
        Val::Int(i) => JsValue::from_f64(*i as f64),
        Val::Real(f) => JsValue::from_f64(*f),
        Val::Text(s) => JsValue::from_str(s),
    }
}

fn from_json(v: Value) -> Val {
    match v {
        Value::Null => Val::Null,
        Value::Bool(b) => Val::Int(b as i64),
        Value::Number(n) => n.as_i64().map(Val::Int).unwrap_or_else(|| Val::Real(n.as_f64().unwrap_or(0.0))),
        Value::String(s) => Val::Text(s),
        // Not produced by our schema.
        other => Val::Text(other.to_string()),
    }
}

impl Store for D1Store {
    async fn batch(&self, stmts: Vec<Stmt>) -> morfem_engine::Result<Vec<Vec<Row>>> {
        let mut prepared = Vec::with_capacity(stmts.len());
        for s in &stmts {
            let args: Vec<JsValue> = s.params.iter().map(to_js).collect();
            prepared.push(self.0.prepare(&s.sql).bind(&args).map_err(store_err)?);
        }
        // D1 runs a batch as one transaction: a failing statement rolls back all of it.
        let results = self.0.batch(prepared).await.map_err(store_err)?;
        let mut out = Vec::with_capacity(results.len());
        for r in &results {
            if let Some(e) = r.error() {
                return Err(store_err(e));
            }
            // Rows arrive as objects; JS preserves column order, and our SELECTs use unique names.
            let rows: Vec<Map<String, Value>> = r.results().map_err(store_err)?;
            out.push(rows.into_iter().map(|m| m.into_iter().map(|(_, v)| from_json(v)).collect()).collect());
        }
        Ok(out)
    }
}

/// `crypto.randomUUID()` without dashes.
fn new_id() -> String {
    let go = || -> Option<String> {
        let crypto = js_sys::Reflect::get(&js_sys::global(), &"crypto".into()).ok()?;
        let f = js_sys::Reflect::get(&crypto, &"randomUUID".into()).ok()?.dyn_into::<js_sys::Function>().ok()?;
        f.call0(&crypto).ok()?.as_string()
    };
    go().unwrap_or_default().replace('-', "")
}

fn authenticator(env: &Env) -> Box<dyn Authenticator + Send + Sync> {
    // Local development only: set MORFEM_NO_AUTH=true in `.dev.vars`, never in wrangler.toml or secrets.
    let no_auth = env.secret("MORFEM_NO_AUTH").map(|s| s.to_string()).or_else(|_| env.var("MORFEM_NO_AUTH").map(|v| v.to_string()));
    if no_auth.is_ok_and(|v| v == "true") {
        return Box::new(morfem_engine::auth::Open);
    }
    // Secret in production (`wrangler secret put MORFEM_KEYS`), `.dev.vars` locally.
    let raw = env.secret("MORFEM_KEYS").map(|s| s.to_string()).or_else(|_| env.var("MORFEM_KEYS").map(|v| v.to_string()));
    match raw.map(|s| StaticKeys::from_json(&s)) {
        Ok(Ok(k)) => Box::new(k),
        Ok(Err(e)) => {
            console_error!("MORFEM_KEYS invalid: {e}");
            Box::new(StaticKeys::deny_all())
        }
        // Not configured: deny everything.
        Err(_) => Box::new(StaticKeys::deny_all()),
    }
}

thread_local! {
    /// Schema is applied once per isolate (wasm is single-threaded). Not set on failure, so the next request retries.
    static SCHEMA_READY: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

#[event(fetch)]
async fn fetch(mut req: Request, env: Env, _ctx: Context) -> Result<Response> {
    let url = req.url()?;
    let bearer = req
        .headers()
        .get("Authorization")?
        .and_then(|v| v.strip_prefix("Bearer ").map(str::to_string));
    let body = req.bytes().await?;
    let engine = Engine::new(D1Store(env.d1("DB")?), authenticator(&env), new_id);

    if !SCHEMA_READY.get() {
        if let Err(e) = engine.init().await {
            console_error!("schema init failed: {e:?}");
            return Response::error("schema init failed", 500);
        }
        SCHEMA_READY.set(true);
    }

    let out = engine
        .handle(morfem_engine::Request {
            method: req.method().to_string(),
            path: url.path().to_string(),
            query: url.query().unwrap_or("").to_string(),
            bearer,
            body,
        })
        .await;

    let content_type = out.content_type;
    let mut resp = Response::from_bytes(out.body)?.with_status(out.status);
    resp.headers_mut().set("content-type", content_type)?;
    Ok(resp)
}
