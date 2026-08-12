//! Read-only SAP S/4HANA Purchase Order adapter.
//!
//! The public library surface is intentionally small so the binary and tests
//! share exactly the same endpoint validation, response normalization, and
//! error handling.

pub mod client;
#[doc(hidden)]
pub mod command;
pub mod config;
pub mod error;
pub mod models;

pub use client::SapClient;
pub use command::{MAX_SERIALIZED_OUTPUT_BYTES, execute_workflow_argv, validate_workflow_argv};
pub use config::{
    MachineProfile, load_default_machine_profile_from_cori_home, load_machine_profile,
};
pub use error::{AdapterError, Result};
pub use models::{
    PurchaseOrder, PurchaseOrderGetOutput, PurchaseOrderItem, PurchaseOrderItemsOutput,
    PurchaseOrderListOutput,
};

/// Fixed OData V4 service path. A workflow can choose an operation and typed
/// arguments, but it can never select an arbitrary HTTP target.
pub const PURCHASE_ORDER_SERVICE_ROOT: &str =
    "/sap/opu/odata4/sap/api_purchaseorder_2/srvd_a2x/sap/purchaseorder/0001/";

/// Bearer-token environment variable supported only by the optional
/// standalone diagnostic binary. Cori workflow dispatch passes its
/// owner-bound token directly to the linked library and removes any ambient
/// value from every child process.
pub const ACCESS_TOKEN_ENV: &str = "SAP_ACCESS_TOKEN";

/// Hard upper bound on records returned by one command.
pub const MAX_RECORD_LIMIT: u16 = 100;

/// Read the bearer token without ever including its contents in an error.
pub fn access_token_from_env() -> Result<String> {
    let token = std::env::var(ACCESS_TOKEN_ENV).map_err(|_| AdapterError::MissingToken)?;
    let token = token.trim();
    if token.is_empty() || token.len() > 64 * 1024 || token.contains(['\r', '\n']) {
        return Err(AdapterError::InvalidToken);
    }
    Ok(token.to_owned())
}

/// Validate the narrow purchase-order identifier accepted by command input.
/// SAP stores the value as a ten-character key; accepting only short ASCII
/// alphanumerics also makes OData expression injection impossible.
pub fn validate_purchase_order_id(value: &str) -> Result<&str> {
    if value.is_empty()
        || value.len() > 10
        || !value.bytes().all(|byte| byte.is_ascii_alphanumeric())
    {
        return Err(AdapterError::InvalidInput {
            field: "id",
            reason: "must contain 1 to 10 ASCII letters or digits",
        });
    }
    Ok(value)
}

/// Validate the bounded record limit shared by list and items operations.
pub fn validate_limit(limit: u16) -> Result<usize> {
    validate_record_limit(usize::from(limit))
}

/// Enforce the same bound at the public Rust client boundary. The binary
/// parses limits as `u16`, while library callers can otherwise pass an
/// unbounded `usize` directly to [`SapClient`].
pub(crate) fn validate_record_limit(limit: usize) -> Result<usize> {
    if limit == 0 || limit > usize::from(MAX_RECORD_LIMIT) {
        return Err(AdapterError::InvalidInput {
            field: "limit",
            reason: "must be between 1 and 100",
        });
    }
    Ok(limit)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn purchase_order_ids_are_narrow() {
        assert_eq!(validate_purchase_order_id("4500001234"), Ok("4500001234"));
        assert_eq!(validate_purchase_order_id("AB12"), Ok("AB12"));
        assert!(validate_purchase_order_id("4500' or true").is_err());
        assert!(validate_purchase_order_id("").is_err());
        assert!(validate_purchase_order_id("12345678901").is_err());
    }

    #[test]
    fn result_limit_is_bounded() {
        assert_eq!(validate_limit(1), Ok(1));
        assert_eq!(validate_limit(100), Ok(100));
        assert!(validate_limit(0).is_err());
        assert!(validate_limit(101).is_err());
    }
}
