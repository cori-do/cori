use std::io::Write as _;
use std::path::PathBuf;

use serde::Serialize;
use thiserror::Error;

pub type Result<T> = std::result::Result<T, AdapterError>;

/// Errors deliberately contain only safe, bounded context. In particular,
/// transport errors do not retain reqwest's URL-bearing display text and HTTP
/// errors do not retain the SAP response body.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum AdapterError {
    #[error("SAP machine configuration was not found")]
    ConfigMissing { path: PathBuf },

    #[error("SAP machine configuration is not trusted")]
    ConfigUnsafe { reason: &'static str },

    #[error("SAP machine configuration is malformed")]
    ConfigMalformed,

    #[error("SAP profile name is invalid")]
    InvalidProfileName,

    #[error("SAP profile `{profile}` is not configured")]
    ProfileMissing { profile: String },

    #[error("SAP profile endpoint is invalid")]
    InvalidEndpoint { reason: &'static str },

    #[error("SAP client id is invalid")]
    InvalidSapClient,

    #[error("SAP_ACCESS_TOKEN is not configured")]
    MissingToken,

    #[error("SAP_ACCESS_TOKEN is invalid")]
    InvalidToken,

    #[error("invalid `{field}`: {reason}")]
    InvalidInput {
        field: &'static str,
        reason: &'static str,
    },

    #[error("SAP profile selection is not available inside workflows")]
    WorkflowProfileDenied,

    #[error("SAP request could not be completed")]
    Transport,

    #[error("SAP returned HTTP {status}")]
    HttpStatus {
        status: u16,
        sap_code: Option<String>,
    },

    #[error("SAP response exceeded the {max_bytes}-byte safety limit")]
    ResponseTooLarge { max_bytes: usize },

    #[error("SAP returned an invalid response: {reason}")]
    InvalidResponse { reason: &'static str },

    #[error("purchase order `{id}` was not found")]
    PurchaseOrderNotFound { id: String },

    #[error("SAP pagination exceeded the safety limit")]
    PaginationLimit,

    #[error("could not serialize adapter output")]
    OutputSerialization,

    #[error("serialized adapter output exceeded the {max_bytes}-byte safety limit")]
    OutputTooLarge { max_bytes: usize },
}

#[derive(Serialize)]
struct ErrorEnvelope<'a> {
    error: ErrorBody<'a>,
}

#[derive(Serialize)]
struct ErrorBody<'a> {
    code: &'a str,
    message: String,
    retryable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    sap_code: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    hint: Option<&'a str>,
}

impl AdapterError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::ConfigMissing { .. } => "config_missing",
            Self::ConfigUnsafe { .. } => "config_unsafe",
            Self::ConfigMalformed => "config_malformed",
            Self::InvalidProfileName => "invalid_profile_name",
            Self::ProfileMissing { .. } => "profile_missing",
            Self::InvalidEndpoint { .. } => "invalid_endpoint",
            Self::InvalidSapClient => "invalid_sap_client",
            Self::MissingToken => "authentication_required",
            Self::InvalidToken => "invalid_bearer_token",
            Self::InvalidInput { .. } => "invalid_input",
            Self::WorkflowProfileDenied => "workflow_profile_denied",
            Self::Transport => "transport_error",
            Self::HttpStatus {
                status: 401 | 403, ..
            } => "authentication_failed",
            Self::HttpStatus { .. } => "sap_http_error",
            Self::ResponseTooLarge { .. } => "response_too_large",
            Self::InvalidResponse { .. } => "invalid_sap_response",
            Self::PurchaseOrderNotFound { .. } => "purchase_order_not_found",
            Self::PaginationLimit => "pagination_limit",
            Self::OutputSerialization => "output_serialization",
            Self::OutputTooLarge { .. } => "output_too_large",
        }
    }

    /// Diagnostic classification for the optional standalone binary. Cori's
    /// linked workflow broker deliberately maps every non-auth SAP failure to
    /// a non-retryable step error in v1.
    pub fn retryable(&self) -> bool {
        match self {
            Self::Transport => true,
            Self::HttpStatus { status, .. } => {
                matches!(*status, 408 | 425 | 429 | 500..=599)
            }
            _ => false,
        }
    }

    /// Standalone diagnostic exit code; it does not configure Temporal retry
    /// behavior for linked workflow execution.
    pub fn exit_code(&self) -> i32 {
        if self.retryable() { 3 } else { 2 }
    }

    fn hint(&self) -> Option<&'static str> {
        match self {
            Self::ConfigMissing { .. } => {
                Some("create $CORI_HOME/sap.toml (or ~/.cori/sap.toml) with a default profile")
            }
            Self::ConfigMalformed => Some(
                "sap.toml accepts only default_profile and profiles.<name>.{base_url,sap_client}",
            ),
            Self::MissingToken => Some(
                "run `cori login cori-sap`; standalone adapter calls may provide SAP_ACCESS_TOKEN only to that process",
            ),
            Self::HttpStatus {
                status: 401 | 403, ..
            } => Some("run `cori login cori-sap` and verify the SAP authorization"),
            _ => None,
        }
    }

    fn sap_code(&self) -> Option<&str> {
        match self {
            Self::HttpStatus { sap_code, .. } => sap_code.as_deref(),
            _ => None,
        }
    }

    /// Emit the stable machine-readable error contract on stderr. A fallback
    /// is retained for the unlikely case that stderr itself cannot accept the
    /// JSON serialization.
    pub fn write_json_stderr(&self) {
        let envelope = ErrorEnvelope {
            error: ErrorBody {
                code: self.code(),
                message: self.to_string(),
                retryable: self.retryable(),
                sap_code: self.sap_code(),
                hint: self.hint(),
            },
        };
        let mut stderr = std::io::stderr().lock();
        if serde_json::to_writer(&mut stderr, &envelope).is_err() {
            let _ = stderr.write_all(b"{\"error\":{\"code\":\"output_serialization\",\"message\":\"could not serialize adapter error\",\"retryable\":false}}\n");
            return;
        }
        let _ = stderr.write_all(b"\n");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retry_taxonomy_is_conservative() {
        assert!(AdapterError::Transport.retryable());
        assert!(
            AdapterError::HttpStatus {
                status: 429,
                sap_code: None,
            }
            .retryable()
        );
        assert!(
            AdapterError::HttpStatus {
                status: 503,
                sap_code: None,
            }
            .retryable()
        );
        assert!(
            !AdapterError::HttpStatus {
                status: 401,
                sap_code: None,
            }
            .retryable()
        );
        assert!(
            !AdapterError::InvalidInput {
                field: "id",
                reason: "bad",
            }
            .retryable()
        );
    }

    #[test]
    fn errors_never_render_sap_response_content() {
        let error = AdapterError::HttpStatus {
            status: 500,
            sap_code: Some("MM_PUR/001".to_string()),
        };
        assert_eq!(error.to_string(), "SAP returned HTTP 500");
    }
}
