//! JS bindings for morfem-engine, consumed by the npm package in `packages/morfem`.
//! The JS side supplies the storage as one function: `(stmtsJson: string) => Promise<string>`.

use morfem_engine::auth::{Authenticator, Open, StaticKeys};
use morfem_engine::{Engine, Error, Row, Stmt, Store, Val};
use js_sys::{Function, Object, Promise, Reflect, Uint8Array};
use serde_json::Value;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;

struct JsStore(Function);

fn store_err(e: impl ToString) -> Error {
    Error::from_store(e.to_string())
}

fn js_err(e: JsValue) -> Error {
    // Keep the message: a failed guard must still carry `morfem_conflict`.
    let msg = Reflect::get(&e, &"message".into()).ok().and_then(|m| m.as_string()).or_else(|| e.as_string());
    store_err(msg.unwrap_or_else(|| "store error".into()))
}

fn val_json(v: &Val) -> Value {
    match v {
        Val::Null => Value::Null,
        Val::Int(i) => Value::from(*i),
        Val::Real(f) => Value::from(*f),
        Val::Text(s) => Value::from(s.as_str()),
    }
}

fn json_val(v: Value) -> Val {
    match v {
        Value::Null => Val::Null,
        Value::Bool(b) => Val::Int(b as i64),
        Value::Number(n) => n.as_i64().map(Val::Int).unwrap_or_else(|| Val::Real(n.as_f64().unwrap_or(0.0))),
        Value::String(s) => Val::Text(s),
        other => Val::Text(other.to_string()),
    }
}

impl Store for JsStore {
    async fn batch(&self, stmts: Vec<Stmt>) -> morfem_engine::Result<Vec<Vec<Row>>> {
        let req: Vec<Value> = stmts
            .iter()
            .map(|s| serde_json::json!({ "sql": s.sql, "params": s.params.iter().map(val_json).collect::<Vec<_>>() }))
            .collect();
        let arg = JsValue::from_str(&Value::Array(req).to_string());
        let ret = self.0.call1(&JsValue::NULL, &arg).map_err(js_err)?;
        let res = JsFuture::from(Promise::resolve(&ret)).await.map_err(js_err)?;
        let text = res.as_string().ok_or_else(|| store_err("store returned a non-string"))?;
        let parsed: Vec<Vec<Vec<Value>>> = serde_json::from_str(&text).map_err(store_err)?;
        Ok(parsed.into_iter().map(|rows| rows.into_iter().map(|r| r.into_iter().map(json_val).collect()).collect()).collect())
    }
}

fn new_id() -> String {
    let go = || -> Option<String> {
        let crypto = Reflect::get(&js_sys::global(), &"crypto".into()).ok()?;
        let f = Reflect::get(&crypto, &"randomUUID".into()).ok()?.dyn_into::<Function>().ok()?;
        f.call0(&crypto).ok()?.as_string()
    };
    go().unwrap_or_default().replace('-', "")
}

#[wasm_bindgen]
pub struct Morfem {
    engine: Engine<JsStore>,
}

#[wasm_bindgen]
impl Morfem {
    /// `keys_json`: MORFEM_KEYS-shaped JSON array. `no_auth`: everyone is admin (development only).
    #[wasm_bindgen(constructor)]
    pub fn new(batch: Function, keys_json: Option<String>, no_auth: bool) -> Result<Morfem, JsError> {
        let auth: Box<dyn Authenticator + Send + Sync> = if no_auth {
            Box::new(Open)
        } else {
            match keys_json {
                Some(k) => Box::new(StaticKeys::from_json(&k).map_err(|e| JsError::new(&e))?),
                None => Box::new(StaticKeys::deny_all()),
            }
        };
        Ok(Morfem { engine: Engine::new(JsStore(batch), auth, new_id) })
    }

    /// Apply the schema. Idempotent.
    pub async fn init(&self) -> Result<(), JsError> {
        self.engine.init().await.map_err(|e| JsError::new(&format!("{e:?}")))
    }

    /// Returns `{ status, contentType, body: Uint8Array }`.
    pub async fn handle(
        &self,
        method: String,
        path: String,
        query: String,
        bearer: Option<String>,
        body: Vec<u8>,
    ) -> Result<JsValue, JsError> {
        let out = self.engine.handle(morfem_engine::Request { method, path, query, bearer, body }).await;
        let obj = Object::new();
        let set = |k: &str, v: JsValue| Reflect::set(&obj, &k.into(), &v).map(|_| ()).map_err(|_| JsError::new("reflect"));
        set("status", JsValue::from_f64(out.status as f64))?;
        set("contentType", JsValue::from_str(out.content_type))?;
        set("body", Uint8Array::from(out.body.as_slice()).into())?;
        Ok(obj.into())
    }
}
