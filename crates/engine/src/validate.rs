use crate::error::{Error, Result};

pub const MAX_DOC_BYTES: usize = 1_000_000;

/// Db names are inlined into SQL (partial indexes need a literal), so they are strict.
pub fn db_name(s: &str) -> Result<()> {
    let ok = !s.is_empty()
        && s.len() <= 64
        && s.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-')
        && !s.starts_with(['_', '-']);
    if ok {
        Ok(())
    } else {
        Err(Error::BadRequest(
            "Invalid database name: use [a-z0-9_-], max 64 chars, no leading _ or -.".into(),
        ))
    }
}

pub fn doc_id(s: &str) -> Result<()> {
    if s.is_empty() || s.len() > 256 || s.starts_with('_') || s.chars().any(|c| c.is_control()) {
        return Err(Error::BadRequest(
            "Invalid document id: 1-256 chars, no control chars, must not start with _.".into(),
        ));
    }
    Ok(())
}

/// Dotted JSON path like `a.b_c`. Inlined into SQL, so strict.
pub fn field_path(s: &str) -> Result<()> {
    let ok = !s.is_empty()
        && s.len() <= 128
        && s.split('.').all(|seg| {
            !seg.is_empty() && seg.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
        });
    if ok {
        Ok(())
    } else {
        Err(Error::BadRequest(format!(
            "Invalid field '{s}': dotted path of [A-Za-z0-9_] segments."
        )))
    }
}

pub fn index_name(s: &str) -> Result<()> {
    if !s.is_empty() && s.len() <= 64 && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') {
        Ok(())
    } else {
        Err(Error::BadRequest("Invalid index name: [A-Za-z0-9_], max 64 chars.".into()))
    }
}

/// `json_extract(body,'$.a.b')`. Must be textually identical in indexes and queries.
pub fn json_expr(path: &str) -> String {
    format!("json_extract(body,'$.{path}')")
}
