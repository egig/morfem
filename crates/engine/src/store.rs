use crate::error::Result;
use serde_json::Value;

/// Bindable / returned SQL value.
#[derive(Debug, Clone, PartialEq)]
pub enum Val {
    Null,
    Int(i64),
    Real(f64),
    Text(String),
}

impl Val {
    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Val::Int(i) => Some(*i),
            // D1 hands numbers back through JS; integral floats are integers.
            Val::Real(f) if f.fract() == 0.0 => Some(*f as i64),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Val::Text(s) => Some(s),
            _ => None,
        }
    }

    /// Convert a JSON scalar into a bindable value.
    pub fn from_json(v: &Value) -> Option<Val> {
        match v {
            Value::Null => Some(Val::Null),
            Value::Bool(b) => Some(Val::Int(*b as i64)),
            Value::Number(n) => match n.as_i64() {
                Some(i) => Some(Val::Int(i)),
                None => n.as_f64().map(Val::Real),
            },
            Value::String(s) => Some(Val::Text(s.clone())),
            _ => None,
        }
    }
}

impl From<&str> for Val {
    fn from(s: &str) -> Self {
        Val::Text(s.to_string())
    }
}
impl From<String> for Val {
    fn from(s: String) -> Self {
        Val::Text(s)
    }
}
impl From<i64> for Val {
    fn from(i: i64) -> Self {
        Val::Int(i)
    }
}

#[derive(Debug, Clone)]
pub struct Stmt {
    pub sql: String,
    pub params: Vec<Val>,
}

impl Stmt {
    pub fn new(sql: impl Into<String>, params: Vec<Val>) -> Self {
        Stmt { sql: sql.into(), params }
    }
}

pub type Row = Vec<Val>;

/// Storage backend. One method on purpose: both SQLite and D1 give atomic batches,
/// and neither gives interactive transactions.
#[allow(async_fn_in_trait)]
pub trait Store {
    /// Run `stmts` in order inside one transaction.
    ///
    /// Returns, per statement, its rows (empty for statements that return none).
    /// Any statement failure rolls back the whole batch. A failure whose message
    /// contains `CONFLICT_MARK` must map to `Error::Conflict` (use `Error::from_store`).
    /// SELECT columns must have unique names (D1 returns rows as objects).
    async fn batch(&self, stmts: Vec<Stmt>) -> Result<Vec<Vec<Row>>>;
}
