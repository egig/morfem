//! Mango-style selector -> SQL `WHERE` fragment over `json_extract(body, ...)`.
//!
//! Supported: implicit equality, nested subfields, `$eq $ne $gt $gte $lt $lte $in $nin $exists`,
//! `$and $or $nor $not`. Field paths are inlined (validated); values are always bound.

use crate::error::{Error, Result};
use crate::store::Val;
use crate::validate::{field_path, json_expr};
use serde_json::{Map, Value};

pub struct Compiled {
    pub sql: String,
    pub params: Vec<Val>,
    /// Field paths the selector touches (for index hints).
    pub fields: Vec<String>,
}

pub fn compile(selector: &Value) -> Result<Compiled> {
    let obj = selector
        .as_object()
        .ok_or_else(|| Error::BadRequest("selector must be an object".into()))?;
    let mut c = Compiler { params: vec![], fields: vec![], depth: 0 };
    let sql = c.object(obj, "")?;
    Ok(Compiled { sql, params: c.params, fields: c.fields })
}

struct Compiler {
    params: Vec<Val>,
    fields: Vec<String>,
    depth: usize,
}

fn bad<T>(msg: impl Into<String>) -> Result<T> {
    Err(Error::BadRequest(msg.into()))
}

impl Compiler {
    fn object(&mut self, obj: &Map<String, Value>, prefix: &str) -> Result<String> {
        self.depth += 1;
        if self.depth > 32 {
            return bad("selector nested too deeply");
        }
        let mut parts = vec![];
        for (key, val) in obj {
            if let Some(op) = key.strip_prefix('$') {
                parts.push(self.combinator(op, val, prefix)?);
            } else {
                let path = if prefix.is_empty() { key.clone() } else { format!("{prefix}.{key}") };
                parts.push(self.field(&path, val)?);
            }
        }
        self.depth -= 1;
        Ok(if parts.is_empty() { "1".into() } else { format!("({})", parts.join(" AND ")) })
    }

    fn combinator(&mut self, op: &str, val: &Value, prefix: &str) -> Result<String> {
        match op {
            "and" | "or" | "nor" => {
                let arr = val.as_array().ok_or_else(|| Error::BadRequest(format!("${op} needs an array")))?;
                if arr.is_empty() {
                    return bad(format!("${op} needs a non-empty array"));
                }
                let mut parts = vec![];
                let seen = self.fields.len();
                for item in arr {
                    let o = item.as_object().ok_or_else(|| Error::BadRequest(format!("${op} items must be objects")))?;
                    parts.push(self.object(o, prefix)?);
                }
                // A disjunction cannot be served by one index; do not claim coverage.
                if op != "and" {
                    self.fields.truncate(seen);
                }
                Ok(match op {
                    "and" => format!("({})", parts.join(" AND ")),
                    "or" => format!("({})", parts.join(" OR ")),
                    _ => format!("NOT ({})", parts.join(" OR ")),
                })
            }
            "not" => {
                let o = val.as_object().ok_or_else(|| Error::BadRequest("$not needs an object".into()))?;
                let seen = self.fields.len();
                let sql = self.object(o, prefix)?;
                self.fields.truncate(seen);
                Ok(format!("NOT {sql}"))
            }
            other => bad(format!("unknown top-level operator ${other}")),
        }
    }

    fn field(&mut self, path: &str, val: &Value) -> Result<String> {
        field_path(path)?;
        match val {
            Value::Object(o) if !o.is_empty() && o.keys().all(|k| k.starts_with('$')) => self.ops(path, o),
            // Plain object: subfield match.
            Value::Object(o) => self.object(o, path),
            _ => {
                self.fields.push(path.to_string());
                self.cmp(path, "$eq", val)
            }
        }
    }

    fn ops(&mut self, path: &str, ops: &Map<String, Value>) -> Result<String> {
        let mut parts = vec![];
        for (op, val) in ops {
            parts.push(match op.as_str() {
                "$eq" | "$ne" | "$gt" | "$gte" | "$lt" | "$lte" => {
                    if op != "$ne" {
                        self.fields.push(path.to_string());
                    }
                    self.cmp(path, op, val)?
                }
                "$in" | "$nin" => {
                    if op == "$in" {
                        self.fields.push(path.to_string());
                    }
                    let arr = val.as_array().ok_or_else(|| Error::BadRequest(format!("{op} needs an array")))?;
                    if arr.len() > 100 {
                        return bad(format!("{op} supports at most 100 values"));
                    }
                    if arr.is_empty() {
                        (if op == "$in" { "0" } else { "1" }).to_string()
                    } else {
                        let mut ph = vec![];
                        for v in arr {
                            self.params.push(scalar(v)?);
                            ph.push("?");
                        }
                        let kw = if op == "$in" { "IN" } else { "NOT IN" };
                        format!("{} {kw} ({})", json_expr(path), ph.join(","))
                    }
                }
                "$exists" => {
                    let b = val.as_bool().ok_or_else(|| Error::BadRequest("$exists needs a boolean".into()))?;
                    let not = if b { "NOT " } else { "" };
                    format!("json_type(body,'$.{path}') IS {not}NULL")
                }
                "$not" => {
                    let o = val.as_object().ok_or_else(|| Error::BadRequest("$not needs an object".into()))?;
                    let seen = self.fields.len();
                    let sql = self.ops(path, o)?;
                    self.fields.truncate(seen);
                    format!("NOT ({sql})")
                }
                other => return bad(format!("unsupported operator {other}")),
            });
        }
        Ok(format!("({})", parts.join(" AND ")))
    }

    fn cmp(&mut self, path: &str, op: &str, val: &Value) -> Result<String> {
        let expr = json_expr(path);
        if val.is_null() {
            return match op {
                "$eq" => Ok(format!("{expr} IS NULL")),
                "$ne" => Ok(format!("{expr} IS NOT NULL")),
                _ => bad(format!("{op} does not accept null")),
            };
        }
        self.params.push(scalar(val)?);
        let sql_op = match op {
            "$eq" => "=",
            "$ne" => "!=",
            "$gt" => ">",
            "$gte" => ">=",
            "$lt" => "<",
            _ => "<=",
        };
        Ok(format!("{expr} {sql_op} ?"))
    }
}

fn scalar(v: &Value) -> Result<Val> {
    match v {
        Value::Array(_) | Value::Object(_) => bad("selector values must be scalars (string, number, bool, null)"),
        _ => Val::from_json(v).ok_or_else(|| Error::BadRequest("bad selector value".into())),
    }
}

/// Parse a Mango `sort` array: `["a", {"b":"desc"}]` -> `ORDER BY` fragment (no keyword).
pub fn order_by(sort: &Value) -> Result<String> {
    let arr = sort.as_array().ok_or_else(|| Error::BadRequest("sort must be an array".into()))?;
    let mut parts = vec![];
    for item in arr {
        let (field, dir) = match item {
            Value::String(s) => (s.as_str(), "ASC"),
            Value::Object(o) if o.len() == 1 => {
                let (k, v) = o.iter().next().unwrap();
                match v.as_str() {
                    Some("asc") => (k.as_str(), "ASC"),
                    Some("desc") => (k.as_str(), "DESC"),
                    _ => return bad("sort direction must be \"asc\" or \"desc\""),
                }
            }
            _ => return bad("sort items are \"field\" or {\"field\":\"asc|desc\"}"),
        };
        field_path(field)?;
        parts.push(format!("{} {dir}", json_expr(field)));
    }
    Ok(parts.join(","))
}
