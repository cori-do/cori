//! IPC error type — discriminated union that the frontend pattern-matches
//! on via `err.code` (see §7.1 of the implementation guide).

use serde::{Serialize, Serializer};
use serde_json::{Value, json};

/// Mirror of `cori_run::ConsentRequired` rendered for the wire. We don't
/// derive `Serialize` on the upstream type to keep `cori-run` framework-
/// agnostic; this struct stays in sync with it by hand.
#[derive(Debug, Clone, Serialize)]
pub struct ConsentDetails {
    pub host: String,
    pub repo: String,
    pub subpath: String,
    pub ref_str: String,
    pub sha: String,
}

impl From<cori_run::ConsentRequired> for ConsentDetails {
    fn from(c: cori_run::ConsentRequired) -> Self {
        Self {
            host: c.spec.host,
            repo: c.spec.repo,
            subpath: c.spec.subpath,
            ref_str: c.spec.ref_str,
            sha: c.sha,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum IpcError {
    #[error("consent required for {}/{}@{}", .0.host, .0.repo, .0.ref_str)]
    ConsentRequired(ConsentDetails),
    #[allow(dead_code)]
    #[error("missing capability: {0}")]
    MissingCapability(String),
    #[allow(dead_code)]
    #[error("needs login: {0}")]
    NeedsLogin(String),
    #[allow(dead_code)]
    #[error("Temporal unavailable: {0}")]
    NoTemporal(String),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("bad request: {0}")]
    BadRequest(String),
    #[error(transparent)]
    Internal(#[from] anyhow::Error),
}

impl Serialize for IpcError {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        use serde::ser::SerializeStruct;

        let (code, message, details): (&str, String, Value) = match self {
            IpcError::ConsentRequired(d) => (
                "consent_required",
                self.to_string(),
                serde_json::to_value(d).unwrap_or(Value::Null),
            ),
            IpcError::MissingCapability(s) => (
                "missing_capability",
                self.to_string(),
                json!({ "capability": s }),
            ),
            IpcError::NeedsLogin(s) => {
                ("needs_login", self.to_string(), json!({ "capability": s }))
            }
            IpcError::NoTemporal(s) => ("no_temporal", self.to_string(), json!({ "reason": s })),
            IpcError::NotFound(s) => ("not_found", self.to_string(), json!({ "resource": s })),
            IpcError::BadRequest(s) => ("bad_request", self.to_string(), json!({ "reason": s })),
            IpcError::Internal(e) => ("internal", format!("{e:#}"), Value::Null),
        };

        let mut s = serializer.serialize_struct("IpcError", 3)?;
        s.serialize_field("code", code)?;
        s.serialize_field("message", &message)?;
        s.serialize_field("details", &details)?;
        s.end()
    }
}

pub type IpcResult<T> = std::result::Result<T, IpcError>;
