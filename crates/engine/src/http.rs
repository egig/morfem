use serde_json::Value;

/// Framework-neutral request. Adapters (axum, workers) build this.
#[derive(Debug, Clone, Default)]
pub struct Request {
    pub method: String,
    /// Raw path, percent-encoded, e.g. `/mydb/doc%201`.
    pub path: String,
    /// Raw query string without `?`.
    pub query: String,
    /// Token from `Authorization: Bearer <token>`.
    pub bearer: Option<String>,
    pub body: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct Response {
    pub status: u16,
    pub content_type: &'static str,
    pub body: Vec<u8>,
}

impl Response {
    pub fn json(status: u16, v: &Value) -> Self {
        Response { status, content_type: CONTENT_TYPE, body: serde_json::to_vec(v).unwrap_or_default() }
    }

    /// Body is already serialized JSON.
    pub fn json_raw(status: u16, body: String) -> Self {
        Response { status, content_type: CONTENT_TYPE, body: body.into_bytes() }
    }

    pub fn html(body: &str) -> Self {
        Response { status: 200, content_type: "text/html; charset=utf-8", body: body.as_bytes().to_vec() }
    }
}

pub const CONTENT_TYPE: &str = "application/json";
