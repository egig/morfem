use serde::Deserialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Access {
    Read,
    Write,
    Admin,
}

#[derive(Debug, Clone)]
pub struct Principal {
    pub access: Access,
    /// `None` = every database.
    pub dbs: Option<Vec<String>>,
}

impl Principal {
    pub fn can(&self, db: &str, need: Access) -> bool {
        self.access >= need && self.in_scope(db)
    }

    pub fn in_scope(&self, db: &str) -> bool {
        match &self.dbs {
            None => true,
            Some(list) => list.iter().any(|d| d == db),
        }
    }
}

pub trait Authenticator {
    /// `bearer` is the token from `Authorization: Bearer <token>`, if any.
    fn authenticate(&self, bearer: Option<&str>) -> Option<Principal>;
}

/// Everyone is admin. For local development only.
pub struct Open;

impl Authenticator for Open {
    fn authenticate(&self, _: Option<&str>) -> Option<Principal> {
        Some(Principal { access: Access::Admin, dbs: None })
    }
}

#[derive(Debug, Deserialize)]
struct KeyEntry {
    key: String,
    access: Access,
    #[serde(default)]
    dbs: Option<Vec<String>>,
}

/// Static API keys, e.g. from a Worker secret:
/// `[{"key":"s3cret","access":"admin"},{"key":"k2","access":"read","dbs":["notes"]}]`
pub struct StaticKeys {
    keys: Vec<KeyEntry>,
}

impl StaticKeys {
    pub fn from_json(s: &str) -> Result<Self, String> {
        let keys: Vec<KeyEntry> = serde_json::from_str(s).map_err(|e| format!("bad keys json: {e}"))?;
        if keys.iter().any(|k| k.key.len() < 8) {
            return Err("keys must be at least 8 characters".into());
        }
        Ok(StaticKeys { keys })
    }

    /// Denies everything. Used when no keys are configured.
    pub fn deny_all() -> Self {
        StaticKeys { keys: vec![] }
    }
}

fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    let mut diff = (a.len() ^ b.len()) as u8;
    for i in 0..a.len().max(b.len()) {
        diff |= a.get(i).copied().unwrap_or(0) ^ b.get(i).copied().unwrap_or(0);
    }
    diff == 0
}

impl Authenticator for StaticKeys {
    fn authenticate(&self, bearer: Option<&str>) -> Option<Principal> {
        let token = bearer?;
        let mut found = None;
        // No early exit: compare against every key.
        for k in &self.keys {
            if ct_eq(k.key.as_bytes(), token.as_bytes()) {
                found = Some(Principal {
                    access: k.access,
                    dbs: k.dbs.clone().filter(|d| !d.iter().any(|x| x == "*")),
                });
            }
        }
        found
    }
}
