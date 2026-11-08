use serde_json::{json, Value};

/// Marker in the SQLite constraint name used by commit guards.
pub const CONFLICT_MARK: &str = "morfem_conflict";

#[derive(Debug, Clone, PartialEq)]
pub enum Error {
    BadRequest(String),
    Unauthorized,
    Forbidden,
    NotFound(String),
    MethodNotAllowed,
    Conflict(String),
    PreconditionFailed(String),
    PayloadTooLarge,
    Internal(String),
}

impl Error {
    /// Map a store failure message to an error. Guard failures become `Conflict`.
    pub fn from_store(msg: impl Into<String>) -> Self {
        let msg = msg.into();
        if msg.contains(CONFLICT_MARK) {
            Error::Conflict("Document update conflict.".into())
        } else {
            Error::Internal(msg)
        }
    }

    pub fn status(&self) -> u16 {
        match self {
            Error::BadRequest(_) => 400,
            Error::Unauthorized => 401,
            Error::Forbidden => 403,
            Error::NotFound(_) => 404,
            Error::MethodNotAllowed => 405,
            Error::Conflict(_) => 409,
            Error::PreconditionFailed(_) => 412,
            Error::PayloadTooLarge => 413,
            Error::Internal(_) => 500,
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            Error::BadRequest(_) => "bad_request",
            Error::Unauthorized => "unauthorized",
            Error::Forbidden => "forbidden",
            Error::NotFound(_) => "not_found",
            Error::MethodNotAllowed => "method_not_allowed",
            Error::Conflict(_) => "conflict",
            Error::PreconditionFailed(_) => "file_exists",
            Error::PayloadTooLarge => "too_large",
            Error::Internal(_) => "internal_error",
        }
    }

    pub fn reason(&self) -> String {
        match self {
            Error::BadRequest(r)
            | Error::NotFound(r)
            | Error::Conflict(r)
            | Error::PreconditionFailed(r) => r.clone(),
            Error::Unauthorized => "Valid bearer token required.".into(),
            Error::Forbidden => "Token lacks access to this resource.".into(),
            Error::MethodNotAllowed => "Method not allowed.".into(),
            Error::PayloadTooLarge => "Request body too large.".into(),
            // Do not leak store internals to clients.
            Error::Internal(_) => "Internal error.".into(),
        }
    }

    pub fn to_json(&self) -> Value {
        json!({ "error": self.name(), "reason": self.reason() })
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Internal(m) => write!(f, "internal: {m}"),
            e => write!(f, "{}: {}", e.name(), e.reason()),
        }
    }
}

impl std::error::Error for Error {}

pub type Result<T> = std::result::Result<T, Error>;
